//! Enrollment lifecycle and phase history (ADR 0008; docs/domain-model.md
//! `Enrollment`, `PhaseTransition`).
//!
//! Both histories are append-only event streams; migration 0006 makes the
//! database refuse edits, an unmediated version repoint, and phase
//! references outside the enrollment's pinned version. This module owns
//! the state derived from the streams (status, current phase, paused) and
//! the service-level rules the schema cannot express: the transition
//! graph, the pause state machine, required reasons, and effective-order
//! append.
//!
//! Phase events carry an effective instant (when the transition took
//! effect) and a recorded instant (when it was written). Backfill is
//! honest — both instants are kept and ordering uses the effective one —
//! but events append in effective order: an event that would land between
//! two already-recorded events is refused rather than silently reordering
//! history.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqliteConnection, SqlitePool};
use time::OffsetDateTime;

use crate::assignments;
use crate::audit::{self, EventKind, Subject};
use crate::capabilities::{self, Capability};

/// Enrollment status, derived from the event stream and never stored
/// beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnrollmentStatus {
    Active,
    Withdrawn,
    Completed,
}

/// Enrollment lifecycle event vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnrollmentEventKind {
    VersionChange,
    Withdraw,
    Complete,
    Reinstate,
}

impl EnrollmentEventKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::VersionChange => "version_change",
            Self::Withdraw => "withdraw",
            Self::Complete => "complete",
            Self::Reinstate => "reinstate",
        }
    }
}

/// Phase history event vocabulary (docs/domain-model.md `PhaseTransition`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseEventKind {
    Advance,
    Return,
    Restart,
    Pause,
    Resume,
    Complete,
}

impl PhaseEventKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Advance => "advance",
            Self::Return => "return",
            Self::Restart => "restart",
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Complete => "complete",
        }
    }
}

/// Why a lifecycle operation was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum LifecycleRefusal {
    CapabilityRequired,
    NoSuchEnrollment,
    /// The enrollment is withdrawn or completed.
    NotActive,
    /// Reinstate targets a withdrawn or completed enrollment.
    AlreadyActive,
    /// This event kind requires a non-empty reason.
    ReasonRequired,
    NoSuchVersion,
    /// Enrollments pin published versions, never drafts.
    NotPublished,
    /// A version change must target a different version.
    SameVersion,
    /// A version change stays within the enrollment's continuing program.
    DifferentProgram,
    /// The trainee already has an enrollment pinning the target version.
    TargetAlreadyEnrolled,
    /// The phase does not belong to the enrollment's pinned version.
    NoSuchPhase,
    /// The pinned transition graph has no matching edge.
    TransitionNotAllowed,
    /// Phase-changing events other than entry need a current phase.
    NoCurrentPhase,
    /// Pause requires an unpaused enrollment.
    AlreadyPaused,
    /// Resume requires a paused enrollment.
    NotPaused,
    /// Advance, return, restart, and complete are refused while paused.
    Paused,
    /// Events append in effective order; interleaving is refused.
    OutOfOrder,
    /// Effective instants never postdate the recording instant.
    EffectiveInFuture,
}

/// One enrollment lifecycle event, with presentation fields resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnrollmentEvent {
    pub id: i64,
    pub kind: String,
    pub occurred_at: i64,
    pub actor_user_id: Option<i64>,
    pub actor_display_name: Option<String>,
    pub reason: String,
    pub from_program_version_id: Option<i64>,
    pub from_version_number: Option<i64>,
    pub from_version_label: Option<String>,
    pub to_program_version_id: Option<i64>,
    pub to_version_number: Option<i64>,
    pub to_version_label: Option<String>,
}

/// One phase history event, with presentation fields resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PhaseEvent {
    pub id: i64,
    pub kind: String,
    pub from_phase_id: Option<i64>,
    pub from_phase_name: Option<String>,
    pub to_phase_id: Option<i64>,
    pub to_phase_name: Option<String>,
    pub effective_at: i64,
    pub recorded_at: i64,
    pub actor_user_id: Option<i64>,
    pub actor_display_name: Option<String>,
    pub reason: String,
}

/// A phase of the pinned version, for the interface's target pickers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PhaseRef {
    pub id: i64,
    pub name: String,
    pub presentation_number: i64,
}

/// An allowed-transition edge of the pinned version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TransitionRef {
    pub from_phase_id: i64,
    pub to_phase_id: i64,
    pub kind: String,
}

/// Everything the enrollment page shows: the pin, derived state, both
/// event streams, assignments, and the pinned version's phase vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnrollmentDetail {
    pub enrollment_id: i64,
    pub trainee_user_id: i64,
    pub trainee_username: String,
    pub trainee_display_name: String,
    pub enrolled_at: i64,
    pub program_id: i64,
    pub program_version_id: i64,
    pub program_name: String,
    pub version_number: i64,
    pub version_label: String,
    pub status: EnrollmentStatus,
    pub paused: bool,
    pub current_phase_id: Option<i64>,
    pub current_phase_name: Option<String>,
    pub events: Vec<EnrollmentEvent>,
    pub phase_events: Vec<PhaseEvent>,
    pub assignments: Vec<assignments::Assignment>,
    pub phases: Vec<PhaseRef>,
    pub transitions: Vec<TransitionRef>,
}

/// The enrollment's status; `None` when the enrollment does not exist.
pub async fn status(
    conn: &mut SqliteConnection,
    enrollment_id: i64,
) -> Result<Option<EnrollmentStatus>> {
    let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM enrollment WHERE id = ?1")
        .bind(enrollment_id)
        .fetch_optional(&mut *conn)
        .await
        .context("checking enrollment")?;
    if exists.is_none() {
        return Ok(None);
    }
    let latest: Option<String> = sqlx::query_scalar(
        "SELECT kind FROM enrollment_event
         WHERE enrollment_id = ?1 AND kind IN ('withdraw', 'complete', 'reinstate')
         ORDER BY id DESC LIMIT 1",
    )
    .bind(enrollment_id)
    .fetch_optional(&mut *conn)
    .await
    .context("deriving enrollment status")?;
    Ok(Some(match latest.as_deref() {
        Some("withdraw") => EnrollmentStatus::Withdrawn,
        Some("complete") => EnrollmentStatus::Completed,
        _ => EnrollmentStatus::Active,
    }))
}

/// The current phase (id, name) under the enrollment's current pin epoch.
/// `None` when the trainee has not entered the epoch's graph — before any
/// phase event, in a phaseless program, or after a version change (every
/// version change opens a fresh epoch, even back to a previously pinned
/// version).
async fn current_phase(
    conn: &mut SqliteConnection,
    enrollment_id: i64,
) -> Result<Option<(i64, String)>> {
    let row = sqlx::query(
        "SELECT pe.to_phase_id AS phase_id, p.name
         FROM phase_event pe
         JOIN phase p ON p.id = pe.to_phase_id
         WHERE pe.enrollment_id = ?1 AND pe.kind IN ('advance', 'return', 'restart')
           AND pe.version_change_event_id IS (
               SELECT MAX(id) FROM enrollment_event
               WHERE enrollment_id = ?1 AND kind = 'version_change')
         ORDER BY pe.effective_at DESC, pe.id DESC LIMIT 1",
    )
    .bind(enrollment_id)
    .fetch_optional(&mut *conn)
    .await
    .context("deriving current phase")?;
    Ok(row.map(|row| (row.get("phase_id"), row.get("name"))))
}

/// Whether the enrollment is paused under its current pin epoch. A pause
/// recorded under an earlier epoch does not carry forward, matching how
/// the current phase resets.
async fn is_paused(conn: &mut SqliteConnection, enrollment_id: i64) -> Result<bool> {
    let latest: Option<String> = sqlx::query_scalar(
        "SELECT kind FROM phase_event
         WHERE enrollment_id = ?1 AND kind IN ('pause', 'resume')
           AND version_change_event_id IS (
               SELECT MAX(id) FROM enrollment_event
               WHERE enrollment_id = ?1 AND kind = 'version_change')
         ORDER BY effective_at DESC, id DESC LIMIT 1",
    )
    .bind(enrollment_id)
    .fetch_optional(&mut *conn)
    .await
    .context("deriving pause state")?;
    Ok(latest.as_deref() == Some("pause"))
}

/// Validates a version-change target: it must exist, be published, stay
/// within the continuing program (changing programs is a new
/// enrollment), and not collide with another enrollment's pin. Returns
/// the refusal, or `None` when the change may proceed.
async fn version_change_refusal(
    tx: &mut SqliteConnection,
    enrollment_id: i64,
    trainee: i64,
    from: i64,
    to: i64,
) -> Result<Option<LifecycleRefusal>> {
    if to == from {
        return Ok(Some(LifecycleRefusal::SameVersion));
    }
    let target = sqlx::query("SELECT published_at, program_id FROM program_version WHERE id = ?1")
        .bind(to)
        .fetch_optional(&mut *tx)
        .await
        .context("checking version")?;
    let Some(target) = target else {
        return Ok(Some(LifecycleRefusal::NoSuchVersion));
    };
    let published: Option<i64> = target.get("published_at");
    if published.is_none() {
        return Ok(Some(LifecycleRefusal::NotPublished));
    }
    let current_program: i64 =
        sqlx::query_scalar("SELECT program_id FROM program_version WHERE id = ?1")
            .bind(from)
            .fetch_one(&mut *tx)
            .await
            .context("reading pinned program")?;
    let target_program: i64 = target.get("program_id");
    if target_program != current_program {
        return Ok(Some(LifecycleRefusal::DifferentProgram));
    }
    let duplicate: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM enrollment
         WHERE user_id = ?1 AND program_version_id = ?2 AND id != ?3",
    )
    .bind(trainee)
    .bind(to)
    .bind(enrollment_id)
    .fetch_optional(&mut *tx)
    .await
    .context("checking target enrollment")?;
    if duplicate.is_some() {
        return Ok(Some(LifecycleRefusal::TargetAlreadyEnrolled));
    }
    Ok(None)
}

/// Records one enrollment lifecycle event, gated on `assign_training`.
/// A version change also repoints the pin in the same transaction; the
/// database refuses the repoint unless the event mediates it.
pub async fn record_enrollment_event(
    pool: &SqlitePool,
    actor_user_id: i64,
    enrollment_id: i64,
    kind: EnrollmentEventKind,
    reason: &str,
    to_version_id: Option<i64>,
) -> Result<std::result::Result<i64, LifecycleRefusal>> {
    if !capabilities::user_has(pool, actor_user_id, Capability::AssignTraining).await? {
        return Ok(Err(LifecycleRefusal::CapabilityRequired));
    }
    let reason = reason.trim();
    let reason_required = !matches!(kind, EnrollmentEventKind::Complete);
    if reason_required && reason.is_empty() {
        return Ok(Err(LifecycleRefusal::ReasonRequired));
    }

    let mut tx = pool.begin().await.context("starting lifecycle event")?;
    let Some(current_status) = status(&mut tx, enrollment_id).await? else {
        return Ok(Err(LifecycleRefusal::NoSuchEnrollment));
    };
    match kind {
        EnrollmentEventKind::Reinstate => {
            if current_status == EnrollmentStatus::Active {
                return Ok(Err(LifecycleRefusal::AlreadyActive));
            }
        }
        _ => {
            if current_status != EnrollmentStatus::Active {
                return Ok(Err(LifecycleRefusal::NotActive));
            }
        }
    }

    let trainee: i64 = sqlx::query_scalar("SELECT user_id FROM enrollment WHERE id = ?1")
        .bind(enrollment_id)
        .fetch_one(&mut *tx)
        .await
        .context("reading enrollment")?;
    let now = OffsetDateTime::now_utc().unix_timestamp();

    let (from_version, to_version, audit_kind) = match kind {
        EnrollmentEventKind::VersionChange => {
            let from: i64 =
                sqlx::query_scalar("SELECT program_version_id FROM enrollment WHERE id = ?1")
                    .bind(enrollment_id)
                    .fetch_one(&mut *tx)
                    .await
                    .context("reading enrollment pin")?;
            let Some(to) = to_version_id else {
                return Ok(Err(LifecycleRefusal::NoSuchVersion));
            };
            if let Some(refusal) =
                version_change_refusal(&mut tx, enrollment_id, trainee, from, to).await?
            {
                return Ok(Err(refusal));
            }
            (Some(from), Some(to), EventKind::EnrollmentVersionChanged)
        }
        EnrollmentEventKind::Withdraw => (None, None, EventKind::EnrollmentWithdrawn),
        EnrollmentEventKind::Complete => (None, None, EventKind::EnrollmentCompleted),
        EnrollmentEventKind::Reinstate => (None, None, EventKind::EnrollmentReinstated),
    };

    let result = sqlx::query(
        "INSERT INTO enrollment_event
             (enrollment_id, kind, occurred_at, actor_user_id, reason,
              from_program_version_id, to_program_version_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(enrollment_id)
    .bind(kind.as_str())
    .bind(now)
    .bind(actor_user_id)
    .bind(reason)
    .bind(from_version)
    .bind(to_version)
    .execute(&mut *tx)
    .await
    .context("recording enrollment event")?;
    let event_id = result.last_insert_rowid();

    if let Some(to) = to_version {
        sqlx::query("UPDATE enrollment SET program_version_id = ?1 WHERE id = ?2")
            .bind(to)
            .bind(enrollment_id)
            .execute(&mut *tx)
            .await
            .context("repointing enrollment pin")?;
    }

    audit::record_for_subject(
        &mut *tx,
        audit_kind,
        Some(actor_user_id),
        Some(trainee),
        Subject::Enrollment(enrollment_id),
    )
    .await?;
    tx.commit().await.context("committing lifecycle event")?;
    Ok(Ok(event_id))
}

/// Records one phase history event, gated on `assign_training`.
///
/// `effective_at` defaults to now; an earlier instant is honest backfill
/// as long as it does not interleave before an already-recorded event.
/// Phase-changing kinds are validated against the pinned version's
/// transition graph: advance follows advance or skip edges, return follows
/// remediation edges, restart follows restart edges, and entry (an advance
/// with no current phase) may target any phase of the pinned version.
#[allow(clippy::too_many_lines)]
pub async fn record_phase_event(
    pool: &SqlitePool,
    actor_user_id: i64,
    enrollment_id: i64,
    kind: PhaseEventKind,
    to_phase_id: Option<i64>,
    effective_at: Option<i64>,
    reason: &str,
) -> Result<std::result::Result<i64, LifecycleRefusal>> {
    if !capabilities::user_has(pool, actor_user_id, Capability::AssignTraining).await? {
        return Ok(Err(LifecycleRefusal::CapabilityRequired));
    }
    let reason = reason.trim();
    if matches!(kind, PhaseEventKind::Return | PhaseEventKind::Restart) && reason.is_empty() {
        return Ok(Err(LifecycleRefusal::ReasonRequired));
    }
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let effective = effective_at.unwrap_or(now);
    if effective > now {
        return Ok(Err(LifecycleRefusal::EffectiveInFuture));
    }

    let mut tx = pool.begin().await.context("starting phase event")?;
    let Some(current_status) = status(&mut tx, enrollment_id).await? else {
        return Ok(Err(LifecycleRefusal::NoSuchEnrollment));
    };
    if current_status != EnrollmentStatus::Active {
        return Ok(Err(LifecycleRefusal::NotActive));
    }
    let latest_effective: Option<i64> =
        sqlx::query_scalar("SELECT MAX(effective_at) FROM phase_event WHERE enrollment_id = ?1")
            .bind(enrollment_id)
            .fetch_one(&mut *tx)
            .await
            .context("reading latest effective instant")?;
    if latest_effective.is_some_and(|latest| effective < latest) {
        return Ok(Err(LifecycleRefusal::OutOfOrder));
    }

    let pinned: i64 = sqlx::query_scalar("SELECT program_version_id FROM enrollment WHERE id = ?1")
        .bind(enrollment_id)
        .fetch_one(&mut *tx)
        .await
        .context("reading enrollment pin")?;
    let current = current_phase(&mut tx, enrollment_id).await?;
    let paused = is_paused(&mut tx, enrollment_id).await?;

    let (from_phase, to_phase) = match kind {
        PhaseEventKind::Advance | PhaseEventKind::Return | PhaseEventKind::Restart => {
            if paused {
                return Ok(Err(LifecycleRefusal::Paused));
            }
            let Some(to) = to_phase_id else {
                return Ok(Err(LifecycleRefusal::NoSuchPhase));
            };
            let target_in_version: Option<i64> =
                sqlx::query_scalar("SELECT 1 FROM phase WHERE id = ?1 AND program_version_id = ?2")
                    .bind(to)
                    .bind(pinned)
                    .fetch_optional(&mut *tx)
                    .await
                    .context("checking target phase")?;
            if target_in_version.is_none() {
                return Ok(Err(LifecycleRefusal::NoSuchPhase));
            }
            match &current {
                // Entry: no current phase, any phase of the pinned
                // version; return and restart need somewhere to come from.
                None if matches!(kind, PhaseEventKind::Advance) => (None, Some(to)),
                None => {
                    return Ok(Err(LifecycleRefusal::NoCurrentPhase));
                }
                Some((from, _)) => {
                    let edge_kind: Option<String> = sqlx::query_scalar(
                        "SELECT kind FROM phase_transition
                         WHERE program_version_id = ?1 AND from_phase_id = ?2 AND to_phase_id = ?3",
                    )
                    .bind(pinned)
                    .bind(from)
                    .bind(to)
                    .fetch_optional(&mut *tx)
                    .await
                    .context("checking transition edge")?;
                    let allowed = match kind {
                        PhaseEventKind::Advance => {
                            matches!(edge_kind.as_deref(), Some("advance" | "skip"))
                        }
                        PhaseEventKind::Return => edge_kind.as_deref() == Some("remediation"),
                        _ => edge_kind.as_deref() == Some("restart"),
                    };
                    if !allowed {
                        return Ok(Err(LifecycleRefusal::TransitionNotAllowed));
                    }
                    (Some(*from), Some(to))
                }
            }
        }
        PhaseEventKind::Pause | PhaseEventKind::Resume | PhaseEventKind::Complete => {
            let Some((from, _)) = current else {
                return Ok(Err(LifecycleRefusal::NoCurrentPhase));
            };
            match kind {
                PhaseEventKind::Pause if paused => {
                    return Ok(Err(LifecycleRefusal::AlreadyPaused));
                }
                PhaseEventKind::Resume if !paused => {
                    return Ok(Err(LifecycleRefusal::NotPaused));
                }
                PhaseEventKind::Complete if paused => {
                    return Ok(Err(LifecycleRefusal::Paused));
                }
                _ => {}
            }
            (Some(from), None)
        }
    };

    let trainee: i64 = sqlx::query_scalar("SELECT user_id FROM enrollment WHERE id = ?1")
        .bind(enrollment_id)
        .fetch_one(&mut *tx)
        .await
        .context("reading enrollment")?;
    let result = sqlx::query(
        "INSERT INTO phase_event
             (enrollment_id, kind, from_phase_id, to_phase_id,
              effective_at, recorded_at, actor_user_id, reason,
              version_change_event_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                 (SELECT MAX(id) FROM enrollment_event
                  WHERE enrollment_id = ?1 AND kind = 'version_change'))",
    )
    .bind(enrollment_id)
    .bind(kind.as_str())
    .bind(from_phase)
    .bind(to_phase)
    .bind(effective)
    .bind(now)
    .bind(actor_user_id)
    .bind(reason)
    .execute(&mut *tx)
    .await
    .context("recording phase event")?;
    let event_id = result.last_insert_rowid();
    audit::record_for_subject(
        &mut *tx,
        EventKind::PhaseEventRecorded,
        Some(actor_user_id),
        Some(trainee),
        Subject::Enrollment(enrollment_id),
    )
    .await?;
    tx.commit().await.context("committing phase event")?;
    Ok(Ok(event_id))
}

/// Whether `actor_user_id` may read this enrollment's training history:
/// `assign_training` reads everything; `view_assigned_records` reads the
/// enrollments the actor holds an active assignment for (PRINCIPLES.md 10).
pub async fn may_read(pool: &SqlitePool, actor_user_id: i64, enrollment_id: i64) -> Result<bool> {
    if capabilities::user_has(pool, actor_user_id, Capability::AssignTraining).await? {
        return Ok(true);
    }
    Ok(
        capabilities::user_has(pool, actor_user_id, Capability::ViewAssignedRecords).await?
            && assignments::is_assigned(pool, actor_user_id, enrollment_id).await?,
    )
}

/// The enrollment's lifecycle events, oldest first, with presentation
/// fields resolved.
async fn list_events(
    conn: &mut SqliteConnection,
    enrollment_id: i64,
) -> Result<Vec<EnrollmentEvent>> {
    Ok(sqlx::query(
        "SELECT ee.id, ee.kind, ee.occurred_at, ee.actor_user_id,
                u.display_name AS actor_display_name, ee.reason,
                ee.from_program_version_id, fv.version_number AS from_version_number,
                fv.label AS from_version_label,
                ee.to_program_version_id, tv.version_number AS to_version_number,
                tv.label AS to_version_label
         FROM enrollment_event ee
         LEFT JOIN user u ON u.id = ee.actor_user_id
         LEFT JOIN program_version fv ON fv.id = ee.from_program_version_id
         LEFT JOIN program_version tv ON tv.id = ee.to_program_version_id
         WHERE ee.enrollment_id = ?1
         ORDER BY ee.id",
    )
    .bind(enrollment_id)
    .fetch_all(conn)
    .await
    .context("listing enrollment events")?
    .iter()
    .map(|row| EnrollmentEvent {
        id: row.get("id"),
        kind: row.get("kind"),
        occurred_at: row.get("occurred_at"),
        actor_user_id: row.get("actor_user_id"),
        actor_display_name: row.get("actor_display_name"),
        reason: row.get("reason"),
        from_program_version_id: row.get("from_program_version_id"),
        from_version_number: row.get("from_version_number"),
        from_version_label: row.get("from_version_label"),
        to_program_version_id: row.get("to_program_version_id"),
        to_version_number: row.get("to_version_number"),
        to_version_label: row.get("to_version_label"),
    })
    .collect())
}

/// The enrollment's phase history in effective order, with presentation
/// fields resolved.
async fn list_phase_events(
    conn: &mut SqliteConnection,
    enrollment_id: i64,
) -> Result<Vec<PhaseEvent>> {
    Ok(sqlx::query(
        "SELECT pe.id, pe.kind, pe.from_phase_id, fp.name AS from_phase_name,
                pe.to_phase_id, tp.name AS to_phase_name,
                pe.effective_at, pe.recorded_at, pe.actor_user_id,
                u.display_name AS actor_display_name, pe.reason
         FROM phase_event pe
         LEFT JOIN phase fp ON fp.id = pe.from_phase_id
         LEFT JOIN phase tp ON tp.id = pe.to_phase_id
         LEFT JOIN user u ON u.id = pe.actor_user_id
         WHERE pe.enrollment_id = ?1
         ORDER BY pe.effective_at, pe.id",
    )
    .bind(enrollment_id)
    .fetch_all(conn)
    .await
    .context("listing phase events")?
    .iter()
    .map(|row| PhaseEvent {
        id: row.get("id"),
        kind: row.get("kind"),
        from_phase_id: row.get("from_phase_id"),
        from_phase_name: row.get("from_phase_name"),
        to_phase_id: row.get("to_phase_id"),
        to_phase_name: row.get("to_phase_name"),
        effective_at: row.get("effective_at"),
        recorded_at: row.get("recorded_at"),
        actor_user_id: row.get("actor_user_id"),
        actor_display_name: row.get("actor_display_name"),
        reason: row.get("reason"),
    })
    .collect())
}

/// The pinned version's phases and allowed-transition edges, for the
/// interface's target pickers.
async fn pinned_vocabulary(
    conn: &mut SqliteConnection,
    version_id: i64,
) -> Result<(Vec<PhaseRef>, Vec<TransitionRef>)> {
    let phases = sqlx::query(
        "SELECT id, name, presentation_number FROM phase
         WHERE program_version_id = ?1
         ORDER BY presentation_number, name COLLATE NOCASE",
    )
    .bind(version_id)
    .fetch_all(&mut *conn)
    .await
    .context("listing phases")?
    .iter()
    .map(|row| PhaseRef {
        id: row.get("id"),
        name: row.get("name"),
        presentation_number: row.get("presentation_number"),
    })
    .collect();
    let transitions = sqlx::query(
        "SELECT from_phase_id, to_phase_id, kind FROM phase_transition
         WHERE program_version_id = ?1",
    )
    .bind(version_id)
    .fetch_all(&mut *conn)
    .await
    .context("listing transitions")?
    .iter()
    .map(|row| TransitionRef {
        from_phase_id: row.get("from_phase_id"),
        to_phase_id: row.get("to_phase_id"),
        kind: row.get("kind"),
    })
    .collect();
    Ok((phases, transitions))
}

/// The enrollment page's whole story. The capability-and-scope gate runs
/// before existence is revealed.
pub async fn enrollment_detail(
    pool: &SqlitePool,
    actor_user_id: i64,
    enrollment_id: i64,
) -> Result<std::result::Result<EnrollmentDetail, LifecycleRefusal>> {
    if !may_read(pool, actor_user_id, enrollment_id).await? {
        return Ok(Err(LifecycleRefusal::CapabilityRequired));
    }
    let mut conn = pool.acquire().await.context("acquiring connection")?;
    let Some(header) = sqlx::query(
        "SELECT e.id, e.user_id, u.username, u.display_name, e.enrolled_at,
                e.program_version_id, pv.program_id, pv.name AS program_name,
                pv.version_number, pv.label
         FROM enrollment e
         JOIN user u ON u.id = e.user_id
         JOIN program_version pv ON pv.id = e.program_version_id
         WHERE e.id = ?1",
    )
    .bind(enrollment_id)
    .fetch_optional(&mut *conn)
    .await
    .context("reading enrollment")?
    else {
        return Ok(Err(LifecycleRefusal::NoSuchEnrollment));
    };
    let pinned: i64 = header.get("program_version_id");

    let status = status(&mut conn, enrollment_id)
        .await?
        .unwrap_or(EnrollmentStatus::Active);
    let current = current_phase(&mut conn, enrollment_id).await?;
    let paused = is_paused(&mut conn, enrollment_id).await?;
    let events = list_events(&mut conn, enrollment_id).await?;
    let phase_events = list_phase_events(&mut conn, enrollment_id).await?;
    let (phases, transitions) = pinned_vocabulary(&mut conn, pinned).await?;
    drop(conn);
    let assignments = assignments::list_for_enrollment(pool, enrollment_id).await?;

    Ok(Ok(EnrollmentDetail {
        enrollment_id: header.get("id"),
        trainee_user_id: header.get("user_id"),
        trainee_username: header.get("username"),
        trainee_display_name: header.get("display_name"),
        enrolled_at: header.get("enrolled_at"),
        program_id: header.get("program_id"),
        program_version_id: pinned,
        program_name: header.get("program_name"),
        version_number: header.get("version_number"),
        version_label: header.get("label"),
        status,
        paused,
        current_phase_id: current.as_ref().map(|(id, _)| *id),
        current_phase_name: current.map(|(_, name)| name),
        events,
        phase_events,
        assignments,
        phases,
        transitions,
    }))
}
