//! Who may act on an evaluation draft (ADR 0008; #38): one owner for
//! the draft authorization gates.
//!
//! Contributor identity, authoring scope, the read rule (contributors,
//! everyone the enrollment history is open to, the trainee's own
//! finalized record, and reviewers once submitted), and who may start
//! a session's draft. Services refuse on these answers; handlers never
//! restate them. Extracted from `evaluation_drafts` when that hub
//! crossed the reorganization threshold (AGENTS.md).

use anyhow::{Context, Result};
use sqlx::{SqliteConnection, SqlitePool};

use crate::assignments;
use crate::capabilities::{self, Capability};
use crate::evaluation_drafts::RecordRow;
use crate::lifecycle;

/// Whether `user_id` is a contributor to the record: its current owner,
/// an actor of created/contributed/submitted events, or an ownership
/// recipient. A coordinator who only moved ownership between others is
/// not one. Self-review turns on this (ADR 0008).
pub(crate) async fn is_contributor(
    conn: &mut SqliteConnection,
    record_id: i64,
    user_id: i64,
) -> Result<bool> {
    let owner: i64 =
        sqlx::query_scalar("SELECT owner_user_id FROM evaluation_record WHERE id = ?1")
            .bind(record_id)
            .fetch_one(&mut *conn)
            .await
            .context("reading owner")?;
    if owner == user_id {
        return Ok(true);
    }
    let touched: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM contributor_event
         WHERE evaluation_record_id = ?1
           AND ((kind IN ('created', 'contributed', 'submitted_for_review')
                   AND actor_user_id = ?2)
               OR (kind = 'ownership_transferred' AND to_user_id = ?2))
         LIMIT 1",
    )
    .bind(record_id)
    .bind(user_id)
    .fetch_optional(&mut *conn)
    .await
    .context("checking contribution")?;
    Ok(touched.is_some())
}

/// Whether `user_id` authors within the record's scope: an active
/// assignment on its enrollment, or membership on a covered session
/// (#22 decision 2 — both grants are real).
pub(crate) async fn in_scope(pool: &SqlitePool, user_id: i64, record: &RecordRow) -> Result<bool> {
    if assignments::is_assigned(pool, user_id, record.enrollment_id).await? {
        return Ok(true);
    }
    let member: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM session_trainer st
         JOIN evaluation_session es ON es.training_session_id = st.session_id
         WHERE es.evaluation_record_id = ?1 AND st.trainer_user_id = ?2",
    )
    .bind(record.id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .context("checking covered-session membership")?;
    Ok(member.is_some())
}

/// Whether the actor may write this draft: a coordinator, or an
/// evaluation author within the record's scope.
pub(crate) async fn may_contribute(
    pool: &SqlitePool,
    actor_user_id: i64,
    record: &RecordRow,
) -> Result<bool> {
    if capabilities::user_has(pool, actor_user_id, Capability::AssignTraining).await? {
        return Ok(true);
    }
    Ok(
        capabilities::user_has(pool, actor_user_id, Capability::AuthorEvaluation).await?
            && in_scope(pool, actor_user_id, record).await?,
    )
}

/// Whether the actor may read this draft: contributors, everyone the
/// enrollment history is open to, the trainee themselves once the
/// record is finalized (`view_own_records`; drafts in progress about
/// them are not theirs to see), and — once the record has been
/// submitted at least once — evaluation reviewers.
pub(crate) async fn may_read(
    pool: &SqlitePool,
    actor_user_id: i64,
    record: &RecordRow,
) -> Result<bool> {
    if may_contribute(pool, actor_user_id, record).await? {
        return Ok(true);
    }
    if lifecycle::may_read(pool, actor_user_id, record.enrollment_id).await? {
        return Ok(true);
    }
    if capabilities::user_has(pool, actor_user_id, Capability::ViewOwnRecords).await? {
        let own_finalized: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM enrollment e
             WHERE e.id = ?1 AND e.user_id = ?2
               AND EXISTS (SELECT 1 FROM evaluation_version v
                           WHERE v.evaluation_record_id = ?3)",
        )
        .bind(record.enrollment_id)
        .bind(actor_user_id)
        .bind(record.id)
        .fetch_optional(pool)
        .await
        .context("checking own finalized record")?;
        if own_finalized.is_some() {
            return Ok(true);
        }
    }
    if capabilities::user_has(pool, actor_user_id, Capability::ReviewEvaluation).await? {
        let submitted: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM contributor_event
             WHERE evaluation_record_id = ?1 AND kind = 'submitted_for_review'
             LIMIT 1",
        )
        .bind(record.id)
        .fetch_optional(pool)
        .await
        .context("checking submission history")?;
        return Ok(submitted.is_some());
    }
    Ok(false)
}

/// Whether the actor may start (or offer to start) the session's draft:
/// a coordinator, or an evaluation author who is a member of the session
/// or assigned to its enrollment.
pub(crate) async fn may_start(
    pool: &SqlitePool,
    actor_user_id: i64,
    session_id: i64,
    enrollment_id: i64,
) -> Result<bool> {
    if capabilities::user_has(pool, actor_user_id, Capability::AssignTraining).await? {
        return Ok(true);
    }
    if !capabilities::user_has(pool, actor_user_id, Capability::AuthorEvaluation).await? {
        return Ok(false);
    }
    Ok(
        crate::session_membership::is_member(pool, actor_user_id, session_id).await?
            || assignments::is_assigned(pool, actor_user_id, enrollment_id).await?,
    )
}
