//! Integration tests for the Milestone 1 operable shell: storage invariants,
//! health endpoint, doctor, and validated backups. All fixtures are invented.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use consolebook_server::data_dir::DataDir;
use consolebook_server::{backup, doctor, http, storage};

fn temp_data_dir() -> (tempfile::TempDir, DataDir) {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let data_dir = DataDir::new(tmp.path().join("data"));
    data_dir.ensure_layout().expect("create layout");
    (tmp, data_dir)
}

#[tokio::test]
async fn open_enforces_and_verifies_invariants() {
    let (_tmp, data_dir) = temp_data_dir();
    let pool = storage::open(&data_dir.database()).await.expect("open");

    let checks = storage::verify_invariants(&pool).await.expect("verify");
    assert_eq!(checks.len(), 4);
    for check in &checks {
        assert!(
            check.holds(),
            "invariant {} expected {} got {}",
            check.name,
            check.expected,
            check.actual
        );
    }
}

#[tokio::test]
async fn instance_identity_is_created_once_and_stable() {
    let (_tmp, data_dir) = temp_data_dir();

    let pool = storage::open(&data_dir.database()).await.expect("open");
    let first = storage::installation_id(&pool).await.expect("id");
    pool.close().await;

    // Re-opening must not mint a new identity.
    let pool = storage::open(&data_dir.database()).await.expect("reopen");
    let second = storage::installation_id(&pool).await.expect("id");
    assert_eq!(first, second);
}

#[tokio::test]
async fn open_existing_refuses_to_create() {
    let (_tmp, data_dir) = temp_data_dir();
    let missing = data_dir.database();
    let err = storage::open_existing(&missing).await;
    assert!(err.is_err(), "open_existing must not create a database");
    assert!(!missing.exists());
}

#[tokio::test]
async fn health_endpoint_reports_ok_with_database() {
    let (_tmp, data_dir) = temp_data_dir();
    let pool = storage::open(&data_dir.database()).await.expect("open");
    let app = http::router(http::AppState { pool });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/health")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(body["status"], "ok");
    assert_eq!(body["database"], "ok");
    assert_eq!(body["version"], consolebook_server::VERSION);
}

#[tokio::test]
async fn health_endpoint_degrades_when_database_is_gone() {
    let (_tmp, data_dir) = temp_data_dir();
    let pool = storage::open(&data_dir.database()).await.expect("open");
    pool.close().await;
    let app = http::router(http::AppState { pool });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/health")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn backup_produces_validated_snapshot() {
    let (_tmp, data_dir) = temp_data_dir();
    let pool = storage::open(&data_dir.database()).await.expect("open");
    let live_id = storage::installation_id(&pool).await.expect("id");
    pool.close().await;

    let report = backup::run(&data_dir, backup::DEFAULT_KEEP)
        .await
        .expect("backup");
    assert!(report.snapshot.starts_with(data_dir.backups()));
    assert!(report.size_bytes > 0);

    // The snapshot is a complete database: it opens on its own and carries
    // the same instance identity.
    let snapshot_pool = storage::open_existing(&report.snapshot)
        .await
        .expect("open snapshot");
    let snapshot_id = storage::installation_id(&snapshot_pool)
        .await
        .expect("snapshot id");
    assert_eq!(live_id, snapshot_id);
    let verdict = storage::integrity_check(&snapshot_pool)
        .await
        .expect("integrity");
    assert_eq!(verdict, ["ok"]);
}

#[tokio::test]
async fn backup_fails_without_database() {
    let (_tmp, data_dir) = temp_data_dir();
    assert!(backup::run(&data_dir, backup::DEFAULT_KEEP).await.is_err());
}

#[tokio::test]
async fn doctor_passes_on_healthy_installation() {
    let (_tmp, data_dir) = temp_data_dir();
    let pool = storage::open(&data_dir.database()).await.expect("open");
    pool.close().await;

    let findings = doctor::run(&data_dir).await;
    assert!(
        !doctor::has_failure(&findings),
        "unexpected failures: {findings:?}"
    );
}

#[tokio::test]
async fn doctor_fails_on_uninitialized_installation() {
    let (_tmp, data_dir) = temp_data_dir();
    let findings = doctor::run(&data_dir).await;
    assert!(doctor::has_failure(&findings));
}

#[tokio::test]
async fn doctor_does_not_create_a_database() {
    let (_tmp, data_dir) = temp_data_dir();
    let _ = doctor::run(&data_dir).await;
    assert!(!data_dir.database().exists());
}
