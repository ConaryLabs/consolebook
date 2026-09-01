//! Acknowledgments and the trainee's own-records timeline
//! (docs/domain-model.md Acknowledgment; Milestone 4 slice 2).
//!
//! An acknowledgment binds the trainee to one finalized
//! `EvaluationVersion` and records receipt, not agreement: the trainee
//! acknowledges, acknowledges with a response, or refuses; a
//! `review_evaluation` holder attests a refusal or unavailability on
//! the trainee's behalf. Each is a permanent append-only row — one per
//! version per person — audited, with a refusal escalated to the
//! reviewers who can act on it. Migration 0011 holds the shape at the
//! database: who may be bound, who may speak, and what text each kind
//! carries.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqliteConnection, SqlitePool};
use time::OffsetDateTime;

use crate::audit::{self, EventKind, Subject};
use crate::capabilities::{self, Capability};
use crate::evaluation_drafts;
use crate::notices::{self, NoticeKind};
use crate::storage;

/// Typed refusals for the acknowledgment workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AckRefusal {
    NoSuchRecord,
    /// Acknowledgments reference a specific finalized version
    /// (domain invariant 2); an unfinalized record has none.
    NotFinalized,
    CapabilityRequired,
    /// The trainee kinds are the trainee's own act on their own record.
    NotYourRecord,
    /// The attested kinds are someone else's statement about the
    /// trainee, never self-recorded.
    SelfAttestation,
    AlreadyAcknowledged,
    ResponseRequired,
    /// A plain acknowledgment carries no text; a response makes it
    /// `acknowledged_with_response`.
    ResponseNotAllowed,
}

/// The kinds a trainee records themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraineeAckKind {
    Acknowledged,
    AcknowledgedWithResponse,
    Refused,
}

impl TraineeAckKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Acknowledged => "acknowledged",
            Self::AcknowledgedWithResponse => "acknowledged_with_response",
            Self::Refused => "refused",
        }
    }
}

/// The kinds recorded about the trainee by someone with review
/// authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttestedKind {
    SupervisorAttestedRefusal,
    Unavailable,
}

impl AttestedKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SupervisorAttestedRefusal => "supervisor_attested_refusal",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Every acknowledgment kind, as stored: the trainee's own kinds and the
/// attested ones together (migration 0011's closed set). Readers that
/// meet a stored kind — exports and packets — parse into this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AckKind {
    Acknowledged,
    AcknowledgedWithResponse,
    Refused,
    SupervisorAttestedRefusal,
    Unavailable,
}

impl AckKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Acknowledged => "acknowledged",
            Self::AcknowledgedWithResponse => "acknowledged_with_response",
            Self::Refused => "refused",
            Self::SupervisorAttestedRefusal => "supervisor_attested_refusal",
            Self::Unavailable => "unavailable",
        }
    }

    /// Whether the kind is the trainee's own act, recorded by the
    /// trainee, rather than someone else's statement about them
    /// (migration 0011's who-speaks rule).
    #[must_use]
    pub fn spoken_by_trainee(self) -> bool {
        matches!(
            self,
            Self::Acknowledged | Self::AcknowledgedWithResponse | Self::Refused
        )
    }
}

/// One recorded acknowledgment, presented.
#[derive(Debug, Clone, Serialize)]
pub struct AckView {
    pub kind: String,
    pub response: String,
    pub user_display_name: String,
    pub recorded_by: i64,
    pub recorded_by_display_name: String,
    pub recorded_at: i64,
}

/// The latest finalized version and the record's trainee, loaded inside
/// the write transaction so the binding holds under races.
struct VersionRow {
    version_id: i64,
    trainee_user_id: i64,
}

async fn latest_version(conn: &mut SqliteConnection, record_id: i64) -> Result<Option<VersionRow>> {
    let row = sqlx::query(
        "SELECT v.id, e.user_id
         FROM evaluation_version v
         JOIN evaluation_record r ON r.id = v.evaluation_record_id
         JOIN enrollment e ON e.id = r.enrollment_id
         WHERE v.evaluation_record_id = ?1
         ORDER BY v.version_number DESC LIMIT 1",
    )
    .bind(record_id)
    .fetch_optional(&mut *conn)
    .await
    .context("reading the finalized version")?;
    Ok(row.map(|row| VersionRow {
        version_id: row.get(0),
        trainee_user_id: row.get(1),
    }))
}

async fn already_acknowledged(
    conn: &mut SqliteConnection,
    version_id: i64,
    user_id: i64,
) -> Result<bool> {
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM acknowledgment
         WHERE evaluation_version_id = ?1 AND user_id = ?2",
    )
    .bind(version_id)
    .bind(user_id)
    .fetch_optional(&mut *conn)
    .await
    .context("checking for an acknowledgment")?;
    Ok(existing.is_some())
}

/// Records the trainee's own acknowledgment of their finalized record:
/// plain receipt, receipt with a response, or refusal. A response is a
/// persisted notice to the record's owner; a refusal is escalated to
/// every `review_evaluation` holder, who can attest it.
#[allow(clippy::too_many_lines)]
pub async fn acknowledge(
    pool: &SqlitePool,
    actor_user_id: i64,
    record_id: i64,
    kind: TraineeAckKind,
    response: &str,
) -> Result<std::result::Result<(), AckRefusal>> {
    let mut conn = pool.acquire().await.context("acquiring connection")?;
    if evaluation_drafts::load_record(&mut conn, record_id)
        .await?
        .is_none()
    {
        return Ok(Err(AckRefusal::NoSuchRecord));
    }
    drop(conn);
    if !capabilities::user_has(pool, actor_user_id, Capability::AcknowledgeOwnRecord).await? {
        return Ok(Err(AckRefusal::CapabilityRequired));
    }
    let response = response.trim();
    match kind {
        TraineeAckKind::Acknowledged if !response.is_empty() => {
            return Ok(Err(AckRefusal::ResponseNotAllowed));
        }
        TraineeAckKind::AcknowledgedWithResponse | TraineeAckKind::Refused
            if response.is_empty() =>
        {
            return Ok(Err(AckRefusal::ResponseRequired));
        }
        _ => {}
    }

    let mut tx = storage::write_tx(pool)
        .await
        .context("starting acknowledgment")?;
    let Some(version) = latest_version(&mut tx, record_id).await? else {
        return storage::refuse(tx, AckRefusal::NotFinalized).await;
    };
    if version.trainee_user_id != actor_user_id {
        return storage::refuse(tx, AckRefusal::NotYourRecord).await;
    }
    if already_acknowledged(&mut tx, version.version_id, actor_user_id).await? {
        return storage::refuse(tx, AckRefusal::AlreadyAcknowledged).await;
    }
    let now = OffsetDateTime::now_utc().unix_timestamp();
    // The permanent act snapshots the speaker's presentation name as of
    // the act; a later profile rename never rewrites it.
    let trainee_name: String = sqlx::query_scalar("SELECT display_name FROM user WHERE id = ?1")
        .bind(actor_user_id)
        .fetch_one(&mut *tx)
        .await
        .context("reading trainee name")?;
    sqlx::query(
        "INSERT INTO acknowledgment
             (evaluation_version_id, user_id, kind, response, recorded_by,
              user_display_name, recorded_by_display_name, recorded_at)
         VALUES (?1, ?2, ?3, ?4, ?2, ?5, ?5, ?6)",
    )
    .bind(version.version_id)
    .bind(actor_user_id)
    .bind(kind.as_str())
    .bind(response)
    .bind(&trainee_name)
    .bind(now)
    .execute(&mut *tx)
    .await
    .context("recording acknowledgment")?;
    audit::record_for_subject(
        &mut *tx,
        EventKind::AcknowledgmentRecorded,
        Some(actor_user_id),
        Some(actor_user_id),
        Subject::Record(record_id),
    )
    .await?;
    // Responses and refusals are persisted notices, never dependent on
    // someone reopening the record (docs/architecture.md Notifications).
    // Messages name the person, never record content.
    match kind {
        TraineeAckKind::Acknowledged => {}
        // The response reaches the record's owner, who acts on trainee
        // feedback about their evaluation.
        TraineeAckKind::AcknowledgedWithResponse => {
            let owner: i64 =
                sqlx::query_scalar("SELECT owner_user_id FROM evaluation_record WHERE id = ?1")
                    .bind(record_id)
                    .fetch_one(&mut *tx)
                    .await
                    .context("reading owner")?;
            notices::notify_user(
                &mut *tx,
                owner,
                NoticeKind::AcknowledgmentResponse,
                &format!(
                    "{trainee_name} acknowledged a finalized evaluation record with a response."
                ),
            )
            .await?;
        }
        // The refusal reaches everyone who can attest it.
        TraineeAckKind::Refused => {
            sqlx::query(
                "INSERT INTO notice (user_id, kind, message, created_at)
                 SELECT cg.user_id, ?1, ?2, ?3 FROM capability_grant cg
                 WHERE cg.capability = ?4",
            )
            .bind(NoticeKind::AcknowledgmentRefused.as_str())
            .bind(format!(
                "{trainee_name} refused to acknowledge a finalized evaluation record."
            ))
            .bind(now)
            .bind(Capability::ReviewEvaluation.as_str())
            .execute(&mut *tx)
            .await
            .context("escalating refusal")?;
        }
    }
    tx.commit().await.context("committing acknowledgment")?;
    Ok(Ok(()))
}

/// Records an attested acknowledgment about the trainee — a
/// supervisor-attested refusal or unavailability — by a
/// `review_evaluation` holder who is not the trainee.
pub async fn attest(
    pool: &SqlitePool,
    actor_user_id: i64,
    record_id: i64,
    kind: AttestedKind,
    reason: &str,
) -> Result<std::result::Result<(), AckRefusal>> {
    let mut conn = pool.acquire().await.context("acquiring connection")?;
    if evaluation_drafts::load_record(&mut conn, record_id)
        .await?
        .is_none()
    {
        return Ok(Err(AckRefusal::NoSuchRecord));
    }
    drop(conn);
    if !capabilities::user_has(pool, actor_user_id, Capability::ReviewEvaluation).await? {
        return Ok(Err(AckRefusal::CapabilityRequired));
    }
    let reason = reason.trim();
    if reason.is_empty() {
        return Ok(Err(AckRefusal::ResponseRequired));
    }

    let mut tx = storage::write_tx(pool)
        .await
        .context("starting attestation")?;
    let Some(version) = latest_version(&mut tx, record_id).await? else {
        return storage::refuse(tx, AckRefusal::NotFinalized).await;
    };
    if version.trainee_user_id == actor_user_id {
        return storage::refuse(tx, AckRefusal::SelfAttestation).await;
    }
    if already_acknowledged(&mut tx, version.version_id, version.trainee_user_id).await? {
        return storage::refuse(tx, AckRefusal::AlreadyAcknowledged).await;
    }
    let now = OffsetDateTime::now_utc().unix_timestamp();
    // Both identities snapshot their presentation names as of the act;
    // a later profile rename never rewrites the permanent attestation.
    let (trainee_name, attester_name): (String, String) = {
        let trainee: String = sqlx::query_scalar("SELECT display_name FROM user WHERE id = ?1")
            .bind(version.trainee_user_id)
            .fetch_one(&mut *tx)
            .await
            .context("reading trainee name")?;
        let attester: String = sqlx::query_scalar("SELECT display_name FROM user WHERE id = ?1")
            .bind(actor_user_id)
            .fetch_one(&mut *tx)
            .await
            .context("reading attester name")?;
        (trainee, attester)
    };
    sqlx::query(
        "INSERT INTO acknowledgment
             (evaluation_version_id, user_id, kind, response, recorded_by,
              user_display_name, recorded_by_display_name, recorded_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )
    .bind(version.version_id)
    .bind(version.trainee_user_id)
    .bind(kind.as_str())
    .bind(reason)
    .bind(actor_user_id)
    .bind(&trainee_name)
    .bind(&attester_name)
    .bind(now)
    .execute(&mut *tx)
    .await
    .context("recording attestation")?;
    audit::record_for_subject(
        &mut *tx,
        EventKind::AcknowledgmentRecorded,
        Some(actor_user_id),
        Some(version.trainee_user_id),
        Subject::Record(record_id),
    )
    .await?;
    let statement = match kind {
        AttestedKind::SupervisorAttestedRefusal => {
            "a refusal to acknowledge was attested on your behalf"
        }
        AttestedKind::Unavailable => "you were recorded as unavailable to acknowledge",
    };
    notices::notify_user(
        &mut *tx,
        version.trainee_user_id,
        NoticeKind::AcknowledgmentAttested,
        &format!("On a finalized evaluation record, {statement}."),
    )
    .await?;
    tx.commit().await.context("committing attestation")?;
    Ok(Ok(()))
}

/// The acknowledgment on the record's latest finalized version, for
/// readers the draft rules already admit. `None` while the version is
/// unacknowledged or the record unfinalized.
pub async fn acknowledgment_of(
    pool: &SqlitePool,
    actor_user_id: i64,
    record_id: i64,
) -> Result<std::result::Result<Option<AckView>, AckRefusal>> {
    let mut conn = pool.acquire().await.context("acquiring connection")?;
    let Some(record) = evaluation_drafts::load_record(&mut conn, record_id).await? else {
        return Ok(Err(AckRefusal::NoSuchRecord));
    };
    drop(conn);
    if !crate::draft_access::may_read(pool, actor_user_id, &record).await? {
        return Ok(Err(AckRefusal::CapabilityRequired));
    }
    // Start from the latest version, not from the acknowledgment: a
    // successor version (slice 3) requires its own acknowledgment, so
    // an unacknowledged latest version answers `None` rather than
    // presenting a predecessor's act as current. Names come from the
    // stored snapshots, never a live join a rename could rewrite.
    let row = sqlx::query(
        "SELECT a.kind, a.response, a.recorded_by, a.recorded_at,
                a.user_display_name, a.recorded_by_display_name
         FROM evaluation_version v
         LEFT JOIN acknowledgment a ON a.evaluation_version_id = v.id
         WHERE v.evaluation_record_id = ?1
         ORDER BY v.version_number DESC LIMIT 1",
    )
    .bind(record_id)
    .fetch_optional(pool)
    .await
    .context("reading acknowledgment")?;
    Ok(Ok(row.and_then(|row| {
        row.get::<Option<String>, _>("kind").map(|kind| AckView {
            kind,
            response: row.get("response"),
            user_display_name: row.get("user_display_name"),
            recorded_by: row.get("recorded_by"),
            recorded_by_display_name: row.get("recorded_by_display_name"),
            recorded_at: row.get("recorded_at"),
        })
    })))
}

/// One finalized record on the trainee's own timeline.
#[derive(Debug, Clone, Serialize)]
pub struct TimelineRow {
    pub record_id: i64,
    pub program_name: String,
    pub version_number: i64,
    pub form_name: String,
    /// The earliest covered session's business date (agency-local
    /// meaning), for presentation; ordering uses the finalized instant.
    pub business_date: Option<String>,
    pub finalized_at: i64,
    /// The latest version's number; above 1 the record was amended.
    pub record_version_number: i64,
    pub acknowledgment_kind: Option<String>,
    pub acknowledged_at: Option<i64>,
}

/// The trainee's own finalized records, newest first, gated on
/// `view_own_records`. Drafts in progress about the trainee are not
/// theirs to see; the timeline begins where the record becomes
/// permanent.
pub async fn own_records(
    pool: &SqlitePool,
    actor_user_id: i64,
) -> Result<std::result::Result<Vec<TimelineRow>, AckRefusal>> {
    if !capabilities::user_has(pool, actor_user_id, Capability::ViewOwnRecords).await? {
        return Ok(Err(AckRefusal::CapabilityRequired));
    }
    let rows = sqlx::query(
        "SELECT r.id AS record_id, pv.name AS program_name, pv.version_number,
                f.name AS form_name, v.finalized_at,
                v.version_number AS record_version_number,
                (SELECT MIN(ts.business_date) FROM evaluation_session es
                 JOIN training_session ts ON ts.id = es.training_session_id
                 WHERE es.evaluation_record_id = r.id) AS business_date,
                a.kind AS ack_kind, a.recorded_at AS ack_at
         FROM evaluation_version v
         JOIN evaluation_record r ON r.id = v.evaluation_record_id
         JOIN enrollment e ON e.id = r.enrollment_id
         JOIN program_version pv ON pv.id = r.program_version_id
         JOIN evaluation_form f ON f.id = r.evaluation_form_id
         LEFT JOIN acknowledgment a
             ON a.evaluation_version_id = v.id AND a.user_id = e.user_id
         WHERE e.user_id = ?1
           AND v.version_number = (SELECT MAX(v2.version_number)
                                   FROM evaluation_version v2
                                   WHERE v2.evaluation_record_id = r.id)
         ORDER BY v.finalized_at DESC, r.id DESC",
    )
    .bind(actor_user_id)
    .fetch_all(pool)
    .await
    .context("listing own records")?;
    Ok(Ok(rows
        .iter()
        .map(|row| TimelineRow {
            record_id: row.get("record_id"),
            program_name: row.get("program_name"),
            version_number: row.get("version_number"),
            form_name: row.get("form_name"),
            business_date: row.get("business_date"),
            finalized_at: row.get("finalized_at"),
            record_version_number: row.get("record_version_number"),
            acknowledgment_kind: row.get("ack_kind"),
            acknowledged_at: row.get("ack_at"),
        })
        .collect()))
}
