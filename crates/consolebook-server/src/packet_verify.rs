//! Trainee-packet verification (ADR 0015; docs/formats/trainee-packet.md).
//!
//! `export_verify` dispatches here when an archive's manifest names the
//! packet format. The unit checks are the record export's, shared
//! through `export_verify`; this module owns what a packet adds: the
//! trainee scope of every unit, the lineage being whole, and the
//! documents — present, hashing to the manifest, canonical, of their
//! kind's typed shape, in their mandated order, obeying the cross-member
//! rules the stored tables impose, referring only to versions the packet
//! carries, and agreeing with one another about the record lineage and
//! the enrollment's pin history.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::canonical;
use crate::export_verify::{
    ArchiveKind, ArchiveReport, DocumentReport, Finding, PredecessorLink, is_lowercase_hex_hash,
    unlisted_entries, verify_units,
};
use crate::lifecycle::EnrollmentEventKind;
use crate::record_envelope;
use crate::record_export::{ARCHIVE_MANIFEST_PATH, RECORD_FILE, UnitEntry, canonical_json};
use crate::trainee_packet::{
    AcknowledgmentDoc, AmendmentDoc, DocumentEntry, DocumentKind, EnrollmentDocument,
    EnrollmentEventDoc, PacketManifest, PhaseEventDoc, SignoffDoc, VersionRef,
    signoff_shape_errors,
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
    if let Some(detail) = manifest.enrollment.shape_error() {
        report
            .findings
            .push(Finding::ManifestEnrollmentInvalid { detail });
    }
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
    // A packet carries every retained version: a unit whose predecessor
    // is not carried is a hole in the lineage, not a scope choice.
    for unit in &mut report.units {
        if unit.predecessor == PredecessorLink::NotInExport {
            unit.findings.push(Finding::PredecessorNotCarried);
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

/// A document read as its kind's shape.
enum Parsed {
    Acknowledgments(Vec<AcknowledgmentDoc>),
    Amendments(Vec<AmendmentDoc>),
    Enrollment(EnrollmentDocument),
    Signoffs(Vec<SignoffDoc>),
}

fn parse_document(kind: DocumentKind, bytes: &[u8]) -> Result<Parsed, serde_json::Error> {
    Ok(match kind {
        DocumentKind::Acknowledgments => Parsed::Acknowledgments(serde_json::from_slice(bytes)?),
        DocumentKind::Amendments => Parsed::Amendments(serde_json::from_slice(bytes)?),
        DocumentKind::Enrollment => Parsed::Enrollment(serde_json::from_slice(bytes)?),
        DocumentKind::Signoffs => Parsed::Signoffs(serde_json::from_slice(bytes)?),
    })
}

/// A version-1 packet's documents: each kind once, in path order, at its
/// derived path, hashing to the manifest, canonical, of its kind's shape
/// and order, referring only to versions the packet carries, and
/// agreeing with one another about the enrollment's pin history.
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

    // Pass one: each document's bytes, hash, canonical form, and shape.
    let mut documents: Vec<(DocumentReport, Option<Parsed>)> = manifest
        .documents
        .iter()
        .map(|doc| read_document(archive, doc, listed, report))
        .collect();

    // Pass two: each document's order, rules, and references.
    for (document, parsed) in &mut documents {
        if let Some(parsed) = parsed {
            document
                .findings
                .extend(check_parsed(&document.path, parsed, manifest, &carried));
        }
    }

    // Pass three: the pin history the enrollment document defines, and
    // every program version the other documents name against it.
    let enrollment = documents
        .iter()
        .find_map(|(document, parsed)| match parsed {
            Some(Parsed::Enrollment(enrollment)) => {
                Some((document.path.clone(), enrollment.clone()))
            }
            _ => None,
        });
    if let Some((enrollment_path, enrollment)) = enrollment {
        let (history, history_findings) = PinHistory::of(&enrollment_path, manifest, &enrollment);
        for (document, parsed) in &mut documents {
            match parsed {
                Some(Parsed::Enrollment(enrollment)) => {
                    document.findings.extend(history_findings.iter().cloned());
                    document
                        .findings
                        .extend(history.check_phase_events(&document.path, enrollment));
                }
                Some(Parsed::Signoffs(signoffs)) => document
                    .findings
                    .extend(history.check_signoffs(&document.path, signoffs)),
                _ => {}
            }
        }
    }
    report
        .documents
        .extend(documents.into_iter().map(|(document, _)| document));
}

/// One listed document: its path, entry, hash, canonical form, and
/// shape, with the parsed document when it has one.
fn read_document(
    archive: &mut Archive<'_>,
    doc: &DocumentEntry,
    listed: &mut BTreeSet<String>,
    report: &mut ArchiveReport,
) -> (DocumentReport, Option<Parsed>) {
    let expected_path = doc.kind.path();
    if doc.path != expected_path {
        report.findings.push(Finding::DocumentPathUnexpected {
            path: doc.path.clone(),
            expected: expected_path,
        });
    }
    listed.insert(doc.path.clone());
    let mut findings = Vec::new();
    let mut parsed = None;
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
            match parse_document(doc.kind, &bytes) {
                Ok(document) => parsed = Some(document),
                Err(err) => findings.push(unparsed(&doc.path, &err)),
            }
        }
        Ok(None) => findings.push(Finding::MissingEntry {
            path: doc.path.clone(),
        }),
        Err(detail) => findings.push(Finding::EntryUnreadable {
            path: doc.path.clone(),
            detail,
        }),
    }
    (
        DocumentReport {
            path: doc.path.clone(),
            kind: doc.kind,
            findings,
        },
        parsed,
    )
}

/// The typed shape, mandated order, cross-member rules, and references
/// of one parsed document.
fn check_parsed(
    path: &str,
    parsed: &Parsed,
    manifest: &PacketManifest,
    carried: &BTreeSet<(i64, i64)>,
) -> Vec<Finding> {
    match parsed {
        Parsed::Acknowledgments(acknowledgments) => {
            check_acknowledgments(path, acknowledgments, manifest, carried)
        }
        Parsed::Amendments(amendments) => check_amendments(path, amendments, manifest, carried),
        Parsed::Enrollment(enrollment) => check_enrollment(path, enrollment, manifest),
        Parsed::Signoffs(signoffs) => check_signoffs(path, signoffs),
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

fn off_history(path: &str, detail: String) -> Finding {
    Finding::DocumentPinHistory {
        path: path.to_owned(),
        detail,
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

/// The lineage the carried units establish, from the manifest's own
/// hashes: which carried version succeeds which. The unit checks hold
/// those hashes to the bytes; this reads them as the record of
/// succession the amendments document must agree with.
struct Lineage {
    /// (`record_id`, `version_number`) → the carried version that
    /// succeeds it.
    successor: BTreeMap<(i64, i64), i64>,
}

impl Lineage {
    fn of(units: &[UnitEntry]) -> Self {
        let by_hash: BTreeMap<(i64, &str), i64> = units
            .iter()
            .map(|unit| {
                (
                    (unit.record_id, unit.content_hash.as_str()),
                    unit.version_number,
                )
            })
            .collect();
        let mut successor = BTreeMap::new();
        for unit in units {
            if let Some(hash) = unit.predecessor_content_hash.as_deref()
                && let Some(&earlier) = by_hash.get(&(unit.record_id, hash))
            {
                successor.insert((unit.record_id, earlier), unit.version_number);
            }
        }
        Self { successor }
    }
}

/// Amendments in their order, giving reasons, referring only to carried
/// versions, and agreeing with the carried lineage both ways: a sealed
/// amendment names the version that succeeds the one it corrected, an
/// amendment in progress has no carried successor, and every carried
/// successor has its amendment recorded.
fn check_amendments(
    path: &str,
    amendments: &[AmendmentDoc],
    manifest: &PacketManifest,
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
    let lineage = Lineage::of(&manifest.units);
    let disagrees = |detail: String| Finding::DocumentLineage {
        path: path.to_owned(),
        detail,
    };
    for amendment in amendments {
        let (record_id, predecessor) = (amendment.record_id, amendment.predecessor_version_number);
        if let Some(detail) = amendment.shape_error() {
            findings.push(misshapen(path, detail));
        }
        if !carried.contains(&(record_id, predecessor)) {
            findings.push(dangling(
                path,
                "an amendment's predecessor",
                record_id,
                predecessor,
            ));
            continue;
        }
        let named = amendment.successor_version_number;
        if let Some(named) = named
            && !carried.contains(&(record_id, named))
        {
            findings.push(dangling(path, "an amendment's successor", record_id, named));
        }
        let what = format!("the amendment of record {record_id} version {predecessor}");
        match (named, lineage.successor.get(&(record_id, predecessor)).copied()) {
            (None, None) => {}
            (Some(named), Some(actual)) if named == actual => {}
            (None, Some(actual)) => findings.push(disagrees(format!(
                "{what} is still in progress, but the packet carries version {actual}, which succeeds version {predecessor}"
            ))),
            (Some(named), Some(actual)) => findings.push(disagrees(format!(
                "{what} names version {named} as its successor, but version {actual} succeeds version {predecessor}"
            ))),
            (Some(named), None) => {
                if carried.contains(&(record_id, named)) {
                    findings.push(disagrees(format!(
                        "{what} names version {named} as its successor, but version {named} does not succeed version {predecessor}"
                    )));
                }
            }
        }
    }
    let recorded: BTreeSet<(i64, i64)> = amendments
        .iter()
        .map(|amendment| (amendment.record_id, amendment.predecessor_version_number))
        .collect();
    for (&(record_id, predecessor), &successor) in &lineage.successor {
        if !recorded.contains(&(record_id, predecessor)) {
            findings.push(disagrees(format!(
                "version {successor} of record {record_id} succeeds version {predecessor}, but no amendment records the correction"
            )));
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

/// The enrollment's pin history, from the manifest's current pin and the
/// lifecycle events' version changes: every version the enrollment ever
/// pinned, labelled as the packet labels it; the original pin; and the
/// version each version change reached, with its epoch's time boundaries.
struct PinHistory {
    labels: BTreeMap<i64, String>,
    original: PinEpoch,
    /// A version change's `event_id` → the epoch it opened.
    epochs: BTreeMap<i64, PinEpoch>,
}

struct PinEpoch {
    version: i64,
    opened_at: Option<i64>,
    closed_at: Option<i64>,
}

impl PinEpoch {
    /// Unix seconds cannot order a change and an act within that second.
    /// Both endpoints therefore belong to the epoch, even when equal.
    fn includes(&self, instant: i64) -> bool {
        self.opened_at.is_none_or(|opened| instant >= opened)
            && self.closed_at.is_none_or(|closed| instant <= closed)
    }
}

impl PinHistory {
    /// Builds the history, reporting where the events contradict it: a
    /// version change leaving a version other than the one pinned at that
    /// point, a history ending elsewhere than the manifest's pin, or a
    /// version labelled two ways.
    fn of(
        path: &str,
        manifest: &PacketManifest,
        enrollment: &EnrollmentDocument,
    ) -> (Self, Vec<Finding>) {
        let current = &manifest.enrollment.program;
        let changes: Vec<(&EnrollmentEventDoc, &VersionRef, &VersionRef)> = enrollment
            .events
            .iter()
            .filter_map(
                |event| match (event.kind, &event.from_version, &event.to_version) {
                    (EnrollmentEventKind::VersionChange, Some(from), Some(to)) => {
                        Some((event, from, to))
                    }
                    _ => None,
                },
            )
            .collect();
        let original = changes
            .first()
            .map_or(current.version_number, |(_, from, _)| from.version_number);
        let mut labels = BTreeMap::new();
        labels.insert(current.version_number, current.label.clone());
        let mut epochs = BTreeMap::new();
        let mut findings = Vec::new();
        let mut pinned = original;
        for (index, (event, from, to)) in changes.iter().enumerate() {
            let id = event.event_id;
            if from.version_number != pinned {
                findings.push(off_history(
                    path,
                    format!(
                        "event {id} leaves version {}, but the enrollment was pinned to version {pinned} at that point",
                        from.version_number
                    ),
                ));
            }
            for version in [from, to] {
                match labels.get(&version.version_number) {
                    Some(known) if *known != version.label => findings.push(off_history(
                        path,
                        format!(
                            "event {id} labels version {} {:?}, but the packet labels it {known:?}",
                            version.version_number, version.label
                        ),
                    )),
                    Some(_) => {}
                    None => {
                        labels.insert(version.version_number, version.label.clone());
                    }
                }
            }
            let closed_at = changes.get(index + 1).map(|(next, _, _)| next.occurred_at);
            if closed_at.is_some_and(|closed| closed < event.occurred_at) {
                findings.push(off_history(
                    path,
                    format!(
                        "version change {id} occurs after the next version change in recorded order"
                    ),
                ));
            }
            epochs.insert(
                id,
                PinEpoch {
                    version: to.version_number,
                    opened_at: Some(event.occurred_at),
                    closed_at,
                },
            );
            pinned = to.version_number;
        }
        if pinned != current.version_number {
            findings.push(off_history(
                path,
                format!(
                    "the version changes end at version {pinned}, but the manifest pins version {}",
                    current.version_number
                ),
            ));
        }
        (
            Self {
                labels,
                original: PinEpoch {
                    version: original,
                    opened_at: None,
                    closed_at: changes.first().map(|(event, _, _)| event.occurred_at),
                },
                epochs,
            },
            findings,
        )
    }

    /// A version some row names: pinned at some point, labelled as the
    /// packet labels it.
    fn check_version(&self, path: &str, who: &str, version: &VersionRef) -> Option<Finding> {
        match self.labels.get(&version.version_number) {
            None => Some(off_history(
                path,
                format!(
                    "{who} names program version {}, which the enrollment never pinned",
                    version.version_number
                ),
            )),
            Some(known) if *known != version.label => Some(off_history(
                path,
                format!(
                    "{who} labels version {} {:?}, but the packet labels it {known:?}",
                    version.version_number, version.label
                ),
            )),
            Some(_) => None,
        }
    }

    fn check_signoffs(&self, path: &str, signoffs: &[SignoffDoc]) -> Vec<Finding> {
        signoffs
            .iter()
            .filter_map(|signoff| {
                let who = format!("signoff {}", signoff.signoff_id);
                if let Some(finding) = self.check_version(path, &who, &signoff.program_version) {
                    return Some(finding);
                }
                let named = signoff.program_version.version_number;
                let pinned = std::iter::once(&self.original)
                    .chain(self.epochs.values())
                    .any(|epoch| epoch.version == named && epoch.includes(signoff.signed_at));
                (!pinned).then(|| off_history(path, format!(
                    "{who} names program version {named}, which was not pinned at signed_at {}",
                    signoff.signed_at,
                )))
            })
            .collect()
    }

    /// Every phase event names a pinned version, and the version its
    /// epoch reached: the original pin under `null`, otherwise the
    /// version the named version change reached. Effective and recorded times
    /// cannot predate the opening; recording cannot postdate the closing.
    fn check_phase_events(&self, path: &str, enrollment: &EnrollmentDocument) -> Vec<Finding> {
        enrollment
            .phase_events
            .iter()
            .filter_map(|event| {
                let who = format!("phase event {}", event.event_id);
                let named = event.program_version.version_number;
                if let Some(finding) = self.check_version(path, &who, &event.program_version) {
                    return Some(finding);
                }
                let epoch = match event.version_change_event_id {
                    None if named != self.original.version => return Some(off_history(
                        path,
                        format!(
                            "{who} is recorded under the original pin, but names version {named} rather than version {}",
                            self.original.version
                        ),
                    )),
                    None => &self.original,
                    Some(epoch) => match self.epochs.get(&epoch) {
                        None => return Some(off_history(
                            path,
                            format!(
                                "{who} names version change {epoch} as its epoch, which the history does not record"
                            ),
                        )),
                        Some(reached) if reached.version != named => return Some(off_history(
                            path,
                            format!(
                                "{who} names version {named} under the epoch that reached version {}",
                                reached.version,
                            ),
                        )),
                        Some(epoch) => epoch,
                    },
                };
                if let Some(opened) = epoch.opened_at {
                    if event.effective_at < opened {
                        return Some(off_history(path, format!(
                            "{who} takes effect at {}, before its epoch opened at {opened}",
                            event.effective_at,
                        )));
                    }
                    if event.recorded_at < opened {
                        return Some(off_history(path, format!(
                            "{who} was recorded at {}, before its epoch opened at {opened}",
                            event.recorded_at,
                        )));
                    }
                }
                epoch.closed_at.filter(|&closed| event.recorded_at > closed).map(|closed| {
                    off_history(path, format!(
                        "{who} was recorded at {}, after its epoch closed at {closed}",
                        event.recorded_at,
                    ))
                })
            })
            .collect()
    }
}
