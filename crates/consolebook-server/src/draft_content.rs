//! The draft's mutable working copy (ADR 0008).
//!
//! One rating per pinned form competency — validated against the pinned
//! scale kind: anchored values stay in bounds, pass/fail is 0 or 1,
//! narrative-only takes no value — plus modifiers from the pinned
//! vocabulary and one text per pinned narrative prompt. Every save is a
//! full replacement of the working copy; consecutive saves by the same
//! contributor coalesce into one contributed event per working stretch,
//! so attribution stays honest without keystroke noise. The sibling
//! `evaluation_drafts` owns the record lifecycle and gates; migration
//! 0008 freezes the copy at the database once the draft is submitted.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqliteConnection, SqlitePool};
use time::OffsetDateTime;

use crate::evaluation_drafts::{self, DraftRefusal, DraftStatus};
use crate::storage;

/// One rating in a save or read: the pinned form competency, the value
/// under its scale kind, and the applied modifiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RatingEntry {
    pub form_competency_id: i64,
    pub value: Option<i64>,
    #[serde(default)]
    pub modifier_ids: Vec<i64>,
}

/// One narrative in a save or read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NarrativeEntry {
    pub form_narrative_id: i64,
    pub text: String,
}

/// The full working copy — also the snapshot serialization: this struct,
/// entries ordered by their vocabulary ids, as JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftContent {
    pub ratings: Vec<RatingEntry>,
    pub narratives: Vec<NarrativeEntry>,
}

// ------------------------------------------------------------- skeleton

/// One anchor of an anchored scale, for presentation.
#[derive(Debug, Clone, Serialize)]
pub struct AnchorRow {
    pub value: i64,
    pub label: String,
    pub definition: String,
}

/// One rated line of the pinned form.
#[derive(Debug, Clone, Serialize)]
pub struct SkeletonCompetency {
    pub form_competency_id: i64,
    pub category: String,
    pub name: String,
    pub description: String,
    pub scale_name: String,
    pub scale_kind: String,
    pub min_value: Option<i64>,
    pub max_value: Option<i64>,
    pub anchors: Vec<AnchorRow>,
}

/// One narrative prompt of the pinned form.
#[derive(Debug, Clone, Serialize)]
pub struct SkeletonNarrative {
    pub form_narrative_id: i64,
    pub prompt: String,
    pub required: bool,
}

/// One modifier of the pinned vocabulary.
#[derive(Debug, Clone, Serialize)]
pub struct SkeletonModifier {
    pub rating_modifier_id: i64,
    pub code: String,
    pub label: String,
    pub description: String,
}

/// The pinned form, rendered for the workspace: what the vocabulary
/// offers, never what was entered.
#[derive(Debug, Serialize)]
pub struct FormSkeleton {
    pub form_name: String,
    pub instructions: String,
    pub competencies: Vec<SkeletonCompetency>,
    pub narratives: Vec<SkeletonNarrative>,
    pub modifiers: Vec<SkeletonModifier>,
}

pub(crate) async fn skeleton(
    conn: &mut SqliteConnection,
    version_id: i64,
    form_id: i64,
) -> Result<FormSkeleton> {
    let form = sqlx::query(
        "SELECT name, instructions FROM evaluation_form
         WHERE id = ?1 AND program_version_id = ?2",
    )
    .bind(form_id)
    .bind(version_id)
    .fetch_one(&mut *conn)
    .await
    .context("reading form")?;
    let rows = sqlx::query(
        "SELECT fc.id, c.category, c.name, c.description,
                rs.id AS scale_id, rs.name AS scale_name, rs.kind,
                rs.min_value, rs.max_value
         FROM form_competency fc
         JOIN competency c ON c.id = fc.competency_id
         JOIN rating_scale rs ON rs.id = fc.rating_scale_id
         WHERE fc.evaluation_form_id = ?1 AND fc.program_version_id = ?2
         ORDER BY fc.sort_order, fc.id",
    )
    .bind(form_id)
    .bind(version_id)
    .fetch_all(&mut *conn)
    .await
    .context("listing form competencies")?;
    let mut competencies = Vec::with_capacity(rows.len());
    for row in &rows {
        let scale_id: i64 = row.get("scale_id");
        let anchors = sqlx::query(
            "SELECT value, label, definition FROM rating_anchor
             WHERE rating_scale_id = ?1 ORDER BY value",
        )
        .bind(scale_id)
        .fetch_all(&mut *conn)
        .await
        .context("listing anchors")?
        .iter()
        .map(|anchor| AnchorRow {
            value: anchor.get("value"),
            label: anchor.get("label"),
            definition: anchor.get("definition"),
        })
        .collect();
        competencies.push(SkeletonCompetency {
            form_competency_id: row.get("id"),
            category: row.get("category"),
            name: row.get("name"),
            description: row.get("description"),
            scale_name: row.get("scale_name"),
            scale_kind: row.get("kind"),
            min_value: row.get("min_value"),
            max_value: row.get("max_value"),
            anchors,
        });
    }
    let narratives = sqlx::query(
        "SELECT id, prompt, required FROM form_narrative
         WHERE evaluation_form_id = ?1 AND program_version_id = ?2
         ORDER BY sort_order, id",
    )
    .bind(form_id)
    .bind(version_id)
    .fetch_all(&mut *conn)
    .await
    .context("listing form narratives")?
    .iter()
    .map(|row| SkeletonNarrative {
        form_narrative_id: row.get("id"),
        prompt: row.get("prompt"),
        required: row.get::<i64, _>("required") != 0,
    })
    .collect();
    let modifiers = sqlx::query(
        "SELECT id, code, label, description FROM rating_modifier
         WHERE program_version_id = ?1 ORDER BY code COLLATE NOCASE",
    )
    .bind(version_id)
    .fetch_all(&mut *conn)
    .await
    .context("listing modifiers")?
    .iter()
    .map(|row| SkeletonModifier {
        rating_modifier_id: row.get("id"),
        code: row.get("code"),
        label: row.get("label"),
        description: row.get("description"),
    })
    .collect();
    Ok(FormSkeleton {
        form_name: form.get("name"),
        instructions: form.get("instructions"),
        competencies,
        narratives,
        modifiers,
    })
}

// ----------------------------------------------------------------- read

pub(crate) async fn content(conn: &mut SqliteConnection, record_id: i64) -> Result<DraftContent> {
    let rating_rows = sqlx::query(
        "SELECT id, form_competency_id, value FROM draft_rating
         WHERE evaluation_record_id = ?1 ORDER BY form_competency_id",
    )
    .bind(record_id)
    .fetch_all(&mut *conn)
    .await
    .context("reading ratings")?;
    let mut ratings = Vec::with_capacity(rating_rows.len());
    for row in &rating_rows {
        let rating_id: i64 = row.get("id");
        let modifier_ids: Vec<i64> = sqlx::query_scalar(
            "SELECT rating_modifier_id FROM draft_rating_modifier
             WHERE draft_rating_id = ?1 ORDER BY rating_modifier_id",
        )
        .bind(rating_id)
        .fetch_all(&mut *conn)
        .await
        .context("reading rating modifiers")?;
        ratings.push(RatingEntry {
            form_competency_id: row.get("form_competency_id"),
            value: row.get("value"),
            modifier_ids,
        });
    }
    let narratives = sqlx::query(
        "SELECT form_narrative_id, text FROM draft_narrative
         WHERE evaluation_record_id = ?1 ORDER BY form_narrative_id",
    )
    .bind(record_id)
    .fetch_all(&mut *conn)
    .await
    .context("reading narratives")?
    .iter()
    .map(|row| NarrativeEntry {
        form_narrative_id: row.get("form_narrative_id"),
        text: row.get("text"),
    })
    .collect();
    Ok(DraftContent {
        ratings,
        narratives,
    })
}

/// The canonical snapshot serialization: the working copy as JSON.
pub(crate) async fn content_json(conn: &mut SqliteConnection, record_id: i64) -> Result<String> {
    let content = content(conn, record_id).await?;
    serde_json::to_string(&content).context("serializing content")
}

// ----------------------------------------------------------------- save

/// Replaces the working copy after validating every id and value against
/// the record's pinned vocabulary, then attributes the save with a
/// coalesced contributed event. The save carries the revision it read;
/// a stale one is refused rather than silently overwriting another
/// contributor's work, and the new revision is returned.
#[allow(clippy::too_many_lines)]
pub async fn save(
    pool: &SqlitePool,
    actor_user_id: i64,
    record_id: i64,
    revision: i64,
    input: &DraftContent,
) -> Result<std::result::Result<i64, DraftRefusal>> {
    let mut conn = pool.acquire().await.context("acquiring connection")?;
    let Some(record) = evaluation_drafts::load_record(&mut conn, record_id).await? else {
        return Ok(Err(DraftRefusal::NoSuchRecord));
    };
    drop(conn);
    if !evaluation_drafts::may_contribute(pool, actor_user_id, &record).await? {
        return Ok(Err(DraftRefusal::CapabilityRequired));
    }

    // A write transaction from the start: the revision comparison below
    // reads the committed state, so a racing contributor's save resolves
    // as the typed stale refusal, never a failed snapshot.
    let mut tx = storage::write_tx(pool).await.context("starting save")?;
    if evaluation_drafts::status_of(&mut tx, record_id).await? == DraftStatus::Submitted {
        return Ok(Err(DraftRefusal::DraftSubmitted));
    }
    let current: i64 = sqlx::query_scalar("SELECT revision FROM evaluation_record WHERE id = ?1")
        .bind(record_id)
        .fetch_one(&mut *tx)
        .await
        .context("reading revision")?;
    if revision != current {
        return Ok(Err(DraftRefusal::StaleSave));
    }

    // Validate against the pinned vocabulary before touching the copy.
    let mut seen_competencies: Vec<i64> = Vec::new();
    for rating in &input.ratings {
        if seen_competencies.contains(&rating.form_competency_id) {
            return Ok(Err(DraftRefusal::DuplicateEntry));
        }
        seen_competencies.push(rating.form_competency_id);
        let Some(scale) = sqlx::query(
            "SELECT rs.kind, rs.min_value, rs.max_value
             FROM form_competency fc
             JOIN rating_scale rs ON rs.id = fc.rating_scale_id
             WHERE fc.id = ?1 AND fc.program_version_id = ?2
               AND fc.evaluation_form_id = ?3",
        )
        .bind(rating.form_competency_id)
        .bind(record.program_version_id)
        .bind(record.evaluation_form_id)
        .fetch_optional(&mut *tx)
        .await
        .context("checking form competency")?
        else {
            return Ok(Err(DraftRefusal::NoSuchFormCompetency));
        };
        let kind: String = scale.get("kind");
        match (kind.as_str(), rating.value) {
            ("narrative_only", Some(_)) => {
                return Ok(Err(DraftRefusal::ValueNotAllowed));
            }
            ("narrative_only", None) | ("pass_fail", Some(0 | 1)) => {}
            ("anchored_numeric", Some(value)) => {
                let min: i64 = scale.get("min_value");
                let max: i64 = scale.get("max_value");
                if value < min || value > max {
                    return Ok(Err(DraftRefusal::ValueOutOfRange));
                }
            }
            _ => return Ok(Err(DraftRefusal::ValueOutOfRange)),
        }
        for modifier_id in &rating.modifier_ids {
            let found: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM rating_modifier WHERE id = ?1 AND program_version_id = ?2",
            )
            .bind(modifier_id)
            .bind(record.program_version_id)
            .fetch_optional(&mut *tx)
            .await
            .context("checking modifier")?;
            if found.is_none() {
                return Ok(Err(DraftRefusal::NoSuchModifier));
            }
        }
    }
    let mut seen_narratives: Vec<i64> = Vec::new();
    for narrative in &input.narratives {
        if seen_narratives.contains(&narrative.form_narrative_id) {
            return Ok(Err(DraftRefusal::DuplicateEntry));
        }
        seen_narratives.push(narrative.form_narrative_id);
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM form_narrative
             WHERE id = ?1 AND program_version_id = ?2 AND evaluation_form_id = ?3",
        )
        .bind(narrative.form_narrative_id)
        .bind(record.program_version_id)
        .bind(record.evaluation_form_id)
        .fetch_optional(&mut *tx)
        .await
        .context("checking form narrative")?;
        if found.is_none() {
            return Ok(Err(DraftRefusal::NoSuchFormNarrative));
        }
    }

    // Full replacement: the draft is one working copy, not an edit log.
    sqlx::query(
        "DELETE FROM draft_rating_modifier
         WHERE draft_rating_id IN
             (SELECT id FROM draft_rating WHERE evaluation_record_id = ?1)",
    )
    .bind(record_id)
    .execute(&mut *tx)
    .await
    .context("clearing rating modifiers")?;
    sqlx::query("DELETE FROM draft_rating WHERE evaluation_record_id = ?1")
        .bind(record_id)
        .execute(&mut *tx)
        .await
        .context("clearing ratings")?;
    sqlx::query("DELETE FROM draft_narrative WHERE evaluation_record_id = ?1")
        .bind(record_id)
        .execute(&mut *tx)
        .await
        .context("clearing narratives")?;
    for rating in &input.ratings {
        let inserted = sqlx::query(
            "INSERT INTO draft_rating
                 (evaluation_record_id, program_version_id, form_competency_id, value)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(record_id)
        .bind(record.program_version_id)
        .bind(rating.form_competency_id)
        .bind(rating.value)
        .execute(&mut *tx)
        .await
        .context("writing rating")?;
        let rating_id = inserted.last_insert_rowid();
        let mut applied: Vec<i64> = Vec::new();
        for modifier_id in &rating.modifier_ids {
            if applied.contains(modifier_id) {
                continue;
            }
            applied.push(*modifier_id);
            sqlx::query(
                "INSERT INTO draft_rating_modifier
                     (draft_rating_id, program_version_id, rating_modifier_id)
                 VALUES (?1, ?2, ?3)",
            )
            .bind(rating_id)
            .bind(record.program_version_id)
            .bind(modifier_id)
            .execute(&mut *tx)
            .await
            .context("writing rating modifier")?;
        }
    }
    for narrative in &input.narratives {
        sqlx::query(
            "INSERT INTO draft_narrative
                 (evaluation_record_id, program_version_id, form_narrative_id, text)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(record_id)
        .bind(record.program_version_id)
        .bind(narrative.form_narrative_id)
        .bind(&narrative.text)
        .execute(&mut *tx)
        .await
        .context("writing narrative")?;
    }

    // The saved copy supersedes revision `current`.
    let next_revision = current + 1;
    sqlx::query("UPDATE evaluation_record SET revision = ?1 WHERE id = ?2")
        .bind(next_revision)
        .bind(record_id)
        .execute(&mut *tx)
        .await
        .context("bumping revision")?;

    // One contributed event per working stretch: consecutive saves by
    // the same contributor coalesce (ADR 0008).
    let latest = sqlx::query(
        "SELECT kind, actor_user_id FROM contributor_event
         WHERE evaluation_record_id = ?1 ORDER BY id DESC LIMIT 1",
    )
    .bind(record_id)
    .fetch_optional(&mut *tx)
    .await
    .context("reading latest contributor event")?;
    let coalesce = latest.is_some_and(|row| {
        row.get::<String, _>("kind") == "contributed"
            && row.get::<i64, _>("actor_user_id") == actor_user_id
    });
    if !coalesce {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        evaluation_drafts::append_event(
            &mut tx,
            record_id,
            "contributed",
            actor_user_id,
            None,
            now,
        )
        .await?;
    }
    tx.commit().await.context("committing save")?;
    Ok(Ok(next_revision))
}
