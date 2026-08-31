//! Finalization (ADR 0011; docs/records-integrity.md; #36).
//!
//! An approved draft whose completion rules pass becomes an immutable
//! `EvaluationVersion`: the complete historical presentation as
//! canonical bytes, with a SHA-256 content hash and the
//! domain-separated chain hash. The envelope is built inside the write
//! transaction from committed rows, the completion rules are evaluated
//! against the pinned version's `finalization_policy`, and migration
//! 0010 holds the workflow gate and immutability at the database. The
//! finalized record presents from its stored bytes only.

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{Row, SqliteConnection, SqlitePool};
use time::OffsetDateTime;

use crate::audit::{self, EventKind, Subject};
use crate::canonical;
use crate::capabilities::{self, Capability};
use crate::evaluation_drafts::{self, DraftStatus};
use crate::notices::{self, NoticeKind};
use crate::storage;

/// Typed refusals for the sealing act.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FinalizeRefusal {
    NoSuchRecord,
    CapabilityRequired,
    AlreadyFinalized,
    NotApproved,
    NarrativesIncomplete,
    RatingsIncomplete,
    /// The record changed since the finalizer viewed it; sealing
    /// carries the viewed revision exactly as submission does, so
    /// content nobody reviewed is never made permanent.
    StaleSave,
}

/// The pinned version's completion rules; a missing row fails closed
/// (every rule on), matching migration 0010's trigger.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Policy {
    pub review_approved: bool,
    pub required_narratives: bool,
    pub ratings_complete: bool,
}

pub(crate) async fn policy(conn: &mut SqliteConnection, version_id: i64) -> Result<Policy> {
    let row = sqlx::query(
        "SELECT review_approved, required_narratives, ratings_complete
         FROM finalization_policy WHERE program_version_id = ?1",
    )
    .bind(version_id)
    .fetch_optional(&mut *conn)
    .await
    .context("reading finalization policy")?;
    Ok(row.map_or(
        Policy {
            review_approved: true,
            required_narratives: true,
            ratings_complete: true,
        },
        |row| Policy {
            review_approved: row.get::<i64, _>("review_approved") != 0,
            required_narratives: row.get::<i64, _>("required_narratives") != 0,
            ratings_complete: row.get::<i64, _>("ratings_complete") != 0,
        },
    ))
}

/// Whether a required narrative prompt lacks non-blank text. Shared by
/// submission and finalization: when the rule is on, a draft cannot
/// enter review missing what finalization will demand, so an approved
/// draft is never wedged between a frozen copy and a failing rule
/// (ADR 0011).
pub(crate) async fn narratives_incomplete(
    conn: &mut SqliteConnection,
    record: &evaluation_drafts::RecordRow,
) -> Result<bool> {
    let texts: Vec<(i64, Option<String>)> = sqlx::query_as(
        "SELECT fn.id, dn.text FROM form_narrative fn
         LEFT JOIN draft_narrative dn
             ON dn.form_narrative_id = fn.id AND dn.evaluation_record_id = ?1
         WHERE fn.evaluation_form_id = ?2 AND fn.program_version_id = ?3
           AND fn.required = 1",
    )
    .bind(record.id)
    .bind(record.evaluation_form_id)
    .bind(record.program_version_id)
    .fetch_all(&mut *conn)
    .await
    .context("checking required narratives")?;
    Ok(texts
        .iter()
        .any(|(_, text)| text.as_deref().is_none_or(|text| text.trim().is_empty())))
}

/// Whether a competency that takes a value carries neither one nor the
/// explicit not-observed marker.
pub(crate) async fn ratings_incomplete(
    conn: &mut SqliteConnection,
    record: &evaluation_drafts::RecordRow,
) -> Result<bool> {
    let unrated: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM form_competency fc
         JOIN rating_scale rs ON rs.id = fc.rating_scale_id
         LEFT JOIN draft_rating dr
             ON dr.form_competency_id = fc.id AND dr.evaluation_record_id = ?1
         WHERE fc.evaluation_form_id = ?2 AND fc.program_version_id = ?3
           AND rs.kind != 'narrative_only'
           AND (dr.id IS NULL OR (dr.value IS NULL AND dr.not_observed = 0))
         LIMIT 1",
    )
    .bind(record.id)
    .bind(record.evaluation_form_id)
    .bind(record.program_version_id)
    .fetch_optional(&mut *conn)
    .await
    .context("checking rating completeness")?;
    Ok(unrated.is_some())
}

/// One finalized version's metadata, presented.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VersionMeta {
    pub version_number: i64,
    pub record_schema: i64,
    pub content_hash: String,
    pub chain_hash: String,
    pub finalized_at: i64,
    pub finalized_by: i64,
    pub finalized_by_display_name: String,
}

/// Seals an approved draft into its first immutable version. The
/// caller passes the revision it viewed; a save that landed since is a
/// typed stale refusal, never content sealed sight unseen.
#[allow(clippy::too_many_lines)]
pub async fn finalize(
    pool: &SqlitePool,
    actor_user_id: i64,
    record_id: i64,
    expected_revision: i64,
) -> Result<std::result::Result<VersionMeta, FinalizeRefusal>> {
    let mut conn = pool.acquire().await.context("acquiring connection")?;
    let Some(record) = evaluation_drafts::load_record(&mut conn, record_id).await? else {
        return Ok(Err(FinalizeRefusal::NoSuchRecord));
    };
    drop(conn);
    if !capabilities::user_has(pool, actor_user_id, Capability::ReviewEvaluation).await? {
        return Ok(Err(FinalizeRefusal::CapabilityRequired));
    }

    let mut tx = storage::write_tx(pool)
        .await
        .context("starting finalization")?;
    let finalized: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM evaluation_version WHERE evaluation_record_id = ?1")
            .bind(record_id)
            .fetch_optional(&mut *tx)
            .await
            .context("checking for a finalized version")?;
    if finalized.is_some() {
        return storage::refuse(tx, FinalizeRefusal::AlreadyFinalized).await;
    }
    // The revision is rechecked inside the transaction (the submit
    // contract): under a policy without required review the copy stays
    // editable up to this moment, and a racing save must resolve as a
    // stale refusal rather than be sealed unseen.
    let revision_now: i64 =
        sqlx::query_scalar("SELECT revision FROM evaluation_record WHERE id = ?1")
            .bind(record_id)
            .fetch_one(&mut *tx)
            .await
            .context("rechecking revision")?;
    if expected_revision != revision_now {
        return storage::refuse(tx, FinalizeRefusal::StaleSave).await;
    }
    let policy = policy(&mut tx, record.program_version_id).await?;
    if policy.review_approved
        && evaluation_drafts::status_of(&mut tx, record_id).await? != DraftStatus::Approved
    {
        return storage::refuse(tx, FinalizeRefusal::NotApproved).await;
    }
    if policy.required_narratives && narratives_incomplete(&mut tx, &record).await? {
        return storage::refuse(tx, FinalizeRefusal::NarrativesIncomplete).await;
    }
    if policy.ratings_complete && ratings_incomplete(&mut tx, &record).await? {
        return storage::refuse(tx, FinalizeRefusal::RatingsIncomplete).await;
    }

    let now = OffsetDateTime::now_utc().unix_timestamp();
    let envelope = envelope(&mut tx, &record, record_id, actor_user_id, now, policy).await?;
    let bytes = canonical::canonical_bytes(&envelope)?;
    let content_hash = canonical::content_hash_hex(&bytes);
    let chain_hash = canonical::chain_hash_hex(None, &bytes)?;
    sqlx::query(
        "INSERT INTO evaluation_version
             (evaluation_record_id, version_number, record_schema, canonical_bytes,
              content_hash, chain_hash, predecessor_id, finalized_at, finalized_by)
         VALUES (?1, 1, ?2, ?3, ?4, ?5, NULL, ?6, ?7)",
    )
    .bind(record_id)
    .bind(canonical::RECORD_SCHEMA)
    .bind(&bytes)
    .bind(&content_hash)
    .bind(&chain_hash)
    .bind(now)
    .bind(actor_user_id)
    .execute(&mut *tx)
    .await
    .context("recording the finalized version")?;
    let trainee_id: i64 = sqlx::query_scalar("SELECT user_id FROM enrollment WHERE id = ?1")
        .bind(record.enrollment_id)
        .fetch_one(&mut *tx)
        .await
        .context("reading enrollment")?;
    audit::record_for_subject(
        &mut *tx,
        EventKind::DraftFinalized,
        Some(actor_user_id),
        Some(trainee_id),
        Subject::Record(record_id),
    )
    .await?;
    let (trainee_name, owner): (String, i64) = sqlx::query_as(
        "SELECT u.display_name, r.owner_user_id
         FROM evaluation_record r
         JOIN enrollment e ON e.id = r.enrollment_id
         JOIN user u ON u.id = e.user_id
         WHERE r.id = ?1",
    )
    .bind(record_id)
    .fetch_one(&mut *tx)
    .await
    .context("reading presentation names")?;
    notices::notify_user(
        &mut *tx,
        owner,
        NoticeKind::DraftFinalized,
        &format!("The evaluation for {trainee_name} was finalized."),
    )
    .await?;
    // Acknowledgment is the trainee's act (slice 2): the record they
    // are bound to now exists, so they are told it awaits them.
    notices::notify_user(
        &mut *tx,
        trainee_id,
        NoticeKind::RecordAwaitsAcknowledgment,
        "A finalized evaluation record awaits your acknowledgment.",
    )
    .await?;
    let sealer_name: String = sqlx::query_scalar("SELECT display_name FROM user WHERE id = ?1")
        .bind(actor_user_id)
        .fetch_one(&mut *tx)
        .await
        .context("reading finalizer")?;
    tx.commit().await.context("committing finalization")?;
    Ok(Ok(VersionMeta {
        version_number: 1,
        record_schema: canonical::RECORD_SCHEMA,
        content_hash,
        chain_hash,
        finalized_at: now,
        finalized_by: actor_user_id,
        finalized_by_display_name: sealer_name,
    }))
}

/// Builds the record-schema-1 envelope (ADR 0011) from committed rows.
#[allow(clippy::too_many_lines)]
async fn envelope(
    conn: &mut SqliteConnection,
    record: &evaluation_drafts::RecordRow,
    record_id: i64,
    finalized_by: i64,
    finalized_at: i64,
    policy: Policy,
) -> Result<Value> {
    let instance: String = sqlx::query_scalar("SELECT installation_id FROM instance WHERE id = 1")
        .fetch_one(&mut *conn)
        .await
        .context("reading instance identity")?;
    let trainee = sqlx::query(
        "SELECT u.id, u.username, u.display_name, u.employee_id, u.title
         FROM enrollment e JOIN user u ON u.id = e.user_id WHERE e.id = ?1",
    )
    .bind(record.enrollment_id)
    .fetch_one(&mut *conn)
    .await
    .context("reading trainee")?;
    let program =
        sqlx::query("SELECT name, version_number, label FROM program_version WHERE id = ?1")
            .bind(record.program_version_id)
            .fetch_one(&mut *conn)
            .await
            .context("reading program version")?;
    let form =
        sqlx::query("SELECT name, instructions, record_type FROM evaluation_form WHERE id = ?1")
            .bind(record.evaluation_form_id)
            .fetch_one(&mut *conn)
            .await
            .context("reading form")?;
    let finalizer = sqlx::query("SELECT id, username, display_name FROM user WHERE id = ?1")
        .bind(finalized_by)
        .fetch_one(&mut *conn)
        .await
        .context("reading finalizer")?;

    let attribution_rows = sqlx::query(
        "SELECT ce.kind, ce.recorded_at,
                a.id AS actor_id, a.username AS actor_username,
                a.display_name AS actor_display_name,
                t.id AS to_id, t.username AS to_username,
                t.display_name AS to_display_name
         FROM contributor_event ce
         JOIN user a ON a.id = ce.actor_user_id
         LEFT JOIN user t ON t.id = ce.to_user_id
         WHERE ce.evaluation_record_id = ?1
         ORDER BY ce.id",
    )
    .bind(record_id)
    .fetch_all(&mut *conn)
    .await
    .context("reading attribution")?;
    let attribution: Vec<Value> = attribution_rows
        .iter()
        .map(|row| {
            let to = row
                .get::<Option<i64>, _>("to_id")
                .map_or(Value::Null, |id| {
                    json!({
                        "id": id,
                        "username": row.get::<String, _>("to_username"),
                        "display_name": row.get::<String, _>("to_display_name"),
                    })
                });
            json!({
                "kind": row.get::<String, _>("kind"),
                "actor": {
                    "id": row.get::<i64, _>("actor_id"),
                    "username": row.get::<String, _>("actor_username"),
                    "display_name": row.get::<String, _>("actor_display_name"),
                },
                "to": to,
                "recorded_at": row.get::<i64, _>("recorded_at"),
            })
        })
        .collect();

    let review_rows = sqlx::query(
        "SELECT rd.decision, rd.comment, rd.decided_at,
                u.id, u.username, u.display_name
         FROM review_decision rd JOIN user u ON u.id = rd.reviewer_user_id
         WHERE rd.evaluation_record_id = ?1 ORDER BY rd.id",
    )
    .bind(record_id)
    .fetch_all(&mut *conn)
    .await
    .context("reading decisions")?;
    let review: Vec<Value> = review_rows
        .iter()
        .map(|row| {
            json!({
                "reviewer": {
                    "id": row.get::<i64, _>("id"),
                    "username": row.get::<String, _>("username"),
                    "display_name": row.get::<String, _>("display_name"),
                },
                "decision": row.get::<String, _>("decision"),
                "comment": row.get::<String, _>("comment"),
                "decided_at": row.get::<i64, _>("decided_at"),
            })
        })
        .collect();

    let task_rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT competency_id, prompt FROM task
         WHERE program_version_id = ?1 ORDER BY competency_id, sort_order, id",
    )
    .bind(record.program_version_id)
    .fetch_all(&mut *conn)
    .await
    .context("reading tasks")?;
    let anchor_rows: Vec<(i64, i64, String, String)> = sqlx::query_as(
        "SELECT rating_scale_id, value, label, definition FROM rating_anchor
         WHERE program_version_id = ?1 ORDER BY rating_scale_id, value",
    )
    .bind(record.program_version_id)
    .fetch_all(&mut *conn)
    .await
    .context("reading anchors")?;
    let modifier_rows = sqlx::query(
        "SELECT drm.draft_rating_id, rm.code, rm.label, rm.description
         FROM draft_rating_modifier drm
         JOIN rating_modifier rm ON rm.id = drm.rating_modifier_id
         JOIN draft_rating dr ON dr.id = drm.draft_rating_id
         WHERE dr.evaluation_record_id = ?1
         ORDER BY drm.draft_rating_id, rm.code",
    )
    .bind(record_id)
    .fetch_all(&mut *conn)
    .await
    .context("reading rating modifiers")?;
    let rating_rows = sqlx::query(
        "SELECT fc.competency_id, c.category, c.name, c.description,
                rs.id AS scale_id, rs.name AS scale_name, rs.kind AS scale_kind,
                rs.min_value, rs.max_value,
                dr.id AS rating_id, dr.value, dr.not_observed
         FROM form_competency fc
         JOIN competency c ON c.id = fc.competency_id
         JOIN rating_scale rs ON rs.id = fc.rating_scale_id
         LEFT JOIN draft_rating dr
             ON dr.form_competency_id = fc.id AND dr.evaluation_record_id = ?1
         WHERE fc.evaluation_form_id = ?2 AND fc.program_version_id = ?3
         ORDER BY fc.sort_order, fc.id",
    )
    .bind(record_id)
    .bind(record.evaluation_form_id)
    .bind(record.program_version_id)
    .fetch_all(&mut *conn)
    .await
    .context("reading ratings")?;
    let ratings: Vec<Value> = rating_rows
        .iter()
        .map(|row| {
            let competency_id: i64 = row.get("competency_id");
            let scale_id: i64 = row.get("scale_id");
            let rating_id: Option<i64> = row.get("rating_id");
            let tasks: Vec<Value> = task_rows
                .iter()
                .filter(|(owner, _)| *owner == competency_id)
                .map(|(_, prompt)| Value::String(prompt.clone()))
                .collect();
            let anchors: Vec<Value> = anchor_rows
                .iter()
                .filter(|(owner, ..)| *owner == scale_id)
                .map(|(_, value, label, definition)| {
                    json!({ "value": value, "label": label, "definition": definition })
                })
                .collect();
            let modifiers: Vec<Value> = modifier_rows
                .iter()
                .filter(|m| Some(m.get::<i64, _>("draft_rating_id")) == rating_id)
                .map(|m| {
                    json!({
                        "code": m.get::<String, _>("code"),
                        "label": m.get::<String, _>("label"),
                        "description": m.get::<String, _>("description"),
                    })
                })
                .collect();
            json!({
                "competency": {
                    "category": row.get::<String, _>("category"),
                    "name": row.get::<String, _>("name"),
                    "description": row.get::<String, _>("description"),
                    "tasks": tasks,
                },
                "scale": {
                    "name": row.get::<String, _>("scale_name"),
                    "kind": row.get::<String, _>("scale_kind"),
                    "min_value": row.get::<Option<i64>, _>("min_value"),
                    "max_value": row.get::<Option<i64>, _>("max_value"),
                    "anchors": anchors,
                },
                "value": row.get::<Option<i64>, _>("value"),
                "not_observed": row.get::<Option<i64>, _>("not_observed").unwrap_or(0) != 0,
                "modifiers": modifiers,
            })
        })
        .collect();

    let narrative_rows = sqlx::query(
        "SELECT fn.prompt, fn.required, dn.text
         FROM form_narrative fn
         LEFT JOIN draft_narrative dn
             ON dn.form_narrative_id = fn.id AND dn.evaluation_record_id = ?1
         WHERE fn.evaluation_form_id = ?2 AND fn.program_version_id = ?3
         ORDER BY fn.sort_order, fn.id",
    )
    .bind(record_id)
    .bind(record.evaluation_form_id)
    .bind(record.program_version_id)
    .fetch_all(&mut *conn)
    .await
    .context("reading narratives")?;
    let narratives: Vec<Value> = narrative_rows
        .iter()
        .map(|row| {
            json!({
                "prompt": row.get::<String, _>("prompt"),
                "required": row.get::<i64, _>("required") != 0,
                "text": row.get::<Option<String>, _>("text"),
            })
        })
        .collect();

    let session_rows = sqlx::query(
        "SELECT ts.id, ts.business_date, ts.timezone, ts.local_start, ts.local_end,
                ts.utc_start, ts.utc_end, ts.disposition,
                p.name AS phase_name, p.presentation_number
         FROM evaluation_session es
         JOIN training_session ts ON ts.id = es.training_session_id
         LEFT JOIN phase p ON p.id = ts.phase_id
         WHERE es.evaluation_record_id = ?1
         ORDER BY ts.utc_start, ts.id",
    )
    .bind(record_id)
    .fetch_all(&mut *conn)
    .await
    .context("reading covered sessions")?;
    let mut sessions: Vec<Value> = Vec::with_capacity(session_rows.len());
    for row in &session_rows {
        let session_id: i64 = row.get("id");
        let trainer_rows = sqlx::query(
            "SELECT u.id, u.username, u.display_name
             FROM session_trainer st JOIN user u ON u.id = st.trainer_user_id
             WHERE st.session_id = ?1 ORDER BY st.id",
        )
        .bind(session_id)
        .fetch_all(&mut *conn)
        .await
        .context("reading session trainers")?;
        let trainers: Vec<Value> = trainer_rows
            .iter()
            .map(|t| {
                json!({
                    "id": t.get::<i64, _>("id"),
                    "username": t.get::<String, _>("username"),
                    "display_name": t.get::<String, _>("display_name"),
                })
            })
            .collect();
        let phase = row
            .get::<Option<String>, _>("phase_name")
            .map_or(Value::Null, |name| {
                json!({
                    "name": name,
                    "presentation_number": row.get::<i64, _>("presentation_number"),
                })
            });
        sessions.push(json!({
            "business_date": row.get::<String, _>("business_date"),
            "timezone": row.get::<String, _>("timezone"),
            "local_start": row.get::<String, _>("local_start"),
            "local_end": row.get::<Option<String>, _>("local_end"),
            "utc_start": row.get::<i64, _>("utc_start"),
            "utc_end": row.get::<Option<i64>, _>("utc_end"),
            "disposition": row.get::<Option<String>, _>("disposition"),
            "phase": phase,
            "trainers": trainers,
        }));
    }

    Ok(json!({
        "attachments": [],
        "attribution": attribution,
        "canonicalization": canonical::CANONICALIZATION,
        "content": { "narratives": narratives, "ratings": ratings },
        "finalization": {
            "finalized_at": finalized_at,
            "finalized_by": {
                "id": finalizer.get::<i64, _>("id"),
                "username": finalizer.get::<String, _>("username"),
                "display_name": finalizer.get::<String, _>("display_name"),
            },
            "policy": {
                "review_approved": policy.review_approved,
                "required_narratives": policy.required_narratives,
                "ratings_complete": policy.ratings_complete,
            },
        },
        "form": {
            "name": form.get::<String, _>("name"),
            "instructions": form.get::<String, _>("instructions"),
            "record_type": form.get::<String, _>("record_type"),
        },
        "instance": instance,
        "program": {
            "name": program.get::<String, _>("name"),
            "version_number": program.get::<i64, _>("version_number"),
            "label": program.get::<String, _>("label"),
        },
        "record": {
            "id": record_id,
            "version_number": 1,
            "record_schema": canonical::RECORD_SCHEMA,
            "predecessor_content_hash": Value::Null,
        },
        "review": review,
        "sessions": sessions,
        "trainee": {
            "id": trainee.get::<i64, _>("id"),
            "username": trainee.get::<String, _>("username"),
            "display_name": trainee.get::<String, _>("display_name"),
            "employee_id": trainee.get::<String, _>("employee_id"),
            "title": trainee.get::<String, _>("title"),
        },
    }))
}

/// The stored version with its envelope, for readers the draft rules
/// already admit.
#[derive(Debug, Serialize)]
pub struct FinalizedView {
    pub meta: VersionMeta,
    pub envelope: Value,
}

/// Reads the finalized version. Access follows the draft's read rule:
/// finalized records were submitted, so reviewers and contributors see
/// them alike.
pub async fn finalized_view(
    pool: &SqlitePool,
    actor_user_id: i64,
    record_id: i64,
) -> Result<std::result::Result<Option<FinalizedView>, FinalizeRefusal>> {
    let mut conn = pool.acquire().await.context("acquiring connection")?;
    let Some(record) = evaluation_drafts::load_record(&mut conn, record_id).await? else {
        return Ok(Err(FinalizeRefusal::NoSuchRecord));
    };
    drop(conn);
    if !evaluation_drafts::may_read(pool, actor_user_id, &record).await? {
        return Ok(Err(FinalizeRefusal::CapabilityRequired));
    }
    let Some(row) = sqlx::query(
        "SELECT v.version_number, v.record_schema, v.canonical_bytes,
                v.content_hash, v.chain_hash, v.finalized_at, v.finalized_by,
                u.display_name
         FROM evaluation_version v JOIN user u ON u.id = v.finalized_by
         WHERE v.evaluation_record_id = ?1
         ORDER BY v.version_number DESC LIMIT 1",
    )
    .bind(record_id)
    .fetch_optional(pool)
    .await
    .context("reading the finalized version")?
    else {
        return Ok(Ok(None));
    };
    let bytes: Vec<u8> = row.get("canonical_bytes");
    let envelope: Value =
        serde_json::from_slice(&bytes).context("parsing stored canonical bytes")?;
    Ok(Ok(Some(FinalizedView {
        meta: VersionMeta {
            version_number: row.get("version_number"),
            record_schema: row.get("record_schema"),
            content_hash: row.get("content_hash"),
            chain_hash: row.get("chain_hash"),
            finalized_at: row.get("finalized_at"),
            finalized_by: row.get("finalized_by"),
            finalized_by_display_name: row.get("display_name"),
        },
        envelope,
    })))
}

/// Hash verification over the stored bytes, with honest wording left
/// to the caller: consistency, never tamper-proofing
/// (`docs/records-integrity.md`; ADR 0010, 0011).
#[derive(Debug, Serialize)]
pub struct Verification {
    pub content_hash_ok: bool,
    pub chain_hash_ok: bool,
}

/// Recomputes both fingerprints from the stored canonical bytes and
/// compares them with the stored values.
pub async fn verify(
    pool: &SqlitePool,
    actor_user_id: i64,
    record_id: i64,
) -> Result<std::result::Result<Option<Verification>, FinalizeRefusal>> {
    let mut conn = pool.acquire().await.context("acquiring connection")?;
    let Some(record) = evaluation_drafts::load_record(&mut conn, record_id).await? else {
        return Ok(Err(FinalizeRefusal::NoSuchRecord));
    };
    drop(conn);
    if !evaluation_drafts::may_read(pool, actor_user_id, &record).await? {
        return Ok(Err(FinalizeRefusal::CapabilityRequired));
    }
    let Some(row) = sqlx::query(
        "SELECT canonical_bytes, content_hash, chain_hash, predecessor_id
         FROM evaluation_version WHERE evaluation_record_id = ?1
         ORDER BY version_number DESC LIMIT 1",
    )
    .bind(record_id)
    .fetch_optional(pool)
    .await
    .context("reading the finalized version")?
    else {
        return Ok(Ok(None));
    };
    let bytes: Vec<u8> = row.get("canonical_bytes");
    let stored_content: String = row.get("content_hash");
    let stored_chain: String = row.get("chain_hash");
    let predecessor: Option<i64> = row.get("predecessor_id");
    let content_ok = canonical::content_hash_hex(&bytes) == stored_content;
    // Slice 1 produces first versions only; slice 3's amendments carry
    // the predecessor hash through this recomputation.
    let chain_ok =
        predecessor.is_none() && canonical::chain_hash_hex(None, &bytes)? == stored_chain;
    Ok(Ok(Some(Verification {
        content_hash_ok: content_ok,
        chain_hash_ok: chain_ok,
    })))
}
