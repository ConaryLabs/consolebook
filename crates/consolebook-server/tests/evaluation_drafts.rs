//! Milestone 3 slice 3: daily evaluation drafts — one-per-session policy,
//! capability-plus-scope gates, scale-kind validation against the pinned
//! vocabulary, coalesced contributor attribution, event-mediated
//! ownership, and the submission snapshot that freezes the working copy.
//! Every fixture is invented.

use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, SET_COOKIE};
use axum::http::{Request, StatusCode};
use consolebook_server::capabilities::RoleBundle;
use consolebook_server::draft_content::{self, DraftContent, NarrativeEntry, RatingEntry};
use consolebook_server::evaluation_drafts::{self, DraftRefusal, DraftStatus};
use consolebook_server::programs::{
    self, AnchorDef, CompetencyDef, FormCompetencyDef, FormDef, ModifierDef, NarrativeDef,
    PhaseDef, RecordType, ScaleDef, ScaleKind, TaskDef, TransitionDef, TransitionKind,
    VersionContent,
};
use consolebook_server::training_sessions::{self, SessionInput};
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

    async fn event_kinds(&self, record_id: i64) -> Vec<(String, i64)> {
        sqlx::query_as(
            "SELECT kind, actor_user_id FROM contributor_event
             WHERE evaluation_record_id = ?1 ORDER BY id",
        )
        .bind(record_id)
        .fetch_all(&self.pool)
        .await
        .expect("events")
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

/// Invented program content with a complete daily report form: three
/// competencies across the three scale kinds, two narrative prompts, and
/// one modifier.
#[allow(clippy::too_many_lines)]
fn evaluated_content() -> VersionContent {
    VersionContent {
        name: "Example County CTO Program".to_owned(),
        label: "2026 rev A".to_owned(),
        description: "Invented program for draft tests.".to_owned(),
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
                description: "Uses clear text and unit identifiers.".to_owned(),
                tasks: vec![TaskDef {
                    prompt: "Dispatches an invented medical call.".to_owned(),
                    citations: Vec::new(),
                }],
                citations: Vec::new(),
            },
            CompetencyDef {
                category: "Performance".to_owned(),
                name: "Stress Response".to_owned(),
                description: "Maintains composure under load.".to_owned(),
                tasks: vec![TaskDef {
                    prompt: "Handles an invented multi-call surge.".to_owned(),
                    citations: Vec::new(),
                }],
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
                        definition: "Did not perform the invented task.".to_owned(),
                    },
                    AnchorDef {
                        value: 1,
                        label: "Pass".to_owned(),
                        definition: "Performed the invented task.".to_owned(),
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
            instructions: "Rate today's observed performance.".to_owned(),
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
    }
}

#[allow(clippy::struct_field_names)]
struct Seeded {
    version_id: i64,
    enrollment_id: i64,
    session_id: i64,
    taylor_id: i64,
    jordan_id: i64,
    rowan_id: i64,
}

/// Publishes the evaluated program and seeds one trainee (Taylor), an
/// assigned trainer who worked the session (Jordan), an unassigned
/// non-member trainer (Rowan), and one open session.
async fn seed(fx: &Fixture) -> Seeded {
    let version_id = fx.publish(&evaluated_content()).await;
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
    Seeded {
        version_id,
        enrollment_id,
        session_id,
        taylor_id,
        jordan_id,
        rowan_id,
    }
}

fn rating(form_competency_id: i64, value: Option<i64>, modifier_ids: Vec<i64>) -> RatingEntry {
    RatingEntry {
        form_competency_id,
        value,
        modifier_ids,
    }
}

fn narrative(form_narrative_id: i64, text: &str) -> NarrativeEntry {
    NarrativeEntry {
        form_narrative_id,
        text: text.to_owned(),
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn create_gates_policy_and_form_resolution() {
    let fx = Fixture::new().await;
    let s = seed(&fx).await;

    // The trainee and an out-of-scope author are both refused.
    let refused = evaluation_drafts::create(&fx.pool, s.taylor_id, s.session_id, None)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::CapabilityRequired));
    let refused = evaluation_drafts::create(&fx.pool, s.rowan_id, s.session_id, None)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::CapabilityRequired));

    // The session member creates the draft; the one-per-session policy
    // refuses a second, whoever asks.
    let record_id = evaluation_drafts::create(&fx.pool, s.jordan_id, s.session_id, None)
        .await
        .expect("call")
        .expect("created");
    let refused = evaluation_drafts::create(&fx.pool, fx.admin_id, s.session_id, None)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::DraftAlreadyExists));

    // Form resolution: zero daily forms refuses, several demand a name,
    // and a named form must be a daily report of the stamped version.
    let mut bare = evaluated_content();
    bare.name = "Example County Dispatcher Academy".to_owned();
    bare.evaluation_forms.clear();
    let bare_version = fx.publish(&bare).await;
    let quinn_id = fx
        .user_with_role("quinn.trainee", "Quinn Trainee", RoleBundle::Trainee)
        .await;
    let bare_enrollment = enrollments::enroll(&fx.pool, fx.admin_id, bare_version, quinn_id)
        .await
        .expect("call")
        .expect("enrolled");
    let bare_session = training_sessions::create(
        &fx.pool,
        fx.admin_id,
        bare_enrollment,
        &SessionInput {
            business_date: "2026-06-02".to_owned(),
            timezone: "America/Chicago".to_owned(),
            local_start: "2026-06-02T09:00".to_owned(),
            local_end: None,
            disposition: None,
            phase_id: None,
            trainer_user_ids: vec![s.jordan_id],
        },
    )
    .await
    .expect("call")
    .expect("created");
    let refused = evaluation_drafts::create(&fx.pool, fx.admin_id, bare_session, None)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::NoDailyForm));

    // A cancelled session never happened and takes no draft — refused
    // ahead of form resolution.
    training_sessions::close(
        &fx.pool,
        fx.admin_id,
        bare_session,
        training_sessions::Disposition::Cancelled,
        None,
    )
    .await
    .expect("call")
    .expect("cancelled");
    let refused = evaluation_drafts::create(&fx.pool, fx.admin_id, bare_session, None)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::SessionCancelled));

    let mut doubled = evaluated_content();
    doubled.name = "Example County EMD Program".to_owned();
    doubled.evaluation_forms.push(FormDef {
        record_type: RecordType::DailyReport,
        name: "DOR Short Form".to_owned(),
        instructions: "Short form.".to_owned(),
        competencies: vec![FormCompetencyDef {
            competency: "Radio Discipline".to_owned(),
            rating_scale: "Check".to_owned(),
        }],
        narratives: Vec::new(),
    });
    doubled.evaluation_forms.push(FormDef {
        record_type: RecordType::WeeklySummary,
        name: "Weekly Summary".to_owned(),
        instructions: "Weekly.".to_owned(),
        competencies: Vec::new(),
        narratives: vec![NarrativeDef {
            prompt: "Weekly progress.".to_owned(),
            required: false,
        }],
    });
    let doubled_version = fx.publish(&doubled).await;
    let marlow_id = fx
        .user_with_role("marlow.trainee", "Marlow Trainee", RoleBundle::Trainee)
        .await;
    let doubled_enrollment = enrollments::enroll(&fx.pool, fx.admin_id, doubled_version, marlow_id)
        .await
        .expect("call")
        .expect("enrolled");
    let doubled_session = training_sessions::create(
        &fx.pool,
        fx.admin_id,
        doubled_enrollment,
        &SessionInput {
            business_date: "2026-06-02".to_owned(),
            timezone: "America/Chicago".to_owned(),
            local_start: "2026-06-02T11:00".to_owned(),
            local_end: None,
            disposition: None,
            phase_id: None,
            trainer_user_ids: vec![s.jordan_id],
        },
    )
    .await
    .expect("call")
    .expect("created");
    let refused = evaluation_drafts::create(&fx.pool, fx.admin_id, doubled_session, None)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::FormRequired));
    let weekly_form: i64 = sqlx::query_scalar(
        "SELECT id FROM evaluation_form
         WHERE program_version_id = ?1 AND record_type = 'weekly_summary'",
    )
    .bind(doubled_version)
    .fetch_one(&fx.pool)
    .await
    .expect("weekly form");
    let refused =
        evaluation_drafts::create(&fx.pool, fx.admin_id, doubled_session, Some(weekly_form))
            .await
            .expect("call");
    assert_eq!(refused, Err(DraftRefusal::NoSuchForm));
    let short_form: i64 = sqlx::query_scalar(
        "SELECT id FROM evaluation_form
         WHERE program_version_id = ?1 AND name = 'DOR Short Form'",
    )
    .bind(doubled_version)
    .fetch_one(&fx.pool)
    .await
    .expect("short form");
    evaluation_drafts::create(&fx.pool, fx.admin_id, doubled_session, Some(short_form))
        .await
        .expect("call")
        .expect("created with the named form");

    // The coverage join agrees at the database: a record cannot cover a
    // session of another enrollment or version.
    let raw = sqlx::query(
        "INSERT INTO evaluation_session (evaluation_record_id, training_session_id)
         VALUES (?1, ?2)",
    )
    .bind(record_id)
    .bind(bare_session)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(
        err.contains("its own enrollment and version"),
        "join: {err}"
    );
}

#[tokio::test]
async fn attribution_streams_and_coalescing() {
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

    // A working stretch by one contributor is one contributed event, no
    // matter how many autosaves it took.
    let content = DraftContent {
        ratings: vec![rating(eci, Some(4), Vec::new())],
        narratives: vec![narrative(most, "Handled the invented fire call cleanly.")],
    };
    draft_content::save(&fx.pool, s.jordan_id, record_id, &content)
        .await
        .expect("call")
        .expect("saved");
    let again = DraftContent {
        ratings: vec![rating(eci, Some(5), Vec::new())],
        narratives: vec![narrative(most, "Handled the invented fire call very well.")],
    };
    draft_content::save(&fx.pool, s.jordan_id, record_id, &again)
        .await
        .expect("call")
        .expect("saved");
    assert_eq!(
        fx.event_kinds(record_id).await,
        vec![
            ("created".to_owned(), s.jordan_id),
            ("contributed".to_owned(), s.jordan_id),
        ],
        "consecutive saves coalesce"
    );

    // An out-of-scope author cannot contribute; membership admits them,
    // and the interleaved stretch is separately attributed.
    let refused = draft_content::save(&fx.pool, s.rowan_id, record_id, &content)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::CapabilityRequired));
    session_membership::add_trainer(&fx.pool, s.jordan_id, s.session_id, s.rowan_id)
        .await
        .expect("call")
        .expect("added");
    draft_content::save(&fx.pool, s.rowan_id, record_id, &content)
        .await
        .expect("call")
        .expect("saved");
    draft_content::save(&fx.pool, s.jordan_id, record_id, &again)
        .await
        .expect("call")
        .expect("saved");
    let kinds = fx.event_kinds(record_id).await;
    assert_eq!(
        kinds,
        vec![
            ("created".to_owned(), s.jordan_id),
            ("contributed".to_owned(), s.jordan_id),
            ("contributed".to_owned(), s.rowan_id),
            ("contributed".to_owned(), s.jordan_id),
        ],
        "interleaved contributors attribute separately"
    );

    // The stream is append-only at the database.
    let raw = sqlx::query("UPDATE contributor_event SET actor_user_id = ?1")
        .bind(s.rowan_id)
        .execute(&fx.pool)
        .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("append-only"), "update: {err}");
    let raw = sqlx::query("DELETE FROM contributor_event WHERE evaluation_record_id = ?1")
        .bind(record_id)
        .execute(&fx.pool)
        .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("append-only"), "delete: {err}");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn content_validates_against_the_pinned_vocabulary() {
    let fx = Fixture::new().await;
    let s = seed(&fx).await;
    let record_id = evaluation_drafts::create(&fx.pool, s.jordan_id, s.session_id, None)
        .await
        .expect("call")
        .expect("created");
    let eci = fx
        .form_competency_id(s.version_id, "Emergency Call Interrogation")
        .await;
    let radio = fx
        .form_competency_id(s.version_id, "Radio Discipline")
        .await;
    let stress = fx.form_competency_id(s.version_id, "Stress Response").await;
    let most = fx
        .narrative_id(s.version_id, "Most acceptable performance.")
        .await;
    let nrt = fx.modifier_id(s.version_id, "NRT").await;

    let save = |actor: i64, content: DraftContent| {
        let pool = fx.pool.clone();
        async move { draft_content::save(&pool, actor, record_id, &content).await }
    };

    // Another version's vocabulary is refused even by a coordinator.
    let mut foreign = evaluated_content();
    foreign.name = "Example County Dispatcher Academy".to_owned();
    let foreign_version = fx.publish(&foreign).await;
    let foreign_eci = fx
        .form_competency_id(foreign_version, "Emergency Call Interrogation")
        .await;
    let refused = save(
        fx.admin_id,
        DraftContent {
            ratings: vec![rating(foreign_eci, Some(4), Vec::new())],
            narratives: Vec::new(),
        },
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(DraftRefusal::NoSuchFormCompetency));

    // Scale kinds: anchored bounds, pass/fail domain, narrative-only.
    for (competency, value, refusal) in [
        (eci, Some(9), DraftRefusal::ValueOutOfRange),
        (eci, None, DraftRefusal::ValueOutOfRange),
        (radio, Some(2), DraftRefusal::ValueOutOfRange),
        (stress, Some(1), DraftRefusal::ValueNotAllowed),
    ] {
        let refused = save(
            s.jordan_id,
            DraftContent {
                ratings: vec![rating(competency, value, Vec::new())],
                narratives: Vec::new(),
            },
        )
        .await
        .expect("call");
        assert_eq!(refused, Err(refusal), "competency {competency} {value:?}");
    }

    // Unknown modifier and narrative ids, and duplicated entries.
    let refused = save(
        s.jordan_id,
        DraftContent {
            ratings: vec![rating(eci, Some(4), vec![999_999])],
            narratives: Vec::new(),
        },
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(DraftRefusal::NoSuchModifier));
    let refused = save(
        s.jordan_id,
        DraftContent {
            ratings: Vec::new(),
            narratives: vec![narrative(999_999, "text")],
        },
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(DraftRefusal::NoSuchFormNarrative));
    let refused = save(
        s.jordan_id,
        DraftContent {
            ratings: vec![
                rating(eci, Some(4), Vec::new()),
                rating(eci, Some(5), Vec::new()),
            ],
            narratives: Vec::new(),
        },
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(DraftRefusal::DuplicateEntry));
    let refused = save(
        s.jordan_id,
        DraftContent {
            ratings: Vec::new(),
            narratives: vec![narrative(most, "one"), narrative(most, "two")],
        },
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(DraftRefusal::DuplicateEntry));

    // The full valid shape saves: anchored, pass/fail, narrative-only
    // with a modifier, and both narratives.
    let least = fx
        .narrative_id(s.version_id, "Least acceptable performance.")
        .await;
    draft_content::save(
        &fx.pool,
        s.jordan_id,
        record_id,
        &DraftContent {
            ratings: vec![
                rating(eci, Some(7), Vec::new()),
                rating(radio, Some(1), Vec::new()),
                rating(stress, None, vec![nrt]),
            ],
            narratives: vec![
                narrative(most, "Ran the invented surge without missed traffic."),
                narrative(least, "Slow unit identifier on one invented call."),
            ],
        },
    )
    .await
    .expect("call")
    .expect("saved");

    // The composite foreign keys are the backstop under the service.
    let raw = sqlx::query(
        "INSERT INTO draft_rating
             (evaluation_record_id, program_version_id, form_competency_id, value)
         VALUES (?1, ?2, ?3, 4)",
    )
    .bind(record_id)
    .bind(s.version_id)
    .bind(foreign_eci)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("FOREIGN KEY"), "composite fk: {err}");
}

#[tokio::test]
async fn transfer_is_event_mediated() {
    let fx = Fixture::new().await;
    let s = seed(&fx).await;
    let record_id = evaluation_drafts::create(&fx.pool, s.jordan_id, s.session_id, None)
        .await
        .expect("call")
        .expect("created");

    // Recipients hold author_evaluation within the record's scope.
    let refused = evaluation_drafts::transfer(&fx.pool, s.jordan_id, record_id, s.taylor_id)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::NotEligible));
    let refused = evaluation_drafts::transfer(&fx.pool, s.jordan_id, record_id, s.rowan_id)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::NotEligible));
    let refused = evaluation_drafts::transfer(&fx.pool, s.jordan_id, record_id, 999_999)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::NoSuchUser));

    session_membership::add_trainer(&fx.pool, s.jordan_id, s.session_id, s.rowan_id)
        .await
        .expect("call")
        .expect("added");
    evaluation_drafts::transfer(&fx.pool, s.jordan_id, record_id, s.rowan_id)
        .await
        .expect("call")
        .expect("transferred");
    let owner: i64 =
        sqlx::query_scalar("SELECT owner_user_id FROM evaluation_record WHERE id = ?1")
            .bind(record_id)
            .fetch_one(&fx.pool)
            .await
            .expect("owner");
    assert_eq!(owner, s.rowan_id);
    let kinds = fx.event_kinds(record_id).await;
    assert_eq!(
        kinds.last(),
        Some(&("ownership_transferred".to_owned(), s.jordan_id))
    );
    let notified: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notice
         WHERE user_id = ?1 AND kind = 'draft_ownership_received'",
    )
    .bind(s.rowan_id)
    .fetch_one(&fx.pool)
    .await
    .expect("notice");
    assert_eq!(notified, 1, "the recipient is notified");
    let audited: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_event
         WHERE kind = 'draft_ownership_transferred'
           AND subject_kind = 'record' AND subject_id = ?1",
    )
    .bind(record_id)
    .fetch_one(&fx.pool)
    .await
    .expect("audit");
    assert_eq!(audited, 1, "the transfer is audited");

    // The previous owner lost the route; the current owner is not a
    // recipient; a raw owner update without its event is refused.
    let refused = evaluation_drafts::transfer(&fx.pool, s.jordan_id, record_id, s.jordan_id)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::CapabilityRequired));
    let refused = evaluation_drafts::transfer(&fx.pool, fx.admin_id, record_id, s.rowan_id)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::AlreadyOwner));
    let raw = sqlx::query("UPDATE evaluation_record SET owner_user_id = ?1 WHERE id = ?2")
        .bind(s.jordan_id)
        .bind(record_id)
        .execute(&fx.pool)
        .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("recorded event"), "owner mediation: {err}");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn submission_snapshots_and_freezes() {
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
        &DraftContent {
            ratings: vec![rating(eci, Some(4), Vec::new())],
            narratives: vec![narrative(most, "Met the invented standard today.")],
        },
    )
    .await
    .expect("call")
    .expect("saved");

    // Only the owner or a coordinator submits.
    let refused = evaluation_drafts::submit(&fx.pool, s.taylor_id, record_id)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::CapabilityRequired));
    evaluation_drafts::submit(&fx.pool, s.jordan_id, record_id)
        .await
        .expect("call")
        .expect("submitted");

    // The snapshot anchors the review to what was reviewed.
    let (reason, content): (String, String) = sqlx::query_as(
        "SELECT reason, content FROM draft_snapshot WHERE evaluation_record_id = ?1",
    )
    .bind(record_id)
    .fetch_one(&fx.pool)
    .await
    .expect("snapshot");
    assert_eq!(reason, "submission");
    let parsed: serde_json::Value = serde_json::from_str(&content).expect("snapshot json");
    assert_eq!(parsed["ratings"][0]["value"], 4);
    assert_eq!(
        parsed["narratives"][0]["text"],
        "Met the invented standard today."
    );

    // Submitted means frozen: at the service and at the database.
    let refused = evaluation_drafts::submit(&fx.pool, s.jordan_id, record_id)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::DraftSubmitted));
    let refused = draft_content::save(
        &fx.pool,
        s.jordan_id,
        record_id,
        &DraftContent {
            ratings: vec![rating(eci, Some(7), Vec::new())],
            narratives: Vec::new(),
        },
    )
    .await
    .expect("call");
    assert_eq!(refused, Err(DraftRefusal::DraftSubmitted));
    session_membership::add_trainer(&fx.pool, s.jordan_id, s.session_id, s.rowan_id)
        .await
        .expect("call")
        .expect("added");
    let refused = evaluation_drafts::transfer(&fx.pool, s.jordan_id, record_id, s.rowan_id)
        .await
        .expect("call");
    assert_eq!(refused, Err(DraftRefusal::DraftSubmitted));

    let raw = sqlx::query(
        "UPDATE draft_narrative SET text = 'rewritten' WHERE evaluation_record_id = ?1",
    )
    .bind(record_id)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("frozen until review"), "narrative: {err}");
    let radio = fx
        .form_competency_id(s.version_id, "Radio Discipline")
        .await;
    let raw = sqlx::query(
        "INSERT INTO draft_rating
             (evaluation_record_id, program_version_id, form_competency_id, value)
         VALUES (?1, ?2, ?3, 1)",
    )
    .bind(record_id)
    .bind(s.version_id)
    .bind(radio)
    .execute(&fx.pool)
    .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("frozen until review"), "insert: {err}");
    let raw = sqlx::query("DELETE FROM draft_rating WHERE evaluation_record_id = ?1")
        .bind(record_id)
        .execute(&fx.pool)
        .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("frozen until review"), "delete: {err}");
    let raw = sqlx::query("UPDATE draft_snapshot SET content = '{}'")
        .execute(&fx.pool)
        .await;
    let err = raw.expect_err("must be refused").to_string();
    assert!(err.contains("append-only"), "snapshot: {err}");

    let detail = evaluation_drafts::detail(&fx.pool, s.jordan_id, record_id)
        .await
        .expect("call")
        .expect("read");
    assert_eq!(detail.status, DraftStatus::Submitted);
    assert_eq!(detail.snapshots.len(), 1);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn drafts_api_round_trip() {
    let fx = Fixture::new().await;
    let s = seed(&fx).await;
    let jordan = fx.login("jordan.trainer", PASSWORD).await;
    let taylor = fx.login("taylor.trainee", PASSWORD).await;

    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/sessions/{}/draft", s.session_id),
        Some(&jordan),
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create: {body}");
    let draft_id = body["id"].as_i64().expect("id");

    // The session rows now carry the draft.
    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/enrollments/{}/sessions", s.enrollment_id),
        Some(&jordan),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["sessions"][0]["draft_id"], draft_id);
    let (status, body) = request(fx.app(), "GET", "/api/sessions/mine", Some(&jordan), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["sessions"][0]["draft_id"], draft_id);

    // The workspace view: pinned skeleton, empty content, open stream.
    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/drafts/{draft_id}"),
        Some(&jordan),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "view: {body}");
    assert_eq!(body["status"], "draft");
    assert_eq!(body["form"]["form_name"], "Daily Observation Report");
    assert_eq!(
        body["form"]["competencies"].as_array().expect("rows").len(),
        3
    );
    assert_eq!(
        body["form"]["narratives"].as_array().expect("rows").len(),
        2
    );
    assert_eq!(
        body["content"]["ratings"].as_array().expect("rows").len(),
        0
    );
    assert_eq!(body["events"][0]["kind"], "created");
    let eci = body["form"]["competencies"][0]["form_competency_id"]
        .as_i64()
        .expect("fc");
    let nrt = body["form"]["modifiers"][0]["rating_modifier_id"]
        .as_i64()
        .expect("modifier");
    let most = body["form"]["narratives"][0]["form_narrative_id"]
        .as_i64()
        .expect("narrative");

    // The trainee has no route into the draft.
    let (status, _) = request(
        fx.app(),
        "GET",
        &format!("/api/drafts/{draft_id}"),
        Some(&taylor),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Contribute, and read the copy back verbatim.
    let (status, body) = request(
        fx.app(),
        "PUT",
        &format!("/api/drafts/{draft_id}/content"),
        Some(&jordan),
        Some(serde_json::json!({
            "ratings": [
                { "form_competency_id": eci, "value": 5, "modifier_ids": [nrt] }
            ],
            "narratives": [
                { "form_narrative_id": most, "text": "Strong invented shift." }
            ]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "save: {body}");
    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/drafts/{draft_id}"),
        Some(&jordan),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["content"]["ratings"][0]["value"], 5);
    assert_eq!(body["content"]["ratings"][0]["modifier_ids"][0], nrt);
    assert_eq!(
        body["content"]["narratives"][0]["text"],
        "Strong invented shift."
    );
    assert_eq!(body["events"][1]["kind"], "contributed");

    // Hand off, submit, and the frozen copy refuses further writes.
    session_membership::add_trainer(&fx.pool, s.jordan_id, s.session_id, s.rowan_id)
        .await
        .expect("call")
        .expect("added");
    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/drafts/{draft_id}/transfer"),
        Some(&jordan),
        Some(serde_json::json!({ "to_user_id": s.rowan_id })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "transfer: {body}");
    let rowan = fx.login("rowan.trainer", PASSWORD).await;
    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/drafts/{draft_id}/submit"),
        Some(&rowan),
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "submit: {body}");
    let (status, body) = request(
        fx.app(),
        "PUT",
        &format!("/api/drafts/{draft_id}/content"),
        Some(&jordan),
        Some(serde_json::json!({ "ratings": [], "narratives": [] })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "draft_submitted");
    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/drafts/{draft_id}"),
        Some(&jordan),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "submitted");
    assert_eq!(body["snapshots"].as_array().expect("rows").len(), 1);
}
