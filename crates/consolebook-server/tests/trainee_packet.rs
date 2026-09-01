//! Milestone 5 slice 2: trainee packets — everything retained about one
//! enrollment as one archive: the record export's units byte for byte,
//! plus typed documents for acknowledgments, amendments, the signoff
//! history, and the enrollment's own history; deterministic; verified
//! from the archive alone with named findings; produced under the read
//! rules that exist; delivered over the API and checked by the CLI.
//! Every fixture is invented.

use std::io::{Cursor, Read, Write};
use std::time::Duration;

use axum::body::Body;
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE, COOKIE, SET_COOKIE};
use axum::http::{HeaderMap, Request, StatusCode};
use consolebook_server::acknowledgments::{self, AckKind, TraineeAckKind};
use consolebook_server::capabilities::RoleBundle;
use consolebook_server::draft_content::{self, DraftContent, NarrativeEntry, RatingEntry};
use consolebook_server::export_verify::{self, ArchiveKind, Finding};
use consolebook_server::lifecycle::{self, EnrollmentEventKind, EnrollmentStatus};
use consolebook_server::programs::{
    self, AnchorDef, CompetencyDef, FormCompetencyDef, FormDef, NarrativeDef, PolicyDef,
    RecordType, ScaleDef, ScaleKind, TaskDef, VersionContent,
};
use consolebook_server::record_export::{self, ARCHIVE_MANIFEST_PATH, Scope};
use consolebook_server::task_signoffs::{self, SignoffKind};
use consolebook_server::trainee_packet::{
    self, AcknowledgmentDoc, Actor, AmendmentDoc, DocumentKind, EnrollmentDocument, PACKET_FORMAT,
    PACKET_FORMAT_VERSION, PacketManifest, PacketRefusal, SignoffDoc,
};
use consolebook_server::training_sessions::{self, Disposition, SessionInput};
use consolebook_server::{
    amendments, assignments, canonical, data_dir::DataDir, enrollments, evaluation_drafts,
    finalization, setup, storage, users,
};
use http_body_util::BodyExt;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use tower::ServiceExt;
use zip::write::{SimpleFileOptions, ZipWriter};

const PASSWORD: &str = "invented-passphrase-1";

/// 2026-09-01T19:00:00Z.
const EXPORTED_AT: i64 = 1_788_289_200;

const OPEN_POLICY: PolicyDef = PolicyDef {
    review_approved: false,
    required_narratives: false,
    ratings_complete: false,
};

struct Fixture {
    _tmp: tempfile::TempDir,
    pool: sqlx::SqlitePool,
    admin_id: i64,
}

impl Fixture {
    async fn new() -> Self {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let data_dir = DataDir::new(tmp.path().join("data"));
        data_dir.ensure_layout().expect("create layout");
        let pool = storage::open(&data_dir.database()).await.expect("open");
        let code = setup::issue_setup_code(&pool)
            .await
            .expect("issue")
            .expect("uninitialized")
            .0;
        let admin_id = setup::initialize(
            &pool,
            &code.raw,
            "Example County Communications",
            "avery.admin",
            "Avery Admin",
            PASSWORD,
        )
        .await
        .expect("initialize")
        .expect("accepted");
        Self {
            _tmp: tmp,
            pool,
            admin_id,
        }
    }

    fn app(&self) -> axum::Router {
        consolebook_server::http::router(consolebook_server::http::AppState {
            pool: self.pool.clone(),
        })
    }

    async fn login(&self, username: &str) -> String {
        let response = self
            .app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "username": username, "password": PASSWORD })
                            .to_string(),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK, "login {username}");
        let cookie = response
            .headers()
            .get(SET_COOKIE)
            .expect("cookie")
            .to_str()
            .expect("ascii");
        let (pair, _) = cookie.split_once(';').expect("attrs");
        pair.split_once('=').expect("pair").1.to_string()
    }

    async fn user_with_role(&self, username: &str, display_name: &str, role: RoleBundle) -> i64 {
        let created = users::create_with_reset_code(
            &self.pool,
            self.admin_id,
            username,
            display_name,
            "",
            "",
            role,
        )
        .await
        .expect("create")
        .expect("accepted");
        assert_eq!(
            users::use_reset_code(&self.pool, username, &created.reset_code.raw, PASSWORD)
                .await
                .expect("reset"),
            users::ResetOutcome::Done
        );
        created.id
    }

    async fn audit_count(&self, kind: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_event WHERE kind = ?1")
            .bind(kind)
            .fetch_one(&self.pool)
            .await
            .expect("count")
    }
}

async fn raw_get(app: axum::Router, uri: &str, cookie: &str) -> (StatusCode, HeaderMap, Vec<u8>) {
    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(
                    COOKIE,
                    format!("{}={}", consolebook_server::http::SESSION_COOKIE, cookie),
                )
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
        .to_vec();
    (status, headers, bytes)
}

/// Invented single-form program with one task to sign off and every
/// completion rule off.
fn program(name: &str) -> VersionContent {
    VersionContent {
        name: name.to_owned(),
        label: "2026 rev A".to_owned(),
        description: "Invented program for packet tests.".to_owned(),
        phases: Vec::new(),
        phase_transitions: Vec::new(),
        competencies: vec![CompetencyDef {
            category: "Call processing".to_owned(),
            name: "Emergency Call Interrogation".to_owned(),
            description: "Obtains and verifies location, callback, and nature.".to_owned(),
            tasks: vec![TaskDef {
                prompt: "Processes an invented structure-fire call.".to_owned(),
                citations: Vec::new(),
            }],
            citations: Vec::new(),
        }],
        rating_scales: vec![ScaleDef {
            name: "Standard 1-7".to_owned(),
            kind: ScaleKind::AnchoredNumeric,
            min_value: Some(1),
            max_value: Some(7),
            anchors: vec![AnchorDef {
                value: 4,
                label: "Meets standards".to_owned(),
                definition: "To the invented standard.".to_owned(),
            }],
        }],
        rating_modifiers: Vec::new(),
        evaluation_forms: vec![FormDef {
            record_type: RecordType::DailyReport,
            name: "Daily Observation Report".to_owned(),
            instructions: "Rate observed performance.".to_owned(),
            competencies: vec![FormCompetencyDef {
                competency: "Emergency Call Interrogation".to_owned(),
                rating_scale: "Standard 1-7".to_owned(),
            }],
            narratives: vec![NarrativeDef {
                prompt: "Most acceptable performance.".to_owned(),
                required: false,
            }],
        }],
        citations: Vec::new(),
        finalization_policy: OPEN_POLICY,
    }
}

#[allow(clippy::struct_field_names)]
struct Seeded {
    version_id: i64,
    enrollment_id: i64,
    record_id: i64,
    task_id: i64,
    taylor_id: i64,
    jordan_id: i64,
    casey_id: i64,
}

/// An enrollment with everything a packet carries: a record sealed
/// twice (an acknowledged version 1, then an amendment sealed as
/// version 2), a task signed off and overridden, and a lifecycle event.
#[allow(clippy::too_many_lines)]
async fn seed(fx: &Fixture, suffix: &str) -> Seeded {
    let content = program(&format!("Example County Program {suffix}"));
    let program_id = programs::create_program(&fx.pool, fx.admin_id, &content.name)
        .await
        .expect("create program")
        .expect("accepted");
    let version_id = programs::create_version(&fx.pool, fx.admin_id, program_id, &content)
        .await
        .expect("create version")
        .expect("accepted");
    programs::publish_version(&fx.pool, fx.admin_id, version_id)
        .await
        .expect("publish")
        .expect("accepted");
    let taylor_id = fx
        .user_with_role(
            &format!("taylor.{suffix}"),
            "Taylor Trainee",
            RoleBundle::Trainee,
        )
        .await;
    let jordan_id = fx
        .user_with_role(
            &format!("jordan.{suffix}"),
            "Jordan Trainer",
            RoleBundle::Trainer,
        )
        .await;
    let casey_id = fx
        .user_with_role(
            &format!("casey.{suffix}"),
            "Casey Coordinator",
            RoleBundle::Coordinator,
        )
        .await;
    let enrollment_id = enrollments::enroll(&fx.pool, fx.admin_id, version_id, taylor_id)
        .await
        .expect("call")
        .expect("enrolled");
    assignments::create(&fx.pool, fx.admin_id, enrollment_id, jordan_id)
        .await
        .expect("call")
        .expect("assigned");
    let task_id: i64 =
        sqlx::query_scalar("SELECT id FROM task WHERE program_version_id = ?1 AND prompt = ?2")
            .bind(version_id)
            .bind("Processes an invented structure-fire call.")
            .fetch_one(&fx.pool)
            .await
            .expect("task id");
    let record_id = draft_for(fx, jordan_id, enrollment_id, "2026-06-02").await;
    let s = Seeded {
        version_id,
        enrollment_id,
        record_id,
        task_id,
        taylor_id,
        jordan_id,
        casey_id,
    };
    author_and_seal(fx, &s, 3, "The invented initial entry.").await;
    acknowledgments::acknowledge(
        &fx.pool,
        s.taylor_id,
        s.record_id,
        TraineeAckKind::Acknowledged,
        "",
    )
    .await
    .expect("call")
    .expect("acknowledged");
    amendments::open(
        &fx.pool,
        s.casey_id,
        s.record_id,
        "The invented rating was entered one point low.",
    )
    .await
    .expect("call")
    .expect("opened");
    author_and_seal(fx, &s, 4, "Corrected the invented rating with context.").await;
    task_signoffs::record(
        &fx.pool,
        s.jordan_id,
        s.enrollment_id,
        s.task_id,
        SignoffKind::Observed,
        "",
    )
    .await
    .expect("call")
    .expect("signed");
    task_signoffs::record(
        &fx.pool,
        s.casey_id,
        s.enrollment_id,
        s.task_id,
        SignoffKind::Demonstrated,
        "Demonstrated on the invented live call.",
    )
    .await
    .expect("call")
    .expect("overridden");
    lifecycle::record_enrollment_event(
        &fx.pool,
        s.casey_id,
        s.enrollment_id,
        EnrollmentEventKind::Withdraw,
        "Invented transfer to another center.",
        None,
    )
    .await
    .expect("call")
    .expect("recorded");
    s
}

async fn draft_for(fx: &Fixture, trainer_id: i64, enrollment_id: i64, date: &str) -> i64 {
    let session_id = training_sessions::create(
        &fx.pool,
        trainer_id,
        enrollment_id,
        &SessionInput {
            business_date: date.to_owned(),
            timezone: "America/Chicago".to_owned(),
            local_start: format!("{date}T07:00"),
            local_end: Some(format!("{date}T15:00")),
            disposition: Some(Disposition::Completed),
            phase_id: None,
            trainer_user_ids: Vec::new(),
        },
    )
    .await
    .expect("call")
    .expect("created");
    evaluation_drafts::create(&fx.pool, trainer_id, session_id, None)
        .await
        .expect("call")
        .expect("created")
}

async fn author_and_seal(fx: &Fixture, s: &Seeded, value: i64, text: &str) {
    let workspace = evaluation_drafts::workspace(&fx.pool, s.jordan_id, s.record_id)
        .await
        .expect("call")
        .expect("readable");
    let revision = draft_content::save(
        &fx.pool,
        s.jordan_id,
        s.record_id,
        workspace.detail.revision,
        &DraftContent {
            ratings: vec![RatingEntry {
                form_competency_id: workspace.form.competencies[0].form_competency_id,
                value: Some(value),
                not_observed: false,
                modifier_ids: Vec::new(),
            }],
            narratives: vec![NarrativeEntry {
                form_narrative_id: workspace.form.narratives[0].form_narrative_id,
                text: text.to_owned(),
            }],
        },
    )
    .await
    .expect("call")
    .expect("saved");
    finalization::finalize(&fx.pool, s.casey_id, s.record_id, revision)
        .await
        .expect("call")
        .expect("sealed");
}

async fn pack(fx: &Fixture, actor: i64, enrollment_id: i64) -> Vec<u8> {
    trainee_packet::export_at(&fx.pool, actor, enrollment_id, EXPORTED_AT)
        .await
        .expect("call")
        .expect("packed")
        .bytes
}

async fn refusal(fx: &Fixture, actor: i64, enrollment_id: i64) -> PacketRefusal {
    trainee_packet::export_at(&fx.pool, actor, enrollment_id, EXPORTED_AT)
        .await
        .expect("call")
        .expect_err("refused")
}

fn entries(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut out = Vec::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).expect("entry");
        let mut content = Vec::new();
        file.read_to_end(&mut content).expect("read");
        out.push((file.name().to_owned(), content));
    }
    out
}

fn repack(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, content) in entries {
        writer.start_file(name.as_str(), options).expect("start");
        writer.write_all(content).expect("write");
    }
    writer.finish().expect("finish").into_inner()
}

fn entry<'a>(entries: &'a [(String, Vec<u8>)], name: &str) -> &'a [u8] {
    &entries
        .iter()
        .find(|(entry_name, _)| entry_name == name)
        .unwrap_or_else(|| panic!("entry {name}"))
        .1
}

fn edit_json(bytes: &[u8], edit: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
    let mut value: serde_json::Value = serde_json::from_slice(bytes).expect("json");
    edit(&mut value);
    canonical::canonical_bytes(&value).expect("canonical")
}

/// The packet with one document's bytes replaced and, when asked, the
/// manifest's hash brought along — a forgery whose only tell is what
/// the document says.
fn with_document(
    listed: &[(String, Vec<u8>)],
    kind: DocumentKind,
    bytes: &[u8],
    rehash: bool,
) -> Vec<u8> {
    let path = kind.path();
    let sha256 = canonical::content_hash_hex(bytes);
    let entries: Vec<(String, Vec<u8>)> = listed
        .iter()
        .map(|(name, content)| {
            if *name == path {
                (name.clone(), bytes.to_vec())
            } else if name == ARCHIVE_MANIFEST_PATH && rehash {
                let edited = edit_json(content, |manifest| {
                    for document in manifest["documents"].as_array_mut().expect("documents") {
                        if document["path"] == path {
                            document["sha256"] = serde_json::Value::String(sha256.clone());
                        }
                    }
                });
                (name.clone(), edited)
            } else {
                (name.clone(), content.clone())
            }
        })
        .collect();
    repack(&entries)
}

fn with_manifest(
    listed: &[(String, Vec<u8>)],
    edit: impl FnOnce(&mut serde_json::Value),
) -> Vec<u8> {
    let mut edit = Some(edit);
    let entries: Vec<(String, Vec<u8>)> = listed
        .iter()
        .map(|(name, content)| {
            if name == ARCHIVE_MANIFEST_PATH {
                (
                    name.clone(),
                    edit_json(content, edit.take().expect("one edit")),
                )
            } else {
                (name.clone(), content.clone())
            }
        })
        .collect();
    repack(&entries)
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn packet_carries_everything_retained() {
    let fx = Fixture::new().await;
    let s = seed(&fx, "packet").await;
    let installation_id = storage::installation_id(&fx.pool).await.expect("id");
    let packet = trainee_packet::export_at(&fx.pool, s.taylor_id, s.enrollment_id, EXPORTED_AT)
        .await
        .expect("call")
        .expect("packed");
    assert_eq!(
        packet.file_name,
        format!(
            "consolebook-packet-enrollment-{}-20260901T190000Z.zip",
            s.enrollment_id
        )
    );
    assert_eq!(packet.unit_count, 2);
    let listed = entries(&packet.bytes);
    let names: Vec<&str> = listed.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            ARCHIVE_MANIFEST_PATH.to_owned(),
            format!("records/{}/v1/record.json", s.record_id),
            format!("records/{}/v1/manifest.json", s.record_id),
            format!("records/{}/v2/record.json", s.record_id),
            format!("records/{}/v2/manifest.json", s.record_id),
            "packet/acknowledgments.json".to_owned(),
            "packet/amendments.json".to_owned(),
            "packet/enrollment.json".to_owned(),
            "packet/signoffs.json".to_owned(),
        ]
    );

    // The units are the record export's units, byte for byte.
    let export = record_export::export_at(
        &fx.pool,
        s.casey_id,
        Scope::Enrollment {
            enrollment_id: s.enrollment_id,
        },
        EXPORTED_AT,
    )
    .await
    .expect("call")
    .expect("exported");
    for (name, content) in entries(&export.bytes) {
        if name.starts_with("records/") {
            assert_eq!(entry(&listed, &name), content.as_slice(), "{name}");
        }
    }

    // The manifest names the enrollment, its trainee and program as
    // presented, the same unit list, and every document with its hash.
    let manifest: PacketManifest =
        serde_json::from_slice(entry(&listed, ARCHIVE_MANIFEST_PATH)).expect("manifest");
    assert_eq!(manifest.format, PACKET_FORMAT);
    assert_eq!(manifest.format_version, PACKET_FORMAT_VERSION);
    assert_eq!(manifest.installation_id, installation_id);
    assert_eq!(manifest.exported_at, EXPORTED_AT);
    assert_eq!(manifest.enrollment.id, s.enrollment_id);
    assert_eq!(manifest.enrollment.trainee.id, s.taylor_id);
    assert_eq!(manifest.enrollment.trainee.display_name, "Taylor Trainee");
    assert_eq!(
        manifest.enrollment.program.name,
        "Example County Program packet"
    );
    assert_eq!(manifest.enrollment.program.version_number, 1);
    assert_eq!(manifest.enrollment.program.label, "2026 rev A");
    let export_manifest: serde_json::Value =
        serde_json::from_slice(entry(&entries(&export.bytes), ARCHIVE_MANIFEST_PATH))
            .expect("export manifest");
    assert_eq!(
        serde_json::to_value(&manifest.units).expect("units"),
        export_manifest["units"]
    );
    let kinds: Vec<DocumentKind> = manifest.documents.iter().map(|doc| doc.kind).collect();
    assert_eq!(
        kinds,
        [
            DocumentKind::Acknowledgments,
            DocumentKind::Amendments,
            DocumentKind::Enrollment,
            DocumentKind::Signoffs,
        ]
    );
    for document in &manifest.documents {
        assert_eq!(document.path, document.kind.path());
        assert_eq!(
            canonical::content_hash_hex(entry(&listed, &document.path)),
            document.sha256
        );
    }

    // Every document carries the stored rows.
    let acks: Vec<AcknowledgmentDoc> =
        serde_json::from_slice(entry(&listed, "packet/acknowledgments.json")).expect("acks");
    assert_eq!(acks.len(), 1);
    assert_eq!(
        (acks[0].record_id, acks[0].version_number, acks[0].kind),
        (s.record_id, 1, AckKind::Acknowledged)
    );
    assert_eq!(
        acks[0].user,
        Actor {
            id: s.taylor_id,
            display_name: "Taylor Trainee".to_owned()
        }
    );
    assert_eq!(acks[0].recorded_by, acks[0].user, "the trainee's own act");
    let amended: Vec<AmendmentDoc> =
        serde_json::from_slice(entry(&listed, "packet/amendments.json")).expect("amendments");
    assert_eq!(amended.len(), 1);
    assert_eq!(amended[0].record_id, s.record_id);
    assert_eq!(amended[0].predecessor_version_number, 1);
    assert_eq!(amended[0].successor_version_number, Some(2));
    assert_eq!(
        amended[0].reason,
        "The invented rating was entered one point low."
    );
    assert_eq!(
        amended[0].opened_by,
        Actor {
            id: s.casey_id,
            display_name: "Casey Coordinator".to_owned()
        }
    );
    let signoffs: Vec<SignoffDoc> =
        serde_json::from_slice(entry(&listed, "packet/signoffs.json")).expect("signoffs");
    assert_eq!(signoffs.len(), 2, "first signoff and override alike");
    assert_eq!(signoffs[0].task_id, s.task_id);
    assert_eq!(signoffs[0].kind, SignoffKind::Observed);
    assert_eq!(
        signoffs[0].signed_by,
        Actor {
            id: s.jordan_id,
            display_name: "Jordan Trainer".to_owned()
        }
    );
    assert_eq!(
        signoffs[0].prompt,
        "Processes an invented structure-fire call."
    );
    assert_eq!(signoffs[0].competency_name, "Emergency Call Interrogation");
    assert_eq!(signoffs[1].kind, SignoffKind::Demonstrated);
    assert_eq!(
        signoffs[1].reason,
        "Demonstrated on the invented live call."
    );
    assert_eq!(
        signoffs[1].signed_by,
        Actor {
            id: s.casey_id,
            display_name: "Casey Coordinator".to_owned()
        }
    );
    assert!(
        signoffs[0].signoff_id < signoffs[1].signoff_id,
        "recorded order is ascending signoff_id"
    );
    let enrollment: EnrollmentDocument =
        serde_json::from_slice(entry(&listed, "packet/enrollment.json")).expect("enrollment");
    assert_eq!(enrollment.enrollment_id, s.enrollment_id);
    assert!(enrollment.enrolled_at > 0);
    assert_eq!(enrollment.events.len(), 1);
    assert_eq!(enrollment.events[0].kind, EnrollmentEventKind::Withdraw);
    assert_eq!(
        enrollment.events[0].reason,
        "Invented transfer to another center."
    );
    assert_eq!(
        enrollment.events[0].actor,
        Some(Actor {
            id: s.casey_id,
            display_name: "Casey Coordinator".to_owned()
        })
    );
    assert!(enrollment.phase_events.is_empty());

    // Verified from the archive alone, as a packet.
    let report = export_verify::verify_archive(&packet.bytes);
    assert!(report.verified(), "{report:?}");
    assert_eq!(report.kind, Some(ArchiveKind::TraineePacket));
    assert_eq!(report.enrollment_id, Some(s.enrollment_id));
    assert_eq!(report.units.len(), 2);
    assert_eq!(report.documents.len(), 4);
    assert!(report.scope.is_none());

    // Deterministic, and audited with the trainee as subject.
    assert_eq!(pack(&fx, s.taylor_id, s.enrollment_id).await, packet.bytes);
    assert_eq!(fx.audit_count("trainee_packet_exported").await, 2);
    let subjects: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_event
         WHERE kind = 'trainee_packet_exported' AND subject_kind = 'enrollment'
           AND subject_id = ?1 AND subject_user_id = ?2",
    )
    .bind(s.enrollment_id)
    .bind(s.taylor_id)
    .fetch_one(&fx.pool)
    .await
    .expect("count");
    assert_eq!(subjects, 2);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn packet_verification_names_findings() {
    let fx = Fixture::new().await;
    let s = seed(&fx, "findings").await;
    let original = pack(&fx, s.casey_id, s.enrollment_id).await;
    let listed = entries(&original);
    let acks_path = DocumentKind::Acknowledgments.path();

    // A document altered without its hash: the hash objects. Altered
    // with the hash brought along: only what it says can object.
    let altered = edit_json(entry(&listed, &acks_path), |acks| {
        acks[0]["record_id"] = serde_json::json!(9999);
    });
    let report = export_verify::verify_archive(&with_document(
        &listed,
        DocumentKind::Acknowledgments,
        &altered,
        false,
    ));
    assert!(!report.verified());
    assert!(
        report.documents[0]
            .findings
            .contains(&Finding::DocumentHashMismatch {
                path: acks_path.clone()
            }),
        "{report:?}"
    );
    let report = export_verify::verify_archive(&with_document(
        &listed,
        DocumentKind::Acknowledgments,
        &altered,
        true,
    ));
    assert!(!report.verified());
    assert_eq!(report.documents[0].findings.len(), 1, "{report:?}");
    assert!(
        matches!(
            &report.documents[0].findings[0],
            Finding::DocumentReference { path, detail }
                if *path == acks_path && detail.contains("record 9999 version 1")
        ),
        "{report:?}"
    );
    assert!(report.units.iter().all(export_verify::UnitReport::verified));

    // An amendment whose successor the packet does not carry.
    let dangling = edit_json(
        entry(&listed, &DocumentKind::Amendments.path()),
        |amendments| {
            amendments[0]["successor_version_number"] = serde_json::json!(7);
        },
    );
    let report = export_verify::verify_archive(&with_document(
        &listed,
        DocumentKind::Amendments,
        &dangling,
        true,
    ));
    assert!(
        matches!(
            &report.documents[1].findings[..],
            [Finding::DocumentReference { detail, .. }] if detail.contains("successor")
        ),
        "{report:?}"
    );

    // A document that is not its kind's shape, or not canonical.
    let mistyped = edit_json(entry(&listed, &DocumentKind::Signoffs.path()), |signoffs| {
        signoffs[0]["signed_at"] = serde_json::json!("yesterday");
    });
    let report = export_verify::verify_archive(&with_document(
        &listed,
        DocumentKind::Signoffs,
        &mistyped,
        true,
    ));
    assert!(
        matches!(
            &report.documents[3].findings[..],
            [Finding::DocumentInvalid { .. }]
        ),
        "{report:?}"
    );
    let pretty: serde_json::Value =
        serde_json::from_slice(entry(&listed, &DocumentKind::Enrollment.path())).expect("json");
    let report = export_verify::verify_archive(&with_document(
        &listed,
        DocumentKind::Enrollment,
        &serde_json::to_vec_pretty(&pretty).expect("pretty"),
        true,
    ));
    assert!(
        matches!(
            &report.documents[2].findings[..],
            [Finding::DocumentInvalid { detail, .. }] if detail == "not canonical JSON"
        ),
        "{report:?}"
    );

    // A discriminator outside its closed set is not the kind's shape,
    // however plausible the string: the vocabularies are the ones the
    // stored tables constrain, and the verifier knows them by name.
    let shrugged = edit_json(entry(&listed, &acks_path), |acks| {
        acks[0]["kind"] = serde_json::json!("shrugged");
    });
    let report = export_verify::verify_archive(&with_document(
        &listed,
        DocumentKind::Acknowledgments,
        &shrugged,
        true,
    ));
    assert!(
        matches!(
            &report.documents[0].findings[..],
            [Finding::DocumentInvalid { detail, .. }] if detail.contains("shrugged")
        ),
        "{report:?}"
    );
    let maybe = edit_json(entry(&listed, &DocumentKind::Signoffs.path()), |signoffs| {
        signoffs[1]["kind"] = serde_json::json!("maybe");
    });
    let report = export_verify::verify_archive(&with_document(
        &listed,
        DocumentKind::Signoffs,
        &maybe,
        true,
    ));
    assert!(
        matches!(
            &report.documents[3].findings[..],
            [Finding::DocumentInvalid { detail, .. }] if detail.contains("maybe")
        ),
        "{report:?}"
    );
    let vanished = edit_json(entry(&listed, &DocumentKind::Enrollment.path()), |doc| {
        doc["events"][0]["kind"] = serde_json::json!("vanished");
    });
    let report = export_verify::verify_archive(&with_document(
        &listed,
        DocumentKind::Enrollment,
        &vanished,
        true,
    ));
    assert!(
        matches!(
            &report.documents[2].findings[..],
            [Finding::DocumentInvalid { detail, .. }] if detail.contains("vanished")
        ),
        "{report:?}"
    );

    // The enrollment document naming another enrollment.
    let other = edit_json(entry(&listed, &DocumentKind::Enrollment.path()), |doc| {
        doc["enrollment_id"] = serde_json::json!(9999);
    });
    let report = export_verify::verify_archive(&with_document(
        &listed,
        DocumentKind::Enrollment,
        &other,
        true,
    ));
    assert!(
        report.documents[2]
            .findings
            .contains(&Finding::DocumentDisagrees {
                path: DocumentKind::Enrollment.path(),
                member: "enrollment_id"
            }),
        "{report:?}"
    );

    // A document missing from the container, and one dropped from the
    // manifest but left in the container.
    let missing: Vec<(String, Vec<u8>)> = listed
        .iter()
        .filter(|(name, _)| *name != DocumentKind::Signoffs.path())
        .cloned()
        .collect();
    let report = export_verify::verify_archive(&repack(&missing));
    assert!(
        report.documents[3]
            .findings
            .contains(&Finding::MissingEntry {
                path: DocumentKind::Signoffs.path()
            }),
        "{report:?}"
    );
    let report = export_verify::verify_archive(&with_manifest(&listed, |manifest| {
        manifest["documents"]
            .as_array_mut()
            .expect("documents")
            .pop();
    }));
    assert!(!report.verified());
    assert!(
        report.findings.contains(&Finding::DocumentsIncomplete),
        "{report:?}"
    );
    assert!(
        report.findings.contains(&Finding::UnlistedEntry {
            path: DocumentKind::Signoffs.path()
        }),
        "{report:?}"
    );

    // Units are one trainee's: a manifest naming another trainee puts
    // every unit outside scope.
    let report = export_verify::verify_archive(&with_manifest(&listed, |manifest| {
        manifest["enrollment"]["trainee"]["id"] = serde_json::json!(9999);
    }));
    assert!(!report.verified());
    for unit in &report.units {
        assert!(
            unit.findings.contains(&Finding::UnitOutsideScope {
                path: unit.path.clone()
            }),
            "{unit:?}"
        );
    }

    // An unknown packet version is refused by name.
    let report = export_verify::verify_archive(&with_manifest(&listed, |manifest| {
        manifest["format_version"] = serde_json::json!(2);
    }));
    assert!(
        report.findings.contains(&Finding::UnsupportedFormat {
            format: PACKET_FORMAT.to_owned(),
            format_version: 2
        }),
        "{report:?}"
    );
    assert!(report.units.is_empty() && report.documents.is_empty());

    // The untouched original still verifies after all that.
    assert!(export_verify::verify_archive(&original).verified());
}

#[tokio::test]
async fn packets_follow_the_read_rules_and_leave_with_an_empty_history() {
    let fx = Fixture::new().await;
    let s = seed(&fx, "rules").await;
    let robin = fx
        .user_with_role("robin.rules", "Robin Outsider", RoleBundle::Trainer)
        .await;
    let riley = fx
        .user_with_role("riley.rules", "Riley Trainee", RoleBundle::Trainee)
        .await;
    let quinn = fx
        .user_with_role("quinn.rules", "Quinn Records", RoleBundle::Trainee)
        .await;
    // An explicit export_records grant and nothing else that reads.
    sqlx::query(
        "INSERT INTO capability_grant (user_id, capability, granted_at, granted_by)
         VALUES (?1, 'export_records', 0, ?2)",
    )
    .bind(quinn)
    .bind(fx.admin_id)
    .execute(&fx.pool)
    .await
    .expect("grant");

    // The trainee, the assigned trainer, the coordinator, the
    // administrator, and the export_records holder all pack; another
    // trainee and an unassigned trainer are refused.
    for actor in [s.taylor_id, s.jordan_id, s.casey_id, fx.admin_id, quinn] {
        let bytes = pack(&fx, actor, s.enrollment_id).await;
        assert!(export_verify::verify_archive(&bytes).verified());
    }
    assert_eq!(
        refusal(&fx, riley, s.enrollment_id).await,
        PacketRefusal::CapabilityRequired
    );
    assert_eq!(
        refusal(&fx, robin, s.enrollment_id).await,
        PacketRefusal::CapabilityRequired
    );
    assert_eq!(
        refusal(&fx, s.casey_id, 9999).await,
        PacketRefusal::NoSuchEnrollment
    );

    // An enrollment with no finalized version still leaves with its
    // history: no units, every document, verified.
    let empty = enrollments::enroll(&fx.pool, fx.admin_id, s.version_id, riley)
        .await
        .expect("call")
        .expect("enrolled");
    let bytes = pack(&fx, riley, empty).await;
    let report = export_verify::verify_archive(&bytes);
    assert!(report.verified(), "{report:?}");
    assert!(report.units.is_empty());
    assert_eq!(report.documents.len(), 4);
    let listed = entries(&bytes);
    let acks: Vec<AcknowledgmentDoc> =
        serde_json::from_slice(entry(&listed, "packet/acknowledgments.json")).expect("acks");
    assert!(acks.is_empty());
    let enrollment: EnrollmentDocument =
        serde_json::from_slice(entry(&listed, "packet/enrollment.json")).expect("enrollment");
    assert_eq!(enrollment.enrollment_id, empty);
    assert!(enrollment.events.is_empty());

    // The trainee's own list, for the My records page.
    let mine = trainee_packet::own_enrollments(&fx.pool, s.taylor_id)
        .await
        .expect("call")
        .expect("listed");
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].enrollment_id, s.enrollment_id);
    assert_eq!(mine[0].program_name, "Example County Program rules");
    assert_eq!(mine[0].finalized_versions, 2);
    assert_eq!(mine[0].status, EnrollmentStatus::Withdrawn);
    let theirs = trainee_packet::own_enrollments(&fx.pool, riley)
        .await
        .expect("call")
        .expect("listed");
    assert_eq!(theirs.len(), 1);
    assert_eq!(theirs[0].finalized_versions, 0);
    assert_eq!(theirs[0].status, EnrollmentStatus::Active);
    assert_eq!(
        trainee_packet::own_enrollments(&fx.pool, s.casey_id)
            .await
            .expect("call"),
        Err(PacketRefusal::CapabilityRequired)
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn packet_api_and_cli() {
    let fx = Fixture::new().await;
    let s = seed(&fx, "api").await;
    let taylor = fx.login("taylor.api").await;
    let casey = fx.login("casey.api").await;
    fx.user_with_role("robin.api", "Robin Outsider", RoleBundle::Trainer)
        .await;
    let robin = fx.login("robin.api").await;

    let (status, headers, bytes) = raw_get(
        fx.app(),
        &format!("/api/enrollments/{}/packet", s.enrollment_id),
        &taylor,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(CONTENT_TYPE)
            .expect("type")
            .to_str()
            .expect("ascii"),
        "application/zip"
    );
    let disposition = headers
        .get(CONTENT_DISPOSITION)
        .expect("disposition")
        .to_str()
        .expect("ascii");
    assert!(
        disposition.starts_with(&format!(
            "attachment; filename=\"consolebook-packet-enrollment-{}-",
            s.enrollment_id
        )),
        "got: {disposition}"
    );
    let report = export_verify::verify_archive(&bytes);
    assert!(report.verified(), "{report:?}");
    assert_eq!(report.kind, Some(ArchiveKind::TraineePacket));
    let (status, _, body) = raw_get(
        fx.app(),
        &format!("/api/enrollments/{}/packet", s.enrollment_id),
        &robin,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let body: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(body["error"], "capability_required");
    let (status, _, body) = raw_get(fx.app(), "/api/enrollments/9999/packet", &casey).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let body: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(body["error"], "no_such_enrollment");

    let (status, _, body) = raw_get(fx.app(), "/api/my/enrollments", &taylor).await;
    assert_eq!(status, StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(body["enrollments"][0]["enrollment_id"], s.enrollment_id);
    assert_eq!(body["enrollments"][0]["finalized_versions"], 2);
    assert_eq!(body["enrollments"][0]["status"], "withdrawn");
    let (status, _, _) = raw_get(fx.app(), "/api/my/enrollments", &casey).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The CLI reads a packet from the file alone and names its kind,
    // its enrollment, and every document.
    let scratch = tempfile::tempdir().expect("scratch");
    let archive = scratch.path().join("packet.zip");
    std::fs::write(&archive, &bytes).expect("write");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_consolebook-server"))
        .args(["export", "verify"])
        .arg(&archive)
        .output()
        .expect("run verifier");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "stdout: {stdout}");
    assert!(
        stdout.contains("kind          trainee packet"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains(&format!("enrollment    {}", s.enrollment_id)),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("ok    packet/signoffs.json"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains(
            "verified 2 of 2 units and 4 of 4 documents: the packet is consistent with its stated fingerprints"
        ),
        "stdout: {stdout}"
    );
    let listed = entries(&bytes);
    std::fs::write(
        &archive,
        with_document(&listed, DocumentKind::Signoffs, b"[]", false),
    )
    .expect("write");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_consolebook-server"))
        .args(["export", "verify"])
        .arg(&archive)
        .output()
        .expect("run verifier");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "stdout: {stdout}");
    assert!(stdout.contains("NOT VERIFIED"), "stdout: {stdout}");
    assert!(
        stdout.contains("FAIL  packet/signoffs.json"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("does not hash to the manifest's sha256"),
        "stdout: {stdout}"
    );
}

/// A forgery whose document is well-typed and rehashed: only what the
/// rows say, and their order, can object.
fn forged(
    listed: &[(String, Vec<u8>)],
    kind: DocumentKind,
    edit: impl FnOnce(&mut serde_json::Value),
) -> export_verify::ArchiveReport {
    let bytes = edit_json(entry(listed, &kind.path()), edit);
    export_verify::verify_archive(&with_document(listed, kind, &bytes, true))
}

/// One phase event as a forger would write it.
fn phase_event(
    kind: &str,
    from: Option<&str>,
    to: Option<&str>,
    effective_at: i64,
    recorded_at: i64,
    event_id: i64,
) -> serde_json::Value {
    serde_json::json!({
        "actor": null,
        "effective_at": effective_at,
        "event_id": event_id,
        "from_phase": from,
        "kind": kind,
        "reason": "",
        "recorded_at": recorded_at,
        "to_phase": to,
    })
}

/// The verifier holds every document to the order and the cross-member
/// rules the format mandates — the stored tables' own constraints — not
/// only to member types: a forger who keeps every member well-typed and
/// rehashes still cannot reorder, duplicate, or misshape a row.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn documents_keep_their_order_and_shape() {
    let fx = Fixture::new().await;
    let s = seed(&fx, "shape").await;
    let original = pack(&fx, s.casey_id, s.enrollment_id).await;
    let listed = entries(&original);
    let acks = DocumentKind::Acknowledgments;
    let amendments = DocumentKind::Amendments;
    let enrollment = DocumentKind::Enrollment;
    let signoffs = DocumentKind::Signoffs;

    // Order: reversed signoffs, a duplicated acknowledgment, a duplicated
    // amendment. A duplicate is out of order because the orders are total.
    let report = forged(&listed, signoffs, |doc| {
        doc.as_array_mut().expect("array").reverse();
    });
    assert!(
        matches!(
            &report.documents[3].findings[0],
            Finding::DocumentOutOfOrder { detail, .. }
                if detail == "signoff 1 does not follow signoff 0 in ascending signoff_id"
        ),
        "{report:?}"
    );
    let report = forged(&listed, acks, |doc| {
        let first = doc[0].clone();
        doc.as_array_mut().expect("array").push(first);
    });
    assert!(
        matches!(
            &report.documents[0].findings[..],
            [Finding::DocumentOutOfOrder { detail, .. }]
                if detail.contains("ascending (record_id, version_number)")
        ),
        "{report:?}"
    );
    let report = forged(&listed, amendments, |doc| {
        let first = doc[0].clone();
        doc.as_array_mut().expect("array").push(first);
    });
    assert!(
        matches!(
            &report.documents[1].findings[..],
            [Finding::DocumentOutOfOrder { detail, .. }]
                if detail.contains("ascending (record_id, predecessor_version_number)")
        ),
        "{report:?}"
    );

    // Lifecycle events: a withdraw carrying version references, and a
    // version change without them.
    let report = forged(&listed, enrollment, |doc| {
        doc["events"][0]["from_version"] =
            serde_json::json!({"version_number": 1, "label": "Invented v1"});
        doc["events"][0]["to_version"] =
            serde_json::json!({"version_number": 2, "label": "Invented v2"});
    });
    assert!(
        matches!(
            &report.documents[2].findings[..],
            [Finding::DocumentInvalid { detail, .. }]
                if detail.contains("(withdraw) carries version references")
        ),
        "{report:?}"
    );
    let report = forged(&listed, enrollment, |doc| {
        doc["events"][0]["kind"] = serde_json::json!("version_change");
    });
    assert!(
        matches!(
            &report.documents[2].findings[..],
            [Finding::DocumentInvalid { detail, .. }]
                if detail.contains("(version_change) lacks its version references")
        ),
        "{report:?}"
    );

    // Phase events: a pause naming no phase, an advance effective after
    // it was recorded, and two well-shaped events out of effective order.
    let report = forged(&listed, enrollment, |doc| {
        doc["phase_events"] = serde_json::json!([phase_event("pause", None, None, 10, 10, 1)]);
    });
    assert!(
        matches!(
            &report.documents[2].findings[..],
            [Finding::DocumentInvalid { detail, .. }]
                if detail == "phase event 1 (pause) names the wrong phases"
        ),
        "{report:?}"
    );
    let report = forged(&listed, enrollment, |doc| {
        doc["phase_events"] =
            serde_json::json!([phase_event("advance", None, Some("Phase One"), 11, 10, 1)]);
    });
    assert!(
        matches!(
            &report.documents[2].findings[..],
            [Finding::DocumentInvalid { detail, .. }]
                if detail == "phase event 1 is effective after it was recorded"
        ),
        "{report:?}"
    );
    let report = forged(&listed, enrollment, |doc| {
        doc["phase_events"] = serde_json::json!([
            phase_event("advance", None, Some("Phase One"), 20, 20, 2),
            phase_event("advance", Some("Phase One"), Some("Phase Two"), 10, 10, 1),
        ]);
    });
    assert!(
        matches!(
            &report.documents[2].findings[..],
            [Finding::DocumentOutOfOrder { detail, .. }]
                if detail.contains("phase event 1 does not follow phase event 0")
        ),
        "{report:?}"
    );

    // Acknowledgments: a plain acknowledgment with a response, one
    // recorded by someone other than the trainee, one binding another
    // person than the packet's trainee.
    let report = forged(&listed, acks, |doc| {
        doc[0]["response"] = serde_json::json!("Noted.");
    });
    assert!(
        matches!(
            &report.documents[0].findings[..],
            [Finding::DocumentInvalid { detail, .. }] if detail.contains("carries a response")
        ),
        "{report:?}"
    );
    let report = forged(&listed, acks, |doc| {
        doc[0]["recorded_by"]["id"] = serde_json::json!(s.casey_id);
    });
    assert!(
        matches!(
            &report.documents[0].findings[..],
            [Finding::DocumentInvalid { detail, .. }]
                if detail.contains("recorded by someone other than the trainee")
        ),
        "{report:?}"
    );
    let report = forged(&listed, acks, |doc| {
        doc[0]["user"]["id"] = serde_json::json!(s.jordan_id);
        doc[0]["recorded_by"]["id"] = serde_json::json!(s.jordan_id);
    });
    assert_eq!(
        report.documents[0].findings,
        vec![Finding::DocumentDisagrees {
            path: acks.path(),
            member: "user.id"
        }],
        "{report:?}"
    );

    // An amendment without a reason; a signoff override without one.
    let report = forged(&listed, amendments, |doc| {
        doc[0]["reason"] = serde_json::json!(" \u{2003} ");
    });
    assert!(
        matches!(
            &report.documents[1].findings[..],
            [Finding::DocumentInvalid { detail, .. }] if detail.contains("gives no reason")
        ),
        "{report:?}"
    );
    let report = forged(&listed, signoffs, |doc| {
        doc[1]["reason"] = serde_json::json!("");
    });
    assert!(
        matches!(
            &report.documents[3].findings[..],
            [Finding::DocumentInvalid { detail, .. }]
                if detail.contains("overrides task") && detail.contains("without a reason")
        ),
        "{report:?}"
    );

    // The genuine packet passes every one of these.
    assert!(export_verify::verify_archive(&original).verified());
}

/// Authorization runs before the packet's read transaction, so a packet
/// never holds a pooled connection while waiting for another. A pool of
/// one connection is the sharpest proof: a packet that acquired twice
/// at once would wait on itself until the pool gave up.
#[tokio::test]
async fn a_packet_never_holds_a_connection_while_waiting_for_one() {
    let fx = Fixture::new().await;
    let s = seed(&fx, "pool").await;
    let database: String =
        sqlx::query_scalar("SELECT file FROM pragma_database_list WHERE name = 'main'")
            .fetch_one(&fx.pool)
            .await
            .expect("database path");
    let single = SqlitePoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&database)
                .journal_mode(SqliteJournalMode::Wal)
                .foreign_keys(true)
                .busy_timeout(Duration::from_secs(5)),
        )
        .await
        .expect("one-connection pool");
    let packet = trainee_packet::export_at(&single, s.casey_id, s.enrollment_id, EXPORTED_AT)
        .await
        .expect("a packet takes one connection at a time")
        .expect("permitted");
    single.close().await;
    let reference = trainee_packet::export_at(&fx.pool, s.casey_id, s.enrollment_id, EXPORTED_AT)
        .await
        .expect("export")
        .expect("permitted");
    assert_eq!(packet.bytes, reference.bytes);
}
