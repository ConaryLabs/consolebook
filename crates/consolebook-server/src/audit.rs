//! Append-only audit events for security-sensitive actions.
//!
//! The `audit_event` table refuses UPDATE and DELETE at the database level
//! (migration 0002). Events carry no record content, narratives, or secret
//! material — only what happened, when, and to whom.

use anyhow::{Context, Result};
use sqlx::{Executor, Sqlite};
use time::OffsetDateTime;

/// The authentication- and configuration-era event vocabulary. Later
/// milestones extend this with record-lifecycle kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    SetupCompleted,
    LoginSucceeded,
    LoginFailed,
    Logout,
    ResetCodeIssued,
    ResetCodeUsed,
    RecoveryCodeIssued,
    BackupCompleted,
    RestoreCompleted,
    ProgramCreated,
    ProgramVersionCreated,
    ProgramVersionPublished,
    ProgramVersionImported,
    ProgramVersionDiscarded,
    UserCreated,
    EnrollmentCreated,
    EnrollmentWithdrawn,
    EnrollmentCompleted,
    EnrollmentReinstated,
    EnrollmentVersionChanged,
    PhaseEventRecorded,
    AssignmentCreated,
    AssignmentEnded,
    SessionCreated,
    SessionUpdated,
    SessionClosed,
    SessionTrainerAdded,
    SessionTrainerRemoved,
    DraftCreated,
    DraftOwnershipTransferred,
    DraftSubmitted,
    DraftReviewDecided,
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
            Self::BackupCompleted => "backup_completed",
            Self::RestoreCompleted => "restore_completed",
            Self::ProgramCreated => "program_created",
            Self::ProgramVersionCreated => "program_version_created",
            Self::ProgramVersionPublished => "program_version_published",
            Self::ProgramVersionImported => "program_version_imported",
            Self::ProgramVersionDiscarded => "program_version_discarded",
            Self::UserCreated => "user_created",
            Self::EnrollmentCreated => "enrollment_created",
            Self::EnrollmentWithdrawn => "enrollment_withdrawn",
            Self::EnrollmentCompleted => "enrollment_completed",
            Self::EnrollmentReinstated => "enrollment_reinstated",
            Self::EnrollmentVersionChanged => "enrollment_version_changed",
            Self::PhaseEventRecorded => "phase_event_recorded",
            Self::AssignmentCreated => "assignment_created",
            Self::AssignmentEnded => "assignment_ended",
            Self::SessionCreated => "session_created",
            Self::SessionUpdated => "session_updated",
            Self::SessionClosed => "session_closed",
            Self::SessionTrainerAdded => "session_trainer_added",
            Self::SessionTrainerRemoved => "session_trainer_removed",
            Self::DraftCreated => "draft_created",
            Self::DraftOwnershipTransferred => "draft_ownership_transferred",
            Self::DraftSubmitted => "draft_submitted",
            Self::DraftReviewDecided => "draft_review_decided",
        }
    }
}

/// A domain row an event acted on. Stored as (`subject_kind`, `subject_id`)
/// with deliberately no foreign key (migration 0004): the audit trail is
/// append-only and must never block lawful disposition of its subjects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subject {
    Program(i64),
    ProgramVersion(i64),
    Enrollment(i64),
    Assignment(i64),
    Session(i64),
    Record(i64),
}

impl Subject {
    #[must_use]
    pub fn kind_str(self) -> &'static str {
        match self {
            Self::Program(_) => "program",
            Self::ProgramVersion(_) => "program_version",
            Self::Enrollment(_) => "enrollment",
            Self::Assignment(_) => "assignment",
            Self::Session(_) => "session",
            Self::Record(_) => "record",
        }
    }

    #[must_use]
    pub fn id(self) -> i64 {
        match self {
            Self::Program(id)
            | Self::ProgramVersion(id)
            | Self::Enrollment(id)
            | Self::Assignment(id)
            | Self::Session(id)
            | Self::Record(id) => id,
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

/// Records one event about a domain subject, optionally also naming the
/// person it concerns (`subject_user_id`). Callers inside a transaction
/// pass the transaction so the event commits or rolls back with the action
/// it describes.
pub async fn record_for_subject<'e>(
    executor: impl Executor<'e, Database = Sqlite>,
    kind: EventKind,
    actor_user_id: Option<i64>,
    subject_user_id: Option<i64>,
    subject: Subject,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO audit_event
             (occurred_at, kind, actor_user_id, subject_user_id, subject_kind, subject_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(OffsetDateTime::now_utc().unix_timestamp())
    .bind(kind.as_str())
    .bind(actor_user_id)
    .bind(subject_user_id)
    .bind(subject.kind_str())
    .bind(subject.id())
    .execute(executor)
    .await
    .context("recording audit event")?;
    Ok(())
}
