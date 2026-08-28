//! Integration tests for persisted in-app notices: recipient scoping,
//! deduplication, the backup-failure producer path, and the API.
//! All fixtures are invented.

use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE};
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use consolebook_server::capabilities::Capability;
use consolebook_server::data_dir::DataDir;
use consolebook_server::notices::{self, NoticeKind};
use consolebook_server::{http, secrets, setup, storage, users};

const PASSWORD: &str = "invented-passphrase-1";

struct Fixture {
    _tmp: tempfile::TempDir,
    pool: sqlx::SqlitePool,
    admin_id: i64,
    trainee_id: i64,
}

impl Fixture {
    /// One administrator (avery.admin) and one capability-less user
    /// (taylor.trainee).
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
        let hash = secrets::hash_password(PASSWORD).expect("hash");
        let mut conn = pool.acquire().await.expect("conn");
        let trainee_id = users::create(&mut conn, "taylor.trainee", "Taylor Trainee", &hash)
            .await
            .expect("create");
        drop(conn);
        Self {
            _tmp: tmp,
            pool,
            admin_id,
            trainee_id,
        }
    }

    fn app(&self) -> axum::Router {
        http::router(http::AppState {
            pool: self.pool.clone(),
        })
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
            .get(axum::http::header::SET_COOKIE)
            .expect("cookie")
            .to_str()
            .expect("ascii");
        let (pair, _) = cookie.split_once(';').expect("attrs");
        pair.split_once('=').expect("pair").1.to_string()
    }
}

async fn get_json(app: axum::Router, uri: &str, cookie: &str) -> (StatusCode, serde_json::Value) {
    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(COOKIE, format!("{}={}", http::SESSION_COOKIE, cookie))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
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

async fn post(app: axum::Router, uri: &str, cookie: &str) -> StatusCode {
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(CONTENT_TYPE, "application/json")
            .header(COOKIE, format!("{}={}", http::SESSION_COOKIE, cookie))
            .body(Body::from("{}"))
            .expect("request"),
    )
    .await
    .expect("response")
    .status()
}

#[tokio::test]
async fn backup_failure_notifies_administrators_once() {
    let fx = Fixture::new().await;

    let created = notices::notify_capability_holders(
        &fx.pool,
        Capability::ManageUsers,
        NoticeKind::BackupFailed,
        "Automatic backup failed: invented disk-full error.",
    )
    .await
    .expect("notify");
    assert_eq!(created, 1, "one administrator, one notice");

    // A second failure while the first is unread creates nothing.
    let created = notices::notify_capability_holders(
        &fx.pool,
        Capability::ManageUsers,
        NoticeKind::BackupFailed,
        "Automatic backup failed again.",
    )
    .await
    .expect("notify");
    assert_eq!(created, 0, "unread notice suppresses duplicates");

    // Acknowledged, the next failure notifies again.
    let list = notices::list_for_user(&fx.pool, fx.admin_id)
        .await
        .expect("list");
    assert_eq!(list.len(), 1);
    assert!(
        notices::mark_read(&fx.pool, fx.admin_id, list[0].id)
            .await
            .expect("mark")
    );
    let created = notices::notify_capability_holders(
        &fx.pool,
        Capability::ManageUsers,
        NoticeKind::BackupFailed,
        "Automatic backup failed a third time.",
    )
    .await
    .expect("notify");
    assert_eq!(created, 1);

    // The capability-less user never heard about any of it.
    let trainee_list = notices::list_for_user(&fx.pool, fx.trainee_id)
        .await
        .expect("list");
    assert!(trainee_list.is_empty());
}

#[tokio::test]
async fn notices_are_recipient_scoped_end_to_end() {
    let fx = Fixture::new().await;
    notices::notify_capability_holders(
        &fx.pool,
        Capability::ManageUsers,
        NoticeKind::BackupFailed,
        "Automatic backup failed: invented error.",
    )
    .await
    .expect("notify");

    let admin_cookie = fx.login("avery.admin").await;
    let trainee_cookie = fx.login("taylor.trainee").await;

    // The administrator sees their notice.
    let (status, body) = get_json(fx.app(), "/api/notices", &admin_cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["unread"], 1);
    let notice_id = body["notices"][0]["id"].as_i64().expect("id");

    // The trainee sees nothing and cannot read or acknowledge the
    // administrator's notice.
    let (status, body) = get_json(fx.app(), "/api/notices", &trainee_cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["unread"], 0);
    let refused = post(
        fx.app(),
        &format!("/api/notices/{notice_id}/read"),
        &trainee_cookie,
    )
    .await;
    assert_eq!(refused, StatusCode::NOT_FOUND);

    // The administrator acknowledges; the unread count drops and a second
    // acknowledgment of the same notice is a 404 (already read).
    let done = post(
        fx.app(),
        &format!("/api/notices/{notice_id}/read"),
        &admin_cookie,
    )
    .await;
    assert_eq!(done, StatusCode::NO_CONTENT);
    let (_, body) = get_json(fx.app(), "/api/notices", &admin_cookie).await;
    assert_eq!(body["unread"], 0);
    assert!(body["notices"][0]["read_at"].is_i64());
    let again = post(
        fx.app(),
        &format!("/api/notices/{notice_id}/read"),
        &admin_cookie,
    )
    .await;
    assert_eq!(again, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn notices_require_authentication() {
    let fx = Fixture::new().await;
    let response = fx
        .app()
        .oneshot(
            Request::builder()
                .uri("/api/notices")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
