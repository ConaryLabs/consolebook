//! The typed shape of a finalized record envelope, as readers see it
//! (ADR 0011 for record schema 1, ADR 0013 for schema 2).
//!
//! `finalization::envelope` is the producing side and the one owner of
//! what goes into a record. This is the reading side: the same member
//! set, typed, with every object refusing members the schema does not
//! name, every nullable member required to be present, and every closed
//! vocabulary (event kinds, decisions, scale kinds, record types) an
//! enum, so "these bytes are a schema-1 or schema-2 envelope" is a
//! typed contract rather than a spot check of a few members. Export verification reads
//! through it; packets and rendering (Milestone 5) will too. A schema
//! bump extends this shape and never reinterprets stored bytes.

use std::fmt;

use serde::{Deserialize, Deserializer};

use crate::canonical;
use crate::draft_review::ReviewDecisionKind;
use crate::programs::{RecordType, ScaleKind};

/// A user as the envelope presents them: identity plus the names shown
/// at finalization.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub display_name: String,
}

/// Attachments exist in no schema yet: the array is always empty. The
/// element type is uninhabited, so no value of any shape deserializes
/// into it and a non-empty array is not a schema-1 or schema-2 record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attachment {}

impl<'de> Deserialize<'de> for Attachment {
    fn deserialize<D>(_: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Err(<D::Error as serde::de::Error>::custom(
            "attachments exist in no known record schema",
        ))
    }
}

/// The contributor-event vocabulary a record's attribution stream uses
/// (ADR 0008; migration 0008's closed set).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContributorEventKind {
    Created,
    Contributed,
    OwnershipTransferred,
    SubmittedForReview,
    ReviewDecided,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttributionEvent {
    pub kind: ContributorEventKind,
    pub actor: User,
    #[serde(deserialize_with = "nullable")]
    pub to: Option<User>,
    pub recorded_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Content {
    pub narratives: Vec<Narrative>,
    pub ratings: Vec<Rating>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Narrative {
    pub prompt: String,
    pub required: bool,
    #[serde(deserialize_with = "nullable")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rating {
    pub competency: Competency,
    pub scale: Scale,
    #[serde(deserialize_with = "nullable")]
    pub value: Option<i64>,
    pub not_observed: bool,
    pub modifiers: Vec<Modifier>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Competency {
    pub category: String,
    pub name: String,
    pub description: String,
    pub tasks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scale {
    pub name: String,
    pub kind: ScaleKind,
    #[serde(deserialize_with = "nullable")]
    pub min_value: Option<i64>,
    #[serde(deserialize_with = "nullable")]
    pub max_value: Option<i64>,
    pub anchors: Vec<Anchor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    pub value: i64,
    pub label: String,
    pub definition: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Modifier {
    pub code: String,
    pub label: String,
    pub description: String,
}

/// One pinned daily-report version a weekly summary covered (schema 2).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DailyReport {
    pub content_hash: String,
    pub record_id: i64,
    pub version_number: i64,
}

/// The `daily_reports` member: absent in schema 1, an array in schema
/// 2. Three states are told apart — absent, `null` (never valid), and
/// present — because schema 1 requires the member to be missing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DailyReports {
    #[default]
    Absent,
    Present(Vec<DailyReport>),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Finalization {
    pub finalized_at: i64,
    pub finalized_by: User,
    pub policy: Policy,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub review_approved: bool,
    pub required_narratives: bool,
    pub ratings_complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Form {
    pub name: String,
    pub instructions: String,
    pub record_type: RecordType,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Program {
    pub name: String,
    pub version_number: i64,
    pub label: String,
}

/// The record's own statement of identity and lineage.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordIdentity {
    pub id: i64,
    pub version_number: i64,
    pub record_schema: i64,
    #[serde(deserialize_with = "nullable")]
    pub predecessor_content_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewDecision {
    pub reviewer: User,
    pub decision: ReviewDecisionKind,
    pub comment: String,
    pub decided_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub business_date: String,
    pub timezone: String,
    pub local_start: String,
    #[serde(deserialize_with = "nullable")]
    pub local_end: Option<String>,
    pub utc_start: i64,
    #[serde(deserialize_with = "nullable")]
    pub utc_end: Option<i64>,
    #[serde(deserialize_with = "nullable")]
    pub disposition: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub phase: Option<Phase>,
    pub trainers: Vec<User>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Phase {
    pub name: String,
    pub presentation_number: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trainee {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub employee_id: String,
    pub title: String,
}

/// The envelope's top level: every member ADR 0011 names, plus
/// `daily_reports` for schema 2 (ADR 0013), and nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub attachments: Vec<Attachment>,
    pub attribution: Vec<AttributionEvent>,
    pub canonicalization: String,
    pub content: Content,
    #[serde(default, deserialize_with = "daily_reports")]
    pub daily_reports: DailyReports,
    pub finalization: Finalization,
    pub form: Form,
    pub instance: String,
    pub program: Program,
    pub record: RecordIdentity,
    pub review: Vec<ReviewDecision>,
    pub sessions: Vec<Session>,
    pub trainee: Trainee,
}

/// A member that may be `null` but must be present: the producer always
/// writes it, so a document without it is not an envelope.
pub(crate) fn nullable<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer)
}

fn daily_reports<'de, D>(deserializer: D) -> Result<DailyReports, D::Error>
where
    D: Deserializer<'de>,
{
    Vec::<DailyReport>::deserialize(deserializer).map(DailyReports::Present)
}

/// Why bytes are not an envelope of any known schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvelopeError {
    /// Not the member set and types of any schema this build knows.
    Malformed(String),
    /// A `record.record_schema` this build does not know.
    UnsupportedSchema(i64),
    /// `daily_reports` is present exactly in schema 2.
    SchemaShape(i64),
}

impl fmt::Display for EnvelopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(detail) => write!(f, "not a record envelope: {detail}"),
            Self::UnsupportedSchema(schema) => {
                write!(f, "unsupported record schema {schema}")
            }
            Self::SchemaShape(schema) => write!(
                f,
                "record schema {schema} and the daily_reports member disagree"
            ),
        }
    }
}

impl std::error::Error for EnvelopeError {}

/// Reads bytes as an envelope of the schema they declare, refusing any
/// document that is not exactly a schema-1 or schema-2 record.
pub fn parse(bytes: &[u8]) -> Result<Envelope, EnvelopeError> {
    let envelope: Envelope =
        serde_json::from_slice(bytes).map_err(|err| EnvelopeError::Malformed(err.to_string()))?;
    let schema = envelope.record.record_schema;
    if !(1..=canonical::RECORD_SCHEMA).contains(&schema) {
        return Err(EnvelopeError::UnsupportedSchema(schema));
    }
    let has_daily_reports = matches!(envelope.daily_reports, DailyReports::Present(_));
    if has_daily_reports != (schema == 2) {
        return Err(EnvelopeError::SchemaShape(schema));
    }
    Ok(envelope)
}
