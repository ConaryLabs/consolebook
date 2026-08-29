//! Persisted in-app notices.
//!
//! Notices are how the application tells its users something happened;
//! they never depend on email. Every read and mutation is recipient-scoped
//! in the query itself — there is no path that lists or edits another
//! user's notices.

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::{Executor, Row, Sqlite, SqlitePool};
use time::OffsetDateTime;

use crate::capabilities::Capability;

/// Notice vocabulary. Training-workflow kinds join with their milestones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    BackupFailed,
    AssignmentCreated,
    DraftOwnershipReceived,
    DraftSubmittedForReview,
    DraftApproved,
    DraftChangesRequested,
    DraftReturned,
}

impl NoticeKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BackupFailed => "backup_failed",
            Self::AssignmentCreated => "assignment_created",
            Self::DraftOwnershipReceived => "draft_ownership_received",
            Self::DraftSubmittedForReview => "draft_submitted_for_review",
            Self::DraftApproved => "draft_approved",
            Self::DraftChangesRequested => "draft_changes_requested",
            Self::DraftReturned => "draft_returned",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Notice {
    pub id: i64,
    pub kind: String,
    pub message: String,
    pub created_at: i64,
    pub read_at: Option<i64>,
}

/// Notifies every user holding `capability`, skipping users who already
/// have an unread notice of the same kind — a repeatedly failing backup
/// becomes one unread notice per administrator, not a flood. Returns how
/// many notices were created.
pub async fn notify_capability_holders(
    pool: &SqlitePool,
    capability: Capability,
    kind: NoticeKind,
    message: &str,
) -> Result<u64> {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let result = sqlx::query(
        "INSERT INTO notice (user_id, kind, message, created_at)
         SELECT cg.user_id, ?1, ?2, ?3
         FROM capability_grant cg
         WHERE cg.capability = ?4
           AND NOT EXISTS (
               SELECT 1 FROM notice n
               WHERE n.user_id = cg.user_id AND n.kind = ?1 AND n.read_at IS NULL
           )",
    )
    .bind(kind.as_str())
    .bind(message)
    .bind(now)
    .bind(capability.as_str())
    .execute(pool)
    .await
    .context("creating notices")?;
    Ok(result.rows_affected())
}

/// Notifies one recipient. Workflow notices name the person they concern
/// and ride the transaction of the action they announce, so a rolled-back
/// action never leaves a notice behind.
pub async fn notify_user<'e>(
    executor: impl Executor<'e, Database = Sqlite>,
    user_id: i64,
    kind: NoticeKind,
    message: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO notice (user_id, kind, message, created_at)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(user_id)
    .bind(kind.as_str())
    .bind(message)
    .bind(OffsetDateTime::now_utc().unix_timestamp())
    .execute(executor)
    .await
    .context("creating notice")?;
    Ok(())
}

/// The recipient's notices, unread first, newest first within each group.
pub async fn list_for_user(pool: &SqlitePool, user_id: i64) -> Result<Vec<Notice>> {
    let rows = sqlx::query(
        "SELECT id, kind, message, created_at, read_at
         FROM notice WHERE user_id = ?1
         ORDER BY (read_at IS NULL) DESC, created_at DESC, id DESC
         LIMIT 100",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .context("listing notices")?;
    Ok(rows
        .into_iter()
        .map(|row| Notice {
            id: row.get("id"),
            kind: row.get("kind"),
            message: row.get("message"),
            created_at: row.get("created_at"),
            read_at: row.get("read_at"),
        })
        .collect())
}

/// Unread count for the recipient, for the shell's header badge.
pub async fn unread_count(pool: &SqlitePool, user_id: i64) -> Result<i64> {
    sqlx::query_scalar("SELECT COUNT(*) FROM notice WHERE user_id = ?1 AND read_at IS NULL")
        .bind(user_id)
        .fetch_one(pool)
        .await
        .context("counting unread notices")
}

/// Marks one of the recipient's own notices read. Returns false when the
/// notice does not exist or belongs to someone else — indistinguishably.
pub async fn mark_read(pool: &SqlitePool, user_id: i64, notice_id: i64) -> Result<bool> {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let result = sqlx::query(
        "UPDATE notice SET read_at = ?1
         WHERE id = ?2 AND user_id = ?3 AND read_at IS NULL",
    )
    .bind(now)
    .bind(notice_id)
    .bind(user_id)
    .execute(pool)
    .await
    .context("marking notice read")?;
    Ok(result.rows_affected() == 1)
}
