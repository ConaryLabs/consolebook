//! Trainer membership on training sessions (ADR 0008; #22 decision 2).
//!
//! Membership is an access grant, and this module is its owner: every
//! member holds `author_evaluation`, every grant and removal is audited
//! in the same transaction, a session keeps at least one trainer (the
//! database holds the floor), and identity never moves — membership
//! changes are inserts and deletes through these services, never edits.

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::{Row, SqliteConnection, SqlitePool};
use time::OffsetDateTime;

use crate::audit::{self, EventKind, Subject};
use crate::capabilities::{self, Capability};
use crate::training_sessions::SessionRefusal;

/// One trainer on a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionTrainerRow {
    pub user_id: i64,
    pub username: String,
    pub display_name: String,
    pub added_at: i64,
}

/// Whether `user_id` is a trainer on `session_id`.
pub async fn is_member(pool: &SqlitePool, user_id: i64, session_id: i64) -> Result<bool> {
    let held: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM session_trainer WHERE session_id = ?1 AND trainer_user_id = ?2",
    )
    .bind(session_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .context("checking session membership")?;
    Ok(held.is_some())
}

/// Whether the actor may work this session: a coordinator or one of the
/// session's trainers.
pub(crate) async fn may_work(
    pool: &SqlitePool,
    actor_user_id: i64,
    session_id: i64,
) -> Result<bool> {
    if capabilities::user_has(pool, actor_user_id, Capability::AssignTraining).await? {
        return Ok(true);
    }
    is_member(pool, actor_user_id, session_id).await
}

/// Refuses trainers who cannot author evaluations; returns the resolved,
/// deduplicated member list.
pub(crate) async fn validate_trainers(
    tx: &mut SqliteConnection,
    actor_user_id: i64,
    requested: &[i64],
) -> Result<std::result::Result<Vec<i64>, SessionRefusal>> {
    let mut trainers: Vec<i64> = Vec::new();
    let candidates: Vec<i64> = if requested.is_empty() {
        vec![actor_user_id]
    } else {
        requested.to_vec()
    };
    for user_id in candidates {
        if trainers.contains(&user_id) {
            continue;
        }
        let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM user WHERE id = ?1")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await
            .context("checking trainer")?;
        if exists.is_none() {
            return Ok(Err(SessionRefusal::NoSuchUser));
        }
        let can_author: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM capability_grant WHERE user_id = ?1 AND capability = ?2",
        )
        .bind(user_id)
        .bind(Capability::AuthorEvaluation.as_str())
        .fetch_optional(&mut *tx)
        .await
        .context("checking trainer capability")?;
        if can_author.is_none() {
            return Ok(Err(SessionRefusal::TrainerLacksCapability));
        }
        trainers.push(user_id);
    }
    if trainers.is_empty() {
        return Ok(Err(SessionRefusal::NoTrainers));
    }
    Ok(Ok(trainers))
}

/// Inserts one membership grant and its audit record in the caller's
/// transaction. Every access grant is audited, initial members included,
/// so a later removal never outlives the record of the grant.
pub(crate) async fn insert_member(
    tx: &mut SqliteConnection,
    session_id: i64,
    trainer_user_id: i64,
    actor_user_id: i64,
    now: i64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO session_trainer (session_id, trainer_user_id, added_at, added_by)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(session_id)
    .bind(trainer_user_id)
    .bind(now)
    .bind(actor_user_id)
    .execute(&mut *tx)
    .await
    .context("adding session trainer")?;
    audit::record_for_subject(
        &mut *tx,
        EventKind::SessionTrainerAdded,
        Some(actor_user_id),
        Some(trainer_user_id),
        Subject::Session(session_id),
    )
    .await?;
    Ok(())
}

/// Adds an `author_evaluation` holder to the session — the ad-hoc,
/// audited addition #22 decision 2 allows. Coordinators and current
/// members may add.
pub async fn add_trainer(
    pool: &SqlitePool,
    actor_user_id: i64,
    session_id: i64,
    trainer_user_id: i64,
) -> Result<std::result::Result<(), SessionRefusal>> {
    if !may_work(pool, actor_user_id, session_id).await? {
        return Ok(Err(SessionRefusal::CapabilityRequired));
    }
    let mut tx = pool.begin().await.context("starting trainer add")?;
    let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM training_session WHERE id = ?1")
        .bind(session_id)
        .fetch_optional(&mut *tx)
        .await
        .context("checking session")?;
    if exists.is_none() {
        return Ok(Err(SessionRefusal::NoSuchSession));
    }
    let user_exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM user WHERE id = ?1")
        .bind(trainer_user_id)
        .fetch_optional(&mut *tx)
        .await
        .context("checking trainer")?;
    if user_exists.is_none() {
        return Ok(Err(SessionRefusal::NoSuchUser));
    }
    let can_author: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM capability_grant WHERE user_id = ?1 AND capability = ?2")
            .bind(trainer_user_id)
            .bind(Capability::AuthorEvaluation.as_str())
            .fetch_optional(&mut *tx)
            .await
            .context("checking trainer capability")?;
    if can_author.is_none() {
        return Ok(Err(SessionRefusal::TrainerLacksCapability));
    }
    let member: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM session_trainer WHERE session_id = ?1 AND trainer_user_id = ?2",
    )
    .bind(session_id)
    .bind(trainer_user_id)
    .fetch_optional(&mut *tx)
    .await
    .context("checking membership")?;
    if member.is_some() {
        return Ok(Err(SessionRefusal::AlreadyMember));
    }
    let now = OffsetDateTime::now_utc().unix_timestamp();
    insert_member(&mut tx, session_id, trainer_user_id, actor_user_id, now).await?;
    tx.commit().await.context("committing trainer add")?;
    Ok(Ok(()))
}

/// Removes a trainer from a session — a correction, coordinator-only and
/// audited. The database keeps the one-trainer floor.
pub async fn remove_trainer(
    pool: &SqlitePool,
    actor_user_id: i64,
    session_id: i64,
    trainer_user_id: i64,
) -> Result<std::result::Result<(), SessionRefusal>> {
    if !capabilities::user_has(pool, actor_user_id, Capability::AssignTraining).await? {
        return Ok(Err(SessionRefusal::CapabilityRequired));
    }
    let mut tx = pool.begin().await.context("starting trainer removal")?;
    let member: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM session_trainer WHERE session_id = ?1 AND trainer_user_id = ?2",
    )
    .bind(session_id)
    .bind(trainer_user_id)
    .fetch_optional(&mut *tx)
    .await
    .context("checking membership")?;
    if member.is_none() {
        return Ok(Err(SessionRefusal::NotMember));
    }
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM session_trainer WHERE session_id = ?1")
            .bind(session_id)
            .fetch_one(&mut *tx)
            .await
            .context("counting trainers")?;
    if count <= 1 {
        return Ok(Err(SessionRefusal::LastTrainer));
    }
    sqlx::query("DELETE FROM session_trainer WHERE session_id = ?1 AND trainer_user_id = ?2")
        .bind(session_id)
        .bind(trainer_user_id)
        .execute(&mut *tx)
        .await
        .context("removing session trainer")?;
    audit::record_for_subject(
        &mut *tx,
        EventKind::SessionTrainerRemoved,
        Some(actor_user_id),
        Some(trainer_user_id),
        Subject::Session(session_id),
    )
    .await?;
    tx.commit().await.context("committing trainer removal")?;
    Ok(Ok(()))
}

/// The trainer rows for each of `session_ids`, in grant order.
pub(crate) async fn trainers_for(
    conn: &mut SqliteConnection,
    session_ids: &[i64],
) -> Result<std::collections::HashMap<i64, Vec<SessionTrainerRow>>> {
    let mut by_session: std::collections::HashMap<i64, Vec<SessionTrainerRow>> =
        std::collections::HashMap::new();
    for chunk in session_ids {
        let rows = sqlx::query(
            "SELECT st.trainer_user_id, u.username, u.display_name, st.added_at
             FROM session_trainer st
             JOIN user u ON u.id = st.trainer_user_id
             WHERE st.session_id = ?1
             ORDER BY st.added_at, st.id",
        )
        .bind(chunk)
        .fetch_all(&mut *conn)
        .await
        .context("listing session trainers")?;
        by_session.insert(
            *chunk,
            rows.iter()
                .map(|row| SessionTrainerRow {
                    user_id: row.get("trainer_user_id"),
                    username: row.get("username"),
                    display_name: row.get("display_name"),
                    added_at: row.get("added_at"),
                })
                .collect(),
        );
    }
    Ok(by_session)
}
