//! Milestone 4 slice 3: amendments and successor versions — a
//! correction reopens the working copy through a recorded amendment,
//! travels the configured workflow, and seals as the next immutable
//! version chained to its predecessor; both versions stay readable,
//! the successor starts unacknowledged, and the database holds the
//! contract raw. Every fixture is invented.

use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, SET_COOKIE};
use axum::http::{Request, StatusCode};
use consolebook_server::acknowledgments::{self, TraineeAckKind};
use consolebook_server::amendments::{self, AmendRefusal};
use consolebook_server::capabilities::RoleBundle;
use consolebook_server::draft_content::{self, DraftContent, NarrativeEntry, RatingEntry};
use consolebook_server::draft_review::{self, ReviewDecisionKind};
use consolebook_server::evaluation_drafts::{self, DraftRefusal, DraftStatus};
use consolebook_server::finalization::{self, FinalizeRefusal};
use consolebook_server::programs::{
    self, AnchorDef, CompetencyDef, FormCompetencyDef, FormDef, NarrativeDef, PolicyDef,
    RecordType, ScaleDef, ScaleKind, VersionContent,
};
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

const REVIEW_POLICY: PolicyDef = PolicyDef {
    review_approved: true,
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

/// Invented single-form program; completion rules per `policy`.
fn program(name: &str, policy: PolicyDef) -> VersionContent {
    VersionContent {
        name: name.to_owned(),
        label: "2026 rev A".to_owned(),
        description: "Invented program for amendment tests.".to_owned(),
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

async fn revision_of(fx: &Fixture, record_id: i64) -> i64 {
    sqlx::query_scalar("SELECT revision FROM evaluation_record WHERE id = ?1")
        .bind(record_id)
        .fetch_one(&fx.pool)
        .await
        .expect("revision")
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn amendment_produces_a_chained_successor() {
    let fx = Fixture::new().await;
    let s = seed(&fx, OPEN_POLICY, "chain").await;

    // Amendments correct finalized records only, with authority and a
    // reason.
    let refused = amendments::open(&fx.pool, s.casey_id, s.record_id, "premature")
        .await
        .expect("call");
    assert_eq!(refused, Err(AmendRefusal::NotFinalized));
    // Version 1 carries jordan's authored content, so the amendment
    // cycle below starts from a stretch this same contributor owns.
    let seeded_workspace = evaluation_drafts::workspace(&fx.pool, s.jordan_id, s.record_id)
        .await
        .expect("call")
        .expect("readable");
    let eci = seeded_workspace.form.competencies[0].form_competency_id;
    let most = seeded_workspace.form.narratives[0].form_narrative_id;
    let revision = draft_content::save(
        &fx.pool,
        s.jordan_id,
        s.record_id,
        0,
        &DraftContent {
            ratings: vec![RatingEntry {
                form_competency_id: eci,
                value: Some(3),
                not_observed: false,
                modifier_ids: Vec::new(),
            }],
            narratives: vec![NarrativeEntry {
                form_narrative_id: most,
                text: "The invented initial entry.".to_owned(),
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
    let refused = amendments::open(&fx.pool, s.casey_id, s.record_id, " \u{2003} ")
        .await
        .expect("call");
    assert_eq!(refused, Err(AmendRefusal::ReasonRequired));
    let refused = amendments::open(&fx.pool, s.jordan_id, s.record_id, "not my authority")
        .await
        .expect("call");
    assert_eq!(refused, Err(AmendRefusal::CapabilityRequired));
    let refused = amendments::open(&fx.pool, s.casey_id, 9999, "no such record")
        .await
        .expect("call");
    assert_eq!(refused, Err(AmendRefusal::NoSuchRecord));

    // The trainee acknowledged version 1; that act stays with it.
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

    // Opening reopens the one working copy: audited, the owner told,
    // one correction cycle at a time.
    amendments::open(
        &fx.pool,
        s.casey_id,
        s.record_id,
        "The invented rating was entered one point low.",
    )
    .await
    .expect("call")
    .expect("opened");
    assert_eq!(fx.notice_count(s.jordan_id, "amendment_opened").await, 1);
    let audited: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_event WHERE kind = 'amendment_opened'")
            .fetch_one(&fx.pool)
            .await
            .expect("audit");
    assert_eq!(audited, 1);
    let refused = amendments::open(&fx.pool, s.casey_id, s.record_id, "twice")
        .await
        .expect("call");
    assert_eq!(refused, Err(AmendRefusal::AmendmentOpen));

    // Opening advanced the revision: a save carrying the prior cycle's
    // token is a typed stale refusal, never a silent overwrite.
    let stale_save = draft_content::save(
        &fx.pool,
        s.jordan_id,
        s.record_id,
        0,
        &DraftContent {
            ratings: Vec::new(),
            narratives: Vec::new(),
        },
    )
    .await
    .expect("call");
    assert_eq!(stale_save, Err(DraftRefusal::StaleSave));

    // The copy is editable again — the reopened cycle, not the stale
    // approved state — and the correction lands in it.
    let workspace = evaluation_drafts::workspace(&fx.pool, s.jordan_id, s.record_id)
        .await
        .expect("call")
        .expect("readable");
    assert_eq!(workspace.detail.status, DraftStatus::Draft);
    assert_eq!(
        workspace
            .detail
            .open_amendment
            .as_ref()
            .expect("open amendment")
            .reason,
        "The invented rating was entered one point low."
    );
    let revision = draft_content::save(
        &fx.pool,
        s.jordan_id,
        s.record_id,
        workspace.detail.revision,
        &DraftContent {
            ratings: vec![RatingEntry {
                form_competency_id: eci,
                value: Some(4),
                not_observed: false,
                modifier_ids: Vec::new(),
            }],
            narratives: vec![NarrativeEntry {
                form_narrative_id: most,
                text: "Corrected the invented rating with context.".to_owned(),
            }],
        },
    )
    .await
    .expect("call")
    .expect("saved");

    // The correction's first save is attributed within its own cycle:
    // it never coalesces into a stretch the sealed version already
    // owns, even for the same contributor (ADR 0012).
    let contributed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM contributor_event
         WHERE evaluation_record_id = ?1 AND kind = 'contributed'",
    )
    .bind(s.record_id)
    .fetch_one(&fx.pool)
    .await
    .expect("count");
    assert_eq!(contributed, 2, "one contributed event per cycle stretch");

    // The in-progress correction is not the trainee's to see: their
    // own-record read presents the sealed self — finalized status, no
    // working copy, no reopened-cycle events, no amendment internals.
    let mine = evaluation_drafts::workspace(&fx.pool, s.taylor_id, s.record_id)
        .await
        .expect("call")
        .expect("readable");
    assert_eq!(mine.detail.status, DraftStatus::Finalized);
    assert!(mine.content.ratings.is_empty() && mine.content.narratives.is_empty());
    assert!(mine.detail.open_amendment.is_none());
    assert_eq!(
        mine.detail
            .events
            .iter()
            .filter(|event| event.kind == "contributed")
            .count(),
        1,
        "the reopened cycle's events stay out of the own-record read"
    );

    // Sealing the correction produces version 2, chained to version 1
    // exactly as ADR 0011 pinned.
    let stale = finalization::finalize(&fx.pool, s.casey_id, s.record_id, revision + 7)
        .await
        .expect("call");
    assert_eq!(stale, Err(FinalizeRefusal::StaleSave));
    let meta = finalization::finalize(&fx.pool, s.casey_id, s.record_id, revision)
        .await
        .expect("call")
        .expect("sealed");
    assert_eq!(meta.version_number, 2);
    let (_, _, v1_content, _) = fx.version_row(s.record_id, 1).await;
    let (_, v2_bytes, v2_content, v2_chain) = fx.version_row(s.record_id, 2).await;
    assert_eq!(canonical::content_hash_hex(&v2_bytes), v2_content);
    assert_eq!(
        canonical::chain_hash_hex(Some(&v1_content), &v2_bytes).expect("chain"),
        v2_chain
    );
    let envelope: serde_json::Value = serde_json::from_slice(&v2_bytes).expect("envelope");
    assert_eq!(envelope["record"]["version_number"], 2);
    assert_eq!(envelope["record"]["predecessor_content_hash"], v1_content);
    let verification = finalization::verify(&fx.pool, s.casey_id, s.record_id)
        .await
        .expect("call")
        .expect("readable")
        .expect("finalized");
    assert!(verification.content_hash_ok && verification.chain_hash_ok);

    // Re-finalizing without a new amendment is refused; the record is
    // sealed again.
    let refused = finalization::finalize(&fx.pool, s.casey_id, s.record_id, revision)
        .await
        .expect("call");
    assert_eq!(refused, Err(FinalizeRefusal::AlreadyFinalized));

    // The successor starts unacknowledged; version 1 keeps its act.
    let current = acknowledgments::acknowledgment_of(&fx.pool, s.casey_id, s.record_id)
        .await
        .expect("call")
        .expect("readable");
    assert!(
        current.is_none(),
        "a successor never inherits acknowledgment"
    );
    let history = amendments::history(&fx.pool, s.casey_id, s.record_id)
        .await
        .expect("call")
        .expect("readable");
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].version_number, 2);
    assert_eq!(
        history[0].amendment.as_ref().expect("amendment").reason,
        "The invented rating was entered one point low."
    );
    assert!(history[0].acknowledgment.is_none());
    assert_eq!(history[1].version_number, 1);
    assert!(history[1].amendment.is_none());
    assert_eq!(
        history[1]
            .acknowledgment
            .as_ref()
            .expect("original act")
            .kind,
        "acknowledged"
    );
    assert_eq!(history[0].finalized_by_display_name, "Casey Coordinator");

    // The trainee is told again and the timeline shows the amended
    // record awaiting them; a fresh acknowledgment binds version 2.
    assert_eq!(
        fx.notice_count(s.taylor_id, "record_awaits_acknowledgment")
            .await,
        2
    );
    let timeline = acknowledgments::own_records(&fx.pool, s.taylor_id)
        .await
        .expect("call")
        .expect("listed");
    assert_eq!(timeline.len(), 1);
    assert_eq!(timeline[0].record_version_number, 2);
    assert_eq!(timeline[0].acknowledgment_kind, None);
    acknowledgments::acknowledge(
        &fx.pool,
        s.taylor_id,
        s.record_id,
        TraineeAckKind::AcknowledgedWithResponse,
        "Received the invented correction.",
    )
    .await
    .expect("call")
    .expect("acknowledged again");
    let history = amendments::history(&fx.pool, s.casey_id, s.record_id)
        .await
        .expect("call")
        .expect("readable");
    assert_eq!(
        history[0].acknowledgment.as_ref().expect("new act").kind,
        "acknowledged_with_response"
    );

    // Every retained version stays readable — the superseded original
    // included, for the trainee too — and verifies by number.
    let original = finalization::finalized_view_at(&fx.pool, s.taylor_id, s.record_id, Some(1))
        .await
        .expect("call")
        .expect("readable")
        .expect("retained");
    assert_eq!(original.meta.version_number, 1);
    assert_eq!(original.envelope["record"]["version_number"], 1);
    let checked = finalization::verify_at(&fx.pool, s.taylor_id, s.record_id, Some(1))
        .await
        .expect("call")
        .expect("readable")
        .expect("retained");
    assert!(checked.content_hash_ok && checked.chain_hash_ok);
    let missing = finalization::finalized_view_at(&fx.pool, s.taylor_id, s.record_id, Some(3))
        .await
        .expect("call")
        .expect("readable");
    assert!(missing.is_none());
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn raw_writes_meet_the_amendment_contract() {
    let fx = Fixture::new().await;
    let s = seed(&fx, OPEN_POLICY, "raw").await;
    finalization::finalize(&fx.pool, s.casey_id, s.record_id, 0)
        .await
        .expect("call")
        .expect("sealed");
    let (v1_id, _, _, _) = fx.version_row(s.record_id, 1).await;

    // A successor without its amendment is refused raw.
    let raw = sqlx::query(
        "INSERT INTO evaluation_version
             (evaluation_record_id, version_number, record_schema, canonical_bytes,
              content_hash, chain_hash, predecessor_id, finalized_at, finalized_by)
         VALUES (?1, 2, 1, X'7B7D', ?2, ?2, ?3, 1, ?4)",
    )
    .bind(s.record_id)
    .bind("0".repeat(64))
    .bind(v1_id)
    .bind(s.casey_id)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("arrives with its amendment"), "raw: {err}");

    // Amendment forgeries: a blank reason, forged reopening marks, and
    // a target that is not the record's own latest version.
    let raw = sqlx::query(
        "INSERT INTO amendment
             (evaluation_record_id, predecessor_version_id, reason, opened_by,
              opened_by_display_name, opened_at, opened_after_event_id,
              opened_after_decision_id)
         SELECT ?1, ?2, char(9, 10, 32), ?3, 'Casey Coordinator', 1,
                COALESCE(MAX(ce.id), 0),
                0
         FROM contributor_event ce WHERE ce.evaluation_record_id = ?1",
    )
    .bind(s.record_id)
    .bind(v1_id)
    .bind(s.casey_id)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("CHECK constraint failed"), "blank: {err}");
    let raw = sqlx::query(
        "INSERT INTO amendment
             (evaluation_record_id, predecessor_version_id, reason, opened_by,
              opened_by_display_name, opened_at, opened_after_event_id,
              opened_after_decision_id)
         VALUES (?1, ?2, 'forged marks', ?3, 'Casey Coordinator', 1, 0, 0)",
    )
    .bind(s.record_id)
    .bind(v1_id)
    .bind(s.casey_id)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(
        err.contains("workflow position it reopened from"),
        "marks: {err}"
    );
    let other_record = draft_for(&fx, s.jordan_id, s.enrollment_id, "2026-06-03").await;
    finalization::finalize(&fx.pool, s.casey_id, other_record, 0)
        .await
        .expect("call")
        .expect("sealed");
    let (other_v1, _, _, _) = fx.version_row(other_record, 1).await;
    let raw = sqlx::query(
        "INSERT INTO amendment
             (evaluation_record_id, predecessor_version_id, reason, opened_by,
              opened_by_display_name, opened_at, opened_after_event_id,
              opened_after_decision_id)
         VALUES (?1, ?2, 'wrong record', ?3, 'Casey Coordinator', 1,
                 (SELECT COALESCE(MAX(id), 0) FROM contributor_event
                  WHERE evaluation_record_id = ?1),
                 (SELECT COALESCE(MAX(id), 0) FROM review_decision
                  WHERE evaluation_record_id = ?1))",
    )
    .bind(s.record_id)
    .bind(other_v1)
    .bind(s.casey_id)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(
        err.contains("latest finalized version"),
        "wrong record: {err}"
    );

    // A real amendment is permanent, and successors arrive only next
    // in order on the true predecessor.
    amendments::open(
        &fx.pool,
        s.casey_id,
        s.record_id,
        "correcting the invented entry",
    )
    .await
    .expect("call")
    .expect("opened");
    let raw = sqlx::query("UPDATE amendment SET reason = 'rewritten' WHERE 1 = 1")
        .execute(&fx.pool)
        .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("permanent"), "update: {err}");
    let raw = sqlx::query("DELETE FROM amendment WHERE 1 = 1")
        .execute(&fx.pool)
        .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("permanent"), "delete: {err}");
    let raw = sqlx::query(
        "INSERT INTO evaluation_version
             (evaluation_record_id, version_number, record_schema, canonical_bytes,
              content_hash, chain_hash, predecessor_id, finalized_at, finalized_by)
         VALUES (?1, 3, 1, X'7B7D', ?2, ?2, ?3, 1, ?4)",
    )
    .bind(s.record_id)
    .bind("0".repeat(64))
    .bind(v1_id)
    .bind(s.casey_id)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("extends the latest version"), "order: {err}");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn amended_record_travels_review_when_configured() {
    let fx = Fixture::new().await;
    let s = seed(&fx, REVIEW_POLICY, "review").await;

    // Version 1 through the configured workflow.
    evaluation_drafts::submit(&fx.pool, s.jordan_id, s.record_id, 0)
        .await
        .expect("call")
        .expect("submitted");
    draft_review::decide(
        &fx.pool,
        s.casey_id,
        s.record_id,
        ReviewDecisionKind::Approved,
        None,
    )
    .await
    .expect("call")
    .expect("approved");
    let revision = revision_of(&fx, s.record_id).await;
    finalization::finalize(&fx.pool, s.casey_id, s.record_id, revision)
        .await
        .expect("call")
        .expect("sealed");

    // The reopened cycle owes its own approval: the superseded cycle's
    // decision never leaks through, at the service or raw.
    amendments::open(
        &fx.pool,
        s.casey_id,
        s.record_id,
        "invented date correction",
    )
    .await
    .expect("call")
    .expect("opened");
    let workspace = evaluation_drafts::workspace(&fx.pool, s.casey_id, s.record_id)
        .await
        .expect("call")
        .expect("readable");
    assert_eq!(workspace.detail.status, DraftStatus::Draft);
    let revision = revision_of(&fx, s.record_id).await;
    let refused = finalization::finalize(&fx.pool, s.casey_id, s.record_id, revision)
        .await
        .expect("call");
    assert_eq!(refused, Err(FinalizeRefusal::NotApproved));
    let (v1_id, _, _, _) = fx.version_row(s.record_id, 1).await;
    let raw = sqlx::query(
        "INSERT INTO evaluation_version
             (evaluation_record_id, version_number, record_schema, canonical_bytes,
              content_hash, chain_hash, predecessor_id, finalized_at, finalized_by)
         VALUES (?1, 2, 1, X'7B7D', ?2, ?2, ?3, 1, ?4)",
    )
    .bind(s.record_id)
    .bind("0".repeat(64))
    .bind(v1_id)
    .bind(s.casey_id)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("approved draft"), "raw approval: {err}");

    // The correction earns its approval, then seals as version 2.
    evaluation_drafts::submit(&fx.pool, s.jordan_id, s.record_id, revision)
        .await
        .expect("call")
        .expect("resubmitted");
    let workspace = evaluation_drafts::workspace(&fx.pool, s.casey_id, s.record_id)
        .await
        .expect("call")
        .expect("readable");
    assert_eq!(workspace.detail.status, DraftStatus::Submitted);
    assert!(workspace.detail.viewer_may_review);
    draft_review::decide(
        &fx.pool,
        s.casey_id,
        s.record_id,
        ReviewDecisionKind::Approved,
        None,
    )
    .await
    .expect("call")
    .expect("approved again");
    let revision = revision_of(&fx, s.record_id).await;
    let meta = finalization::finalize(&fx.pool, s.casey_id, s.record_id, revision)
        .await
        .expect("call")
        .expect("sealed");
    assert_eq!(meta.version_number, 2);
    let verification = finalization::verify(&fx.pool, s.casey_id, s.record_id)
        .await
        .expect("call")
        .expect("readable")
        .expect("finalized");
    assert!(verification.content_hash_ok && verification.chain_hash_ok);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn amendment_api_round_trip() {
    let fx = Fixture::new().await;
    let s = seed(&fx, OPEN_POLICY, "api").await;
    finalization::finalize(&fx.pool, s.casey_id, s.record_id, 0)
        .await
        .expect("call")
        .expect("sealed");
    let casey = fx.login("casey.api", PASSWORD).await;
    let taylor = fx.login("taylor.api", PASSWORD).await;

    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/drafts/{}/amend", s.record_id),
        Some(&taylor),
        Some(serde_json::json!({ "reason": "not my authority" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "capability_required");
    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/drafts/{}/amend", s.record_id),
        Some(&casey),
        Some(serde_json::json!({ "reason": "   " })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "reason_required");
    let (status, _) = request(
        fx.app(),
        "POST",
        &format!("/api/drafts/{}/amend", s.record_id),
        Some(&casey),
        Some(serde_json::json!({ "reason": "Correct the invented narrative." })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/drafts/{}/amend", s.record_id),
        Some(&casey),
        Some(serde_json::json!({ "reason": "twice" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "amendment_open");

    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/drafts/{}", s.record_id),
        Some(&casey),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "draft");
    assert_eq!(
        body["open_amendment"]["reason"],
        "Correct the invented narrative."
    );
    assert_eq!(body["latest_version_number"], 1);

    // Seal the correction; both versions list for the trainee, whose
    // timeline shows the amended record awaiting acknowledgment again.
    let revision: i64 = body["revision"].as_i64().expect("revision");
    let (status, _) = request(
        fx.app(),
        "POST",
        &format!("/api/drafts/{}/finalize", s.record_id),
        Some(&casey),
        Some(serde_json::json!({ "revision": revision })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/drafts/{}/versions", s.record_id),
        Some(&taylor),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let versions = body["versions"].as_array().expect("versions");
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0]["version_number"], 2);
    assert_eq!(
        versions[0]["amendment"]["reason"],
        "Correct the invented narrative."
    );
    let (status, body) = request(fx.app(), "GET", "/api/my/records", Some(&taylor), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["records"][0]["record_version_number"], 2);
    assert_eq!(
        body["records"][0]["acknowledgment_kind"],
        serde_json::Value::Null
    );

    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/drafts/{}/versions/1", s.record_id),
        Some(&taylor),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["envelope"]["record"]["version_number"], 1);
    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/drafts/{}/versions/9", s.record_id),
        Some(&taylor),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "no_such_version");
}
