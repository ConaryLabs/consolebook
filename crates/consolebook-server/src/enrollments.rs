//! Enrollment pinning: one trainee, one published program version
//! (ADR 0007; docs/domain-model.md Enrollment).
//!
//! Lifecycle history — phase transitions, version changes with reason,
//! withdrawal — arrives with Milestone 3. This module owns only creating
//! and listing the pin, gated on `assign_training`; migration 0005 makes
//! the database itself refuse a draft version or a silent version change.

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::{Row, SqlitePool};
use time::OffsetDateTime;

use crate::audit::{self, EventKind, Subject};
use crate::capabilities::{self, Capability};

/// One enrollee of a program version, with presentation fields resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Enrollee {
    pub enrollment_id: i64,
    pub user_id: i64,
    pub username: String,
    pub display_name: String,
    pub enrolled_at: i64,
    pub enrolled_by: Option<i64>,
}

/// Why an enrollment operation was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum EnrollRefusal {
    CapabilityRequired,
    NoSuchVersion,
    /// Enrollments pin published versions, never drafts.
    NotPublished,
    NoSuchUser,
    AlreadyEnrolled,
}

async fn holds_assign_training(pool: &SqlitePool, user_id: i64) -> Result<bool> {
    capabilities::user_has(pool, user_id, Capability::AssignTraining).await
}

/// Enrolls `user_id` in the published version `version_id`.
pub async fn enroll(
    pool: &SqlitePool,
    actor_user_id: i64,
    version_id: i64,
    user_id: i64,
) -> Result<std::result::Result<i64, EnrollRefusal>> {
    if !holds_assign_training(pool, actor_user_id).await? {
        return Ok(Err(EnrollRefusal::CapabilityRequired));
    }
    let mut tx = pool.begin().await.context("starting enrollment")?;
    let version = sqlx::query("SELECT published_at FROM program_version WHERE id = ?1")
        .bind(version_id)
        .fetch_optional(&mut *tx)
        .await
        .context("checking version")?;
    match version {
        None => return Ok(Err(EnrollRefusal::NoSuchVersion)),
        Some(row) => {
            let published_at: Option<i64> = row.get("published_at");
            if published_at.is_none() {
                return Ok(Err(EnrollRefusal::NotPublished));
            }
        }
    }
    let user_exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM user WHERE id = ?1")
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .context("checking user")?;
    if user_exists.is_none() {
        return Ok(Err(EnrollRefusal::NoSuchUser));
    }
    let duplicate: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM enrollment WHERE user_id = ?1 AND program_version_id = ?2",
    )
    .bind(user_id)
    .bind(version_id)
    .fetch_optional(&mut *tx)
    .await
    .context("checking enrollment")?;
    if duplicate.is_some() {
        return Ok(Err(EnrollRefusal::AlreadyEnrolled));
    }
    let result = sqlx::query(
        "INSERT INTO enrollment (user_id, program_version_id, enrolled_at, enrolled_by)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(user_id)
    .bind(version_id)
    .bind(OffsetDateTime::now_utc().unix_timestamp())
    .bind(actor_user_id)
    .execute(&mut *tx)
    .await
    .context("creating enrollment")?;
    let enrollment_id = result.last_insert_rowid();
    audit::record_for_subject(
        &mut *tx,
        EventKind::EnrollmentCreated,
        Some(actor_user_id),
        Some(user_id),
        Subject::Enrollment(enrollment_id),
    )
    .await?;
    tx.commit().await.context("committing enrollment")?;
    Ok(Ok(enrollment_id))
}

/// The enrollees of one version. Enrollment is personnel-adjacent, so
/// reading it takes the same capability as writing it.
pub async fn list_for_version(
    pool: &SqlitePool,
    actor_user_id: i64,
    version_id: i64,
) -> Result<std::result::Result<Vec<Enrollee>, EnrollRefusal>> {
    if !holds_assign_training(pool, actor_user_id).await? {
        return Ok(Err(EnrollRefusal::CapabilityRequired));
    }
    let version_exists: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM program_version WHERE id = ?1")
            .bind(version_id)
            .fetch_optional(pool)
            .await
            .context("checking version")?;
    if version_exists.is_none() {
        return Ok(Err(EnrollRefusal::NoSuchVersion));
    }
    let rows = sqlx::query(
        "SELECT e.id AS enrollment_id, e.user_id, u.username, u.display_name,
                e.enrolled_at, e.enrolled_by
         FROM enrollment e
         JOIN user u ON u.id = e.user_id
         WHERE e.program_version_id = ?1
         ORDER BY u.display_name COLLATE NOCASE, u.username COLLATE NOCASE",
    )
    .bind(version_id)
    .fetch_all(pool)
    .await
    .context("listing enrollments")?;
    Ok(Ok(rows
        .iter()
        .map(|row| Enrollee {
            enrollment_id: row.get("enrollment_id"),
            user_id: row.get("user_id"),
            username: row.get("username"),
            display_name: row.get("display_name"),
            enrolled_at: row.get("enrolled_at"),
            enrolled_by: row.get("enrolled_by"),
        })
        .collect()))
}
