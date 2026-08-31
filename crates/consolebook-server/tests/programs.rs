//! Milestone 2 slice 1: program-version authoring, database-enforced
//! publish freeze, validation, capability gates, audit, and the
//! export/import round trip. Every fixture is invented.

use consolebook_server::data_dir::DataDir;
use consolebook_server::program_export::{self, ImportRefusal, ImportTarget};
use consolebook_server::programs::{
    self, AnchorDef, AuthorRefusal, CitationDef, CompetencyDef, FormCompetencyDef, FormDef,
    ModifierDef, NarrativeDef, PhaseDef, PolicyDef, ProgramRefusal, PublishRefusal, RecordType, ScaleDef,
    ScaleKind, TaskDef, TransitionDef, TransitionKind, VersionContent,
};
use consolebook_server::{setup, storage, users};

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

    /// A user with no capability grants at all.
    async fn plain_user(&self) -> i64 {
        let mut conn = self.pool.acquire().await.expect("conn");
        users::create(
            &mut conn,
            "jordan.trainer",
            "Jordan Trainer",
            "",
            "",
            "invented-hash",
        )
        .await
        .expect("create user")
    }

    /// Creates a program with a full draft version and returns
    /// (program id, version id).
    async fn program_with_draft(&self) -> (i64, i64) {
        let program_id =
            programs::create_program(&self.pool, self.admin_id, "Example County CTO Program")
                .await
                .expect("create program")
                .expect("accepted");
        let version_id =
            programs::create_version(&self.pool, self.admin_id, program_id, &full_content())
                .await
                .expect("create version")
                .expect("accepted");
        (program_id, version_id)
    }
}

fn citation(body: &str, edition: &str, clause: &str, note: &str) -> CitationDef {
    CitationDef {
        body: body.to_owned(),
        edition: edition.to_owned(),
        clause: clause.to_owned(),
        note: note.to_owned(),
    }
}

fn phase(name: &str, description: &str, number: i64) -> PhaseDef {
    PhaseDef {
        name: name.to_owned(),
        description: description.to_owned(),
        presentation_number: number,
    }
}

fn edge(from: &str, to: &str, kind: TransitionKind) -> TransitionDef {
    TransitionDef {
        from_phase: from.to_owned(),
        to_phase: to.to_owned(),
        kind,
    }
}

fn anchor(value: i64, label: &str, definition: &str) -> AnchorDef {
    AnchorDef {
        value,
        label: label.to_owned(),
        definition: definition.to_owned(),
    }
}

fn binding(competency: &str, rating_scale: &str) -> FormCompetencyDef {
    FormCompetencyDef {
        competency: competency.to_owned(),
        rating_scale: rating_scale.to_owned(),
    }
}

fn narrative(prompt: &str, required: bool) -> NarrativeDef {
    NarrativeDef {
        prompt: prompt.to_owned(),
        required,
    }
}

fn full_competencies() -> Vec<CompetencyDef> {
    vec![
        CompetencyDef {
            category: "Call Processing".to_owned(),
            name: "Emergency Call Interrogation".to_owned(),
            description: "Obtains and verifies location, callback, and nature.".to_owned(),
            tasks: vec![TaskDef {
                prompt: "Processes an invented structure-fire call from answer to dispatch."
                    .to_owned(),
                citations: vec![citation(
                    "Example Accreditation Program",
                    "3rd",
                    "6.1.2",
                    "",
                )],
            }],
            citations: vec![
                citation("Example Accreditation Program", "3rd", "6.1", ""),
                citation(
                    "Example State Training Rule",
                    "",
                    "T-100",
                    "annual requirement",
                ),
            ],
        },
        CompetencyDef {
            category: "Radio Operations".to_owned(),
            name: "Radio Discipline".to_owned(),
            description: "Clear, complete, and professional transmissions.".to_owned(),
            tasks: vec![
                TaskDef {
                    prompt: "Dispatches an invented two-unit response using plain language."
                        .to_owned(),
                    citations: Vec::new(),
                },
                TaskDef {
                    prompt: "Runs an invented status check on an overdue unit.".to_owned(),
                    citations: Vec::new(),
                },
            ],
            citations: Vec::new(),
        },
    ]
}

fn full_scales() -> Vec<ScaleDef> {
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
                anchor(
                    0,
                    "Not Demonstrated",
                    "The task was not performed to standard.",
                ),
                anchor(1, "Demonstrated", "The task was performed to standard."),
            ],
        },
        ScaleDef {
            name: "Seven Point".to_owned(),
            kind: ScaleKind::AnchoredNumeric,
            min_value: Some(1),
            max_value: Some(7),
            anchors: vec![
                anchor(
                    1,
                    "Unacceptable",
                    "Performance well below trainee standard.",
                ),
                anchor(
                    4,
                    "Meets Standards",
                    "Performance at the expected trainee standard.",
                ),
                anchor(7, "Superior", "Performance beyond solo-capable standard."),
            ],
        },
    ]
}

fn full_forms() -> Vec<FormDef> {
    vec![
        FormDef {
            record_type: RecordType::DailyReport,
            name: "Daily Observation Report".to_owned(),
            instructions: "Rate observed performance; narrate the most and least acceptable."
                .to_owned(),
            competencies: vec![
                binding("Emergency Call Interrogation", "Seven Point"),
                binding("Radio Discipline", "Pass Fail Check"),
            ],
            narratives: vec![
                narrative("Most acceptable performance", true),
                narrative("Least acceptable performance", true),
            ],
        },
        FormDef {
            record_type: RecordType::WeeklySummary,
            name: "Weekly Performance Summary".to_owned(),
            instructions: String::new(),
            competencies: vec![binding(
                "Emergency Call Interrogation",
                "Narrative Assessment",
            )],
            narratives: vec![narrative("Overall progress this week", true)],
        },
    ]
}

/// A complete invented program. Collections are listed in the
/// deterministic order `load_content` guarantees, so equality asserts
/// compare directly against this value.
fn full_content() -> VersionContent {
    VersionContent {
        name: "Example County CTO Program".to_owned(),
        label: "2026 rev A".to_owned(),
        description: "Invented communications training officer program.".to_owned(),
        phases: vec![
            phase("Phase One", "Observation.", 1),
            phase("Phase Two", "Guided practice.", 2),
            phase("Phase Three", "Independent performance.", 3),
        ],
        phase_transitions: vec![
            edge("Phase One", "Phase Three", TransitionKind::Skip),
            edge("Phase One", "Phase Two", TransitionKind::Advance),
            edge("Phase Three", "Phase One", TransitionKind::Restart),
            edge("Phase Three", "Phase Two", TransitionKind::Remediation),
            edge("Phase Two", "Phase Three", TransitionKind::Advance),
        ],
        competencies: full_competencies(),
        rating_scales: full_scales(),
        rating_modifiers: vec![ModifierDef {
            code: "NRT".to_owned(),
            label: "Not Responding to Training".to_owned(),
            description: "Documented remediation has not produced improvement.".to_owned(),
        }],
        evaluation_forms: full_forms(),
        citations: vec![citation(
            "Example Accreditation Program",
            "3rd",
            "6.1",
            "program-level mapping",
        )],
        finalization_policy: PolicyDef::default(),
    }
}

/// An invented annual in-service shape: no phases at all.
fn in_service_content() -> VersionContent {
    VersionContent {
        name: "Example County Annual In-Service".to_owned(),
        label: "2026".to_owned(),
        description: "Invented yearly continuing training.".to_owned(),
        phases: Vec::new(),
        phase_transitions: Vec::new(),
        competencies: vec![CompetencyDef {
            category: String::new(),
            name: "Policy Refresher".to_owned(),
            description: "Annual review of invented operational policies.".to_owned(),
            tasks: Vec::new(),
            citations: vec![citation("Example State Training Rule", "", "T-200", "")],
        }],
        rating_scales: vec![ScaleDef {
            name: "Completion".to_owned(),
            kind: ScaleKind::PassFail,
            min_value: None,
            max_value: None,
            anchors: vec![
                AnchorDef {
                    value: 0,
                    label: "Incomplete".to_owned(),
                    definition: String::new(),
                },
                AnchorDef {
                    value: 1,
                    label: "Complete".to_owned(),
                    definition: String::new(),
                },
            ],
        }],
        rating_modifiers: Vec::new(),
        evaluation_forms: vec![FormDef {
            record_type: RecordType::DailyReport,
            name: "In-Service Completion Record".to_owned(),
            instructions: String::new(),
            competencies: vec![FormCompetencyDef {
                competency: "Policy Refresher".to_owned(),
                rating_scale: "Completion".to_owned(),
            }],
            narratives: Vec::new(),
        }],
        citations: Vec::new(),
        finalization_policy: PolicyDef::default(),
    }
}

const OWNED_TABLES: [&str; 11] = [
    "phase",
    "phase_transition",
    "competency",
    "task",
    "rating_scale",
    "rating_anchor",
    "rating_modifier",
    "evaluation_form",
    "form_competency",
    "form_narrative",
    "standards_citation",
];

const FROZEN_UPDATES: [&str; 11] = [
    "UPDATE phase SET description = 'edited' WHERE program_version_id = ?1",
    "UPDATE phase_transition SET kind = 'advance' WHERE program_version_id = ?1",
    "UPDATE competency SET description = 'edited' WHERE program_version_id = ?1",
    "UPDATE task SET sort_order = sort_order + 1 WHERE program_version_id = ?1",
    "UPDATE rating_scale SET name = name WHERE program_version_id = ?1",
    "UPDATE rating_anchor SET label = 'edited' WHERE program_version_id = ?1",
    "UPDATE rating_modifier SET label = 'edited' WHERE program_version_id = ?1",
    "UPDATE evaluation_form SET instructions = 'edited' WHERE program_version_id = ?1",
    "UPDATE form_competency SET sort_order = sort_order + 1 WHERE program_version_id = ?1",
    "UPDATE form_narrative SET required = 0 WHERE program_version_id = ?1",
    "UPDATE standards_citation SET note = 'edited' WHERE program_version_id = ?1",
];

async fn expect_frozen(pool: &sqlx::SqlitePool, sql: &str, binds: &[i64]) {
    let mut query = sqlx::query(sql);
    for bind in binds {
        query = query.bind(*bind);
    }
    let result = query.execute(pool).await;
    let err = result.expect_err(&format!("must be rejected on a published version: {sql}"));
    let message = err.to_string();
    assert!(
        message.contains("immutable"),
        "rejection must come from the freeze trigger, got: {message}"
    );
}

async fn scalar(pool: &sqlx::SqlitePool, sql: &str, bind: i64) -> i64 {
    sqlx::query_scalar(sql)
        .bind(bind)
        .fetch_one(pool)
        .await
        .expect("scalar query")
}

/// INSERT statements that would be valid rows of the published version —
/// text values inline, integer binds listed — all of which the freeze
/// triggers must reject.
async fn frozen_inserts(pool: &sqlx::SqlitePool, version_id: i64) -> Vec<(String, Vec<i64>)> {
    let phase_id = scalar(
        pool,
        "SELECT id FROM phase WHERE program_version_id = ?1 LIMIT 1",
        version_id,
    )
    .await;
    let competency_id = scalar(
        pool,
        "SELECT id FROM competency WHERE program_version_id = ?1 LIMIT 1",
        version_id,
    )
    .await;
    let scale_id = scalar(
        pool,
        "SELECT id FROM rating_scale WHERE program_version_id = ?1 LIMIT 1",
        version_id,
    )
    .await;
    let form_id = scalar(
        pool,
        "SELECT id FROM evaluation_form WHERE program_version_id = ?1 LIMIT 1",
        version_id,
    )
    .await;
    let statements: [(&str, Vec<i64>); 11] = [
        (
            "INSERT INTO phase (program_version_id, name, description, presentation_number)
             VALUES (?1, 'Sneaky Phase', '', 9)",
            vec![version_id],
        ),
        (
            "INSERT INTO phase_transition (program_version_id, from_phase_id, to_phase_id, kind)
             VALUES (?1, ?2, ?2, 'advance')",
            vec![version_id, phase_id],
        ),
        (
            "INSERT INTO competency (program_version_id, category, name, description, sort_order)
             VALUES (?1, '', 'Sneaky Competency', '', 99)",
            vec![version_id],
        ),
        (
            "INSERT INTO task (program_version_id, competency_id, prompt, sort_order)
             VALUES (?1, ?2, 'Sneaky prompt.', 99)",
            vec![version_id, competency_id],
        ),
        (
            "INSERT INTO rating_scale (program_version_id, name, kind, min_value, max_value)
             VALUES (?1, 'Sneaky Scale', 'narrative_only', NULL, NULL)",
            vec![version_id],
        ),
        (
            "INSERT INTO rating_anchor (program_version_id, rating_scale_id, value, label, definition)
             VALUES (?1, ?2, 99, 'Sneaky', '')",
            vec![version_id, scale_id],
        ),
        (
            "INSERT INTO rating_modifier (program_version_id, code, label, description)
             VALUES (?1, 'ZZ', 'Sneaky', '')",
            vec![version_id],
        ),
        (
            "INSERT INTO evaluation_form (program_version_id, record_type, name, instructions)
             VALUES (?1, 'daily_report', 'Sneaky Form', '')",
            vec![version_id],
        ),
        (
            "INSERT INTO form_competency
                 (program_version_id, evaluation_form_id, competency_id, rating_scale_id, sort_order)
             VALUES (?1, ?2, ?3, ?4, 99)",
            vec![version_id, form_id, competency_id, scale_id],
        ),
        (
            "INSERT INTO form_narrative (program_version_id, evaluation_form_id, prompt, required, sort_order)
             VALUES (?1, ?2, 'Sneaky narrative', 1, 99)",
            vec![version_id, form_id],
        ),
        (
            "INSERT INTO standards_citation
                 (program_version_id, competency_id, task_id, body, edition, clause, note)
             VALUES (?1, NULL, NULL, 'Sneaky Body', '', '1.1', '')",
            vec![version_id],
        ),
    ];
    statements
        .into_iter()
        .map(|(sql, binds)| (sql.to_owned(), binds))
        .collect()
}

#[tokio::test]
async fn draft_content_loads_back_exactly_as_authored() {
    let fx = Fixture::new().await;
    let (_, version_id) = fx.program_with_draft().await;
    let loaded = programs::load_content(&fx.pool, version_id)
        .await
        .expect("load")
        .expect("exists");
    assert_eq!(
        loaded,
        full_content(),
        "load must reproduce authored content"
    );
}

#[tokio::test]
async fn published_versions_are_immutable_at_the_database() {
    let fx = Fixture::new().await;
    let (_, version_id) = fx.program_with_draft().await;
    programs::publish_version(&fx.pool, fx.admin_id, version_id)
        .await
        .expect("publish")
        .expect("accepted");

    // The version row itself.
    expect_frozen(
        &fx.pool,
        "UPDATE program_version SET label = 'edited' WHERE id = ?1",
        &[version_id],
    )
    .await;
    expect_frozen(
        &fx.pool,
        "DELETE FROM program_version WHERE id = ?1",
        &[version_id],
    )
    .await;

    // Every owned row rejects UPDATE and DELETE.
    for sql in FROZEN_UPDATES {
        expect_frozen(&fx.pool, sql, &[version_id]).await;
    }
    for table in OWNED_TABLES {
        let sql = format!("DELETE FROM {table} WHERE program_version_id = ?1");
        expect_frozen(&fx.pool, &sql, &[version_id]).await;
    }

    // Nothing new can be inserted into a published version, even rows
    // that would satisfy every foreign key.
    for (sql, binds) in frozen_inserts(&fx.pool, version_id).await {
        expect_frozen(&fx.pool, &sql, &binds).await;
    }

    // The content is exactly what was published.
    let loaded = programs::load_content(&fx.pool, version_id)
        .await
        .expect("load")
        .expect("exists");
    assert_eq!(
        loaded,
        full_content(),
        "published content must be untouched"
    );

    // Service-level mutation is refused too.
    let refused = programs::replace_draft(&fx.pool, fx.admin_id, version_id, &full_content())
        .await
        .expect("call");
    assert_eq!(refused, Err(AuthorRefusal::AlreadyPublished));
    let refused = programs::discard_draft(&fx.pool, fx.admin_id, version_id)
        .await
        .expect("call");
    assert_eq!(refused, Err(AuthorRefusal::AlreadyPublished));
}

#[tokio::test]
async fn a_version_with_no_phases_publishes() {
    let fx = Fixture::new().await;
    let program_id =
        programs::create_program(&fx.pool, fx.admin_id, "Example County Annual In-Service")
            .await
            .expect("create program")
            .expect("accepted");
    let version_id =
        programs::create_version(&fx.pool, fx.admin_id, program_id, &in_service_content())
            .await
            .expect("create version")
            .expect("accepted");
    programs::publish_version(&fx.pool, fx.admin_id, version_id)
        .await
        .expect("publish")
        .expect("phase-less versions are a valid shape");
    let loaded = programs::load_content(&fx.pool, version_id)
        .await
        .expect("load")
        .expect("exists");
    assert_eq!(loaded, in_service_content());
}

#[tokio::test]
async fn structural_validation_rejects_broken_content() {
    let fx = Fixture::new().await;
    let (_, version_id) = fx.program_with_draft().await;

    let mut unknown_phase = full_content();
    unknown_phase.phase_transitions.push(TransitionDef {
        from_phase: "Phase Nine".to_owned(),
        to_phase: "Phase One".to_owned(),
        kind: TransitionKind::Advance,
    });
    let mut unknown_scale = full_content();
    unknown_scale.evaluation_forms[0].competencies[0].rating_scale = "Missing Scale".to_owned();
    let mut duplicate_competency = full_content();
    duplicate_competency.competencies[1].name = "emergency call interrogation".to_owned();
    let mut bad_pass_fail = full_content();
    bad_pass_fail.rating_scales[1].anchors.push(AnchorDef {
        value: 2,
        label: "Extra".to_owned(),
        definition: String::new(),
    });
    let mut unbounded_numeric = full_content();
    unbounded_numeric.rating_scales[2].min_value = None;
    let mut anchor_out_of_range = full_content();
    anchor_out_of_range.rating_scales[2].anchors[2].value = 9;

    let cases: Vec<(VersionContent, &str)> = vec![
        (unknown_phase, "unknown phase 'Phase Nine'"),
        (unknown_scale, "unknown rating scale 'Missing Scale'"),
        (duplicate_competency, "duplicate competency name"),
        (bad_pass_fail, "exactly two anchors"),
        (unbounded_numeric, "requires min_value and max_value"),
        (anchor_out_of_range, "outside 1..=7"),
    ];
    for (content, expected) in &cases {
        let outcome = programs::replace_draft(&fx.pool, fx.admin_id, version_id, content)
            .await
            .expect("call");
        let Err(AuthorRefusal::Invalid(problems)) = outcome else {
            panic!("content must be refused as invalid (expected '{expected}')");
        };
        assert!(
            problems.iter().any(|p| p.contains(expected)),
            "problems must name the defect '{expected}', got: {problems:?}"
        );
    }
}

#[tokio::test]
async fn publish_requires_forms_with_content_and_happens_once() {
    let fx = Fixture::new().await;
    let (_, version_id) = fx.program_with_draft().await;

    let mut empty_form = full_content();
    empty_form.evaluation_forms[1].competencies.clear();
    empty_form.evaluation_forms[1].narratives.clear();
    programs::replace_draft(&fx.pool, fx.admin_id, version_id, &empty_form)
        .await
        .expect("call")
        .expect("structurally valid");
    let outcome = programs::publish_version(&fx.pool, fx.admin_id, version_id)
        .await
        .expect("call");
    let Err(PublishRefusal::Incomplete(problems)) = outcome else {
        panic!("publish must refuse a form with no competencies and no narratives");
    };
    assert!(
        problems
            .iter()
            .any(|p| p.contains("Weekly Performance Summary")),
        "refusal must name the empty form, got: {problems:?}"
    );

    programs::replace_draft(&fx.pool, fx.admin_id, version_id, &full_content())
        .await
        .expect("call")
        .expect("accepted");
    programs::publish_version(&fx.pool, fx.admin_id, version_id)
        .await
        .expect("call")
        .expect("accepted");
    let again = programs::publish_version(&fx.pool, fx.admin_id, version_id)
        .await
        .expect("call");
    assert_eq!(again, Err(PublishRefusal::AlreadyPublished));
}

#[tokio::test]
async fn drafts_replace_wholesale_with_last_write_winning() {
    let fx = Fixture::new().await;
    let (_, version_id) = fx.program_with_draft().await;
    programs::replace_draft(&fx.pool, fx.admin_id, version_id, &in_service_content())
        .await
        .expect("call")
        .expect("accepted");
    let loaded = programs::load_content(&fx.pool, version_id)
        .await
        .expect("load")
        .expect("exists");
    assert_eq!(loaded, in_service_content(), "last write must fully win");
    let leftover_modifiers = scalar(
        &fx.pool,
        "SELECT COUNT(*) FROM rating_modifier WHERE program_version_id = ?1",
        version_id,
    )
    .await;
    assert_eq!(
        leftover_modifiers, 0,
        "replaced content must leave no orphan rows"
    );
}

#[tokio::test]
async fn discarding_a_draft_removes_it_entirely() {
    let fx = Fixture::new().await;
    let (program_id, version_id) = fx.program_with_draft().await;
    programs::discard_draft(&fx.pool, fx.admin_id, version_id)
        .await
        .expect("call")
        .expect("accepted");
    assert!(
        programs::load_content(&fx.pool, version_id)
            .await
            .expect("load")
            .is_none(),
        "discarded draft must be gone"
    );
    for table in OWNED_TABLES {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE program_version_id = ?1");
        assert_eq!(
            scalar(&fx.pool, &sql, version_id).await,
            0,
            "no rows may remain in {table}"
        );
    }
    assert!(
        programs::list_versions(&fx.pool, program_id)
            .await
            .expect("list")
            .is_empty(),
        "version list must be empty after discard"
    );
}

#[tokio::test]
async fn version_numbers_are_monotonic_per_program() {
    let fx = Fixture::new().await;
    let (program_id, _) = fx.program_with_draft().await;
    programs::create_version(&fx.pool, fx.admin_id, program_id, &full_content())
        .await
        .expect("call")
        .expect("accepted");
    let versions = programs::list_versions(&fx.pool, program_id)
        .await
        .expect("list");
    let numbers: Vec<i64> = versions.iter().map(|v| v.version_number).collect();
    assert_eq!(numbers, vec![1, 2], "version numbers must count up from 1");
    assert_eq!(versions[0].label, "2026 rev A");
    assert_eq!(versions[0].name, "Example County CTO Program");
}

#[tokio::test]
async fn authoring_requires_the_manage_programs_capability() {
    let fx = Fixture::new().await;
    let (program_id, version_id) = fx.program_with_draft().await;
    let plain = fx.plain_user().await;

    let refused = programs::create_program(&fx.pool, plain, "Unauthorized Program")
        .await
        .expect("call");
    assert_eq!(refused, Err(ProgramRefusal::CapabilityRequired));
    let refused = programs::create_version(&fx.pool, plain, program_id, &full_content())
        .await
        .expect("call");
    assert_eq!(refused, Err(AuthorRefusal::CapabilityRequired));
    let refused = programs::publish_version(&fx.pool, plain, version_id)
        .await
        .expect("call");
    assert_eq!(refused, Err(PublishRefusal::CapabilityRequired));
    let refused = program_export::import_version(&fx.pool, plain, "{}", ImportTarget::NewProgram)
        .await
        .expect("call");
    assert_eq!(refused, Err(ImportRefusal::CapabilityRequired));
}

#[tokio::test]
async fn lifecycle_actions_append_attributable_audit_events() {
    let fx = Fixture::new().await;
    let (program_id, version_id) = fx.program_with_draft().await;
    programs::publish_version(&fx.pool, fx.admin_id, version_id)
        .await
        .expect("publish")
        .expect("accepted");

    let expectations = [
        ("program_created", "program", program_id),
        ("program_version_created", "program_version", version_id),
        ("program_version_published", "program_version", version_id),
    ];
    for (kind, subject_kind, subject_id) in expectations {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_event
             WHERE kind = ?1 AND subject_kind = ?2 AND subject_id = ?3 AND actor_user_id = ?4",
        )
        .bind(kind)
        .bind(subject_kind)
        .bind(subject_id)
        .bind(fx.admin_id)
        .fetch_one(&fx.pool)
        .await
        .expect("count");
        assert_eq!(
            count, 1,
            "exactly one attributable '{kind}' event must exist"
        );
    }
}

#[tokio::test]
async fn export_import_round_trip_reproduces_identical_bytes() {
    let source = Fixture::new().await;
    let (_, version_id) = source.program_with_draft().await;
    programs::publish_version(&source.pool, source.admin_id, version_id)
        .await
        .expect("publish")
        .expect("accepted");
    let exported = program_export::export_version(&source.pool, version_id)
        .await
        .expect("export")
        .expect("exists");

    // A second, empty installation reproduces the version from bytes.
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
    assert_eq!(exported, re_exported, "round trip must be byte-identical");

    let loaded = programs::load_content(&target.pool, imported_id)
        .await
        .expect("load")
        .expect("exists");
    assert_eq!(
        loaded,
        full_content(),
        "imported content must equal the source"
    );
    programs::publish_version(&target.pool, target.admin_id, imported_id)
        .await
        .expect("publish")
        .expect("an imported draft publishes through the normal path");
}

#[tokio::test]
async fn import_can_add_the_next_version_of_an_existing_program() {
    let fx = Fixture::new().await;
    let (program_id, version_id) = fx.program_with_draft().await;
    let exported = program_export::export_version(&fx.pool, version_id)
        .await
        .expect("export")
        .expect("exists");
    let imported_id = program_export::import_version(
        &fx.pool,
        fx.admin_id,
        &exported,
        ImportTarget::VersionOf(program_id),
    )
    .await
    .expect("import")
    .expect("accepted");
    let versions = programs::list_versions(&fx.pool, program_id)
        .await
        .expect("list");
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[1].id, imported_id);
    assert_eq!(
        versions[1].version_number, 2,
        "import must take the next number"
    );
}

#[tokio::test]
async fn import_refuses_wrong_envelopes_and_taken_names() {
    let fx = Fixture::new().await;
    let (_, version_id) = fx.program_with_draft().await;
    let exported = program_export::export_version(&fx.pool, version_id)
        .await
        .expect("export")
        .expect("exists");

    // The source program still exists, so the same name is taken.
    let refused =
        program_export::import_version(&fx.pool, fx.admin_id, &exported, ImportTarget::NewProgram)
            .await
            .expect("call");
    assert_eq!(refused, Err(ImportRefusal::ProgramNameTaken));

    let wrong_family = exported.replace(
        "\"format\":\"consolebook-program-version\"",
        "\"format\":\"invented-other-format\"",
    );
    let outcome = program_export::import_version(
        &fx.pool,
        fx.admin_id,
        &wrong_family,
        ImportTarget::NewProgram,
    )
    .await
    .expect("call");
    assert!(
        matches!(outcome, Err(ImportRefusal::UnsupportedFormat(_))),
        "a different format family must be refused"
    );

    let wrong_version = exported.replace("\"format_version\":1", "\"format_version\":99");
    let outcome = program_export::import_version(
        &fx.pool,
        fx.admin_id,
        &wrong_version,
        ImportTarget::NewProgram,
    )
    .await
    .expect("call");
    assert!(
        matches!(outcome, Err(ImportRefusal::UnsupportedFormat(_))),
        "an unknown format version must be refused"
    );

    let outcome =
        program_export::import_version(&fx.pool, fx.admin_id, "not json", ImportTarget::NewProgram)
            .await
            .expect("call");
    assert!(
        matches!(outcome, Err(ImportRefusal::UnsupportedFormat(_))),
        "malformed JSON must be refused"
    );

    let outcome = program_export::import_version(
        &fx.pool,
        fx.admin_id,
        &exported,
        ImportTarget::VersionOf(9999),
    )
    .await
    .expect("call");
    assert_eq!(outcome, Err(ImportRefusal::NoSuchProgram));
}

#[tokio::test]
async fn export_of_a_missing_version_is_none() {
    let fx = Fixture::new().await;
    let exported = program_export::export_version(&fx.pool, 4242)
        .await
        .expect("call");
    assert!(exported.is_none());
}
