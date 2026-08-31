//! Milestone 3 slice 1: role bundles and profile fields, training
//! assignments with scoped reads and notices, enrollment lifecycle events
//! with database-mediated version changes, and phase history validated
//! against the pinned transition graph. Every fixture is invented.

use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, SET_COOKIE};
use axum::http::{Request, StatusCode};
use consolebook_server::capabilities::RoleBundle;
use consolebook_server::data_dir::DataDir;
use consolebook_server::lifecycle::{
    self, EnrollmentEventKind, EnrollmentStatus, LifecycleRefusal, PhaseEventKind,
};
use consolebook_server::programs::{
    self, CompetencyDef, PhaseDef, PolicyDef, ScaleDef, ScaleKind, TaskDef, TransitionDef,
    TransitionKind, VersionContent,
};
use consolebook_server::{assignments, enrollments, notices, setup, storage, users};
use http_body_util::BodyExt;
use tower::ServiceExt;

const PASSWORD: &str = "invented-passphrase-1";

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

    /// Creates a user with `role`, sets a password through the standard
    /// reset flow, and returns their id.
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

    /// Publishes `content` as a new version of `program_id` (creating the
    /// program when `None`); returns (program id, version id).
    async fn publish(&self, program_id: Option<i64>, content: &VersionContent) -> (i64, i64) {
        let program_id = match program_id {
            Some(id) => id,
            None => programs::create_program(&self.pool, self.admin_id, &content.name)
                .await
                .expect("create program")
                .expect("accepted"),
        };
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

    /// The pinned version's phase id for `name`.
    async fn phase_id(&self, version_id: i64, name: &str) -> i64 {
        sqlx::query_scalar("SELECT id FROM phase WHERE program_version_id = ?1 AND name = ?2")
            .bind(version_id)
            .bind(name)
            .fetch_one(&self.pool)
            .await
            .expect("phase id")
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

/// Three phases with an advance chain, a remediation loop, a skip, and a
/// restart — enough graph to prove every edge-kind rule.
fn phased_content() -> VersionContent {
    VersionContent {
        name: "Example County CTO Program".to_owned(),
        label: "2026 rev A".to_owned(),
        description: "Invented program for lifecycle tests.".to_owned(),
        phases: vec![
            PhaseDef {
                name: "Phase One".to_owned(),
                description: "Observation.".to_owned(),
                presentation_number: 1,
            },
            PhaseDef {
                name: "Phase Two".to_owned(),
                description: "Guided performance.".to_owned(),
                presentation_number: 2,
            },
            PhaseDef {
                name: "Phase Three".to_owned(),
                description: "Independent performance.".to_owned(),
                presentation_number: 3,
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
                to_phase: "Phase Three".to_owned(),
                kind: TransitionKind::Advance,
            },
            TransitionDef {
                from_phase: "Phase Two".to_owned(),
                to_phase: "Phase One".to_owned(),
                kind: TransitionKind::Remediation,
            },
            TransitionDef {
                from_phase: "Phase One".to_owned(),
                to_phase: "Phase Three".to_owned(),
                kind: TransitionKind::Skip,
            },
            TransitionDef {
                from_phase: "Phase Three".to_owned(),
                to_phase: "Phase One".to_owned(),
                kind: TransitionKind::Restart,
            },
        ],
        competencies: vec![CompetencyDef {
            category: String::new(),
            name: "Emergency Call Interrogation".to_owned(),
            description: "Obtains and verifies location, callback, and nature.".to_owned(),
            tasks: vec![TaskDef {
                prompt: "Processes an invented structure-fire call.".to_owned(),
                citations: Vec::new(),
            }],
            citations: Vec::new(),
        }],
        rating_scales: vec![ScaleDef {
            name: "Narrative Assessment".to_owned(),
            kind: ScaleKind::NarrativeOnly,
            min_value: None,
            max_value: None,
            anchors: Vec::new(),
        }],
        rating_modifiers: Vec::new(),
        evaluation_forms: Vec::new(),
        citations: Vec::new(),
        finalization_policy: PolicyDef::default(),
    }
}

#[tokio::test]
async fn role_bundles_grant_capabilities_and_profile_fields_list() {
    let fx = Fixture::new().await;
    let admin = fx.login("avery.admin", PASSWORD).await;

    // Role and profile fields ride user creation over the API.
    let (status, body) = request(
        fx.app(),
        "POST",
        "/api/users",
        Some(&admin),
        Some(serde_json::json!({
            "username": "casey.coordinator",
            "display_name": "Casey Coordinator",
            "employee_id": "C-12",
            "title": "Training Coordinator",
            "role": "coordinator",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create coordinator: {body}");
    let reset_code = body["reset_code"].as_str().expect("code").to_owned();
    let (status, _) = request(
        fx.app(),
        "POST",
        "/api/auth/reset",
        None,
        Some(serde_json::json!({
            "username": "casey.coordinator",
            "reset_code": reset_code,
            "new_password": PASSWORD,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let coordinator = fx.login("casey.coordinator", PASSWORD).await;
    let (status, body) = request(
        fx.app(),
        "GET",
        "/api/auth/session",
        Some(&coordinator),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let held: Vec<&str> = body["capabilities"]
        .as_array()
        .expect("capabilities")
        .iter()
        .map(|value| value.as_str().expect("string"))
        .collect();
    assert_eq!(
        held,
        vec![
            "assign_training",
            "review_evaluation",
            "view_assigned_records"
        ],
        "the Coordinator bundle and nothing else"
    );

    // Trainer bundle at the service; trainee default grants nothing.
    let jordan_id = fx
        .user_with_role("jordan.trainer", "Jordan Trainer", RoleBundle::Trainer)
        .await;
    let jordan_caps: Vec<String> =
        consolebook_server::capabilities::list_for_user(&fx.pool, jordan_id)
            .await
            .expect("list");
    assert_eq!(
        jordan_caps,
        vec!["author_evaluation", "view_assigned_records"]
    );
    let taylor_id = fx
        .user_with_role("taylor.trainee", "Taylor Trainee", RoleBundle::Trainee)
        .await;
    let taylor_caps: Vec<String> =
        consolebook_server::capabilities::list_for_user(&fx.pool, taylor_id)
            .await
            .expect("list");
    assert_eq!(
        taylor_caps,
        vec!["acknowledge_own_record", "view_own_records"],
        "the Trainee bundle grants the own-record capabilities (slice 2)"
    );

    // The roster presents the profile fields.
    let (status, body) = request(fx.app(), "GET", "/api/users", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    let coordinator_row = body["users"]
        .as_array()
        .expect("users")
        .iter()
        .find(|row| row["username"] == "casey.coordinator")
        .expect("coordinator listed");
    assert_eq!(coordinator_row["employee_id"], "C-12");
    assert_eq!(coordinator_row["title"], "Training Coordinator");
    assert_eq!(
        coordinator_row["capabilities"],
        serde_json::json!([
            "assign_training",
            "review_evaluation",
            "view_assigned_records"
        ]),
        "the roster presents held capabilities for eligibility pickers"
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn assignments_scope_reads_notify_and_end() {
    let fx = Fixture::new().await;
    let (_, version_id) = fx.publish(None, &phased_content()).await;
    let taylor_id = fx
        .user_with_role("taylor.trainee", "Taylor Trainee", RoleBundle::Trainee)
        .await;
    let jordan_id = fx
        .user_with_role("jordan.trainer", "Jordan Trainer", RoleBundle::Trainer)
        .await;
    fx.user_with_role("rowan.trainer", "Rowan Trainer", RoleBundle::Trainer)
        .await;
    let enrollment_id = enrollments::enroll(&fx.pool, fx.admin_id, version_id, taylor_id)
        .await
        .expect("call")
        .expect("enrolled");

    // A trainer cannot assign; the coordinator-capable admin can, once.
    let refused = assignments::create(&fx.pool, jordan_id, enrollment_id, jordan_id)
        .await
        .expect("call");
    assert_eq!(refused, Err(assignments::AssignRefusal::CapabilityRequired));
    // An assignment grants scoped reads, so its trainer must be able to
    // read: a capability-less user is refused, and the notice naming the
    // trainee therefore only ever reaches view_assigned_records holders.
    let refused = assignments::create(&fx.pool, fx.admin_id, enrollment_id, taylor_id)
        .await
        .expect("call");
    assert_eq!(
        refused,
        Err(assignments::AssignRefusal::TrainerLacksCapability)
    );
    let assignment_id = assignments::create(&fx.pool, fx.admin_id, enrollment_id, jordan_id)
        .await
        .expect("call")
        .expect("assigned");
    let duplicate = assignments::create(&fx.pool, fx.admin_id, enrollment_id, jordan_id)
        .await
        .expect("call");
    assert_eq!(duplicate, Err(assignments::AssignRefusal::AlreadyAssigned));

    // The trainer is notified inside the same action.
    let trainer_notices = notices::list_for_user(&fx.pool, jordan_id)
        .await
        .expect("notices");
    assert!(
        trainer_notices
            .iter()
            .any(|notice| notice.kind == "assignment_created"
                && notice.message.contains("Taylor Trainee")),
        "assignment must notify the trainer: {trainer_notices:?}"
    );

    // Scoped reads: the assigned trainer sees the enrollment; an
    // unassigned trainer and the capability-less trainee do not.
    let jordan = fx.login("jordan.trainer", PASSWORD).await;
    let other = fx.login("rowan.trainer", PASSWORD).await;
    let taylor = fx.login("taylor.trainee", PASSWORD).await;
    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/enrollments/{enrollment_id}"),
        Some(&jordan),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "assigned trainer reads: {body}");
    assert_eq!(body["trainee_username"], "taylor.trainee");
    assert_eq!(body["status"], "active");
    let (status, _) = request(
        fx.app(),
        "GET",
        &format!("/api/enrollments/{enrollment_id}"),
        Some(&other),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "unassigned trainer refused");
    let (status, _) = request(
        fx.app(),
        "GET",
        &format!("/api/enrollments/{enrollment_id}"),
        Some(&taylor),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "trainee refused until Milestone 4"
    );
    // Being assigned is not enough to read trainee identities: the mine
    // listing takes view_assigned_records like every other scoped read.
    let (status, _) = request(
        fx.app(),
        "GET",
        "/api/assignments/mine",
        Some(&taylor),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "mine listing requires view_assigned_records"
    );

    // "My trainees" lists the active assignment, and ending it closes
    // both the list and the scoped read.
    let (status, body) = request(
        fx.app(),
        "GET",
        "/api/assignments/mine",
        Some(&jordan),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["assignments"][0]["trainee_username"], "taylor.trainee");
    assert_eq!(
        body["assignments"][0]["enrollment_id"],
        serde_json::json!(enrollment_id)
    );
    assignments::end(&fx.pool, fx.admin_id, assignment_id)
        .await
        .expect("call")
        .expect("ended");
    let again = assignments::end(&fx.pool, fx.admin_id, assignment_id)
        .await
        .expect("call");
    assert_eq!(again, Err(assignments::AssignRefusal::AlreadyEnded));
    let (status, body) = request(
        fx.app(),
        "GET",
        "/api/assignments/mine",
        Some(&jordan),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["assignments"].as_array().expect("array").is_empty());
    let (status, _) = request(
        fx.app(),
        "GET",
        &format!("/api/enrollments/{enrollment_id}"),
        Some(&jordan),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "ended assignment reads nothing"
    );

    // Assignments are audited with the trainer as the person concerned.
    let audited: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_event
         WHERE kind = 'assignment_created' AND actor_user_id = ?1
           AND subject_user_id = ?2 AND subject_kind = 'assignment' AND subject_id = ?3",
    )
    .bind(fx.admin_id)
    .bind(jordan_id)
    .bind(assignment_id)
    .fetch_one(&fx.pool)
    .await
    .expect("count");
    assert_eq!(audited, 1);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn enrollment_lifecycle_events_mediate_version_changes() {
    let fx = Fixture::new().await;
    let (program_id, version_id) = fx.publish(None, &phased_content()).await;
    let taylor_id = fx
        .user_with_role("taylor.trainee", "Taylor Trainee", RoleBundle::Trainee)
        .await;
    let enrollment_id = enrollments::enroll(&fx.pool, fx.admin_id, version_id, taylor_id)
        .await
        .expect("call")
        .expect("enrolled");

    // Withdraw needs a reason; withdrawn enrollments refuse training work.
    let refused = lifecycle::record_enrollment_event(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        EnrollmentEventKind::Withdraw,
        "  ",
        None,
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(LifecycleRefusal::ReasonRequired));
    lifecycle::record_enrollment_event(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        EnrollmentEventKind::Withdraw,
        "Separated from the invented agency.",
        None,
    )
    .await
    .expect("call")
    .expect("recorded");
    let mut conn = fx.pool.acquire().await.expect("conn");
    assert_eq!(
        lifecycle::status(&mut conn, enrollment_id)
            .await
            .expect("status"),
        Some(EnrollmentStatus::Withdrawn)
    );
    drop(conn);
    let refused = lifecycle::record_phase_event(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        PhaseEventKind::Advance,
        Some(fx.phase_id(version_id, "Phase One").await),
        None,
        "",
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(LifecycleRefusal::NotActive));
    let refused = assignments::create(&fx.pool, fx.admin_id, enrollment_id, fx.admin_id)
        .await
        .expect("call");
    assert_eq!(refused, Err(assignments::AssignRefusal::EnrollmentInactive));

    // Reinstate, complete, and reinstate again walk the status machine.
    lifecycle::record_enrollment_event(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        EnrollmentEventKind::Reinstate,
        "Rehired for the invented fall academy.",
        None,
    )
    .await
    .expect("call")
    .expect("recorded");
    let refused = lifecycle::record_enrollment_event(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        EnrollmentEventKind::Reinstate,
        "Twice.",
        None,
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(LifecycleRefusal::AlreadyActive));
    lifecycle::record_enrollment_event(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        EnrollmentEventKind::Complete,
        "",
        None,
    )
    .await
    .expect("call")
    .expect("recorded");
    let mut conn = fx.pool.acquire().await.expect("conn");
    assert_eq!(
        lifecycle::status(&mut conn, enrollment_id)
            .await
            .expect("status"),
        Some(EnrollmentStatus::Completed)
    );
    drop(conn);
    lifecycle::record_enrollment_event(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        EnrollmentEventKind::Reinstate,
        "Program change requires an active enrollment.",
        None,
    )
    .await
    .expect("call")
    .expect("recorded");

    // Version changes are modeled events that repoint the pin.
    let (_, second_version) = fx.publish(Some(program_id), &phased_content()).await;
    let refused = lifecycle::record_enrollment_event(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        EnrollmentEventKind::VersionChange,
        "Moving to the fall revision.",
        Some(version_id),
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(LifecycleRefusal::SameVersion));
    let draft_version =
        programs::create_version(&fx.pool, fx.admin_id, program_id, &phased_content())
            .await
            .expect("create version")
            .expect("accepted");
    let refused = lifecycle::record_enrollment_event(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        EnrollmentEventKind::VersionChange,
        "Drafts never pin.",
        Some(draft_version),
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(LifecycleRefusal::NotPublished));
    lifecycle::record_enrollment_event(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        EnrollmentEventKind::VersionChange,
        "Moving to the fall revision.",
        Some(second_version),
    )
    .await
    .expect("call")
    .expect("recorded");
    let pinned: i64 = sqlx::query_scalar("SELECT program_version_id FROM enrollment WHERE id = ?1")
        .bind(enrollment_id)
        .fetch_one(&fx.pool)
        .await
        .expect("pin");
    assert_eq!(pinned, second_version, "the event repointed the pin");

    // A version change stays within the continuing program…
    let mut other_program = phased_content();
    other_program.name = "Example County Dispatcher Academy".to_owned();
    let (_, foreign_version) = fx.publish(None, &other_program).await;
    let refused = lifecycle::record_enrollment_event(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        EnrollmentEventKind::VersionChange,
        "Wrong continuing program.",
        Some(foreign_version),
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(LifecycleRefusal::DifferentProgram));
    // …and cannot collide with another enrollment's pin: a typed
    // refusal, never a constraint violation surfacing as a 500.
    let (_, third_version) = fx.publish(Some(program_id), &phased_content()).await;
    enrollments::enroll(&fx.pool, fx.admin_id, third_version, taylor_id)
        .await
        .expect("call")
        .expect("enrolled");
    let refused = lifecycle::record_enrollment_event(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        EnrollmentEventKind::VersionChange,
        "Target collision.",
        Some(third_version),
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(LifecycleRefusal::TargetAlreadyEnrolled));

    // The database refuses an unmediated repoint and any event edit.
    let repoint = sqlx::query("UPDATE enrollment SET program_version_id = ?1 WHERE id = ?2")
        .bind(version_id)
        .bind(enrollment_id)
        .execute(&fx.pool)
        .await;
    let err = repoint.expect_err("must be refused").to_string();
    assert!(err.contains("recorded event"), "unmediated repoint: {err}");
    let edit =
        sqlx::query("UPDATE enrollment_event SET reason = 'rewritten' WHERE enrollment_id = ?1")
            .bind(enrollment_id)
            .execute(&fx.pool)
            .await;
    let err = edit.expect_err("must be refused").to_string();
    assert!(err.contains("append-only"), "event edit: {err}");
    let delete = sqlx::query("DELETE FROM enrollment_event WHERE enrollment_id = ?1")
        .bind(enrollment_id)
        .execute(&fx.pool)
        .await;
    let err = delete.expect_err("must be refused").to_string();
    assert!(err.contains("append-only"), "event delete: {err}");

    // Every lifecycle action left an attributable audit event.
    for kind in [
        "enrollment_withdrawn",
        "enrollment_completed",
        "enrollment_reinstated",
        "enrollment_version_changed",
    ] {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_event
             WHERE kind = ?1 AND actor_user_id = ?2 AND subject_user_id = ?3
               AND subject_kind = 'enrollment' AND subject_id = ?4",
        )
        .bind(kind)
        .bind(fx.admin_id)
        .bind(taylor_id)
        .bind(enrollment_id)
        .fetch_one(&fx.pool)
        .await
        .expect("count");
        assert!(count >= 1, "{kind} must be audited");
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn phase_history_validates_graph_pause_and_effective_order() {
    let fx = Fixture::new().await;
    let (program_id, version_id) = fx.publish(None, &phased_content()).await;
    let taylor_id = fx
        .user_with_role("taylor.trainee", "Taylor Trainee", RoleBundle::Trainee)
        .await;
    let enrollment_id = enrollments::enroll(&fx.pool, fx.admin_id, version_id, taylor_id)
        .await
        .expect("call")
        .expect("enrolled");
    let one = fx.phase_id(version_id, "Phase One").await;
    let two = fx.phase_id(version_id, "Phase Two").await;
    let three = fx.phase_id(version_id, "Phase Three").await;
    let now = time::OffsetDateTime::now_utc().unix_timestamp();

    let record = |kind, to, effective, reason: &'static str| {
        lifecycle::record_phase_event(
            &fx.pool,
            fx.admin_id,
            enrollment_id,
            kind,
            to,
            effective,
            reason,
        )
    };

    // Return and restart need a current phase and a reason; entry is an
    // advance from nowhere, honestly backdated to shift start.
    let refused = record(PhaseEventKind::Return, Some(one), None, "Too early.")
        .await
        .expect("call");
    assert_eq!(refused, Err(LifecycleRefusal::NoCurrentPhase));
    let refused = record(PhaseEventKind::Advance, Some(one), Some(now + 3600), "")
        .await
        .expect("call");
    assert_eq!(refused, Err(LifecycleRefusal::EffectiveInFuture));
    record(PhaseEventKind::Advance, Some(one), Some(now - 500), "")
        .await
        .expect("call")
        .expect("entered");

    // Backfill cannot interleave before recorded history.
    let refused = record(PhaseEventKind::Advance, Some(two), Some(now - 900), "")
        .await
        .expect("call");
    assert_eq!(refused, Err(LifecycleRefusal::OutOfOrder));

    // Graph rules: advance follows advance or skip edges, return follows
    // remediation, restart follows restart; anything else is refused.
    let refused = record(PhaseEventKind::Return, Some(two), None, "Wrong direction.")
        .await
        .expect("call");
    assert_eq!(refused, Err(LifecycleRefusal::TransitionNotAllowed));
    record(PhaseEventKind::Advance, Some(three), None, "")
        .await
        .expect("call")
        .expect("skip edge advances");
    let refused = record(PhaseEventKind::Restart, Some(one), None, "")
        .await
        .expect("call");
    assert_eq!(refused, Err(LifecycleRefusal::ReasonRequired));
    record(
        PhaseEventKind::Restart,
        Some(one),
        None,
        "Restarting after invented leave.",
    )
    .await
    .expect("call")
    .expect("restart edge");
    record(PhaseEventKind::Advance, Some(two), None, "")
        .await
        .expect("call")
        .expect("advance edge");
    record(
        PhaseEventKind::Return,
        Some(one),
        None,
        "Invented remediation plan.",
    )
    .await
    .expect("call")
    .expect("remediation edge");

    // The pause machine blocks phase changes until resume.
    record(
        PhaseEventKind::Pause,
        None,
        None,
        "Invented military leave.",
    )
    .await
    .expect("call")
    .expect("paused");
    let refused = record(PhaseEventKind::Pause, None, None, "")
        .await
        .expect("call");
    assert_eq!(refused, Err(LifecycleRefusal::AlreadyPaused));
    let refused = record(PhaseEventKind::Advance, Some(two), None, "")
        .await
        .expect("call");
    assert_eq!(refused, Err(LifecycleRefusal::Paused));
    record(PhaseEventKind::Resume, None, None, "")
        .await
        .expect("call")
        .expect("resumed");
    let refused = record(PhaseEventKind::Resume, None, None, "")
        .await
        .expect("call");
    assert_eq!(refused, Err(LifecycleRefusal::NotPaused));

    // A target outside the pinned version is refused by the service and,
    // for a raw write, by the database.
    let (_, second_version) = fx.publish(Some(program_id), &phased_content()).await;
    let foreign_phase = fx.phase_id(second_version, "Phase Two").await;
    let refused = record(PhaseEventKind::Advance, Some(foreign_phase), None, "")
        .await
        .expect("call");
    assert_eq!(refused, Err(LifecycleRefusal::NoSuchPhase));
    let raw = sqlx::query(
        "INSERT INTO phase_event
             (enrollment_id, kind, from_phase_id, to_phase_id, effective_at, recorded_at, actor_user_id, reason)
         VALUES (?1, 'advance', NULL, ?2, ?3, ?3, NULL, '')",
    )
    .bind(enrollment_id)
    .bind(foreign_phase)
    .bind(now)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("pinned version"), "foreign phase: {err}");
    let edit = sqlx::query("UPDATE phase_event SET reason = 'rewritten' WHERE enrollment_id = ?1")
        .bind(enrollment_id)
        .execute(&fx.pool)
        .await;
    let err = edit.expect_err("must be refused").to_string();
    assert!(err.contains("append-only"), "phase event edit: {err}");

    // The API records completion and the detail tells the whole story,
    // with effective and recorded instants preserved separately.
    let admin = fx.login("avery.admin", PASSWORD).await;
    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/enrollments/{enrollment_id}/phase-events"),
        Some(&admin),
        Some(serde_json::json!({ "kind": "complete" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "complete: {body}");
    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/enrollments/{enrollment_id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["current_phase_name"], "Phase One");
    assert_eq!(body["paused"], false);
    let phase_events = body["phase_events"].as_array().expect("phase events");
    let entry = &phase_events[0];
    assert_eq!(entry["kind"], "advance");
    assert!(entry["from_phase_id"].is_null(), "entry comes from nowhere");
    assert_eq!(entry["to_phase_name"], "Phase One");
    assert_eq!(entry["effective_at"], serde_json::json!(now - 500));
    assert!(
        entry["recorded_at"].as_i64().expect("recorded") >= now,
        "recorded stays honest while effective backfills"
    );
    let last = phase_events.last().expect("complete event");
    assert_eq!(last["kind"], "complete");
    assert_eq!(last["from_phase_name"], "Phase One");
    assert_eq!(body["phases"].as_array().expect("phases").len(), 3);
    assert!(
        body["transitions"].as_array().expect("transitions").len() >= 5,
        "the pinned graph rides the detail"
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn version_change_opens_a_fresh_phase_epoch() {
    let fx = Fixture::new().await;
    let (program_id, v1) = fx.publish(None, &phased_content()).await;
    let taylor_id = fx
        .user_with_role("taylor.trainee", "Taylor Trainee", RoleBundle::Trainee)
        .await;
    let enrollment_id = enrollments::enroll(&fx.pool, fx.admin_id, v1, taylor_id)
        .await
        .expect("call")
        .expect("enrolled");
    let one_v1 = fx.phase_id(v1, "Phase One").await;

    // Enter Phase One and pause under the original pin, honestly
    // backdated so the epoch boundary is later distinguishable from
    // plain effective ordering.
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    lifecycle::record_phase_event(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        PhaseEventKind::Advance,
        Some(one_v1),
        Some(now - 5000),
        "",
    )
    .await
    .expect("call")
    .expect("entered");
    lifecycle::record_phase_event(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        PhaseEventKind::Pause,
        None,
        Some(now - 4000),
        "Invented military leave.",
    )
    .await
    .expect("call")
    .expect("paused");

    // Change to v2 and straight back to v1, recording nothing between.
    let (_, v2) = fx.publish(Some(program_id), &phased_content()).await;
    for (target, reason) in [(v2, "Trying the fall revision."), (v1, "Reverting.")] {
        lifecycle::record_enrollment_event(
            &fx.pool,
            fx.admin_id,
            enrollment_id,
            EnrollmentEventKind::VersionChange,
            reason,
            Some(target),
        )
        .await
        .expect("call")
        .expect("recorded");
    }

    // The old v1 epoch must not resurrect: no current phase, not paused,
    // and phase work requires re-entry.
    let detail = lifecycle::enrollment_detail(&fx.pool, fx.admin_id, enrollment_id)
        .await
        .expect("call")
        .expect("read");
    assert_eq!(
        detail.current_phase_id, None,
        "phase state must not survive a version change, even back to the same version"
    );
    assert!(!detail.paused, "pause must not survive a version change");
    let refused = lifecycle::record_phase_event(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        PhaseEventKind::Pause,
        None,
        None,
        "",
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(LifecycleRefusal::NoCurrentPhase));
    // Re-entry cannot take effect before the version change that opened
    // the epoch: later than every phase event, still refused.
    let refused = lifecycle::record_phase_event(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        PhaseEventKind::Advance,
        Some(one_v1),
        Some(now - 1000),
        "",
    )
    .await
    .expect("call");
    assert_eq!(
        refused,
        Err(LifecycleRefusal::OutOfOrder),
        "an event cannot predate its epoch"
    );
    lifecycle::record_phase_event(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        PhaseEventKind::Advance,
        Some(one_v1),
        None,
        "",
    )
    .await
    .expect("call")
    .expect("re-entered the fresh epoch");

    // The database stamps epochs itself: a raw insert claiming the
    // original epoch is refused.
    let raw = sqlx::query(
        "INSERT INTO phase_event
             (enrollment_id, kind, from_phase_id, to_phase_id, effective_at,
              recorded_at, actor_user_id, reason, version_change_event_id)
         VALUES (?1, 'pause', ?2, NULL, ?3, ?3, NULL, '', NULL)",
    )
    .bind(enrollment_id)
    .bind(one_v1)
    .bind(now)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("pin epoch"), "stale epoch: {err}");
}
