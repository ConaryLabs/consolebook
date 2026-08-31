//! Task signoffs (docs/domain-model.md `TaskSignoff`; ADR 0013;
//! Milestone 4 slice 4).
//!
//! A signoff is a versioned record that a configured task was observed
//! or demonstrated: append-only rows per (enrollment, task) where the
//! latest row answers the current state. The first signoff takes
//! authoring scope; every later row is an override taking
//! `review_evaluation` and a recorded reason, and a revocation exists
//! only where there is something to revoke. Migration 0013 holds the
//! pinning, ordering, reason shape, and permanence raw.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use time::OffsetDateTime;

use crate::audit::{self, EventKind, Subject};
use crate::capabilities::{self, Capability};
use crate::{assignments, lifecycle, storage};

/// Typed refusals for recording a signoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignoffRefusal {
    NoSuchEnrollment,
    /// The task is not in the enrollment's pinned version's vocabulary.
    NoSuchTask,
    CapabilityRequired,
    /// An override explains itself (#32 decision 6; ADR 0013).
    ReasonRequired,
    /// A revocation supersedes a signoff.
    NothingToRevoke,
}

/// The closed signoff kind set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignoffKind {
    Observed,
    Demonstrated,
    Revoked,
}

impl SignoffKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::Demonstrated => "demonstrated",
            Self::Revoked => "revoked",
        }
    }
}

/// Records one signoff row. The first for a task takes authoring scope
/// (a coordinator, or an assigned evaluation author); any later row is
/// an override taking `review_evaluation` and a non-blank reason.
pub async fn record(
    pool: &SqlitePool,
    actor_user_id: i64,
    enrollment_id: i64,
    task_id: i64,
    kind: SignoffKind,
    reason: &str,
) -> Result<std::result::Result<(), SignoffRefusal>> {
    let Some(version_id): Option<i64> =
        sqlx::query_scalar("SELECT program_version_id FROM enrollment WHERE id = ?1")
            .bind(enrollment_id)
            .fetch_optional(pool)
            .await
            .context("reading enrollment")?
    else {
        return Ok(Err(SignoffRefusal::NoSuchEnrollment));
    };
    let pinned: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM task WHERE id = ?1 AND program_version_id = ?2")
            .bind(task_id)
            .bind(version_id)
            .fetch_optional(pool)
            .await
            .context("checking task")?;
    if pinned.is_none() {
        return Ok(Err(SignoffRefusal::NoSuchTask));
    }
    let coordinator =
        capabilities::user_has(pool, actor_user_id, Capability::AssignTraining).await?;
    let author_in_scope = capabilities::user_has(pool, actor_user_id, Capability::AuthorEvaluation)
        .await?
        && assignments::is_assigned(pool, actor_user_id, enrollment_id).await?;
    let reviewer =
        capabilities::user_has(pool, actor_user_id, Capability::ReviewEvaluation).await?;
    let reason = reason.trim();

    let mut tx = storage::write_tx(pool).await.context("starting signoff")?;
    let prior: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM task_signoff WHERE enrollment_id = ?1 AND task_id = ?2 LIMIT 1",
    )
    .bind(enrollment_id)
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await
    .context("checking prior signoffs")?;
    if prior.is_some() {
        // An override supersedes recorded state: explicit authority and
        // a recorded reason (ADR 0013).
        if !reviewer {
            return storage::refuse(tx, SignoffRefusal::CapabilityRequired).await;
        }
        if reason.is_empty() {
            return storage::refuse(tx, SignoffRefusal::ReasonRequired).await;
        }
    } else {
        if !coordinator && !author_in_scope {
            return storage::refuse(tx, SignoffRefusal::CapabilityRequired).await;
        }
        if kind == SignoffKind::Revoked {
            return storage::refuse(tx, SignoffRefusal::NothingToRevoke).await;
        }
    }
    let signer_name: String = sqlx::query_scalar("SELECT display_name FROM user WHERE id = ?1")
        .bind(actor_user_id)
        .fetch_one(&mut *tx)
        .await
        .context("reading signer name")?;
    let now = OffsetDateTime::now_utc().unix_timestamp();
    sqlx::query(
        "INSERT INTO task_signoff
             (enrollment_id, task_id, kind, reason, signed_by,
              signed_by_display_name, signed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(enrollment_id)
    .bind(task_id)
    .bind(kind.as_str())
    .bind(reason)
    .bind(actor_user_id)
    .bind(&signer_name)
    .bind(now)
    .execute(&mut *tx)
    .await
    .context("recording signoff")?;
    let trainee: i64 = sqlx::query_scalar("SELECT user_id FROM enrollment WHERE id = ?1")
        .bind(enrollment_id)
        .fetch_one(&mut *tx)
        .await
        .context("reading trainee")?;
    audit::record_for_subject(
        &mut *tx,
        EventKind::TaskSignoffRecorded,
        Some(actor_user_id),
        Some(trainee),
        Subject::Enrollment(enrollment_id),
    )
    .await?;
    tx.commit().await.context("committing signoff")?;
    Ok(Ok(()))
}

/// One task's row in the signoff matrix: the pinned task and its
/// current state, from the latest row's stored snapshot.
#[derive(Debug, Serialize)]
pub struct MatrixRow {
    pub task_id: i64,
    pub competency_category: String,
    pub competency_name: String,
    pub prompt: String,
    pub kind: Option<String>,
    pub reason: Option<String>,
    pub signed_by_display_name: Option<String>,
    pub signed_at: Option<i64>,
    pub history: i64,
}

/// Every task of the enrollment's pinned version with its current
/// signoff state, for readers the enrollment history is open to.
pub async fn matrix(
    pool: &SqlitePool,
    actor_user_id: i64,
    enrollment_id: i64,
) -> Result<std::result::Result<Vec<MatrixRow>, SignoffRefusal>> {
    let Some(version_id): Option<i64> =
        sqlx::query_scalar("SELECT program_version_id FROM enrollment WHERE id = ?1")
            .bind(enrollment_id)
            .fetch_optional(pool)
            .await
            .context("reading enrollment")?
    else {
        return Ok(Err(SignoffRefusal::NoSuchEnrollment));
    };
    if !lifecycle::may_read(pool, actor_user_id, enrollment_id).await? {
        return Ok(Err(SignoffRefusal::CapabilityRequired));
    }
    let rows = sqlx::query(
        "SELECT t.id AS task_id, c.category, c.name, t.prompt,
                s.kind, s.reason, s.signed_by_display_name, s.signed_at,
                (SELECT COUNT(*) FROM task_signoff h
                 WHERE h.enrollment_id = ?1 AND h.task_id = t.id) AS history
         FROM task t
         JOIN competency c ON c.id = t.competency_id
         LEFT JOIN task_signoff s
             ON s.id = (SELECT MAX(s2.id) FROM task_signoff s2
                        WHERE s2.enrollment_id = ?1 AND s2.task_id = t.id)
         WHERE t.program_version_id = ?2
         ORDER BY c.category COLLATE NOCASE, c.name COLLATE NOCASE,
                  t.sort_order, t.id",
    )
    .bind(enrollment_id)
    .bind(version_id)
    .fetch_all(pool)
    .await
    .context("listing the signoff matrix")?;
    Ok(Ok(rows
        .iter()
        .map(|row| MatrixRow {
            task_id: row.get("task_id"),
            competency_category: row.get("category"),
            competency_name: row.get("name"),
            prompt: row.get("prompt"),
            kind: row.get("kind"),
            reason: row.get("reason"),
            signed_by_display_name: row.get("signed_by_display_name"),
            signed_at: row.get("signed_at"),
            history: row.get("history"),
        })
        .collect()))
}
