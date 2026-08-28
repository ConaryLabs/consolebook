//! Integration tests for first-run setup, sessions, password reset, and
//! recovery. All fixtures are invented (Example County Communications).

use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, SET_COOKIE};
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use consolebook_server::data_dir::DataDir;
use consolebook_server::users::{IssueRefusal, ResetOrigin};
use consolebook_server::{http, setup, storage, users};

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
        Self { _tmp: tmp, pool }
    }

    fn app(&self) -> axum::Router {
        http::router(http::AppState {
            pool: self.pool.clone(),
        })
    }

    async fn issue_setup_code(&self) -> String {
        setup::issue_setup_code(&self.pool)
            .await
            .expect("issue setup code")
            .expect("uninitialized")
            .0
            .raw
    }

    /// Completes setup for the invented agency and returns nothing; login
    /// is the caller's business.
    async fn initialized(&self) {
        let code = self.issue_setup_code().await;
        let response = post_json(
            self.app(),
            "/api/setup",
            None,
            json!({
                "setup_code": code,
                "agency_name": "Example County Communications",
                "username": "avery.admin",
                "display_name": "Avery Admin",
                "password": PASSWORD,
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
    }

    /// Logs in and returns the session cookie value.
    async fn login(&self, username: &str, password: &str) -> String {
        let response = post_json(
            self.app(),
            "/api/auth/login",
            None,
            json!({ "username": username, "password": password }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        session_cookie(&response)
    }
}

async fn post_json(
    app: axum::Router,
    uri: &str,
    cookie: Option<&str>,
    body: Value,
) -> axum::response::Response {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header(CONTENT_TYPE, "application/json");
    if let Some(cookie) = cookie {
        builder = builder.header(COOKIE, format!("{}={}", http::SESSION_COOKIE, cookie));
    }
    app.oneshot(builder.body(Body::from(body.to_string())).expect("request"))
        .await
        .expect("response")
}

async fn get(app: axum::Router, uri: &str, cookie: Option<&str>) -> axum::response::Response {
    let mut builder = Request::builder().uri(uri);
    if let Some(cookie) = cookie {
        builder = builder.header(COOKIE, format!("{}={}", http::SESSION_COOKIE, cookie));
    }
    app.oneshot(builder.body(Body::empty()).expect("request"))
        .await
        .expect("response")
}

fn session_cookie(response: &axum::response::Response) -> String {
    let set_cookie = response
        .headers()
        .get(SET_COOKIE)
        .expect("session cookie set")
        .to_str()
        .expect("cookie is ascii");
    assert!(set_cookie.contains("HttpOnly"), "cookie must be HttpOnly");
    assert!(
        set_cookie.contains("SameSite=Strict"),
        "cookie must be SameSite=Strict"
    );
    let (name_value, _) = set_cookie.split_once(';').expect("cookie attributes");
    let (name, value) = name_value.split_once('=').expect("cookie pair");
    assert_eq!(name, http::SESSION_COOKIE);
    value.to_string()
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("json body")
}

// ------------------------------------------------------------------ setup

#[tokio::test]
async fn setup_login_session_lifecycle() {
    let fx = Fixture::new().await;
    fx.initialized().await;

    let cookie = fx.login("avery.admin", PASSWORD).await;

    let response = get(fx.app(), "/api/auth/session", Some(&cookie)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["user"]["username"], "avery.admin");
    assert!(
        body["capabilities"]
            .as_array()
            .expect("capabilities array")
            .iter()
            .any(|c| c == "manage_users"),
        "first administrator holds manage_users"
    );

    // Logout revokes immediately.
    let response = post_json(fx.app(), "/api/auth/logout", Some(&cookie), json!({})).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let response = get(fx.app(), "/api/auth/session", Some(&cookie)).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn setup_requires_valid_code() {
    let fx = Fixture::new().await;
    let _real = fx.issue_setup_code().await;
    let response = post_json(
        fx.app(),
        "/api/setup",
        None,
        json!({
            "setup_code": "00000000000000000000000000000000",
            "agency_name": "Example County Communications",
            "username": "avery.admin",
            "password": PASSWORD,
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(!setup::is_initialized(&fx.pool).await.expect("state"));
}

#[tokio::test]
async fn setup_code_is_single_use_and_setup_unrepeatable() {
    let fx = Fixture::new().await;
    let code = fx.issue_setup_code().await;
    let request = json!({
        "setup_code": code,
        "agency_name": "Example County Communications",
        "username": "avery.admin",
        "password": PASSWORD,
    });
    let response = post_json(fx.app(), "/api/setup", None, request.clone()).await;
    assert_eq!(response.status(), StatusCode::CREATED);

    // Same code again: the installation is initialized and the code is gone.
    let response = post_json(fx.app(), "/api/setup", None, request).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);

    // No further setup codes once initialized.
    assert!(
        setup::issue_setup_code(&fx.pool)
            .await
            .expect("issue")
            .is_none()
    );
}

#[tokio::test]
async fn setup_rejects_expired_code() {
    let fx = Fixture::new().await;
    let code = fx.issue_setup_code().await;
    sqlx::query("UPDATE setup_code SET expires_at = 1")
        .execute(&fx.pool)
        .await
        .expect("expire code");
    let response = post_json(
        fx.app(),
        "/api/setup",
        None,
        json!({
            "setup_code": code,
            "agency_name": "Example County Communications",
            "username": "avery.admin",
            "password": PASSWORD,
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn setup_enforces_password_policy() {
    let fx = Fixture::new().await;
    let code = fx.issue_setup_code().await;
    let response = post_json(
        fx.app(),
        "/api/setup",
        None,
        json!({
            "setup_code": code,
            "agency_name": "Example County Communications",
            "username": "avery.admin",
            "password": "short",
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

// ------------------------------------------------------------------ login

#[tokio::test]
async fn login_rejects_bad_password_and_unknown_user_identically() {
    let fx = Fixture::new().await;
    fx.initialized().await;

    let wrong_password = post_json(
        fx.app(),
        "/api/auth/login",
        None,
        json!({ "username": "avery.admin", "password": "wrong-password-1" }),
    )
    .await;
    let unknown_user = post_json(
        fx.app(),
        "/api/auth/login",
        None,
        json!({ "username": "nobody.here", "password": "wrong-password-1" }),
    )
    .await;
    assert_eq!(wrong_password.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(unknown_user.status(), StatusCode::UNAUTHORIZED);
    let a = body_json(wrong_password).await;
    let b = body_json(unknown_user).await;
    assert_eq!(a, b, "responses must not reveal whether the account exists");
}

#[tokio::test]
async fn login_is_case_insensitive_on_username() {
    let fx = Fixture::new().await;
    fx.initialized().await;
    let _cookie = fx.login("AVERY.ADMIN", PASSWORD).await;
}

#[tokio::test]
async fn expired_session_is_rejected() {
    let fx = Fixture::new().await;
    fx.initialized().await;
    let cookie = fx.login("avery.admin", PASSWORD).await;

    sqlx::query("UPDATE session SET expires_at = 1")
        .execute(&fx.pool)
        .await
        .expect("expire sessions");
    let response = get(fx.app(), "/api/auth/session", Some(&cookie)).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn garbage_session_token_is_rejected() {
    let fx = Fixture::new().await;
    fx.initialized().await;
    let response = get(fx.app(), "/api/auth/session", Some("not-a-real-token")).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

// ------------------------------------------------------------------ reset

#[tokio::test]
async fn admin_issued_reset_revokes_sessions_and_rotates_password() {
    let fx = Fixture::new().await;
    fx.initialized().await;
    let admin_cookie = fx.login("avery.admin", PASSWORD).await;

    let response = post_json(
        fx.app(),
        "/api/auth/reset-codes",
        Some(&admin_cookie),
        json!({ "username": "avery.admin" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = body_json(response).await;
    let reset_code = body["reset_code"].as_str().expect("code").to_string();

    let new_password = "rotated-passphrase-2";
    let response = post_json(
        fx.app(),
        "/api/auth/reset",
        None,
        json!({
            "username": "avery.admin",
            "reset_code": reset_code,
            "new_password": new_password,
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Old session revoked, old password dead, new password works.
    let response = get(fx.app(), "/api/auth/session", Some(&admin_cookie)).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = post_json(
        fx.app(),
        "/api/auth/login",
        None,
        json!({ "username": "avery.admin", "password": PASSWORD }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let _cookie = fx.login("avery.admin", new_password).await;

    // The code is single-use.
    let response = post_json(
        fx.app(),
        "/api/auth/reset",
        None,
        json!({
            "username": "avery.admin",
            "reset_code": reset_code,
            "new_password": "another-passphrase-3",
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn issuing_reset_codes_requires_manage_users() {
    let fx = Fixture::new().await;
    fx.initialized().await;

    // A user with no capabilities (created directly; user management API is
    // a later slice).
    let hash = consolebook_server::secrets::hash_password(PASSWORD).expect("hash");
    let mut conn = fx.pool.acquire().await.expect("conn");
    users::create(&mut conn, "taylor.trainee", "Taylor Trainee", "", "", &hash)
        .await
        .expect("create user");
    drop(conn);

    let cookie = fx.login("taylor.trainee", PASSWORD).await;
    let response = post_json(
        fx.app(),
        "/api/auth/reset-codes",
        Some(&cookie),
        json!({ "username": "avery.admin" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // And unauthenticated is unauthenticated.
    let response = post_json(
        fx.app(),
        "/api/auth/reset-codes",
        None,
        json!({ "username": "avery.admin" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn expired_reset_code_is_refused() {
    let fx = Fixture::new().await;
    fx.initialized().await;
    let issued = users::issue_reset_code(
        &fx.pool,
        "avery.admin",
        ResetOrigin::Administrator { issued_by: 1 },
    )
    .await
    .expect("issue")
    .expect("issued");
    sqlx::query("UPDATE password_reset_code SET expires_at = 1")
        .execute(&fx.pool)
        .await
        .expect("expire");
    let outcome = users::use_reset_code(
        &fx.pool,
        "avery.admin",
        &issued.code.raw,
        "rotated-passphrase-2",
    )
    .await
    .expect("use");
    assert_eq!(outcome, users::ResetOutcome::Invalid);
}

// --------------------------------------------------------------- recovery

#[tokio::test]
async fn recovery_rescues_an_administrator_without_credentials() {
    let fx = Fixture::new().await;
    fx.initialized().await;

    let issued = users::issue_reset_code(&fx.pool, "avery.admin", ResetOrigin::Recovery)
        .await
        .expect("recover")
        .expect("issued");
    let outcome = users::use_reset_code(
        &fx.pool,
        "avery.admin",
        &issued.code.raw,
        "recovered-passphrase-4",
    )
    .await
    .expect("use");
    assert_eq!(outcome, users::ResetOutcome::Done);
    let _cookie = fx.login("avery.admin", "recovered-passphrase-4").await;

    // The recovery is on the audit record.
    let kinds: Vec<String> = sqlx::query_scalar("SELECT kind FROM audit_event ORDER BY id")
        .fetch_all(&fx.pool)
        .await
        .expect("audit rows");
    assert!(kinds.iter().any(|k| k == "recovery_code_issued"));
}

#[tokio::test]
async fn recovery_refuses_non_administrators_and_unknown_users() {
    let fx = Fixture::new().await;
    fx.initialized().await;
    let hash = consolebook_server::secrets::hash_password(PASSWORD).expect("hash");
    let mut conn = fx.pool.acquire().await.expect("conn");
    users::create(&mut conn, "taylor.trainee", "Taylor Trainee", "", "", &hash)
        .await
        .expect("create user");
    drop(conn);

    let refused = users::issue_reset_code(&fx.pool, "taylor.trainee", ResetOrigin::Recovery)
        .await
        .expect("call");
    assert_eq!(refused.unwrap_err(), IssueRefusal::NotAnAdministrator);

    let refused = users::issue_reset_code(&fx.pool, "nobody.here", ResetOrigin::Recovery)
        .await
        .expect("call");
    assert_eq!(refused.unwrap_err(), IssueRefusal::NoSuchUser);
}

// ------------------------------------------------------------------ audit

#[tokio::test]
async fn audit_events_are_append_only_at_the_database() {
    let fx = Fixture::new().await;
    fx.initialized().await;

    let update = sqlx::query("UPDATE audit_event SET kind = 'rewritten'")
        .execute(&fx.pool)
        .await;
    assert!(update.is_err(), "audit events must reject UPDATE");
    let delete = sqlx::query("DELETE FROM audit_event")
        .execute(&fx.pool)
        .await;
    assert!(delete.is_err(), "audit events must reject DELETE");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_event")
        .fetch_one(&fx.pool)
        .await
        .expect("count");
    assert!(count >= 1, "setup itself must be audited");
}
