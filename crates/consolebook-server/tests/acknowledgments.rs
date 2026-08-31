//! Milestone 4 slice 2: acknowledgments and the trainee's own-records
//! timeline — the trainee reads and acknowledges their own finalized
//! record (never a working draft), refusals escalate, attested kinds
//! are recorded about the trainee by review authority, the database
//! holds the shape raw, and the API round-trips. Every fixture is
//! invented.

use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, SET_COOKIE};
use axum::http::{Request, StatusCode};
use consolebook_server::acknowledgments::{self, AckRefusal, AttestedKind, TraineeAckKind};
use consolebook_server::capabilities::{self, Capability, RoleBundle};
use consolebook_server::evaluation_drafts::{self, DraftRefusal};
use consolebook_server::finalization;
use consolebook_server::programs::{
    self, AnchorDef, CompetencyDef, FormCompetencyDef, FormDef, NarrativeDef, PolicyDef,
    RecordType, ScaleDef, ScaleKind, VersionContent,
};
use consolebook_server::training_sessions::{self, Disposition, SessionInput};
use consolebook_server::{assignments, data_dir::DataDir, enrollments, setup, storage, users};
use http_body_util::BodyExt;
use tower::ServiceExt;

const PASSWORD: &str = "invented-passphrase-1";

/// Every rule off: an open draft seals directly, which keeps each test
/// record cheap — acknowledgment behavior is independent of how the
/// version came to exist.
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

    async fn notice_count(&self, user_id: i64, kind: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM notice WHERE user_id = ?1 AND kind = ?2")
            .bind(user_id)
            .bind(kind)
            .fetch_one(&self.pool)
            .await
            .expect("count notices")
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

/// Invented single-form program; completion rules per `policy`.
fn program(name: &str, policy: PolicyDef) -> VersionContent {
    VersionContent {
        name: name.to_owned(),
        label: "2026 rev A".to_owned(),
        description: "Invented program for acknowledgment tests.".to_owned(),
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
        finalization_policy: policy,
    }
}

#[allow(clippy::struct_field_names)]
struct Seeded {
    enrollment_id: i64,
    record_id: i64,
    taylor_id: i64,
    jordan_id: i64,
    casey_id: i64,
}

/// Publishes an open-policy program, seeds a trainee, an assigned
/// trainer, and a coordinator, opens one session, and starts its draft.
async fn seed(fx: &Fixture, suffix: &str) -> Seeded {
    let content = program(&format!("Example County Program {suffix}"), OPEN_POLICY);
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
        enrollment_id,
        record_id,
        taylor_id,
        jordan_id,
        casey_id,
    }
}

/// Opens one session on `date` and starts its draft.
async fn draft_for(fx: &Fixture, trainer_id: i64, enrollment_id: i64, date: &str) -> i64 {
    let session_id = training_sessions::create(
        &fx.pool,
        trainer_id,
        enrollment_id,
        &SessionInput {
            business_date: date.to_owned(),
            timezone: "America/Chicago".to_owned(),
            local_start: format!("{date}T07:00"),
            // Closed at creation so a later session never overlaps an
            // open interval (invariant 7).
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

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn trainee_reads_and_acknowledges_own_finalized_record() {
    let fx = Fixture::new().await;
    let s = seed(&fx, "ack").await;

    // The Trainee bundle grants exactly the two own-record capabilities.
    let held = capabilities::list_for_user(&fx.pool, s.taylor_id)
        .await
        .expect("list");
    assert_eq!(held, vec!["acknowledge_own_record", "view_own_records"]);

    // A working draft about the trainee is not theirs to see, and an
    // unfinalized record takes no acknowledgment.
    let refused = evaluation_drafts::workspace(&fx.pool, s.taylor_id, s.record_id)
        .await
        .expect("call");
    assert!(matches!(refused, Err(DraftRefusal::CapabilityRequired)));
    let refused = acknowledgments::acknowledge(
        &fx.pool,
        s.taylor_id,
        s.record_id,
        TraineeAckKind::Acknowledged,
        "",
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(AckRefusal::NotFinalized));
    let timeline = acknowledgments::own_records(&fx.pool, s.taylor_id)
        .await
        .expect("call")
        .expect("listed");
    assert!(timeline.is_empty());

    finalization::finalize(&fx.pool, s.casey_id, s.record_id, 0)
        .await
        .expect("call")
        .expect("sealed");
    assert_eq!(
        fx.notice_count(s.taylor_id, "record_awaits_acknowledgment")
            .await,
        1
    );

    // Finalized, the record is the trainee's to read — the sealed
    // presentation — and appears on their timeline awaiting them.
    finalization::finalized_view(&fx.pool, s.taylor_id, s.record_id)
        .await
        .expect("call")
        .expect("readable")
        .expect("finalized");
    let timeline = acknowledgments::own_records(&fx.pool, s.taylor_id)
        .await
        .expect("call")
        .expect("listed");
    assert_eq!(timeline.len(), 1);
    assert_eq!(timeline[0].record_id, s.record_id);
    assert_eq!(timeline[0].form_name, "Daily Observation Report");
    assert_eq!(timeline[0].business_date.as_deref(), Some("2026-06-02"));
    assert_eq!(timeline[0].acknowledgment_kind, None);
    // An unacknowledged latest version answers None — never a
    // predecessor's act presented as current.
    let unacked = acknowledgments::acknowledgment_of(&fx.pool, s.casey_id, s.record_id)
        .await
        .expect("call")
        .expect("readable");
    assert!(unacked.is_none());

    // The trainee reads the finalized record, not the workflow: the
    // transfer roster and snapshot bookkeeping are redacted on the
    // own-record basis, while a workflow reader keeps them.
    let rowan_id = fx
        .user_with_role("rowan.ack", "Rowan Trainer", RoleBundle::Trainer)
        .await;
    assignments::create(&fx.pool, fx.admin_id, s.enrollment_id, rowan_id)
        .await
        .expect("call")
        .expect("assigned");
    let mine = evaluation_drafts::workspace(&fx.pool, s.taylor_id, s.record_id)
        .await
        .expect("call")
        .expect("readable");
    assert!(mine.detail.eligible_recipients.is_empty());
    assert!(mine.detail.snapshots.is_empty());
    let theirs = evaluation_drafts::workspace(&fx.pool, s.casey_id, s.record_id)
        .await
        .expect("call")
        .expect("readable");
    assert_eq!(theirs.detail.eligible_recipients.len(), 1);

    // The timeline is gated on its capability; other roles hold their
    // own read paths, not this one.
    let refused = acknowledgments::own_records(&fx.pool, s.jordan_id)
        .await
        .expect("call");
    assert!(matches!(refused, Err(AckRefusal::CapabilityRequired)));

    // Raw forgeries meet the database before the legitimate act: the
    // shape rules and the trainee binding hold without the service.
    let version_id: i64 =
        sqlx::query_scalar("SELECT id FROM evaluation_version WHERE evaluation_record_id = ?1")
            .bind(s.record_id)
            .fetch_one(&fx.pool)
            .await
            .expect("version id");
    let raw = sqlx::query(
        "INSERT INTO acknowledgment
             (evaluation_version_id, user_id, kind, response, recorded_by,
              user_display_name, recorded_by_display_name, recorded_at)
         VALUES (?1, ?2, 'acknowledged', 'smuggled text', ?2,
                 'Taylor Trainee', 'Taylor Trainee', 1)",
    )
    .bind(version_id)
    .bind(s.taylor_id)
    .execute(&fx.pool)
    .await;
    assert!(raw.is_err(), "a plain acknowledgment carries no text");
    let raw = sqlx::query(
        "INSERT INTO acknowledgment
             (evaluation_version_id, user_id, kind, response, recorded_by,
              user_display_name, recorded_by_display_name, recorded_at)
         VALUES (?1, ?2, 'refused', char(9, 10, 32, 160, 8199, 12288), ?2,
                 'Taylor Trainee', 'Taylor Trainee', 1)",
    )
    .bind(version_id)
    .bind(s.taylor_id)
    .execute(&fx.pool)
    .await;
    assert!(raw.is_err(), "a refusal explains itself past blank text");
    let raw = sqlx::query(
        "INSERT INTO acknowledgment
             (evaluation_version_id, user_id, kind, response, recorded_by,
              user_display_name, recorded_by_display_name, recorded_at)
         VALUES (?1, ?2, 'refused', 'forged by another hand', ?3,
                 'Taylor Trainee', 'Casey Coordinator', 1)",
    )
    .bind(version_id)
    .bind(s.taylor_id)
    .bind(s.casey_id)
    .execute(&fx.pool)
    .await;
    assert!(raw.is_err(), "trainee kinds are recorded by the trainee");
    let raw = sqlx::query(
        "INSERT INTO acknowledgment
             (evaluation_version_id, user_id, kind, response, recorded_by,
              user_display_name, recorded_by_display_name, recorded_at)
         VALUES (?1, ?2, 'acknowledged', '', ?2,
                 'Jordan Trainer', 'Jordan Trainer', 1)",
    )
    .bind(version_id)
    .bind(s.jordan_id)
    .execute(&fx.pool)
    .await;
    assert!(
        raw.is_err(),
        "an acknowledgment binds the version's trainee"
    );

    // Typed shape refusals at the service.
    let refused = acknowledgments::acknowledge(
        &fx.pool,
        s.taylor_id,
        s.record_id,
        TraineeAckKind::Acknowledged,
        "an unexpected response",
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(AckRefusal::ResponseNotAllowed));
    let refused = acknowledgments::acknowledge(
        &fx.pool,
        s.taylor_id,
        s.record_id,
        TraineeAckKind::AcknowledgedWithResponse,
        " \u{2003}\t",
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(AckRefusal::ResponseRequired));

    // Someone else's trainee capability does not reach this record.
    let riley_id = fx
        .user_with_role("riley.ack", "Riley Trainee", RoleBundle::Trainee)
        .await;
    let refused = acknowledgments::acknowledge(
        &fx.pool,
        riley_id,
        s.record_id,
        TraineeAckKind::Acknowledged,
        "",
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(AckRefusal::NotYourRecord));

    // The legitimate act: a plain acknowledgment, audited, once.
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
    let audited: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_event
         WHERE kind = 'acknowledgment_recorded' AND actor_user_id = ?1",
    )
    .bind(s.taylor_id)
    .fetch_one(&fx.pool)
    .await
    .expect("audit");
    assert_eq!(audited, 1);
    let ack = acknowledgments::acknowledgment_of(&fx.pool, s.casey_id, s.record_id)
        .await
        .expect("call")
        .expect("readable")
        .expect("recorded");
    assert_eq!(ack.kind, "acknowledged");
    assert_eq!(ack.response, "");
    assert_eq!(ack.recorded_by, s.taylor_id);
    let timeline = acknowledgments::own_records(&fx.pool, s.taylor_id)
        .await
        .expect("call")
        .expect("listed");
    assert_eq!(
        timeline[0].acknowledgment_kind.as_deref(),
        Some("acknowledged")
    );

    // One acknowledgment per version per person, at the service and raw.
    let refused = acknowledgments::acknowledge(
        &fx.pool,
        s.taylor_id,
        s.record_id,
        TraineeAckKind::Acknowledged,
        "",
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(AckRefusal::AlreadyAcknowledged));
    let refused = acknowledgments::attest(
        &fx.pool,
        s.casey_id,
        s.record_id,
        AttestedKind::Unavailable,
        "already acknowledged in person",
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(AckRefusal::AlreadyAcknowledged));

    // Permanence held by the database, not application manners.
    let raw = sqlx::query("UPDATE acknowledgment SET response = 'rewritten' WHERE 1 = 1")
        .execute(&fx.pool)
        .await;
    assert!(raw.is_err(), "acknowledgments never update");
    let raw = sqlx::query("DELETE FROM acknowledgment WHERE 1 = 1")
        .execute(&fx.pool)
        .await;
    assert!(raw.is_err(), "acknowledgments never delete");
}

#[tokio::test]
async fn enrollment_grants_own_record_capabilities() {
    let fx = Fixture::new().await;
    let content = program("Example County Program grants", OPEN_POLICY);
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

    // A trainer-bundle account enrolled as a trainee gains the
    // own-record capabilities with the enrollment: the 0011 backfill
    // ran once, so the enrollment transaction is what makes later
    // trainees whole.
    let jordan_id = fx
        .user_with_role("jordan.grant", "Jordan Trainer", RoleBundle::Trainer)
        .await;
    enrollments::enroll(&fx.pool, fx.admin_id, version_id, jordan_id)
        .await
        .expect("call")
        .expect("enrolled");
    let held = capabilities::list_for_user(&fx.pool, jordan_id)
        .await
        .expect("list");
    assert_eq!(
        held,
        vec![
            "acknowledge_own_record",
            "author_evaluation",
            "view_assigned_records",
            "view_own_records"
        ]
    );

    // Idempotent over grants a bundle already made: a plain trainee
    // enrolls without duplicate-grant failures or extra rows.
    let taylor_id = fx
        .user_with_role("taylor.grant", "Taylor Trainee", RoleBundle::Trainee)
        .await;
    enrollments::enroll(&fx.pool, fx.admin_id, version_id, taylor_id)
        .await
        .expect("call")
        .expect("enrolled");
    let held = capabilities::list_for_user(&fx.pool, taylor_id)
        .await
        .expect("list");
    assert_eq!(held, vec!["acknowledge_own_record", "view_own_records"]);
}

#[tokio::test]
async fn refusal_escalates_to_review_holders() {
    let fx = Fixture::new().await;
    let s = seed(&fx, "refuse").await;
    finalization::finalize(&fx.pool, s.casey_id, s.record_id, 0)
        .await
        .expect("call")
        .expect("sealed");

    acknowledgments::acknowledge(
        &fx.pool,
        s.taylor_id,
        s.record_id,
        TraineeAckKind::Refused,
        "I dispute the invented ratings.",
    )
    .await
    .expect("call")
    .expect("refusal recorded");

    // Every review_evaluation holder is told; nobody else is, and a
    // refusal is not a response.
    assert_eq!(
        fx.notice_count(s.casey_id, "acknowledgment_refused").await,
        1
    );
    assert_eq!(
        fx.notice_count(s.jordan_id, "acknowledgment_response")
            .await,
        0
    );
    assert_eq!(
        fx.notice_count(fx.admin_id, "acknowledgment_refused").await,
        0
    );
    assert_eq!(
        fx.notice_count(s.jordan_id, "acknowledgment_refused").await,
        0
    );

    let ack = acknowledgments::acknowledgment_of(&fx.pool, s.casey_id, s.record_id)
        .await
        .expect("call")
        .expect("readable")
        .expect("recorded");
    assert_eq!(ack.kind, "refused");
    assert_eq!(ack.response, "I dispute the invented ratings.");
    assert_eq!(ack.recorded_by, s.taylor_id);

    // The permanent act displays the identity recorded at the act: a
    // later profile rename never rewrites it.
    sqlx::query("UPDATE user SET display_name = 'Taylor Renamed' WHERE id = ?1")
        .bind(s.taylor_id)
        .execute(&fx.pool)
        .await
        .expect("rename");
    let ack = acknowledgments::acknowledgment_of(&fx.pool, s.casey_id, s.record_id)
        .await
        .expect("call")
        .expect("readable")
        .expect("recorded");
    assert_eq!(ack.user_display_name, "Taylor Trainee");
    assert_eq!(ack.recorded_by_display_name, "Taylor Trainee");

    // The refusal is the trainee's one binding to this version; an
    // attestation over it is refused.
    let refused = acknowledgments::attest(
        &fx.pool,
        s.casey_id,
        s.record_id,
        AttestedKind::SupervisorAttestedRefusal,
        "refused verbally at shift end",
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(AckRefusal::AlreadyAcknowledged));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn attested_kinds_take_review_authority_and_a_reason() {
    let fx = Fixture::new().await;
    let s = seed(&fx, "attest").await;
    let refused = acknowledgments::attest(
        &fx.pool,
        s.casey_id,
        s.record_id,
        AttestedKind::Unavailable,
        "separated before finalization",
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(AckRefusal::NotFinalized));
    finalization::finalize(&fx.pool, s.casey_id, s.record_id, 0)
        .await
        .expect("call")
        .expect("sealed");

    // Gates: the reason explains itself; attestation takes review
    // authority; and the trainee never attests about themselves even
    // when they hold that authority.
    let refused = acknowledgments::attest(
        &fx.pool,
        s.casey_id,
        s.record_id,
        AttestedKind::Unavailable,
        "   ",
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(AckRefusal::ResponseRequired));
    let refused = acknowledgments::attest(
        &fx.pool,
        s.jordan_id,
        s.record_id,
        AttestedKind::Unavailable,
        "trainers do not attest",
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(AckRefusal::CapabilityRequired));
    let mut conn = fx.pool.acquire().await.expect("conn");
    capabilities::grant_bundle(
        &mut conn,
        s.taylor_id,
        &[Capability::ReviewEvaluation],
        None,
    )
    .await
    .expect("grant");
    drop(conn);
    let refused = acknowledgments::attest(
        &fx.pool,
        s.taylor_id,
        s.record_id,
        AttestedKind::SupervisorAttestedRefusal,
        "attesting my own refusal",
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(AckRefusal::SelfAttestation));
    let refused = acknowledgments::attest(
        &fx.pool,
        s.casey_id,
        9999,
        AttestedKind::Unavailable,
        "no such record",
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(AckRefusal::NoSuchRecord));

    // The attestation binds the trainee, is recorded by the attester,
    // and tells the trainee.
    acknowledgments::attest(
        &fx.pool,
        s.casey_id,
        s.record_id,
        AttestedKind::Unavailable,
        "separated from employment before finalization",
    )
    .await
    .expect("call")
    .expect("attested");
    assert_eq!(
        fx.notice_count(s.taylor_id, "acknowledgment_attested")
            .await,
        1
    );
    let ack = acknowledgments::acknowledgment_of(&fx.pool, s.casey_id, s.record_id)
        .await
        .expect("call")
        .expect("readable")
        .expect("recorded");
    assert_eq!(ack.kind, "unavailable");
    assert_eq!(ack.recorded_by, s.casey_id);
    assert_eq!(ack.user_display_name, "Taylor Trainee");
    let timeline = acknowledgments::own_records(&fx.pool, s.taylor_id)
        .await
        .expect("call")
        .expect("listed");
    assert_eq!(
        timeline[0].acknowledgment_kind.as_deref(),
        Some("unavailable")
    );

    // The version already carries its acknowledgment; the trainee's own
    // later attempt meets the same one-per-version contract.
    let refused = acknowledgments::acknowledge(
        &fx.pool,
        s.taylor_id,
        s.record_id,
        TraineeAckKind::Acknowledged,
        "",
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(AckRefusal::AlreadyAcknowledged));
}

#[tokio::test]
async fn acknowledgment_api_round_trip() {
    let fx = Fixture::new().await;
    let s = seed(&fx, "api").await;
    finalization::finalize(&fx.pool, s.casey_id, s.record_id, 0)
        .await
        .expect("call")
        .expect("sealed");
    // A second, unfinalized draft stays out of the trainee's reach.
    let open_id = draft_for(&fx, s.jordan_id, s.enrollment_id, "2026-06-03").await;

    let taylor = fx.login("taylor.api", PASSWORD).await;
    let casey = fx.login("casey.api", PASSWORD).await;

    let (status, body) = request(fx.app(), "GET", "/api/my/records", Some(&taylor), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["records"].as_array().expect("rows").len(), 1);
    assert_eq!(body["records"][0]["record_id"], s.record_id);
    assert_eq!(
        body["records"][0]["acknowledgment_kind"],
        serde_json::Value::Null
    );

    let (status, _) = request(fx.app(), "GET", "/api/my/records", Some(&casey), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The trainee reads their finalized record but not the open draft.
    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/drafts/{}", s.record_id),
        Some(&taylor),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // Workflow-only fields are redacted for the own-record reader.
    assert_eq!(body["eligible_recipients"], serde_json::json!([]));
    assert_eq!(body["snapshots"], serde_json::json!([]));
    let (status, _) = request(
        fx.app(),
        "GET",
        &format!("/api/drafts/{open_id}"),
        Some(&taylor),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Refusal shapes come back typed.
    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/drafts/{}/acknowledge", s.record_id),
        Some(&taylor),
        Some(serde_json::json!({ "kind": "acknowledged_with_response" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "response_required");
    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/drafts/{}/attest", s.record_id),
        Some(&taylor),
        Some(serde_json::json!({ "kind": "unavailable", "reason": "not mine to attest" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "capability_required");

    let (status, _) = request(
        fx.app(),
        "POST",
        &format!("/api/drafts/{}/acknowledge", s.record_id),
        Some(&taylor),
        Some(serde_json::json!({
            "kind": "acknowledged_with_response",
            "response": "Received; I have added context to the invented narrative."
        })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    // The response is a persisted notice to the record's owner
    // (docs/architecture.md Notifications).
    assert_eq!(
        fx.notice_count(s.jordan_id, "acknowledgment_response")
            .await,
        1
    );

    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/drafts/{}/acknowledgment", s.record_id),
        Some(&casey),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["acknowledgment"]["kind"], "acknowledged_with_response");
    assert_eq!(
        body["acknowledgment"]["response"],
        "Received; I have added context to the invented narrative."
    );

    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/drafts/{}/acknowledge", s.record_id),
        Some(&taylor),
        Some(serde_json::json!({ "kind": "acknowledged" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "already_acknowledged");
}
