//! Packet pin-history membership and timeline proof.

use super::*;

/// One version-change event as a forger would write it.
fn version_change(event_id: i64, from: (i64, &str), to: (i64, &str)) -> serde_json::Value {
    serde_json::json!({
        "actor": null,
        "event_id": event_id,
        "from_version": {"label": from.1, "version_number": from.0},
        "kind": "version_change",
        "occurred_at": 1_780_000_000,
        "reason": "Invented version change.",
        "to_version": {"label": to.1, "version_number": to.0},
    })
}

/// A phase event under a named epoch, naming a version.
fn epoch_phase_event(event_id: i64, epoch: Option<i64>, version: (i64, &str)) -> serde_json::Value {
    let mut event = phase_event("advance", None, Some("Phase One"), 10, 10, event_id);
    event["program_version"] = serde_json::json!({"label": version.1, "version_number": version.0});
    event["version_change_event_id"] = serde_json::json!(epoch);
    event
}

/// The lifecycle events define the enrollment's pin history, and every
/// program version the packet names belongs to it: the verifier refuses
/// a version the enrollment never pinned, a label that disagrees, a
/// version change that leaves a version other than the one pinned, a
/// history ending elsewhere than the manifest's pin, and a phase event
/// naming a version its epoch did not reach.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn packets_agree_with_their_pin_history() {
    let fx = Fixture::new().await;
    let s = seed(&fx, "pins").await;
    let original = pack(&fx, s.casey_id, s.enrollment_id).await;
    let listed = entries(&original);
    let enrollment = DocumentKind::Enrollment;
    let signoffs = DocumentKind::Signoffs;
    let next_event_id = |doc: &serde_json::Value| -> i64 {
        doc["events"]
            .as_array()
            .expect("events")
            .iter()
            .map(|event| event["event_id"].as_i64().expect("id"))
            .max()
            .unwrap_or(0)
            + 1
    };
    let history = |findings: &[Finding], expected: &str| {
        assert!(
            findings.len() == 1
                && matches!(
                    &findings[0],
                    Finding::DocumentPinHistory { detail, .. } if detail.contains(expected)
                ),
            "expected one pin-history finding containing {expected:?}: {findings:?}"
        );
    };

    // Signoffs naming a version the enrollment never pinned, and ones
    // labelling the pinned version differently from the packet.
    let report = forged(&listed, signoffs, |doc| {
        for row in doc.as_array_mut().expect("rows") {
            row["program_version"]["version_number"] = serde_json::json!(7);
        }
    });
    assert_eq!(report.documents[3].findings.len(), 2, "{report:?}");
    assert!(
        report.documents[3].findings.iter().all(|finding| matches!(
            finding,
            Finding::DocumentPinHistory { detail, .. } if detail.contains("never pinned")
        )),
        "{report:?}"
    );
    let report = forged(&listed, signoffs, |doc| {
        for row in doc.as_array_mut().expect("rows") {
            row["program_version"]["label"] = serde_json::json!("2026 rev B");
        }
    });
    assert!(
        report.documents[3].findings.iter().all(|finding| matches!(
            finding,
            Finding::DocumentPinHistory { detail, .. } if detail.contains("labels version 1")
        )),
        "{report:?}"
    );

    // A version change ending elsewhere than the manifest's pin; a
    // second change leaving a version other than the one pinned; an
    // event labelling the manifest's version another way.
    let report = forged(&listed, enrollment, |doc| {
        let id = next_event_id(doc);
        doc["events"]
            .as_array_mut()
            .expect("events")
            .push(version_change(id, (1, "2026 rev A"), (2, "2026 rev B")));
        doc["phase_events"] = serde_json::json!([]);
    });
    history(
        &report.documents[2].findings,
        "end at version 2, but the manifest pins version 1",
    );
    let report = forged(&listed, enrollment, |doc| {
        let id = next_event_id(doc);
        let events = doc["events"].as_array_mut().expect("events");
        events.push(version_change(id, (1, "2026 rev A"), (2, "2026 rev B")));
        events.push(version_change(id + 1, (3, "2026 rev C"), (1, "2026 rev A")));
        doc["phase_events"] = serde_json::json!([]);
    });
    history(
        &report.documents[2].findings,
        "leaves version 3, but the enrollment was pinned to version 2",
    );
    let report = forged(&listed, enrollment, |doc| {
        let id = next_event_id(doc);
        doc["events"]
            .as_array_mut()
            .expect("events")
            .push(version_change(id, (2, "2026 rev B"), (1, "Renamed")));
        doc["phase_events"] = serde_json::json!([]);
    });
    history(
        &report.documents[2].findings,
        "labels version 1 \"Renamed\", but the packet labels it \"2026 rev A\"",
    );

    // Phase events: an epoch the history does not record; a version the
    // named epoch did not reach; the original pin naming another version.
    let report = forged(&listed, enrollment, |doc| {
        doc["phase_events"] =
            serde_json::json!([epoch_phase_event(1, Some(999), (1, "2026 rev A"))]);
    });
    history(
        &report.documents[2].findings,
        "names version change 999 as its epoch, which the history does not record",
    );
    let report = forged(&listed, enrollment, |doc| {
        let id = next_event_id(doc);
        doc["events"]
            .as_array_mut()
            .expect("events")
            .push(version_change(id, (2, "2026 rev B"), (1, "2026 rev A")));
        doc["phase_events"] =
            serde_json::json!([epoch_phase_event(1, Some(id), (2, "2026 rev B"))]);
    });
    history(
        &report.documents[2].findings,
        "names version 2 under the epoch that reached version 1",
    );
    let report = forged(&listed, enrollment, |doc| {
        let id = next_event_id(doc);
        doc["events"]
            .as_array_mut()
            .expect("events")
            .push(version_change(id, (2, "2026 rev B"), (1, "2026 rev A")));
        doc["phase_events"] = serde_json::json!([epoch_phase_event(1, None, (1, "2026 rev A"))]);
    });
    history(
        &report.documents[2].findings,
        "recorded under the original pin, but names version 1 rather than version 2",
    );

    // The genuine packet's history is coherent.
    assert!(export_verify::verify_archive(&original).verified());
}

const TIMELINE_START: i64 = EXPORTED_AT - 100;

async fn published_timeline_versions(fx: &Fixture) -> Vec<i64> {
    let mut content = program("Invented Pin Timeline Program");
    let program_id = programs::create_program(&fx.pool, fx.admin_id, &content.name)
        .await
        .expect("create")
        .expect("accepted");
    let mut versions = Vec::new();
    for label in ["2026 rev A", "2026 rev B", "2026 rev C"] {
        content.label = label.to_owned();
        let id = programs::create_version(&fx.pool, fx.admin_id, program_id, &content)
            .await
            .expect("version")
            .expect("accepted");
        programs::publish_version(&fx.pool, fx.admin_id, id)
            .await
            .expect("publish")
            .expect("accepted");
        versions.push(id);
    }
    versions
}

/// The real producer exports constrained, append-only fixture rows. Explicit
/// timestamps make boundary cases deterministic without sleeping or rewriting
/// retained history. The pin visits 1 -> 2 -> 3 -> 1, including signoffs on
/// both sides of each change in the same second.
async fn timeline_packet(change_offsets: [i64; 3]) -> Vec<u8> {
    let fx = Fixture::new().await;
    let versions = published_timeline_versions(&fx).await;
    let mut tx = storage::write_tx(&fx.pool)
        .await
        .expect("fixture transaction");
    let enrollment_id = sqlx::query(
        "INSERT INTO enrollment (user_id, program_version_id, enrolled_at, enrolled_by)
         VALUES (?1, ?2, ?3, ?1)",
    )
    .bind(fx.admin_id)
    .bind(versions[0])
    .bind(TIMELINE_START - 1)
    .execute(&mut *tx)
    .await
    .expect("enroll fixture")
    .last_insert_rowid();
    let changes = change_offsets.map(|offset| TIMELINE_START + offset);
    let mut epoch_id = None;
    for (index, version) in [versions[0], versions[1], versions[2], versions[0]]
        .into_iter()
        .enumerate()
    {
        let opened = if index == 0 {
            TIMELINE_START
        } else {
            changes[index - 1]
        };
        if index > 0 {
            epoch_id = Some(
                sqlx::query(
                    "INSERT INTO enrollment_event
                 (enrollment_id, kind, occurred_at, actor_user_id, reason,
                  from_program_version_id, to_program_version_id)
                 SELECT id, 'version_change', ?2, ?3, 'Invented timeline change.',
                        program_version_id, ?4 FROM enrollment WHERE id = ?1",
                )
                .bind(enrollment_id)
                .bind(opened)
                .bind(fx.admin_id)
                .bind(version)
                .execute(&mut *tx)
                .await
                .expect("append version change")
                .last_insert_rowid(),
            );
            sqlx::query("UPDATE enrollment SET program_version_id = ?2 WHERE id = ?1")
                .bind(enrollment_id)
                .bind(version)
                .execute(&mut *tx)
                .await
                .expect("repoint through event");
        }
        sqlx::query(
            "INSERT INTO phase_event
             (enrollment_id, kind, to_phase_id, effective_at, recorded_at,
              actor_user_id, reason, version_change_event_id)
             SELECT ?1, 'advance', id, ?2, ?2, ?3, '', ?4
             FROM phase WHERE program_version_id = ?5",
        )
        .bind(enrollment_id)
        .bind(opened)
        .bind(fx.admin_id)
        .bind(epoch_id)
        .bind(version)
        .execute(&mut *tx)
        .await
        .expect("append phase entry");
        let closed = changes.get(index).copied().unwrap_or(TIMELINE_START + 40);
        for signed_at in [opened, closed] {
            sqlx::query(
                "INSERT INTO task_signoff
                 (enrollment_id, task_id, kind, reason, signed_by, signed_by_display_name, signed_at)
                 SELECT ?1, id, 'observed', 'Invented repeated observation.', ?2, 'Avery Admin', ?3
                 FROM task WHERE program_version_id = ?4",
            )
            .bind(enrollment_id).bind(fx.admin_id).bind(signed_at).bind(version)
            .execute(&mut *tx).await.expect("append signoff under current pin");
        }
    }
    tx.commit().await.expect("commit fixture");
    let packet = pack(&fx, fx.admin_id, enrollment_id).await;
    fx.pool.close().await;
    packet
}

fn only_timeline_findings(
    report: &export_verify::ArchiveReport,
    kind: DocumentKind,
    expected: &str,
) {
    assert!(!report.verified(), "forgery verified");
    assert!(report.findings.is_empty(), "{report:?}");
    assert!(
        report.units.iter().all(|unit| unit.findings.is_empty()),
        "{report:?}"
    );
    let document = report
        .documents
        .iter()
        .find(|doc| doc.path == kind.path())
        .expect("document");
    assert!(!document.findings.is_empty(), "{report:?}");
    assert!(
        document.findings.iter().all(|finding| matches!(finding,
            Finding::DocumentPinHistory { detail, .. } if detail.contains(expected)
        )),
        "{report:?}"
    );
    assert!(
        report
            .documents
            .iter()
            .filter(|doc| doc.path != kind.path())
            .all(|doc| doc.findings.is_empty()),
        "{report:?}"
    );
}

#[tokio::test]
async fn signoffs_follow_the_pin_at_the_signed_second() {
    let packet = timeline_packet([10, 20, 30]).await;
    assert!(export_verify::verify_archive(&packet).verified());
    let listed = entries(&packet);
    // Relabel both original rows as the later version's actual task so
    // task-description consistency holds, including the return to version 1.
    // Version 2 is genuinely pinned later; it cannot explain the earlier act.
    let report = forged(&listed, DocumentKind::Signoffs, |doc| {
        let later_task = doc[2]["task_id"].clone();
        for row in &mut doc.as_array_mut().expect("rows")[..2] {
            row["task_id"] = later_task.clone();
            row["program_version"] =
                serde_json::json!({"version_number": 2, "label": "2026 rev B"});
        }
    });
    only_timeline_findings(&report, DocumentKind::Signoffs, "not pinned at signed_at");
    // Version 1 is revisited, but its two epochs cannot cover the gap between.
    for (row, offset) in [(0, 11), (2, 9), (3, 21), (4, 19), (5, 31), (6, 29)] {
        let report = forged(&listed, DocumentKind::Signoffs, |doc| {
            doc[row]["signed_at"] = serde_json::json!(TIMELINE_START + offset);
        });
        only_timeline_findings(&report, DocumentKind::Signoffs, "not pinned at signed_at");
    }
}

#[tokio::test]
async fn phase_events_stay_within_their_named_epoch() {
    let packet = timeline_packet([10, 20, 30]).await;
    let listed = entries(&packet);
    // Keep effective order and effective <= recorded, so the failure is the
    // epoch boundary, even when both instants predate the opening.
    for recorded in [TIMELINE_START + 9, TIMELINE_START + 10] {
        let report = forged(&listed, DocumentKind::Enrollment, |doc| {
            doc["phase_events"][1]["effective_at"] = serde_json::json!(TIMELINE_START + 9);
            doc["phase_events"][1]["recorded_at"] = serde_json::json!(recorded);
        });
        only_timeline_findings(&report, DocumentKind::Enrollment, "before its epoch opened");
    }
    // A backdated effective instant cannot rescue a record made after closing,
    // under either the original pin or a subsequently opened epoch.
    for (row, offset) in [(0, 11), (1, 21), (2, 31)] {
        let report = forged(&listed, DocumentKind::Enrollment, |doc| {
            doc["phase_events"][row]["recorded_at"] = serde_json::json!(TIMELINE_START + offset);
        });
        only_timeline_findings(&report, DocumentKind::Enrollment, "after its epoch closed");
    }
    // Inclusive closing boundaries and backdating inside an epoch are valid.
    let report = forged(&listed, DocumentKind::Enrollment, |doc| {
        for (row, offset) in [(0, 10), (1, 20), (2, 30), (3, 40)] {
            doc["phase_events"][row]["recorded_at"] = serde_json::json!(TIMELINE_START + offset);
        }
    });
    assert!(report.verified(), "{report:?}");
}

#[tokio::test]
async fn multiple_changes_and_acts_in_one_second_verify() {
    let packet = timeline_packet([10, 10, 30]).await;
    let report = export_verify::verify_archive(&packet);
    assert!(report.verified(), "{report:?}");
    let listed = entries(&packet);
    // The intermediate pin exists only in the shared second. Neither adjacent
    // second can borrow that version, while before/intermediate/after pins all
    // appear legitimately in the unmodified packet at the boundary.
    for offset in [9, 11] {
        let report = forged(&listed, DocumentKind::Signoffs, |doc| {
            doc[2]["signed_at"] = serde_json::json!(TIMELINE_START + offset);
        });
        only_timeline_findings(&report, DocumentKind::Signoffs, "not pinned at signed_at");
    }
}

#[tokio::test]
async fn backwards_version_change_times_are_an_incoherent_timeline() {
    let packet = timeline_packet([10, 20, 30]).await;
    let listed = entries(&packet);
    let report = forged(&listed, DocumentKind::Enrollment, |doc| {
        doc["events"][1]["occurred_at"] = serde_json::json!(TIMELINE_START + 9);
        // Isolate the incoherent history from individual event-boundary checks.
        doc["phase_events"] = serde_json::json!([]);
    });
    assert!(report.documents[2].findings.iter().any(|finding| matches!(finding,
        Finding::DocumentPinHistory { detail, .. } if detail.contains("after the next version change")
    )), "{report:?}");
}

#[tokio::test]
async fn service_recorded_pin_changes_and_acts_verify() {
    let fx = Fixture::new().await;
    let versions = published_timeline_versions(&fx).await;
    let enrollment_id = enrollments::enroll(&fx.pool, fx.admin_id, versions[0], fx.admin_id)
        .await
        .expect("enroll")
        .expect("accepted");
    for (index, version) in [versions[0], versions[1], versions[2], versions[0]]
        .into_iter()
        .enumerate()
    {
        if index > 0 {
            lifecycle::record_enrollment_event(
                &fx.pool,
                fx.admin_id,
                enrollment_id,
                EnrollmentEventKind::VersionChange,
                "Invented configuration update.",
                Some(version),
            )
            .await
            .expect("change pin")
            .expect("accepted");
        }
        let phase: i64 = sqlx::query_scalar("SELECT id FROM phase WHERE program_version_id = ?1")
            .bind(version)
            .fetch_one(&fx.pool)
            .await
            .expect("phase");
        lifecycle::record_phase_event(
            &fx.pool,
            fx.admin_id,
            enrollment_id,
            PhaseEventKind::Advance,
            Some(phase),
            None,
            "",
        )
        .await
        .expect("phase entry")
        .expect("accepted");
        let task: i64 = sqlx::query_scalar("SELECT id FROM task WHERE program_version_id = ?1")
            .bind(version)
            .fetch_one(&fx.pool)
            .await
            .expect("task");
        task_signoffs::record(
            &fx.pool,
            fx.admin_id,
            enrollment_id,
            task,
            SignoffKind::Observed,
            "Invented observation after configuration update.",
        )
        .await
        .expect("signoff")
        .expect("accepted");
    }
    let packet = pack(&fx, fx.admin_id, enrollment_id).await;
    let report = export_verify::verify_archive(&packet);
    assert!(report.verified(), "{report:?}");
    fx.pool.close().await;
}
