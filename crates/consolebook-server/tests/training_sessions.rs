//! Milestone 3 slice 2: training sessions with explicit time semantics —
//! verbatim local representation beside server-resolved UTC instants,
//! database-enforced interval invariants, trainer membership, and
//! capability-plus-scope gates. Every fixture is invented.

use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, SET_COOKIE};
use axum::http::{Request, StatusCode};
use consolebook_server::capabilities::RoleBundle;
use consolebook_server::lifecycle::{self, EnrollmentEventKind};
use consolebook_server::programs::{
    self, CompetencyDef, PhaseDef, ScaleDef, ScaleKind, TaskDef, TransitionDef, TransitionKind,
    VersionContent,
};
use consolebook_server::training_sessions::{
    self, Disposition, SessionInput, SessionRefusal, SessionUpdate,
};
use consolebook_server::{
    assignments, data_dir::DataDir, enrollments, session_membership, setup, storage, users,
};
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

fn phased_content() -> VersionContent {
    VersionContent {
        name: "Example County CTO Program".to_owned(),
        label: "2026 rev A".to_owned(),
        description: "Invented program for session tests.".to_owned(),
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
        ],
        phase_transitions: vec![TransitionDef {
            from_phase: "Phase One".to_owned(),
            to_phase: "Phase Two".to_owned(),
            kind: TransitionKind::Advance,
        }],
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
    }
}

fn input(
    business_date: &str,
    timezone: &str,
    local_start: &str,
    local_end: Option<&str>,
    disposition: Option<Disposition>,
    trainers: Vec<i64>,
) -> SessionInput {
    SessionInput {
        business_date: business_date.to_owned(),
        timezone: timezone.to_owned(),
        local_start: local_start.to_owned(),
        local_end: local_end.map(str::to_owned),
        disposition,
        phase_id: None,
        trainer_user_ids: trainers,
    }
}

/// The instant a fixed-offset RFC 3339 string names — independent of the
/// IANA rules under test.
fn instant(rfc3339: &str) -> i64 {
    rfc3339
        .parse::<jiff::Timestamp>()
        .expect("timestamp")
        .as_second()
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn local_times_store_verbatim_and_resolve_per_adr_0009() {
    let fx = Fixture::new().await;
    let (_, version_id) = fx.publish(None, &phased_content()).await;
    let taylor_id = fx
        .user_with_role("taylor.trainee", "Taylor Trainee", RoleBundle::Trainee)
        .await;
    let jordan_id = fx
        .user_with_role("jordan.trainer", "Jordan Trainer", RoleBundle::Trainer)
        .await;
    let enrollment_id = enrollments::enroll(&fx.pool, fx.admin_id, version_id, taylor_id)
        .await
        .expect("call")
        .expect("enrolled");

    // Spring-forward gap: 02:30 does not exist on 2026-03-08 in
    // America/New_York; the compatible rule rolls it forward to 03:30 EDT.
    let session_id = training_sessions::create(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        &input(
            "2026-03-08",
            "America/New_York",
            "2026-03-08T02:30",
            Some("2026-03-08T07:00"),
            Some(Disposition::Completed),
            vec![jordan_id],
        ),
    )
    .await
    .expect("call")
    .expect("created");
    let sessions = training_sessions::list_for_enrollment(&fx.pool, fx.admin_id, enrollment_id)
        .await
        .expect("call")
        .expect("listed");
    let gap = sessions
        .iter()
        .find(|session| session.id == session_id)
        .expect("session listed");
    assert_eq!(gap.local_start, "2026-03-08T02:30", "stored verbatim");
    assert_eq!(gap.utc_start, instant("2026-03-08T03:30:00-04:00"));
    assert_eq!(gap.utc_end, Some(instant("2026-03-08T07:00:00-04:00")));
    assert_eq!(gap.disposition.as_deref(), Some("completed"));
    assert_eq!(gap.trainers.len(), 1);
    assert_eq!(gap.trainers[0].username, "jordan.trainer");

    // Fall-back fold: 01:30 happens twice on 2026-11-01; the compatible
    // rule takes the earlier offset (EDT).
    training_sessions::create(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        &input(
            "2026-11-01",
            "America/New_York",
            "2026-11-01T01:30",
            Some("2026-11-01T05:00"),
            Some(Disposition::Completed),
            vec![jordan_id],
        ),
    )
    .await
    .expect("call")
    .expect("created");
    let sessions = training_sessions::list_for_enrollment(&fx.pool, fx.admin_id, enrollment_id)
        .await
        .expect("call")
        .expect("listed");
    let fold = sessions
        .iter()
        .find(|session| session.local_start == "2026-11-01T01:30")
        .expect("fold session");
    assert_eq!(fold.utc_start, instant("2026-11-01T01:30:00-04:00"));

    // Invariant 8: a second session on the same trainee and business
    // date is legal, and a contiguous one may start exactly at an end.
    training_sessions::create(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        &input(
            "2026-03-08",
            "America/New_York",
            "2026-03-08T07:00",
            Some("2026-03-08T12:00"),
            Some(Disposition::Completed),
            vec![jordan_id],
        ),
    )
    .await
    .expect("call")
    .expect("contiguous same-date session is legal");

    // Entered strings are validated, never defaulted.
    let refused = training_sessions::create(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        &input(
            "2026-02-30",
            "America/New_York",
            "2026-02-27T07:00",
            None,
            None,
            vec![jordan_id],
        ),
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(SessionRefusal::InvalidBusinessDate));
    let refused = training_sessions::create(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        &input(
            "2026-02-27",
            "Example/Invented_Zone",
            "2026-02-27T07:00",
            None,
            None,
            vec![jordan_id],
        ),
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(SessionRefusal::UnknownTimezone));
    let refused = training_sessions::create(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        &input(
            "2026-02-27",
            "America/New_York",
            "2026-02-27T07:00",
            Some("2026-02-27T06:00"),
            Some(Disposition::Completed),
            vec![jordan_id],
        ),
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(SessionRefusal::EndBeforeStart));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn overlap_is_refused_per_trainee_and_cancel_releases_the_interval() {
    let fx = Fixture::new().await;
    let (program_id, v1) = fx.publish(None, &phased_content()).await;
    let taylor_id = fx
        .user_with_role("taylor.trainee", "Taylor Trainee", RoleBundle::Trainee)
        .await;
    let jordan_id = fx
        .user_with_role("jordan.trainer", "Jordan Trainer", RoleBundle::Trainer)
        .await;
    let enrollment_id = enrollments::enroll(&fx.pool, fx.admin_id, v1, taylor_id)
        .await
        .expect("call")
        .expect("enrolled");

    // An open session is unbounded on the right…
    let open_id = training_sessions::create(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        &input(
            "2026-06-02",
            "America/Chicago",
            "2026-06-02T07:00",
            None,
            None,
            vec![jordan_id],
        ),
    )
    .await
    .expect("call")
    .expect("open session");
    // …so anything later that day overlaps it, at the service…
    let refused = training_sessions::create(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        &input(
            "2026-06-02",
            "America/Chicago",
            "2026-06-02T15:00",
            None,
            None,
            vec![jordan_id],
        ),
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(SessionRefusal::Overlap));
    // …and at the database, even across another enrollment of the same
    // trainee.
    let (_, v2) = fx.publish(Some(program_id), &phased_content()).await;
    let second_enrollment = enrollments::enroll(&fx.pool, fx.admin_id, v2, taylor_id)
        .await
        .expect("call")
        .expect("enrolled");
    let raw = sqlx::query(
        "INSERT INTO training_session
             (enrollment_id, program_version_id, business_date, timezone,
              local_start, local_end, utc_start, utc_end, phase_id, disposition,
              created_at, created_by)
         VALUES (?1, (SELECT program_version_id FROM enrollment WHERE id = ?1),
                 '2026-06-02', 'America/Chicago', '2026-06-02T15:00', NULL,
                 ?2, NULL, NULL, NULL, ?3, NULL)",
    )
    .bind(second_enrollment)
    .bind(instant("2026-06-02T15:00:00-05:00"))
    .bind(instant("2026-06-02T15:00:00-05:00"))
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("cannot overlap"), "db overlap: {err}");

    // Cancelling releases the interval; the same window then books.
    training_sessions::close(&fx.pool, fx.admin_id, open_id, Disposition::Cancelled, None)
        .await
        .expect("call")
        .expect("cancelled");
    training_sessions::create(
        &fx.pool,
        fx.admin_id,
        second_enrollment,
        &input(
            "2026-06-02",
            "America/Chicago",
            "2026-06-02T15:00",
            Some("2026-06-02T19:00"),
            Some(Disposition::Completed),
            vec![jordan_id],
        ),
    )
    .await
    .expect("call")
    .expect("cancelled interval released");

    // End-before-start is refused raw as well (invariant 6).
    let raw = sqlx::query(
        "INSERT INTO training_session
             (enrollment_id, program_version_id, business_date, timezone,
              local_start, local_end, utc_start, utc_end, phase_id, disposition,
              created_at, created_by, closed_at, closed_by)
         VALUES (?1, (SELECT program_version_id FROM enrollment WHERE id = ?1),
                 '2026-06-03', 'America/Chicago', '2026-06-03T07:00',
                 '2026-06-03T06:00', ?2, ?3, NULL, 'completed', ?2, NULL, ?2, NULL)",
    )
    .bind(second_enrollment)
    .bind(instant("2026-06-03T07:00:00-05:00"))
    .bind(instant("2026-06-03T06:00:00-05:00"))
    .execute(&fx.pool)
    .await;
    assert!(
        raw.is_err(),
        "utc end preceding start must violate the schema"
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn session_gates_membership_and_close_rules() {
    let fx = Fixture::new().await;
    let (_, version_id) = fx.publish(None, &phased_content()).await;
    let taylor_id = fx
        .user_with_role("taylor.trainee", "Taylor Trainee", RoleBundle::Trainee)
        .await;
    let jordan_id = fx
        .user_with_role("jordan.trainer", "Jordan Trainer", RoleBundle::Trainer)
        .await;
    let rowan_id = fx
        .user_with_role("rowan.trainer", "Rowan Trainer", RoleBundle::Trainer)
        .await;
    let enrollment_id = enrollments::enroll(&fx.pool, fx.admin_id, version_id, taylor_id)
        .await
        .expect("call")
        .expect("enrolled");

    // An unassigned trainer cannot create; an assigned one records their
    // own session and becomes its member.
    let refused = training_sessions::create(
        &fx.pool,
        jordan_id,
        enrollment_id,
        &input(
            "2026-06-02",
            "America/Chicago",
            "2026-06-02T07:00",
            None,
            None,
            Vec::new(),
        ),
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(SessionRefusal::CapabilityRequired));
    assignments::create(&fx.pool, fx.admin_id, enrollment_id, jordan_id)
        .await
        .expect("call")
        .expect("assigned");
    let session_id = training_sessions::create(
        &fx.pool,
        jordan_id,
        enrollment_id,
        &input(
            "2026-06-02",
            "America/Chicago",
            "2026-06-02T07:00",
            None,
            None,
            Vec::new(),
        ),
    )
    .await
    .expect("call")
    .expect("created");
    assert!(
        session_membership::is_member(&fx.pool, jordan_id, session_id)
            .await
            .expect("check"),
        "the recording trainer is the default member"
    );

    // Reads: a member reads the session without any assignment; the
    // enrollment history stays closed to them.
    let jordan = fx.login("jordan.trainer", PASSWORD).await;
    let rowan = fx.login("rowan.trainer", PASSWORD).await;
    let taylor = fx.login("taylor.trainee", PASSWORD).await;
    let (status, _) = request(
        fx.app(),
        "GET",
        &format!("/api/sessions/{session_id}"),
        Some(&rowan),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "unassigned non-member refused"
    );
    session_membership::add_trainer(&fx.pool, jordan_id, session_id, rowan_id)
        .await
        .expect("call")
        .expect("a member may add a handoff trainer");
    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/sessions/{session_id}"),
        Some(&rowan),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "member reads the session: {body}");
    assert_eq!(body["trainee_username"], "taylor.trainee");
    let (status, _) = request(
        fx.app(),
        "GET",
        &format!("/api/enrollments/{enrollment_id}"),
        Some(&rowan),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "session membership does not open the enrollment history"
    );
    let (status, body) = request(fx.app(), "GET", "/api/sessions/mine", Some(&rowan), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["sessions"][0]["trainee_username"], "taylor.trainee",
        "members see their own session list"
    );
    let (status, _) = request(fx.app(), "GET", "/api/sessions/mine", Some(&taylor), None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "mine requires author_evaluation"
    );

    // Membership rules: capability floor, no duplicates, coordinator-only
    // removal, and the database keeps the last trainer.
    let refused = session_membership::add_trainer(&fx.pool, jordan_id, session_id, taylor_id)
        .await
        .expect("call");
    assert_eq!(refused, Err(SessionRefusal::TrainerLacksCapability));
    let refused = session_membership::add_trainer(&fx.pool, jordan_id, session_id, rowan_id)
        .await
        .expect("call");
    assert_eq!(refused, Err(SessionRefusal::AlreadyMember));
    let refused = session_membership::remove_trainer(&fx.pool, jordan_id, session_id, rowan_id)
        .await
        .expect("call");
    assert_eq!(refused, Err(SessionRefusal::CapabilityRequired));
    session_membership::remove_trainer(&fx.pool, fx.admin_id, session_id, rowan_id)
        .await
        .expect("call")
        .expect("removed");
    let refused = session_membership::remove_trainer(&fx.pool, fx.admin_id, session_id, jordan_id)
        .await
        .expect("call");
    assert_eq!(refused, Err(SessionRefusal::LastTrainer));
    let raw = sqlx::query("DELETE FROM session_trainer WHERE session_id = ?1")
        .bind(session_id)
        .execute(&fx.pool)
        .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("at least one trainer"), "floor: {err}");
    // Membership identity never moves either: an UPDATE cannot transfer
    // access around the audited insert-and-delete path.
    let raw = sqlx::query("UPDATE session_trainer SET trainer_user_id = ?1 WHERE session_id = ?2")
        .bind(rowan_id)
        .bind(session_id)
        .execute(&fx.pool)
        .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("never edits"), "membership identity: {err}");
    // The trainee identity of an enrollment never moves either: sessions
    // and the overlap triggers derive ownership from enrollment.user_id,
    // so a raw reassignment would rewrite whose training the sessions
    // recorded and slip past the interval invariant.
    let raw = sqlx::query("UPDATE enrollment SET user_id = ?1 WHERE id = ?2")
        .bind(rowan_id)
        .bind(enrollment_id)
        .execute(&fx.pool)
        .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(
        err.contains("belongs to its trainee"),
        "enrollment identity: {err}"
    );

    // Editing an open session re-resolves UTC and stays verbatim; the
    // member closes it; closed sessions refuse further work.
    training_sessions::update_open(
        &fx.pool,
        jordan_id,
        session_id,
        &SessionUpdate {
            business_date: "2026-06-02".to_owned(),
            timezone: "America/Chicago".to_owned(),
            local_start: "2026-06-02T06:30".to_owned(),
            phase_id: None,
        },
    )
    .await
    .expect("call")
    .expect("updated");
    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/sessions/{session_id}"),
        Some(&jordan),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["local_start"], "2026-06-02T06:30");
    assert_eq!(
        body["utc_start"],
        serde_json::json!(instant("2026-06-02T06:30:00-05:00"))
    );
    let refused = training_sessions::close(
        &fx.pool,
        jordan_id,
        session_id,
        Disposition::Completed,
        None,
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(SessionRefusal::EndRequired));
    let refused = training_sessions::close(
        &fx.pool,
        jordan_id,
        session_id,
        Disposition::Cancelled,
        Some("2026-06-02T15:00"),
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(SessionRefusal::EndNotAllowed));
    training_sessions::close(
        &fx.pool,
        jordan_id,
        session_id,
        Disposition::Completed,
        Some("2026-06-02T15:00"),
    )
    .await
    .expect("call")
    .expect("closed");
    let refused = training_sessions::close(
        &fx.pool,
        jordan_id,
        session_id,
        Disposition::Completed,
        Some("2026-06-02T16:00"),
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(SessionRefusal::SessionClosed));
    let refused = training_sessions::update_open(
        &fx.pool,
        jordan_id,
        session_id,
        &SessionUpdate {
            business_date: "2026-06-02".to_owned(),
            timezone: "America/Chicago".to_owned(),
            local_start: "2026-06-02T06:00".to_owned(),
            phase_id: None,
        },
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(SessionRefusal::SessionClosed));

    // Session phases pin the enrollment's version, at the service and raw.
    let (_, foreign_version) = {
        let mut other = phased_content();
        other.name = "Example County Dispatcher Academy".to_owned();
        fx.publish(None, &other).await
    };
    let foreign_phase = fx.phase_id(foreign_version, "Phase One").await;
    let mut bad = input(
        "2026-06-04",
        "America/Chicago",
        "2026-06-04T07:00",
        None,
        None,
        vec![jordan_id],
    );
    bad.phase_id = Some(foreign_phase);
    let refused = training_sessions::create(&fx.pool, fx.admin_id, enrollment_id, &bad)
        .await
        .expect("call");
    assert_eq!(refused, Err(SessionRefusal::NoSuchPhase));
    let raw = sqlx::query(
        "INSERT INTO training_session
             (enrollment_id, program_version_id, business_date, timezone,
              local_start, local_end, utc_start, utc_end, phase_id, disposition,
              created_at, created_by)
         VALUES (?1, (SELECT program_version_id FROM enrollment WHERE id = ?1),
                 '2026-06-04', 'America/Chicago', '2026-06-04T07:00', NULL,
                 ?2, NULL, ?3, NULL, ?2, NULL)",
    )
    .bind(enrollment_id)
    .bind(instant("2026-06-04T07:00:00-05:00"))
    .bind(foreign_phase)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(
        err.contains("FOREIGN KEY"),
        "invariant 5 is a composite foreign key against the stamp: {err}"
    );

    // Sessions are created on active enrollments only.
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
    let refused = training_sessions::create(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        &input(
            "2026-06-05",
            "America/Chicago",
            "2026-06-05T07:00",
            None,
            None,
            vec![jordan_id],
        ),
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(SessionRefusal::EnrollmentInactive));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn sessions_api_round_trip() {
    let fx = Fixture::new().await;
    let (_, version_id) = fx.publish(None, &phased_content()).await;
    let taylor_id = fx
        .user_with_role("taylor.trainee", "Taylor Trainee", RoleBundle::Trainee)
        .await;
    let jordan_id = fx
        .user_with_role("jordan.trainer", "Jordan Trainer", RoleBundle::Trainer)
        .await;
    let enrollment_id = enrollments::enroll(&fx.pool, fx.admin_id, version_id, taylor_id)
        .await
        .expect("call")
        .expect("enrolled");
    let admin = fx.login("avery.admin", PASSWORD).await;

    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/enrollments/{enrollment_id}/sessions"),
        Some(&admin),
        Some(serde_json::json!({
            "business_date": "2026-06-02",
            "timezone": "America/Chicago",
            "local_start": "2026-06-02T07:00",
            "trainer_user_ids": [jordan_id],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create: {body}");
    let session_id = body["id"].as_i64().expect("id");

    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/enrollments/{enrollment_id}/sessions"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["sessions"][0]["id"], serde_json::json!(session_id));
    assert!(body["sessions"][0]["disposition"].is_null(), "open");
    assert_eq!(
        body["sessions"][0]["trainers"][0]["username"],
        "jordan.trainer"
    );

    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/sessions/{session_id}/close"),
        Some(&admin),
        Some(serde_json::json!({ "disposition": "completed" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "end_required");
    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/sessions/{session_id}/close"),
        Some(&admin),
        Some(serde_json::json!({
            "disposition": "completed",
            "local_end": "2026-06-02T15:00",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "close: {body}");
    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/sessions/{session_id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["disposition"], "completed");
    assert_eq!(body["local_end"], "2026-06-02T15:00");

    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/enrollments/{enrollment_id}/sessions"),
        Some(&admin),
        Some(serde_json::json!({
            "business_date": "2026-06-02",
            "timezone": "Example/Invented_Zone",
            "local_start": "2026-06-02T16:00",
            "trainer_user_ids": [jordan_id],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "unknown_timezone");

    // The session is audited with the trainee as the person concerned.
    let audited: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_event
         WHERE kind = 'session_created' AND actor_user_id = ?1
           AND subject_user_id = ?2 AND subject_kind = 'session' AND subject_id = ?3",
    )
    .bind(fx.admin_id)
    .bind(taylor_id)
    .bind(session_id)
    .fetch_one(&fx.pool)
    .await
    .expect("count");
    assert_eq!(audited, 1);
    // Initial members are access grants too: each one is audited, so a
    // later removal never outlives the record of the grant.
    let granted: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_event
         WHERE kind = 'session_trainer_added' AND actor_user_id = ?1
           AND subject_user_id = ?2 AND subject_kind = 'session' AND subject_id = ?3",
    )
    .bind(fx.admin_id)
    .bind(jordan_id)
    .bind(session_id)
    .fetch_one(&fx.pool)
    .await
    .expect("count");
    assert_eq!(granted, 1, "the initial trainer's grant is audited");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn blank_ends_normalize_and_historic_phases_survive_edits() {
    let fx = Fixture::new().await;
    let (program_id, v1) = fx.publish(None, &phased_content()).await;
    let taylor_id = fx
        .user_with_role("taylor.trainee", "Taylor Trainee", RoleBundle::Trainee)
        .await;
    let jordan_id = fx
        .user_with_role("jordan.trainer", "Jordan Trainer", RoleBundle::Trainer)
        .await;
    let enrollment_id = enrollments::enroll(&fx.pool, fx.admin_id, v1, taylor_id)
        .await
        .expect("call")
        .expect("enrolled");
    let one_v1 = fx.phase_id(v1, "Phase One").await;

    // A blank end is no end: with a disposition that is a typed refusal,
    // never a constraint violation surfacing as a 500…
    let refused = training_sessions::create(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        &input(
            "2026-06-02",
            "America/Chicago",
            "2026-06-02T07:00",
            Some("   "),
            Some(Disposition::Completed),
            vec![jordan_id],
        ),
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(SessionRefusal::EndRequired));
    // …a real end without a disposition is refused, not defaulted…
    let refused = training_sessions::create(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        &input(
            "2026-06-02",
            "America/Chicago",
            "2026-06-02T07:00",
            Some("2026-06-02T15:00"),
            None,
            vec![jordan_id],
        ),
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(SessionRefusal::DispositionRequired));
    // …and an empty-string end records an ordinary open session.
    let mut open_input = input(
        "2026-06-02",
        "America/Chicago",
        "2026-06-02T07:00",
        Some(""),
        None,
        vec![jordan_id],
    );
    open_input.phase_id = Some(one_v1);
    let session_id = training_sessions::create(&fx.pool, fx.admin_id, enrollment_id, &open_input)
        .await
        .expect("call")
        .expect("open session with a blank end");

    // A version change leaves the open session's historic phase intact:
    // editing other fields with the unchanged phase still succeeds.
    let (_, v2) = fx.publish(Some(program_id), &phased_content()).await;
    lifecycle::record_enrollment_event(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        EnrollmentEventKind::VersionChange,
        "Moving to the fall revision.",
        Some(v2),
    )
    .await
    .expect("call")
    .expect("recorded");
    training_sessions::update_open(
        &fx.pool,
        fx.admin_id,
        session_id,
        &SessionUpdate {
            business_date: "2026-06-02".to_owned(),
            timezone: "America/Chicago".to_owned(),
            local_start: "2026-06-02T06:30".to_owned(),
            phase_id: Some(one_v1),
        },
    )
    .await
    .expect("call")
    .expect("an unchanged historic phase survives the edit");
    // An actual phase change validates against the session's own stamped
    // version: another phase of that version is accepted, a phase of the
    // enrollment's newer pin is not.
    let two_v1 = fx.phase_id(v1, "Phase Two").await;
    training_sessions::update_open(
        &fx.pool,
        fx.admin_id,
        session_id,
        &SessionUpdate {
            business_date: "2026-06-02".to_owned(),
            timezone: "America/Chicago".to_owned(),
            local_start: "2026-06-02T06:30".to_owned(),
            phase_id: Some(two_v1),
        },
    )
    .await
    .expect("call")
    .expect("another stamped-version phase is accepted");
    let two_v2 = fx.phase_id(v2, "Phase Two").await;
    let refused = training_sessions::update_open(
        &fx.pool,
        fx.admin_id,
        session_id,
        &SessionUpdate {
            business_date: "2026-06-02".to_owned(),
            timezone: "America/Chicago".to_owned(),
            local_start: "2026-06-02T06:30".to_owned(),
            phase_id: Some(two_v2),
        },
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(SessionRefusal::NoSuchPhase));

    // The session presents the version it was recorded under, not the
    // enrollment's newer pin.
    let detail = training_sessions::get(&fx.pool, fx.admin_id, session_id)
        .await
        .expect("call")
        .expect("read");
    assert_eq!(
        detail.version_number, 1,
        "sessions present their stamped version"
    );
    assert_eq!(detail.session.phase_name.as_deref(), Some("Phase Two"));

    // The stamp never moves, and a raw insert cannot stamp a version the
    // enrollment does not currently pin.
    let raw = sqlx::query("UPDATE training_session SET program_version_id = ?1 WHERE id = ?2")
        .bind(v2)
        .bind(session_id)
        .execute(&fx.pool)
        .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("recorded under"), "stamp is immutable: {err}");
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let raw = sqlx::query(
        "INSERT INTO training_session
             (enrollment_id, program_version_id, business_date, timezone,
              local_start, local_end, utc_start, utc_end, phase_id, disposition,
              created_at, created_by, closed_at, closed_by)
         VALUES (?1, ?2, '2026-06-01', 'America/Chicago', '2026-06-01T07:00',
                 '2026-06-01T12:00', ?3, ?4, NULL, 'completed', ?5, NULL, ?5, NULL)",
    )
    .bind(enrollment_id)
    .bind(v1)
    .bind(instant("2026-06-01T07:00:00-05:00"))
    .bind(instant("2026-06-01T12:00:00-05:00"))
    .bind(now)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(
        err.contains("stamps the enrollment"),
        "stamp equals the pin at creation: {err}"
    );
}
