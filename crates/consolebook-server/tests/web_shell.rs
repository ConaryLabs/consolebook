//! Integration tests for the embedded web shell and the instance endpoint.
//!
//! Asset assertions adapt to whether `web/` was built before the test run:
//! the SPA behaviors are asserted strictly when assets are embedded, and
//! the honest degraded notice is asserted otherwise. CI builds `web/`
//! first, so the strict branch is the one the gate proves.

use axum::body::Body;
use axum::http::header::CONTENT_TYPE;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use consolebook_server::data_dir::DataDir;
use consolebook_server::{http, setup, storage, web_assets};

async fn app() -> (tempfile::TempDir, axum::Router, sqlx::SqlitePool) {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let data_dir = DataDir::new(tmp.path().join("data"));
    data_dir.ensure_layout().expect("create layout");
    let pool = storage::open(&data_dir.database()).await.expect("open");
    let router = http::router(http::AppState { pool: pool.clone() });
    (tmp, router, pool)
}

async fn get(router: axum::Router, uri: &str) -> axum::response::Response {
    router
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response")
}

#[tokio::test]
async fn unknown_api_routes_return_json_404_not_html() {
    let (_tmp, router, _pool) = app().await;
    let response = get(router, "/api/does-not-exist").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .expect("content type")
        .to_str()
        .expect("ascii");
    assert!(content_type.starts_with("application/json"));
}

#[tokio::test]
async fn root_serves_spa_or_honest_notice() {
    let (_tmp, router, _pool) = app().await;
    let response = get(router.clone(), "/").await;
    if web_assets::embedded() {
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .expect("content type")
            .to_str()
            .expect("ascii");
        assert!(content_type.starts_with("text/html"));

        // Client-side routes fall back to the same SPA entry point.
        let spa_route = get(router, "/login").await;
        assert_eq!(spa_route.status(), StatusCode::OK);
    } else {
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}

#[tokio::test]
async fn instance_reports_initialization_state() {
    let (_tmp, router, pool) = app().await;

    let response = get(router.clone(), "/api/instance").await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(body["initialized"], false);
    assert_eq!(body["agency"], serde_json::Value::Null);
    assert_eq!(body["version"], consolebook_server::VERSION);

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
        "invented-passphrase-1",
    )
    .await
    .expect("initialize")
    .expect("accepted");

    let response = get(router, "/api/instance").await;
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(body["initialized"], true);
    assert_eq!(body["agency"], "Example County Communications");
}
