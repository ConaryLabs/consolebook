//! Trainee-packet verification (ADR 0015; docs/formats/trainee-packet.md).
//!
//! `export_verify` dispatches here when an archive's manifest names the
//! packet format. The unit checks are the record export's, shared
//! through `export_verify`; this module owns what a packet adds: the
//! trainee scope of every unit, and the documents — present, hashing to
//! the manifest, canonical, of their kind's typed shape, in their
//! mandated order, obeying the cross-member rules the stored tables
//! impose, and referring only to versions the packet carries.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::canonical;
use crate::export_verify::{
    ArchiveKind, ArchiveReport, DocumentReport, Finding, is_lowercase_hex_hash, unlisted_entries,
    verify_units,
};
use crate::record_envelope;
use crate::record_export::{ARCHIVE_MANIFEST_PATH, RECORD_FILE, UnitEntry, canonical_json};
use crate::trainee_packet::{
    AcknowledgmentDoc, AmendmentDoc, DocumentEntry, DocumentKind, EnrollmentDocument,
    EnrollmentEventDoc, PacketManifest, PhaseEventDoc, SignoffDoc, signoff_shape_errors,
};
use crate::zip_container::{Archive, read_entry};

pub(crate) fn verify_packet(
    archive: &mut Archive<'_>,
    names: &[String],
    manifest_bytes: &[u8],
    report: &mut ArchiveReport,
) {
    report.kind = Some(ArchiveKind::TraineePacket);
    let manifest: PacketManifest = match serde_json::from_slice(manifest_bytes) {
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
    report.enrollment_id = Some(manifest.enrollment.id);
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
    // A packet is one trainee's: every unit's envelope names them.
    for (index, entry) in manifest.units.iter().enumerate() {
        if let Some(trainee) = unit_trainee(archive, entry)
            && trainee != manifest.enrollment.trainee.id
        {
            report.units[index]
                .findings
                .push(Finding::UnitOutsideScope {
                    path: entry.path.clone(),
                });
        }
    }
    verify_documents(archive, &manifest, &mut listed, report);
    unlisted_entries(names, &listed, report);
}

/// The trainee a unit's envelope names, when the unit reads as one.
fn unit_trainee(archive: &mut Archive<'_>, entry: &UnitEntry) -> Option<i64> {
    let bytes = read_entry(archive, &format!("{}/{RECORD_FILE}", entry.path)).ok()??;
    record_envelope::parse(&bytes)
        .ok()
        .map(|envelope| envelope.trainee.id)
}

/// A version-1 packet's documents: each kind once, in path order, at its
/// derived path, hashing to the manifest, canonical, of its kind's shape
/// and order, and referring only to versions the packet carries.
fn verify_documents(
    archive: &mut Archive<'_>,
    manifest: &PacketManifest,
    listed: &mut BTreeSet<String>,
    report: &mut ArchiveReport,
) {
    let mut kinds: Vec<DocumentKind> = manifest.documents.iter().map(|doc| doc.kind).collect();
    kinds.sort();
    let mut expected: Vec<DocumentKind> = DocumentKind::ALL.to_vec();
    expected.sort();
    let ordered = manifest
        .documents
        .windows(2)
        .all(|pair| pair[0].path < pair[1].path);
    if kinds != expected || !ordered {
        report.findings.push(Finding::DocumentsIncomplete);
    }
    let carried: BTreeSet<(i64, i64)> = manifest
        .units
        .iter()
        .map(|unit| (unit.record_id, unit.version_number))
        .collect();
    for doc in &manifest.documents {
        let expected_path = doc.kind.path();
        if doc.path != expected_path {
            report.findings.push(Finding::DocumentPathUnexpected {
                path: doc.path.clone(),
                expected: expected_path,
            });
        }
        listed.insert(doc.path.clone());
        let mut findings = Vec::new();
        if !is_lowercase_hex_hash(&doc.sha256) {
            findings.push(Finding::HashNotCanonical { member: "sha256" });
        }
        match read_entry(archive, &doc.path) {
            Ok(Some(bytes)) => {
                if canonical::content_hash_hex(&bytes) != doc.sha256 {
                    findings.push(Finding::DocumentHashMismatch {
                        path: doc.path.clone(),
                    });
                }
                match serde_json::from_slice::<Value>(&bytes) {
                    Ok(document) => match canonical::canonical_bytes(&document) {
                        Ok(again) if again == bytes => {}
                        _ => findings.push(Finding::DocumentInvalid {
                            path: doc.path.clone(),
                            detail: "not canonical JSON".to_owned(),
                        }),
                    },
                    Err(err) => findings.push(Finding::DocumentInvalid {
                        path: doc.path.clone(),
                        detail: format!("not JSON: {err}"),
                    }),
                }
                findings.extend(check_document(doc, &bytes, manifest, &carried));
            }
            Ok(None) => findings.push(Finding::MissingEntry {
                path: doc.path.clone(),
            }),
            Err(detail) => findings.push(Finding::EntryUnreadable {
                path: doc.path.clone(),
                detail,
            }),
        }
        report.documents.push(DocumentReport {
            path: doc.path.clone(),
            kind: doc.kind,
            findings,
        });
    }
}

/// The typed shape, mandated order, cross-member rules, and references
/// of one document kind.
fn check_document(
    doc: &DocumentEntry,
    bytes: &[u8],
    manifest: &PacketManifest,
    carried: &BTreeSet<(i64, i64)>,
) -> Vec<Finding> {
    let path = doc.path.as_str();
    match doc.kind {
        DocumentKind::Acknowledgments => {
            match serde_json::from_slice::<Vec<AcknowledgmentDoc>>(bytes) {
                Ok(acknowledgments) => {
                    check_acknowledgments(path, &acknowledgments, manifest, carried)
                }
                Err(err) => vec![unparsed(path, &err)],
            }
        }
        DocumentKind::Amendments => match serde_json::from_slice::<Vec<AmendmentDoc>>(bytes) {
            Ok(amendments) => check_amendments(path, &amendments, carried),
            Err(err) => vec![unparsed(path, &err)],
        },
        DocumentKind::Enrollment => match serde_json::from_slice::<EnrollmentDocument>(bytes) {
            Ok(enrollment) => check_enrollment(path, &enrollment, manifest),
            Err(err) => vec![unparsed(path, &err)],
        },
        DocumentKind::Signoffs => match serde_json::from_slice::<Vec<SignoffDoc>>(bytes) {
            Ok(signoffs) => check_signoffs(path, &signoffs),
            Err(err) => vec![unparsed(path, &err)],
        },
    }
}

fn unparsed(path: &str, err: &serde_json::Error) -> Finding {
    Finding::DocumentInvalid {
        path: path.to_owned(),
        detail: err.to_string(),
    }
}

fn misshapen(path: &str, detail: String) -> Finding {
    Finding::DocumentInvalid {
        path: path.to_owned(),
        detail,
    }
}

fn dangling(path: &str, what: &str, record_id: i64, version_number: i64) -> Finding {
    Finding::DocumentReference {
        path: path.to_owned(),
        detail: format!(
            "{what} names record {record_id} version {version_number}, which the packet does not carry"
        ),
    }
}

/// The first adjacent pair of rows not strictly ascending by `key`, as
/// a finding. The format's orders are total, so equal keys — a
/// duplicated row — are out of order too.
fn out_of_order<T, K: Ord>(
    path: &str,
    rows: &[T],
    what: &str,
    by: &str,
    key: impl Fn(&T) -> K,
) -> Option<Finding> {
    rows.windows(2)
        .position(|pair| key(&pair[0]) >= key(&pair[1]))
        .map(|index| Finding::DocumentOutOfOrder {
            path: path.to_owned(),
            detail: format!(
                "{what} {} does not follow {what} {index} in ascending {by}",
                index + 1
            ),
        })
}

fn check_acknowledgments(
    path: &str,
    acknowledgments: &[AcknowledgmentDoc],
    manifest: &PacketManifest,
    carried: &BTreeSet<(i64, i64)>,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    findings.extend(out_of_order(
        path,
        acknowledgments,
        "acknowledgment",
        "(record_id, version_number)",
        |ack| (ack.record_id, ack.version_number),
    ));
    if acknowledgments
        .iter()
        .any(|ack| ack.user.id != manifest.enrollment.trainee.id)
    {
        findings.push(Finding::DocumentDisagrees {
            path: path.to_owned(),
            member: "user.id",
        });
    }
    for ack in acknowledgments {
        if !carried.contains(&(ack.record_id, ack.version_number)) {
            findings.push(dangling(
                path,
                "an acknowledgment",
                ack.record_id,
                ack.version_number,
            ));
        }
        if let Some(detail) = ack.shape_error() {
            findings.push(misshapen(path, detail));
        }
    }
    findings
}

fn check_amendments(
    path: &str,
    amendments: &[AmendmentDoc],
    carried: &BTreeSet<(i64, i64)>,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    findings.extend(out_of_order(
        path,
        amendments,
        "amendment",
        "(record_id, predecessor_version_number)",
        |amendment| (amendment.record_id, amendment.predecessor_version_number),
    ));
    for amendment in amendments {
        if !carried.contains(&(amendment.record_id, amendment.predecessor_version_number)) {
            findings.push(dangling(
                path,
                "an amendment's predecessor",
                amendment.record_id,
                amendment.predecessor_version_number,
            ));
        }
        if let Some(successor) = amendment.successor_version_number
            && !carried.contains(&(amendment.record_id, successor))
        {
            findings.push(dangling(
                path,
                "an amendment's successor",
                amendment.record_id,
                successor,
            ));
        }
        if let Some(detail) = amendment.shape_error() {
            findings.push(misshapen(path, detail));
        }
    }
    findings
}

fn check_enrollment(
    path: &str,
    enrollment: &EnrollmentDocument,
    manifest: &PacketManifest,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    if enrollment.enrollment_id != manifest.enrollment.id {
        findings.push(Finding::DocumentDisagrees {
            path: path.to_owned(),
            member: "enrollment_id",
        });
    }
    findings.extend(out_of_order(
        path,
        &enrollment.events,
        "event",
        "event_id",
        |event| event.event_id,
    ));
    findings.extend(out_of_order(
        path,
        &enrollment.phase_events,
        "phase event",
        "(effective_at, event_id)",
        |event| (event.effective_at, event.event_id),
    ));
    findings.extend(
        enrollment
            .events
            .iter()
            .filter_map(EnrollmentEventDoc::shape_error)
            .map(|detail| misshapen(path, detail)),
    );
    findings.extend(
        enrollment
            .phase_events
            .iter()
            .filter_map(PhaseEventDoc::shape_error)
            .map(|detail| misshapen(path, detail)),
    );
    findings
}

fn check_signoffs(path: &str, signoffs: &[SignoffDoc]) -> Vec<Finding> {
    let mut findings = Vec::new();
    findings.extend(out_of_order(
        path,
        signoffs,
        "signoff",
        "signoff_id",
        |signoff| signoff.signoff_id,
    ));
    findings.extend(
        signoff_shape_errors(signoffs)
            .into_iter()
            .map(|detail| misshapen(path, detail)),
    );
    findings
}
