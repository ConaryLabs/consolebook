//! Program-configuration HTTP API: authentication and capability gates,
//! the full authoring flow, stable refusal codes, and export/import over
//! the wire. Every fixture is invented.

use axum::body::Body;
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE, COOKIE, SET_COOKIE};
use axum::http::{Request, StatusCode};
use consolebook_server::data_dir::DataDir;
use consolebook_server::{http, secrets, setup, storage, users};
use http_body_util::BodyExt;
use tower::ServiceExt;

const PASSWORD: &str = "invented-passphrase-1";

struct Fixture {
    _tmp: tempfile::TempDir,
    pool: sqlx::SqlitePool,
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
        setup::initialize(
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
        Self { _tmp: tmp, pool }
    }

    fn app(&self) -> axum::Router {
        http::router(http::AppState {
            pool: self.pool.clone(),
        })
    }

    /// Creates a user with no capability grants who can sign in.
    async fn create_plain_user(&self) {
        let hash = secrets::hash_password(PASSWORD).expect("hash");
        let mut conn = self.pool.acquire().await.expect("conn");
        users::create(&mut conn, "jordan.trainer", "Jordan Trainer", &hash)
            .await
            .expect("create user");
    }

    async fn login(&self, username: &str) -> String {
        let response = self
            .app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "username": username, "password": PASSWORD })
                            .to_string(),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response
            .headers()
            .get(SET_COOKIE)
            .expect("cookie")
            .to_str()
            .expect("ascii");
        let (pair, _) = cookie.split_once(';').expect("attrs");
        pair.split_once('=').expect("pair").1.to_string()
    }
}

async fn request(
    app: axum::Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let (status, bytes) = raw_request(app, method, uri, cookie, body).await;
    let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, body)
}

async fn raw_request(
    app: axum::Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Option<serde_json::Value>,
) -> (StatusCode, Vec<u8>) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(cookie) = cookie {
        builder = builder.header(COOKIE, format!("{}={}", http::SESSION_COOKIE, cookie));
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
        .to_bytes()
        .to_vec();
    (status, bytes)
}

/// A minimal complete invented content document, as the wire JSON the
/// interface would send.
fn content_json(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "label": "2026",
        "description": "Invented content for API tests.",
        "phases": [
            {"name": "Phase One", "description": "Observation.", "presentation_number": 1}
        ],
        "phase_transitions": [],
        "competencies": [{
            "category": "Call Processing",
            "name": "Emergency Call Interrogation",
            "description": "Obtains and verifies location, callback, and nature.",
            "tasks": [{"prompt": "Processes an invented alarm call.", "citations": []}],
            "citations": [
                {"body": "Example Accreditation Program", "edition": "3rd", "clause": "6.1", "note": ""}
            ]
        }],
        "rating_scales": [{
            "name": "Seven Point",
            "kind": "anchored_numeric",
            "min_value": 1,
            "max_value": 7,
            "anchors": [
                {"value": 1, "label": "Unacceptable", "definition": ""},
                {"value": 7, "label": "Superior", "definition": ""}
            ]
        }],
        "rating_modifiers": [],
        "evaluation_forms": [{
            "record_type": "daily_report",
            "name": "Daily Observation Report",
            "instructions": "",
            "competencies": [
                {"competency": "Emergency Call Interrogation", "rating_scale": "Seven Point"}
            ],
            "narratives": [{"prompt": "Most acceptable performance", "required": true}]
        }],
        "citations": []
    })
}

#[tokio::test]
async fn program_endpoints_require_a_session() {
    let fx = Fixture::new().await;
    let (status, body) = request(fx.app(), "GET", "/api/programs", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "unauthenticated");
    let (status, _) = request(
        fx.app(),
        "POST",
        "/api/programs",
        None,
        Some(serde_json::json!({"name": "Unauthenticated Program"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// Creates a program and one draft version over the API, asserting both
/// succeed; returns (program id, version id).
async fn seed_program(fx: &Fixture, cookie: &str, name: &str) -> (i64, i64) {
    let (status, body) = request(
        fx.app(),
        "POST",
        "/api/programs",
        Some(cookie),
        Some(serde_json::json!({ "name": name })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create program: {body}");
    let program_id = body["id"].as_i64().expect("program id");
    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/programs/{program_id}/versions"),
        Some(cookie),
        Some(content_json(name)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create version: {body}");
    (program_id, body["id"].as_i64().expect("version id"))
}

#[tokio::test]
async fn the_full_authoring_flow_works_over_http() {
    let fx = Fixture::new().await;
    let cookie = fx.login("avery.admin").await;
    let (program_id, version_id) = seed_program(&fx, &cookie, "Example County CTO Program").await;

    let (status, body) = request(fx.app(), "GET", "/api/programs", Some(&cookie), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["programs"][0]["name"], "Example County CTO Program");

    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/programs/{program_id}/versions"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["program"]["id"], program_id);
    assert_eq!(body["versions"][0]["version_number"], 1);
    assert!(body["versions"][0]["published_at"].is_null());

    let mut relabeled = content_json("Example County CTO Program");
    relabeled["label"] = serde_json::json!("2026 rev B");
    let (status, body) = request(
        fx.app(),
        "PUT",
        &format!("/api/program-versions/{version_id}/content"),
        Some(&cookie),
        Some(relabeled),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "replace content: {body}");

    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/program-versions/{version_id}"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["summary"]["label"], "2026 rev B");
    assert_eq!(
        body["content"]["competencies"][0]["name"],
        "Emergency Call Interrogation"
    );

    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/program-versions/{version_id}/publish"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "publish: {body}");

    let (status, body) = request(
        fx.app(),
        "PUT",
        &format!("/api/program-versions/{version_id}/content"),
        Some(&cookie),
        Some(content_json("Example County CTO Program")),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "already_published");

    let (status, body) = request(
        fx.app(),
        "DELETE",
        &format!("/api/program-versions/{version_id}"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "already_published");
}

#[tokio::test]
async fn refusals_map_to_stable_error_codes() {
    let fx = Fixture::new().await;
    let cookie = fx.login("avery.admin").await;

    let (status, body) = request(
        fx.app(),
        "POST",
        "/api/programs",
        Some(&cookie),
        Some(serde_json::json!({"name": "   "})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "name_empty");

    let (_, body) = request(
        fx.app(),
        "POST",
        "/api/programs",
        Some(&cookie),
        Some(serde_json::json!({"name": "Example County CTO Program"})),
    )
    .await;
    let program_id = body["id"].as_i64().expect("program id");
    let (status, body) = request(
        fx.app(),
        "POST",
        "/api/programs",
        Some(&cookie),
        Some(serde_json::json!({"name": "example county cto program"})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "names are case-insensitively unique"
    );
    assert_eq!(body["error"], "name_taken");

    let mut broken = content_json("Example County CTO Program");
    broken["evaluation_forms"][0]["competencies"][0]["rating_scale"] =
        serde_json::json!("Missing Scale");
    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/programs/{program_id}/versions"),
        Some(&cookie),
        Some(broken),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "invalid_content");
    let problems = body["problems"].as_array().expect("problems array");
    assert!(
        problems
            .iter()
            .any(|p| p.as_str().expect("string").contains("Missing Scale")),
        "problems must name the defect: {problems:?}"
    );

    let (status, body) = request(
        fx.app(),
        "POST",
        "/api/programs/424242/versions",
        Some(&cookie),
        Some(content_json("Orphan Program")),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "no_such_program");

    let (status, body) = request(
        fx.app(),
        "GET",
        "/api/program-versions/424242",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "no_such_version");

    let mut empty_form = content_json("Example County CTO Program");
    empty_form["evaluation_forms"][0]["competencies"] = serde_json::json!([]);
    empty_form["evaluation_forms"][0]["narratives"] = serde_json::json!([]);
    let (_, body) = request(
        fx.app(),
        "POST",
        &format!("/api/programs/{program_id}/versions"),
        Some(&cookie),
        Some(empty_form),
    )
    .await;
    let version_id = body["id"].as_i64().expect("version id");
    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/program-versions/{version_id}/publish"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "incomplete");
    assert!(body["problems"].as_array().is_some_and(|p| !p.is_empty()));
}

#[tokio::test]
async fn mutations_require_manage_programs_but_reads_do_not() {
    let fx = Fixture::new().await;
    let admin = fx.login("avery.admin").await;
    let (program_id, version_id) = seed_program(&fx, &admin, "Example County CTO Program").await;

    fx.create_plain_user().await;
    let plain = fx.login("jordan.trainer").await;

    let (status, _) = request(fx.app(), "GET", "/api/programs", Some(&plain), None).await;
    assert_eq!(status, StatusCode::OK, "signed-in reads are allowed");
    let (status, _) = request(
        fx.app(),
        "GET",
        &format!("/api/program-versions/{version_id}"),
        Some(&plain),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let mutations: Vec<(&str, String, Option<serde_json::Value>)> = vec![
        (
            "POST",
            "/api/programs".to_owned(),
            Some(serde_json::json!({"name": "Unauthorized Program"})),
        ),
        (
            "POST",
            format!("/api/programs/{program_id}/versions"),
            Some(content_json("Example County CTO Program")),
        ),
        (
            "PUT",
            format!("/api/program-versions/{version_id}/content"),
            Some(content_json("Example County CTO Program")),
        ),
        (
            "POST",
            format!("/api/program-versions/{version_id}/publish"),
            None,
        ),
        (
            "DELETE",
            format!("/api/program-versions/{version_id}"),
            None,
        ),
        (
            "POST",
            "/api/programs/import".to_owned(),
            Some(serde_json::json!({"document": "{}"})),
        ),
    ];
    for (method, uri, body) in mutations {
        let (status, response) = request(fx.app(), method, &uri, Some(&plain), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {response}");
        assert_eq!(response["error"], "capability_required", "{method} {uri}");
    }
}

#[tokio::test]
async fn export_and_import_work_over_the_wire() {
    let fx = Fixture::new().await;
    let cookie = fx.login("avery.admin").await;
    let (program_id, version_id) = seed_program(&fx, &cookie, "Alpha Program").await;

    // Export delivers the documented bytes as a download.
    let response = fx
        .app()
        .oneshot(
            Request::builder()
                .uri(format!("/api/program-versions/{version_id}/export"))
                .header(COOKIE, format!("{}={}", http::SESSION_COOKIE, cookie))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let disposition = response
        .headers()
        .get(CONTENT_DISPOSITION)
        .expect("disposition")
        .to_str()
        .expect("ascii")
        .to_owned();
    assert!(disposition.starts_with("attachment"), "got: {disposition}");
    let document = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes()
            .to_vec(),
    )
    .expect("utf-8");
    assert!(document.contains("\"format\":\"consolebook-program-version\""));

    // The same document imports as the next version of its program.
    let (status, body) = request(
        fx.app(),
        "POST",
        &format!("/api/programs/{program_id}/versions/import"),
        Some(&cookie),
        Some(serde_json::json!({"document": document})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "import next version: {body}");
    assert_eq!(body["program_id"], program_id);
    let (_, body) = request(
        fx.app(),
        "GET",
        &format!("/api/programs/{program_id}/versions"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(body["versions"].as_array().expect("versions").len(), 2);
    assert_eq!(body["versions"][1]["version_number"], 2);

    // Renamed, it imports as a new program; same-named it is refused.
    let renamed = document.replace("\"name\":\"Alpha Program\"", "\"name\":\"Beta Program\"");
    let (status, body) = request(
        fx.app(),
        "POST",
        "/api/programs/import",
        Some(&cookie),
        Some(serde_json::json!({"document": renamed})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "import new program: {body}");
    assert_ne!(body["program_id"].as_i64().expect("program id"), program_id);

    let (status, body) = request(
        fx.app(),
        "POST",
        "/api/programs/import",
        Some(&cookie),
        Some(serde_json::json!({"document": document})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "name_taken");

    let (status, body) = request(
        fx.app(),
        "POST",
        "/api/programs/import",
        Some(&cookie),
        Some(serde_json::json!({"document": "not json"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "unsupported_format");
}
