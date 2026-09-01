//! Trainee packets (ADR 0015; docs/formats/trainee-packet.md; #48):
//! everything retained about one enrollment, as one archive.
//!
//! A packet is a record export's units — every retained version of every
//! record of the enrollment, byte for byte as `record_export` writes
//! them — plus typed documents for what the units do not carry: the
//! enrollment's lifecycle and phase history, every acknowledgment, every
//! amendment, and the full task signoff history. One packet manifest
//! names all of it with hashes. Production follows the read rules that
//! exist (the trainee on their own enrollment, whoever may read the
//! training history, `export_records`); `export_verify` checks a packet
//! from its bytes alone.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use time::OffsetDateTime;

use crate::audit::{self, EventKind, Subject};
use crate::canonical;
use crate::capabilities::{self, Capability};
use crate::lifecycle::{self, EnrollmentStatus};
use crate::record_envelope::nullable;
use crate::record_export::{
    self, ARCHIVE_MANIFEST_PATH, ArchiveWriter, Scope, UnitEntry, canonical_json,
};
use crate::storage;

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

/// A program version an enrollment event names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionRef {
    pub version_number: i64,
    pub label: String,
}

/// One enrollment lifecycle event, presented as of export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentEventDoc {
    pub kind: String,
    pub occurred_at: i64,
    #[serde(deserialize_with = "nullable")]
    pub actor_display_name: Option<String>,
    pub reason: String,
    #[serde(deserialize_with = "nullable")]
    pub from_version: Option<VersionRef>,
    #[serde(deserialize_with = "nullable")]
    pub to_version: Option<VersionRef>,
}

/// One phase history event, presented as of export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhaseEventDoc {
    pub kind: String,
    pub effective_at: i64,
    pub recorded_at: i64,
    #[serde(deserialize_with = "nullable")]
    pub actor_display_name: Option<String>,
    pub reason: String,
    #[serde(deserialize_with = "nullable")]
    pub from_phase: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub to_phase: Option<String>,
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
    pub kind: String,
    pub response: String,
    pub user_display_name: String,
    pub recorded_by_display_name: String,
    pub recorded_at: i64,
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
    pub opened_by_display_name: String,
    pub opened_at: i64,
}

/// One task signoff row, with the task text it signed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignoffDoc {
    pub task_id: i64,
    pub competency_category: String,
    pub competency_name: String,
    pub prompt: String,
    pub kind: String,
    pub reason: String,
    pub signed_by_display_name: String,
    pub signed_at: i64,
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
/// pure function of the enrollment's rows and this instant.
pub async fn export_at(
    pool: &SqlitePool,
    actor_user_id: i64,
    enrollment_id: i64,
    exported_at: i64,
) -> Result<std::result::Result<Packet, PacketRefusal>> {
    let Some(enrollment) = load_enrollment(pool, enrollment_id).await? else {
        return Ok(Err(PacketRefusal::NoSuchEnrollment));
    };
    if !may_pack(pool, actor_user_id, enrollment_id, enrollment.trainee.id).await? {
        return Ok(Err(PacketRefusal::CapabilityRequired));
    }
    let installation_id = storage::installation_id(pool).await?;
    let rows = record_export::collect(pool, Scope::Enrollment { enrollment_id }).await?;
    let units = record_export::unit_entries(&rows);
    let unit_count = rows.len();

    let documents = [
        (
            DocumentKind::Acknowledgments,
            canonical_json(&acknowledgments(pool, enrollment_id).await?)?,
        ),
        (
            DocumentKind::Amendments,
            canonical_json(&amendments(pool, enrollment_id).await?)?,
        ),
        (
            DocumentKind::Enrollment,
            canonical_json(&enrollment_document(pool, enrollment_id).await?)?,
        ),
        (
            DocumentKind::Signoffs,
            canonical_json(&signoffs(pool, enrollment_id).await?)?,
        ),
    ];
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
/// (ADR 0010); none is invented here.
async fn may_pack(
    pool: &SqlitePool,
    actor_user_id: i64,
    enrollment_id: i64,
    trainee_user_id: i64,
) -> Result<bool> {
    if lifecycle::may_read(pool, actor_user_id, enrollment_id).await? {
        return Ok(true);
    }
    if actor_user_id == trainee_user_id
        && capabilities::user_has(pool, actor_user_id, Capability::ViewOwnRecords).await?
    {
        return Ok(true);
    }
    capabilities::user_has(pool, actor_user_id, Capability::ExportRecords).await
}

async fn load_enrollment(
    pool: &SqlitePool,
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
    .fetch_optional(pool)
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

async fn enrollment_document(pool: &SqlitePool, enrollment_id: i64) -> Result<EnrollmentDocument> {
    let enrolled_at: i64 = sqlx::query_scalar("SELECT enrolled_at FROM enrollment WHERE id = ?1")
        .bind(enrollment_id)
        .fetch_one(pool)
        .await
        .context("reading enrollment")?;
    let mut conn = pool.acquire().await.context("acquiring connection")?;
    let events = lifecycle::list_events(&mut conn, enrollment_id)
        .await?
        .into_iter()
        .map(|event| EnrollmentEventDoc {
            kind: event.kind,
            occurred_at: event.occurred_at,
            actor_display_name: event.actor_display_name,
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
        .collect();
    let phase_events = lifecycle::list_phase_events(&mut conn, enrollment_id)
        .await?
        .into_iter()
        .map(|event| PhaseEventDoc {
            kind: event.kind,
            effective_at: event.effective_at,
            recorded_at: event.recorded_at,
            actor_display_name: event.actor_display_name,
            reason: event.reason,
            from_phase: event.from_phase_name,
            to_phase: event.to_phase_name,
        })
        .collect();
    Ok(EnrollmentDocument {
        enrollment_id,
        enrolled_at,
        events,
        phase_events,
    })
}

async fn acknowledgments(pool: &SqlitePool, enrollment_id: i64) -> Result<Vec<AcknowledgmentDoc>> {
    let rows = sqlx::query(
        "SELECT v.evaluation_record_id AS record_id, v.version_number, a.kind, a.response,
                a.user_display_name, a.recorded_by_display_name, a.recorded_at
         FROM acknowledgment a
         JOIN evaluation_version v ON v.id = a.evaluation_version_id
         JOIN evaluation_record r ON r.id = v.evaluation_record_id
         WHERE r.enrollment_id = ?1
         ORDER BY v.evaluation_record_id, v.version_number",
    )
    .bind(enrollment_id)
    .fetch_all(pool)
    .await
    .context("reading acknowledgments")?;
    Ok(rows
        .iter()
        .map(|row| AcknowledgmentDoc {
            record_id: row.get("record_id"),
            version_number: row.get("version_number"),
            kind: row.get("kind"),
            response: row.get("response"),
            user_display_name: row.get("user_display_name"),
            recorded_by_display_name: row.get("recorded_by_display_name"),
            recorded_at: row.get("recorded_at"),
        })
        .collect())
}

async fn amendments(pool: &SqlitePool, enrollment_id: i64) -> Result<Vec<AmendmentDoc>> {
    let rows = sqlx::query(
        "SELECT am.evaluation_record_id AS record_id,
                p.version_number AS predecessor_version_number,
                s.version_number AS successor_version_number,
                am.reason, am.opened_by_display_name, am.opened_at
         FROM amendment am
         JOIN evaluation_version p ON p.id = am.predecessor_version_id
         LEFT JOIN evaluation_version s ON s.predecessor_id = am.predecessor_version_id
         JOIN evaluation_record r ON r.id = am.evaluation_record_id
         WHERE r.enrollment_id = ?1
         ORDER BY am.evaluation_record_id, p.version_number",
    )
    .bind(enrollment_id)
    .fetch_all(pool)
    .await
    .context("reading amendments")?;
    Ok(rows
        .iter()
        .map(|row| AmendmentDoc {
            record_id: row.get("record_id"),
            predecessor_version_number: row.get("predecessor_version_number"),
            successor_version_number: row.get("successor_version_number"),
            reason: row.get("reason"),
            opened_by_display_name: row.get("opened_by_display_name"),
            opened_at: row.get("opened_at"),
        })
        .collect())
}

async fn signoffs(pool: &SqlitePool, enrollment_id: i64) -> Result<Vec<SignoffDoc>> {
    let rows = sqlx::query(
        "SELECT s.task_id, c.category, c.name, t.prompt, s.kind, s.reason,
                s.signed_by_display_name, s.signed_at
         FROM task_signoff s
         JOIN task t ON t.id = s.task_id
         JOIN competency c ON c.id = t.competency_id
         WHERE s.enrollment_id = ?1
         ORDER BY s.id",
    )
    .bind(enrollment_id)
    .fetch_all(pool)
    .await
    .context("reading signoffs")?;
    Ok(rows
        .iter()
        .map(|row| SignoffDoc {
            task_id: row.get("task_id"),
            competency_category: row.get("category"),
            competency_name: row.get("name"),
            prompt: row.get("prompt"),
            kind: row.get("kind"),
            reason: row.get("reason"),
            signed_by_display_name: row.get("signed_by_display_name"),
            signed_at: row.get("signed_at"),
        })
        .collect())
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
