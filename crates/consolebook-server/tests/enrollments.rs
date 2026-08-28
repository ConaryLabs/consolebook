//! Milestone 2 slice 3: minimal user creation, enrollment pinning, and
//! the milestone exit — a complete invented program published, enrolled,
//! exported, and reproduced on a clean installation. Every fixture is
//! invented.

use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, SET_COOKIE};
use axum::http::{Request, StatusCode};
use consolebook_server::capabilities::RoleBundle;
use consolebook_server::data_dir::DataDir;
use consolebook_server::program_export::{self, ImportTarget};
use consolebook_server::programs::{
    self, AnchorDef, CitationDef, CompetencyDef, FormCompetencyDef, FormDef, ModifierDef,
    NarrativeDef, PhaseDef, RecordType, ScaleDef, ScaleKind, TaskDef, TransitionDef,
    TransitionKind, VersionContent,
};
use consolebook_server::{enrollments, setup, storage, users};
use http_body_util::BodyExt;
use tower::ServiceExt;

const PASSWORD: &str = "invented-passphrase-1";
const TRAINEE_PASSWORD: &str = "trainee-passphrase-3";

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

    /// Creates a program with one published version of `content`; returns
    /// (program id, version id).
    async fn published_program(&self, content: &VersionContent) -> (i64, i64) {
        let program_id = programs::create_program(&self.pool, self.admin_id, &content.name)
            .await
            .expect("create program")
            .expect("accepted");
        let version_id = programs::create_version(&self.pool, self.admin_id, program_id, content)
            .await
            .expect("create version")
            .expect("accepted");
        programs::publish_version(&self.pool, self.admin_id, version_id)
            .await
            .expect("publish")
            .expect("accepted");
        (program_id, version_id)
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

fn citation(body: &str, edition: &str, clause: &str, note: &str) -> CitationDef {
    CitationDef {
        body: body.to_owned(),
        edition: edition.to_owned(),
        clause: clause.to_owned(),
        note: note.to_owned(),
    }
}

fn all_scale_kinds() -> Vec<ScaleDef> {
    vec![
        ScaleDef {
            name: "Narrative Assessment".to_owned(),
            kind: ScaleKind::NarrativeOnly,
            min_value: None,
            max_value: None,
            anchors: Vec::new(),
        },
        ScaleDef {
            name: "Pass Fail Check".to_owned(),
            kind: ScaleKind::PassFail,
            min_value: None,
            max_value: None,
            anchors: vec![
                AnchorDef {
                    value: 0,
                    label: "Not Demonstrated".to_owned(),
                    definition: String::new(),
                },
                AnchorDef {
                    value: 1,
                    label: "Demonstrated".to_owned(),
                    definition: String::new(),
                },
            ],
        },
        ScaleDef {
            name: "Seven Point".to_owned(),
            kind: ScaleKind::AnchoredNumeric,
            min_value: Some(1),
            max_value: Some(7),
            anchors: vec![
                AnchorDef {
                    value: 1,
                    label: "Unacceptable".to_owned(),
                    definition: "Well below trainee standard.".to_owned(),
                },
                AnchorDef {
                    value: 7,
                    label: "Superior".to_owned(),
                    definition: "Beyond solo-capable standard.".to_owned(),
                },
            ],
        },
    ]
}

/// A complete invented program: phases with a remediation loop, tasks,
/// all three scale kinds, a modifier, forms, and citations at every
/// level — the "complete" the milestone exit demands.
fn complete_content() -> VersionContent {
    VersionContent {
        name: "Example County CTO Program".to_owned(),
        label: "2026 rev A".to_owned(),
        description: "Invented communications training officer program.".to_owned(),
        phases: vec![
            PhaseDef {
                name: "Phase One".to_owned(),
                description: "Observation.".to_owned(),
                presentation_number: 1,
            },
            PhaseDef {
                name: "Phase Two".to_owned(),
                description: "Independent performance.".to_owned(),
                presentation_number: 2,
            },
        ],
        phase_transitions: vec![
            TransitionDef {
                from_phase: "Phase One".to_owned(),
                to_phase: "Phase Two".to_owned(),
                kind: TransitionKind::Advance,
            },
            TransitionDef {
                from_phase: "Phase Two".to_owned(),
                to_phase: "Phase One".to_owned(),
                kind: TransitionKind::Remediation,
            },
        ],
        competencies: vec![CompetencyDef {
            category: "Call Processing".to_owned(),
            name: "Emergency Call Interrogation".to_owned(),
            description: "Obtains and verifies location, callback, and nature.".to_owned(),
            tasks: vec![TaskDef {
                prompt: "Processes an invented structure-fire call.".to_owned(),
                citations: vec![citation(
                    "Example Accreditation Program",
                    "3rd",
                    "6.1.2",
                    "",
                )],
            }],
            citations: vec![citation(
                "Example State Training Rule",
                "",
                "T-100",
                "annual",
            )],
        }],
        rating_scales: all_scale_kinds(),
        rating_modifiers: vec![ModifierDef {
            code: "NRT".to_owned(),
            label: "Not Responding to Training".to_owned(),
            description: String::new(),
        }],
        evaluation_forms: vec![FormDef {
            record_type: RecordType::DailyReport,
            name: "Daily Observation Report".to_owned(),
            instructions: String::new(),
            competencies: vec![FormCompetencyDef {
                competency: "Emergency Call Interrogation".to_owned(),
                rating_scale: "Seven Point".to_owned(),
            }],
            narratives: vec![NarrativeDef {
                prompt: "Most acceptable performance".to_owned(),
                required: true,
            }],
        }],
        citations: vec![citation("Example Accreditation Program", "3rd", "6.1", "")],
    }
}

#[tokio::test]
async fn a_created_user_signs_in_through_the_reset_flow() {
    let fx = Fixture::new().await;
    let admin = fx.login("avery.admin", PASSWORD).await;

    let (status, body) = request(
        fx.app(),
        "POST",
        "/api/users",
        Some(&admin),
        Some(serde_json::json!({"username": "jordan.trainee", "display_name": "Jordan Trainee"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create user: {body}");
    let reset_code = body["reset_code"].as_str().expect("reset code").to_owned();

    // The account exists but has no usable password.
    let (status, _) = request(
        fx.app(),
        "POST",
        "/api/auth/login",
        None,
        Some(serde_json::json!({"username": "jordan.trainee", "password": ""})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // The reset code sets the first password through the standard flow.
    let (status, body) = request(
        fx.app(),
        "POST",
        "/api/auth/reset",
        None,
        Some(serde_json::json!({
            "username": "jordan.trainee",
            "reset_code": reset_code,
            "new_password": TRAINEE_PASSWORD,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "reset: {body}");
    let _trainee_cookie = fx.login("jordan.trainee", TRAINEE_PASSWORD).await;
}

#[tokio::test]
async fn user_creation_refusals_and_roster_gates() {
    let fx = Fixture::new().await;
    let admin = fx.login("avery.admin", PASSWORD).await;
    users::create_with_reset_code(
        &fx.pool,
        fx.admin_id,
        "jordan.trainee",
        "Jordan Trainee",
        "",
        "",
        RoleBundle::Trainee,
    )
    .await
    .expect("create")
    .expect("accepted");
    // The created account holds no capabilities; give it a session by
    // setting a password through the reset flow at the service level.
    let issued = users::issue_reset_code(
        &fx.pool,
        "jordan.trainee",
        users::ResetOrigin::Administrator {
            issued_by: fx.admin_id,
        },
    )
    .await
    .expect("issue")
    .expect("issued");
    assert_eq!(
        users::use_reset_code(
            &fx.pool,
            "jordan.trainee",
            &issued.code.raw,
            TRAINEE_PASSWORD
        )
        .await
        .expect("reset"),
        users::ResetOutcome::Done
    );

    // Refusals: duplicate, blank, and capability-less creation.
    let (status, body) = request(
        fx.app(),
        "POST",
        "/api/users",
        Some(&admin),
        Some(serde_json::json!({"username": "JORDAN.TRAINEE", "display_name": ""})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "usernames are case-insensitive"
    );
    assert_eq!(body["error"], "username_taken");
    let (status, body) = request(
        fx.app(),
        "POST",
        "/api/users",
        Some(&admin),
        Some(serde_json::json!({"username": "  ", "display_name": ""})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "username_invalid");
    let trainee = fx.login("jordan.trainee", TRAINEE_PASSWORD).await;
    let (status, body) = request(
        fx.app(),
        "POST",
        "/api/users",
        Some(&trainee),
        Some(serde_json::json!({"username": "casey.caller", "display_name": ""})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "capability_required");
    // The roster needs manage_users or assign_training.
    let (status, _) = request(fx.app(), "GET", "/api/users", Some(&trainee), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = request(fx.app(), "GET", "/api/users", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["users"].as_array().expect("users").len(), 2);
}

#[tokio::test]
async fn enrollment_pins_published_versions_only() {
    let fx = Fixture::new().await;
    let admin = fx.login("avery.admin", PASSWORD).await;
    let trainee = users::create_with_reset_code(
        &fx.pool,
        fx.admin_id,
        "jordan.trainee",
        "Jordan Trainee",
        "",
        "",
        RoleBundle::Trainee,
    )
    .await
    .expect("create")
    .expect("accepted");

    // A draft refuses enrollment at the service…
    let program_id = programs::create_program(&fx.pool, fx.admin_id, "Example County CTO Program")
        .await
        .expect("create program")
        .expect("accepted");
    let draft_id = programs::create_version(&fx.pool, fx.admin_id, program_id, &complete_content())
        .await
        .expect("create version")
        .expect("accepted");
    let refused = enrollments::enroll(&fx.pool, fx.admin_id, draft_id, trainee.id)
        .await
        .expect("call");
    assert_eq!(refused, Err(enrollments::EnrollRefusal::NotPublished));

    // …and at the database, even for a raw INSERT.
    let raw = sqlx::query(
        "INSERT INTO enrollment (user_id, program_version_id, enrolled_at, enrolled_by)
         VALUES (?1, ?2, 0, NULL)",
    )
    .bind(trainee.id)
    .bind(draft_id)
    .execute(&fx.pool)
    .await;
    let err = raw
        .expect_err("draft enrollment must be rejected")
        .to_string();
    assert!(
        err.contains("published"),
        "trigger must refuse drafts: {err}"
    );

    programs::publish_version(&fx.pool, fx.admin_id, draft_id)
        .await
        .expect("publish")
        .expect("accepted");

    // Over the API: enroll, list, and every refusal code.
    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/program-versions/{draft_id}/enrollments"),
        Some(&admin),
        Some(serde_json::json!({"user_id": trainee.id})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "enroll: {body}");
    let enrollment_id = body["id"].as_i64().expect("enrollment id");

    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/program-versions/{draft_id}/enrollments"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["enrollees"][0]["username"], "jordan.trainee");
    assert_eq!(body["enrollees"][0]["enrollment_id"], enrollment_id);

    // The enrollment is audited with both the trainee and the row.
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_event
         WHERE kind = 'enrollment_created' AND actor_user_id = ?1
           AND subject_user_id = ?2 AND subject_kind = 'enrollment' AND subject_id = ?3",
    )
    .bind(fx.admin_id)
    .bind(trainee.id)
    .bind(enrollment_id)
    .fetch_one(&fx.pool)
    .await
    .expect("count");
    assert_eq!(count, 1, "enrollment must be attributably audited");
}

#[tokio::test]
async fn enrollment_refusals_and_pin_protection() {
    let fx = Fixture::new().await;
    let admin = fx.login("avery.admin", PASSWORD).await;
    let (program_id, version_id) = fx.published_program(&complete_content()).await;
    let trainee = users::create_with_reset_code(
        &fx.pool,
        fx.admin_id,
        "jordan.trainee",
        "Jordan Trainee",
        "",
        "",
        RoleBundle::Trainee,
    )
    .await
    .expect("create")
    .expect("accepted");
    let enrollment_id = enrollments::enroll(&fx.pool, fx.admin_id, version_id, trainee.id)
        .await
        .expect("call")
        .expect("enrolled");

    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/program-versions/{version_id}/enrollments"),
        Some(&admin),
        Some(serde_json::json!({"user_id": trainee.id})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "already_enrolled");
    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/program-versions/{version_id}/enrollments"),
        Some(&admin),
        Some(serde_json::json!({"user_id": 4242})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "no_such_user");
    let (status, body) = request(
        fx.app(),
        "POST",
        "/api/program-versions/4242/enrollments",
        Some(&admin),
        Some(serde_json::json!({"user_id": trainee.id})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "no_such_version");

    // A user without assign_training can neither enroll nor read.
    let plain = users::create_with_reset_code(
        &fx.pool,
        fx.admin_id,
        "casey.caller",
        "Casey Caller",
        "",
        "",
        RoleBundle::Trainee,
    )
    .await
    .expect("create")
    .expect("accepted");
    let refused = enrollments::enroll(&fx.pool, plain.id, version_id, plain.id)
        .await
        .expect("call");
    assert_eq!(refused, Err(enrollments::EnrollRefusal::CapabilityRequired));
    let refused = enrollments::list_for_version(&fx.pool, plain.id, version_id)
        .await
        .expect("call");
    assert_eq!(refused, Err(enrollments::EnrollRefusal::CapabilityRequired));

    // The pin cannot be silently repointed at the database.
    let second_version = {
        let vid = programs::create_version(&fx.pool, fx.admin_id, program_id, &complete_content())
            .await
            .expect("create version")
            .expect("accepted");
        programs::publish_version(&fx.pool, fx.admin_id, vid)
            .await
            .expect("publish")
            .expect("accepted");
        vid
    };
    let repoint = sqlx::query("UPDATE enrollment SET program_version_id = ?1 WHERE id = ?2")
        .bind(second_version)
        .bind(enrollment_id)
        .execute(&fx.pool)
        .await;
    let err = repoint
        .expect_err("repointing must be rejected")
        .to_string();
    assert!(
        err.contains("recorded event"),
        "trigger must refuse an unmediated repoint: {err}"
    );
}

#[tokio::test]
async fn milestone_two_exit_publish_enroll_export_reproduce() {
    // Publish a complete invented program and enroll a trainee in it.
    let source = Fixture::new().await;
    let (_, version_id) = source.published_program(&complete_content()).await;
    let trainee = users::create_with_reset_code(
        &source.pool,
        source.admin_id,
        "jordan.trainee",
        "Jordan Trainee",
        "",
        "",
        RoleBundle::Trainee,
    )
    .await
    .expect("create")
    .expect("accepted");
    enrollments::enroll(&source.pool, source.admin_id, version_id, trainee.id)
        .await
        .expect("call")
        .expect("enrolled");

    // Export it, and reproduce it byte-for-byte on a clean installation.
    let exported = program_export::export_version(&source.pool, version_id)
        .await
        .expect("export")
        .expect("exists");
    let target = Fixture::new().await;
    let imported_id = program_export::import_version(
        &target.pool,
        target.admin_id,
        &exported,
        ImportTarget::NewProgram,
    )
    .await
    .expect("import")
    .expect("accepted");
    let re_exported = program_export::export_version(&target.pool, imported_id)
        .await
        .expect("export")
        .expect("exists");
    assert_eq!(exported, re_exported, "reproduction must be byte-identical");

    // The reproduced program publishes and enrolls on the clean install.
    programs::publish_version(&target.pool, target.admin_id, imported_id)
        .await
        .expect("publish")
        .expect("accepted");
    let target_trainee = users::create_with_reset_code(
        &target.pool,
        target.admin_id,
        "rowan.trainee",
        "Rowan Trainee",
        "",
        "",
        RoleBundle::Trainee,
    )
    .await
    .expect("create")
    .expect("accepted");
    enrollments::enroll(
        &target.pool,
        target.admin_id,
        imported_id,
        target_trainee.id,
    )
    .await
    .expect("call")
    .expect("enrolled");
    let enrollees = enrollments::list_for_version(&target.pool, target.admin_id, imported_id)
        .await
        .expect("call")
        .expect("listed");
    assert_eq!(enrollees.len(), 1);
    assert_eq!(enrollees[0].display_name, "Rowan Trainee");
}
