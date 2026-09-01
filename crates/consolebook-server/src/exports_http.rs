//! Record export HTTP handlers (Milestone 5 slice 1; ADR 0014).
//!
//! `http.rs` remains the hub; this module owns the export downloads and
//! the installation-export summary. Scope rules live in
//! `record_export`; handlers translate refusals into stable error codes
//! and deliver the documented archive bytes as attachments.

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;

use crate::http::{ApiError, AppState, CurrentUser};
use crate::record_export::{self, ExportRefusal, Scope};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/drafts/{id}/export", get(export_record))
        .route(
            "/api/drafts/{id}/versions/{number}/export",
            get(export_version),
        )
        .route("/api/enrollments/{id}/export", get(export_enrollment))
        .route("/api/exports/records", get(export_installation))
        .route("/api/exports/summary", get(export_summary))
}

fn export_refusal(refusal: ExportRefusal) -> ApiError {
    match refusal {
        ExportRefusal::NoSuchRecord => {
            ApiError::new(StatusCode::NOT_FOUND, "no_such_record", "no such record")
        }
        ExportRefusal::NoSuchVersion => ApiError::new(
            StatusCode::NOT_FOUND,
            "no_such_version",
            "this record has no finalized version with that number",
        ),
        ExportRefusal::NoSuchEnrollment => ApiError::new(
            StatusCode::NOT_FOUND,
            "no_such_enrollment",
            "no such enrollment",
        ),
        ExportRefusal::CapabilityRequired => ApiError::new(
            StatusCode::FORBIDDEN,
            "capability_required",
            "exporting takes the scope's read authority; the whole installation takes export_records",
        ),
        ExportRefusal::NothingToExport => ApiError::new(
            StatusCode::CONFLICT,
            "nothing_to_export",
            "this scope holds no finalized version; an export never claims completeness it lacks",
        ),
    }
}

/// The archive as a download: exactly the documented bytes.
async fn deliver(state: &AppState, actor_user_id: i64, scope: Scope) -> Result<Response, ApiError> {
    match record_export::export(&state.pool, actor_user_id, scope).await? {
        Ok(export) => {
            let disposition = format!("attachment; filename=\"{}\"", export.file_name);
            Ok((
                [
                    (header::CONTENT_TYPE, "application/zip".to_owned()),
                    (header::CONTENT_DISPOSITION, disposition),
                ],
                export.bytes,
            )
                .into_response())
        }
        Err(refusal) => Err(export_refusal(refusal)),
    }
}

async fn export_version(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((record_id, version_number)): Path<(i64, i64)>,
) -> Result<Response, ApiError> {
    deliver(
        &state,
        current.user.id,
        Scope::Version {
            record_id,
            version_number,
        },
    )
    .await
}

async fn export_record(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(record_id): Path<i64>,
) -> Result<Response, ApiError> {
    deliver(&state, current.user.id, Scope::Record { record_id }).await
}

async fn export_enrollment(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(enrollment_id): Path<i64>,
) -> Result<Response, ApiError> {
    deliver(&state, current.user.id, Scope::Enrollment { enrollment_id }).await
}

async fn export_installation(
    State(state): State<AppState>,
    current: CurrentUser,
) -> Result<Response, ApiError> {
    deliver(&state, current.user.id, Scope::Installation).await
}

async fn export_summary(
    State(state): State<AppState>,
    current: CurrentUser,
) -> Result<Response, ApiError> {
    match record_export::summary(&state.pool, current.user.id).await? {
        Ok(summary) => Ok(Json(summary).into_response()),
        Err(refusal) => Err(export_refusal(refusal)),
    }
}
