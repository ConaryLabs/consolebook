//! HTTP API.
//!
//! Routes are versionless under `/api/`; application boundaries follow
//! domain capabilities, not web routes. Handlers translate between HTTP and
//! the domain services; policy lives in the services.
//!
//! This module is the hub: it owns the router, `ApiError`, and the
//! `CurrentUser` extractor. Handler groups for larger domains live in
//! their own modules (`programs_http`) and register through `router`.
//!
//! Error responses are `{"error": <stable machine code>, "message": ...}`,
//! plus a `problems` array when a refusal carries itemized reasons.
//! Authentication failures stay deliberately generic so responses do not
//! reveal whether an account exists.

use axum::Router;
use axum::extract::{FromRequestParts, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::VERSION;
use crate::audit::{self, EventKind};
use crate::capabilities::{self, Capability};
use crate::secrets;
use crate::sessions;
use crate::setup::{self, SetupRefusal};
use crate::users::{self, IssueRefusal, ResetOrigin, ResetOutcome};

/// Name of the session cookie.
pub const SESSION_COOKIE: &str = "consolebook_session";

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/instance", get(instance))
        .route("/api/setup", post(setup_handler))
        .route("/api/auth/login", post(login))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/session", get(current_session))
        .route("/api/auth/reset-codes", post(issue_reset_code))
        .route("/api/auth/reset", post(reset_password))
        .route("/api/users", get(list_users).post(create_user))
        .route("/api/notices", get(list_notices))
        .route("/api/notices/{id}/read", post(mark_notice_read))
        .merge(crate::programs_http::routes())
        .merge(crate::training_http::routes())
        .merge(crate::drafts_http::routes())
        .fallback(crate::web_assets::serve)
        .with_state(state)
}

// ---------------------------------------------------------------- errors

/// An API error with a stable machine-readable code.
pub(crate) struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    problems: Vec<String>,
}

impl ApiError {
    pub(crate) fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            problems: Vec::new(),
        }
    }

    /// An error carrying itemized refusal reasons the interface can list.
    pub(crate) fn with_problems(
        status: StatusCode,
        code: &'static str,
        message: impl Into<String>,
        problems: Vec<String>,
    ) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            problems,
        }
    }

    fn unauthenticated() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
            "sign in to continue",
        )
    }

    fn internal(err: &anyhow::Error) -> Self {
        // Log the cause chain; the response stays content-free.
        tracing::error!("internal error: {err:#}");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "internal error",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = serde_json::json!({ "error": self.code, "message": self.message });
        if !self.problems.is_empty() {
            body["problems"] = serde_json::json!(self.problems);
        }
        (self.status, Json(body)).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        Self::internal(&err)
    }
}

// ------------------------------------------------------------ extractor

/// The authenticated caller, resolved from the session cookie.
pub struct CurrentUser {
    pub user: users::User,
    pub session_expires_at: i64,
}

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let jar = CookieJar::from_headers(&parts.headers);
        let Some(cookie) = jar.get(SESSION_COOKIE) else {
            return Err(ApiError::unauthenticated().into_response());
        };
        let live = sessions::validate(&state.pool, cookie.value())
            .await
            .map_err(|err| ApiError::internal(&err).into_response())?;
        let Some(live) = live else {
            return Err(ApiError::unauthenticated().into_response());
        };
        let user = users::find_by_id(&state.pool, live.user_id)
            .await
            .map_err(|err| ApiError::internal(&err).into_response())?
            .ok_or_else(|| ApiError::unauthenticated().into_response())?;
        Ok(Self {
            user,
            session_expires_at: live.expires_at,
        })
    }
}

fn session_cookie(token: String) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, token))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Strict)
        .max_age(time::Duration::seconds(sessions::SESSION_TTL_SECONDS))
        .build()
}

fn removal_cookie() -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, ""))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Strict)
        .max_age(time::Duration::ZERO)
        .build()
}

// --------------------------------------------------------------- health

#[derive(Serialize)]
struct Health {
    status: &'static str,
    version: &'static str,
    database: &'static str,
}

/// Liveness plus a lightweight database round trip. Returns 503 when the
/// database cannot answer, so a reverse proxy or monitor can act on it.
async fn health(State(state): State<AppState>) -> Response {
    let database_ok = sqlx::query("SELECT 1").fetch_one(&state.pool).await.is_ok();
    let body = Health {
        status: if database_ok { "ok" } else { "degraded" },
        version: VERSION,
        database: if database_ok { "ok" } else { "unavailable" },
    };
    let code = if database_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (code, Json(body)).into_response()
}

// ------------------------------------------------------------- instance

#[derive(Serialize)]
struct Instance {
    initialized: bool,
    version: &'static str,
    agency: Option<String>,
}

/// Unauthenticated installation facts the web shell needs to route:
/// whether setup has run, the running version, and the agency name.
async fn instance(State(state): State<AppState>) -> Result<Json<Instance>, ApiError> {
    let agency = setup::agency_name(&state.pool).await?;
    Ok(Json(Instance {
        initialized: agency.is_some(),
        version: VERSION,
        agency,
    }))
}

// ---------------------------------------------------------------- setup

#[derive(Deserialize)]
struct SetupRequest {
    setup_code: String,
    agency_name: String,
    username: String,
    #[serde(default)]
    display_name: String,
    password: String,
}

async fn setup_handler(
    State(state): State<AppState>,
    Json(req): Json<SetupRequest>,
) -> Result<Response, ApiError> {
    let outcome = setup::initialize(
        &state.pool,
        &req.setup_code,
        &req.agency_name,
        &req.username,
        &req.display_name,
        &req.password,
    )
    .await?;
    match outcome {
        Ok(user_id) => {
            tracing::info!(user_id, "first-run setup completed");
            let body = serde_json::json!({ "administrator_user_id": user_id });
            Ok((StatusCode::CREATED, Json(body)).into_response())
        }
        Err(SetupRefusal::AlreadyInitialized) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "already_initialized",
            "this installation is already initialized",
        )),
        Err(SetupRefusal::InvalidCode) => Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "invalid_setup",
            "setup code or request is invalid; restart the server or run `consolebook setup-code` for a fresh code",
        )),
        Err(SetupRefusal::PasswordPolicy(reason)) => Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "password_policy",
            reason,
        )),
    }
}

// ----------------------------------------------------------------- auth

#[derive(Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct SessionBody {
    user: UserBody,
    capabilities: Vec<String>,
    expires_at: i64,
}

#[derive(Serialize)]
struct UserBody {
    id: i64,
    username: String,
    display_name: String,
}

async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(req): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    let invalid = || {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "username or password is incorrect",
        )
    };
    let Some(user) = users::find_by_username(&state.pool, &req.username).await? else {
        // Equalize timing with a real verification against a dummy hash.
        let _ = secrets::verify_password(&req.password, secrets::dummy_password_hash());
        return Err(invalid());
    };
    if !secrets::verify_password(&req.password, &user.password_hash) {
        audit::record(&state.pool, EventKind::LoginFailed, None, Some(user.id)).await?;
        return Err(invalid());
    }

    let (token, expires_at) = sessions::create(&state.pool, user.id).await?;
    audit::record(
        &state.pool,
        EventKind::LoginSucceeded,
        Some(user.id),
        Some(user.id),
    )
    .await?;
    let capabilities = capabilities::list_for_user(&state.pool, user.id).await?;
    let body = SessionBody {
        user: UserBody {
            id: user.id,
            username: user.username,
            display_name: user.display_name,
        },
        capabilities,
        expires_at,
    };
    Ok((jar.add(session_cookie(token.raw)), Json(body)).into_response())
}

async fn logout(State(state): State<AppState>, jar: CookieJar) -> Result<Response, ApiError> {
    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        if let Some(live) = sessions::validate(&state.pool, cookie.value()).await? {
            audit::record(
                &state.pool,
                EventKind::Logout,
                Some(live.user_id),
                Some(live.user_id),
            )
            .await?;
        }
        sessions::revoke(&state.pool, cookie.value()).await?;
    }
    Ok((jar.add(removal_cookie()), StatusCode::NO_CONTENT).into_response())
}

async fn current_session(
    State(state): State<AppState>,
    current: CurrentUser,
) -> Result<Json<SessionBody>, ApiError> {
    let capabilities = capabilities::list_for_user(&state.pool, current.user.id).await?;
    Ok(Json(SessionBody {
        user: UserBody {
            id: current.user.id,
            username: current.user.username,
            display_name: current.user.display_name,
        },
        capabilities,
        expires_at: current.session_expires_at,
    }))
}

// ---------------------------------------------------------------- reset

#[derive(Deserialize)]
struct IssueResetRequest {
    username: String,
}

async fn issue_reset_code(
    State(state): State<AppState>,
    current: CurrentUser,
    Json(req): Json<IssueResetRequest>,
) -> Result<Response, ApiError> {
    if !capabilities::user_has(&state.pool, current.user.id, Capability::ManageUsers).await? {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "capability_required",
            "issuing reset codes requires the manage_users capability",
        ));
    }
    let issued = users::issue_reset_code(
        &state.pool,
        &req.username,
        ResetOrigin::Administrator {
            issued_by: current.user.id,
        },
    )
    .await?;
    match issued {
        Ok(issued) => {
            let body = serde_json::json!({
                "username": issued.user.username,
                "reset_code": issued.code.raw,
                "expires_at": issued.expires_at,
            });
            Ok((StatusCode::CREATED, Json(body)).into_response())
        }
        Err(IssueRefusal::NoSuchUser) => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no_such_user",
            "no user with that username",
        )),
        // Administrator-issued codes have no administrator-only restriction;
        // this refusal only exists on the recovery path.
        Err(IssueRefusal::NotAnAdministrator) => Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "internal error",
        )),
    }
}

#[derive(Deserialize)]
struct ResetRequest {
    username: String,
    reset_code: String,
    new_password: String,
}

async fn reset_password(
    State(state): State<AppState>,
    Json(req): Json<ResetRequest>,
) -> Result<Response, ApiError> {
    let outcome = users::use_reset_code(
        &state.pool,
        &req.username,
        &req.reset_code,
        &req.new_password,
    )
    .await?;
    match outcome {
        ResetOutcome::Done => Ok(StatusCode::NO_CONTENT.into_response()),
        ResetOutcome::Invalid => Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "invalid_reset",
            "username or reset code is invalid, expired, or already used",
        )),
        ResetOutcome::PasswordPolicy(reason) => Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "password_policy",
            reason,
        )),
    }
}

// ---------------------------------------------------------------- users

#[derive(Deserialize)]
struct CreateUserRequest {
    username: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    employee_id: String,
    #[serde(default)]
    title: String,
    /// Role bundle consumed at creation; defaults to Trainee (no grants).
    #[serde(default)]
    role: capabilities::RoleBundle,
}

/// Creates a user with the chosen role bundle's grants and returns the
/// one-time reset code their first sign-in redeems.
async fn create_user(
    State(state): State<AppState>,
    current: CurrentUser,
    Json(req): Json<CreateUserRequest>,
) -> Result<Response, ApiError> {
    if !capabilities::user_has(&state.pool, current.user.id, Capability::ManageUsers).await? {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "capability_required",
            "creating users requires the manage_users capability",
        ));
    }
    let created = users::create_with_reset_code(
        &state.pool,
        current.user.id,
        &req.username,
        &req.display_name,
        &req.employee_id,
        &req.title,
        req.role,
    )
    .await?;
    match created {
        Ok(created) => {
            let body = serde_json::json!({
                "id": created.id,
                "username": created.username,
                "display_name": created.display_name,
                "reset_code": created.reset_code.raw,
                "reset_expires_at": created.reset_expires_at,
            });
            Ok((StatusCode::CREATED, Json(body)).into_response())
        }
        Err(users::CreateUserRefusal::UsernameInvalid(reason)) => Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "username_invalid",
            reason,
        )),
        Err(users::CreateUserRefusal::UsernameTaken) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "username_taken",
            "a user with that username already exists",
        )),
    }
}

#[derive(Serialize)]
struct UsersBody {
    users: Vec<users::UserSummary>,
}

/// The roster, for user management and training assignment.
async fn list_users(
    State(state): State<AppState>,
    current: CurrentUser,
) -> Result<Json<UsersBody>, ApiError> {
    let allowed = capabilities::user_has(&state.pool, current.user.id, Capability::ManageUsers)
        .await?
        || capabilities::user_has(&state.pool, current.user.id, Capability::AssignTraining).await?;
    if !allowed {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "capability_required",
            "listing users requires the manage_users or assign_training capability",
        ));
    }
    let users = users::list(&state.pool).await?;
    Ok(Json(UsersBody { users }))
}

// -------------------------------------------------------------- notices

#[derive(Serialize)]
struct NoticesBody {
    notices: Vec<crate::notices::Notice>,
    unread: i64,
}

/// The caller's own notices, unread first.
async fn list_notices(
    State(state): State<AppState>,
    current: CurrentUser,
) -> Result<Json<NoticesBody>, ApiError> {
    let notices = crate::notices::list_for_user(&state.pool, current.user.id).await?;
    let unread = crate::notices::unread_count(&state.pool, current.user.id).await?;
    Ok(Json(NoticesBody { notices, unread }))
}

/// Marks one of the caller's own notices read. A notice that is not the
/// caller's is a 404, indistinguishable from one that does not exist.
async fn mark_notice_read(
    State(state): State<AppState>,
    current: CurrentUser,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Result<Response, ApiError> {
    if crate::notices::mark_read(&state.pool, current.user.id, id).await? {
        Ok(StatusCode::NO_CONTENT.into_response())
    } else {
        Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no_such_notice",
            "no such unread notice",
        ))
    }
}

// ---------------------------------------------------------------- serve

/// Serves the API until the process receives SIGINT or SIGTERM.
pub async fn serve(listener: tokio::net::TcpListener, state: AppState) -> anyhow::Result<()> {
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("installing SIGINT handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("installing SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("shutdown signal received");
}
