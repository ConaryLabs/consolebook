//! Milestone 4 slice 4: weekly summaries and task signoffs — a summary
//! is an ordinary record whose copy links the exact finalized daily
//! versions it covers, sealed into the record-schema-2 envelope; a
//! signoff is versioned state per (enrollment, task) whose overrides
//! take authority and a reason. The database holds both raw. Every
//! fixture is invented.

use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, SET_COOKIE};
use axum::http::{Request, StatusCode};
use consolebook_server::acknowledgments::{self, TraineeAckKind};
use consolebook_server::amendments;
use consolebook_server::capabilities::RoleBundle;
use consolebook_server::evaluation_drafts::{self, DraftRefusal, DraftStatus};
use consolebook_server::finalization;
use consolebook_server::programs::{
    self, AnchorDef, CompetencyDef, FormCompetencyDef, FormDef, NarrativeDef, PolicyDef,
    RecordType, ScaleDef, ScaleKind, TaskDef, VersionContent,
};
use consolebook_server::summaries;
use consolebook_server::task_signoffs::{self, SignoffKind, SignoffRefusal};
use consolebook_server::training_sessions::{self, Disposition, SessionInput};
use consolebook_server::{
    assignments, canonical, data_dir::DataDir, enrollments, setup, storage, users,
};
use http_body_util::BodyExt;
use tower::ServiceExt;

const PASSWORD: &str = "invented-passphrase-1";

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

    async fn login(&self, username: &str, password: &str) -> String {
        let response = self
            .app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "username": username, "password": password })
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

    async fn version_row(&self, record_id: i64, number: i64) -> (i64, Vec<u8>, String, String) {
        let row: (i64, Vec<u8>, String, String) = sqlx::query_as(
            "SELECT id, canonical_bytes, content_hash, chain_hash
             FROM evaluation_version
             WHERE evaluation_record_id = ?1 AND version_number = ?2",
        )
        .bind(record_id)
        .bind(number)
        .fetch_one(&self.pool)
        .await
        .expect("version row");
        row
    }
}

async fn request(
    app: axum::Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(cookie) = cookie {
        builder = builder.header(
            COOKIE,
            format!("{}={}", consolebook_server::http::SESSION_COOKIE, cookie),
        );
    }
    let request = if let Some(body) = body {
        builder
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
    } else {
        builder.body(Body::empty())
    }
    .expect("request");
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, body)
}

/// Invented program with one tasked competency, a daily form, and a
/// weekly summary form; completion rules per `policy`.
fn program(name: &str, policy: PolicyDef) -> VersionContent {
    VersionContent {
        name: name.to_owned(),
        label: "2026 rev A".to_owned(),
        description: "Invented program for summary and signoff tests.".to_owned(),
        phases: Vec::new(),
        phase_transitions: Vec::new(),
        competencies: vec![CompetencyDef {
            category: "Call processing".to_owned(),
            name: "Emergency Call Interrogation".to_owned(),
            description: "Obtains and verifies location, callback, and nature.".to_owned(),
            tasks: vec![
                TaskDef {
                    prompt: "Processes an invented structure-fire call.".to_owned(),
                    citations: Vec::new(),
                },
                TaskDef {
                    prompt: "Processes an invented medical call.".to_owned(),
                    citations: Vec::new(),
                },
            ],
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
        evaluation_forms: vec![
            FormDef {
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
            },
            FormDef {
                record_type: RecordType::WeeklySummary,
                name: "Weekly Summary".to_owned(),
                instructions: "Summarize the week.".to_owned(),
                competencies: Vec::new(),
                narratives: vec![NarrativeDef {
                    prompt: "Weekly overview.".to_owned(),
                    required: false,
                }],
            },
        ],
        citations: Vec::new(),
        finalization_policy: policy,
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

async fn seed(fx: &Fixture, policy: PolicyDef, suffix: &str) -> Seeded {
    let content = program(&format!("Example County Program {suffix}"), policy);
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
    Seeded {
        version_id,
        enrollment_id,
        record_id,
        taylor_id,
        jordan_id,
        casey_id,
    }
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

async fn task_ids(fx: &Fixture, version_id: i64) -> Vec<i64> {
    sqlx::query_scalar("SELECT id FROM task WHERE program_version_id = ?1 ORDER BY sort_order, id")
        .bind(version_id)
        .fetch_all(&fx.pool)
        .await
        .expect("tasks")
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn weekly_summary_links_and_seals() {
    let fx = Fixture::new().await;
    let s = seed(&fx, OPEN_POLICY, "weekly").await;
    finalization::finalize(&fx.pool, s.casey_id, s.record_id, 0)
        .await
        .expect("call")
        .expect("daily sealed");
    let (daily_v1, _, daily_hash, _) = fx.version_row(s.record_id, 1).await;

    // Creation takes authoring scope on the enrollment.
    let refused = summaries::create(&fx.pool, s.taylor_id, s.enrollment_id, None)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::CapabilityRequired));
    let refused = summaries::create(&fx.pool, s.casey_id, 9999, None)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::NoSuchEnrollment));
    let summary_id = summaries::create(&fx.pool, s.jordan_id, s.enrollment_id, None)
        .await
        .expect("call")
        .expect("created");
    let forms = summaries::list_summary_forms(&fx.pool, s.casey_id, s.enrollment_id)
        .await
        .expect("call")
        .expect("listed");
    assert_eq!(forms.len(), 1);
    assert_eq!(forms[0].name, "Weekly Summary");

    // Links are validated typed: version, enrollment, record type,
    // duplication — and the picker offers exactly the unlinked dailies.
    let linkable = summaries::linkable(&fx.pool, s.jordan_id, summary_id)
        .await
        .expect("call")
        .expect("listed");
    assert_eq!(linkable.len(), 1);
    assert_eq!(linkable[0].daily_version_id, daily_v1);
    assert_eq!(linkable[0].business_date.as_deref(), Some("2026-06-02"));
    let refused = summaries::add_link(&fx.pool, s.jordan_id, summary_id, 9999, 0)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::NoSuchVersion));
    let riley_id = fx
        .user_with_role("riley.weekly", "Riley Trainee", RoleBundle::Trainee)
        .await;
    let other_enrollment = enrollments::enroll(&fx.pool, fx.admin_id, s.version_id, riley_id)
        .await
        .expect("call")
        .expect("enrolled");
    assignments::create(&fx.pool, fx.admin_id, other_enrollment, s.jordan_id)
        .await
        .expect("call")
        .expect("assigned");
    let other_daily = draft_for(&fx, s.jordan_id, other_enrollment, "2026-06-03").await;
    finalization::finalize(&fx.pool, s.casey_id, other_daily, 0)
        .await
        .expect("call")
        .expect("sealed");
    let (other_v1, _, _, _) = fx.version_row(other_daily, 1).await;
    let refused = summaries::add_link(&fx.pool, s.jordan_id, summary_id, other_v1, 0)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::WrongEnrollment));
    let revision = summaries::add_link(&fx.pool, s.jordan_id, summary_id, daily_v1, 0)
        .await
        .expect("call")
        .expect("linked");
    let refused = summaries::add_link(&fx.pool, s.jordan_id, summary_id, daily_v1, revision)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::DuplicateLink));
    let linkable = summaries::linkable(&fx.pool, s.jordan_id, summary_id)
        .await
        .expect("call")
        .expect("listed");
    assert!(linkable.is_empty());

    // The shape holds raw: links belong to summaries (probed on an
    // unfinalized daily so the type rule answers, not the freeze),
    // stay home, and are never edited.
    let open_daily = draft_for(&fx, s.jordan_id, s.enrollment_id, "2026-06-04").await;
    let raw = sqlx::query(
        "INSERT INTO summary_daily_link (summary_record_id, daily_version_id)
         VALUES (?1, ?2)",
    )
    .bind(open_daily)
    .bind(daily_v1)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("belong to weekly summaries"), "type: {err}");
    let raw = sqlx::query(
        "INSERT INTO summary_daily_link (summary_record_id, daily_version_id)
         VALUES (?1, ?2)",
    )
    .bind(summary_id)
    .bind(other_v1)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("own enrollment"), "home: {err}");
    let raw = sqlx::query("UPDATE summary_daily_link SET daily_version_id = ?1")
        .bind(other_v1)
        .execute(&fx.pool)
        .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("never edited"), "edit: {err}");

    // Sealing commits the coverage into the schema-2 bytes.
    let meta = finalization::finalize(&fx.pool, s.casey_id, summary_id, revision)
        .await
        .expect("call")
        .expect("sealed");
    assert_eq!(meta.record_schema, 2);
    let (_, bytes, content_hash, _) = fx.version_row(summary_id, 1).await;
    assert_eq!(canonical::content_hash_hex(&bytes), content_hash);
    let envelope: serde_json::Value = serde_json::from_slice(&bytes).expect("envelope");
    assert_eq!(envelope["record"]["record_schema"], 2);
    assert_eq!(envelope["form"]["record_type"], "weekly_summary");
    let covered = envelope["daily_reports"].as_array().expect("coverage");
    assert_eq!(covered.len(), 1);
    assert_eq!(covered[0]["record_id"], s.record_id);
    assert_eq!(covered[0]["version_number"], 1);
    assert_eq!(covered[0]["content_hash"], daily_hash);

    // Sealed links are frozen raw and refused typed.
    let raw = sqlx::query("DELETE FROM summary_daily_link WHERE summary_record_id = ?1")
        .bind(summary_id)
        .execute(&fx.pool)
        .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("frozen"), "frozen: {err}");
    let refused = summaries::remove_link(&fx.pool, s.jordan_id, summary_id, daily_v1, revision)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::DraftFinalized));
    // A sealed summary version is not a linkable daily.
    let another = summaries::create(&fx.pool, s.jordan_id, s.enrollment_id, None)
        .await
        .expect("call")
        .expect("created");
    let (summary_v1, _, _, _) = fx.version_row(summary_id, 1).await;
    let refused = summaries::add_link(&fx.pool, s.jordan_id, another, summary_v1, 0)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::NotADaily));

    // The whole record lifecycle applies to summaries: the trainee
    // acknowledges it, and an amendment thaws the links and re-seals
    // as version 2 with the reduced coverage.
    acknowledgments::acknowledge(
        &fx.pool,
        s.taylor_id,
        summary_id,
        TraineeAckKind::Acknowledged,
        "",
    )
    .await
    .expect("call")
    .expect("acknowledged");
    let timeline = acknowledgments::own_records(&fx.pool, s.taylor_id)
        .await
        .expect("call")
        .expect("listed");
    assert_eq!(timeline.len(), 2, "the daily and the summary");
    amendments::open(
        &fx.pool,
        s.casey_id,
        summary_id,
        "The invented week was linked to the wrong day.",
    )
    .await
    .expect("call")
    .expect("opened");
    let workspace = evaluation_drafts::workspace(&fx.pool, s.jordan_id, summary_id)
        .await
        .expect("call")
        .expect("readable");
    assert_eq!(workspace.detail.status, DraftStatus::Draft);
    assert_eq!(workspace.detail.summary_links.len(), 1);
    let revision = summaries::remove_link(
        &fx.pool,
        s.jordan_id,
        summary_id,
        daily_v1,
        workspace.detail.revision,
    )
    .await
    .expect("call")
    .expect("unlinked");
    let meta = finalization::finalize(&fx.pool, s.casey_id, summary_id, revision)
        .await
        .expect("call")
        .expect("resealed");
    assert_eq!(meta.version_number, 2);
    let verification = finalization::verify(&fx.pool, s.casey_id, summary_id)
        .await
        .expect("call")
        .expect("readable")
        .expect("finalized");
    assert!(verification.content_hash_ok && verification.chain_hash_ok);
    let (_, v2_bytes, _, _) = fx.version_row(summary_id, 2).await;
    let v2_envelope: serde_json::Value = serde_json::from_slice(&v2_bytes).expect("envelope");
    assert_eq!(v2_envelope["daily_reports"], serde_json::json!([]));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_signoffs_version_per_task() {
    let fx = Fixture::new().await;
    let s = seed(&fx, OPEN_POLICY, "signoff").await;
    let tasks = task_ids(&fx, s.version_id).await;
    assert_eq!(tasks.len(), 2);

    // Reads take the enrollment-history gate; the matrix starts empty.
    let refused = task_signoffs::matrix(&fx.pool, s.taylor_id, s.enrollment_id)
        .await
        .expect("call");
    assert!(matches!(refused, Err(SignoffRefusal::CapabilityRequired)));
    let matrix = task_signoffs::matrix(&fx.pool, s.casey_id, s.enrollment_id)
        .await
        .expect("call")
        .expect("listed");
    assert_eq!(matrix.len(), 2);
    assert!(
        matrix
            .iter()
            .all(|row| row.kind.is_none() && row.history == 0)
    );

    // The first signoff takes authoring scope; a revocation needs
    // something to revoke; the task must be pinned.
    let outsider = fx
        .user_with_role("rowan.signoff", "Rowan Trainer", RoleBundle::Trainer)
        .await;
    let refused = task_signoffs::record(
        &fx.pool,
        outsider,
        s.enrollment_id,
        tasks[0],
        SignoffKind::Observed,
        "",
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(SignoffRefusal::CapabilityRequired));
    let refused = task_signoffs::record(
        &fx.pool,
        s.jordan_id,
        s.enrollment_id,
        tasks[1],
        SignoffKind::Revoked,
        "",
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(SignoffRefusal::NothingToRevoke));
    let refused = task_signoffs::record(
        &fx.pool,
        s.jordan_id,
        s.enrollment_id,
        9999,
        SignoffKind::Observed,
        "",
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(SignoffRefusal::NoSuchTask));
    task_signoffs::record(
        &fx.pool,
        s.jordan_id,
        s.enrollment_id,
        tasks[0],
        SignoffKind::Observed,
        "",
    )
    .await
    .expect("call")
    .expect("signed");
    let matrix = task_signoffs::matrix(&fx.pool, s.jordan_id, s.enrollment_id)
        .await
        .expect("call")
        .expect("listed");
    let first = matrix
        .iter()
        .find(|row| row.task_id == tasks[0])
        .expect("row");
    assert_eq!(first.kind.as_deref(), Some("observed"));
    assert_eq!(
        first.signed_by_display_name.as_deref(),
        Some("Jordan Trainer")
    );
    assert_eq!(first.history, 1);

    // Overrides supersede recorded state: review authority and a
    // recorded reason, at the service and raw.
    let refused = task_signoffs::record(
        &fx.pool,
        s.jordan_id,
        s.enrollment_id,
        tasks[0],
        SignoffKind::Demonstrated,
        "trainers do not override",
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(SignoffRefusal::CapabilityRequired));
    let refused = task_signoffs::record(
        &fx.pool,
        s.casey_id,
        s.enrollment_id,
        tasks[0],
        SignoffKind::Demonstrated,
        " \u{2003} ",
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(SignoffRefusal::ReasonRequired));
    task_signoffs::record(
        &fx.pool,
        s.casey_id,
        s.enrollment_id,
        tasks[0],
        SignoffKind::Demonstrated,
        "Re-observed at the invented console; performance improved.",
    )
    .await
    .expect("call")
    .expect("overridden");
    task_signoffs::record(
        &fx.pool,
        s.casey_id,
        s.enrollment_id,
        tasks[0],
        SignoffKind::Revoked,
        "Signed off in error for the invented shift.",
    )
    .await
    .expect("call")
    .expect("revoked");
    let matrix = task_signoffs::matrix(&fx.pool, s.casey_id, s.enrollment_id)
        .await
        .expect("call")
        .expect("listed");
    let first = matrix
        .iter()
        .find(|row| row.task_id == tasks[0])
        .expect("row");
    assert_eq!(first.kind.as_deref(), Some("revoked"));
    assert_eq!(first.history, 3);

    // Permanence, ordering, and pinning hold raw; the first row's name
    // snapshot survives a rename.
    let raw = sqlx::query("UPDATE task_signoff SET kind = 'demonstrated' WHERE 1 = 1")
        .execute(&fx.pool)
        .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("never edited"), "update: {err}");
    let raw = sqlx::query("DELETE FROM task_signoff WHERE 1 = 1")
        .execute(&fx.pool)
        .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("never edited"), "delete: {err}");
    let raw = sqlx::query(
        "INSERT INTO task_signoff
             (enrollment_id, task_id, kind, reason, signed_by,
              signed_by_display_name, signed_at)
         VALUES (?1, ?2, 'observed', '', ?3, 'Casey Coordinator', 1)",
    )
    .bind(s.enrollment_id)
    .bind(tasks[0])
    .bind(s.casey_id)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("records its reason"), "override raw: {err}");
    let raw = sqlx::query(
        "INSERT INTO task_signoff
             (enrollment_id, task_id, kind, reason, signed_by,
              signed_by_display_name, signed_at)
         VALUES (?1, ?2, 'revoked', 'nothing there', ?3, 'Casey Coordinator', 1)",
    )
    .bind(s.enrollment_id)
    .bind(tasks[1])
    .bind(s.casey_id)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("supersedes"), "revoke raw: {err}");
    let other = seed(&fx, OPEN_POLICY, "signoff2").await;
    let other_tasks = task_ids(&fx, other.version_id).await;
    let raw = sqlx::query(
        "INSERT INTO task_signoff
             (enrollment_id, task_id, kind, reason, signed_by,
              signed_by_display_name, signed_at)
         VALUES (?1, ?2, 'observed', '', ?3, 'Casey Coordinator', 1)",
    )
    .bind(s.enrollment_id)
    .bind(other_tasks[0])
    .bind(s.casey_id)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("pinned version"), "pinning: {err}");
    sqlx::query("UPDATE user SET display_name = 'Jordan Renamed' WHERE id = ?1")
        .bind(s.jordan_id)
        .execute(&fx.pool)
        .await
        .expect("rename");
    let snapshot: String = sqlx::query_scalar(
        "SELECT signed_by_display_name FROM task_signoff
         WHERE enrollment_id = ?1 AND task_id = ?2 ORDER BY id LIMIT 1",
    )
    .bind(s.enrollment_id)
    .bind(tasks[0])
    .fetch_one(&fx.pool)
    .await
    .expect("snapshot");
    assert_eq!(snapshot, "Jordan Trainer");
    let audited: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_event WHERE kind = 'task_signoff_recorded'")
            .fetch_one(&fx.pool)
            .await
            .expect("audit");
    assert_eq!(audited, 3);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn summary_and_signoff_api_round_trip() {
    let fx = Fixture::new().await;
    let s = seed(&fx, OPEN_POLICY, "api").await;
    finalization::finalize(&fx.pool, s.casey_id, s.record_id, 0)
        .await
        .expect("call")
        .expect("daily sealed");
    let (daily_v1, _, _, _) = fx.version_row(s.record_id, 1).await;
    let casey = fx.login("casey.api", PASSWORD).await;
    let tasks = task_ids(&fx, s.version_id).await;

    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/enrollments/{}/weekly-summary", s.enrollment_id),
        Some(&casey),
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let summary_id = body["id"].as_i64().expect("id");
    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/drafts/{summary_id}"),
        Some(&casey),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["record_type"], "weekly_summary");
    assert_eq!(body["summary_links"], serde_json::json!([]));
    let (status, _) = request(
        fx.app(),
        "POST",
        &format!("/api/drafts/{summary_id}/links"),
        Some(&casey),
        Some(serde_json::json!({ "daily_version_id": daily_v1, "revision": 0 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/drafts/{summary_id}/linkable-dailies"),
        Some(&casey),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["dailies"], serde_json::json!([]));
    let (status, _) = request(
        fx.app(),
        "POST",
        &format!("/api/drafts/{summary_id}/finalize"),
        Some(&casey),
        Some(serde_json::json!({ "revision": 1 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/drafts/{summary_id}/version"),
        Some(&casey),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["envelope"]["daily_reports"][0]["record_id"],
        s.record_id
    );

    let (status, _) = request(
        fx.app(),
        "POST",
        &format!("/api/enrollments/{}/signoffs", s.enrollment_id),
        Some(&casey),
        Some(serde_json::json!({ "task_id": tasks[0], "kind": "observed" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/enrollments/{}/signoffs", s.enrollment_id),
        Some(&casey),
        Some(serde_json::json!({ "task_id": tasks[0], "kind": "demonstrated" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "reason_required");
    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/enrollments/{}/signoffs", s.enrollment_id),
        Some(&casey),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let row = body["tasks"]
        .as_array()
        .expect("tasks")
        .iter()
        .find(|row| row["task_id"] == tasks[0])
        .expect("row");
    assert_eq!(row["kind"], "observed");
}
