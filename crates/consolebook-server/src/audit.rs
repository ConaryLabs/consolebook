//! Append-only audit events for security-sensitive actions.
//!
//! The `audit_event` table refuses UPDATE and DELETE at the database level
//! (migration 0002). Events carry no record content, narratives, or secret
//! material — only what happened, when, and to whom.

use anyhow::{Context, Result};
use sqlx::{Executor, Sqlite};
use time::OffsetDateTime;

/// The authentication-era event vocabulary. Later milestones extend this
/// with record-lifecycle kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    SetupCompleted,
    LoginSucceeded,
    LoginFailed,
    Logout,
    ResetCodeIssued,
    ResetCodeUsed,
    RecoveryCodeIssued,
}

impl EventKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SetupCompleted => "setup_completed",
            Self::LoginSucceeded => "login_succeeded",
            Self::LoginFailed => "login_failed",
            Self::Logout => "logout",
            Self::ResetCodeIssued => "reset_code_issued",
            Self::ResetCodeUsed => "reset_code_used",
            Self::RecoveryCodeIssued => "recovery_code_issued",
        }
    }
}

/// Records one event. Callers inside a transaction pass the transaction so
/// the event commits or rolls back with the action it describes.
pub async fn record<'e>(
    executor: impl Executor<'e, Database = Sqlite>,
    kind: EventKind,
    actor_user_id: Option<i64>,
    subject_user_id: Option<i64>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO audit_event (occurred_at, kind, actor_user_id, subject_user_id)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(OffsetDateTime::now_utc().unix_timestamp())
    .bind(kind.as_str())
    .bind(actor_user_id)
    .bind(subject_user_id)
    .execute(executor)
    .await
    .context("recording audit event")?;
    Ok(())
}
