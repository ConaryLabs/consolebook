//! The single-step review workflow (ADR 0008; docs/domain-model.md
//! `ReviewDecision`).
//!
//! Any `review_evaluation` holder who is not a contributor decides a
//! submitted draft: approve, request changes (with a required comment),
//! or return it. Self-review is refused. Approval keeps the working copy
//! frozen for Milestone 4's finalization; a change request takes the
//! second snapshot ADR 0008 names — anchoring what was reviewed — and
//! reopens the copy; a plain return reopens it without one. Every
//! decision is a permanent append-only row paired with a
//! `review_decided` contributor event, audited, and announced to the
//! draft's owner. Migration 0009 holds the shape at the database.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use time::OffsetDateTime;

use crate::audit::{self, EventKind, Subject};
use crate::capabilities::{self, Capability};
use crate::draft_content;
use crate::evaluation_drafts::{self, DraftRefusal, DraftStatus};
use crate::notices::{self, NoticeKind};
use crate::storage;

/// The closed decision set (ADR 0007's pattern for closed sets).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecisionKind {
    Approved,
    ChangesRequested,
    Returned,
}

impl ReviewDecisionKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::ChangesRequested => "changes_requested",
            Self::Returned => "returned",
        }
    }

    fn notice(self) -> NoticeKind {
        match self {
            Self::Approved => NoticeKind::DraftApproved,
            Self::ChangesRequested => NoticeKind::DraftChangesRequested,
            Self::Returned => NoticeKind::DraftReturned,
        }
    }

    fn verdict(self) -> &'static str {
        match self {
            Self::Approved => "was approved",
            Self::ChangesRequested => "needs changes",
            Self::Returned => "was returned",
        }
    }
}

/// Decides a submitted draft. The eligibility and workflow state are
/// rechecked inside the write transaction — and held again by migration
/// 0009's triggers — so a raced resubmission, transfer, or competing
/// decision resolves as its typed refusal.
pub async fn decide(
    pool: &SqlitePool,
    actor_user_id: i64,
    record_id: i64,
    decision: ReviewDecisionKind,
    comment: Option<&str>,
) -> Result<std::result::Result<(), DraftRefusal>> {
    let mut conn = pool.acquire().await.context("acquiring connection")?;
    let Some(record) = evaluation_drafts::load_record(&mut conn, record_id).await? else {
        return Ok(Err(DraftRefusal::NoSuchRecord));
    };
    drop(conn);
    if !capabilities::user_has(pool, actor_user_id, Capability::ReviewEvaluation).await? {
        return Ok(Err(DraftRefusal::CapabilityRequired));
    }
    let comment = comment.map(str::trim).unwrap_or_default();
    if decision == ReviewDecisionKind::ChangesRequested && comment.is_empty() {
        return Ok(Err(DraftRefusal::CommentRequired));
    }

    let mut tx = storage::write_tx(pool).await.context("starting review")?;
    if evaluation_drafts::status_of(&mut tx, record_id).await? != DraftStatus::Submitted {
        return storage::refuse(tx, DraftRefusal::NotSubmitted).await;
    }
    if evaluation_drafts::is_contributor(&mut tx, record_id, actor_user_id).await? {
        return storage::refuse(tx, DraftRefusal::SelfReview).await;
    }
    let now = OffsetDateTime::now_utc().unix_timestamp();
    // The change-request return is ADR 0008's second snapshot point:
    // the copy reopens, anchored to exactly what the reviewer saw. It
    // is taken before the decision row, whose triggers require it.
    if decision == ReviewDecisionKind::ChangesRequested {
        let content = draft_content::content_json(&mut tx, record_id).await?;
        sqlx::query(
            "INSERT INTO draft_snapshot
                 (evaluation_record_id, reason, content, taken_at, taken_by)
             VALUES (?1, 'change_request_return', ?2, ?3, ?4)",
        )
        .bind(record_id)
        .bind(&content)
        .bind(now)
        .bind(actor_user_id)
        .execute(&mut *tx)
        .await
        .context("taking return snapshot")?;
    }
    sqlx::query(
        "INSERT INTO review_decision
             (evaluation_record_id, reviewer_user_id, decision, comment, decided_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(record_id)
    .bind(actor_user_id)
    .bind(decision.as_str())
    .bind(comment)
    .bind(now)
    .execute(&mut *tx)
    .await
    .context("recording decision")?;
    // The paired review_decided event is appended by the database
    // itself (migration 0009's review_decision_advances_workflow), so
    // raw writes advance the workflow exactly as this path does.
    let owner: i64 =
        sqlx::query_scalar("SELECT owner_user_id FROM evaluation_record WHERE id = ?1")
            .bind(record_id)
            .fetch_one(&mut *tx)
            .await
            .context("reading owner")?;
    audit::record_for_subject(
        &mut *tx,
        EventKind::DraftReviewDecided,
        Some(actor_user_id),
        Some(owner),
        Subject::Record(record_id),
    )
    .await?;
    let trainee: String = sqlx::query_scalar(
        "SELECT u.display_name FROM enrollment e
         JOIN user u ON u.id = e.user_id
         WHERE e.id = ?1",
    )
    .bind(record.enrollment_id)
    .fetch_one(&mut *tx)
    .await
    .context("reading trainee name")?;
    notices::notify_user(
        &mut *tx,
        owner,
        decision.notice(),
        &format!(
            "Your draft evaluation for {trainee} {}.",
            decision.verdict()
        ),
    )
    .await?;
    tx.commit().await.context("committing review")?;
    Ok(Ok(()))
}

/// One submitted draft awaiting review, with the caller's eligibility.
#[derive(Debug, Clone, Serialize)]
pub struct QueueRow {
    pub record_id: i64,
    pub trainee_user_id: i64,
    pub trainee_display_name: String,
    pub owner_display_name: String,
    pub program_name: String,
    pub version_number: i64,
    pub submitted_at: i64,
    /// False when the caller is a contributor and self-review applies.
    pub eligible: bool,
}

/// The currently submitted drafts, oldest submission first, gated on
/// `review_evaluation`.
pub async fn queue(
    pool: &SqlitePool,
    actor_user_id: i64,
) -> Result<std::result::Result<Vec<QueueRow>, DraftRefusal>> {
    if !capabilities::user_has(pool, actor_user_id, Capability::ReviewEvaluation).await? {
        return Ok(Err(DraftRefusal::CapabilityRequired));
    }
    let rows = sqlx::query(
        "SELECT r.id, e.user_id AS trainee_user_id,
                tu.display_name AS trainee_display_name,
                ou.display_name AS owner_display_name,
                pv.name AS program_name, pv.version_number,
                (SELECT ce2.recorded_at FROM contributor_event ce2
                 WHERE ce2.evaluation_record_id = r.id
                   AND ce2.kind = 'submitted_for_review'
                 ORDER BY ce2.id DESC LIMIT 1) AS submitted_at,
                CASE WHEN r.owner_user_id = ?1
                        OR EXISTS (
                            SELECT 1 FROM contributor_event ce3
                            WHERE ce3.evaluation_record_id = r.id
                              AND ((ce3.kind IN ('created', 'contributed',
                                                 'submitted_for_review')
                                      AND ce3.actor_user_id = ?1)
                                  OR (ce3.kind = 'ownership_transferred'
                                      AND ce3.to_user_id = ?1))
                        )
                     THEN 0 ELSE 1 END AS eligible
         FROM evaluation_record r
         JOIN enrollment e ON e.id = r.enrollment_id
         JOIN user tu ON tu.id = e.user_id
         JOIN user ou ON ou.id = r.owner_user_id
         JOIN program_version pv ON pv.id = r.program_version_id
         WHERE (SELECT ce.kind FROM contributor_event ce
                WHERE ce.evaluation_record_id = r.id
                ORDER BY ce.id DESC LIMIT 1) = 'submitted_for_review'
         ORDER BY submitted_at, r.id",
    )
    .bind(actor_user_id)
    .fetch_all(pool)
    .await
    .context("listing the review queue")?;
    Ok(Ok(rows
        .iter()
        .map(|row| QueueRow {
            record_id: row.get("id"),
            trainee_user_id: row.get("trainee_user_id"),
            trainee_display_name: row.get("trainee_display_name"),
            owner_display_name: row.get("owner_display_name"),
            program_name: row.get("program_name"),
            version_number: row.get("version_number"),
            submitted_at: row.get("submitted_at"),
            eligible: row.get::<i64, _>("eligible") != 0,
        })
        .collect()))
}
