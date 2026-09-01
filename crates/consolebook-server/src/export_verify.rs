//! Verification of record export archives from their bytes alone
//! (ADR 0014; docs/formats/record-export.md; #45).
//!
//! `record_export` produces archives; this module owns the independent
//! check: typed findings, per-unit and per-archive reports, the
//! container walk (including the central directory, which the `zip`
//! reader collapses by name), and the normative check list of the
//! format document. The verdict is consistency with the stated
//! fingerprints, never tamper-proofing (ADR 0010, ADR 0011). The
//! container's raw reading lives in `zip_container` and the trainee
//! packet's document checks in `packet_verify`; this module owns the
//! findings, the reports, the format dispatch, and the unit checks every
//! format shares.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Cursor;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::canonical;
use crate::record_envelope;
use crate::record_export::{
    ARCHIVE_FORMAT, ARCHIVE_MANIFEST_PATH, ArchiveManifest, FORMAT_VERSION, RECORD_FILE, Scope,
    UNIT_FORMAT, UNIT_MANIFEST_FILE, UnitEntry, UnitManifest, canonical_json, unit_path,
};
use crate::trainee_packet::{DocumentKind, PACKET_FORMAT, PACKET_FORMAT_VERSION};
use crate::zip_container::{Archive, central_directory_names, read_entry};

/// One thing a verifier found wrong. The verdict derives from the
/// absence of findings; wording is presentation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Finding {
    /// A packet document's bytes do not hash to the manifest's `sha256`.
    DocumentHashMismatch {
        path: String,
    },
    /// A packet document is not canonical JSON of its kind's shape.
    DocumentInvalid {
        path: String,
        detail: String,
    },
    /// A packet document disagrees with the manifest on a member.
    DocumentDisagrees {
        path: String,
        member: &'static str,
    },
    /// A packet document refers to a version the packet does not list.
    DocumentReference {
        path: String,
        detail: String,
    },
    /// A packet document's rows are not in the order the format
    /// mandates (a duplicated row included).
    DocumentOutOfOrder {
        path: String,
        detail: String,
    },
    /// A version-1 packet lists each document kind exactly once, in path
    /// order.
    DocumentsIncomplete,
    DocumentPathUnexpected {
        path: String,
        expected: String,
    },
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
    /// The bytes are not an envelope of any known record schema: a
    /// member missing, unnamed by the schema, or of the wrong type.
    EnvelopeInvalid {
        detail: String,
    },
    ChainHashMismatch,
    /// A hash member is not 64 lowercase hex characters.
    HashNotCanonical {
        member: &'static str,
    },
    /// A first version with a predecessor, or a later one without.
    LineageShape,
    /// `record_id` and `version_number` are positive integers.
    IdentityOutOfRange {
        member: &'static str,
    },
    /// The predecessor is in the archive and its content hash is not
    /// what this unit's chain was computed over.
    PredecessorMismatch,
    /// A packet carries every retained version, so a unit whose
    /// predecessor is not carried is a hole in the lineage.
    PredecessorNotCarried,
    /// A packet document disagrees with the lineage the carried units
    /// establish.
    DocumentLineage {
        path: String,
        detail: String,
    },
}

impl fmt::Display for Finding {
    // One arm per finding: the list is as long as the format's vocabulary.
    #[allow(clippy::too_many_lines)]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DocumentHashMismatch { path } => {
                write!(f, "{path} does not hash to the manifest's sha256")
            }
            Self::DocumentInvalid { path, detail } => {
                write!(f, "{path} is not a valid document: {detail}")
            }
            Self::DocumentDisagrees { path, member } => {
                write!(f, "{path} disagrees with the manifest on {member}")
            }
            Self::DocumentReference { path, detail } => {
                write!(f, "{path} refers outside the packet: {detail}")
            }
            Self::DocumentOutOfOrder { path, detail } => {
                write!(f, "{path} is out of order: {detail}")
            }
            Self::DocumentsIncomplete => f.write_str(
                "the packet lists its document kinds incompletely, twice, or out of order",
            ),
            Self::DocumentPathUnexpected { path, expected } => {
                write!(f, "document path {path} should be {expected}")
            }
            Self::HashNotCanonical { member } => {
                write!(f, "{member} is not 64 lowercase hex characters")
            }
            Self::IdentityOutOfRange { member } => {
                write!(f, "{member} is not a positive integer")
            }
            Self::EnvelopeInvalid { detail } => {
                write!(f, "the record bytes are not a valid envelope: {detail}")
            }
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
            Self::PredecessorNotCarried => f.write_str(
                "the predecessor this version names is not carried; a packet carries every retained version",
            ),
            Self::DocumentLineage { path, detail } => {
                write!(f, "{path} disagrees with the carried lineage: {detail}")
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

/// Which format the archive declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveKind {
    RecordExport,
    TraineePacket,
}

impl fmt::Display for ArchiveKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::RecordExport => "record export",
            Self::TraineePacket => "trainee packet",
        })
    }
}

/// One packet document's verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DocumentReport {
    pub path: String,
    pub kind: DocumentKind,
    pub findings: Vec<Finding>,
}

impl DocumentReport {
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
    pub kind: Option<ArchiveKind>,
    pub installation_id: Option<String>,
    pub exported_at: Option<i64>,
    /// A record export's declared scope.
    pub scope: Option<Scope>,
    /// A trainee packet's enrollment.
    pub enrollment_id: Option<i64>,
    pub units: Vec<UnitReport>,
    pub documents: Vec<DocumentReport>,
    pub findings: Vec<Finding>,
}

impl ArchiveReport {
    #[must_use]
    pub fn verified(&self) -> bool {
        self.findings.is_empty()
            && self.units.iter().all(UnitReport::verified)
            && self.documents.iter().all(DocumentReport::verified)
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

/// Verifies an archive from its bytes alone, per the normative checks
/// in docs/formats/record-export.md.
#[must_use]
pub fn verify_archive(bytes: &[u8]) -> ArchiveReport {
    let mut report = ArchiveReport {
        kind: None,
        installation_id: None,
        exported_at: None,
        scope: None,
        enrollment_id: None,
        units: Vec::new(),
        documents: Vec::new(),
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
    // The manifest names its own format; everything else follows from
    // that name and version, never from guessing at the members.
    let probe: FormatProbe = match serde_json::from_slice(&manifest_bytes) {
        Ok(probe) => probe,
        Err(err) => {
            report.findings.push(Finding::ArchiveManifestUnreadable {
                detail: err.to_string(),
            });
            return report;
        }
    };
    match (probe.format.as_str(), probe.format_version) {
        (ARCHIVE_FORMAT, FORMAT_VERSION) => {
            verify_export(&mut archive, &names, &manifest_bytes, &mut report);
        }
        (PACKET_FORMAT, PACKET_FORMAT_VERSION) => {
            crate::packet_verify::verify_packet(&mut archive, &names, &manifest_bytes, &mut report);
        }
        _ => report.findings.push(Finding::UnsupportedFormat {
            format: probe.format,
            format_version: probe.format_version,
        }),
    }
    report
}

/// The two members every manifest of every known format carries.
#[derive(Deserialize)]
struct FormatProbe {
    format: String,
    format_version: i64,
}

fn verify_export(
    archive: &mut Archive<'_>,
    names: &[String],
    manifest_bytes: &[u8],
    report: &mut ArchiveReport,
) {
    report.kind = Some(ArchiveKind::RecordExport);
    let manifest: ArchiveManifest = match serde_json::from_slice(manifest_bytes) {
        Ok(manifest) => manifest,
        Err(err) => {
            report.findings.push(Finding::ArchiveManifestUnreadable {
                detail: err.to_string(),
            });
            return;
        }
    };
    if canonical_json(&manifest).ok().as_deref() != Some(manifest_bytes) {
        report.findings.push(Finding::ManifestNotCanonical {
            path: ARCHIVE_MANIFEST_PATH.to_owned(),
        });
    }
    report.installation_id = Some(manifest.installation_id.clone());
    report.exported_at = Some(manifest.exported_at);
    report.scope = Some(manifest.scope);
    report.findings.extend(scope_findings(&manifest));
    let mut listed: BTreeSet<String> = BTreeSet::new();
    listed.insert(ARCHIVE_MANIFEST_PATH.to_owned());
    verify_units(
        archive,
        &manifest.installation_id,
        manifest.exported_at,
        &manifest.units,
        &mut listed,
        report,
    );
    unlisted_entries(names, &listed, report);
}

/// The unit checks shared by every format: order, derived paths, and
/// each unit's own verification.
pub(crate) fn verify_units(
    archive: &mut Archive<'_>,
    installation_id: &str,
    exported_at: i64,
    units: &[UnitEntry],
    listed: &mut BTreeSet<String>,
    report: &mut ArchiveReport,
) {
    let ordered = units.windows(2).all(|pair| {
        (pair[0].record_id, pair[0].version_number) < (pair[1].record_id, pair[1].version_number)
    });
    if !ordered {
        report.findings.push(Finding::UnitsOutOfOrder);
    }
    for entry in units {
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
    let by_identity: BTreeMap<(i64, i64), &UnitEntry> = units
        .iter()
        .map(|entry| ((entry.record_id, entry.version_number), entry))
        .collect();
    for entry in units {
        report.units.push(verify_unit(
            archive,
            installation_id,
            exported_at,
            entry,
            &by_identity,
        ));
    }
}

pub(crate) fn unlisted_entries(
    names: &[String],
    listed: &BTreeSet<String>,
    report: &mut ArchiveReport,
) {
    for name in names {
        if !listed.contains(name) {
            report
                .findings
                .push(Finding::UnlistedEntry { path: name.clone() });
        }
    }
}

#[allow(clippy::too_many_lines)]
fn verify_unit(
    archive: &mut Archive<'_>,
    installation_id: &str,
    exported_at: i64,
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
                    ("installation_id", unit.installation_id != installation_id),
                    ("exported_at", unit.exported_at != exported_at),
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
                Ok(document) => match canonical::canonical_bytes(&document) {
                    Ok(again) if again == bytes => {}
                    Ok(_) => findings.push(Finding::NotCanonical {
                        detail: "re-serialization differs from the stored bytes".to_owned(),
                    }),
                    Err(err) => findings.push(Finding::NotCanonical {
                        detail: err.to_string(),
                    }),
                },
                Err(err) => findings.push(Finding::NotCanonical {
                    detail: format!("not JSON: {err}"),
                }),
            }
            // The bytes must be an envelope of a known record schema —
            // every member the schema names, typed, and no other — before
            // the identity they carry is compared with the manifest.
            match record_envelope::parse(&bytes) {
                Ok(envelope) => {
                    let disagreements: [(&'static str, bool); 6] = [
                        ("record.id", envelope.record.id != entry.record_id),
                        (
                            "record.version_number",
                            envelope.record.version_number != entry.version_number,
                        ),
                        (
                            "record.record_schema",
                            envelope.record.record_schema != entry.record_schema,
                        ),
                        (
                            "record.predecessor_content_hash",
                            envelope.record.predecessor_content_hash
                                != entry.predecessor_content_hash,
                        ),
                        ("instance", envelope.instance != installation_id),
                        (
                            "canonicalization",
                            envelope.canonicalization != canonical::CANONICALIZATION,
                        ),
                    ];
                    for (member, disagrees) in disagreements {
                        if disagrees {
                            findings.push(Finding::EnvelopeDisagrees { member });
                        }
                    }
                }
                Err(err) => findings.push(Finding::EnvelopeInvalid {
                    detail: err.to_string(),
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

    // Hashes are 64 lowercase hex characters by the format. The chain
    // recomputation decodes hex case-insensitively, so without this an
    // uppercase predecessor hash would pass for a lone successor and
    // then fail to link once its lowercase predecessor joined it.
    let hash_members: [(&'static str, Option<&str>); 3] = [
        ("content_hash", Some(entry.content_hash.as_str())),
        ("chain_hash", Some(entry.chain_hash.as_str())),
        (
            "predecessor_content_hash",
            entry.predecessor_content_hash.as_deref(),
        ),
    ];
    for (member, value) in hash_members {
        if let Some(hex) = value
            && !is_lowercase_hex_hash(hex)
        {
            findings.push(Finding::HashNotCanonical { member });
        }
    }
    // Identity is positive by the format: the database assigns record
    // ids from 1 and version numbers start at 1, so a zero or negative
    // number is not an identity the lineage rule below can reason about.
    if entry.record_id < 1 {
        findings.push(Finding::IdentityOutOfRange {
            member: "record_id",
        });
    }
    if entry.version_number < 1 {
        findings.push(Finding::IdentityOutOfRange {
            member: "version_number",
        });
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

pub(crate) fn is_lowercase_hex_hash(hex: &str) -> bool {
    hex.len() == 64
        && hex
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}
