//! Structured record exports (ADR 0014; docs/formats/record-export.md;
//! #45).
//!
//! An export unit is one finalized version's stored canonical bytes,
//! copied verbatim, beside a canonical-JSON unit manifest; an archive
//! is a ZIP container with stored entries in a fixed order holding an
//! archive manifest and its units. The archive is a pure function of
//! the scope's stored rows and the export instant, so the same scope
//! exported at the same instant is byte-identical. Verification reads
//! only the archive and reports typed findings; its verdict is
//! consistency with the stated fingerprints, never tamper-proofing
//! (ADR 0010, ADR 0011). Export follows the read rules that already
//! exist — a unit contains exactly what its reader may already read —
//! and the installation scope takes `export_records`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{Cursor, Read, Write};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Row, SqlitePool};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
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
fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
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

// ------------------------------------------------------------ verifying

/// One thing a verifier found wrong. The verdict derives from the
/// absence of findings; wording is presentation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Finding {
    NotAnArchive {
        detail: String,
    },
    ArchiveManifestMissing,
    ArchiveManifestUnreadable {
        detail: String,
    },
    UnsupportedFormat {
        format: String,
        format_version: i64,
    },
    /// A manifest's bytes are not the canonical serialization of what
    /// they parse to: a member is missing, reordered, or reformatted.
    ManifestNotCanonical {
        path: String,
    },
    /// Units are not strictly ascending by (record, version).
    UnitsOutOfOrder,
    /// The manifest lists no unit; the format refuses empty exports.
    NoUnits,
    /// The declared scope calls for a different number of units.
    ScopeCardinality {
        expected: usize,
        listed: usize,
    },
    /// A listed unit's identity contradicts the declared scope.
    UnitOutsideScope {
        path: String,
    },
    /// The container's central directory could not be walked.
    CentralDirectoryUnreadable {
        detail: String,
    },
    /// The central directory names one entry more than once; extraction
    /// tools disagree on which copy they take.
    DuplicateEntry {
        path: String,
    },
    UnitPathUnexpected {
        path: String,
        expected: String,
    },
    /// The container holds an entry the manifest does not name.
    UnlistedEntry {
        path: String,
    },
    MissingEntry {
        path: String,
    },
    EntryUnreadable {
        path: String,
        detail: String,
    },
    UnitManifestUnreadable {
        detail: String,
    },
    /// The unit manifest and the archive manifest disagree on a member.
    UnitManifestDisagrees {
        member: &'static str,
    },
    ContentHashMismatch,
    /// `record.json` is not canonical bytes (or not JSON at all).
    NotCanonical {
        detail: String,
    },
    /// The envelope's own identity members disagree with the manifest.
    EnvelopeDisagrees {
        member: &'static str,
    },
    ChainHashMismatch,
    /// A first version with a predecessor, or a later one without.
    LineageShape,
    /// The predecessor is in the archive and its content hash is not
    /// what this unit's chain was computed over.
    PredecessorMismatch,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAnArchive { detail } => write!(f, "not a readable ZIP archive: {detail}"),
            Self::ArchiveManifestMissing => f.write_str("the archive manifest is missing"),
            Self::ArchiveManifestUnreadable { detail } => {
                write!(f, "the archive manifest is unreadable: {detail}")
            }
            Self::UnsupportedFormat {
                format,
                format_version,
            } => write!(f, "unsupported format '{format}' version {format_version}"),
            Self::ManifestNotCanonical { path } => {
                write!(f, "{path} is not canonical JSON")
            }
            Self::UnitsOutOfOrder => {
                f.write_str("units are not strictly ascending by record and version")
            }
            Self::NoUnits => f.write_str("the manifest lists no unit"),
            Self::ScopeCardinality { expected, listed } => write!(
                f,
                "the declared scope calls for {expected} unit(s); the manifest lists {listed}"
            ),
            Self::UnitOutsideScope { path } => {
                write!(f, "unit {path} is outside the declared scope")
            }
            Self::CentralDirectoryUnreadable { detail } => {
                write!(f, "the central directory could not be walked: {detail}")
            }
            Self::DuplicateEntry { path } => {
                write!(
                    f,
                    "entry {path} appears more than once in the central directory"
                )
            }
            Self::UnitPathUnexpected { path, expected } => {
                write!(f, "unit path {path} should be {expected}")
            }
            Self::UnlistedEntry { path } => write!(f, "entry {path} is not listed by the manifest"),
            Self::MissingEntry { path } => write!(f, "entry {path} is missing"),
            Self::EntryUnreadable { path, detail } => {
                write!(f, "entry {path} is unreadable: {detail}")
            }
            Self::UnitManifestUnreadable { detail } => {
                write!(f, "the unit manifest is unreadable: {detail}")
            }
            Self::UnitManifestDisagrees { member } => {
                write!(
                    f,
                    "the unit manifest disagrees with the archive on {member}"
                )
            }
            Self::ContentHashMismatch => {
                f.write_str("the content hash does not match the record bytes")
            }
            Self::NotCanonical { detail } => {
                write!(f, "the record bytes are not canonical: {detail}")
            }
            Self::EnvelopeDisagrees { member } => {
                write!(f, "the record's own {member} disagrees with the manifest")
            }
            Self::ChainHashMismatch => {
                f.write_str("the chain hash does not match the predecessor hash and record bytes")
            }
            Self::LineageShape => {
                f.write_str("a predecessor hash is present exactly for versions after the first")
            }
            Self::PredecessorMismatch => {
                f.write_str("the predecessor in this archive has a different content hash")
            }
        }
    }
}

/// Whether a unit's predecessor was checked against the archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PredecessorLink {
    /// A first version.
    None,
    /// The predecessor is in the archive and its content hash matches.
    Linked,
    /// The archive does not carry the predecessor; the chain hash was
    /// still recomputed from the carried predecessor hash.
    NotInExport,
}

impl fmt::Display for PredecessorLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::None => "none (first version)",
            Self::Linked => "linked",
            Self::NotInExport => "not in export",
        })
    }
}

/// One unit's verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnitReport {
    pub path: String,
    pub record_id: i64,
    pub version_number: i64,
    pub record_schema: i64,
    pub predecessor: PredecessorLink,
    pub findings: Vec<Finding>,
}

impl UnitReport {
    #[must_use]
    pub fn verified(&self) -> bool {
        self.findings.is_empty()
    }
}

/// The whole archive's verification. `verified` when nothing was found
/// wrong anywhere: internally consistent with its stated fingerprints,
/// which is what the format can prove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArchiveReport {
    pub installation_id: Option<String>,
    pub exported_at: Option<i64>,
    pub scope: Option<Scope>,
    pub units: Vec<UnitReport>,
    pub findings: Vec<Finding>,
}

impl ArchiveReport {
    #[must_use]
    pub fn verified(&self) -> bool {
        self.findings.is_empty() && self.units.iter().all(UnitReport::verified)
    }

    /// The export instant as RFC 3339, for presentation.
    #[must_use]
    pub fn exported_at_rfc3339(&self) -> Option<String> {
        self.exported_at.and_then(|at| {
            OffsetDateTime::from_unix_timestamp(at)
                .ok()?
                .format(&Rfc3339)
                .ok()
        })
    }
}

type Archive<'a> = zip::ZipArchive<Cursor<&'a [u8]>>;

/// Verifies an archive from its bytes alone, per the normative checks
/// in docs/formats/record-export.md.
#[must_use]
pub fn verify_archive(bytes: &[u8]) -> ArchiveReport {
    let mut report = ArchiveReport {
        installation_id: None,
        exported_at: None,
        scope: None,
        units: Vec::new(),
        findings: Vec::new(),
    };
    let mut archive = match zip::ZipArchive::new(Cursor::new(bytes)) {
        Ok(archive) => archive,
        Err(err) => {
            report.findings.push(Finding::NotAnArchive {
                detail: err.to_string(),
            });
            return report;
        }
    };
    let names: Vec<String> = archive.file_names().map(str::to_owned).collect();
    report.findings.extend(duplicate_entry_findings(bytes));
    let manifest_bytes = match read_entry(&mut archive, ARCHIVE_MANIFEST_PATH) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            report.findings.push(Finding::ArchiveManifestMissing);
            return report;
        }
        Err(detail) => {
            report.findings.push(Finding::EntryUnreadable {
                path: ARCHIVE_MANIFEST_PATH.to_owned(),
                detail,
            });
            return report;
        }
    };
    let manifest: ArchiveManifest = match serde_json::from_slice(&manifest_bytes) {
        Ok(manifest) => manifest,
        Err(err) => {
            report.findings.push(Finding::ArchiveManifestUnreadable {
                detail: err.to_string(),
            });
            return report;
        }
    };
    if manifest.format != ARCHIVE_FORMAT || manifest.format_version != FORMAT_VERSION {
        report.findings.push(Finding::UnsupportedFormat {
            format: manifest.format.clone(),
            format_version: manifest.format_version,
        });
        return report;
    }
    if canonical_json(&manifest).ok().as_deref() != Some(manifest_bytes.as_slice()) {
        report.findings.push(Finding::ManifestNotCanonical {
            path: ARCHIVE_MANIFEST_PATH.to_owned(),
        });
    }
    report.installation_id = Some(manifest.installation_id.clone());
    report.exported_at = Some(manifest.exported_at);
    report.scope = Some(manifest.scope);

    let ordered = manifest.units.windows(2).all(|pair| {
        (pair[0].record_id, pair[0].version_number) < (pair[1].record_id, pair[1].version_number)
    });
    if !ordered {
        report.findings.push(Finding::UnitsOutOfOrder);
    }
    report.findings.extend(scope_findings(&manifest));
    let mut listed: BTreeSet<String> = BTreeSet::new();
    listed.insert(ARCHIVE_MANIFEST_PATH.to_owned());
    for entry in &manifest.units {
        let expected = unit_path(entry.record_id, entry.version_number);
        if entry.path != expected {
            report.findings.push(Finding::UnitPathUnexpected {
                path: entry.path.clone(),
                expected,
            });
        }
        listed.insert(format!("{}/{RECORD_FILE}", entry.path));
        listed.insert(format!("{}/{UNIT_MANIFEST_FILE}", entry.path));
    }
    for name in &names {
        if !listed.contains(name) {
            report
                .findings
                .push(Finding::UnlistedEntry { path: name.clone() });
        }
    }
    let by_identity: BTreeMap<(i64, i64), &UnitEntry> = manifest
        .units
        .iter()
        .map(|entry| ((entry.record_id, entry.version_number), entry))
        .collect();
    for entry in &manifest.units {
        report
            .units
            .push(verify_unit(&mut archive, &manifest, entry, &by_identity));
    }
    report
}

#[allow(clippy::too_many_lines)]
fn verify_unit(
    archive: &mut Archive<'_>,
    manifest: &ArchiveManifest,
    entry: &UnitEntry,
    by_identity: &BTreeMap<(i64, i64), &UnitEntry>,
) -> UnitReport {
    let mut findings = Vec::new();
    let record_path = format!("{}/{RECORD_FILE}", entry.path);
    let manifest_path = format!("{}/{UNIT_MANIFEST_FILE}", entry.path);

    match read_entry(archive, &manifest_path) {
        Ok(Some(bytes)) => match serde_json::from_slice::<UnitManifest>(&bytes) {
            Ok(unit) => {
                if unit.format != UNIT_FORMAT || unit.format_version != FORMAT_VERSION {
                    findings.push(Finding::UnsupportedFormat {
                        format: unit.format.clone(),
                        format_version: unit.format_version,
                    });
                }
                if canonical_json(&unit).ok().as_deref() != Some(bytes.as_slice()) {
                    findings.push(Finding::ManifestNotCanonical {
                        path: manifest_path.clone(),
                    });
                }
                let disagreements: [(&'static str, bool); 8] = [
                    (
                        "installation_id",
                        unit.installation_id != manifest.installation_id,
                    ),
                    ("exported_at", unit.exported_at != manifest.exported_at),
                    ("record_id", unit.record_id != entry.record_id),
                    (
                        "version_number",
                        unit.version_number != entry.version_number,
                    ),
                    ("record_schema", unit.record_schema != entry.record_schema),
                    ("content_hash", unit.content_hash != entry.content_hash),
                    ("chain_hash", unit.chain_hash != entry.chain_hash),
                    (
                        "predecessor_content_hash",
                        unit.predecessor_content_hash != entry.predecessor_content_hash,
                    ),
                ];
                for (member, disagrees) in disagreements {
                    if disagrees {
                        findings.push(Finding::UnitManifestDisagrees { member });
                    }
                }
            }
            Err(err) => findings.push(Finding::UnitManifestUnreadable {
                detail: err.to_string(),
            }),
        },
        Ok(None) => findings.push(Finding::MissingEntry {
            path: manifest_path.clone(),
        }),
        Err(detail) => findings.push(Finding::EntryUnreadable {
            path: manifest_path.clone(),
            detail,
        }),
    }

    match read_entry(archive, &record_path) {
        Ok(Some(bytes)) => {
            if canonical::content_hash_hex(&bytes) != entry.content_hash {
                findings.push(Finding::ContentHashMismatch);
            }
            match serde_json::from_slice::<Value>(&bytes) {
                Ok(envelope) => {
                    match canonical::canonical_bytes(&envelope) {
                        Ok(again) if again == bytes => {}
                        Ok(_) => findings.push(Finding::NotCanonical {
                            detail: "re-serialization differs from the stored bytes".to_owned(),
                        }),
                        Err(err) => findings.push(Finding::NotCanonical {
                            detail: err.to_string(),
                        }),
                    }
                    let predecessor = entry
                        .predecessor_content_hash
                        .as_ref()
                        .map_or(Value::Null, |hash| Value::String(hash.clone()));
                    let disagreements: [(&'static str, bool); 6] = [
                        ("record.id", envelope["record"]["id"] != entry.record_id),
                        (
                            "record.version_number",
                            envelope["record"]["version_number"] != entry.version_number,
                        ),
                        (
                            "record.record_schema",
                            envelope["record"]["record_schema"] != entry.record_schema,
                        ),
                        (
                            "record.predecessor_content_hash",
                            envelope["record"]["predecessor_content_hash"] != predecessor,
                        ),
                        (
                            "instance",
                            envelope["instance"] != manifest.installation_id.as_str(),
                        ),
                        (
                            "canonicalization",
                            envelope["canonicalization"] != canonical::CANONICALIZATION,
                        ),
                    ];
                    for (member, disagrees) in disagreements {
                        if disagrees {
                            findings.push(Finding::EnvelopeDisagrees { member });
                        }
                    }
                }
                Err(err) => findings.push(Finding::NotCanonical {
                    detail: format!("not JSON: {err}"),
                }),
            }
            match canonical::chain_hash_hex(entry.predecessor_content_hash.as_deref(), &bytes) {
                Ok(chain) if chain == entry.chain_hash => {}
                _ => findings.push(Finding::ChainHashMismatch),
            }
        }
        Ok(None) => findings.push(Finding::MissingEntry {
            path: record_path.clone(),
        }),
        Err(detail) => findings.push(Finding::EntryUnreadable {
            path: record_path.clone(),
            detail,
        }),
    }

    if (entry.version_number == 1) != entry.predecessor_content_hash.is_none() {
        findings.push(Finding::LineageShape);
    }
    let predecessor = match &entry.predecessor_content_hash {
        None => PredecessorLink::None,
        Some(hash) => match entry
            .version_number
            .checked_sub(1)
            .and_then(|number| by_identity.get(&(entry.record_id, number)))
        {
            Some(previous) => {
                if previous.content_hash != *hash {
                    findings.push(Finding::PredecessorMismatch);
                }
                PredecessorLink::Linked
            }
            None => PredecessorLink::NotInExport,
        },
    };

    UnitReport {
        path: entry.path.clone(),
        record_id: entry.record_id,
        version_number: entry.version_number,
        record_schema: entry.record_schema,
        predecessor,
        findings,
    }
}

/// The declared scope checked as far as the archive itself allows: no
/// scope is empty, a version scope is exactly its one unit, and a
/// record scope holds only that record's versions. Enrollment and
/// installation scopes state nothing the bytes can confirm.
fn scope_findings(manifest: &ArchiveManifest) -> Vec<Finding> {
    let mut findings = Vec::new();
    if manifest.units.is_empty() {
        findings.push(Finding::NoUnits);
    }
    match manifest.scope {
        Scope::Version {
            record_id,
            version_number,
        } => {
            if manifest.units.len() != 1 {
                findings.push(Finding::ScopeCardinality {
                    expected: 1,
                    listed: manifest.units.len(),
                });
            }
            for entry in &manifest.units {
                if (entry.record_id, entry.version_number) != (record_id, version_number) {
                    findings.push(Finding::UnitOutsideScope {
                        path: entry.path.clone(),
                    });
                }
            }
        }
        Scope::Record { record_id } => {
            for entry in &manifest.units {
                if entry.record_id != record_id {
                    findings.push(Finding::UnitOutsideScope {
                        path: entry.path.clone(),
                    });
                }
            }
        }
        Scope::Enrollment { .. } | Scope::Installation => {}
    }
    findings
}

/// The reader keeps one entry per name; only the central directory
/// itself says whether a name was written twice.
fn duplicate_entry_findings(bytes: &[u8]) -> Vec<Finding> {
    match central_directory_names(bytes) {
        Ok(directory) => {
            let mut occurrences: BTreeMap<&str, usize> = BTreeMap::new();
            for name in &directory {
                *occurrences.entry(name.as_str()).or_default() += 1;
            }
            occurrences
                .into_iter()
                .filter(|(_, count)| *count > 1)
                .map(|(name, _)| Finding::DuplicateEntry {
                    path: name.to_owned(),
                })
                .collect()
        }
        Err(detail) => vec![Finding::CentralDirectoryUnreadable { detail }],
    }
}

/// Every entry name in the central directory, duplicates included, in
/// directory order. The `zip` reader indexes entries by name and keeps
/// one per name, so a name written twice — which extraction tools
/// resolve differently — is visible only here. The walk follows
/// APPNOTE 6.3: the end-of-central-directory record (the last record,
/// followed by at most a 65535-byte comment), the ZIP64 locator and
/// record when the classic fields overflow, then the fixed 46-byte
/// central headers with their variable name, extra, and comment parts.
fn central_directory_names(bytes: &[u8]) -> std::result::Result<Vec<String>, String> {
    const EOCD: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
    const ZIP64_LOCATOR: [u8; 4] = [0x50, 0x4b, 0x06, 0x07];
    const ZIP64_EOCD: [u8; 4] = [0x50, 0x4b, 0x06, 0x06];
    const CENTRAL_HEADER: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
    let eocd = (0..=bytes.len().saturating_sub(22))
        .rev()
        .take(usize::from(u16::MAX) + 1)
        .find(|&at| bytes.get(at..at + 4) == Some(&EOCD[..]))
        .ok_or("no end-of-central-directory record")?;
    let mut count = u64::from(le_u16(bytes, eocd + 10)?);
    let mut start = u64::from(le_u32(bytes, eocd + 16)?);
    if count == u64::from(u16::MAX) || start == u64::from(u32::MAX) {
        let locator = eocd
            .checked_sub(20)
            .filter(|&at| bytes.get(at..at + 4) == Some(&ZIP64_LOCATOR[..]))
            .ok_or("ZIP64 fields without a ZIP64 locator")?;
        let zip64 = usize::try_from(le_u64(bytes, locator + 8)?)
            .map_err(|_| "ZIP64 record offset out of range".to_owned())?;
        if bytes.get(zip64..zip64 + 4) != Some(&ZIP64_EOCD[..]) {
            return Err("ZIP64 locator points at no ZIP64 record".to_owned());
        }
        count = le_u64(bytes, zip64 + 32)?;
        start = le_u64(bytes, zip64 + 48)?;
    }
    let mut at = usize::try_from(start).map_err(|_| "central directory offset out of range")?;
    let mut names = Vec::new();
    for _ in 0..count {
        if bytes.get(at..at + 4) != Some(&CENTRAL_HEADER[..]) {
            return Err(format!("no central directory header at offset {at}"));
        }
        let name_len = usize::from(le_u16(bytes, at + 28)?);
        let extra_len = usize::from(le_u16(bytes, at + 30)?);
        let comment_len = usize::from(le_u16(bytes, at + 32)?);
        let name = bytes
            .get(at + 46..at + 46 + name_len)
            .ok_or("truncated central directory header")?;
        names.push(String::from_utf8_lossy(name).into_owned());
        at += 46 + name_len + extra_len + comment_len;
    }
    Ok(names)
}

fn le_u16(bytes: &[u8], at: usize) -> std::result::Result<u16, String> {
    bytes
        .get(at..at + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or_else(|| format!("truncated record at offset {at}"))
}

fn le_u32(bytes: &[u8], at: usize) -> std::result::Result<u32, String> {
    bytes
        .get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| format!("truncated record at offset {at}"))
}

fn le_u64(bytes: &[u8], at: usize) -> std::result::Result<u64, String> {
    bytes
        .get(at..at + 8)
        .map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
        .ok_or_else(|| format!("truncated record at offset {at}"))
}

/// Reads one entry: `Ok(None)` when absent, `Err(detail)` when the
/// container cannot deliver it (a CRC mismatch included).
fn read_entry(
    archive: &mut Archive<'_>,
    name: &str,
) -> std::result::Result<Option<Vec<u8>>, String> {
    match archive.by_name(name) {
        Ok(mut file) => {
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)
                .map_err(|err| err.to_string())?;
            Ok(Some(bytes))
        }
        Err(zip::result::ZipError::FileNotFound) => Ok(None),
        Err(err) => Err(err.to_string()),
    }
}
