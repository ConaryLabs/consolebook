//! Milestone 3 slice 4: the single-step review workflow — eligibility
//! and self-review, the comment rule, the submit → changes → revise →
//! resubmit → approve cycle with its thaw and freeze at both layers,
//! notices, the review queue, and the API round trip. Every fixture is
//! invented.

use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, SET_COOKIE};
use axum::http::{Request, StatusCode};
use consolebook_server::capabilities::RoleBundle;
use consolebook_server::draft_content::{self, DraftContent, NarrativeEntry, RatingEntry};
use consolebook_server::draft_review::{self, ReviewDecisionKind};
use consolebook_server::evaluation_drafts::{self, DraftRefusal, DraftStatus};
use consolebook_server::programs::{
    self, AnchorDef, CompetencyDef, FormCompetencyDef, FormDef, NarrativeDef, PhaseDef, PolicyDef, RecordType,
    ScaleDef, ScaleKind, TaskDef, TransitionDef, TransitionKind, VersionContent,
};
use consolebook_server::training_sessions::{self, SessionInput};
use consolebook_server::{assignments, data_dir::DataDir, enrollments, setup, storage, users};
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

    async fn form_competency_id(&self, version_id: i64, competency: &str) -> i64 {
        sqlx::query_scalar(
            "SELECT fc.id FROM form_competency fc
             JOIN competency c ON c.id = fc.competency_id
             WHERE fc.program_version_id = ?1 AND c.name = ?2",
        )
        .bind(version_id)
        .bind(competency)
        .fetch_one(&self.pool)
        .await
        .expect("form competency id")
    }

    async fn narrative_id(&self, version_id: i64, prompt: &str) -> i64 {
        sqlx::query_scalar(
            "SELECT id FROM form_narrative
             WHERE program_version_id = ?1 AND prompt = ?2",
        )
        .bind(version_id)
        .bind(prompt)
        .fetch_one(&self.pool)
        .await
        .expect("narrative id")
    }

    async fn notice_count(&self, user_id: i64, kind: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM notice WHERE user_id = ?1 AND kind = ?2")
            .bind(user_id)
            .bind(kind)
            .fetch_one(&self.pool)
            .await
            .expect("notice count")
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

/// Invented program content with one daily report form: an anchored
/// competency and two narrative prompts.
fn evaluated_content() -> VersionContent {
    VersionContent {
        name: "Example County CTO Program".to_owned(),
        label: "2026 rev A".to_owned(),
        description: "Invented program for review tests.".to_owned(),
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
            anchors: vec![
                AnchorDef {
                    value: 1,
                    label: "Unacceptable".to_owned(),
                    definition: "Performs contrary to training.".to_owned(),
                },
                AnchorDef {
                    value: 4,
                    label: "Meets standards".to_owned(),
                    definition: "Performs to the invented standard.".to_owned(),
                },
                AnchorDef {
                    value: 7,
                    label: "Superior".to_owned(),
                    definition: "Performs beyond the invented standard.".to_owned(),
                },
            ],
        }],
        rating_modifiers: Vec::new(),
        evaluation_forms: vec![FormDef {
            record_type: RecordType::DailyReport,
            name: "Daily Observation Report".to_owned(),
            instructions: "Rate today's observed performance.".to_owned(),
            competencies: vec![FormCompetencyDef {
                competency: "Emergency Call Interrogation".to_owned(),
                rating_scale: "Standard 1-7".to_owned(),
            }],
            narratives: vec![
                NarrativeDef {
                    prompt: "Most acceptable performance.".to_owned(),
                    required: true,
                },
                NarrativeDef {
                    prompt: "Least acceptable performance.".to_owned(),
                    required: false,
                },
            ],
        }],
        citations: Vec::new(),
        finalization_policy: PolicyDef::default(),
    }
}

#[allow(clippy::struct_field_names)]
struct Seeded {
    version_id: i64,
    session_id: i64,
    taylor_id: i64,
    jordan_id: i64,
    rowan_id: i64,
    casey_id: i64,
    marlow_id: i64,
}

/// Publishes the program and seeds a trainee (Taylor), two trainer
/// members (Jordan the assigned author, Rowan), and two coordinators who
/// hold `review_evaluation` (Casey, Marlow); one open session with both
/// trainers.
async fn seed(fx: &Fixture) -> Seeded {
    let version_id = fx.publish_program().await;
    let taylor_id = fx
        .user_with_role("taylor.trainee", "Taylor Trainee", RoleBundle::Trainee)
        .await;
    let jordan_id = fx
        .user_with_role("jordan.trainer", "Jordan Trainer", RoleBundle::Trainer)
        .await;
    let rowan_id = fx
        .user_with_role("rowan.trainer", "Rowan Trainer", RoleBundle::Trainer)
        .await;
    let casey_id = fx
        .user_with_role("casey.coord", "Casey Coordinator", RoleBundle::Coordinator)
        .await;
    let marlow_id = fx
        .user_with_role(
            "marlow.coord",
            "Marlow Coordinator",
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
    let session_id = training_sessions::create(
        &fx.pool,
        jordan_id,
        enrollment_id,
        &SessionInput {
            business_date: "2026-06-02".to_owned(),
            timezone: "America/Chicago".to_owned(),
            local_start: "2026-06-02T07:00".to_owned(),
            local_end: None,
            disposition: None,
            phase_id: None,
            trainer_user_ids: vec![jordan_id, rowan_id],
        },
    )
    .await
    .expect("call")
    .expect("created");
    Seeded {
        version_id,
        session_id,
        taylor_id,
        jordan_id,
        rowan_id,
        casey_id,
        marlow_id,
    }
}

impl Fixture {
    async fn publish_program(&self) -> i64 {
        let content = evaluated_content();
        let program_id = programs::create_program(&self.pool, self.admin_id, &content.name)
            .await
            .expect("create program")
            .expect("accepted");
        let version_id = programs::create_version(&self.pool, self.admin_id, program_id, &content)
            .await
            .expect("create version")
            .expect("accepted");
        programs::publish_version(&self.pool, self.admin_id, version_id)
            .await
            .expect("publish")
            .expect("accepted");
        version_id
    }
}

fn content_with(eci: i64, value: i64, most: i64, text: &str) -> DraftContent {
    DraftContent {
        ratings: vec![RatingEntry {
            form_competency_id: eci,
            value: Some(value),
            not_observed: false,
            modifier_ids: Vec::new(),
        }],
        narratives: vec![NarrativeEntry {
            form_narrative_id: most,
            text: text.to_owned(),
        }],
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn eligibility_matrix_and_comment_rule() {
    let fx = Fixture::new().await;
    let s = seed(&fx).await;
    let record_id = evaluation_drafts::create(&fx.pool, s.jordan_id, s.session_id, None)
        .await
        .expect("call")
        .expect("created");
    let eci = fx
        .form_competency_id(s.version_id, "Emergency Call Interrogation")
        .await;
    let most = fx
        .narrative_id(s.version_id, "Most acceptable performance.")
        .await;

    // Marlow the coordinator contributes — and thereby loses review
    // eligibility for this draft.
    draft_content::save(
        &fx.pool,
        s.marlow_id,
        record_id,
        0,
        &content_with(eci, 4, most, "Handled the invented fire call."),
    )
    .await
    .expect("call")
    .expect("saved");

    // Reviews decide submitted drafts.
    let refused = draft_review::decide(
        &fx.pool,
        s.marlow_id,
        record_id,
        ReviewDecisionKind::Approved,
        None,
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(DraftRefusal::NotSubmitted));

    // Casey the uninvolved coordinator moves ownership to Rowan — a
    // workflow action, not a contribution.
    evaluation_drafts::transfer(&fx.pool, s.casey_id, record_id, s.rowan_id)
        .await
        .expect("call")
        .expect("transferred");
    draft_content::save(
        &fx.pool,
        s.jordan_id,
        record_id,
        1,
        &content_with(eci, 5, most, "Handled the invented fire call well."),
    )
    .await
    .expect("call")
    .expect("saved");
    evaluation_drafts::submit(&fx.pool, s.rowan_id, record_id, 2)
        .await
        .expect("call")
        .expect("submitted");

    // No review capability: the trainee and the trainer alike.
    let refused = draft_review::decide(
        &fx.pool,
        s.taylor_id,
        record_id,
        ReviewDecisionKind::Approved,
        None,
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(DraftRefusal::CapabilityRequired));
    let refused = draft_review::decide(
        &fx.pool,
        s.jordan_id,
        record_id,
        ReviewDecisionKind::Approved,
        None,
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(DraftRefusal::CapabilityRequired));

    // A contributor with the capability is still refused: self-review.
    let refused = draft_review::decide(
        &fx.pool,
        s.marlow_id,
        record_id,
        ReviewDecisionKind::Approved,
        None,
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(DraftRefusal::SelfReview));

    // A change request explains itself.
    let refused = draft_review::decide(
        &fx.pool,
        s.casey_id,
        record_id,
        ReviewDecisionKind::ChangesRequested,
        Some("   "),
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(DraftRefusal::CommentRequired));

    // The database backstops self-review while submitted.
    let raw = sqlx::query(
        "INSERT INTO review_decision
             (evaluation_record_id, reviewer_user_id, decision, comment, decided_at)
         VALUES (?1, ?2, 'approved', '', 1)",
    )
    .bind(record_id)
    .bind(s.marlow_id)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("cannot review"), "self-review raw: {err}");

    // The transfer actor stayed eligible and decides.
    draft_review::decide(
        &fx.pool,
        s.casey_id,
        record_id,
        ReviewDecisionKind::Approved,
        None,
    )
    .await
    .expect("call")
    .expect("decided");
    let workspace = evaluation_drafts::workspace(&fx.pool, s.casey_id, record_id)
        .await
        .expect("call")
        .expect("read");
    assert_eq!(workspace.detail.status, DraftStatus::Approved);

    // Decisions are permanent: no update, no delete, and none on a
    // draft that is not submitted.
    let raw = sqlx::query("UPDATE review_decision SET decision = 'returned'")
        .execute(&fx.pool)
        .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("append-only"), "update: {err}");
    let raw = sqlx::query("DELETE FROM review_decision")
        .execute(&fx.pool)
        .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("append-only"), "delete: {err}");
    let raw = sqlx::query(
        "INSERT INTO review_decision
             (evaluation_record_id, reviewer_user_id, decision, comment, decided_at)
         VALUES (?1, ?2, 'returned', '', 1)",
    )
    .bind(record_id)
    .bind(s.casey_id)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(
        err.contains("decide submitted drafts"),
        "unsubmitted: {err}"
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn review_cycle_thaws_and_freezes() {
    let fx = Fixture::new().await;
    let s = seed(&fx).await;
    let record_id = evaluation_drafts::create(&fx.pool, s.jordan_id, s.session_id, None)
        .await
        .expect("call")
        .expect("created");
    let eci = fx
        .form_competency_id(s.version_id, "Emergency Call Interrogation")
        .await;
    let most = fx
        .narrative_id(s.version_id, "Most acceptable performance.")
        .await;
    draft_content::save(
        &fx.pool,
        s.jordan_id,
        record_id,
        0,
        &content_with(eci, 4, most, "Met the invented standard."),
    )
    .await
    .expect("call")
    .expect("saved");
    evaluation_drafts::submit(&fx.pool, s.jordan_id, record_id, 1)
        .await
        .expect("call")
        .expect("submitted");

    // Changes requested: the second snapshot lands and the copy thaws.
    draft_review::decide(
        &fx.pool,
        s.casey_id,
        record_id,
        ReviewDecisionKind::ChangesRequested,
        Some("Add the invented callback detail."),
    )
    .await
    .expect("call")
    .expect("decided");
    let reasons: Vec<(String,)> = sqlx::query_as(
        "SELECT reason FROM draft_snapshot WHERE evaluation_record_id = ?1 ORDER BY id",
    )
    .bind(record_id)
    .fetch_all(&fx.pool)
    .await
    .expect("snapshots");
    assert_eq!(
        reasons,
        vec![
            ("submission".to_owned(),),
            ("change_request_return".to_owned(),),
        ]
    );
    let workspace = evaluation_drafts::workspace(&fx.pool, s.jordan_id, record_id)
        .await
        .expect("call")
        .expect("read");
    assert_eq!(workspace.detail.status, DraftStatus::ChangesRequested);
    assert_eq!(
        workspace.detail.decisions[0].comment,
        "Add the invented callback detail."
    );
    // Thawed at the database too.
    sqlx::query("UPDATE draft_narrative SET text = 'revised raw' WHERE evaluation_record_id = ?1")
        .bind(record_id)
        .execute(&fx.pool)
        .await
        .expect("thawed update");

    // Revise and resubmit under the same revision contract; the earlier
    // reviewer approves the resubmission.
    draft_content::save(
        &fx.pool,
        s.jordan_id,
        record_id,
        1,
        &content_with(
            eci,
            5,
            most,
            "Met the invented standard; callback verified.",
        ),
    )
    .await
    .expect("call")
    .expect("saved");
    evaluation_drafts::submit(&fx.pool, s.jordan_id, record_id, 2)
        .await
        .expect("call")
        .expect("resubmitted");
    draft_review::decide(
        &fx.pool,
        s.casey_id,
        record_id,
        ReviewDecisionKind::Approved,
        None,
    )
    .await
    .expect("call")
    .expect("approved");

    // Approved stays frozen: at the service and at the database.
    let refused = draft_content::save(
        &fx.pool,
        s.jordan_id,
        record_id,
        3,
        &content_with(eci, 7, most, "rewrite"),
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(DraftRefusal::DraftApproved));
    let refused = evaluation_drafts::transfer(&fx.pool, s.casey_id, record_id, s.rowan_id)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::DraftApproved));
    let refused = evaluation_drafts::submit(&fx.pool, s.jordan_id, record_id, 3)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::DraftApproved));
    let refused = draft_review::decide(
        &fx.pool,
        s.marlow_id,
        record_id,
        ReviewDecisionKind::Returned,
        None,
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(DraftRefusal::NotSubmitted));
    let raw = sqlx::query(
        "UPDATE draft_narrative SET text = 'rewritten' WHERE evaluation_record_id = ?1",
    )
    .bind(record_id)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("is frozen"), "approved freeze: {err}");

    // A plain return reopens without a snapshot: separate draft.
    let session2 = {
        training_sessions::close(
            &fx.pool,
            s.jordan_id,
            s.session_id,
            training_sessions::Disposition::Completed,
            Some("2026-06-02T15:00"),
        )
        .await
        .expect("call")
        .expect("closed");
        training_sessions::create(
            &fx.pool,
            s.jordan_id,
            sqlx::query_scalar("SELECT enrollment_id FROM training_session WHERE id = ?1")
                .bind(s.session_id)
                .fetch_one(&fx.pool)
                .await
                .expect("enrollment"),
            &SessionInput {
                business_date: "2026-06-03".to_owned(),
                timezone: "America/Chicago".to_owned(),
                local_start: "2026-06-03T07:00".to_owned(),
                local_end: None,
                disposition: None,
                phase_id: None,
                trainer_user_ids: vec![s.jordan_id],
            },
        )
        .await
        .expect("call")
        .expect("created")
    };
    let record2 = evaluation_drafts::create(&fx.pool, s.jordan_id, session2, None)
        .await
        .expect("call")
        .expect("created");
    evaluation_drafts::submit(&fx.pool, s.jordan_id, record2, 0)
        .await
        .expect("call")
        .expect("submitted");
    draft_review::decide(
        &fx.pool,
        s.casey_id,
        record2,
        ReviewDecisionKind::Returned,
        None,
    )
    .await
    .expect("call")
    .expect("returned");
    let workspace = evaluation_drafts::workspace(&fx.pool, s.jordan_id, record2)
        .await
        .expect("call")
        .expect("read");
    assert_eq!(workspace.detail.status, DraftStatus::Returned);
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM draft_snapshot
         WHERE evaluation_record_id = ?1 AND reason = 'change_request_return'",
    )
    .bind(record2)
    .fetch_one(&fx.pool)
    .await
    .expect("count");
    assert_eq!(count, 0, "a plain return takes no snapshot");

    // Notices and audit: the owner heard each verdict, the reviewers
    // heard each submission, and every decision is audited.
    assert_eq!(
        fx.notice_count(s.jordan_id, "draft_changes_requested")
            .await,
        1
    );
    assert_eq!(fx.notice_count(s.jordan_id, "draft_approved").await, 1);
    assert_eq!(fx.notice_count(s.jordan_id, "draft_returned").await, 1);
    assert_eq!(
        fx.notice_count(s.casey_id, "draft_submitted_for_review")
            .await,
        3,
        "two submissions of the first draft and one of the second"
    );
    assert_eq!(
        fx.notice_count(s.jordan_id, "draft_submitted_for_review")
            .await,
        0,
        "the submitter is not nudged"
    );
    let audited: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_event WHERE kind = 'draft_review_decided'")
            .fetch_one(&fx.pool)
            .await
            .expect("audit");
    assert_eq!(audited, 3);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn review_queue_and_api_round_trip() {
    let fx = Fixture::new().await;
    let s = seed(&fx).await;
    let record_id = evaluation_drafts::create(&fx.pool, s.jordan_id, s.session_id, None)
        .await
        .expect("call")
        .expect("created");
    let eci = fx
        .form_competency_id(s.version_id, "Emergency Call Interrogation")
        .await;
    let most = fx
        .narrative_id(s.version_id, "Most acceptable performance.")
        .await;
    draft_content::save(
        &fx.pool,
        s.jordan_id,
        record_id,
        0,
        &content_with(eci, 4, most, "Strong invented shift."),
    )
    .await
    .expect("call")
    .expect("saved");
    evaluation_drafts::submit(&fx.pool, s.jordan_id, record_id, 1)
        .await
        .expect("call")
        .expect("submitted");

    let taylor = fx.login("taylor.trainee", PASSWORD).await;
    let casey = fx.login("casey.coord", PASSWORD).await;
    let jordan = fx.login("jordan.trainer", PASSWORD).await;

    // The queue is gated and lists the submitted draft.
    let (status, _) = request(fx.app(), "GET", "/api/reviews/queue", Some(&taylor), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = request(fx.app(), "GET", "/api/reviews/queue", Some(&casey), None).await;
    assert_eq!(status, StatusCode::OK, "queue: {body}");
    assert_eq!(body["drafts"].as_array().expect("rows").len(), 1);
    assert_eq!(body["drafts"][0]["record_id"], record_id);
    assert_eq!(body["drafts"][0]["trainee_display_name"], "Taylor Trainee");
    assert_eq!(body["drafts"][0]["eligible"], true);

    // The reviewer sees the decision surface; the owner does not.
    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/drafts/{record_id}"),
        Some(&casey),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["viewer_may_review"], true);
    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/drafts/{record_id}"),
        Some(&jordan),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["viewer_may_review"], false);

    // A change request without a comment is the typed refusal; with one
    // it lands and reopens the draft.
    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/drafts/{record_id}/review"),
        Some(&casey),
        Some(serde_json::json!({ "decision": "changes_requested" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "comment_required");
    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/drafts/{record_id}/review"),
        Some(&casey),
        Some(serde_json::json!({
            "decision": "changes_requested",
            "comment": "Name the invented callback number."
        })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "decide: {body}");
    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/drafts/{record_id}"),
        Some(&jordan),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "changes_requested");
    assert_eq!(
        body["decisions"][0]["comment"],
        "Name the invented callback number."
    );

    // Revise, resubmit, approve; the approved copy refuses writes and
    // the queue drains.
    let (status, body) = request(
        fx.app(),
        "PUT",
        &format!("/api/drafts/{record_id}/content"),
        Some(&jordan),
        Some(serde_json::json!({
            "revision": 1,
            "ratings": [{ "form_competency_id": eci, "value": 5, "modifier_ids": [] }],
            "narratives": [
                { "form_narrative_id": most, "text": "Callback named and verified." }
            ]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "revise: {body}");
    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/drafts/{record_id}/submit"),
        Some(&jordan),
        Some(serde_json::json!({ "revision": 2 })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "resubmit: {body}");
    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/drafts/{record_id}/review"),
        Some(&casey),
        Some(serde_json::json!({ "decision": "approved" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "approve: {body}");
    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/drafts/{record_id}"),
        Some(&jordan),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "approved");
    assert_eq!(body["decisions"].as_array().expect("rows").len(), 2);
    let (status, body) = request(
        fx.app(),
        "PUT",
        &format!("/api/drafts/{record_id}/content"),
        Some(&jordan),
        Some(serde_json::json!({ "revision": 2, "ratings": [], "narratives": [] })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "draft_approved");
    let (status, body) = request(fx.app(), "GET", "/api/reviews/queue", Some(&casey), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["drafts"].as_array().expect("rows").len(), 0);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn raw_decisions_advance_the_workflow() {
    let fx = Fixture::new().await;
    let s = seed(&fx).await;
    let record_id = evaluation_drafts::create(&fx.pool, s.jordan_id, s.session_id, None)
        .await
        .expect("call")
        .expect("created");
    evaluation_drafts::submit(&fx.pool, s.jordan_id, record_id, 0)
        .await
        .expect("call")
        .expect("submitted");

    // A change request cannot land without ADR 0008's second snapshot
    // anchoring what was reviewed — the snapshot trigger fires ahead of
    // the comment CHECK, so this holds even for a well-commented raw
    // insert.
    let raw = sqlx::query(
        "INSERT INTO review_decision
             (evaluation_record_id, reviewer_user_id, decision, comment, decided_at)
         VALUES (?1, ?2, 'changes_requested', 'Add the invented callback detail.', 1)",
    )
    .bind(record_id)
    .bind(s.casey_id)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(
        err.contains("snapshots what was reviewed"),
        "unanchored change request: {err}"
    );

    // With the snapshot anchored, the comment rule still holds against
    // raw writes: whitespace padding is not an explanation.
    sqlx::query(
        "INSERT INTO draft_snapshot
             (evaluation_record_id, reason, content, taken_at, taken_by)
         VALUES (?1, 'change_request_return', '{}', 1, ?2)",
    )
    .bind(record_id)
    .bind(s.casey_id)
    .execute(&fx.pool)
    .await
    .expect("raw snapshot");
    let raw = sqlx::query(
        "INSERT INTO review_decision
             (evaluation_record_id, reviewer_user_id, decision, comment, decided_at)
         VALUES (?1, ?2, 'changes_requested', '  \t ', 1)",
    )
    .bind(record_id)
    .bind(s.casey_id)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(
        err.contains("CHECK constraint failed"),
        "blank comment: {err}"
    );

    // The event stream is guarded while frozen: no raw append thaws a
    // submitted draft, and a review event without its decision is
    // refused.
    let raw = sqlx::query(
        "INSERT INTO contributor_event
             (evaluation_record_id, kind, actor_user_id, to_user_id, recorded_at)
         VALUES (?1, 'contributed', ?2, NULL, 1)",
    )
    .bind(record_id)
    .bind(s.jordan_id)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("is frozen"), "submitted thaw-forge: {err}");
    let raw = sqlx::query(
        "INSERT INTO contributor_event
             (evaluation_record_id, kind, actor_user_id, to_user_id, recorded_at)
         VALUES (?1, 'review_decided', ?2, NULL, 1)",
    )
    .bind(record_id)
    .bind(s.casey_id)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("pairs with its decision"), "bare event: {err}");

    // The blank-comment rule covers the whole White_Space set, not just
    // the common characters.
    let raw = sqlx::query(
        "INSERT INTO review_decision
             (evaluation_record_id, reviewer_user_id, decision, comment, decided_at)
         VALUES (?1, ?2, 'changes_requested', char(11) || char(12) || char(8232), 1)",
    )
    .bind(record_id)
    .bind(s.casey_id)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(
        err.contains("CHECK constraint failed"),
        "exotic blank comment: {err}"
    );

    // A raw decision is one atomic write: the database itself appends
    // the paired review_decided event...
    sqlx::query(
        "INSERT INTO review_decision
             (evaluation_record_id, reviewer_user_id, decision, comment, decided_at)
         VALUES (?1, ?2, 'returned', 'Returned for another invented pass.', 2)",
    )
    .bind(record_id)
    .bind(s.casey_id)
    .execute(&fx.pool)
    .await
    .expect("raw return");
    let events: Vec<(String, i64)> = sqlx::query_as(
        "SELECT kind, actor_user_id FROM contributor_event
         WHERE evaluation_record_id = ?1 ORDER BY id",
    )
    .bind(record_id)
    .fetch_all(&fx.pool)
    .await
    .expect("events");
    assert_eq!(
        events,
        vec![
            ("created".to_owned(), s.jordan_id),
            ("submitted_for_review".to_owned(), s.jordan_id),
            ("review_decided".to_owned(), s.casey_id),
        ]
    );
    let workspace = evaluation_drafts::workspace(&fx.pool, s.casey_id, record_id)
        .await
        .expect("call")
        .expect("read");
    assert_eq!(workspace.detail.status, DraftStatus::Returned);

    // ...so a second decision on the same submission meets the
    // submitted gate instead of stacking silently.
    let raw = sqlx::query(
        "INSERT INTO review_decision
             (evaluation_record_id, reviewer_user_id, decision, comment, decided_at)
         VALUES (?1, ?2, 'approved', '', 3)",
    )
    .bind(record_id)
    .bind(s.casey_id)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("decide submitted drafts"), "double: {err}");

    // The service path appends nothing extra: after a resubmission and
    // a service approval, the stream carries exactly one event per
    // decision.
    evaluation_drafts::submit(&fx.pool, s.jordan_id, record_id, 0)
        .await
        .expect("call")
        .expect("resubmitted");
    draft_review::decide(
        &fx.pool,
        s.casey_id,
        record_id,
        ReviewDecisionKind::Approved,
        None,
    )
    .await
    .expect("call")
    .expect("decided");
    let decided: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM contributor_event
         WHERE evaluation_record_id = ?1 AND kind = 'review_decided'",
    )
    .bind(record_id)
    .fetch_one(&fx.pool)
    .await
    .expect("count");
    assert_eq!(decided, 2);
    let workspace = evaluation_drafts::workspace(&fx.pool, s.casey_id, record_id)
        .await
        .expect("call")
        .expect("read");
    assert_eq!(workspace.detail.status, DraftStatus::Approved);
    assert_eq!(workspace.detail.decisions.len(), 2);

    // Approval is permanent at the database: no raw event thaws or
    // reroutes the approved copy.
    let raw = sqlx::query(
        "INSERT INTO contributor_event
             (evaluation_record_id, kind, actor_user_id, to_user_id, recorded_at)
         VALUES (?1, 'contributed', ?2, NULL, 9)",
    )
    .bind(record_id)
    .bind(s.jordan_id)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("is frozen"), "approved thaw-forge: {err}");
    let raw = sqlx::query(
        "INSERT INTO contributor_event
             (evaluation_record_id, kind, actor_user_id, to_user_id, recorded_at)
         VALUES (?1, 'ownership_transferred', ?2, ?3, 9)",
    )
    .bind(record_id)
    .bind(s.casey_id)
    .bind(s.rowan_id)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("is frozen"), "approved reroute: {err}");
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM contributor_event WHERE evaluation_record_id = ?1",
    )
    .bind(record_id)
    .fetch_one(&fx.pool)
    .await
    .expect("count");
    assert_eq!(total, 5);
}
