//! Evaluation record lifecycle (ADR 0008; docs/domain-model.md
//! `EvaluationRecord`, `ContributorEvent`).
//!
//! An evaluation record is the continuing identity of a daily evaluation:
//! it stamps the covered session's program version, is typed by a pinned
//! `daily_report` form, and carries a current owner. Attribution is a
//! metadata-only append-only contributor stream, and ownership moves only
//! with its recorded event (migration 0008 holds both at the database).
//! One daily draft per training session is v1 policy here, deliberately
//! not schema. Sibling owner: `draft_content` holds the mutable working
//! copy and its vocabulary validation; this module owns creation,
//! ownership transfer, submission (with its content snapshot), the
//! derived workflow status, and the attribution reads.

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::{Row, SqliteConnection, SqlitePool};
use time::OffsetDateTime;

use crate::audit::{self, EventKind, Subject};
use crate::capabilities::{self, Capability};
use crate::draft_content;
use crate::notices::{self, NoticeKind};
use crate::storage;

/// Why a draft operation was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum DraftRefusal {
    CapabilityRequired,
    NoSuchSession,
    /// A cancelled session never happened; it takes no draft.
    SessionCancelled,
    /// v1 policy: one daily draft per training session.
    DraftAlreadyExists,
    /// The stamped version defines no `daily_report` form.
    NoDailyForm,
    /// The stamped version defines several `daily_report` forms; the
    /// caller names one.
    FormRequired,
    /// The named form is not a `daily_report` form of the stamped version.
    NoSuchForm,
    NoSuchRecord,
    /// The draft is submitted for review and frozen.
    DraftSubmitted,
    NoSuchUser,
    /// Recipients author evaluations within the record's scope.
    NotEligible,
    AlreadyOwner,
    /// A content id is not in the record's pinned vocabulary.
    NoSuchFormCompetency,
    NoSuchFormNarrative,
    NoSuchModifier,
    /// The value violates the pinned scale kind or bounds.
    ValueOutOfRange,
    /// Narrative-only scales take no value.
    ValueNotAllowed,
    /// The same competency or narrative appears twice in one save.
    DuplicateEntry,
    /// The save was based on an older revision of the working copy;
    /// another contributor saved first.
    StaleSave,
    /// An approved draft stays frozen until finalization.
    DraftApproved,
    /// A contributor cannot review their own draft (ADR 0008).
    SelfReview,
    /// Reviews decide submitted drafts.
    NotSubmitted,
    /// A change request explains itself.
    CommentRequired,
    /// A finalized record is permanent; corrections are amendments
    /// (Milestone 4 slice 3).
    DraftFinalized,
    /// The pinned version requires every required narrative before
    /// submission or finalization (ADR 0011).
    NarrativesIncomplete,
    /// The pinned version requires every competency rated or marked
    /// not observed before submission or finalization (ADR 0011).
    RatingsIncomplete,
    NoSuchEnrollment,
    /// The pinned version defines no `weekly_summary` form.
    NoSummaryForm,
    /// Daily links belong to weekly summaries (ADR 0013).
    NotASummary,
    /// A summary links finalized daily reports.
    NotADaily,
    /// A summary links its own enrollment's daily reports.
    WrongEnrollment,
    NoSuchVersion,
    DuplicateLink,
    NoSuchLink,
}

/// Workflow status derived from the latest contributor event plus the
/// latest review decision — never stored beside the streams (ADR 0008's
/// enrollment pattern).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DraftStatus {
    Draft,
    Submitted,
    ChangesRequested,
    Returned,
    Approved,
    /// An immutable `EvaluationVersion` exists (ADR 0011); terminal
    /// for the working copy.
    Finalized,
}

impl DraftStatus {
    /// The typed refusal a frozen state answers writes with; editable
    /// states answer `None`.
    pub(crate) fn frozen_refusal(self) -> Option<DraftRefusal> {
        match self {
            Self::Submitted => Some(DraftRefusal::DraftSubmitted),
            Self::Approved => Some(DraftRefusal::DraftApproved),
            Self::Finalized => Some(DraftRefusal::DraftFinalized),
            Self::Draft | Self::ChangesRequested | Self::Returned => None,
        }
    }
}

/// The record row the services act on.
#[derive(Debug, Clone)]
pub(crate) struct RecordRow {
    pub id: i64,
    pub enrollment_id: i64,
    pub program_version_id: i64,
    pub evaluation_form_id: i64,
    pub owner_user_id: i64,
    pub revision: i64,
}

pub(crate) async fn load_record(
    conn: &mut SqliteConnection,
    record_id: i64,
) -> Result<Option<RecordRow>> {
    let row = sqlx::query(
        "SELECT id, enrollment_id, program_version_id, evaluation_form_id,
                owner_user_id, revision
         FROM evaluation_record WHERE id = ?1",
    )
    .bind(record_id)
    .fetch_optional(&mut *conn)
    .await
    .context("reading record")?;
    Ok(row.map(|row| RecordRow {
        id: row.get("id"),
        enrollment_id: row.get("enrollment_id"),
        program_version_id: row.get("program_version_id"),
        evaluation_form_id: row.get("evaluation_form_id"),
        owner_user_id: row.get("owner_user_id"),
        revision: row.get("revision"),
    }))
}

pub(crate) async fn status_of(conn: &mut SqliteConnection, record_id: i64) -> Result<DraftStatus> {
    // A finalized version is terminal unless an amendment has reopened
    // the working copy; the reopened cycle derives from the events and
    // decisions after the amendment's reopening marks (migration 0012).
    let finalized: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM evaluation_version WHERE evaluation_record_id = ?1")
            .bind(record_id)
            .fetch_optional(&mut *conn)
            .await
            .context("checking for a finalized version")?;
    let (event_mark, decision_mark) = if finalized.is_some() {
        match crate::amendments::open_scope(&mut *conn, record_id).await? {
            None => return Ok(DraftStatus::Finalized),
            Some(scope) => (scope.opened_after_event_id, scope.opened_after_decision_id),
        }
    } else {
        (0, 0)
    };
    let latest: Option<String> = sqlx::query_scalar(
        "SELECT kind FROM contributor_event
         WHERE evaluation_record_id = ?1 AND id > ?2
         ORDER BY id DESC LIMIT 1",
    )
    .bind(record_id)
    .bind(event_mark)
    .fetch_optional(&mut *conn)
    .await
    .context("reading latest contributor event")?;
    Ok(match latest.as_deref() {
        Some("submitted_for_review") => DraftStatus::Submitted,
        Some("review_decided") => {
            let decision: Option<String> = sqlx::query_scalar(
                "SELECT decision FROM review_decision
                 WHERE evaluation_record_id = ?1 AND id > ?2
                 ORDER BY id DESC LIMIT 1",
            )
            .bind(record_id)
            .bind(decision_mark)
            .fetch_optional(&mut *conn)
            .await
            .context("reading latest review decision")?;
            match decision.as_deref() {
                Some("approved") => DraftStatus::Approved,
                Some("changes_requested") => DraftStatus::ChangesRequested,
                Some("returned") => DraftStatus::Returned,
                _ => DraftStatus::Draft,
            }
        }
        _ => DraftStatus::Draft,
    })
}

/// One `daily_report` form of a session's stamped version, for the
/// start-draft picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DailyForm {
    pub id: i64,
    pub name: String,
}

/// The stamped version's daily report forms, gated like starting the
/// draft itself.
pub async fn list_daily_forms(
    pool: &SqlitePool,
    actor_user_id: i64,
    session_id: i64,
) -> Result<std::result::Result<Vec<DailyForm>, DraftRefusal>> {
    let Some(session) =
        sqlx::query("SELECT enrollment_id, program_version_id FROM training_session WHERE id = ?1")
            .bind(session_id)
            .fetch_optional(pool)
            .await
            .context("reading session")?
    else {
        return Ok(Err(DraftRefusal::NoSuchSession));
    };
    let enrollment_id: i64 = session.get("enrollment_id");
    let version_id: i64 = session.get("program_version_id");
    if !crate::draft_access::may_start(pool, actor_user_id, session_id, enrollment_id).await? {
        return Ok(Err(DraftRefusal::CapabilityRequired));
    }
    let forms = sqlx::query(
        "SELECT id, name FROM evaluation_form
         WHERE program_version_id = ?1 AND record_type = 'daily_report'
         ORDER BY name COLLATE NOCASE, id",
    )
    .bind(version_id)
    .fetch_all(pool)
    .await
    .context("listing daily forms")?
    .iter()
    .map(|row| DailyForm {
        id: row.get("id"),
        name: row.get("name"),
    })
    .collect();
    Ok(Ok(forms))
}

// ---------------------------------------------------------------- write

/// Creates the daily draft for a session: stamps the session's version,
/// pins the `daily_report` form, makes the actor the owner, and opens the
/// attribution stream with its created event.
pub async fn create(
    pool: &SqlitePool,
    actor_user_id: i64,
    session_id: i64,
    form_id: Option<i64>,
) -> Result<std::result::Result<i64, DraftRefusal>> {
    let Some(session) = sqlx::query(
        "SELECT enrollment_id, program_version_id, disposition
         FROM training_session WHERE id = ?1",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await
    .context("reading session")?
    else {
        return Ok(Err(DraftRefusal::NoSuchSession));
    };
    let enrollment_id: i64 = session.get("enrollment_id");
    let disposition: Option<String> = session.get("disposition");
    if disposition.as_deref() == Some("cancelled") {
        return Ok(Err(DraftRefusal::SessionCancelled));
    }

    if !crate::draft_access::may_start(pool, actor_user_id, session_id, enrollment_id).await? {
        return Ok(Err(DraftRefusal::CapabilityRequired));
    }

    // A write transaction from the start: the rereads below see the
    // committed state, so a racing cancel or a second create resolves as
    // its typed refusal, never a stale read.
    let mut tx = storage::write_tx(pool).await.context("starting draft")?;
    let Some(session) = sqlx::query(
        "SELECT enrollment_id, program_version_id, disposition
         FROM training_session WHERE id = ?1",
    )
    .bind(session_id)
    .fetch_optional(&mut *tx)
    .await
    .context("rereading session")?
    else {
        return storage::refuse(tx, DraftRefusal::NoSuchSession).await;
    };
    let enrollment_id: i64 = session.get("enrollment_id");
    let version_id: i64 = session.get("program_version_id");
    let disposition: Option<String> = session.get("disposition");
    if disposition.as_deref() == Some("cancelled") {
        return storage::refuse(tx, DraftRefusal::SessionCancelled).await;
    }
    // v1 policy, not schema: one daily draft per training session.
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT evaluation_record_id FROM evaluation_session WHERE training_session_id = ?1",
    )
    .bind(session_id)
    .fetch_optional(&mut *tx)
    .await
    .context("checking existing draft")?;
    if existing.is_some() {
        return storage::refuse(tx, DraftRefusal::DraftAlreadyExists).await;
    }
    let form_id = match resolve_form(&mut tx, version_id, form_id).await? {
        Ok(form_id) => form_id,
        Err(refusal) => return storage::refuse(tx, refusal).await,
    };

    let now = OffsetDateTime::now_utc().unix_timestamp();
    let result = sqlx::query(
        "INSERT INTO evaluation_record
             (enrollment_id, program_version_id, evaluation_form_id,
              owner_user_id, created_at, created_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?4)",
    )
    .bind(enrollment_id)
    .bind(version_id)
    .bind(form_id)
    .bind(actor_user_id)
    .bind(now)
    .execute(&mut *tx)
    .await
    .context("creating record")?;
    let record_id = result.last_insert_rowid();
    append_event(&mut tx, record_id, "created", actor_user_id, None, now).await?;
    sqlx::query(
        "INSERT INTO evaluation_session (evaluation_record_id, training_session_id)
         VALUES (?1, ?2)",
    )
    .bind(record_id)
    .bind(session_id)
    .execute(&mut *tx)
    .await
    .context("covering session")?;
    let trainee: i64 = sqlx::query_scalar("SELECT user_id FROM enrollment WHERE id = ?1")
        .bind(enrollment_id)
        .fetch_one(&mut *tx)
        .await
        .context("reading enrollment")?;
    audit::record_for_subject(
        &mut *tx,
        EventKind::DraftCreated,
        Some(actor_user_id),
        Some(trainee),
        Subject::Record(record_id),
    )
    .await?;
    tx.commit().await.context("committing draft")?;
    Ok(Ok(record_id))
}

/// Picks the pinned `daily_report` form: the named one, or the version's
/// only one.
async fn resolve_form(
    tx: &mut SqliteConnection,
    version_id: i64,
    form_id: Option<i64>,
) -> Result<std::result::Result<i64, DraftRefusal>> {
    if let Some(form_id) = form_id {
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM evaluation_form
             WHERE id = ?1 AND program_version_id = ?2 AND record_type = 'daily_report'",
        )
        .bind(form_id)
        .bind(version_id)
        .fetch_optional(&mut *tx)
        .await
        .context("checking form")?;
        return Ok(if found.is_some() {
            Ok(form_id)
        } else {
            Err(DraftRefusal::NoSuchForm)
        });
    }
    let forms: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM evaluation_form
         WHERE program_version_id = ?1 AND record_type = 'daily_report'
         ORDER BY id",
    )
    .bind(version_id)
    .fetch_all(&mut *tx)
    .await
    .context("listing daily forms")?;
    Ok(match forms.as_slice() {
        [] => Err(DraftRefusal::NoDailyForm),
        [only] => Ok(*only),
        _ => Err(DraftRefusal::FormRequired),
    })
}

pub(crate) async fn append_event(
    tx: &mut SqliteConnection,
    record_id: i64,
    kind: &str,
    actor_user_id: i64,
    to_user_id: Option<i64>,
    now: i64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO contributor_event
             (evaluation_record_id, kind, actor_user_id, to_user_id, recorded_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(record_id)
    .bind(kind)
    .bind(actor_user_id)
    .bind(to_user_id)
    .bind(now)
    .execute(&mut *tx)
    .await
    .context("appending contributor event")?;
    Ok(())
}

/// Transfers ownership: the owner (or a coordinator) hands the draft to
/// another eligible author. The event mediates the owner update at the
/// database, and the recipient is notified.
pub async fn transfer(
    pool: &SqlitePool,
    actor_user_id: i64,
    record_id: i64,
    to_user_id: i64,
) -> Result<std::result::Result<(), DraftRefusal>> {
    let mut conn = pool.acquire().await.context("acquiring connection")?;
    let Some(record) = load_record(&mut conn, record_id).await? else {
        return Ok(Err(DraftRefusal::NoSuchRecord));
    };
    drop(conn);
    let coordinator =
        capabilities::user_has(pool, actor_user_id, Capability::AssignTraining).await?;
    if actor_user_id != record.owner_user_id && !coordinator {
        return Ok(Err(DraftRefusal::CapabilityRequired));
    }
    if to_user_id == record.owner_user_id {
        return Ok(Err(DraftRefusal::AlreadyOwner));
    }
    let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM user WHERE id = ?1")
        .bind(to_user_id)
        .fetch_optional(pool)
        .await
        .context("checking recipient")?;
    if exists.is_none() {
        return Ok(Err(DraftRefusal::NoSuchUser));
    }
    // Recipients hold the draft as authors: capability plus scope, the
    // same eligibility contributing takes.
    if !capabilities::user_has(pool, to_user_id, Capability::AuthorEvaluation).await?
        || !crate::draft_access::in_scope(pool, to_user_id, &record).await?
    {
        return Ok(Err(DraftRefusal::NotEligible));
    }

    let mut tx = storage::write_tx(pool).await.context("starting transfer")?;
    if let Some(refusal) = status_of(&mut tx, record_id).await?.frozen_refusal() {
        return storage::refuse(tx, refusal).await;
    }
    // Ownership is rechecked inside the transaction: a raced transfer
    // must never let a former owner exercise authority they no longer
    // hold.
    let owner_now: i64 =
        sqlx::query_scalar("SELECT owner_user_id FROM evaluation_record WHERE id = ?1")
            .bind(record_id)
            .fetch_one(&mut *tx)
            .await
            .context("rechecking owner")?;
    if actor_user_id != owner_now && !coordinator {
        return storage::refuse(tx, DraftRefusal::CapabilityRequired).await;
    }
    if to_user_id == owner_now {
        return storage::refuse(tx, DraftRefusal::AlreadyOwner).await;
    }
    let now = OffsetDateTime::now_utc().unix_timestamp();
    append_event(
        &mut tx,
        record_id,
        "ownership_transferred",
        actor_user_id,
        Some(to_user_id),
        now,
    )
    .await?;
    sqlx::query("UPDATE evaluation_record SET owner_user_id = ?1 WHERE id = ?2")
        .bind(to_user_id)
        .bind(record_id)
        .execute(&mut *tx)
        .await
        .context("updating owner")?;
    audit::record_for_subject(
        &mut *tx,
        EventKind::DraftOwnershipTransferred,
        Some(actor_user_id),
        Some(to_user_id),
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
        to_user_id,
        NoticeKind::DraftOwnershipReceived,
        &format!("You are now the owner of the draft evaluation for {trainee}."),
    )
    .await?;
    tx.commit().await.context("committing transfer")?;
    Ok(Ok(()))
}

/// Submits the draft for review: snapshots the full content — anchoring
/// the review to what was reviewed — and appends the event that freezes
/// the working copy. The submission carries the revision the submitter
/// viewed, so content another contributor saved meanwhile is never
/// frozen sight unseen.
#[allow(clippy::too_many_lines)]
pub async fn submit(
    pool: &SqlitePool,
    actor_user_id: i64,
    record_id: i64,
    expected_revision: i64,
) -> Result<std::result::Result<(), DraftRefusal>> {
    let mut conn = pool.acquire().await.context("acquiring connection")?;
    let Some(record) = load_record(&mut conn, record_id).await? else {
        return Ok(Err(DraftRefusal::NoSuchRecord));
    };
    drop(conn);
    let coordinator =
        capabilities::user_has(pool, actor_user_id, Capability::AssignTraining).await?;
    if actor_user_id != record.owner_user_id && !coordinator {
        return Ok(Err(DraftRefusal::CapabilityRequired));
    }

    let mut tx = storage::write_tx(pool)
        .await
        .context("starting submission")?;
    if let Some(refusal) = status_of(&mut tx, record_id).await?.frozen_refusal() {
        return storage::refuse(tx, refusal).await;
    }
    // Ownership and revision are rechecked inside the transaction: a
    // raced transfer or a concurrent save means the submitter is no
    // longer freezing what they viewed.
    let row = sqlx::query("SELECT owner_user_id, revision FROM evaluation_record WHERE id = ?1")
        .bind(record_id)
        .fetch_one(&mut *tx)
        .await
        .context("rechecking record")?;
    let owner_now: i64 = row.get("owner_user_id");
    if actor_user_id != owner_now && !coordinator {
        return storage::refuse(tx, DraftRefusal::CapabilityRequired).await;
    }
    let revision_now: i64 = row.get("revision");
    if expected_revision != revision_now {
        return storage::refuse(tx, DraftRefusal::StaleSave).await;
    }
    // Completion rules gate the path toward finalization here too
    // (ADR 0011): a draft never enters review missing what finalization
    // will demand, so an approved draft cannot wedge between a frozen
    // copy and a failing rule.
    let policy = crate::finalization::policy(&mut tx, record.program_version_id).await?;
    if policy.required_narratives
        && crate::finalization::narratives_incomplete(&mut tx, &record).await?
    {
        return storage::refuse(tx, DraftRefusal::NarrativesIncomplete).await;
    }
    if policy.ratings_complete && crate::finalization::ratings_incomplete(&mut tx, &record).await? {
        return storage::refuse(tx, DraftRefusal::RatingsIncomplete).await;
    }
    let content = draft_content::content_json(&mut tx, record_id).await?;
    let now = OffsetDateTime::now_utc().unix_timestamp();
    sqlx::query(
        "INSERT INTO draft_snapshot
             (evaluation_record_id, reason, content, taken_at, taken_by)
         VALUES (?1, 'submission', ?2, ?3, ?4)",
    )
    .bind(record_id)
    .bind(&content)
    .bind(now)
    .bind(actor_user_id)
    .execute(&mut *tx)
    .await
    .context("taking snapshot")?;
    append_event(
        &mut tx,
        record_id,
        "submitted_for_review",
        actor_user_id,
        None,
        now,
    )
    .await?;
    audit::record_for_subject(
        &mut *tx,
        EventKind::DraftSubmitted,
        Some(actor_user_id),
        None,
        Subject::Record(record_id),
    )
    .await?;
    // The review queue's nudge: every evaluation reviewer except the
    // submitter hears a draft awaits them (ADR 0008 notices).
    let trainee: String = sqlx::query_scalar(
        "SELECT u.display_name FROM enrollment e
         JOIN user u ON u.id = e.user_id
         WHERE e.id = ?1",
    )
    .bind(record.enrollment_id)
    .fetch_one(&mut *tx)
    .await
    .context("reading trainee name")?;
    let reviewers: Vec<i64> =
        sqlx::query_scalar("SELECT user_id FROM capability_grant WHERE capability = ?1")
            .bind(Capability::ReviewEvaluation.as_str())
            .fetch_all(&mut *tx)
            .await
            .context("listing reviewers")?;
    for reviewer in reviewers {
        if reviewer != actor_user_id {
            notices::notify_user(
                &mut *tx,
                reviewer,
                NoticeKind::DraftSubmittedForReview,
                &format!("A draft evaluation for {trainee} awaits review."),
            )
            .await?;
        }
    }
    tx.commit().await.context("committing submission")?;
    Ok(Ok(()))
}

// ----------------------------------------------------------------- read

/// One contributor event, presented.
#[derive(Debug, Clone, Serialize)]
pub struct ContributorEventRow {
    pub id: i64,
    pub kind: String,
    pub actor_user_id: i64,
    pub actor_display_name: String,
    pub to_user_id: Option<i64>,
    pub to_display_name: Option<String>,
    pub recorded_at: i64,
}

/// A covered session, for the workspace header.
#[derive(Debug, Clone, Serialize)]
pub struct CoveredSession {
    pub session_id: i64,
    pub business_date: String,
    pub timezone: String,
    pub local_start: String,
    pub local_end: Option<String>,
}

/// A snapshot's metadata; content stays in the database until review
/// needs it.
#[derive(Debug, Clone, Serialize)]
pub struct SnapshotMeta {
    pub id: i64,
    pub reason: String,
    pub taken_at: i64,
    pub taken_by: Option<i64>,
}

/// An eligible ownership recipient for the transfer picker.
#[derive(Debug, Clone, Serialize)]
pub struct EligibleRecipient {
    pub user_id: i64,
    pub display_name: String,
}

/// One review decision, presented with its comment — the workflow's
/// permanent verdicts.
#[derive(Debug, Clone, Serialize)]
pub struct ReviewDecisionRow {
    pub id: i64,
    pub reviewer_user_id: i64,
    pub reviewer_display_name: String,
    pub decision: String,
    pub comment: String,
    pub decided_at: i64,
}

/// The draft's workflow view: identity, status, attribution, coverage.
#[derive(Debug, Serialize)]
pub struct DraftDetail {
    pub id: i64,
    pub enrollment_id: i64,
    pub program_version_id: i64,
    pub evaluation_form_id: i64,
    pub owner_user_id: i64,
    pub owner_display_name: String,
    pub status: DraftStatus,
    pub trainee_user_id: i64,
    pub trainee_display_name: String,
    pub program_name: String,
    pub version_number: i64,
    /// The pinned form's record type; weekly summaries carry links.
    pub record_type: String,
    /// The pinned daily links (weekly summaries; empty otherwise).
    pub summary_links: Vec<crate::summaries::SummaryLink>,
    pub sessions: Vec<CoveredSession>,
    pub events: Vec<ContributorEventRow>,
    pub snapshots: Vec<SnapshotMeta>,
    pub eligible_recipients: Vec<EligibleRecipient>,
    pub decisions: Vec<ReviewDecisionRow>,
    /// Whether the caller may decide this draft right now: a qualified
    /// non-contributor reviewer looking at a submitted draft.
    pub viewer_may_review: bool,
    /// Whether the caller may attempt finalization: a reviewer looking
    /// at an unfinalized record. Completion rules answer at the act
    /// (ADR 0011).
    pub viewer_may_finalize: bool,
    /// Whether the caller may open an amendment: a reviewer looking at
    /// a finalized record with no reopening in progress.
    pub viewer_may_amend: bool,
    /// The latest finalized version's number, when any exists.
    pub latest_version_number: Option<i64>,
    /// The open amendment reopening this record's working copy, if any.
    pub open_amendment: Option<crate::amendments::AmendmentView>,
    pub created_at: i64,
    /// The working copy's optimistic-concurrency revision; every save
    /// carries the revision it read.
    pub revision: i64,
}

/// The workspace read: the workflow view, the pinned form skeleton, and
/// the working copy, assembled under one database snapshot so the
/// revision describes exactly the content returned beside it.
#[derive(Debug)]
pub struct DraftWorkspace {
    pub detail: DraftDetail,
    pub form: draft_content::FormSkeleton,
    pub content: draft_content::DraftContent,
}

/// Reads the workspace, gated like the record's reads. Every data read
/// shares one transaction: a concurrent save moves the whole view or
/// none of it, never the content without its revision.
#[allow(clippy::too_many_lines)]
pub async fn workspace(
    pool: &SqlitePool,
    actor_user_id: i64,
    record_id: i64,
) -> Result<std::result::Result<DraftWorkspace, DraftRefusal>> {
    let mut conn = pool.acquire().await.context("acquiring connection")?;
    let Some(record) = load_record(&mut conn, record_id).await? else {
        return Ok(Err(DraftRefusal::NoSuchRecord));
    };
    drop(conn);
    let Some(grant) = crate::draft_access::read_grant(pool, actor_user_id, &record).await? else {
        return Ok(Err(DraftRefusal::CapabilityRequired));
    };
    // An own-record reader is authorized for the finalized record, not
    // the workflow: what exists solely for the workflow (the transfer
    // picker's roster, snapshot bookkeeping) is redacted below.
    let workflow_reader = grant == crate::draft_access::ReadGrant::Workflow;
    let reviewer_capability =
        capabilities::user_has(pool, actor_user_id, Capability::ReviewEvaluation).await?;
    let mut conn = pool.begin().await.context("starting workspace read")?;
    // Reread inside the snapshot: this revision is the one the returned
    // content answers to.
    let Some(record) = load_record(&mut conn, record_id).await? else {
        return Ok(Err(DraftRefusal::NoSuchRecord));
    };
    // For an own-record reader the record presents as its sealed self:
    // an in-progress correction is a draft about them, not theirs to
    // see, so the status reads finalized, the working copy stays out of
    // the response, and the streams stop at the sealed cycle's marks
    // (the envelope carries exactly that history).
    let scope = crate::amendments::open_scope(&mut conn, record_id).await?;
    let (event_cap, decision_cap) = match (&scope, workflow_reader) {
        (Some(scope), false) => (scope.opened_after_event_id, scope.opened_after_decision_id),
        _ => (i64::MAX, i64::MAX),
    };
    let status = if workflow_reader {
        status_of(&mut conn, record_id).await?
    } else {
        DraftStatus::Finalized
    };
    let viewer_may_review = reviewer_capability
        && status == DraftStatus::Submitted
        && !crate::draft_access::is_contributor(&mut conn, record_id, actor_user_id).await?;
    let viewer_may_finalize =
        workflow_reader && reviewer_capability && status != DraftStatus::Finalized;
    let viewer_may_amend =
        workflow_reader && reviewer_capability && status == DraftStatus::Finalized;
    let latest_version_number: Option<i64> = sqlx::query_scalar(
        "SELECT MAX(version_number) FROM evaluation_version WHERE evaluation_record_id = ?1",
    )
    .bind(record_id)
    .fetch_one(&mut *conn)
    .await
    .context("reading latest version number")?;
    let open_amendment = if workflow_reader {
        scope.map(|scope| crate::amendments::AmendmentView {
            reason: scope.reason,
            opened_by_display_name: scope.opened_by_display_name,
            opened_at: scope.opened_at,
        })
    } else {
        None
    };
    let header = sqlx::query(
        "SELECT r.created_at, e.user_id AS trainee_user_id,
                tu.display_name AS trainee_display_name,
                ou.display_name AS owner_display_name,
                pv.name AS program_name, pv.version_number,
                f.record_type
         FROM evaluation_record r
         JOIN enrollment e ON e.id = r.enrollment_id
         JOIN user tu ON tu.id = e.user_id
         JOIN user ou ON ou.id = r.owner_user_id
         JOIN program_version pv ON pv.id = r.program_version_id
         JOIN evaluation_form f ON f.id = r.evaluation_form_id
         WHERE r.id = ?1",
    )
    .bind(record_id)
    .fetch_one(&mut *conn)
    .await
    .context("reading record header")?;
    // Working-copy state travels to workflow readers; an own-record
    // reader's coverage is the sealed envelope's daily_reports.
    let summary_links = if workflow_reader {
        crate::summaries::links(&mut conn, record_id).await?
    } else {
        Vec::new()
    };
    let sessions = sqlx::query(
        "SELECT s.id, s.business_date, s.timezone, s.local_start, s.local_end
         FROM evaluation_session es
         JOIN training_session s ON s.id = es.training_session_id
         WHERE es.evaluation_record_id = ?1
         ORDER BY s.utc_start, s.id",
    )
    .bind(record_id)
    .fetch_all(&mut *conn)
    .await
    .context("listing covered sessions")?
    .iter()
    .map(|row| CoveredSession {
        session_id: row.get("id"),
        business_date: row.get("business_date"),
        timezone: row.get("timezone"),
        local_start: row.get("local_start"),
        local_end: row.get("local_end"),
    })
    .collect();
    let events = sqlx::query(
        "SELECT ce.id, ce.kind, ce.actor_user_id, au.display_name AS actor_display_name,
                ce.to_user_id, tu.display_name AS to_display_name, ce.recorded_at
         FROM contributor_event ce
         JOIN user au ON au.id = ce.actor_user_id
         LEFT JOIN user tu ON tu.id = ce.to_user_id
         WHERE ce.evaluation_record_id = ?1 AND ce.id <= ?2
         ORDER BY ce.id",
    )
    .bind(record_id)
    .bind(event_cap)
    .fetch_all(&mut *conn)
    .await
    .context("listing contributor events")?
    .iter()
    .map(|row| ContributorEventRow {
        id: row.get("id"),
        kind: row.get("kind"),
        actor_user_id: row.get("actor_user_id"),
        actor_display_name: row.get("actor_display_name"),
        to_user_id: row.get("to_user_id"),
        to_display_name: row.get("to_display_name"),
        recorded_at: row.get("recorded_at"),
    })
    .collect();
    let snapshots = if workflow_reader {
        sqlx::query(
            "SELECT id, reason, taken_at, taken_by FROM draft_snapshot
             WHERE evaluation_record_id = ?1 ORDER BY id",
        )
        .bind(record_id)
        .fetch_all(&mut *conn)
        .await
        .context("listing snapshots")?
        .iter()
        .map(|row| SnapshotMeta {
            id: row.get("id"),
            reason: row.get("reason"),
            taken_at: row.get("taken_at"),
            taken_by: row.get("taken_by"),
        })
        .collect()
    } else {
        Vec::new()
    };
    // The transfer picker: authors within the record's scope, minus the
    // current owner. Workflow readers only — the roster of possible
    // recipients names staff who may never touch this record, which is
    // not an own-record reader's to see.
    let eligible_recipients = if workflow_reader {
        sqlx::query(
            "SELECT DISTINCT u.id, u.display_name
             FROM user u
             JOIN capability_grant cg ON cg.user_id = u.id AND cg.capability = ?3
             WHERE u.id != ?2
               AND u.id IN (
                   SELECT ta.trainer_user_id FROM training_assignment ta
                   JOIN evaluation_record r ON r.enrollment_id = ta.enrollment_id
                   WHERE r.id = ?1 AND ta.ended_at IS NULL
                   UNION
                   SELECT st.trainer_user_id FROM session_trainer st
                   JOIN evaluation_session es ON es.training_session_id = st.session_id
                   WHERE es.evaluation_record_id = ?1
               )
             ORDER BY u.display_name COLLATE NOCASE",
        )
        .bind(record_id)
        .bind(record.owner_user_id)
        .bind(Capability::AuthorEvaluation.as_str())
        .fetch_all(&mut *conn)
        .await
        .context("listing eligible recipients")?
        .iter()
        .map(|row| EligibleRecipient {
            user_id: row.get("id"),
            display_name: row.get("display_name"),
        })
        .collect()
    } else {
        Vec::new()
    };
    let decisions = sqlx::query(
        "SELECT rd.id, rd.reviewer_user_id, u.display_name AS reviewer_display_name,
                rd.decision, rd.comment, rd.decided_at
         FROM review_decision rd
         JOIN user u ON u.id = rd.reviewer_user_id
         WHERE rd.evaluation_record_id = ?1 AND rd.id <= ?2
         ORDER BY rd.id",
    )
    .bind(record_id)
    .bind(decision_cap)
    .fetch_all(&mut *conn)
    .await
    .context("listing review decisions")?
    .iter()
    .map(|row| ReviewDecisionRow {
        id: row.get("id"),
        reviewer_user_id: row.get("reviewer_user_id"),
        reviewer_display_name: row.get("reviewer_display_name"),
        decision: row.get("decision"),
        comment: row.get("comment"),
        decided_at: row.get("decided_at"),
    })
    .collect();
    let form = draft_content::skeleton(
        &mut conn,
        record.program_version_id,
        record.evaluation_form_id,
    )
    .await?;
    // The working copy travels only to workflow readers; an own-record
    // reader's content is the stored envelope, fetched separately.
    let content = if workflow_reader {
        draft_content::content(&mut conn, record_id).await?
    } else {
        draft_content::DraftContent {
            ratings: Vec::new(),
            narratives: Vec::new(),
        }
    };
    Ok(Ok(DraftWorkspace {
        detail: DraftDetail {
            id: record.id,
            enrollment_id: record.enrollment_id,
            program_version_id: record.program_version_id,
            evaluation_form_id: record.evaluation_form_id,
            owner_user_id: record.owner_user_id,
            owner_display_name: header.get("owner_display_name"),
            status,
            trainee_user_id: header.get("trainee_user_id"),
            trainee_display_name: header.get("trainee_display_name"),
            program_name: header.get("program_name"),
            version_number: header.get("version_number"),
            record_type: header.get("record_type"),
            summary_links,
            sessions,
            events,
            snapshots,
            eligible_recipients,
            decisions,
            viewer_may_review,
            viewer_may_finalize,
            viewer_may_amend,
            latest_version_number,
            open_amendment,
            created_at: header.get("created_at"),
            revision: record.revision,
        },
        form,
        content,
    }))
}
