//! Training session lifecycle (ADR 0008, ADR 0009;
//! docs/domain-model.md `TrainingSession`).
//!
//! A session captures agency-local meaning (business date, timezone
//! snapshot, local start/end stored verbatim) beside the UTC instants
//! that ordering, duration, and the overlap invariant reason about
//! (PRINCIPLES.md 6). Each capability has one owner: `session_time`
//! resolves entered local times to instants (ADR 0009),
//! `session_membership` owns the trainer-membership grants, migration
//! 0007 enforces the schema-level invariants (end never precedes start,
//! active intervals never overlap, phases belong to the stamped version,
//! the one-trainer floor, immutable identities), and this module owns
//! the lifecycle: capability-plus-scope gates, the disposition rules,
//! creation, editing open sessions, closing, and the session reads —
//! with typed refusals ahead of every database backstop.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqliteConnection, SqlitePool};
use time::OffsetDateTime;

use crate::assignments;
use crate::audit::{self, EventKind, Subject};
use crate::capabilities::{self, Capability};
use crate::lifecycle::{self, EnrollmentStatus};
use crate::session_membership::{self, SessionTrainerRow};
use crate::session_time::{self, TimeRefusal};

/// Session dispositions: a closed set, like scale kinds (ADR 0007's
/// pattern). Completed and interrupted training occupied their interval;
/// a cancelled session never happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Completed,
    Interrupted,
    Cancelled,
}

impl Disposition {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Interrupted => "interrupted",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Why a session operation was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum SessionRefusal {
    CapabilityRequired,
    NoSuchEnrollment,
    /// Sessions are created on active enrollments.
    EnrollmentInactive,
    NoSuchSession,
    /// The session already carries a disposition.
    SessionClosed,
    /// The business date is not a real calendar date.
    InvalidBusinessDate,
    /// The timezone name is not in the IANA database.
    UnknownTimezone,
    /// A local time failed to parse or resolve.
    InvalidLocalTime,
    /// The end instant precedes the start instant.
    EndBeforeStart,
    /// Completed and interrupted sessions carry an end time.
    EndRequired,
    /// Cancelled sessions carry no end time.
    EndNotAllowed,
    /// An end time at creation needs a disposition.
    DispositionRequired,
    /// Sessions are cancelled after creation, never created cancelled.
    InvalidDisposition,
    /// Active training intervals for one trainee cannot overlap.
    Overlap,
    /// The phase does not belong to the enrollment's pinned version.
    NoSuchPhase,
    NoSuchUser,
    /// Session trainers hold `author_evaluation` (#22 decision 2).
    TrainerLacksCapability,
    AlreadyMember,
    NotMember,
    /// A session keeps at least one trainer.
    LastTrainer,
    /// A session needs at least one trainer at creation.
    NoTrainers,
}

impl From<TimeRefusal> for SessionRefusal {
    fn from(refusal: TimeRefusal) -> Self {
        match refusal {
            TimeRefusal::InvalidBusinessDate => Self::InvalidBusinessDate,
            TimeRefusal::UnknownTimezone => Self::UnknownTimezone,
            TimeRefusal::InvalidLocalTime => Self::InvalidLocalTime,
            TimeRefusal::EndBeforeStart => Self::EndBeforeStart,
        }
    }
}

/// What the operator entered for a new session, verbatim.
#[derive(Debug, Clone, Deserialize)]
pub struct SessionInput {
    pub business_date: String,
    pub timezone: String,
    pub local_start: String,
    #[serde(default)]
    pub local_end: Option<String>,
    /// Retroactively recorded sessions close at creation; open sessions
    /// omit this.
    #[serde(default)]
    pub disposition: Option<Disposition>,
    #[serde(default)]
    pub phase_id: Option<i64>,
    /// Empty means the acting trainer records their own session.
    #[serde(default)]
    pub trainer_user_ids: Vec<i64>,
}

/// One session with presentation fields resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionRow {
    pub id: i64,
    pub enrollment_id: i64,
    pub business_date: String,
    pub timezone: String,
    pub local_start: String,
    pub local_end: Option<String>,
    pub utc_start: i64,
    pub utc_end: Option<i64>,
    pub phase_id: Option<i64>,
    pub phase_name: Option<String>,
    pub disposition: Option<String>,
    pub created_at: i64,
    pub created_by: Option<i64>,
    pub closed_at: Option<i64>,
    pub closed_by: Option<i64>,
    /// The session's daily draft, once one exists (slice 3).
    pub draft_id: Option<i64>,
    pub trainers: Vec<SessionTrainerRow>,
}

/// A session with its enrollment context, for the session view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionDetail {
    #[serde(flatten)]
    pub session: SessionRow,
    pub trainee_user_id: i64,
    pub trainee_username: String,
    pub trainee_display_name: String,
    pub program_id: i64,
    pub program_name: String,
    pub version_number: i64,
}

/// One of the caller's own sessions, for the "my sessions" view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MySession {
    pub session_id: i64,
    pub enrollment_id: i64,
    pub business_date: String,
    pub timezone: String,
    pub local_start: String,
    pub local_end: Option<String>,
    pub utc_start: i64,
    pub disposition: Option<String>,
    pub phase_name: Option<String>,
    pub trainee_user_id: i64,
    pub trainee_username: String,
    pub trainee_display_name: String,
    pub program_name: String,
    pub version_number: i64,
    /// The session's daily draft, once one exists (slice 3).
    pub draft_id: Option<i64>,
}

// ---------------------------------------------------------------- gates

/// Whether the actor may create sessions on this enrollment: a
/// coordinator, or an assigned trainer who authors evaluations.
async fn may_create(pool: &SqlitePool, actor_user_id: i64, enrollment_id: i64) -> Result<bool> {
    if capabilities::user_has(pool, actor_user_id, Capability::AssignTraining).await? {
        return Ok(true);
    }
    Ok(
        capabilities::user_has(pool, actor_user_id, Capability::AuthorEvaluation).await?
            && assignments::is_assigned(pool, actor_user_id, enrollment_id).await?,
    )
}

// ---------------------------------------------------------------- write

/// Typed overlap check ahead of the database trigger. `exclude` skips the
/// session being updated; pass 0 on creation.
async fn overlaps(
    tx: &mut SqliteConnection,
    enrollment_id: i64,
    utc_start: i64,
    utc_end: Option<i64>,
    exclude: i64,
) -> Result<bool> {
    let hit: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM training_session ts
         JOIN enrollment other ON other.id = ts.enrollment_id
         WHERE ts.id != ?4
           AND other.user_id = (SELECT user_id FROM enrollment WHERE id = ?1)
           AND (ts.disposition IS NULL OR ts.disposition != 'cancelled')
           AND ts.utc_start < COALESCE(?3, 9223372036854775807)
           AND ?2 < COALESCE(ts.utc_end, 9223372036854775807)",
    )
    .bind(enrollment_id)
    .bind(utc_start)
    .bind(utc_end)
    .bind(exclude)
    .fetch_optional(&mut *tx)
    .await
    .context("checking interval overlap")?;
    Ok(hit.is_some())
}

/// Creates a session — open, or retroactively complete when the input
/// carries an end and a completed/interrupted disposition. The creating
/// trainer is the default member.
#[allow(clippy::too_many_lines)]
pub async fn create(
    pool: &SqlitePool,
    actor_user_id: i64,
    enrollment_id: i64,
    input: &SessionInput,
) -> Result<std::result::Result<i64, SessionRefusal>> {
    if !may_create(pool, actor_user_id, enrollment_id).await? {
        return Ok(Err(SessionRefusal::CapabilityRequired));
    }
    // Normalize before the disposition rules so an empty or whitespace
    // end means "no end", matching how the resolver stores it.
    let local_end = input
        .local_end
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match (input.disposition, local_end) {
        (None, Some(_)) => return Ok(Err(SessionRefusal::DispositionRequired)),
        (Some(Disposition::Cancelled), _) => {
            return Ok(Err(SessionRefusal::InvalidDisposition));
        }
        (Some(_), None) => return Ok(Err(SessionRefusal::EndRequired)),
        _ => {}
    }
    let times = match session_time::resolve(
        &input.business_date,
        &input.timezone,
        &input.local_start,
        local_end,
    ) {
        Ok(times) => times,
        Err(refusal) => return Ok(Err(refusal.into())),
    };

    let mut tx = pool.begin().await.context("starting session")?;
    let Some(status) = lifecycle::status(&mut tx, enrollment_id).await? else {
        return Ok(Err(SessionRefusal::NoSuchEnrollment));
    };
    if status != EnrollmentStatus::Active {
        return Ok(Err(SessionRefusal::EnrollmentInactive));
    }
    // The session stamps the pin at creation (migration 0007), so its
    // program and phase context stay historic across version changes.
    let pinned: i64 = sqlx::query_scalar("SELECT program_version_id FROM enrollment WHERE id = ?1")
        .bind(enrollment_id)
        .fetch_one(&mut *tx)
        .await
        .context("reading enrollment pin")?;
    if let Some(phase_id) = input.phase_id {
        let in_version: Option<i64> =
            sqlx::query_scalar("SELECT 1 FROM phase WHERE id = ?1 AND program_version_id = ?2")
                .bind(phase_id)
                .bind(pinned)
                .fetch_optional(&mut *tx)
                .await
                .context("checking session phase")?;
        if in_version.is_none() {
            return Ok(Err(SessionRefusal::NoSuchPhase));
        }
    }
    let trainers = match session_membership::validate_trainers(
        &mut tx,
        actor_user_id,
        &input.trainer_user_ids,
    )
    .await?
    {
        Ok(trainers) => trainers,
        Err(refusal) => return Ok(Err(refusal)),
    };
    if overlaps(&mut tx, enrollment_id, times.utc_start, times.utc_end, 0).await? {
        return Ok(Err(SessionRefusal::Overlap));
    }

    let now = OffsetDateTime::now_utc().unix_timestamp();
    let (disposition, closed_at, closed_by) = match input.disposition {
        Some(disposition) => (Some(disposition.as_str()), Some(now), Some(actor_user_id)),
        None => (None, None, None),
    };
    let result = sqlx::query(
        "INSERT INTO training_session
             (enrollment_id, program_version_id, business_date, timezone,
              local_start, local_end, utc_start, utc_end, phase_id, disposition,
              created_at, created_by, closed_at, closed_by)
         VALUES (?1, ?14, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
    )
    .bind(enrollment_id)
    .bind(&times.business_date)
    .bind(&times.timezone)
    .bind(&times.local_start)
    .bind(&times.local_end)
    .bind(times.utc_start)
    .bind(times.utc_end)
    .bind(input.phase_id)
    .bind(disposition)
    .bind(now)
    .bind(actor_user_id)
    .bind(closed_at)
    .bind(closed_by)
    .bind(pinned)
    .execute(&mut *tx)
    .await
    .context("creating session")?;
    let session_id = result.last_insert_rowid();
    // Creation is recorded before its grants: audit ids are the tie-break
    // within one second, so the stream never shows access preceding the
    // session it opens.
    let trainee: i64 = sqlx::query_scalar("SELECT user_id FROM enrollment WHERE id = ?1")
        .bind(enrollment_id)
        .fetch_one(&mut *tx)
        .await
        .context("reading enrollment")?;
    audit::record_for_subject(
        &mut *tx,
        EventKind::SessionCreated,
        Some(actor_user_id),
        Some(trainee),
        Subject::Session(session_id),
    )
    .await?;
    for trainer in &trainers {
        session_membership::insert_member(&mut tx, session_id, *trainer, actor_user_id, now)
            .await?;
    }
    tx.commit().await.context("committing session")?;
    Ok(Ok(session_id))
}

/// What the operator may change on an open session.
#[derive(Debug, Clone, Deserialize)]
pub struct SessionUpdate {
    pub business_date: String,
    pub timezone: String,
    pub local_start: String,
    #[serde(default)]
    pub phase_id: Option<i64>,
}

/// Edits an open session's entered fields, re-resolving UTC. Closed
/// sessions are not editable; corrections to closed sessions arrive with
/// the record-correction machinery.
pub async fn update_open(
    pool: &SqlitePool,
    actor_user_id: i64,
    session_id: i64,
    update: &SessionUpdate,
) -> Result<std::result::Result<(), SessionRefusal>> {
    if !session_membership::may_work(pool, actor_user_id, session_id).await? {
        return Ok(Err(SessionRefusal::CapabilityRequired));
    }
    let times = match session_time::resolve(
        &update.business_date,
        &update.timezone,
        &update.local_start,
        None,
    ) {
        Ok(times) => times,
        Err(refusal) => return Ok(Err(refusal.into())),
    };
    let mut tx = pool.begin().await.context("starting session update")?;
    let Some(row) = sqlx::query(
        "SELECT enrollment_id, disposition, phase_id FROM training_session WHERE id = ?1",
    )
    .bind(session_id)
    .fetch_optional(&mut *tx)
    .await
    .context("reading session")?
    else {
        return Ok(Err(SessionRefusal::NoSuchSession));
    };
    let disposition: Option<String> = row.get("disposition");
    if disposition.is_some() {
        return Ok(Err(SessionRefusal::SessionClosed));
    }
    let enrollment_id: i64 = row.get("enrollment_id");
    // A session's phase context comes from the version it was recorded
    // under (its stamp, migration 0007), so a phase change validates
    // against that version — never the enrollment's possibly newer pin.
    let stored_phase: Option<i64> = row.get("phase_id");
    if let Some(phase_id) = update.phase_id
        && Some(phase_id) != stored_phase
    {
        let in_version: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM phase
             WHERE id = ?1 AND program_version_id
                 = (SELECT program_version_id FROM training_session WHERE id = ?2)",
        )
        .bind(phase_id)
        .bind(session_id)
        .fetch_optional(&mut *tx)
        .await
        .context("checking session phase")?;
        if in_version.is_none() {
            return Ok(Err(SessionRefusal::NoSuchPhase));
        }
    }
    if overlaps(&mut tx, enrollment_id, times.utc_start, None, session_id).await? {
        return Ok(Err(SessionRefusal::Overlap));
    }
    sqlx::query(
        "UPDATE training_session
         SET business_date = ?1, timezone = ?2, local_start = ?3,
             utc_start = ?4, phase_id = ?5
         WHERE id = ?6",
    )
    .bind(&times.business_date)
    .bind(&times.timezone)
    .bind(&times.local_start)
    .bind(times.utc_start)
    .bind(update.phase_id)
    .bind(session_id)
    .execute(&mut *tx)
    .await
    .context("updating session")?;
    audit::record_for_subject(
        &mut *tx,
        EventKind::SessionUpdated,
        Some(actor_user_id),
        None,
        Subject::Session(session_id),
    )
    .await?;
    tx.commit().await.context("committing session update")?;
    Ok(Ok(()))
}

/// Closes an open session with a disposition: completed and interrupted
/// take the entered local end; cancelled takes none — the session never
/// happened and releases its interval.
pub async fn close(
    pool: &SqlitePool,
    actor_user_id: i64,
    session_id: i64,
    disposition: Disposition,
    local_end: Option<&str>,
) -> Result<std::result::Result<(), SessionRefusal>> {
    if !session_membership::may_work(pool, actor_user_id, session_id).await? {
        return Ok(Err(SessionRefusal::CapabilityRequired));
    }
    let mut tx = pool.begin().await.context("starting session close")?;
    let Some(row) =
        sqlx::query("SELECT timezone, utc_start, disposition FROM training_session WHERE id = ?1")
            .bind(session_id)
            .fetch_optional(&mut *tx)
            .await
            .context("reading session")?
    else {
        return Ok(Err(SessionRefusal::NoSuchSession));
    };
    let already: Option<String> = row.get("disposition");
    if already.is_some() {
        return Ok(Err(SessionRefusal::SessionClosed));
    }

    let local_end = local_end.map(str::trim).filter(|value| !value.is_empty());
    let (local_end, utc_end) = match disposition {
        Disposition::Cancelled => {
            if local_end.is_some() {
                return Ok(Err(SessionRefusal::EndNotAllowed));
            }
            (None, None)
        }
        Disposition::Completed | Disposition::Interrupted => {
            let Some(value) = local_end else {
                return Ok(Err(SessionRefusal::EndRequired));
            };
            let timezone: String = row.get("timezone");
            let utc_start: i64 = row.get("utc_start");
            match session_time::resolve_end(&timezone, value, utc_start) {
                Ok(instant) => (Some(value.to_owned()), Some(instant)),
                Err(refusal) => return Ok(Err(refusal.into())),
            }
        }
    };

    let now = OffsetDateTime::now_utc().unix_timestamp();
    sqlx::query(
        "UPDATE training_session
         SET local_end = ?1, utc_end = ?2, disposition = ?3,
             closed_at = ?4, closed_by = ?5
         WHERE id = ?6",
    )
    .bind(&local_end)
    .bind(utc_end)
    .bind(disposition.as_str())
    .bind(now)
    .bind(actor_user_id)
    .bind(session_id)
    .execute(&mut *tx)
    .await
    .context("closing session")?;
    audit::record_for_subject(
        &mut *tx,
        EventKind::SessionClosed,
        Some(actor_user_id),
        None,
        Subject::Session(session_id),
    )
    .await?;
    tx.commit().await.context("committing session close")?;
    Ok(Ok(()))
}

// ----------------------------------------------------------------- read

fn session_from_row(row: &sqlx::sqlite::SqliteRow) -> SessionRow {
    SessionRow {
        id: row.get("id"),
        enrollment_id: row.get("enrollment_id"),
        business_date: row.get("business_date"),
        timezone: row.get("timezone"),
        local_start: row.get("local_start"),
        local_end: row.get("local_end"),
        utc_start: row.get("utc_start"),
        utc_end: row.get("utc_end"),
        phase_id: row.get("phase_id"),
        phase_name: row.get("phase_name"),
        disposition: row.get("disposition"),
        created_at: row.get("created_at"),
        created_by: row.get("created_by"),
        closed_at: row.get("closed_at"),
        closed_by: row.get("closed_by"),
        draft_id: row.get("draft_id"),
        trainers: Vec::new(),
    }
}

/// The session's daily draft as a scalar subselect — fan-out safe even
/// though the coverage schema is many-to-many.
const DRAFT_ID_SELECT: &str = "(SELECT es.evaluation_record_id FROM evaluation_session es
          WHERE es.training_session_id = s.id
          ORDER BY es.evaluation_record_id LIMIT 1) AS draft_id";

/// The enrollment's sessions, newest first, gated like the enrollment
/// detail (capability plus assignment scope).
pub async fn list_for_enrollment(
    pool: &SqlitePool,
    actor_user_id: i64,
    enrollment_id: i64,
) -> Result<std::result::Result<Vec<SessionRow>, SessionRefusal>> {
    if !lifecycle::may_read(pool, actor_user_id, enrollment_id).await? {
        return Ok(Err(SessionRefusal::CapabilityRequired));
    }
    let mut conn = pool.acquire().await.context("acquiring connection")?;
    let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM enrollment WHERE id = ?1")
        .bind(enrollment_id)
        .fetch_optional(&mut *conn)
        .await
        .context("checking enrollment")?;
    if exists.is_none() {
        return Ok(Err(SessionRefusal::NoSuchEnrollment));
    }
    let rows = sqlx::query(&format!(
        "SELECT s.id, s.enrollment_id, s.business_date, s.timezone, s.local_start,
                s.local_end, s.utc_start, s.utc_end, s.phase_id, p.name AS phase_name,
                s.disposition, s.created_at, s.created_by, s.closed_at, s.closed_by,
                {DRAFT_ID_SELECT}
         FROM training_session s
         LEFT JOIN phase p ON p.id = s.phase_id
         WHERE s.enrollment_id = ?1
         ORDER BY s.utc_start DESC, s.id DESC",
    ))
    .bind(enrollment_id)
    .fetch_all(&mut *conn)
    .await
    .context("listing sessions")?;
    let mut sessions: Vec<SessionRow> = rows.iter().map(session_from_row).collect();
    let ids: Vec<i64> = sessions.iter().map(|session| session.id).collect();
    let mut trainers = session_membership::trainers_for(&mut conn, &ids).await?;
    for session in &mut sessions {
        session.trainers = trainers.remove(&session.id).unwrap_or_default();
    }
    Ok(Ok(sessions))
}

/// One session with its enrollment context. Coordinators, the session's
/// trainers, and assigned trainers holding `view_assigned_records` read
/// it.
pub async fn get(
    pool: &SqlitePool,
    actor_user_id: i64,
    session_id: i64,
) -> Result<std::result::Result<SessionDetail, SessionRefusal>> {
    let allowed = session_membership::may_work(pool, actor_user_id, session_id).await? || {
        let enrollment: Option<i64> =
            sqlx::query_scalar("SELECT enrollment_id FROM training_session WHERE id = ?1")
                .bind(session_id)
                .fetch_optional(pool)
                .await
                .context("reading session")?;
        match enrollment {
            Some(enrollment_id) => lifecycle::may_read(pool, actor_user_id, enrollment_id).await?,
            None => false,
        }
    };
    if !allowed {
        return Ok(Err(SessionRefusal::CapabilityRequired));
    }
    let mut conn = pool.acquire().await.context("acquiring connection")?;
    let Some(row) = sqlx::query(&format!(
        "SELECT s.id, s.enrollment_id, s.business_date, s.timezone, s.local_start,
                s.local_end, s.utc_start, s.utc_end, s.phase_id, p.name AS phase_name,
                s.disposition, s.created_at, s.created_by, s.closed_at, s.closed_by,
                e.user_id AS trainee_user_id, u.username AS trainee_username,
                u.display_name AS trainee_display_name,
                pv.program_id, pv.name AS program_name, pv.version_number,
                {DRAFT_ID_SELECT}
         FROM training_session s
         LEFT JOIN phase p ON p.id = s.phase_id
         JOIN enrollment e ON e.id = s.enrollment_id
         JOIN user u ON u.id = e.user_id
         JOIN program_version pv ON pv.id = s.program_version_id
         WHERE s.id = ?1",
    ))
    .bind(session_id)
    .fetch_optional(&mut *conn)
    .await
    .context("reading session")?
    else {
        return Ok(Err(SessionRefusal::NoSuchSession));
    };
    let mut session = session_from_row(&row);
    let mut trainers = session_membership::trainers_for(&mut conn, &[session.id]).await?;
    session.trainers = trainers.remove(&session.id).unwrap_or_default();
    Ok(Ok(SessionDetail {
        session,
        trainee_user_id: row.get("trainee_user_id"),
        trainee_username: row.get("trainee_username"),
        trainee_display_name: row.get("trainee_display_name"),
        program_id: row.get("program_id"),
        program_name: row.get("program_name"),
        version_number: row.get("version_number"),
    }))
}

/// The caller's own sessions — the ones they are a trainer on — open
/// first, then newest. Gated on `author_evaluation`, the capability that
/// admitted them to every session listed.
pub async fn list_mine(
    pool: &SqlitePool,
    actor_user_id: i64,
) -> Result<std::result::Result<Vec<MySession>, SessionRefusal>> {
    if !capabilities::user_has(pool, actor_user_id, Capability::AuthorEvaluation).await? {
        return Ok(Err(SessionRefusal::CapabilityRequired));
    }
    let rows = sqlx::query(&format!(
        "SELECT s.id AS session_id, s.enrollment_id, s.business_date, s.timezone,
                s.local_start, s.local_end, s.utc_start, s.disposition,
                p.name AS phase_name,
                e.user_id AS trainee_user_id, u.username AS trainee_username,
                u.display_name AS trainee_display_name,
                pv.name AS program_name, pv.version_number,
                {DRAFT_ID_SELECT}
         FROM session_trainer st
         JOIN training_session s ON s.id = st.session_id
         LEFT JOIN phase p ON p.id = s.phase_id
         JOIN enrollment e ON e.id = s.enrollment_id
         JOIN user u ON u.id = e.user_id
         JOIN program_version pv ON pv.id = s.program_version_id
         WHERE st.trainer_user_id = ?1
         ORDER BY (s.disposition IS NULL) DESC, s.utc_start DESC, s.id DESC",
    ))
    .bind(actor_user_id)
    .fetch_all(pool)
    .await
    .context("listing own sessions")?;
    Ok(Ok(rows
        .iter()
        .map(|row| MySession {
            session_id: row.get("session_id"),
            enrollment_id: row.get("enrollment_id"),
            business_date: row.get("business_date"),
            timezone: row.get("timezone"),
            local_start: row.get("local_start"),
            local_end: row.get("local_end"),
            utc_start: row.get("utc_start"),
            disposition: row.get("disposition"),
            phase_name: row.get("phase_name"),
            trainee_user_id: row.get("trainee_user_id"),
            trainee_username: row.get("trainee_username"),
            trainee_display_name: row.get("trainee_display_name"),
            program_name: row.get("program_name"),
            version_number: row.get("version_number"),
            draft_id: row.get("draft_id"),
        })
        .collect()))
}
