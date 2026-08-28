//! HTTP API.
//!
//! Milestone 1 only exposes health. Routes are versionless under `/api/`;
//! application boundaries follow domain capabilities, not web routes.

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use serde::Serialize;
use sqlx::SqlitePool;

use crate::VERSION;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .with_state(state)
}

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
