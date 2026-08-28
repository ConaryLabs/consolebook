//! Trainer-to-enrollment training assignments (ADR 0008; PRINCIPLES.md 10).
//!
//! An assignment is the durable grant behind assignment-scoped access: a
//! trainer holding `view_assigned_records` reads exactly the enrollments
//! they hold an active assignment for. Creating and ending assignments is
//! coordinator work, gated on `assign_training`. Ending closes the
//! interval in place with attribution — assignments are access grants,
//! not records.

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::{Row, SqlitePool};
use time::OffsetDateTime;

use crate::audit::{self, EventKind, Subject};
use crate::capabilities::{self, Capability};
use crate::lifecycle::{self, EnrollmentStatus};
use crate::notices::{self, NoticeKind};

/// One assignment on an enrollment, with the trainer resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Assignment {
    pub id: i64,
    pub enrollment_id: i64,
    pub trainer_user_id: i64,
    pub trainer_username: String,
    pub trainer_display_name: String,
    pub assigned_at: i64,
    pub assigned_by: Option<i64>,
    pub ended_at: Option<i64>,
    pub ended_by: Option<i64>,
}

/// One of the caller's own active assignments, with the trainee and the
/// pinned version resolved for the "my trainees" view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AssignedTrainee {
    pub assignment_id: i64,
    pub enrollment_id: i64,
    pub trainee_user_id: i64,
    pub trainee_username: String,
    pub trainee_display_name: String,
    pub program_version_id: i64,
    pub program_name: String,
    pub version_number: i64,
    pub version_label: String,
    pub assigned_at: i64,
}

/// Why an assignment operation was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum AssignRefusal {
    CapabilityRequired,
    NoSuchEnrollment,
    /// Assignments attach to active enrollments.
    EnrollmentInactive,
    NoSuchUser,
    AlreadyAssigned,
    NoSuchAssignment,
    AlreadyEnded,
}

/// Whether `user_id` holds an active assignment on `enrollment_id`.
pub async fn is_assigned(pool: &SqlitePool, user_id: i64, enrollment_id: i64) -> Result<bool> {
    let held: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM training_assignment
         WHERE enrollment_id = ?1 AND trainer_user_id = ?2 AND ended_at IS NULL",
    )
    .bind(enrollment_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .context("checking assignment")?;
    Ok(held.is_some())
}

/// Assigns `trainer_user_id` to the enrollment and notifies them, gated
/// on `assign_training`.
pub async fn create(
    pool: &SqlitePool,
    actor_user_id: i64,
    enrollment_id: i64,
    trainer_user_id: i64,
) -> Result<std::result::Result<i64, AssignRefusal>> {
    if !capabilities::user_has(pool, actor_user_id, Capability::AssignTraining).await? {
        return Ok(Err(AssignRefusal::CapabilityRequired));
    }
    let mut tx = pool.begin().await.context("starting assignment")?;
    let Some(status) = lifecycle::status(&mut tx, enrollment_id).await? else {
        return Ok(Err(AssignRefusal::NoSuchEnrollment));
    };
    if status != EnrollmentStatus::Active {
        return Ok(Err(AssignRefusal::EnrollmentInactive));
    }
    let trainer_exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM user WHERE id = ?1")
        .bind(trainer_user_id)
        .fetch_optional(&mut *tx)
        .await
        .context("checking trainer")?;
    if trainer_exists.is_none() {
        return Ok(Err(AssignRefusal::NoSuchUser));
    }
    let duplicate: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM training_assignment
         WHERE enrollment_id = ?1 AND trainer_user_id = ?2 AND ended_at IS NULL",
    )
    .bind(enrollment_id)
    .bind(trainer_user_id)
    .fetch_optional(&mut *tx)
    .await
    .context("checking duplicate assignment")?;
    if duplicate.is_some() {
        return Ok(Err(AssignRefusal::AlreadyAssigned));
    }

    let result = sqlx::query(
        "INSERT INTO training_assignment
             (enrollment_id, trainer_user_id, assigned_at, assigned_by)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(enrollment_id)
    .bind(trainer_user_id)
    .bind(OffsetDateTime::now_utc().unix_timestamp())
    .bind(actor_user_id)
    .execute(&mut *tx)
    .await
    .context("creating assignment")?;
    let assignment_id = result.last_insert_rowid();

    let context_row = sqlx::query(
        "SELECT u.display_name, pv.name AS program_name
         FROM enrollment e
         JOIN user u ON u.id = e.user_id
         JOIN program_version pv ON pv.id = e.program_version_id
         WHERE e.id = ?1",
    )
    .bind(enrollment_id)
    .fetch_one(&mut *tx)
    .await
    .context("reading assignment context")?;
    let trainee_name: String = context_row.get("display_name");
    let program_name: String = context_row.get("program_name");
    notices::notify_user(
        &mut *tx,
        trainer_user_id,
        NoticeKind::AssignmentCreated,
        &format!("New training assignment: {trainee_name} — {program_name}"),
    )
    .await?;

    audit::record_for_subject(
        &mut *tx,
        EventKind::AssignmentCreated,
        Some(actor_user_id),
        Some(trainer_user_id),
        Subject::Assignment(assignment_id),
    )
    .await?;
    tx.commit().await.context("committing assignment")?;
    Ok(Ok(assignment_id))
}

/// Ends an active assignment, gated on `assign_training`.
pub async fn end(
    pool: &SqlitePool,
    actor_user_id: i64,
    assignment_id: i64,
) -> Result<std::result::Result<(), AssignRefusal>> {
    if !capabilities::user_has(pool, actor_user_id, Capability::AssignTraining).await? {
        return Ok(Err(AssignRefusal::CapabilityRequired));
    }
    let mut tx = pool.begin().await.context("starting assignment end")?;
    let ended_at: Option<Option<i64>> =
        sqlx::query_scalar("SELECT ended_at FROM training_assignment WHERE id = ?1")
            .bind(assignment_id)
            .fetch_optional(&mut *tx)
            .await
            .context("checking assignment")?;
    match ended_at {
        None => return Ok(Err(AssignRefusal::NoSuchAssignment)),
        Some(Some(_)) => return Ok(Err(AssignRefusal::AlreadyEnded)),
        Some(None) => {}
    }
    sqlx::query("UPDATE training_assignment SET ended_at = ?1, ended_by = ?2 WHERE id = ?3")
        .bind(OffsetDateTime::now_utc().unix_timestamp())
        .bind(actor_user_id)
        .bind(assignment_id)
        .execute(&mut *tx)
        .await
        .context("ending assignment")?;
    audit::record_for_subject(
        &mut *tx,
        EventKind::AssignmentEnded,
        Some(actor_user_id),
        None,
        Subject::Assignment(assignment_id),
    )
    .await?;
    tx.commit().await.context("committing assignment end")?;
    Ok(Ok(()))
}

/// Every assignment on the enrollment, active first. Capability and scope
/// are the caller's responsibility — this backs the already-gated
/// enrollment detail.
pub async fn list_for_enrollment(pool: &SqlitePool, enrollment_id: i64) -> Result<Vec<Assignment>> {
    let rows = sqlx::query(
        "SELECT ta.id, ta.enrollment_id, ta.trainer_user_id, u.username, u.display_name,
                ta.assigned_at, ta.assigned_by, ta.ended_at, ta.ended_by
         FROM training_assignment ta
         JOIN user u ON u.id = ta.trainer_user_id
         WHERE ta.enrollment_id = ?1
         ORDER BY (ta.ended_at IS NULL) DESC, ta.assigned_at, ta.id",
    )
    .bind(enrollment_id)
    .fetch_all(pool)
    .await
    .context("listing assignments")?;
    Ok(rows
        .iter()
        .map(|row| Assignment {
            id: row.get("id"),
            enrollment_id: row.get("enrollment_id"),
            trainer_user_id: row.get("trainer_user_id"),
            trainer_username: row.get("username"),
            trainer_display_name: row.get("display_name"),
            assigned_at: row.get("assigned_at"),
            assigned_by: row.get("assigned_by"),
            ended_at: row.get("ended_at"),
            ended_by: row.get("ended_by"),
        })
        .collect())
}

/// The caller's own active assignments — their own data, no capability
/// needed beyond a session.
pub async fn list_for_trainer(
    pool: &SqlitePool,
    trainer_user_id: i64,
) -> Result<Vec<AssignedTrainee>> {
    let rows = sqlx::query(
        "SELECT ta.id AS assignment_id, ta.enrollment_id, ta.assigned_at,
                e.user_id AS trainee_user_id, u.username, u.display_name,
                e.program_version_id, pv.name AS program_name, pv.version_number, pv.label
         FROM training_assignment ta
         JOIN enrollment e ON e.id = ta.enrollment_id
         JOIN user u ON u.id = e.user_id
         JOIN program_version pv ON pv.id = e.program_version_id
         WHERE ta.trainer_user_id = ?1 AND ta.ended_at IS NULL
         ORDER BY u.display_name COLLATE NOCASE, ta.id",
    )
    .bind(trainer_user_id)
    .fetch_all(pool)
    .await
    .context("listing own assignments")?;
    Ok(rows
        .iter()
        .map(|row| AssignedTrainee {
            assignment_id: row.get("assignment_id"),
            enrollment_id: row.get("enrollment_id"),
            trainee_user_id: row.get("trainee_user_id"),
            trainee_username: row.get("username"),
            trainee_display_name: row.get("display_name"),
            program_version_id: row.get("program_version_id"),
            program_name: row.get("program_name"),
            version_number: row.get("version_number"),
            version_label: row.get("label"),
            assigned_at: row.get("assigned_at"),
        })
        .collect())
}
