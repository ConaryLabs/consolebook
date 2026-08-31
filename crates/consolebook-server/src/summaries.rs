//! Weekly summaries (docs/domain-model.md `WeeklySummary`; ADR 0013;
//! Milestone 4 slice 4).
//!
//! A weekly summary is an ordinary evaluation record — same working
//! copy, policy gates, review, finalization, acknowledgment, and
//! amendment machinery — whose copy additionally carries links pinning
//! the exact finalized daily-report versions it covers. Links are
//! authored while the copy is editable, freeze with it (migration
//! 0013 holds the shape raw), and seal into the record-schema-2
//! envelope's `daily_reports` member.

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::{Row, SqliteConnection, SqlitePool};
use time::OffsetDateTime;

use crate::audit::{self, EventKind, Subject};
use crate::capabilities::{self, Capability};
use crate::draft_content;
use crate::evaluation_drafts::{self, DraftRefusal};
use crate::storage;
use crate::{assignments, evaluation_drafts::DailyForm};

/// Whether the actor may start a weekly summary on this enrollment: a
/// coordinator, or an assigned evaluation author.
async fn may_start(pool: &SqlitePool, actor_user_id: i64, enrollment_id: i64) -> Result<bool> {
    if capabilities::user_has(pool, actor_user_id, Capability::AssignTraining).await? {
        return Ok(true);
    }
    Ok(
        capabilities::user_has(pool, actor_user_id, Capability::AuthorEvaluation).await?
            && assignments::is_assigned(pool, actor_user_id, enrollment_id).await?,
    )
}

/// The pinned version's `weekly_summary` forms, for the start picker.
pub async fn list_summary_forms(
    pool: &SqlitePool,
    actor_user_id: i64,
    enrollment_id: i64,
) -> Result<std::result::Result<Vec<DailyForm>, DraftRefusal>> {
    let Some(version_id) = enrollment_version(pool, enrollment_id).await? else {
        return Ok(Err(DraftRefusal::NoSuchEnrollment));
    };
    if !may_start(pool, actor_user_id, enrollment_id).await? {
        return Ok(Err(DraftRefusal::CapabilityRequired));
    }
    let forms = sqlx::query(
        "SELECT id, name FROM evaluation_form
         WHERE program_version_id = ?1 AND record_type = 'weekly_summary'
         ORDER BY name COLLATE NOCASE, id",
    )
    .bind(version_id)
    .fetch_all(pool)
    .await
    .context("listing summary forms")?
    .iter()
    .map(|row| DailyForm {
        id: row.get("id"),
        name: row.get("name"),
    })
    .collect();
    Ok(Ok(forms))
}

async fn enrollment_version(pool: &SqlitePool, enrollment_id: i64) -> Result<Option<i64>> {
    sqlx::query_scalar("SELECT program_version_id FROM enrollment WHERE id = ?1")
        .bind(enrollment_id)
        .fetch_optional(pool)
        .await
        .context("reading enrollment")
}

/// Creates a weekly summary draft on the enrollment: stamps the pinned
/// version, pins the `weekly_summary` form, makes the actor the owner,
/// and opens the attribution stream. Summaries cover no sessions;
/// their coverage is the daily links.
pub async fn create(
    pool: &SqlitePool,
    actor_user_id: i64,
    enrollment_id: i64,
    form_id: Option<i64>,
) -> Result<std::result::Result<i64, DraftRefusal>> {
    if enrollment_version(pool, enrollment_id).await?.is_none() {
        return Ok(Err(DraftRefusal::NoSuchEnrollment));
    }
    if !may_start(pool, actor_user_id, enrollment_id).await? {
        return Ok(Err(DraftRefusal::CapabilityRequired));
    }

    let mut tx = storage::write_tx(pool).await.context("starting summary")?;
    let Some(version_id): Option<i64> =
        sqlx::query_scalar("SELECT program_version_id FROM enrollment WHERE id = ?1")
            .bind(enrollment_id)
            .fetch_optional(&mut *tx)
            .await
            .context("rereading enrollment")?
    else {
        return storage::refuse(tx, DraftRefusal::NoSuchEnrollment).await;
    };
    let form_id = match resolve_summary_form(&mut tx, version_id, form_id).await? {
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
    .context("creating summary record")?;
    let record_id = result.last_insert_rowid();
    evaluation_drafts::append_event(&mut tx, record_id, "created", actor_user_id, None, now)
        .await?;
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
    tx.commit().await.context("committing summary")?;
    Ok(Ok(record_id))
}

/// Picks the pinned `weekly_summary` form: the named one, or the
/// version's only one.
async fn resolve_summary_form(
    tx: &mut SqliteConnection,
    version_id: i64,
    form_id: Option<i64>,
) -> Result<std::result::Result<i64, DraftRefusal>> {
    if let Some(form_id) = form_id {
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM evaluation_form
             WHERE id = ?1 AND program_version_id = ?2 AND record_type = 'weekly_summary'",
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
         WHERE program_version_id = ?1 AND record_type = 'weekly_summary'
         ORDER BY id",
    )
    .bind(version_id)
    .fetch_all(&mut *tx)
    .await
    .context("listing summary forms")?;
    Ok(match forms.as_slice() {
        [] => Err(DraftRefusal::NoSummaryForm),
        [only] => Ok(*only),
        _ => Err(DraftRefusal::FormRequired),
    })
}

/// One pinned daily link, presented. Labels come from the linked
/// version's stored envelope — immutable data addressed by pinned id,
/// never a mutable join.
#[derive(Debug, Clone, Serialize)]
pub struct SummaryLink {
    pub daily_version_id: i64,
    pub record_id: i64,
    pub version_number: i64,
    pub content_hash: String,
    pub finalized_at: i64,
    pub form_name: Option<String>,
    pub business_date: Option<String>,
}

/// The record's pinned links, for the workspace read.
pub(crate) async fn links(conn: &mut SqliteConnection, record_id: i64) -> Result<Vec<SummaryLink>> {
    let rows = sqlx::query(
        "SELECT v.id, v.evaluation_record_id, v.version_number, v.content_hash,
                v.finalized_at,
                json_extract(CAST(v.canonical_bytes AS TEXT), '$.form.name')
                    AS form_name,
                json_extract(CAST(v.canonical_bytes AS TEXT),
                             '$.sessions[0].business_date') AS business_date
         FROM summary_daily_link l
         JOIN evaluation_version v ON v.id = l.daily_version_id
         WHERE l.summary_record_id = ?1
         ORDER BY v.evaluation_record_id, v.version_number",
    )
    .bind(record_id)
    .fetch_all(&mut *conn)
    .await
    .context("listing summary links")?;
    Ok(rows
        .iter()
        .map(|row| SummaryLink {
            daily_version_id: row.get("id"),
            record_id: row.get("evaluation_record_id"),
            version_number: row.get("version_number"),
            content_hash: row.get("content_hash"),
            finalized_at: row.get("finalized_at"),
            form_name: row.get("form_name"),
            business_date: row.get("business_date"),
        })
        .collect())
}

/// The enrollment's finalized daily versions not yet linked, newest
/// first, for the link picker.
pub async fn linkable(
    pool: &SqlitePool,
    actor_user_id: i64,
    record_id: i64,
) -> Result<std::result::Result<Vec<SummaryLink>, DraftRefusal>> {
    let mut conn = pool.acquire().await.context("acquiring connection")?;
    let Some(record) = evaluation_drafts::load_record(&mut conn, record_id).await? else {
        return Ok(Err(DraftRefusal::NoSuchRecord));
    };
    drop(conn);
    if !crate::draft_access::may_contribute(pool, actor_user_id, &record).await? {
        return Ok(Err(DraftRefusal::CapabilityRequired));
    }
    let rows = sqlx::query(
        "SELECT v.id, v.evaluation_record_id, v.version_number, v.content_hash,
                v.finalized_at,
                json_extract(CAST(v.canonical_bytes AS TEXT), '$.form.name')
                    AS form_name,
                json_extract(CAST(v.canonical_bytes AS TEXT),
                             '$.sessions[0].business_date') AS business_date
         FROM evaluation_version v
         JOIN evaluation_record r ON r.id = v.evaluation_record_id
         JOIN evaluation_form f ON f.id = r.evaluation_form_id
         WHERE r.enrollment_id = ?1
           AND f.record_type = 'daily_report'
           AND NOT EXISTS (SELECT 1 FROM summary_daily_link l
                           WHERE l.summary_record_id = ?2
                             AND l.daily_version_id = v.id)
         ORDER BY v.finalized_at DESC, v.id DESC",
    )
    .bind(record.enrollment_id)
    .bind(record_id)
    .fetch_all(pool)
    .await
    .context("listing linkable dailies")?;
    Ok(Ok(rows
        .iter()
        .map(|row| SummaryLink {
            daily_version_id: row.get("id"),
            record_id: row.get("evaluation_record_id"),
            version_number: row.get("version_number"),
            content_hash: row.get("content_hash"),
            finalized_at: row.get("finalized_at"),
            form_name: row.get("form_name"),
            business_date: row.get("business_date"),
        })
        .collect()))
}

/// Adds one pinned daily link to the summary's working copy. Carries
/// the revision the editor viewed, like every working-copy write.
pub async fn add_link(
    pool: &SqlitePool,
    actor_user_id: i64,
    record_id: i64,
    daily_version_id: i64,
    expected_revision: i64,
) -> Result<std::result::Result<i64, DraftRefusal>> {
    edit_links(
        pool,
        actor_user_id,
        record_id,
        daily_version_id,
        expected_revision,
        true,
    )
    .await
}

/// Removes one link from the summary's working copy.
pub async fn remove_link(
    pool: &SqlitePool,
    actor_user_id: i64,
    record_id: i64,
    daily_version_id: i64,
    expected_revision: i64,
) -> Result<std::result::Result<i64, DraftRefusal>> {
    edit_links(
        pool,
        actor_user_id,
        record_id,
        daily_version_id,
        expected_revision,
        false,
    )
    .await
}

#[allow(clippy::too_many_lines)]
async fn edit_links(
    pool: &SqlitePool,
    actor_user_id: i64,
    record_id: i64,
    daily_version_id: i64,
    expected_revision: i64,
    add: bool,
) -> Result<std::result::Result<i64, DraftRefusal>> {
    let mut conn = pool.acquire().await.context("acquiring connection")?;
    let Some(record) = evaluation_drafts::load_record(&mut conn, record_id).await? else {
        return Ok(Err(DraftRefusal::NoSuchRecord));
    };
    drop(conn);
    if !crate::draft_access::may_contribute(pool, actor_user_id, &record).await? {
        return Ok(Err(DraftRefusal::CapabilityRequired));
    }

    let mut tx = storage::write_tx(pool)
        .await
        .context("starting link edit")?;
    if let Some(refusal) = evaluation_drafts::status_of(&mut tx, record_id)
        .await?
        .frozen_refusal()
    {
        return storage::refuse(tx, refusal).await;
    }
    let record_type: String = sqlx::query_scalar(
        "SELECT f.record_type FROM evaluation_record r
         JOIN evaluation_form f ON f.id = r.evaluation_form_id
         WHERE r.id = ?1",
    )
    .bind(record_id)
    .fetch_one(&mut *tx)
    .await
    .context("reading record type")?;
    if record_type != "weekly_summary" {
        return storage::refuse(tx, DraftRefusal::NotASummary).await;
    }
    let revision_now: i64 =
        sqlx::query_scalar("SELECT revision FROM evaluation_record WHERE id = ?1")
            .bind(record_id)
            .fetch_one(&mut *tx)
            .await
            .context("rechecking revision")?;
    if expected_revision != revision_now {
        return storage::refuse(tx, DraftRefusal::StaleSave).await;
    }
    if add {
        let Some(version) = sqlx::query(
            "SELECT r.enrollment_id, f.record_type
             FROM evaluation_version v
             JOIN evaluation_record r ON r.id = v.evaluation_record_id
             JOIN evaluation_form f ON f.id = r.evaluation_form_id
             WHERE v.id = ?1",
        )
        .bind(daily_version_id)
        .fetch_optional(&mut *tx)
        .await
        .context("reading linked version")?
        else {
            return storage::refuse(tx, DraftRefusal::NoSuchVersion).await;
        };
        if version.get::<i64, _>("enrollment_id") != record.enrollment_id {
            return storage::refuse(tx, DraftRefusal::WrongEnrollment).await;
        }
        if version.get::<String, _>("record_type") != "daily_report" {
            return storage::refuse(tx, DraftRefusal::NotADaily).await;
        }
        let duplicate: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM summary_daily_link
             WHERE summary_record_id = ?1 AND daily_version_id = ?2",
        )
        .bind(record_id)
        .bind(daily_version_id)
        .fetch_optional(&mut *tx)
        .await
        .context("checking duplicate link")?;
        if duplicate.is_some() {
            return storage::refuse(tx, DraftRefusal::DuplicateLink).await;
        }
        sqlx::query(
            "INSERT INTO summary_daily_link (summary_record_id, daily_version_id)
             VALUES (?1, ?2)",
        )
        .bind(record_id)
        .bind(daily_version_id)
        .execute(&mut *tx)
        .await
        .context("adding link")?;
    } else {
        let removed = sqlx::query(
            "DELETE FROM summary_daily_link
             WHERE summary_record_id = ?1 AND daily_version_id = ?2",
        )
        .bind(record_id)
        .bind(daily_version_id)
        .execute(&mut *tx)
        .await
        .context("removing link")?;
        if removed.rows_affected() == 0 {
            return storage::refuse(tx, DraftRefusal::NoSuchLink).await;
        }
    }
    let next_revision = revision_now + 1;
    sqlx::query("UPDATE evaluation_record SET revision = ?1 WHERE id = ?2")
        .bind(next_revision)
        .bind(record_id)
        .execute(&mut *tx)
        .await
        .context("bumping revision")?;
    draft_content::attribute_contribution(&mut tx, record_id, actor_user_id).await?;
    tx.commit().await.context("committing link edit")?;
    Ok(Ok(next_revision))
}
