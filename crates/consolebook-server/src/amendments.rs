//! Amendments and version history (docs/domain-model.md Amendment;
//! Milestone 4 slice 3; #40).
//!
//! A correction never edits a finalized version: opening an amendment
//! — `review_evaluation`, with a required reason — reopens the
//! record's one working copy, the correction travels the ordinary
//! workflow under the pinned version's policy, and sealing produces
//! the successor version linked to its predecessor. The amendment row
//! is both the permanent record of reason and authority and the
//! open-state marker: it targets the record's latest version, and the
//! successor's arrival ends it by derivation. Migration 0012 holds the
//! shape raw; the reopening marks scope every workflow derivation to
//! the reopened cycle.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqliteConnection, SqlitePool};
use time::OffsetDateTime;

use crate::acknowledgments::AckView;
use crate::audit::{self, EventKind, Subject};
use crate::capabilities::{self, Capability};
use crate::evaluation_drafts;
use crate::notices::{self, NoticeKind};
use crate::storage;

/// Typed refusals for opening an amendment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AmendRefusal {
    NoSuchRecord,
    CapabilityRequired,
    /// Amendments correct finalized versions; an unfinalized record's
    /// working copy is already the place to work.
    NotFinalized,
    /// The record is already reopened; one correction cycle at a time.
    AmendmentOpen,
    /// An amendment explains itself (#32 decision 4).
    ReasonRequired,
}

/// The record's open amendment, when its latest version carries one:
/// the reopened cycle's identity and the marks it derives from.
#[derive(Debug, Clone)]
pub(crate) struct OpenAmendment {
    pub reason: String,
    pub opened_by_display_name: String,
    pub opened_at: i64,
    pub predecessor_version_id: i64,
    pub predecessor_version_number: i64,
    pub predecessor_content_hash: String,
    pub opened_after_event_id: i64,
    pub opened_after_decision_id: i64,
}

/// Reads the open amendment: the amendment row targeting the record's
/// latest version. `None` when the record is unfinalized or sealed
/// with no reopening in progress.
pub(crate) async fn open_scope(
    conn: &mut SqliteConnection,
    record_id: i64,
) -> Result<Option<OpenAmendment>> {
    let row = sqlx::query(
        "SELECT a.reason, a.opened_by_display_name, a.opened_at,
                a.opened_after_event_id, a.opened_after_decision_id,
                v.id AS version_id, v.version_number, v.content_hash
         FROM evaluation_version v
         LEFT JOIN amendment a ON a.predecessor_version_id = v.id
         WHERE v.evaluation_record_id = ?1
         ORDER BY v.version_number DESC LIMIT 1",
    )
    .bind(record_id)
    .fetch_optional(&mut *conn)
    .await
    .context("reading open amendment")?;
    Ok(row.and_then(|row| {
        row.get::<Option<String>, _>("reason")
            .map(|reason| OpenAmendment {
                reason,
                opened_by_display_name: row.get("opened_by_display_name"),
                opened_at: row.get("opened_at"),
                predecessor_version_id: row.get("version_id"),
                predecessor_version_number: row.get("version_number"),
                predecessor_content_hash: row.get("content_hash"),
                opened_after_event_id: row.get("opened_after_event_id"),
                opened_after_decision_id: row.get("opened_after_decision_id"),
            })
    }))
}

/// Opens an amendment: reopens the finalized record's working copy for
/// a correction, recording the reason and authority permanently. The
/// copy re-enters the ordinary workflow; sealing it produces the
/// successor version, which requires its own acknowledgment.
#[allow(clippy::too_many_lines)]
pub async fn open(
    pool: &SqlitePool,
    actor_user_id: i64,
    record_id: i64,
    reason: &str,
) -> Result<std::result::Result<(), AmendRefusal>> {
    let mut conn = pool.acquire().await.context("acquiring connection")?;
    if evaluation_drafts::load_record(&mut conn, record_id)
        .await?
        .is_none()
    {
        return Ok(Err(AmendRefusal::NoSuchRecord));
    }
    drop(conn);
    if !capabilities::user_has(pool, actor_user_id, Capability::ReviewEvaluation).await? {
        return Ok(Err(AmendRefusal::CapabilityRequired));
    }
    let reason = reason.trim();
    if reason.is_empty() {
        return Ok(Err(AmendRefusal::ReasonRequired));
    }

    let mut tx = storage::write_tx(pool)
        .await
        .context("starting amendment")?;
    let latest = sqlx::query(
        "SELECT v.id, EXISTS (SELECT 1 FROM amendment a
                              WHERE a.predecessor_version_id = v.id) AS reopened
         FROM evaluation_version v
         WHERE v.evaluation_record_id = ?1
         ORDER BY v.version_number DESC LIMIT 1",
    )
    .bind(record_id)
    .fetch_optional(&mut *tx)
    .await
    .context("reading latest version")?;
    let Some(latest) = latest else {
        return storage::refuse(tx, AmendRefusal::NotFinalized).await;
    };
    if latest.get::<i64, _>("reopened") != 0 {
        return storage::refuse(tx, AmendRefusal::AmendmentOpen).await;
    }
    let latest_version_id: i64 = latest.get("id");
    let event_mark: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(id), 0) FROM contributor_event WHERE evaluation_record_id = ?1",
    )
    .bind(record_id)
    .fetch_one(&mut *tx)
    .await
    .context("reading event mark")?;
    let decision_mark: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(id), 0) FROM review_decision WHERE evaluation_record_id = ?1",
    )
    .bind(record_id)
    .fetch_one(&mut *tx)
    .await
    .context("reading decision mark")?;
    let opener_name: String = sqlx::query_scalar("SELECT display_name FROM user WHERE id = ?1")
        .bind(actor_user_id)
        .fetch_one(&mut *tx)
        .await
        .context("reading opener name")?;
    let now = OffsetDateTime::now_utc().unix_timestamp();
    sqlx::query(
        "INSERT INTO amendment
             (evaluation_record_id, predecessor_version_id, reason, opened_by,
              opened_by_display_name, opened_at, opened_after_event_id,
              opened_after_decision_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )
    .bind(record_id)
    .bind(latest_version_id)
    .bind(reason)
    .bind(actor_user_id)
    .bind(&opener_name)
    .bind(now)
    .bind(event_mark)
    .bind(decision_mark)
    .execute(&mut *tx)
    .await
    .context("recording amendment")?;
    let (trainee_id, trainee_name, owner): (i64, String, i64) = sqlx::query_as(
        "SELECT e.user_id, u.display_name, r.owner_user_id
         FROM evaluation_record r
         JOIN enrollment e ON e.id = r.enrollment_id
         JOIN user u ON u.id = e.user_id
         WHERE r.id = ?1",
    )
    .bind(record_id)
    .fetch_one(&mut *tx)
    .await
    .context("reading record parties")?;
    audit::record_for_subject(
        &mut *tx,
        EventKind::AmendmentOpened,
        Some(actor_user_id),
        Some(trainee_id),
        Subject::Record(record_id),
    )
    .await?;
    notices::notify_user(
        &mut *tx,
        owner,
        NoticeKind::AmendmentOpened,
        &format!("The finalized evaluation for {trainee_name} was reopened for amendment."),
    )
    .await?;
    tx.commit().await.context("committing amendment")?;
    Ok(Ok(()))
}

/// The open amendment as the workspace presents it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmendmentView {
    pub reason: String,
    pub opened_by_display_name: String,
    pub opened_at: i64,
}

/// One finalized version in the record's history, with the amendment
/// that produced it and its acknowledgment. Presentation comes from
/// stored snapshots — the envelope's finalizer, the amendment's and
/// acknowledgment's name snapshots — never a live join a rename could
/// rewrite.
#[derive(Debug, Serialize)]
pub struct VersionHistoryRow {
    pub version_number: i64,
    pub record_schema: i64,
    pub content_hash: String,
    pub chain_hash: String,
    pub finalized_at: i64,
    pub finalized_by_display_name: String,
    /// The amendment that produced this version; `None` for a first
    /// version.
    pub amendment: Option<AmendmentView>,
    pub acknowledgment: Option<AckView>,
}

/// The record's finalized versions, newest first, for readers the
/// draft rules admit.
pub async fn history(
    pool: &SqlitePool,
    actor_user_id: i64,
    record_id: i64,
) -> Result<std::result::Result<Vec<VersionHistoryRow>, AmendRefusal>> {
    let mut conn = pool.acquire().await.context("acquiring connection")?;
    let Some(record) = evaluation_drafts::load_record(&mut conn, record_id).await? else {
        return Ok(Err(AmendRefusal::NoSuchRecord));
    };
    drop(conn);
    if !crate::draft_access::may_read(pool, actor_user_id, &record).await? {
        return Ok(Err(AmendRefusal::CapabilityRequired));
    }
    let rows = sqlx::query(
        "SELECT v.version_number, v.record_schema, v.content_hash, v.chain_hash,
                v.finalized_at,
                json_extract(CAST(v.canonical_bytes AS TEXT),
                             '$.finalization.finalized_by.display_name')
                    AS finalized_by_display_name,
                am.reason AS amendment_reason,
                am.opened_by_display_name AS amendment_opened_by,
                am.opened_at AS amendment_opened_at,
                a.kind AS ack_kind, a.response AS ack_response,
                a.user_display_name AS ack_user_name,
                a.recorded_by AS ack_recorded_by,
                a.recorded_by_display_name AS ack_recorder_name,
                a.recorded_at AS ack_recorded_at
         FROM evaluation_version v
         LEFT JOIN amendment am ON am.predecessor_version_id = v.predecessor_id
         LEFT JOIN acknowledgment a ON a.evaluation_version_id = v.id
         WHERE v.evaluation_record_id = ?1
         ORDER BY v.version_number DESC",
    )
    .bind(record_id)
    .fetch_all(pool)
    .await
    .context("listing version history")?;
    Ok(Ok(rows
        .iter()
        .map(|row| VersionHistoryRow {
            version_number: row.get("version_number"),
            record_schema: row.get("record_schema"),
            content_hash: row.get("content_hash"),
            chain_hash: row.get("chain_hash"),
            finalized_at: row.get("finalized_at"),
            finalized_by_display_name: row
                .get::<Option<String>, _>("finalized_by_display_name")
                .unwrap_or_default(),
            amendment: row
                .get::<Option<String>, _>("amendment_reason")
                .map(|reason| AmendmentView {
                    reason,
                    opened_by_display_name: row.get("amendment_opened_by"),
                    opened_at: row.get("amendment_opened_at"),
                }),
            acknowledgment: row
                .get::<Option<String>, _>("ack_kind")
                .map(|kind| AckView {
                    kind,
                    response: row.get("ack_response"),
                    user_display_name: row.get("ack_user_name"),
                    recorded_by: row.get("ack_recorded_by"),
                    recorded_by_display_name: row.get("ack_recorder_name"),
                    recorded_at: row.get("ack_recorded_at"),
                }),
        })
        .collect()))
}
