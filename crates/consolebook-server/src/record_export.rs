//! Structured record exports: the format vocabulary and the producing
//! side (ADR 0014; docs/formats/record-export.md; #45).
//!
//! An export unit is one finalized version's stored canonical bytes,
//! copied verbatim, beside a canonical-JSON unit manifest; an archive
//! is a ZIP container with stored entries in a fixed order holding an
//! archive manifest and its units. The archive is a pure function of
//! the scope's stored rows and the export instant, so the same scope
//! exported at the same instant is byte-identical. Export follows the
//! read rules that already exist — a unit contains exactly what its
//! reader may already read — and the installation scope takes
//! `export_records`. Verification from the archive alone is
//! `export_verify`'s, which reads the manifests defined here.

use std::fmt;
use std::io::{Cursor, Write};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use time::OffsetDateTime;
use zip::CompressionMethod;
use zip::write::{SimpleFileOptions, ZipWriter};

use crate::audit::{self, EventKind, Subject};
use crate::canonical;
use crate::capabilities::{self, Capability};
use crate::evaluation_drafts;
use crate::lifecycle;
use crate::storage;

/// Archive-manifest discriminator; never changes.
pub const ARCHIVE_FORMAT: &str = "consolebook-record-export";
/// Unit-manifest discriminator; never changes.
pub const UNIT_FORMAT: &str = "consolebook-record-unit";
/// Shared by both manifests; bumped by any change to either shape.
pub const FORMAT_VERSION: i64 = 1;
/// The archive manifest's entry name.
pub const ARCHIVE_MANIFEST_PATH: &str = "manifest.json";
/// The canonical record bytes within a unit directory.
pub const RECORD_FILE: &str = "record.json";
/// The unit manifest within a unit directory.
pub const UNIT_MANIFEST_FILE: &str = "manifest.json";

/// What an archive claims to contain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Scope {
    /// Exactly one finalized version.
    Version { record_id: i64, version_number: i64 },
    /// Every retained version of one record, superseded originals
    /// included.
    Record { record_id: i64 },
    /// Every finalized version of every record of one enrollment.
    Enrollment { enrollment_id: i64 },
    /// Every finalized version the installation holds.
    Installation,
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Version {
                record_id,
                version_number,
            } => write!(f, "record {record_id}, version {version_number}"),
            Self::Record { record_id } => {
                write!(f, "record {record_id}, every retained version")
            }
            Self::Enrollment { enrollment_id } => write!(f, "enrollment {enrollment_id}"),
            Self::Installation => f.write_str("the whole installation"),
        }
    }
}

/// Typed refusals for the export act.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportRefusal {
    NoSuchRecord,
    NoSuchVersion,
    NoSuchEnrollment,
    CapabilityRequired,
    /// The scope exists but holds no finalized version; an empty
    /// archive is never presented as a complete export.
    NothingToExport,
}

/// One unit as the archive manifest lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitEntry {
    pub path: String,
    pub record_id: i64,
    pub version_number: i64,
    pub record_schema: i64,
    pub content_hash: String,
    pub chain_hash: String,
    pub predecessor_content_hash: Option<String>,
}

/// The archive manifest (`manifest.json` at the root).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveManifest {
    pub format: String,
    pub format_version: i64,
    pub installation_id: String,
    pub exported_at: i64,
    pub scope: Scope,
    pub units: Vec<UnitEntry>,
}

/// The unit manifest beside each unit's record bytes. It repeats what
/// the archive manifest says so a unit directory stands on its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitManifest {
    pub format: String,
    pub format_version: i64,
    pub installation_id: String,
    pub exported_at: i64,
    pub record_id: i64,
    pub version_number: i64,
    pub record_schema: i64,
    pub content_hash: String,
    pub chain_hash: String,
    pub predecessor_content_hash: Option<String>,
}

/// A produced archive, ready to deliver.
#[derive(Debug)]
pub struct Export {
    /// The documented download name, `consolebook-<scope>-<stamp>.zip`.
    pub file_name: String,
    pub bytes: Vec<u8>,
    pub exported_at: i64,
    pub unit_count: usize,
}

/// The unit directory for one version: `records/{record_id}/v{n}`.
#[must_use]
pub fn unit_path(record_id: i64, version_number: i64) -> String {
    format!("records/{record_id}/v{version_number}")
}

// ------------------------------------------------------------ producing

/// Exports `scope` now, for an actor the scope's read rule admits.
pub async fn export(
    pool: &SqlitePool,
    actor_user_id: i64,
    scope: Scope,
) -> Result<std::result::Result<Export, ExportRefusal>> {
    export_at(
        pool,
        actor_user_id,
        scope,
        OffsetDateTime::now_utc().unix_timestamp(),
    )
    .await
}

/// Exports `scope` stamped with `exported_at` (UTC unix seconds). The
/// archive is a pure function of the scope's rows and this instant.
pub async fn export_at(
    pool: &SqlitePool,
    actor_user_id: i64,
    scope: Scope,
    exported_at: i64,
) -> Result<std::result::Result<Export, ExportRefusal>> {
    let audited = match authorize(pool, actor_user_id, scope).await? {
        Ok(audited) => audited,
        Err(refusal) => return Ok(Err(refusal)),
    };
    let rows = collect(pool, scope).await?;
    if rows.is_empty() {
        return Ok(Err(match scope {
            Scope::Version { .. } => ExportRefusal::NoSuchVersion,
            Scope::Record { .. } | Scope::Enrollment { .. } | Scope::Installation => {
                ExportRefusal::NothingToExport
            }
        }));
    }
    let installation_id = storage::installation_id(pool).await?;
    let bytes = build_archive(&installation_id, exported_at, scope, &rows)?;
    // The export is audited once it exists: actor and subject, never
    // content (docs/records-integrity.md).
    match audited.subject {
        Some(subject) => {
            audit::record_for_subject(
                pool,
                EventKind::RecordExported,
                Some(actor_user_id),
                audited.trainee,
                subject,
            )
            .await?;
        }
        None => audit::record(pool, EventKind::RecordExported, Some(actor_user_id), None).await?,
    }
    Ok(Ok(Export {
        file_name: file_name(scope, exported_at)?,
        bytes,
        exported_at,
        unit_count: rows.len(),
    }))
}

/// Counts for the installation-export interface, for `export_records`
/// holders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportSummary {
    pub installation_id: String,
    pub record_count: i64,
    pub version_count: i64,
}

/// How many finalized records and versions an installation export
/// would carry.
pub async fn summary(
    pool: &SqlitePool,
    actor_user_id: i64,
) -> Result<std::result::Result<ExportSummary, ExportRefusal>> {
    if !capabilities::user_has(pool, actor_user_id, Capability::ExportRecords).await? {
        return Ok(Err(ExportRefusal::CapabilityRequired));
    }
    let (record_count, version_count): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(DISTINCT evaluation_record_id), COUNT(*) FROM evaluation_version",
    )
    .fetch_one(pool)
    .await
    .context("counting finalized versions")?;
    Ok(Ok(ExportSummary {
        installation_id: storage::installation_id(pool).await?,
        record_count,
        version_count,
    }))
}

/// What the audit event names once the export exists.
struct Audited {
    subject: Option<Subject>,
    trainee: Option<i64>,
}

/// The scope's read rule, as the typed contract it already is elsewhere
/// (ADR 0010): the record read rule for a version or record, the
/// training-history read rule for an enrollment, and the explicit
/// `export_records` authority for the installation.
async fn authorize(
    pool: &SqlitePool,
    actor_user_id: i64,
    scope: Scope,
) -> Result<std::result::Result<Audited, ExportRefusal>> {
    match scope {
        Scope::Version { record_id, .. } | Scope::Record { record_id } => {
            let mut conn = pool.acquire().await.context("acquiring connection")?;
            let Some(record) = evaluation_drafts::load_record(&mut conn, record_id).await? else {
                return Ok(Err(ExportRefusal::NoSuchRecord));
            };
            drop(conn);
            if !crate::draft_access::may_read(pool, actor_user_id, &record).await? {
                return Ok(Err(ExportRefusal::CapabilityRequired));
            }
            let trainee: i64 = sqlx::query_scalar("SELECT user_id FROM enrollment WHERE id = ?1")
                .bind(record.enrollment_id)
                .fetch_one(pool)
                .await
                .context("reading enrollment")?;
            Ok(Ok(Audited {
                subject: Some(Subject::Record(record_id)),
                trainee: Some(trainee),
            }))
        }
        Scope::Enrollment { enrollment_id } => {
            let trainee: Option<i64> =
                sqlx::query_scalar("SELECT user_id FROM enrollment WHERE id = ?1")
                    .bind(enrollment_id)
                    .fetch_optional(pool)
                    .await
                    .context("reading enrollment")?;
            let Some(trainee) = trainee else {
                return Ok(Err(ExportRefusal::NoSuchEnrollment));
            };
            if !lifecycle::may_read(pool, actor_user_id, enrollment_id).await? {
                return Ok(Err(ExportRefusal::CapabilityRequired));
            }
            Ok(Ok(Audited {
                subject: Some(Subject::Enrollment(enrollment_id)),
                trainee: Some(trainee),
            }))
        }
        Scope::Installation => {
            if !capabilities::user_has(pool, actor_user_id, Capability::ExportRecords).await? {
                return Ok(Err(ExportRefusal::CapabilityRequired));
            }
            Ok(Ok(Audited {
                subject: None,
                trainee: None,
            }))
        }
    }
}

/// One stored version, exactly as the archive carries it.
struct VersionRow {
    record_id: i64,
    version_number: i64,
    record_schema: i64,
    bytes: Vec<u8>,
    content_hash: String,
    chain_hash: String,
    predecessor_content_hash: Option<String>,
}

/// The stored rows of a scope in archive order: ascending record id,
/// then version number. The predecessor's content hash is read from
/// its own row, so the manifest states what the chain hash was
/// computed over (ADR 0011).
macro_rules! unit_query {
    ($where:literal) => {
        concat!(
            "SELECT v.evaluation_record_id AS record_id, v.version_number,
                    v.record_schema, v.canonical_bytes, v.content_hash,
                    v.chain_hash, p.content_hash AS predecessor_content_hash
             FROM evaluation_version v
             LEFT JOIN evaluation_version p ON p.id = v.predecessor_id
             JOIN evaluation_record r ON r.id = v.evaluation_record_id ",
            $where,
            " ORDER BY v.evaluation_record_id, v.version_number"
        )
    };
}

async fn collect(pool: &SqlitePool, scope: Scope) -> Result<Vec<VersionRow>> {
    let rows = match scope {
        Scope::Version {
            record_id,
            version_number,
        } => {
            sqlx::query(unit_query!(
                "WHERE v.evaluation_record_id = ?1 AND v.version_number = ?2"
            ))
            .bind(record_id)
            .bind(version_number)
            .fetch_all(pool)
            .await
        }
        Scope::Record { record_id } => {
            sqlx::query(unit_query!("WHERE v.evaluation_record_id = ?1"))
                .bind(record_id)
                .fetch_all(pool)
                .await
        }
        Scope::Enrollment { enrollment_id } => {
            sqlx::query(unit_query!("WHERE r.enrollment_id = ?1"))
                .bind(enrollment_id)
                .fetch_all(pool)
                .await
        }
        Scope::Installation => sqlx::query(unit_query!("")).fetch_all(pool).await,
    }
    .context("reading finalized versions")?;
    Ok(rows
        .iter()
        .map(|row| VersionRow {
            record_id: row.get("record_id"),
            version_number: row.get("version_number"),
            record_schema: row.get("record_schema"),
            bytes: row.get("canonical_bytes"),
            content_hash: row.get("content_hash"),
            chain_hash: row.get("chain_hash"),
            predecessor_content_hash: row.get("predecessor_content_hash"),
        })
        .collect())
}

/// Writes the container exactly as docs/formats/record-export.md lays
/// it out: manifest first, then units in order, stored entries, the
/// export instant as every entry's modification time, `0644`.
fn build_archive(
    installation_id: &str,
    exported_at: i64,
    scope: Scope,
    rows: &[VersionRow],
) -> Result<Vec<u8>> {
    let units: Vec<UnitEntry> = rows
        .iter()
        .map(|row| UnitEntry {
            path: unit_path(row.record_id, row.version_number),
            record_id: row.record_id,
            version_number: row.version_number,
            record_schema: row.record_schema,
            content_hash: row.content_hash.clone(),
            chain_hash: row.chain_hash.clone(),
            predecessor_content_hash: row.predecessor_content_hash.clone(),
        })
        .collect();
    let manifest = ArchiveManifest {
        format: ARCHIVE_FORMAT.to_owned(),
        format_version: FORMAT_VERSION,
        installation_id: installation_id.to_owned(),
        exported_at,
        scope,
        units,
    };
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(dos_time(exported_at)?)
        .unix_permissions(0o644);
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    add_entry(
        &mut writer,
        ARCHIVE_MANIFEST_PATH,
        &canonical_json(&manifest)?,
        options,
    )?;
    for (row, entry) in rows.iter().zip(&manifest.units) {
        add_entry(
            &mut writer,
            &format!("{}/{RECORD_FILE}", entry.path),
            &row.bytes,
            options,
        )?;
        let unit = UnitManifest {
            format: UNIT_FORMAT.to_owned(),
            format_version: FORMAT_VERSION,
            installation_id: installation_id.to_owned(),
            exported_at,
            record_id: entry.record_id,
            version_number: entry.version_number,
            record_schema: entry.record_schema,
            content_hash: entry.content_hash.clone(),
            chain_hash: entry.chain_hash.clone(),
            predecessor_content_hash: entry.predecessor_content_hash.clone(),
        };
        add_entry(
            &mut writer,
            &format!("{}/{UNIT_MANIFEST_FILE}", entry.path),
            &canonical_json(&unit)?,
            options,
        )?;
    }
    let cursor = writer.finish().context("finishing the export archive")?;
    Ok(cursor.into_inner())
}

fn add_entry(
    writer: &mut ZipWriter<Cursor<Vec<u8>>>,
    name: &str,
    bytes: &[u8],
    options: SimpleFileOptions,
) -> Result<()> {
    writer
        .start_file(name, options)
        .with_context(|| format!("starting archive entry {name}"))?;
    writer
        .write_all(bytes)
        .with_context(|| format!("writing archive entry {name}"))?;
    Ok(())
}

/// Manifests are canonical JSON under the record format's subset, so
/// the archive is deterministic and a manifest is itself checkable.
pub(crate) fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let value = serde_json::to_value(value).context("serializing manifest")?;
    canonical::canonical_bytes(&value)
}

fn dos_time(exported_at: i64) -> Result<zip::DateTime> {
    let at = OffsetDateTime::from_unix_timestamp(exported_at).context("export instant")?;
    zip::DateTime::from_date_and_time(
        u16::try_from(at.year()).context("export year")?,
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second(),
    )
    .map_err(|_| anyhow!("export instant {exported_at} is outside the ZIP date range"))
}

fn file_name(scope: Scope, exported_at: i64) -> Result<String> {
    let stamp = OffsetDateTime::from_unix_timestamp(exported_at)
        .context("export instant")?
        .format(&time::macros::format_description!(
            "[year][month][day]T[hour][minute][second]Z"
        ))
        .context("formatting export instant")?;
    let scope_part = match scope {
        Scope::Version {
            record_id,
            version_number,
        } => format!("record-{record_id}-v{version_number}"),
        Scope::Record { record_id } => format!("record-{record_id}"),
        Scope::Enrollment { enrollment_id } => format!("enrollment-{enrollment_id}"),
        Scope::Installation => "installation".to_owned(),
    };
    Ok(format!("consolebook-{scope_part}-{stamp}.zip"))
}
