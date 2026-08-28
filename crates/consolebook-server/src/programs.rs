//! Versioned program configuration: programs, immutable published program
//! versions, and the typed content a version owns (ADR 0007).
//!
//! A draft version is edited by wholesale content replacement — single
//! editor with honest last-write behavior. Publishing freezes the version;
//! after that the database itself (migration 0004) rejects every mutation
//! of the version and its owned rows. Content is validated before any row
//! is written, so the composite foreign keys that enforce domain
//! invariant 5 never see a dangling reference.

use std::collections::HashMap;
use std::collections::HashSet;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqliteConnection, SqlitePool};
use time::OffsetDateTime;

use crate::audit::{self, EventKind, Subject};
use crate::capabilities::{self, Capability};

// ---- content document
//
// The same typed document is the authoring input (`replace_draft`), the
// read model (`load_content`), and the export/import payload
// (`program_export`). Strings are stored verbatim; required fields must
// contain non-whitespace content. References between parts use exact
// names, and name uniqueness is ASCII-case-insensitive to match the
// database's NOCASE indexes.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionContent {
    /// Snapshot of the program name as presented by this version. A later
    /// program rename never rewrites it.
    pub name: String,
    /// Agency-visible free-text label; presentation, never identity.
    pub label: String,
    pub description: String,
    pub phases: Vec<PhaseDef>,
    pub phase_transitions: Vec<TransitionDef>,
    pub competencies: Vec<CompetencyDef>,
    pub rating_scales: Vec<ScaleDef>,
    pub rating_modifiers: Vec<ModifierDef>,
    pub evaluation_forms: Vec<FormDef>,
    /// Version-level standards citations; competency- and task-level
    /// citations nest under their owners.
    pub citations: Vec<CitationDef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhaseDef {
    pub name: String,
    pub description: String,
    /// Presentation data (docs/domain-model.md): ordering, never progress.
    pub presentation_number: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionDef {
    pub from_phase: String,
    pub to_phase: String,
    pub kind: TransitionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKind {
    Advance,
    Remediation,
    Skip,
    Restart,
}

impl TransitionKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Advance => "advance",
            Self::Remediation => "remediation",
            Self::Skip => "skip",
            Self::Restart => "restart",
        }
    }

    fn from_db(value: &str) -> Result<Self> {
        match value {
            "advance" => Ok(Self::Advance),
            "remediation" => Ok(Self::Remediation),
            "skip" => Ok(Self::Skip),
            "restart" => Ok(Self::Restart),
            other => bail!("unknown transition kind in database: {other}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompetencyDef {
    /// Free-text grouping label; empty means uncategorized.
    pub category: String,
    pub name: String,
    pub description: String,
    pub tasks: Vec<TaskDef>,
    pub citations: Vec<CitationDef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskDef {
    pub prompt: String,
    pub citations: Vec<CitationDef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScaleDef {
    pub name: String,
    pub kind: ScaleKind,
    /// Present exactly when `kind` is `anchored_numeric`.
    pub min_value: Option<i64>,
    /// Present exactly when `kind` is `anchored_numeric`.
    pub max_value: Option<i64>,
    pub anchors: Vec<AnchorDef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScaleKind {
    AnchoredNumeric,
    PassFail,
    NarrativeOnly,
}

impl ScaleKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AnchoredNumeric => "anchored_numeric",
            Self::PassFail => "pass_fail",
            Self::NarrativeOnly => "narrative_only",
        }
    }

    fn from_db(value: &str) -> Result<Self> {
        match value {
            "anchored_numeric" => Ok(Self::AnchoredNumeric),
            "pass_fail" => Ok(Self::PassFail),
            "narrative_only" => Ok(Self::NarrativeOnly),
            other => bail!("unknown rating scale kind in database: {other}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnchorDef {
    pub value: i64,
    pub label: String,
    pub definition: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModifierDef {
    pub code: String,
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormDef {
    pub record_type: RecordType,
    pub name: String,
    pub instructions: String,
    pub competencies: Vec<FormCompetencyDef>,
    pub narratives: Vec<NarrativeDef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordType {
    DailyReport,
    WeeklySummary,
    PhaseEvaluation,
}

impl RecordType {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DailyReport => "daily_report",
            Self::WeeklySummary => "weekly_summary",
            Self::PhaseEvaluation => "phase_evaluation",
        }
    }

    fn from_db(value: &str) -> Result<Self> {
        match value {
            "daily_report" => Ok(Self::DailyReport),
            "weekly_summary" => Ok(Self::WeeklySummary),
            "phase_evaluation" => Ok(Self::PhaseEvaluation),
            other => bail!("unknown record type in database: {other}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormCompetencyDef {
    /// Exact name of a competency defined in this version.
    pub competency: String,
    /// Exact name of a rating scale defined in this version.
    pub rating_scale: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NarrativeDef {
    pub prompt: String,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CitationDef {
    /// Standards body, e.g. an accreditation program name.
    pub body: String,
    /// Edition or revision of the cited standard; may be empty.
    pub edition: String,
    pub clause: String,
    pub note: String,
}

// ---- summaries

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProgramSummary {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VersionSummary {
    pub id: i64,
    pub program_id: i64,
    pub version_number: i64,
    pub label: String,
    pub name: String,
    pub created_at: i64,
    pub published_at: Option<i64>,
}

// ---- refusals

/// Why creating a program was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum ProgramRefusal {
    CapabilityRequired,
    NameEmpty,
    NameTaken,
}

/// Why an authoring operation on a version was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum AuthorRefusal {
    CapabilityRequired,
    NoSuchProgram,
    NoSuchVersion,
    AlreadyPublished,
    Invalid(Vec<String>),
}

/// Why publishing was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum PublishRefusal {
    CapabilityRequired,
    NoSuchVersion,
    AlreadyPublished,
    Incomplete(Vec<String>),
}

// ---- validation

fn blank(value: &str) -> bool {
    value.trim().is_empty()
}

/// Detects a duplicate under the database's ASCII-case-insensitive
/// uniqueness rules.
fn note_duplicate(seen: &mut HashSet<String>, value: &str) -> bool {
    !seen.insert(value.to_ascii_lowercase())
}

fn validate_citations(problems: &mut Vec<String>, owner: &str, citations: &[CitationDef]) {
    for citation in citations {
        if blank(&citation.body) {
            problems.push(format!("{owner}: citation has an empty standards body"));
        }
        if blank(&citation.clause) {
            problems.push(format!("{owner}: citation has an empty clause"));
        }
    }
}

fn validate_phases(problems: &mut Vec<String>, content: &VersionContent) {
    let mut names = HashSet::new();
    for phase in &content.phases {
        if blank(&phase.name) {
            problems.push("phase has an empty name".to_owned());
        } else if note_duplicate(&mut names, &phase.name) {
            problems.push(format!("duplicate phase name '{}'", phase.name));
        }
    }
    let defined: HashSet<&str> = content.phases.iter().map(|p| p.name.as_str()).collect();
    let mut edges = HashSet::new();
    for transition in &content.phase_transitions {
        for endpoint in [&transition.from_phase, &transition.to_phase] {
            if !defined.contains(endpoint.as_str()) {
                problems.push(format!("transition references unknown phase '{endpoint}'"));
            }
        }
        if !edges.insert((transition.from_phase.clone(), transition.to_phase.clone())) {
            problems.push(format!(
                "duplicate transition from '{}' to '{}'",
                transition.from_phase, transition.to_phase
            ));
        }
    }
}

fn validate_competencies(problems: &mut Vec<String>, content: &VersionContent) {
    let mut names = HashSet::new();
    for competency in &content.competencies {
        if blank(&competency.name) {
            problems.push("competency has an empty name".to_owned());
            continue;
        }
        if note_duplicate(&mut names, &competency.name) {
            problems.push(format!("duplicate competency name '{}'", competency.name));
        }
        let mut prompts = HashSet::new();
        for task in &competency.tasks {
            if blank(&task.prompt) {
                problems.push(format!(
                    "competency '{}': task has an empty prompt",
                    competency.name
                ));
            } else if note_duplicate(&mut prompts, &task.prompt) {
                problems.push(format!(
                    "competency '{}': duplicate task prompt '{}'",
                    competency.name, task.prompt
                ));
            }
            validate_citations(
                problems,
                &format!("task '{}'", task.prompt),
                &task.citations,
            );
        }
        validate_citations(
            problems,
            &format!("competency '{}'", competency.name),
            &competency.citations,
        );
    }
}

fn validate_scale(problems: &mut Vec<String>, scale: &ScaleDef) {
    let mut values = HashSet::new();
    for anchor in &scale.anchors {
        if blank(&anchor.label) {
            problems.push(format!("scale '{}': anchor has an empty label", scale.name));
        }
        if !values.insert(anchor.value) {
            problems.push(format!(
                "scale '{}': duplicate anchor value {}",
                scale.name, anchor.value
            ));
        }
    }
    match scale.kind {
        ScaleKind::AnchoredNumeric => {
            let (Some(min), Some(max)) = (scale.min_value, scale.max_value) else {
                problems.push(format!(
                    "scale '{}': anchored_numeric requires min_value and max_value",
                    scale.name
                ));
                return;
            };
            if min >= max {
                problems.push(format!(
                    "scale '{}': min_value must be less than max_value",
                    scale.name
                ));
            }
            if scale.anchors.is_empty() {
                problems.push(format!(
                    "scale '{}': anchored_numeric requires at least one anchor",
                    scale.name
                ));
            }
            for anchor in &scale.anchors {
                if anchor.value < min || anchor.value > max {
                    problems.push(format!(
                        "scale '{}': anchor value {} is outside {min}..={max}",
                        scale.name, anchor.value
                    ));
                }
            }
        }
        ScaleKind::PassFail => {
            if scale.min_value.is_some() || scale.max_value.is_some() {
                problems.push(format!(
                    "scale '{}': pass_fail does not take numeric bounds",
                    scale.name
                ));
            }
            let values: Vec<i64> = scale.anchors.iter().map(|a| a.value).collect();
            if !(values.len() == 2 && values.contains(&0) && values.contains(&1)) {
                problems.push(format!(
                    "scale '{}': pass_fail requires exactly two anchors with values 0 and 1",
                    scale.name
                ));
            }
        }
        ScaleKind::NarrativeOnly => {
            if scale.min_value.is_some() || scale.max_value.is_some() {
                problems.push(format!(
                    "scale '{}': narrative_only does not take numeric bounds",
                    scale.name
                ));
            }
            if !scale.anchors.is_empty() {
                problems.push(format!(
                    "scale '{}': narrative_only takes no anchors",
                    scale.name
                ));
            }
        }
    }
}

fn validate_forms(problems: &mut Vec<String>, content: &VersionContent) {
    let competencies: HashSet<&str> = content
        .competencies
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    let scales: HashSet<&str> = content
        .rating_scales
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    let mut names = HashSet::new();
    for form in &content.evaluation_forms {
        if blank(&form.name) {
            problems.push("evaluation form has an empty name".to_owned());
            continue;
        }
        if note_duplicate(&mut names, &form.name) {
            problems.push(format!("duplicate evaluation form name '{}'", form.name));
        }
        let mut bound = HashSet::new();
        for binding in &form.competencies {
            if !competencies.contains(binding.competency.as_str()) {
                problems.push(format!(
                    "form '{}': references unknown competency '{}'",
                    form.name, binding.competency
                ));
            }
            if !scales.contains(binding.rating_scale.as_str()) {
                problems.push(format!(
                    "form '{}': references unknown rating scale '{}'",
                    form.name, binding.rating_scale
                ));
            }
            if !bound.insert(binding.competency.clone()) {
                problems.push(format!(
                    "form '{}': competency '{}' is bound more than once",
                    form.name, binding.competency
                ));
            }
        }
        for narrative in &form.narratives {
            if blank(&narrative.prompt) {
                problems.push(format!(
                    "form '{}': narrative has an empty prompt",
                    form.name
                ));
            }
        }
    }
}

/// Structural validation of a content document: required text present,
/// names unique under the database's case-insensitive rules, and every
/// cross-reference resolving inside the document. Returns problems;
/// empty means valid.
#[must_use]
pub fn validate_content(content: &VersionContent) -> Vec<String> {
    let mut problems = Vec::new();
    if blank(&content.name) {
        problems.push("version has an empty program name".to_owned());
    }
    validate_phases(&mut problems, content);
    validate_competencies(&mut problems, content);
    let mut scale_names = HashSet::new();
    for scale in &content.rating_scales {
        if blank(&scale.name) {
            problems.push("rating scale has an empty name".to_owned());
            continue;
        }
        if note_duplicate(&mut scale_names, &scale.name) {
            problems.push(format!("duplicate rating scale name '{}'", scale.name));
        }
        validate_scale(&mut problems, scale);
    }
    let mut codes = HashSet::new();
    for modifier in &content.rating_modifiers {
        if blank(&modifier.code) || blank(&modifier.label) {
            problems.push("rating modifier has an empty code or label".to_owned());
        } else if note_duplicate(&mut codes, &modifier.code) {
            problems.push(format!(
                "duplicate rating modifier code '{}'",
                modifier.code
            ));
        }
    }
    validate_forms(&mut problems, content);
    validate_citations(&mut problems, "version", &content.citations);
    problems
}

// ---- services

async fn holds_manage_programs(pool: &SqlitePool, user_id: i64) -> Result<bool> {
    capabilities::user_has(pool, user_id, Capability::ManagePrograms).await
}

/// Creates a program identity. The name is the mutable discovery name;
/// each version snapshots the name it presents.
pub async fn create_program(
    pool: &SqlitePool,
    actor_user_id: i64,
    name: &str,
) -> Result<std::result::Result<i64, ProgramRefusal>> {
    if !holds_manage_programs(pool, actor_user_id).await? {
        return Ok(Err(ProgramRefusal::CapabilityRequired));
    }
    let name = name.trim();
    if name.is_empty() {
        return Ok(Err(ProgramRefusal::NameEmpty));
    }
    let mut tx = pool.begin().await.context("starting program creation")?;
    let taken: Option<i64> =
        sqlx::query_scalar("SELECT id FROM program WHERE name = ?1 COLLATE NOCASE")
            .bind(name)
            .fetch_optional(&mut *tx)
            .await
            .context("checking program name")?;
    if taken.is_some() {
        return Ok(Err(ProgramRefusal::NameTaken));
    }
    let program_id = insert_program(&mut tx, name, actor_user_id).await?;
    tx.commit().await.context("committing program creation")?;
    Ok(Ok(program_id))
}

pub(crate) async fn insert_program(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    name: &str,
    actor_user_id: i64,
) -> Result<i64> {
    let result =
        sqlx::query("INSERT INTO program (name, created_at, created_by) VALUES (?1, ?2, ?3)")
            .bind(name)
            .bind(OffsetDateTime::now_utc().unix_timestamp())
            .bind(actor_user_id)
            .execute(&mut **tx)
            .await
            .context("creating program")?;
    let program_id = result.last_insert_rowid();
    audit::record_for_subject(
        &mut **tx,
        EventKind::ProgramCreated,
        Some(actor_user_id),
        Subject::Program(program_id),
    )
    .await?;
    Ok(program_id)
}

pub async fn list_programs(pool: &SqlitePool) -> Result<Vec<ProgramSummary>> {
    let rows = sqlx::query("SELECT id, name, created_at FROM program ORDER BY name COLLATE NOCASE")
        .fetch_all(pool)
        .await
        .context("listing programs")?;
    Ok(rows
        .iter()
        .map(|row| ProgramSummary {
            id: row.get("id"),
            name: row.get("name"),
            created_at: row.get("created_at"),
        })
        .collect())
}

/// Looks up one program's summary.
pub async fn get_program(pool: &SqlitePool, program_id: i64) -> Result<Option<ProgramSummary>> {
    let row = sqlx::query("SELECT id, name, created_at FROM program WHERE id = ?1")
        .bind(program_id)
        .fetch_optional(pool)
        .await
        .context("looking up program")?;
    Ok(row.as_ref().map(|row| ProgramSummary {
        id: row.get("id"),
        name: row.get("name"),
        created_at: row.get("created_at"),
    }))
}

/// Looks up one version's summary.
pub async fn version_summary(pool: &SqlitePool, version_id: i64) -> Result<Option<VersionSummary>> {
    let row = sqlx::query(
        "SELECT id, program_id, version_number, label, name, created_at, published_at
         FROM program_version WHERE id = ?1",
    )
    .bind(version_id)
    .fetch_optional(pool)
    .await
    .context("looking up program version")?;
    Ok(row.as_ref().map(|row| VersionSummary {
        id: row.get("id"),
        program_id: row.get("program_id"),
        version_number: row.get("version_number"),
        label: row.get("label"),
        name: row.get("name"),
        created_at: row.get("created_at"),
        published_at: row.get("published_at"),
    }))
}

pub async fn list_versions(pool: &SqlitePool, program_id: i64) -> Result<Vec<VersionSummary>> {
    let rows = sqlx::query(
        "SELECT id, program_id, version_number, label, name, created_at, published_at
         FROM program_version WHERE program_id = ?1 ORDER BY version_number",
    )
    .bind(program_id)
    .fetch_all(pool)
    .await
    .context("listing program versions")?;
    Ok(rows
        .iter()
        .map(|row| VersionSummary {
            id: row.get("id"),
            program_id: row.get("program_id"),
            version_number: row.get("version_number"),
            label: row.get("label"),
            name: row.get("name"),
            created_at: row.get("created_at"),
            published_at: row.get("published_at"),
        })
        .collect())
}

/// Creates a draft version of `program_id` holding `content`, assigning
/// the next monotonic version number.
pub async fn create_version(
    pool: &SqlitePool,
    actor_user_id: i64,
    program_id: i64,
    content: &VersionContent,
) -> Result<std::result::Result<i64, AuthorRefusal>> {
    if !holds_manage_programs(pool, actor_user_id).await? {
        return Ok(Err(AuthorRefusal::CapabilityRequired));
    }
    let problems = validate_content(content);
    if !problems.is_empty() {
        return Ok(Err(AuthorRefusal::Invalid(problems)));
    }
    let mut tx = pool.begin().await.context("starting version creation")?;
    let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM program WHERE id = ?1")
        .bind(program_id)
        .fetch_optional(&mut *tx)
        .await
        .context("checking program")?;
    if exists.is_none() {
        return Ok(Err(AuthorRefusal::NoSuchProgram));
    }
    let version_id = insert_version(
        &mut tx,
        program_id,
        content,
        actor_user_id,
        EventKind::ProgramVersionCreated,
    )
    .await?;
    tx.commit().await.context("committing version creation")?;
    Ok(Ok(version_id))
}

/// Inserts a draft version row plus its content and records the lifecycle
/// audit event. Callers have already validated the content.
pub(crate) async fn insert_version(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    program_id: i64,
    content: &VersionContent,
    actor_user_id: i64,
    kind: EventKind,
) -> Result<i64> {
    let next_number: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(version_number), 0) + 1 FROM program_version WHERE program_id = ?1",
    )
    .bind(program_id)
    .fetch_one(&mut **tx)
    .await
    .context("numbering version")?;
    let result = sqlx::query(
        "INSERT INTO program_version
             (program_id, version_number, label, name, description, created_at, created_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(program_id)
    .bind(next_number)
    .bind(&content.label)
    .bind(&content.name)
    .bind(&content.description)
    .bind(OffsetDateTime::now_utc().unix_timestamp())
    .bind(actor_user_id)
    .execute(&mut **tx)
    .await
    .context("creating program version")?;
    let version_id = result.last_insert_rowid();
    insert_content(tx, version_id, content).await?;
    audit::record_for_subject(
        &mut **tx,
        kind,
        Some(actor_user_id),
        Subject::ProgramVersion(version_id),
    )
    .await?;
    Ok(version_id)
}

/// Replaces a draft's entire content — single editor, honest last write
/// (ADR 0007). Refused once the version is published.
pub async fn replace_draft(
    pool: &SqlitePool,
    actor_user_id: i64,
    version_id: i64,
    content: &VersionContent,
) -> Result<std::result::Result<(), AuthorRefusal>> {
    if !holds_manage_programs(pool, actor_user_id).await? {
        return Ok(Err(AuthorRefusal::CapabilityRequired));
    }
    let problems = validate_content(content);
    if !problems.is_empty() {
        return Ok(Err(AuthorRefusal::Invalid(problems)));
    }
    let mut tx = pool.begin().await.context("starting draft replacement")?;
    match version_state(&mut tx, version_id).await? {
        VersionState::Missing => return Ok(Err(AuthorRefusal::NoSuchVersion)),
        VersionState::Published => return Ok(Err(AuthorRefusal::AlreadyPublished)),
        VersionState::Draft => {}
    }
    delete_content(&mut tx, version_id).await?;
    sqlx::query("UPDATE program_version SET label = ?2, name = ?3, description = ?4 WHERE id = ?1")
        .bind(version_id)
        .bind(&content.label)
        .bind(&content.name)
        .bind(&content.description)
        .execute(&mut *tx)
        .await
        .context("updating version row")?;
    insert_content(&mut tx, version_id, content).await?;
    tx.commit().await.context("committing draft replacement")?;
    Ok(Ok(()))
}

/// Publishes a draft: completeness-checks it, stamps `published_at`, and
/// leaves all further mutation to be refused by the database.
pub async fn publish_version(
    pool: &SqlitePool,
    actor_user_id: i64,
    version_id: i64,
) -> Result<std::result::Result<(), PublishRefusal>> {
    if !holds_manage_programs(pool, actor_user_id).await? {
        return Ok(Err(PublishRefusal::CapabilityRequired));
    }
    let mut tx = pool.begin().await.context("starting publish")?;
    match version_state(&mut tx, version_id).await? {
        VersionState::Missing => return Ok(Err(PublishRefusal::NoSuchVersion)),
        VersionState::Published => return Ok(Err(PublishRefusal::AlreadyPublished)),
        VersionState::Draft => {}
    }
    let empty_forms: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM evaluation_form
         WHERE program_version_id = ?1
           AND id NOT IN (SELECT evaluation_form_id FROM form_competency WHERE program_version_id = ?1)
           AND id NOT IN (SELECT evaluation_form_id FROM form_narrative WHERE program_version_id = ?1)
         ORDER BY name",
    )
    .bind(version_id)
    .fetch_all(&mut *tx)
    .await
    .context("checking form completeness")?;
    if !empty_forms.is_empty() {
        let problems = empty_forms
            .iter()
            .map(|name| format!("form '{name}' has no competencies and no narratives"))
            .collect();
        return Ok(Err(PublishRefusal::Incomplete(problems)));
    }
    let stamped = sqlx::query(
        "UPDATE program_version SET published_at = ?2, published_by = ?3
         WHERE id = ?1 AND published_at IS NULL",
    )
    .bind(version_id)
    .bind(OffsetDateTime::now_utc().unix_timestamp())
    .bind(actor_user_id)
    .execute(&mut *tx)
    .await
    .context("publishing version")?;
    if stamped.rows_affected() != 1 {
        return Ok(Err(PublishRefusal::AlreadyPublished));
    }
    audit::record_for_subject(
        &mut *tx,
        EventKind::ProgramVersionPublished,
        Some(actor_user_id),
        Subject::ProgramVersion(version_id),
    )
    .await?;
    tx.commit().await.context("committing publish")?;
    Ok(Ok(()))
}

/// Deletes a draft version and its content. Published versions are
/// immutable and refuse this at the database as well as here.
pub async fn discard_draft(
    pool: &SqlitePool,
    actor_user_id: i64,
    version_id: i64,
) -> Result<std::result::Result<(), AuthorRefusal>> {
    if !holds_manage_programs(pool, actor_user_id).await? {
        return Ok(Err(AuthorRefusal::CapabilityRequired));
    }
    let mut tx = pool.begin().await.context("starting draft discard")?;
    match version_state(&mut tx, version_id).await? {
        VersionState::Missing => return Ok(Err(AuthorRefusal::NoSuchVersion)),
        VersionState::Published => return Ok(Err(AuthorRefusal::AlreadyPublished)),
        VersionState::Draft => {}
    }
    delete_content(&mut tx, version_id).await?;
    sqlx::query("DELETE FROM program_version WHERE id = ?1")
        .bind(version_id)
        .execute(&mut *tx)
        .await
        .context("deleting draft version")?;
    audit::record_for_subject(
        &mut *tx,
        EventKind::ProgramVersionDiscarded,
        Some(actor_user_id),
        Subject::ProgramVersion(version_id),
    )
    .await?;
    tx.commit().await.context("committing draft discard")?;
    Ok(Ok(()))
}

enum VersionState {
    Missing,
    Draft,
    Published,
}

async fn version_state(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    version_id: i64,
) -> Result<VersionState> {
    let row = sqlx::query("SELECT published_at FROM program_version WHERE id = ?1")
        .bind(version_id)
        .fetch_optional(&mut **tx)
        .await
        .context("checking version state")?;
    Ok(match row {
        None => VersionState::Missing,
        Some(row) => {
            let published_at: Option<i64> = row.get("published_at");
            if published_at.is_some() {
                VersionState::Published
            } else {
                VersionState::Draft
            }
        }
    })
}

// ---- content writes

async fn insert_citation(
    conn: &mut SqliteConnection,
    version_id: i64,
    competency_id: Option<i64>,
    task_id: Option<i64>,
    citation: &CitationDef,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO standards_citation
             (program_version_id, competency_id, task_id, body, edition, clause, note)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(version_id)
    .bind(competency_id)
    .bind(task_id)
    .bind(&citation.body)
    .bind(&citation.edition)
    .bind(&citation.clause)
    .bind(&citation.note)
    .execute(conn)
    .await
    .context("inserting standards citation")?;
    Ok(())
}

async fn insert_phases(
    conn: &mut SqliteConnection,
    version_id: i64,
    content: &VersionContent,
) -> Result<()> {
    let mut phase_ids: HashMap<&str, i64> = HashMap::new();
    for phase in &content.phases {
        let result = sqlx::query(
            "INSERT INTO phase (program_version_id, name, description, presentation_number)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(version_id)
        .bind(&phase.name)
        .bind(&phase.description)
        .bind(phase.presentation_number)
        .execute(&mut *conn)
        .await
        .context("inserting phase")?;
        phase_ids.insert(phase.name.as_str(), result.last_insert_rowid());
    }
    for transition in &content.phase_transitions {
        sqlx::query(
            "INSERT INTO phase_transition
                 (program_version_id, from_phase_id, to_phase_id, kind)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(version_id)
        .bind(phase_ids[transition.from_phase.as_str()])
        .bind(phase_ids[transition.to_phase.as_str()])
        .bind(transition.kind.as_str())
        .execute(&mut *conn)
        .await
        .context("inserting phase transition")?;
    }
    Ok(())
}

async fn insert_competencies(
    conn: &mut SqliteConnection,
    version_id: i64,
    content: &VersionContent,
) -> Result<HashMap<String, i64>> {
    let mut competency_ids: HashMap<String, i64> = HashMap::new();
    for (order, competency) in (0_i64..).zip(&content.competencies) {
        let result = sqlx::query(
            "INSERT INTO competency (program_version_id, category, name, description, sort_order)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(version_id)
        .bind(&competency.category)
        .bind(&competency.name)
        .bind(&competency.description)
        .bind(order)
        .execute(&mut *conn)
        .await
        .context("inserting competency")?;
        let competency_id = result.last_insert_rowid();
        competency_ids.insert(competency.name.clone(), competency_id);
        for (task_order, task) in (0_i64..).zip(&competency.tasks) {
            let inserted = sqlx::query(
                "INSERT INTO task (program_version_id, competency_id, prompt, sort_order)
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .bind(version_id)
            .bind(competency_id)
            .bind(&task.prompt)
            .bind(task_order)
            .execute(&mut *conn)
            .await
            .context("inserting task")?;
            let task_id = inserted.last_insert_rowid();
            for citation in &task.citations {
                insert_citation(&mut *conn, version_id, None, Some(task_id), citation).await?;
            }
        }
        for citation in &competency.citations {
            insert_citation(&mut *conn, version_id, Some(competency_id), None, citation).await?;
        }
    }
    Ok(competency_ids)
}

async fn insert_scales(
    conn: &mut SqliteConnection,
    version_id: i64,
    content: &VersionContent,
) -> Result<HashMap<String, i64>> {
    let mut scale_ids: HashMap<String, i64> = HashMap::new();
    for scale in &content.rating_scales {
        let result = sqlx::query(
            "INSERT INTO rating_scale (program_version_id, name, kind, min_value, max_value)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(version_id)
        .bind(&scale.name)
        .bind(scale.kind.as_str())
        .bind(scale.min_value)
        .bind(scale.max_value)
        .execute(&mut *conn)
        .await
        .context("inserting rating scale")?;
        let scale_id = result.last_insert_rowid();
        scale_ids.insert(scale.name.clone(), scale_id);
        for anchor in &scale.anchors {
            sqlx::query(
                "INSERT INTO rating_anchor
                     (program_version_id, rating_scale_id, value, label, definition)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .bind(version_id)
            .bind(scale_id)
            .bind(anchor.value)
            .bind(&anchor.label)
            .bind(&anchor.definition)
            .execute(&mut *conn)
            .await
            .context("inserting rating anchor")?;
        }
    }
    for modifier in &content.rating_modifiers {
        sqlx::query(
            "INSERT INTO rating_modifier (program_version_id, code, label, description)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(version_id)
        .bind(&modifier.code)
        .bind(&modifier.label)
        .bind(&modifier.description)
        .execute(&mut *conn)
        .await
        .context("inserting rating modifier")?;
    }
    Ok(scale_ids)
}

async fn insert_forms(
    conn: &mut SqliteConnection,
    version_id: i64,
    content: &VersionContent,
    competency_ids: &HashMap<String, i64>,
    scale_ids: &HashMap<String, i64>,
) -> Result<()> {
    for form in &content.evaluation_forms {
        let result = sqlx::query(
            "INSERT INTO evaluation_form (program_version_id, record_type, name, instructions)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(version_id)
        .bind(form.record_type.as_str())
        .bind(&form.name)
        .bind(&form.instructions)
        .execute(&mut *conn)
        .await
        .context("inserting evaluation form")?;
        let form_id = result.last_insert_rowid();
        for (order, binding) in (0_i64..).zip(&form.competencies) {
            sqlx::query(
                "INSERT INTO form_competency
                     (program_version_id, evaluation_form_id, competency_id, rating_scale_id, sort_order)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .bind(version_id)
            .bind(form_id)
            .bind(competency_ids[&binding.competency])
            .bind(scale_ids[&binding.rating_scale])
            .bind(order)
            .execute(&mut *conn)
            .await
            .context("inserting form competency")?;
        }
        for (order, narrative) in (0_i64..).zip(&form.narratives) {
            sqlx::query(
                "INSERT INTO form_narrative
                     (program_version_id, evaluation_form_id, prompt, required, sort_order)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .bind(version_id)
            .bind(form_id)
            .bind(&narrative.prompt)
            .bind(i64::from(narrative.required))
            .bind(order)
            .execute(&mut *conn)
            .await
            .context("inserting form narrative")?;
        }
    }
    Ok(())
}

/// Writes every owned row of a validated content document.
async fn insert_content(
    conn: &mut SqliteConnection,
    version_id: i64,
    content: &VersionContent,
) -> Result<()> {
    insert_phases(&mut *conn, version_id, content).await?;
    let competency_ids = insert_competencies(&mut *conn, version_id, content).await?;
    let scale_ids = insert_scales(&mut *conn, version_id, content).await?;
    insert_forms(&mut *conn, version_id, content, &competency_ids, &scale_ids).await?;
    for citation in &content.citations {
        insert_citation(&mut *conn, version_id, None, None, citation).await?;
    }
    Ok(())
}

/// Deletes every owned row of a draft version, children before parents so
/// foreign keys hold throughout.
async fn delete_content(conn: &mut SqliteConnection, version_id: i64) -> Result<()> {
    for statement in [
        "DELETE FROM standards_citation WHERE program_version_id = ?1",
        "DELETE FROM form_narrative WHERE program_version_id = ?1",
        "DELETE FROM form_competency WHERE program_version_id = ?1",
        "DELETE FROM evaluation_form WHERE program_version_id = ?1",
        "DELETE FROM rating_anchor WHERE program_version_id = ?1",
        "DELETE FROM rating_scale WHERE program_version_id = ?1",
        "DELETE FROM rating_modifier WHERE program_version_id = ?1",
        "DELETE FROM task WHERE program_version_id = ?1",
        "DELETE FROM competency WHERE program_version_id = ?1",
        "DELETE FROM phase_transition WHERE program_version_id = ?1",
        "DELETE FROM phase WHERE program_version_id = ?1",
    ] {
        sqlx::query(statement)
            .bind(version_id)
            .execute(&mut *conn)
            .await
            .context("deleting draft content")?;
    }
    Ok(())
}

// ---- content reads

/// Loads a version's complete content document, or `None` when the
/// version does not exist. Arrays come back in the deterministic export
/// order (authored order where one exists, content order otherwise).
pub async fn load_content(pool: &SqlitePool, version_id: i64) -> Result<Option<VersionContent>> {
    // One transaction so every query reads the same snapshot.
    let mut tx = pool.begin().await.context("starting content load")?;
    let Some(header) =
        sqlx::query("SELECT name, label, description FROM program_version WHERE id = ?1")
            .bind(version_id)
            .fetch_optional(&mut *tx)
            .await
            .context("loading version row")?
    else {
        return Ok(None);
    };
    let mut content = VersionContent {
        name: header.get("name"),
        label: header.get("label"),
        description: header.get("description"),
        phases: Vec::new(),
        phase_transitions: Vec::new(),
        competencies: Vec::new(),
        rating_scales: Vec::new(),
        rating_modifiers: Vec::new(),
        evaluation_forms: Vec::new(),
        citations: Vec::new(),
    };
    load_phases(&mut tx, version_id, &mut content).await?;
    let competency_index = load_competencies(&mut tx, version_id, &mut content).await?;
    load_scales(&mut tx, version_id, &mut content).await?;
    load_forms(&mut tx, version_id, &mut content).await?;
    load_citations(&mut tx, version_id, &mut content, &competency_index).await?;
    Ok(Some(content))
}

async fn load_phases(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    version_id: i64,
    content: &mut VersionContent,
) -> Result<()> {
    let rows = sqlx::query(
        "SELECT name, description, presentation_number FROM phase
         WHERE program_version_id = ?1 ORDER BY presentation_number, name",
    )
    .bind(version_id)
    .fetch_all(&mut **tx)
    .await
    .context("loading phases")?;
    content.phases = rows
        .iter()
        .map(|row| PhaseDef {
            name: row.get("name"),
            description: row.get("description"),
            presentation_number: row.get("presentation_number"),
        })
        .collect();
    let rows = sqlx::query(
        "SELECT f.name AS from_name, t.name AS to_name, pt.kind
         FROM phase_transition pt
         JOIN phase f ON f.id = pt.from_phase_id
         JOIN phase t ON t.id = pt.to_phase_id
         WHERE pt.program_version_id = ?1
         ORDER BY f.name, t.name",
    )
    .bind(version_id)
    .fetch_all(&mut **tx)
    .await
    .context("loading phase transitions")?;
    for row in &rows {
        content.phase_transitions.push(TransitionDef {
            from_phase: row.get("from_name"),
            to_phase: row.get("to_name"),
            kind: TransitionKind::from_db(row.get("kind"))?,
        });
    }
    Ok(())
}

/// Loads competencies and their tasks; returns row-id lookup maps used to
/// route citations.
async fn load_competencies(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    version_id: i64,
    content: &mut VersionContent,
) -> Result<CompetencyIndex> {
    let rows = sqlx::query(
        "SELECT id, category, name, description FROM competency
         WHERE program_version_id = ?1 ORDER BY sort_order, name",
    )
    .bind(version_id)
    .fetch_all(&mut **tx)
    .await
    .context("loading competencies")?;
    let mut index = CompetencyIndex::default();
    for row in &rows {
        let id: i64 = row.get("id");
        index
            .by_competency_row
            .insert(id, content.competencies.len());
        content.competencies.push(CompetencyDef {
            category: row.get("category"),
            name: row.get("name"),
            description: row.get("description"),
            tasks: Vec::new(),
            citations: Vec::new(),
        });
    }
    let rows = sqlx::query(
        "SELECT id, competency_id, prompt FROM task
         WHERE program_version_id = ?1 ORDER BY sort_order, prompt",
    )
    .bind(version_id)
    .fetch_all(&mut **tx)
    .await
    .context("loading tasks")?;
    for row in &rows {
        let competency_row: i64 = row.get("competency_id");
        let competency_slot = index.by_competency_row[&competency_row];
        let tasks = &mut content.competencies[competency_slot].tasks;
        index
            .by_task_row
            .insert(row.get("id"), (competency_slot, tasks.len()));
        tasks.push(TaskDef {
            prompt: row.get("prompt"),
            citations: Vec::new(),
        });
    }
    Ok(index)
}

#[derive(Default)]
struct CompetencyIndex {
    /// competency row id -> index into `content.competencies`
    by_competency_row: HashMap<i64, usize>,
    /// task row id -> (competency index, task index)
    by_task_row: HashMap<i64, (usize, usize)>,
}

async fn load_scales(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    version_id: i64,
    content: &mut VersionContent,
) -> Result<()> {
    let rows = sqlx::query(
        "SELECT id, name, kind, min_value, max_value FROM rating_scale
         WHERE program_version_id = ?1 ORDER BY name",
    )
    .bind(version_id)
    .fetch_all(&mut **tx)
    .await
    .context("loading rating scales")?;
    let mut slot_by_row: HashMap<i64, usize> = HashMap::new();
    for row in &rows {
        slot_by_row.insert(row.get("id"), content.rating_scales.len());
        content.rating_scales.push(ScaleDef {
            name: row.get("name"),
            kind: ScaleKind::from_db(row.get("kind"))?,
            min_value: row.get("min_value"),
            max_value: row.get("max_value"),
            anchors: Vec::new(),
        });
    }
    let rows = sqlx::query(
        "SELECT rating_scale_id, value, label, definition FROM rating_anchor
         WHERE program_version_id = ?1 ORDER BY value",
    )
    .bind(version_id)
    .fetch_all(&mut **tx)
    .await
    .context("loading rating anchors")?;
    for row in &rows {
        let scale_row: i64 = row.get("rating_scale_id");
        content.rating_scales[slot_by_row[&scale_row]]
            .anchors
            .push(AnchorDef {
                value: row.get("value"),
                label: row.get("label"),
                definition: row.get("definition"),
            });
    }
    let rows = sqlx::query(
        "SELECT code, label, description FROM rating_modifier
         WHERE program_version_id = ?1 ORDER BY code",
    )
    .bind(version_id)
    .fetch_all(&mut **tx)
    .await
    .context("loading rating modifiers")?;
    content.rating_modifiers = rows
        .iter()
        .map(|row| ModifierDef {
            code: row.get("code"),
            label: row.get("label"),
            description: row.get("description"),
        })
        .collect();
    Ok(())
}

async fn load_forms(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    version_id: i64,
    content: &mut VersionContent,
) -> Result<()> {
    let rows = sqlx::query(
        "SELECT id, record_type, name, instructions FROM evaluation_form
         WHERE program_version_id = ?1 ORDER BY name",
    )
    .bind(version_id)
    .fetch_all(&mut **tx)
    .await
    .context("loading evaluation forms")?;
    let mut slot_by_row: HashMap<i64, usize> = HashMap::new();
    for row in &rows {
        slot_by_row.insert(row.get("id"), content.evaluation_forms.len());
        content.evaluation_forms.push(FormDef {
            record_type: RecordType::from_db(row.get("record_type"))?,
            name: row.get("name"),
            instructions: row.get("instructions"),
            competencies: Vec::new(),
            narratives: Vec::new(),
        });
    }
    let rows = sqlx::query(
        "SELECT fc.evaluation_form_id, c.name AS competency, s.name AS rating_scale
         FROM form_competency fc
         JOIN competency c ON c.id = fc.competency_id
         JOIN rating_scale s ON s.id = fc.rating_scale_id
         WHERE fc.program_version_id = ?1
         ORDER BY fc.sort_order, c.name",
    )
    .bind(version_id)
    .fetch_all(&mut **tx)
    .await
    .context("loading form competencies")?;
    for row in &rows {
        let form_row: i64 = row.get("evaluation_form_id");
        content.evaluation_forms[slot_by_row[&form_row]]
            .competencies
            .push(FormCompetencyDef {
                competency: row.get("competency"),
                rating_scale: row.get("rating_scale"),
            });
    }
    let rows = sqlx::query(
        "SELECT evaluation_form_id, prompt, required FROM form_narrative
         WHERE program_version_id = ?1 ORDER BY sort_order, prompt",
    )
    .bind(version_id)
    .fetch_all(&mut **tx)
    .await
    .context("loading form narratives")?;
    for row in &rows {
        let form_row: i64 = row.get("evaluation_form_id");
        let required: i64 = row.get("required");
        content.evaluation_forms[slot_by_row[&form_row]]
            .narratives
            .push(NarrativeDef {
                prompt: row.get("prompt"),
                required: required != 0,
            });
    }
    Ok(())
}

async fn load_citations(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    version_id: i64,
    content: &mut VersionContent,
    index: &CompetencyIndex,
) -> Result<()> {
    let rows = sqlx::query(
        "SELECT competency_id, task_id, body, edition, clause, note
         FROM standards_citation WHERE program_version_id = ?1
         ORDER BY body, edition, clause, note",
    )
    .bind(version_id)
    .fetch_all(&mut **tx)
    .await
    .context("loading standards citations")?;
    for row in &rows {
        let citation = CitationDef {
            body: row.get("body"),
            edition: row.get("edition"),
            clause: row.get("clause"),
            note: row.get("note"),
        };
        let competency_id: Option<i64> = row.get("competency_id");
        let task_id: Option<i64> = row.get("task_id");
        match (competency_id, task_id) {
            (Some(competency_row), None) => {
                let slot = index.by_competency_row[&competency_row];
                content.competencies[slot].citations.push(citation);
            }
            (None, Some(task_row)) => {
                let (competency_slot, task_slot) = index.by_task_row[&task_row];
                content.competencies[competency_slot].tasks[task_slot]
                    .citations
                    .push(citation);
            }
            (None, None) => content.citations.push(citation),
            (Some(_), Some(_)) => {
                bail!("citation targets both a competency and a task; the schema forbids this")
            }
        }
    }
    Ok(())
}
