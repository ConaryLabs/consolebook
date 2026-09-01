//! Milestone 5 slice 1: structured record exports — the stored canonical
//! bytes travel verbatim beside manifests, archives are deterministic,
//! verification needs only the archive and names every finding, scopes
//! follow the read rules that already exist, the API delivers the
//! documented bytes, and the CLI verifier works from the file alone.
//! Every fixture is invented.

use std::io::{Cursor, Read, Write};

use axum::body::Body;
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE, COOKIE, SET_COOKIE};
use axum::http::{HeaderMap, Request, StatusCode};
use consolebook_server::capabilities::RoleBundle;
use consolebook_server::draft_content::{self, DraftContent, NarrativeEntry, RatingEntry};
use consolebook_server::programs::{
    self, AnchorDef, CompetencyDef, FormCompetencyDef, FormDef, NarrativeDef, PolicyDef,
    RecordType, ScaleDef, ScaleKind, VersionContent,
};
use consolebook_server::record_export::{
    self, ARCHIVE_FORMAT, ARCHIVE_MANIFEST_PATH, ArchiveManifest, ExportRefusal, FORMAT_VERSION,
    Finding, PredecessorLink, Scope, UNIT_FORMAT, UnitManifest,
};
use consolebook_server::training_sessions::{self, Disposition, SessionInput};
use consolebook_server::{
    amendments, assignments, canonical, data_dir::DataDir, enrollments, evaluation_drafts,
    finalization, setup, storage, users,
};
use http_body_util::BodyExt;
use tower::ServiceExt;
use zip::write::{SimpleFileOptions, ZipWriter};

const PASSWORD: &str = "invented-passphrase-1";

/// 2026-09-01T19:00:00Z, the instant every deterministic export below
/// is stamped with.
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

    /// The stored row of one version: bytes and both hashes.
    async fn version_row(&self, record_id: i64, number: i64) -> (Vec<u8>, String, String) {
        sqlx::query_as(
            "SELECT canonical_bytes, content_hash, chain_hash FROM evaluation_version
             WHERE evaluation_record_id = ?1 AND version_number = ?2",
        )
        .bind(record_id)
        .bind(number)
        .fetch_one(&self.pool)
        .await
        .expect("version row")
    }

    async fn audit_count(&self, subject_kind: Option<&str>) -> i64 {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_event
             WHERE kind = 'record_exported' AND subject_kind IS ?1",
        )
        .bind(subject_kind)
        .fetch_one(&self.pool)
        .await
        .expect("count")
    }
}

/// A GET whose body is delivered raw, for downloads.
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

/// Invented single-form program with every completion rule off, so
/// records seal from the working copy as soon as they are authored.
fn program(name: &str) -> VersionContent {
    VersionContent {
        name: name.to_owned(),
        label: "2026 rev A".to_owned(),
        description: "Invented program for export tests.".to_owned(),
        phases: Vec::new(),
        phase_transitions: Vec::new(),
        competencies: vec![CompetencyDef {
            category: "Call processing".to_owned(),
            name: "Emergency Call Interrogation".to_owned(),
            description: "Obtains and verifies location, callback, and nature.".to_owned(),
            tasks: Vec::new(),
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
    taylor_id: i64,
    jordan_id: i64,
    casey_id: i64,
}

/// A published program, an enrolled trainee, an assigned trainer, a
/// coordinator, and one daily record sealed twice: version 1, then an
/// amendment sealed as version 2 chained to it.
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
    let record_id = draft_for(fx, jordan_id, enrollment_id, "2026-06-02").await;
    let s = Seeded {
        version_id,
        enrollment_id,
        record_id,
        taylor_id,
        jordan_id,
        casey_id,
    };
    seal_twice(fx, &s).await;
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

async fn seal_twice(fx: &Fixture, s: &Seeded) {
    author_and_seal(fx, s, 3, "The invented initial entry.").await;
    amendments::open(
        &fx.pool,
        s.casey_id,
        s.record_id,
        "The invented rating was entered one point low.",
    )
    .await
    .expect("call")
    .expect("opened");
    author_and_seal(fx, s, 4, "Corrected the invented rating with context.").await;
}

async fn export(fx: &Fixture, actor: i64, scope: Scope) -> Vec<u8> {
    record_export::export_at(&fx.pool, actor, scope, EXPORTED_AT)
        .await
        .expect("call")
        .expect("exported")
        .bytes
}

async fn refusal(fx: &Fixture, actor: i64, scope: Scope) -> ExportRefusal {
    record_export::export_at(&fx.pool, actor, scope, EXPORTED_AT)
        .await
        .expect("call")
        .expect_err("refused")
}

/// Every entry of an archive in container order.
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

/// A well-formed container holding exactly `entries`: the way a
/// tamperer who understands ZIP would repack an export.
fn repack(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, content) in entries {
        writer.start_file(name.as_str(), options).expect("start");
        writer.write_all(content).expect("write");
    }
    writer.finish().expect("finish").into_inner()
}

fn edit_json(bytes: &[u8], edit: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
    let mut value: serde_json::Value = serde_json::from_slice(bytes).expect("json");
    edit(&mut value);
    canonical::canonical_bytes(&value).expect("canonical")
}

fn entry<'a>(entries: &'a [(String, Vec<u8>)], name: &str) -> &'a [u8] {
    &entries
        .iter()
        .find(|(entry_name, _)| entry_name == name)
        .unwrap_or_else(|| panic!("entry {name}"))
        .1
}

fn with_entry(
    entries: &[(String, Vec<u8>)],
    name: &str,
    replace: impl FnOnce(&[u8]) -> Option<Vec<u8>>,
) -> Vec<(String, Vec<u8>)> {
    let mut replace = Some(replace);
    entries
        .iter()
        .filter_map(|(entry_name, content)| {
            if entry_name == name {
                replace.take().expect("one edit")(content)
            } else {
                Some(content.clone())
            }
            .map(|content| (entry_name.clone(), content))
        })
        .collect()
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn exports_carry_stored_bytes_verbatim_and_verify() {
    let fx = Fixture::new().await;
    let s = seed(&fx, "verbatim").await;
    let installation_id = storage::installation_id(&fx.pool).await.expect("id");
    let (v1_bytes, v1_content, v1_chain) = fx.version_row(s.record_id, 1).await;
    let (v2_bytes, v2_content, v2_chain) = fx.version_row(s.record_id, 2).await;

    // One version: the archive lays out exactly the documented entries,
    // the record bytes are the stored bytes, and both manifests carry
    // the stored identity and fingerprints.
    let scope = Scope::Version {
        record_id: s.record_id,
        version_number: 2,
    };
    let exported = record_export::export_at(&fx.pool, s.casey_id, scope, EXPORTED_AT)
        .await
        .expect("call")
        .expect("exported");
    assert_eq!(
        exported.file_name,
        format!("consolebook-record-{}-v2-20260901T190000Z.zip", s.record_id)
    );
    assert_eq!(exported.unit_count, 1);
    let unit_dir = format!("records/{}/v2", s.record_id);
    let listed = entries(&exported.bytes);
    let names: Vec<&str> = listed.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            ARCHIVE_MANIFEST_PATH.to_owned(),
            format!("{unit_dir}/record.json"),
            format!("{unit_dir}/manifest.json"),
        ]
    );
    assert_eq!(
        entry(&listed, &format!("{unit_dir}/record.json")),
        v2_bytes.as_slice(),
        "the record bytes are the stored bytes, not a re-serialization"
    );
    let manifest_bytes = entry(&listed, ARCHIVE_MANIFEST_PATH);
    let manifest: ArchiveManifest = serde_json::from_slice(manifest_bytes).expect("manifest");
    assert_eq!(manifest.format, ARCHIVE_FORMAT);
    assert_eq!(manifest.format_version, FORMAT_VERSION);
    assert_eq!(manifest.installation_id, installation_id);
    assert_eq!(manifest.exported_at, EXPORTED_AT);
    assert_eq!(manifest.scope, scope);
    assert_eq!(manifest.units.len(), 1);
    let unit = &manifest.units[0];
    assert_eq!(unit.path, unit_dir);
    assert_eq!(unit.record_id, s.record_id);
    assert_eq!(unit.version_number, 2);
    assert_eq!(unit.record_schema, canonical::RECORD_SCHEMA);
    assert_eq!(unit.content_hash, v2_content);
    assert_eq!(unit.chain_hash, v2_chain);
    assert_eq!(
        unit.predecessor_content_hash.as_deref(),
        Some(v1_content.as_str())
    );
    // Manifests are canonical JSON: members sorted, compact.
    let manifest_value: serde_json::Value = serde_json::from_slice(manifest_bytes).expect("json");
    assert_eq!(
        canonical::canonical_bytes(&manifest_value).expect("canonical"),
        manifest_bytes
    );
    let unit_manifest: UnitManifest =
        serde_json::from_slice(entry(&listed, &format!("{unit_dir}/manifest.json")))
            .expect("unit manifest");
    assert_eq!(unit_manifest.format, UNIT_FORMAT);
    assert_eq!(unit_manifest.format_version, FORMAT_VERSION);
    assert_eq!(unit_manifest.installation_id, installation_id);
    assert_eq!(unit_manifest.exported_at, EXPORTED_AT);
    assert_eq!(unit_manifest.record_id, s.record_id);
    assert_eq!(unit_manifest.version_number, 2);
    assert_eq!(unit_manifest.content_hash, v2_content);
    assert_eq!(unit_manifest.chain_hash, v2_chain);
    assert_eq!(
        unit_manifest.predecessor_content_hash.as_deref(),
        Some(v1_content.as_str())
    );

    // Verification from the archive alone: the chain hash recomputes
    // from the carried predecessor hash, and the predecessor itself is
    // honestly reported as not in this export.
    let report = record_export::verify_archive(&exported.bytes);
    assert!(report.verified(), "{report:?}");
    assert_eq!(
        report.installation_id.as_deref(),
        Some(installation_id.as_str())
    );
    assert_eq!(report.exported_at, Some(EXPORTED_AT));
    assert_eq!(report.scope, Some(scope));
    assert_eq!(report.units.len(), 1);
    assert_eq!(report.units[0].predecessor, PredecessorLink::NotInExport);

    // Deterministic: the same scope at the same instant is byte-identical.
    let again = export(&fx, s.casey_id, scope).await;
    assert_eq!(again, exported.bytes);
    let later = export(&fx, s.casey_id, scope).await;
    assert_eq!(later, exported.bytes);

    // The whole record: both versions in order, the successor linked to
    // its predecessor within the archive.
    let record_scope = Scope::Record {
        record_id: s.record_id,
    };
    let bytes = export(&fx, s.casey_id, record_scope).await;
    let listed = entries(&bytes);
    assert_eq!(listed.len(), 5);
    assert_eq!(
        entry(&listed, &format!("records/{}/v1/record.json", s.record_id)),
        v1_bytes.as_slice()
    );
    assert_eq!(
        entry(&listed, &format!("records/{}/v2/record.json", s.record_id)),
        v2_bytes.as_slice()
    );
    let manifest: ArchiveManifest =
        serde_json::from_slice(entry(&listed, ARCHIVE_MANIFEST_PATH)).expect("manifest");
    assert_eq!(manifest.scope, record_scope);
    assert_eq!(manifest.units.len(), 2);
    assert_eq!(manifest.units[0].version_number, 1);
    assert_eq!(manifest.units[0].chain_hash, v1_chain);
    assert_eq!(manifest.units[0].predecessor_content_hash, None);
    assert_eq!(manifest.units[1].version_number, 2);
    let report = record_export::verify_archive(&bytes);
    assert!(report.verified(), "{report:?}");
    assert_eq!(report.units[0].predecessor, PredecessorLink::None);
    assert_eq!(report.units[1].predecessor, PredecessorLink::Linked);

    // Every export is audited with its subject and never with content.
    assert_eq!(fx.audit_count(Some("record")).await, 4);
    let bytes = export(
        &fx,
        s.jordan_id,
        Scope::Enrollment {
            enrollment_id: s.enrollment_id,
        },
    )
    .await;
    assert!(record_export::verify_archive(&bytes).verified());
    assert_eq!(fx.audit_count(Some("enrollment")).await, 1);
    let bytes = export(&fx, fx.admin_id, Scope::Installation).await;
    let report = record_export::verify_archive(&bytes);
    assert!(report.verified(), "{report:?}");
    assert_eq!(report.scope, Some(Scope::Installation));
    assert_eq!(report.units.len(), 2);
    assert_eq!(fx.audit_count(None).await, 1);
    let with_content: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_event WHERE kind = 'record_exported'
           AND (subject_user_id IS NULL) != (subject_kind IS NULL)",
    )
    .fetch_one(&fx.pool)
    .await
    .expect("count");
    assert_eq!(with_content, 0, "subject and trainee travel together");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn verification_names_every_finding() {
    let fx = Fixture::new().await;
    let s = seed(&fx, "findings").await;
    let original = export(
        &fx,
        s.casey_id,
        Scope::Record {
            record_id: s.record_id,
        },
    )
    .await;
    let listed = entries(&original);
    let v1 = format!("records/{}/v1", s.record_id);
    let v2 = format!("records/{}/v2", s.record_id);
    let (v1_bytes, _, _) = fx.version_row(s.record_id, 1).await;

    // Not an archive at all.
    let report = record_export::verify_archive(b"not a zip archive");
    assert!(!report.verified());
    assert!(matches!(report.findings[0], Finding::NotAnArchive { .. }));

    // Repacked with altered record content: the content hash no longer
    // covers the bytes, and neither does the chain hash.
    let altered = repack(&with_entry(
        &listed,
        &format!("{v2}/record.json"),
        |bytes| {
            let text = String::from_utf8(bytes.to_vec()).expect("utf-8");
            Some(
                text.replace("Corrected the invented", "Corrected the inveNted")
                    .into_bytes(),
            )
        },
    ));
    let report = record_export::verify_archive(&altered);
    assert!(!report.verified());
    assert!(report.findings.is_empty(), "{report:?}");
    assert!(report.units[0].verified());
    let findings = &report.units[1].findings;
    assert!(
        findings.contains(&Finding::ContentHashMismatch),
        "{findings:?}"
    );
    assert!(
        findings.contains(&Finding::ChainHashMismatch),
        "{findings:?}"
    );
    assert!(
        !findings
            .iter()
            .any(|f| matches!(f, Finding::NotCanonical { .. })),
        "the altered text is still canonical JSON: {findings:?}"
    );

    // A byte flipped inside the container without repacking fails too:
    // the container's own checksum or the content hash catches it.
    let mut flipped = original.clone();
    let needle = b"Corrected the invented";
    let at = flipped
        .windows(needle.len())
        .position(|window| window == needle)
        .expect("record text in the archive");
    flipped[at] ^= 0x20;
    assert!(!record_export::verify_archive(&flipped).verified());

    // The archive manifest claims a different content hash for version
    // 1: the bytes disagree, the unit manifest disagrees, and version
    // 2's predecessor no longer matches the version 1 in the archive.
    let other_hash = "0".repeat(64);
    let swapped = repack(&with_entry(&listed, ARCHIVE_MANIFEST_PATH, |bytes| {
        Some(edit_json(bytes, |manifest| {
            manifest["units"][0]["content_hash"] = serde_json::Value::String(other_hash.clone());
        }))
    }));
    let report = record_export::verify_archive(&swapped);
    assert!(!report.verified());
    assert!(report.findings.is_empty(), "{report:?}");
    let findings = &report.units[0].findings;
    assert!(
        findings.contains(&Finding::ContentHashMismatch),
        "{findings:?}"
    );
    assert!(
        findings.contains(&Finding::UnitManifestDisagrees {
            member: "content_hash"
        }),
        "{findings:?}"
    );
    assert!(
        !findings.contains(&Finding::ChainHashMismatch),
        "the chain hash still covers the bytes: {findings:?}"
    );
    let findings = &report.units[1].findings;
    assert!(
        findings.contains(&Finding::PredecessorMismatch),
        "{findings:?}"
    );
    assert_eq!(report.units[1].predecessor, PredecessorLink::Linked);

    // A unit's record bytes replaced by another version's: every
    // identity member the envelope itself carries disagrees.
    let replaced = repack(&with_entry(&listed, &format!("{v2}/record.json"), |_| {
        Some(v1_bytes.clone())
    }));
    let report = record_export::verify_archive(&replaced);
    let findings = &report.units[1].findings;
    assert!(
        findings.contains(&Finding::ContentHashMismatch),
        "{findings:?}"
    );
    assert!(
        findings.contains(&Finding::EnvelopeDisagrees {
            member: "record.version_number"
        }),
        "{findings:?}"
    );
    assert!(
        findings.contains(&Finding::EnvelopeDisagrees {
            member: "record.predecessor_content_hash"
        }),
        "{findings:?}"
    );

    // Missing and unlisted entries.
    let missing = repack(&with_entry(&listed, &format!("{v1}/record.json"), |_| None));
    let report = record_export::verify_archive(&missing);
    assert!(
        report.units[0].findings.contains(&Finding::MissingEntry {
            path: format!("{v1}/record.json")
        }),
        "{report:?}"
    );
    let mut extra = listed.clone();
    extra.push(("README.txt".to_owned(), b"nothing to see here".to_vec()));
    let report = record_export::verify_archive(&repack(&extra));
    assert!(!report.verified());
    assert!(
        report.findings.contains(&Finding::UnlistedEntry {
            path: "README.txt".to_owned()
        }),
        "{report:?}"
    );
    assert!(report.units.iter().all(record_export::UnitReport::verified));

    // A manifest that parses but is not canonical (pretty-printed) is a
    // finding: the format fixes the bytes, not only the members.
    let pretty = repack(&with_entry(
        &listed,
        &format!("{v2}/manifest.json"),
        |bytes| {
            let value: serde_json::Value = serde_json::from_slice(bytes).expect("json");
            Some(serde_json::to_vec_pretty(&value).expect("pretty"))
        },
    ));
    let report = record_export::verify_archive(&pretty);
    assert!(
        report.units[1]
            .findings
            .contains(&Finding::ManifestNotCanonical {
                path: format!("{v2}/manifest.json")
            }),
        "{report:?}"
    );

    // An unknown format version is refused by name, not guessed at.
    let future = repack(&with_entry(&listed, ARCHIVE_MANIFEST_PATH, |bytes| {
        Some(edit_json(bytes, |manifest| {
            manifest["format_version"] = serde_json::Value::from(2);
        }))
    }));
    let report = record_export::verify_archive(&future);
    assert!(!report.verified());
    assert!(
        report.findings.contains(&Finding::UnsupportedFormat {
            format: ARCHIVE_FORMAT.to_owned(),
            format_version: 2
        }),
        "{report:?}"
    );
    assert!(report.units.is_empty());

    // Units out of order, with paths that do not derive from identity.
    let reordered = repack(&with_entry(&listed, ARCHIVE_MANIFEST_PATH, |bytes| {
        Some(edit_json(bytes, |manifest| {
            let units = manifest["units"].as_array_mut().expect("units");
            units.swap(0, 1);
        }))
    }));
    let report = record_export::verify_archive(&reordered);
    assert!(
        report.findings.contains(&Finding::UnitsOutOfOrder),
        "{report:?}"
    );
    let renamed = repack(&with_entry(&listed, ARCHIVE_MANIFEST_PATH, |bytes| {
        Some(edit_json(bytes, |manifest| {
            manifest["units"][1]["path"] = serde_json::Value::String("records/elsewhere".into());
        }))
    }));
    let report = record_export::verify_archive(&renamed);
    assert!(
        report.findings.contains(&Finding::UnitPathUnexpected {
            path: "records/elsewhere".to_owned(),
            expected: v2.clone()
        }),
        "{report:?}"
    );

    // Lineage shape: a version 2 claiming no predecessor.
    let orphan = repack(&with_entry(&listed, ARCHIVE_MANIFEST_PATH, |bytes| {
        Some(edit_json(bytes, |manifest| {
            manifest["units"][1]["predecessor_content_hash"] = serde_json::Value::Null;
        }))
    }));
    let report = record_export::verify_archive(&orphan);
    let findings = &report.units[1].findings;
    assert!(findings.contains(&Finding::LineageShape), "{findings:?}");
    assert!(
        findings.contains(&Finding::ChainHashMismatch),
        "{findings:?}"
    );
    assert_eq!(report.units[1].predecessor, PredecessorLink::None);

    // The untouched original still verifies after all that.
    assert!(record_export::verify_archive(&original).verified());
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn scopes_follow_the_read_rules() {
    let fx = Fixture::new().await;
    let s = seed(&fx, "scopes").await;
    let outsider = fx
        .user_with_role("robin.scopes", "Robin Outsider", RoleBundle::Trainer)
        .await;
    let version = Scope::Version {
        record_id: s.record_id,
        version_number: 1,
    };
    let record = Scope::Record {
        record_id: s.record_id,
    };
    let enrollment = Scope::Enrollment {
        enrollment_id: s.enrollment_id,
    };

    // A version or record exports for whoever may read the record: the
    // assigned trainer, the coordinator, and the trainee on their own
    // finalized record — every retained version included. A trainer
    // outside the scope is refused.
    for actor in [s.jordan_id, s.casey_id, s.taylor_id] {
        let bytes = export(&fx, actor, record).await;
        let report = record_export::verify_archive(&bytes);
        assert!(report.verified());
        assert_eq!(report.units.len(), 2, "actor {actor} sees both versions");
        export(&fx, actor, version).await;
    }
    assert_eq!(
        refusal(&fx, outsider, version).await,
        ExportRefusal::CapabilityRequired
    );
    assert_eq!(
        refusal(&fx, outsider, record).await,
        ExportRefusal::CapabilityRequired
    );

    // An enrollment exports for whoever may read its training history;
    // the trainee's own-record grant is not that.
    export(&fx, s.jordan_id, enrollment).await;
    export(&fx, s.casey_id, enrollment).await;
    export(&fx, fx.admin_id, enrollment).await;
    assert_eq!(
        refusal(&fx, outsider, enrollment).await,
        ExportRefusal::CapabilityRequired
    );
    assert_eq!(
        refusal(&fx, s.taylor_id, enrollment).await,
        ExportRefusal::CapabilityRequired
    );

    // The whole installation takes export_records: the administrator
    // bundle carries it, the coordinator bundle does not.
    export(&fx, fx.admin_id, Scope::Installation).await;
    assert_eq!(
        refusal(&fx, s.casey_id, Scope::Installation).await,
        ExportRefusal::CapabilityRequired
    );
    let summary = record_export::summary(&fx.pool, fx.admin_id)
        .await
        .expect("call")
        .expect("summarized");
    assert_eq!((summary.record_count, summary.version_count), (1, 2));
    assert_eq!(
        record_export::summary(&fx.pool, s.casey_id)
            .await
            .expect("call"),
        Err(ExportRefusal::CapabilityRequired)
    );

    // Unknown identities and empty scopes are typed, never an empty
    // archive presented as complete.
    assert_eq!(
        refusal(&fx, s.casey_id, Scope::Record { record_id: 9999 }).await,
        ExportRefusal::NoSuchRecord
    );
    assert_eq!(
        refusal(
            &fx,
            s.casey_id,
            Scope::Version {
                record_id: s.record_id,
                version_number: 7,
            }
        )
        .await,
        ExportRefusal::NoSuchVersion
    );
    assert_eq!(
        refusal(
            &fx,
            s.casey_id,
            Scope::Enrollment {
                enrollment_id: 9999
            }
        )
        .await,
        ExportRefusal::NoSuchEnrollment
    );
    let unfinalized = draft_for(&fx, s.jordan_id, s.enrollment_id, "2026-06-03").await;
    assert_eq!(
        refusal(
            &fx,
            s.jordan_id,
            Scope::Record {
                record_id: unfinalized
            }
        )
        .await,
        ExportRefusal::NothingToExport
    );
    let riley = fx
        .user_with_role("riley.scopes", "Riley Trainee", RoleBundle::Trainee)
        .await;
    let empty_enrollment = enrollments::enroll(&fx.pool, fx.admin_id, s.version_id, riley)
        .await
        .expect("call")
        .expect("enrolled");
    assert_eq!(
        refusal(
            &fx,
            fx.admin_id,
            Scope::Enrollment {
                enrollment_id: empty_enrollment
            }
        )
        .await,
        ExportRefusal::NothingToExport
    );
    // Refusals leave no export audit behind.
    let audited: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_event WHERE kind = 'record_exported'")
            .fetch_one(&fx.pool)
            .await
            .expect("count");
    assert_eq!(audited, 10);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn export_api_delivers_the_documented_bytes() {
    let fx = Fixture::new().await;
    let s = seed(&fx, "api").await;
    let casey = fx.login("casey.api").await;
    let admin = fx.login("avery.admin").await;

    let (status, headers, bytes) = raw_get(
        fx.app(),
        &format!("/api/drafts/{}/versions/2/export", s.record_id),
        &casey,
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
            "attachment; filename=\"consolebook-record-{}-v2-",
            s.record_id
        )) && disposition.ends_with("Z.zip\""),
        "got: {disposition}"
    );
    let report = record_export::verify_archive(&bytes);
    assert!(report.verified(), "{report:?}");
    assert_eq!(report.units.len(), 1);

    let (status, _, bytes) = raw_get(
        fx.app(),
        &format!("/api/drafts/{}/export", s.record_id),
        &casey,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(record_export::verify_archive(&bytes).units.len(), 2);
    let (status, _, bytes) = raw_get(
        fx.app(),
        &format!("/api/enrollments/{}/export", s.enrollment_id),
        &casey,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(record_export::verify_archive(&bytes).verified());

    // The installation scope answers to export_records only.
    let (status, _, body) = raw_get(fx.app(), "/api/exports/records", &casey).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let body: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(body["error"], "capability_required");
    let (status, _, bytes) = raw_get(fx.app(), "/api/exports/records", &admin).await;
    assert_eq!(status, StatusCode::OK);
    let report = record_export::verify_archive(&bytes);
    assert!(report.verified());
    assert_eq!(report.scope, Some(Scope::Installation));
    let (status, _, body) = raw_get(fx.app(), "/api/exports/summary", &admin).await;
    assert_eq!(status, StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(body["record_count"], 1);
    assert_eq!(body["version_count"], 2);
    let (status, _, _) = raw_get(fx.app(), "/api/exports/summary", &casey).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Typed refusals over the wire.
    let (status, _, body) = raw_get(fx.app(), "/api/enrollments/9999/export", &casey).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let body: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(body["error"], "no_such_enrollment");
    let (status, _, body) = raw_get(
        fx.app(),
        &format!("/api/drafts/{}/versions/9/export", s.record_id),
        &casey,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let body: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(body["error"], "no_such_version");
    let unfinalized = draft_for(&fx, s.jordan_id, s.enrollment_id, "2026-06-03").await;
    let (status, _, body) = raw_get(
        fx.app(),
        &format!("/api/drafts/{unfinalized}/export"),
        &casey,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let body: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(body["error"], "nothing_to_export");
    let outsider_cookie = {
        fx.user_with_role("robin.api", "Robin Outsider", RoleBundle::Trainer)
            .await;
        fx.login("robin.api").await
    };
    let (status, _, _) = raw_get(
        fx.app(),
        &format!("/api/drafts/{}/export", s.record_id),
        &outsider_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = raw_get(
        fx.app(),
        &format!("/api/drafts/{}/export", s.record_id),
        "not-a-session",
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn cli_verifies_from_the_file_alone() {
    let fx = Fixture::new().await;
    let s = seed(&fx, "cli").await;
    let bytes = export(
        &fx,
        s.casey_id,
        Scope::Record {
            record_id: s.record_id,
        },
    )
    .await;
    let scratch = tempfile::tempdir().expect("scratch");
    let archive = scratch.path().join("export.zip");
    std::fs::write(&archive, &bytes).expect("write");
    // The data directory named here must never be touched: the archive
    // carries everything the checks need.
    let untouched = scratch.path().join("never-created");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_consolebook-server"))
        .args(["--data-dir"])
        .arg(&untouched)
        .args(["export", "verify"])
        .arg(&archive)
        .output()
        .expect("run verifier");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "stdout: {stdout}");
    assert!(stdout.contains("verified 2 of 2 units"), "stdout: {stdout}");
    assert!(stdout.contains("predecessor linked"), "stdout: {stdout}");
    assert!(!untouched.exists(), "the verifier opened no data directory");

    // A tampered file fails with a named finding and a failing exit code.
    let listed = entries(&bytes);
    let tampered = repack(&with_entry(
        &listed,
        &format!("records/{}/v1/record.json", s.record_id),
        |content| {
            let text = String::from_utf8(content.to_vec()).expect("utf-8");
            Some(
                text.replace("initial entry", "initial entry, revised")
                    .into_bytes(),
            )
        },
    ));
    std::fs::write(&archive, &tampered).expect("write");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_consolebook-server"))
        .args(["export", "verify"])
        .arg(&archive)
        .output()
        .expect("run verifier");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "stdout: {stdout}");
    assert!(stdout.contains("NOT VERIFIED"), "stdout: {stdout}");
    assert!(
        stdout.contains("the content hash does not match the record bytes"),
        "stdout: {stdout}"
    );
    // Version 2 still verifies on its own: its chain covers the hash the
    // manifest states for version 1, and that statement is unchanged;
    // the altered bytes fail where they sit.
    assert!(
        stdout.contains("NOT VERIFIED: 1 of 2 units consistent, 0 archive finding(s)"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains(&format!("FAIL  records/{}/v1", s.record_id)),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains(&format!("ok    records/{}/v2", s.record_id)),
        "stdout: {stdout}"
    );
}
