//! Milestone 4 slice 1: finalization — completion rules gating
//! submission and the sealing act, the immutable `EvaluationVersion`
//! with its content and chain hashes, reproducible presentation from
//! stored bytes, database-enforced immutability, and the API round
//! trip. Every fixture is invented.

use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, SET_COOKIE};
use axum::http::{Request, StatusCode};
use consolebook_server::capabilities::RoleBundle;
use consolebook_server::draft_content::{self, DraftContent, NarrativeEntry, RatingEntry};
use consolebook_server::draft_review::{self, ReviewDecisionKind};
use consolebook_server::evaluation_drafts::{self, DraftRefusal};
use consolebook_server::finalization::{self, FinalizeRefusal};
use consolebook_server::programs::{
    self, AnchorDef, CompetencyDef, FormCompetencyDef, FormDef, ModifierDef, NarrativeDef,
    PolicyDef, RecordType, ScaleDef, ScaleKind, TaskDef, VersionContent,
};
use consolebook_server::training_sessions::{self, SessionInput};
use consolebook_server::{
    assignments, canonical, data_dir::DataDir, enrollments, setup, storage, users,
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

    async fn publish(&self, content: &VersionContent) -> i64 {
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
        version_id
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

    async fn modifier_id(&self, version_id: i64, code: &str) -> i64 {
        sqlx::query_scalar(
            "SELECT id FROM rating_modifier
             WHERE program_version_id = ?1 AND code = ?2",
        )
        .bind(version_id)
        .bind(code)
        .fetch_one(&self.pool)
        .await
        .expect("modifier id")
    }

    async fn count(&self, sql: &str, bind: i64) -> i64 {
        sqlx::query_scalar(sql)
            .bind(bind)
            .fetch_one(&self.pool)
            .await
            .expect("count")
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

/// Invented program with the three scale kinds, one modifier, one
/// required and one optional narrative — completion rules per `policy`.
#[allow(clippy::too_many_lines)]
fn sealed_content(name: &str, policy: PolicyDef) -> VersionContent {
    VersionContent {
        name: name.to_owned(),
        label: "2026 rev A".to_owned(),
        description: "Invented program for finalization tests.".to_owned(),
        phases: Vec::new(),
        phase_transitions: Vec::new(),
        competencies: vec![
            CompetencyDef {
                category: "Call processing".to_owned(),
                name: "Emergency Call Interrogation".to_owned(),
                description: "Obtains and verifies location, callback, and nature.".to_owned(),
                tasks: vec![TaskDef {
                    prompt: "Processes an invented structure-fire call.".to_owned(),
                    citations: Vec::new(),
                }],
                citations: Vec::new(),
            },
            CompetencyDef {
                category: "Radio".to_owned(),
                name: "Radio Discipline".to_owned(),
                description: "Clear, complete transmissions.".to_owned(),
                tasks: Vec::new(),
                citations: Vec::new(),
            },
            CompetencyDef {
                category: String::new(),
                name: "Stress Response".to_owned(),
                description: "Composure under an invented surge.".to_owned(),
                tasks: Vec::new(),
                citations: Vec::new(),
            },
        ],
        rating_scales: vec![
            ScaleDef {
                name: "Standard 1-7".to_owned(),
                kind: ScaleKind::AnchoredNumeric,
                min_value: Some(1),
                max_value: Some(7),
                anchors: vec![
                    AnchorDef {
                        value: 1,
                        label: "Unacceptable".to_owned(),
                        definition: "Contrary to training.".to_owned(),
                    },
                    AnchorDef {
                        value: 4,
                        label: "Meets standards".to_owned(),
                        definition: "To the invented standard.".to_owned(),
                    },
                    AnchorDef {
                        value: 7,
                        label: "Superior".to_owned(),
                        definition: "Beyond the invented standard.".to_owned(),
                    },
                ],
            },
            ScaleDef {
                name: "Check".to_owned(),
                kind: ScaleKind::PassFail,
                min_value: None,
                max_value: None,
                anchors: vec![
                    AnchorDef {
                        value: 0,
                        label: "Fail".to_owned(),
                        definition: "Not yet to standard.".to_owned(),
                    },
                    AnchorDef {
                        value: 1,
                        label: "Pass".to_owned(),
                        definition: "To standard.".to_owned(),
                    },
                ],
            },
            ScaleDef {
                name: "Narrative Assessment".to_owned(),
                kind: ScaleKind::NarrativeOnly,
                min_value: None,
                max_value: None,
                anchors: Vec::new(),
            },
        ],
        rating_modifiers: vec![ModifierDef {
            code: "NRT".to_owned(),
            label: "Not responding to training".to_owned(),
            description: "Remedial attention documented in the narrative.".to_owned(),
        }],
        evaluation_forms: vec![FormDef {
            record_type: RecordType::DailyReport,
            name: "Daily Observation Report".to_owned(),
            instructions: "Rate observed performance.".to_owned(),
            competencies: vec![
                FormCompetencyDef {
                    competency: "Emergency Call Interrogation".to_owned(),
                    rating_scale: "Standard 1-7".to_owned(),
                },
                FormCompetencyDef {
                    competency: "Radio Discipline".to_owned(),
                    rating_scale: "Check".to_owned(),
                },
                FormCompetencyDef {
                    competency: "Stress Response".to_owned(),
                    rating_scale: "Narrative Assessment".to_owned(),
                },
            ],
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
        finalization_policy: policy,
    }
}

#[allow(clippy::struct_field_names)]
struct Seeded {
    version_id: i64,
    session_id: i64,
    record_id: i64,
    taylor_id: i64,
    jordan_id: i64,
    casey_id: i64,
}

/// Publishes `content`, seeds a trainee, an assigned trainer, and a
/// reviewer, opens one session, and starts its draft.
async fn seed(fx: &Fixture, content: &VersionContent, suffix: &str) -> Seeded {
    let version_id = fx.publish(content).await;
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
            trainer_user_ids: Vec::new(),
        },
    )
    .await
    .expect("call")
    .expect("created");
    let record_id = evaluation_drafts::create(&fx.pool, jordan_id, session_id, None)
        .await
        .expect("call")
        .expect("created");
    Seeded {
        version_id,
        session_id,
        record_id,
        taylor_id,
        jordan_id,
        casey_id,
    }
}

fn rating(form_competency_id: i64, value: Option<i64>, modifier_ids: Vec<i64>) -> RatingEntry {
    RatingEntry {
        form_competency_id,
        value,
        not_observed: false,
        modifier_ids,
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn completion_rules_gate_submission_and_sealing() {
    let fx = Fixture::new().await;
    let s = seed(
        &fx,
        &sealed_content("Example County CTO Program", PolicyDef::default()),
        "cto",
    )
    .await;
    let eci = fx
        .form_competency_id(s.version_id, "Emergency Call Interrogation")
        .await;
    let radio = fx
        .form_competency_id(s.version_id, "Radio Discipline")
        .await;
    let most = fx
        .narrative_id(s.version_id, "Most acceptable performance.")
        .await;
    let nrt = fx.modifier_id(s.version_id, "NRT").await;

    // An empty draft cannot even enter review: the required narrative
    // is missing.
    let refused = evaluation_drafts::submit(&fx.pool, s.jordan_id, s.record_id, 0)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::NarrativesIncomplete));

    // Narrative present, one valued competency unrated: still refused.
    let revision = draft_content::save(
        &fx.pool,
        s.jordan_id,
        s.record_id,
        0,
        &DraftContent {
            ratings: vec![rating(eci, Some(4), vec![nrt])],
            narratives: vec![NarrativeEntry {
                form_narrative_id: most,
                text: "Handled the invented fire call to standard.".to_owned(),
            }],
        },
    )
    .await
    .expect("call")
    .expect("saved");
    let refused = evaluation_drafts::submit(&fx.pool, s.jordan_id, s.record_id, revision)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::RatingsIncomplete));

    // The explicit not-observed marker completes it; a marker beside a
    // value is refused typed.
    let both = draft_content::save(
        &fx.pool,
        s.jordan_id,
        s.record_id,
        revision,
        &DraftContent {
            ratings: vec![
                rating(eci, Some(4), vec![nrt]),
                RatingEntry {
                    form_competency_id: radio,
                    value: Some(1),
                    not_observed: true,
                    modifier_ids: Vec::new(),
                },
            ],
            narratives: vec![NarrativeEntry {
                form_narrative_id: most,
                text: "Handled the invented fire call to standard.".to_owned(),
            }],
        },
    )
    .await
    .expect("call");
    assert_eq!(both, Err(DraftRefusal::ValueNotAllowed));
    let revision = draft_content::save(
        &fx.pool,
        s.jordan_id,
        s.record_id,
        revision,
        &DraftContent {
            ratings: vec![
                rating(eci, Some(4), vec![nrt]),
                RatingEntry {
                    form_competency_id: radio,
                    value: None,
                    not_observed: true,
                    modifier_ids: Vec::new(),
                },
            ],
            narratives: vec![NarrativeEntry {
                form_narrative_id: most,
                text: "Handled the invented fire call to standard.".to_owned(),
            }],
        },
    )
    .await
    .expect("call")
    .expect("saved");
    evaluation_drafts::submit(&fx.pool, s.jordan_id, s.record_id, revision)
        .await
        .expect("call")
        .expect("submitted");

    // Sealing an unapproved draft is refused; so is a sealer without
    // the capability; the database holds the approval gate raw.
    let refused = finalization::finalize(&fx.pool, s.casey_id, s.record_id, revision)
        .await
        .expect("call");
    assert_eq!(refused, Err(FinalizeRefusal::NotApproved));
    let refused = finalization::finalize(&fx.pool, s.taylor_id, s.record_id, revision)
        .await
        .expect("call");
    assert_eq!(refused, Err(FinalizeRefusal::CapabilityRequired));
    let raw = sqlx::query(
        "INSERT INTO evaluation_version
             (evaluation_record_id, version_number, record_schema, canonical_bytes,
              content_hash, chain_hash, predecessor_id, finalized_at, finalized_by)
         VALUES (?1, 1, 1, X'7B7D', ?2, ?2, NULL, 1, ?3)",
    )
    .bind(s.record_id)
    .bind("0".repeat(64))
    .bind(s.casey_id)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("takes an approved draft"), "raw gate: {err}");

    // Approved, the draft seals; the hashes verify; a second sealing is
    // refused at the service and raw.
    draft_review::decide(
        &fx.pool,
        s.casey_id,
        s.record_id,
        ReviewDecisionKind::Approved,
        None,
    )
    .await
    .expect("call")
    .expect("decided");
    // Sealing carries the viewed revision, exactly like submission.
    let stale = finalization::finalize(&fx.pool, s.casey_id, s.record_id, revision + 1)
        .await
        .expect("call");
    assert_eq!(stale, Err(FinalizeRefusal::StaleSave));
    let meta = finalization::finalize(&fx.pool, s.casey_id, s.record_id, revision)
        .await
        .expect("call")
        .expect("sealed");
    assert_eq!(meta.version_number, 1);
    assert_eq!(meta.content_hash.len(), 64);
    assert_eq!(meta.chain_hash.len(), 64);
    let again = finalization::finalize(&fx.pool, s.casey_id, s.record_id, revision)
        .await
        .expect("call");
    assert_eq!(again, Err(FinalizeRefusal::AlreadyFinalized));

    // The stored bytes reproduce: re-canonicalizing the parsed envelope
    // yields the stored content hash, and verification agrees.
    let view = finalization::finalized_view(&fx.pool, s.casey_id, s.record_id)
        .await
        .expect("call")
        .expect("readable")
        .expect("finalized");
    let bytes = canonical::canonical_bytes(&view.envelope).expect("canonical");
    assert_eq!(canonical::content_hash_hex(&bytes), meta.content_hash);
    let verification = finalization::verify(&fx.pool, s.casey_id, s.record_id)
        .await
        .expect("call")
        .expect("readable")
        .expect("finalized");
    assert!(verification.content_hash_ok);
    assert!(verification.chain_hash_ok);

    // The envelope carries the historical presentation.
    let envelope = &view.envelope;
    assert_eq!(envelope["canonicalization"], "jcs-v1");
    assert_eq!(
        envelope["record"]["record_schema"],
        consolebook_server::canonical::RECORD_SCHEMA
    );
    // Schema 2 (ADR 0013): the coverage member is always present and
    // empty for a record that links nothing.
    assert_eq!(envelope["daily_reports"], serde_json::json!([]));
    assert_eq!(
        envelope["record"]["predecessor_content_hash"],
        serde_json::Value::Null
    );
    assert_eq!(envelope["trainee"]["display_name"], "Taylor Trainee");
    assert_eq!(envelope["program"]["name"], "Example County CTO Program");
    assert_eq!(envelope["form"]["record_type"], "daily_report");
    let ratings = envelope["content"]["ratings"].as_array().expect("rows");
    assert_eq!(ratings.len(), 3);
    assert_eq!(ratings[0]["value"], 4);
    assert_eq!(ratings[0]["modifiers"][0]["code"], "NRT");
    assert_eq!(
        ratings[0]["competency"]["tasks"][0],
        "Processes an invented structure-fire call."
    );
    assert_eq!(
        ratings[0]["scale"]["anchors"][1]["label"],
        "Meets standards"
    );
    assert_eq!(ratings[1]["not_observed"], true);
    assert_eq!(ratings[1]["value"], serde_json::Value::Null);
    let narratives = envelope["content"]["narratives"].as_array().expect("rows");
    assert_eq!(narratives[0]["required"], true);
    assert_eq!(
        narratives[0]["text"],
        "Handled the invented fire call to standard."
    );
    assert_eq!(envelope["review"][0]["decision"], "approved");
    assert_eq!(envelope["sessions"][0]["business_date"], "2026-06-02");
    let kinds: Vec<&str> = envelope["attribution"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|event| event["kind"].as_str().expect("kind"))
        .collect();
    // Consecutive saves by one contributor coalesce, so the exact count
    // of contributed events is the stream's business; the shape is not.
    assert_eq!(kinds.first(), Some(&"created"));
    assert_eq!(kinds.last(), Some(&"review_decided"));
    assert!(kinds.contains(&"contributed"));
    assert!(kinds.contains(&"submitted_for_review"));

    // Finalized is terminal: every service write refuses typed, and the
    // database refuses raw content edits, event appends, and version
    // mutations.
    let refused = draft_content::save(
        &fx.pool,
        s.jordan_id,
        s.record_id,
        revision + 1,
        &DraftContent {
            ratings: Vec::new(),
            narratives: Vec::new(),
        },
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(DraftRefusal::DraftFinalized));
    let refused = evaluation_drafts::submit(&fx.pool, s.jordan_id, s.record_id, revision + 1)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::DraftFinalized));
    let refused = draft_review::decide(
        &fx.pool,
        s.casey_id,
        s.record_id,
        ReviewDecisionKind::Returned,
        None,
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(DraftRefusal::NotSubmitted));
    let raw = sqlx::query("UPDATE draft_rating SET value = 7 WHERE evaluation_record_id = ?1")
        .bind(s.record_id)
        .execute(&fx.pool)
        .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("is frozen"), "content: {err}");
    let raw = sqlx::query(
        "INSERT INTO contributor_event
             (evaluation_record_id, kind, actor_user_id, to_user_id, recorded_at)
         VALUES (?1, 'contributed', ?2, NULL, 9)",
    )
    .bind(s.record_id)
    .bind(s.jordan_id)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("is frozen"), "events: {err}");
    let raw = sqlx::query("UPDATE evaluation_version SET content_hash = ?1")
        .bind("f".repeat(64))
        .execute(&fx.pool)
        .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("immutable while retained"), "update: {err}");
    let raw = sqlx::query("DELETE FROM evaluation_version")
        .execute(&fx.pool)
        .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("immutable while retained"), "delete: {err}");
    let raw = sqlx::query(
        "INSERT INTO evaluation_version
             (evaluation_record_id, version_number, record_schema, canonical_bytes,
              content_hash, chain_hash, predecessor_id, finalized_at, finalized_by)
         VALUES (?1, 1, 1, X'7B7D', ?2, ?2, NULL, 1, ?3)",
    )
    .bind(s.record_id)
    .bind("0".repeat(64))
    .bind(s.casey_id)
    .execute(&fx.pool)
    .await;
    // Since migration 0012, a duplicate first version meets the UNIQUE
    // constraint; true successors are the amendment contract's to
    // admit (proven in the amendments suite).
    let err = raw.expect_err("must be refused").to_string();
    assert!(
        err.contains("UNIQUE constraint failed"),
        "second version: {err}"
    );

    // The sealing is audited and the owner hears about it.
    assert_eq!(
        fx.count(
            "SELECT COUNT(*) FROM audit_event WHERE kind = 'draft_finalized'
             AND actor_user_id = ?1",
            s.casey_id,
        )
        .await,
        1
    );
    assert_eq!(
        fx.count(
            "SELECT COUNT(*) FROM notice WHERE kind = 'draft_finalized' AND user_id = ?1",
            s.jordan_id,
        )
        .await,
        1
    );
}

#[tokio::test]
async fn policy_off_seals_without_review() {
    let fx = Fixture::new().await;
    let s = seed(
        &fx,
        &sealed_content(
            "Example County Annual In-Service",
            PolicyDef {
                review_approved: false,
                required_narratives: false,
                ratings_complete: false,
            },
        ),
        "inservice",
    )
    .await;

    // No submission, no review, no content: the configured rules are
    // all off, so the empty draft seals directly from its open state.
    let refused = finalization::finalize(&fx.pool, s.jordan_id, s.record_id, 0)
        .await
        .expect("call");
    assert_eq!(refused, Err(FinalizeRefusal::CapabilityRequired));
    // The copy stays editable up to sealing here, so the race Codex
    // named is live: a save landing after the finalizer viewed the
    // record resolves as a stale refusal, never sealed sight unseen.
    let revision = draft_content::save(
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
    .expect("call")
    .expect("saved");
    let stale = finalization::finalize(&fx.pool, s.casey_id, s.record_id, 0)
        .await
        .expect("call");
    assert_eq!(stale, Err(FinalizeRefusal::StaleSave));
    let meta = finalization::finalize(&fx.pool, s.casey_id, s.record_id, revision)
        .await
        .expect("call")
        .expect("sealed");
    assert_eq!(meta.version_number, 1);
    let view = finalization::finalized_view(&fx.pool, s.casey_id, s.record_id)
        .await
        .expect("call")
        .expect("readable")
        .expect("finalized");
    assert_eq!(
        view.envelope["finalization"]["policy"]["review_approved"],
        false
    );
    assert_eq!(view.envelope["review"].as_array().expect("rows").len(), 0);
    // A cancelled-session-free open session stays untouched; the frozen
    // record's coverage is sealed with it.
    let raw = sqlx::query("DELETE FROM evaluation_session WHERE evaluation_record_id = ?1")
        .bind(s.record_id)
        .execute(&fx.pool)
        .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("is frozen"), "coverage: {err}");
    let _ = s.session_id;
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn finalize_api_round_trip() {
    let fx = Fixture::new().await;
    let s = seed(
        &fx,
        &sealed_content("Example County CTO Program", PolicyDef::default()),
        "api",
    )
    .await;
    let eci = fx
        .form_competency_id(s.version_id, "Emergency Call Interrogation")
        .await;
    let radio = fx
        .form_competency_id(s.version_id, "Radio Discipline")
        .await;
    let most = fx
        .narrative_id(s.version_id, "Most acceptable performance.")
        .await;
    let jordan = fx.login("jordan.api", PASSWORD).await;
    let casey = fx.login("casey.api", PASSWORD).await;

    // Incomplete submission refuses over the API with the typed code.
    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/drafts/{}/submit", s.record_id),
        Some(&jordan),
        Some(serde_json::json!({ "revision": 0 })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "narratives_incomplete");

    let (status, body) = request(
        fx.app(),
        "PUT",
        &format!("/api/drafts/{}/content", s.record_id),
        Some(&jordan),
        Some(serde_json::json!({
            "revision": 0,
            "ratings": [
                { "form_competency_id": eci, "value": 4, "modifier_ids": [] },
                { "form_competency_id": radio, "value": null, "not_observed": true,
                  "modifier_ids": [] }
            ],
            "narratives": [
                { "form_narrative_id": most,
                  "text": "Cleared the invented medical call correctly." }
            ]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "save: {body}");
    let revision = body["revision"].as_i64().expect("revision");
    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/drafts/{}/submit", s.record_id),
        Some(&jordan),
        Some(serde_json::json!({ "revision": revision })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "submit: {body}");

    // Finalize before approval: typed conflict. Without the capability:
    // forbidden.
    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/drafts/{}/finalize", s.record_id),
        Some(&casey),
        Some(serde_json::json!({ "revision": revision })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "not_approved");
    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/drafts/{}/finalize", s.record_id),
        Some(&jordan),
        Some(serde_json::json!({ "revision": revision })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "capability_required");
    let (status, _) = request(
        fx.app(),
        "GET",
        &format!("/api/drafts/{}/version", s.record_id),
        Some(&casey),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Approve, seal, read, verify.
    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/drafts/{}/review", s.record_id),
        Some(&casey),
        Some(serde_json::json!({ "decision": "approved" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "approve: {body}");
    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/drafts/{}/finalize", s.record_id),
        Some(&casey),
        Some(serde_json::json!({ "revision": revision })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "finalize: {body}");
    assert_eq!(body["version_number"], 1);
    let content_hash = body["content_hash"].as_str().expect("hash").to_owned();
    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/drafts/{}/version", s.record_id),
        Some(&casey),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["meta"]["content_hash"], content_hash.as_str());
    assert_eq!(
        body["envelope"]["trainee"]["display_name"],
        "Taylor Trainee"
    );
    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/drafts/{}/version/verify", s.record_id),
        Some(&casey),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["content_hash_ok"], true);
    assert_eq!(body["chain_hash_ok"], true);
    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/drafts/{}/finalize", s.record_id),
        Some(&casey),
        Some(serde_json::json!({ "revision": revision })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "already_finalized");

    // The workspace reports the terminal status and the sealed flag.
    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/drafts/{}", s.record_id),
        Some(&casey),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "finalized");
    assert_eq!(body["viewer_may_finalize"], false);
}
