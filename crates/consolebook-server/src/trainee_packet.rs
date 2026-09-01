//! Trainee packets (ADR 0015; docs/formats/trainee-packet.md; #48):
//! everything retained about one enrollment, as one archive.
//!
//! A packet is a record export's units — every retained version of every
//! record of the enrollment, byte for byte as `record_export` writes
//! them — plus typed documents for what the units do not carry: the
//! enrollment's lifecycle and phase history, every acknowledgment, every
//! amendment, and the full task signoff history. One packet manifest
//! names all of it with hashes. Every component is read from one
//! database snapshot, so a packet never mixes states; every stored
//! discriminator is parsed into its closed set, so a document's `kind`
//! is a vocabulary, not a string. Production follows the read rules
//! that exist (the trainee on their own enrollment, whoever may read the
//! training history, `export_records`); `export_verify` checks a packet
//! from its bytes alone.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Row, SqliteConnection, SqlitePool};
use time::OffsetDateTime;

use crate::acknowledgments::AckKind;
use crate::audit::{self, EventKind, Subject};
use crate::canonical;
use crate::capabilities::{self, Capability};
use crate::lifecycle::{self, EnrollmentEventKind, EnrollmentStatus, PhaseEventKind};
use crate::record_envelope::nullable;
use crate::record_export::{
    self, ARCHIVE_MANIFEST_PATH, ArchiveWriter, Scope, UnitEntry, canonical_json,
};
use crate::storage;
use crate::task_signoffs::SignoffKind;

/// Packet-manifest discriminator; never changes.
pub const PACKET_FORMAT: &str = "consolebook-trainee-packet";
/// Bumped by any change to the manifest or a document's shape, and by
/// any new document kind (rendered PDFs arrive this way).
pub const PACKET_FORMAT_VERSION: i64 = 1;
/// The directory holding the packet's documents.
pub const DOCUMENT_DIR: &str = "packet";

/// The closed set of documents a version-1 packet carries, each exactly
/// once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentKind {
    Acknowledgments,
    Amendments,
    Enrollment,
    Signoffs,
}

impl DocumentKind {
    pub const ALL: [Self; 4] = [
        Self::Acknowledgments,
        Self::Amendments,
        Self::Enrollment,
        Self::Signoffs,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Acknowledgments => "acknowledgments",
            Self::Amendments => "amendments",
            Self::Enrollment => "enrollment",
            Self::Signoffs => "signoffs",
        }
    }

    /// The document's entry name: `packet/{kind}.json`.
    #[must_use]
    pub fn path(self) -> String {
        format!("{DOCUMENT_DIR}/{}.json", self.as_str())
    }
}

/// One document as the packet manifest lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentEntry {
    pub kind: DocumentKind,
    pub path: String,
    /// SHA-256 of the document's bytes, lowercase hex.
    pub sha256: String,
}

/// The trainee as presented at export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PacketTrainee {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub employee_id: String,
    pub title: String,
}

/// The pinned program version as presented at export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PacketProgram {
    pub name: String,
    pub version_number: i64,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PacketEnrollment {
    pub id: i64,
    pub program: PacketProgram,
    pub trainee: PacketTrainee,
}

impl PacketEnrollment {
    /// The shape the format mandates of the manifest's enrollment
    /// member, mirroring the `enrollment`, `user`, and `program_version`
    /// tables: positive identities, a username and a display name, a
    /// program name, and a version number of at least 1.
    #[must_use]
    pub fn shape_error(&self) -> Option<String> {
        if self.id <= 0 {
            return Some("the enrollment id is not positive".to_owned());
        }
        if self.trainee.id <= 0 {
            return Some("the trainee id is not positive".to_owned());
        }
        if self.trainee.username.is_empty() {
            return Some("the trainee has an empty username".to_owned());
        }
        if self.trainee.display_name.is_empty() {
            return Some("the trainee has an empty display name".to_owned());
        }
        if self.program.name.is_empty() {
            return Some("the program has an empty name".to_owned());
        }
        if self.program.version_number < 1 {
            return Some("the program version number is below 1".to_owned());
        }
        None
    }
}

/// The packet manifest (`manifest.json` at the root).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PacketManifest {
    pub format: String,
    pub format_version: i64,
    pub installation_id: String,
    pub exported_at: i64,
    pub enrollment: PacketEnrollment,
    pub units: Vec<UnitEntry>,
    pub documents: Vec<DocumentEntry>,
}

// ------------------------------------------------------------ documents
//
// Every `kind` below is the closed set its table's migration constrains,
// reused from the module that owns it: a reader parses the vocabulary,
// never a string, and a value outside the set is a finding, not a row.

/// A program version an enrollment event names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionRef {
    pub version_number: i64,
    pub label: String,
}

/// A person as a document names them: the stable identity beside the
/// name shown (`docs/records-integrity.md`: stable ids preserve
/// identity, snapshots preserve what the record said). The name is the
/// stored snapshot where the act stored one — acknowledgments,
/// amendments, signoffs — and the export-time name for enrollment and
/// phase events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Actor {
    pub id: i64,
    pub display_name: String,
}

impl Actor {
    /// Every stored name snapshot and every user's name is constrained
    /// non-empty, so an empty name cannot have come from the tables.
    #[must_use]
    pub fn is_named(&self) -> bool {
        !self.display_name.is_empty()
    }
}

/// One enrollment lifecycle event, presented as of export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentEventDoc {
    /// The installation's row identity: recorded order is ascending
    /// `event_id`.
    pub event_id: i64,
    pub kind: EnrollmentEventKind,
    pub occurred_at: i64,
    #[serde(deserialize_with = "nullable")]
    pub actor: Option<Actor>,
    pub reason: String,
    #[serde(deserialize_with = "nullable")]
    pub from_version: Option<VersionRef>,
    #[serde(deserialize_with = "nullable")]
    pub to_version: Option<VersionRef>,
}

impl EnrollmentEventDoc {
    /// The shape the format mandates beyond member types, mirroring
    /// `enrollment_event`'s constraints: a version change names the
    /// version left and a different version reached and gives a reason;
    /// no other kind names a version; a named actor has a name.
    #[must_use]
    pub fn shape_error(&self) -> Option<String> {
        let (id, kind) = (self.event_id, self.kind.as_str());
        if self.actor.as_ref().is_some_and(|actor| !actor.is_named()) {
            return Some(format!("event {id} names its actor with an empty name"));
        }
        match (self.kind, &self.from_version, &self.to_version) {
            (EnrollmentEventKind::VersionChange, Some(from), Some(to)) => {
                if from.version_number < 1 || to.version_number < 1 {
                    Some(format!(
                        "event {id} ({kind}) names a program version below 1"
                    ))
                } else if from.version_number == to.version_number {
                    Some(format!("event {id} ({kind}) reaches the version it left"))
                } else if self.reason.is_empty() {
                    Some(format!("event {id} ({kind}) gives no reason"))
                } else {
                    None
                }
            }
            (EnrollmentEventKind::VersionChange, _, _) => {
                Some(format!("event {id} ({kind}) lacks its version references"))
            }
            (_, None, None) => None,
            _ => Some(format!("event {id} ({kind}) carries version references")),
        }
    }
}

/// One phase history event, presented as of export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhaseEventDoc {
    /// The installation's row identity: effective order is ascending
    /// (`effective_at`, `event_id`).
    pub event_id: i64,
    pub kind: PhaseEventKind,
    pub effective_at: i64,
    pub recorded_at: i64,
    #[serde(deserialize_with = "nullable")]
    pub actor: Option<Actor>,
    pub reason: String,
    #[serde(deserialize_with = "nullable")]
    pub from_phase: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub to_phase: Option<String>,
}

impl PhaseEventDoc {
    /// The shape the format mandates beyond member types, mirroring
    /// `phase_event`'s constraints: an advance names its target, a
    /// return or restart names both phases, and a pause, resume, or
    /// completion names only the phase it happened in; nothing is
    /// effective after it was recorded; a named actor has a name.
    #[must_use]
    pub fn shape_error(&self) -> Option<String> {
        let (id, kind) = (self.event_id, self.kind.as_str());
        if self.actor.as_ref().is_some_and(|actor| !actor.is_named()) {
            return Some(format!(
                "phase event {id} names its actor with an empty name"
            ));
        }
        if [&self.from_phase, &self.to_phase]
            .iter()
            .any(|phase| phase.as_deref() == Some(""))
        {
            return Some(format!("phase event {id} names an empty phase"));
        }
        let (from, to) = (self.from_phase.is_some(), self.to_phase.is_some());
        let shaped = match self.kind {
            PhaseEventKind::Advance => to,
            PhaseEventKind::Return | PhaseEventKind::Restart => from && to,
            PhaseEventKind::Pause | PhaseEventKind::Resume | PhaseEventKind::Complete => {
                from && !to
            }
        };
        if !shaped {
            return Some(format!("phase event {id} ({kind}) names the wrong phases"));
        }
        if self.effective_at > self.recorded_at {
            return Some(format!(
                "phase event {id} is effective after it was recorded"
            ));
        }
        None
    }
}

/// `packet/enrollment.json`: the enrollment's own history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentDocument {
    pub enrollment_id: i64,
    pub enrolled_at: i64,
    pub events: Vec<EnrollmentEventDoc>,
    pub phase_events: Vec<PhaseEventDoc>,
}

/// One acknowledgment, from its stored snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcknowledgmentDoc {
    pub record_id: i64,
    pub version_number: i64,
    pub kind: AckKind,
    pub response: String,
    /// The person bound to the version: always the packet's trainee.
    pub user: Actor,
    /// Who spoke: the trainee for their own kinds, never the trainee
    /// for the attested ones.
    pub recorded_by: Actor,
    pub recorded_at: i64,
}

impl AcknowledgmentDoc {
    /// The shape the format mandates beyond member types, mirroring
    /// `acknowledgment`'s constraints: both people have names; a plain
    /// acknowledgment carries no response and every other kind explains
    /// itself; the trainee's own kinds are recorded by the trainee and
    /// the attested kinds never are.
    #[must_use]
    pub fn shape_error(&self) -> Option<String> {
        let what = format!(
            "the {} acknowledgment of record {} version {}",
            self.kind.as_str(),
            self.record_id,
            self.version_number
        );
        if !self.user.is_named() {
            return Some(format!("{what} names its trainee with an empty name"));
        }
        if !self.recorded_by.is_named() {
            return Some(format!("{what} names its recorder with an empty name"));
        }
        let plain = self.kind == AckKind::Acknowledged;
        if plain && !self.response.is_empty() {
            return Some(format!("{what} carries a response"));
        }
        if !plain && self.response.trim().is_empty() {
            return Some(format!("{what} gives no response"));
        }
        let self_recorded = self.recorded_by.id == self.user.id;
        if self.kind.spoken_by_trainee() && !self_recorded {
            return Some(format!(
                "{what} is recorded by someone other than the trainee"
            ));
        }
        if !self.kind.spoken_by_trainee() && self_recorded {
            return Some(format!(
                "{what} is recorded by the trainee it attests about"
            ));
        }
        None
    }
}

/// One amendment: the version it corrected and, once sealed, the
/// successor it produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AmendmentDoc {
    pub record_id: i64,
    pub predecessor_version_number: i64,
    #[serde(deserialize_with = "nullable")]
    pub successor_version_number: Option<i64>,
    pub reason: String,
    pub opened_by: Actor,
    pub opened_at: i64,
}

impl AmendmentDoc {
    /// The shape the format mandates beyond member types, mirroring
    /// `amendment`'s constraints: an amendment explains itself and its
    /// authority has a name.
    #[must_use]
    pub fn shape_error(&self) -> Option<String> {
        let what = format!(
            "the amendment of record {} version {}",
            self.record_id, self.predecessor_version_number
        );
        if self.reason.trim().is_empty() {
            return Some(format!("{what} gives no reason"));
        }
        if !self.opened_by.is_named() {
            return Some(format!("{what} names its authority with an empty name"));
        }
        None
    }
}

/// One task signoff row, with the task text it signed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignoffDoc {
    /// The installation's row identity: recorded order is ascending
    /// `signoff_id`.
    pub signoff_id: i64,
    pub task_id: i64,
    /// The pinned program version whose task was signed: a history that
    /// spans a version change keeps each signoff's configuration
    /// provenance without the installation.
    pub program_version: VersionRef,
    pub competency_category: String,
    pub competency_name: String,
    pub prompt: String,
    pub kind: SignoffKind,
    pub reason: String,
    pub signed_by: Actor,
    pub signed_at: i64,
}

impl SignoffDoc {
    /// Whether two rows describe their task alike: the pinned version,
    /// competency, and prompt are configuration the version fixes, so
    /// every row for one task carries the same description.
    #[must_use]
    pub fn describes_task_like(&self, other: &Self) -> bool {
        self.task_id == other.task_id
            && self.program_version == other.program_version
            && self.competency_category == other.competency_category
            && self.competency_name == other.competency_name
            && self.prompt == other.prompt
    }
}

/// The shape the format mandates of the signoff history beyond member
/// types, mirroring `task_signoff`'s constraints and triggers and the
/// configuration tables it joins: every signer has a name, every task
/// prompt and competency name is non-empty, every program version is
/// at least 1, every row for one task describes it alike; in recorded
/// order, any signoff after the first for a task supersedes it and
/// records its reason, and a revocation has something to revoke.
#[must_use]
pub fn signoff_shape_errors(signoffs: &[SignoffDoc]) -> Vec<String> {
    let mut first_for_task: BTreeMap<i64, &SignoffDoc> = BTreeMap::new();
    signoffs
        .iter()
        .filter_map(|signoff| {
            let id = signoff.signoff_id;
            let earlier = first_for_task.get(&signoff.task_id).copied();
            if earlier.is_none() {
                first_for_task.insert(signoff.task_id, signoff);
            }
            if !signoff.signed_by.is_named() {
                Some(format!("signoff {id} names its signer with an empty name"))
            } else if signoff.prompt.is_empty() {
                Some(format!("signoff {id} carries an empty task prompt"))
            } else if signoff.competency_name.is_empty() {
                Some(format!("signoff {id} carries an empty competency name"))
            } else if signoff.program_version.version_number < 1 {
                Some(format!("signoff {id} names a program version below 1"))
            } else if let Some(earlier) = earlier
                && !signoff.describes_task_like(earlier)
            {
                Some(format!(
                    "signoff {id} describes task {} differently from signoff {}",
                    signoff.task_id, earlier.signoff_id
                ))
            } else if earlier.is_none() && signoff.kind == SignoffKind::Revoked {
                Some(format!(
                    "signoff {id} revokes task {} before any signoff",
                    signoff.task_id
                ))
            } else if earlier.is_some() && signoff.reason.trim().is_empty() {
                Some(format!(
                    "signoff {id} overrides task {} without a reason",
                    signoff.task_id
                ))
            } else {
                None
            }
        })
        .collect()
}

// ------------------------------------------------------------ producing

/// Typed refusals for packing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PacketRefusal {
    NoSuchEnrollment,
    CapabilityRequired,
}

/// A produced packet, ready to deliver.
#[derive(Debug)]
pub struct Packet {
    pub file_name: String,
    pub bytes: Vec<u8>,
    pub exported_at: i64,
    pub unit_count: usize,
}

/// Packs `enrollment_id` now.
pub async fn export(
    pool: &SqlitePool,
    actor_user_id: i64,
    enrollment_id: i64,
) -> Result<std::result::Result<Packet, PacketRefusal>> {
    export_at(
        pool,
        actor_user_id,
        enrollment_id,
        OffsetDateTime::now_utc().unix_timestamp(),
    )
    .await
}

/// Packs `enrollment_id` stamped with `exported_at`. The packet is a
/// pure function of the enrollment's rows and this instant: the
/// enrollment, its units, and every document are read inside one
/// transaction, so they describe one committed state — a finalization,
/// acknowledgment, or signoff landing mid-pack is wholly in or wholly
/// out, never in the manifest and missing from a document.
/// The authorization that governs the packet is evaluated inside the
/// same transaction, so permission and contents describe one committed
/// state — an assignment ended mid-pack is wholly in or wholly out too —
/// and the transaction's connection is the only one the request holds:
/// the pool is bounded, and a packet must never be the reason it runs
/// dry.
pub async fn export_at(
    pool: &SqlitePool,
    actor_user_id: i64,
    enrollment_id: i64,
    exported_at: i64,
) -> Result<std::result::Result<Packet, PacketRefusal>> {
    let mut tx = pool.begin().await.context("beginning packet read")?;
    let Some(enrollment) = load_enrollment(&mut tx, enrollment_id).await? else {
        return Ok(Err(PacketRefusal::NoSuchEnrollment));
    };
    if !may_pack(&mut tx, actor_user_id, enrollment_id, enrollment.trainee.id).await? {
        return Ok(Err(PacketRefusal::CapabilityRequired));
    }
    let installation_id = storage::installation_id(&mut *tx).await?;
    let rows = record_export::collect(&mut tx, Scope::Enrollment { enrollment_id }).await?;
    let units = record_export::unit_entries(&rows);
    let unit_count = rows.len();
    let documents = [
        (
            DocumentKind::Acknowledgments,
            canonical_json(&acknowledgments(&mut tx, enrollment_id).await?)?,
        ),
        (
            DocumentKind::Amendments,
            canonical_json(&amendments(&mut tx, enrollment_id).await?)?,
        ),
        (
            DocumentKind::Enrollment,
            canonical_json(&enrollment_document(&mut tx, enrollment_id).await?)?,
        ),
        (
            DocumentKind::Signoffs,
            canonical_json(&signoffs(&mut tx, enrollment_id).await?)?,
        ),
    ];
    tx.commit().await.context("ending packet read")?;

    let manifest = PacketManifest {
        format: PACKET_FORMAT.to_owned(),
        format_version: PACKET_FORMAT_VERSION,
        installation_id: installation_id.clone(),
        exported_at,
        enrollment: enrollment.clone(),
        units,
        documents: documents
            .iter()
            .map(|(kind, bytes)| DocumentEntry {
                kind: *kind,
                path: kind.path(),
                sha256: canonical::content_hash_hex(bytes),
            })
            .collect(),
    };
    let mut writer = ArchiveWriter::new(exported_at)?;
    writer.add(ARCHIVE_MANIFEST_PATH, &canonical_json(&manifest)?)?;
    writer.add_units(&installation_id, exported_at, rows, &manifest.units)?;
    for (kind, bytes) in &documents {
        writer.add(&kind.path(), bytes)?;
    }
    let bytes = writer.finish()?;
    audit::record_for_subject(
        pool,
        EventKind::TraineePacketExported,
        Some(actor_user_id),
        Some(enrollment.trainee.id),
        Subject::Enrollment(enrollment_id),
    )
    .await?;
    Ok(Ok(Packet {
        file_name: format!(
            "consolebook-packet-enrollment-{enrollment_id}-{}.zip",
            record_export::stamp(exported_at)?
        ),
        bytes,
        exported_at,
        unit_count,
    }))
}

/// Who may pack an enrollment: whoever may read its training history,
/// the trainee themselves on their own enrollment, or an
/// `export_records` holder. Each is an existing read contract
/// (ADR 0010); none is invented here. Evaluated on the packet's own
/// connection, inside its snapshot.
async fn may_pack(
    conn: &mut SqliteConnection,
    actor_user_id: i64,
    enrollment_id: i64,
    trainee_user_id: i64,
) -> Result<bool> {
    if lifecycle::may_read_on(&mut *conn, actor_user_id, enrollment_id).await? {
        return Ok(true);
    }
    if actor_user_id == trainee_user_id
        && capabilities::user_has_on(&mut *conn, actor_user_id, Capability::ViewOwnRecords).await?
    {
        return Ok(true);
    }
    capabilities::user_has_on(conn, actor_user_id, Capability::ExportRecords).await
}

/// A stored discriminator, parsed into the closed set its table's
/// migration constrains. The constraint makes any other value
/// corruption, and corruption is an error to surface, never a string
/// to pass along.
fn closed_kind<T: DeserializeOwned>(what: &str, stored: &str) -> Result<T> {
    serde_json::from_value(Value::String(stored.to_owned()))
        .with_context(|| format!("stored {what} kind {stored:?} is outside its closed set"))
}

async fn load_enrollment(
    conn: &mut SqliteConnection,
    enrollment_id: i64,
) -> Result<Option<PacketEnrollment>> {
    let row = sqlx::query(
        "SELECT e.id, u.id AS user_id, u.username, u.display_name, u.employee_id, u.title,
                pv.name, pv.version_number, pv.label
         FROM enrollment e
         JOIN user u ON u.id = e.user_id
         JOIN program_version pv ON pv.id = e.program_version_id
         WHERE e.id = ?1",
    )
    .bind(enrollment_id)
    .fetch_optional(&mut *conn)
    .await
    .context("reading enrollment")?;
    Ok(row.map(|row| PacketEnrollment {
        id: row.get("id"),
        program: PacketProgram {
            name: row.get("name"),
            version_number: row.get("version_number"),
            label: row.get("label"),
        },
        trainee: PacketTrainee {
            id: row.get("user_id"),
            username: row.get("username"),
            display_name: row.get("display_name"),
            employee_id: row.get("employee_id"),
            title: row.get("title"),
        },
    }))
}

async fn enrollment_document(
    conn: &mut SqliteConnection,
    enrollment_id: i64,
) -> Result<EnrollmentDocument> {
    let enrolled_at: i64 = sqlx::query_scalar("SELECT enrolled_at FROM enrollment WHERE id = ?1")
        .bind(enrollment_id)
        .fetch_one(&mut *conn)
        .await
        .context("reading enrollment")?;
    let events = lifecycle::list_events(&mut *conn, enrollment_id)
        .await?
        .into_iter()
        .map(|event| {
            Ok(EnrollmentEventDoc {
                event_id: event.id,
                kind: closed_kind("enrollment event", &event.kind)?,
                occurred_at: event.occurred_at,
                actor: event.actor_user_id.map(|id| Actor {
                    id,
                    display_name: event.actor_display_name.clone().unwrap_or_default(),
                }),
                reason: event.reason,
                from_version: event.from_version_number.map(|number| VersionRef {
                    version_number: number,
                    label: event.from_version_label.clone().unwrap_or_default(),
                }),
                to_version: event.to_version_number.map(|number| VersionRef {
                    version_number: number,
                    label: event.to_version_label.clone().unwrap_or_default(),
                }),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let phase_events = lifecycle::list_phase_events(&mut *conn, enrollment_id)
        .await?
        .into_iter()
        .map(|event| {
            Ok(PhaseEventDoc {
                event_id: event.id,
                kind: closed_kind("phase event", &event.kind)?,
                effective_at: event.effective_at,
                recorded_at: event.recorded_at,
                actor: event.actor_user_id.map(|id| Actor {
                    id,
                    display_name: event.actor_display_name.clone().unwrap_or_default(),
                }),
                reason: event.reason,
                from_phase: event.from_phase_name,
                to_phase: event.to_phase_name,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(EnrollmentDocument {
        enrollment_id,
        enrolled_at,
        events,
        phase_events,
    })
}

async fn acknowledgments(
    conn: &mut SqliteConnection,
    enrollment_id: i64,
) -> Result<Vec<AcknowledgmentDoc>> {
    let rows = sqlx::query(
        "SELECT v.evaluation_record_id AS record_id, v.version_number, a.kind, a.response,
                a.user_id, a.user_display_name, a.recorded_by, a.recorded_by_display_name,
                a.recorded_at
         FROM acknowledgment a
         JOIN evaluation_version v ON v.id = a.evaluation_version_id
         JOIN evaluation_record r ON r.id = v.evaluation_record_id
         WHERE r.enrollment_id = ?1
         ORDER BY v.evaluation_record_id, v.version_number",
    )
    .bind(enrollment_id)
    .fetch_all(&mut *conn)
    .await
    .context("reading acknowledgments")?;
    rows.iter()
        .map(|row| {
            Ok(AcknowledgmentDoc {
                record_id: row.get("record_id"),
                version_number: row.get("version_number"),
                kind: closed_kind("acknowledgment", row.get("kind"))?,
                response: row.get("response"),
                user: Actor {
                    id: row.get("user_id"),
                    display_name: row.get("user_display_name"),
                },
                recorded_by: Actor {
                    id: row.get("recorded_by"),
                    display_name: row.get("recorded_by_display_name"),
                },
                recorded_at: row.get("recorded_at"),
            })
        })
        .collect()
}

async fn amendments(conn: &mut SqliteConnection, enrollment_id: i64) -> Result<Vec<AmendmentDoc>> {
    let rows = sqlx::query(
        "SELECT am.evaluation_record_id AS record_id,
                p.version_number AS predecessor_version_number,
                s.version_number AS successor_version_number,
                am.reason, am.opened_by, am.opened_by_display_name, am.opened_at
         FROM amendment am
         JOIN evaluation_version p ON p.id = am.predecessor_version_id
         LEFT JOIN evaluation_version s ON s.predecessor_id = am.predecessor_version_id
         JOIN evaluation_record r ON r.id = am.evaluation_record_id
         WHERE r.enrollment_id = ?1
         ORDER BY am.evaluation_record_id, p.version_number",
    )
    .bind(enrollment_id)
    .fetch_all(&mut *conn)
    .await
    .context("reading amendments")?;
    Ok(rows
        .iter()
        .map(|row| AmendmentDoc {
            record_id: row.get("record_id"),
            predecessor_version_number: row.get("predecessor_version_number"),
            successor_version_number: row.get("successor_version_number"),
            reason: row.get("reason"),
            opened_by: Actor {
                id: row.get("opened_by"),
                display_name: row.get("opened_by_display_name"),
            },
            opened_at: row.get("opened_at"),
        })
        .collect())
}

async fn signoffs(conn: &mut SqliteConnection, enrollment_id: i64) -> Result<Vec<SignoffDoc>> {
    let rows = sqlx::query(
        "SELECT s.id AS signoff_id, s.task_id, pv.version_number, pv.label,
                c.category, c.name, t.prompt, s.kind, s.reason,
                s.signed_by, s.signed_by_display_name, s.signed_at
         FROM task_signoff s
         JOIN task t ON t.id = s.task_id
         JOIN competency c ON c.id = t.competency_id
         JOIN program_version pv ON pv.id = t.program_version_id
         WHERE s.enrollment_id = ?1
         ORDER BY s.id",
    )
    .bind(enrollment_id)
    .fetch_all(&mut *conn)
    .await
    .context("reading signoffs")?;
    rows.iter()
        .map(|row| {
            Ok(SignoffDoc {
                signoff_id: row.get("signoff_id"),
                task_id: row.get("task_id"),
                program_version: VersionRef {
                    version_number: row.get("version_number"),
                    label: row.get("label"),
                },
                competency_category: row.get("category"),
                competency_name: row.get("name"),
                prompt: row.get("prompt"),
                kind: closed_kind("signoff", row.get("kind"))?,
                reason: row.get("reason"),
                signed_by: Actor {
                    id: row.get("signed_by"),
                    display_name: row.get("signed_by_display_name"),
                },
                signed_at: row.get("signed_at"),
            })
        })
        .collect()
}

// ------------------------------------------------------------ own list

/// One of the trainee's own enrollments, for the My records page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OwnEnrollment {
    pub enrollment_id: i64,
    pub program_name: String,
    pub version_number: i64,
    pub version_label: String,
    pub enrolled_at: i64,
    pub status: EnrollmentStatus,
    pub finalized_versions: i64,
}

/// The actor's own enrollments, newest first, for `view_own_records`
/// holders.
pub async fn own_enrollments(
    pool: &SqlitePool,
    actor_user_id: i64,
) -> Result<std::result::Result<Vec<OwnEnrollment>, PacketRefusal>> {
    if !capabilities::user_has(pool, actor_user_id, Capability::ViewOwnRecords).await? {
        return Ok(Err(PacketRefusal::CapabilityRequired));
    }
    let rows = sqlx::query(
        "SELECT e.id, pv.name, pv.version_number, pv.label, e.enrolled_at,
                (SELECT COUNT(*) FROM evaluation_version v
                 JOIN evaluation_record r ON r.id = v.evaluation_record_id
                 WHERE r.enrollment_id = e.id) AS finalized_versions
         FROM enrollment e
         JOIN program_version pv ON pv.id = e.program_version_id
         WHERE e.user_id = ?1
         ORDER BY e.enrolled_at DESC, e.id DESC",
    )
    .bind(actor_user_id)
    .fetch_all(pool)
    .await
    .context("listing own enrollments")?;
    let mut conn = pool.acquire().await.context("acquiring connection")?;
    let mut enrollments = Vec::with_capacity(rows.len());
    for row in &rows {
        let enrollment_id: i64 = row.get("id");
        let status = lifecycle::status(&mut conn, enrollment_id)
            .await?
            .unwrap_or(EnrollmentStatus::Active);
        enrollments.push(OwnEnrollment {
            enrollment_id,
            program_name: row.get("name"),
            version_number: row.get("version_number"),
            version_label: row.get("label"),
            enrolled_at: row.get("enrolled_at"),
            status,
            finalized_versions: row.get("finalized_versions"),
        });
    }
    Ok(Ok(enrollments))
}
