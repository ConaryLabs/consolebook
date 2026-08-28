//! Program-configuration HTTP handlers (Milestone 2 authoring slice).
//!
//! `http.rs` remains the hub — router registration, `ApiError`, and the
//! `CurrentUser` extractor live there; this module owns only the
//! program endpoints. Reads require a signed-in session; mutations are
//! additionally refused by the domain services without the
//! `manage_programs` capability. Handlers translate refusal enums into
//! stable error codes and never restate policy.

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post, put};
use serde::{Deserialize, Serialize};

use crate::http::{ApiError, AppState, CurrentUser};
use crate::program_export::{self, ImportRefusal, ImportTarget};
use crate::programs::{
    self, AuthorRefusal, ProgramRefusal, ProgramSummary, PublishRefusal, VersionContent,
    VersionSummary,
};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/programs", get(list_programs).post(create_program))
        .route("/api/programs/import", post(import_new_program))
        .route(
            "/api/programs/{id}/versions",
            get(list_versions).post(create_version),
        )
        .route(
            "/api/programs/{id}/versions/import",
            post(import_next_version),
        )
        .route(
            "/api/program-versions/{id}",
            get(get_version).delete(discard_version),
        )
        .route("/api/program-versions/{id}/content", put(replace_content))
        .route("/api/program-versions/{id}/publish", post(publish_version))
        .route("/api/program-versions/{id}/export", get(export_version))
}

// ------------------------------------------------------ refusal mapping

fn capability_required() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "capability_required",
        "managing programs requires the manage_programs capability",
    )
}

fn no_such_program() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "no_such_program", "no such program")
}

fn no_such_version() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "no_such_version",
        "no such program version",
    )
}

fn already_published() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "already_published",
        "published program versions are immutable",
    )
}

fn invalid_content(problems: Vec<String>) -> ApiError {
    ApiError::with_problems(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_content",
        "the content document is invalid",
        problems,
    )
}

fn program_refusal(refusal: &ProgramRefusal) -> ApiError {
    match refusal {
        ProgramRefusal::CapabilityRequired => capability_required(),
        ProgramRefusal::NameEmpty => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "name_empty",
            "a program needs a non-empty name",
        ),
        ProgramRefusal::NameTaken => ApiError::new(
            StatusCode::CONFLICT,
            "name_taken",
            "a program with that name already exists",
        ),
    }
}

fn author_refusal(refusal: AuthorRefusal) -> ApiError {
    match refusal {
        AuthorRefusal::CapabilityRequired => capability_required(),
        AuthorRefusal::NoSuchProgram => no_such_program(),
        AuthorRefusal::NoSuchVersion => no_such_version(),
        AuthorRefusal::AlreadyPublished => already_published(),
        AuthorRefusal::Invalid(problems) => invalid_content(problems),
    }
}

fn publish_refusal(refusal: PublishRefusal) -> ApiError {
    match refusal {
        PublishRefusal::CapabilityRequired => capability_required(),
        PublishRefusal::NoSuchVersion => no_such_version(),
        PublishRefusal::AlreadyPublished => already_published(),
        PublishRefusal::Incomplete(problems) => ApiError::with_problems(
            StatusCode::UNPROCESSABLE_ENTITY,
            "incomplete",
            "the version is not complete enough to publish",
            problems,
        ),
    }
}

fn import_refusal(refusal: ImportRefusal) -> ApiError {
    match refusal {
        ImportRefusal::CapabilityRequired => capability_required(),
        ImportRefusal::UnsupportedFormat(message) => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "unsupported_format",
            message,
        ),
        ImportRefusal::Invalid(problems) => invalid_content(problems),
        ImportRefusal::ProgramNameTaken => ApiError::new(
            StatusCode::CONFLICT,
            "name_taken",
            "a program with the document's name already exists",
        ),
        ImportRefusal::NoSuchProgram => no_such_program(),
    }
}

// ------------------------------------------------------------- programs

#[derive(Serialize)]
struct ProgramsBody {
    programs: Vec<ProgramSummary>,
}

async fn list_programs(
    State(state): State<AppState>,
    _current: CurrentUser,
) -> Result<Json<ProgramsBody>, ApiError> {
    let programs = programs::list_programs(&state.pool).await?;
    Ok(Json(ProgramsBody { programs }))
}

#[derive(Deserialize)]
struct CreateProgramRequest {
    name: String,
}

async fn create_program(
    State(state): State<AppState>,
    current: CurrentUser,
    Json(req): Json<CreateProgramRequest>,
) -> Result<Response, ApiError> {
    match programs::create_program(&state.pool, current.user.id, &req.name).await? {
        Ok(id) => {
            let body = serde_json::json!({ "id": id });
            Ok((StatusCode::CREATED, Json(body)).into_response())
        }
        Err(refusal) => Err(program_refusal(&refusal)),
    }
}

// ------------------------------------------------------------- versions

#[derive(Serialize)]
struct VersionsBody {
    program: ProgramSummary,
    versions: Vec<VersionSummary>,
}

async fn list_versions(
    State(state): State<AppState>,
    _current: CurrentUser,
    Path(program_id): Path<i64>,
) -> Result<Json<VersionsBody>, ApiError> {
    let Some(program) = programs::get_program(&state.pool, program_id).await? else {
        return Err(no_such_program());
    };
    let versions = programs::list_versions(&state.pool, program_id).await?;
    Ok(Json(VersionsBody { program, versions }))
}

async fn create_version(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(program_id): Path<i64>,
    Json(content): Json<VersionContent>,
) -> Result<Response, ApiError> {
    match programs::create_version(&state.pool, current.user.id, program_id, &content).await? {
        Ok(id) => {
            let body = serde_json::json!({ "id": id });
            Ok((StatusCode::CREATED, Json(body)).into_response())
        }
        Err(refusal) => Err(author_refusal(refusal)),
    }
}

#[derive(Serialize)]
struct VersionBody {
    summary: VersionSummary,
    content: VersionContent,
}

async fn get_version(
    State(state): State<AppState>,
    _current: CurrentUser,
    Path(version_id): Path<i64>,
) -> Result<Json<VersionBody>, ApiError> {
    let Some(summary) = programs::version_summary(&state.pool, version_id).await? else {
        return Err(no_such_version());
    };
    let Some(content) = programs::load_content(&state.pool, version_id).await? else {
        return Err(no_such_version());
    };
    Ok(Json(VersionBody { summary, content }))
}

async fn replace_content(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(version_id): Path<i64>,
    Json(content): Json<VersionContent>,
) -> Result<Response, ApiError> {
    match programs::replace_draft(&state.pool, current.user.id, version_id, &content).await? {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(refusal) => Err(author_refusal(refusal)),
    }
}

async fn publish_version(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(version_id): Path<i64>,
) -> Result<Response, ApiError> {
    match programs::publish_version(&state.pool, current.user.id, version_id).await? {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(refusal) => Err(publish_refusal(refusal)),
    }
}

async fn discard_version(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(version_id): Path<i64>,
) -> Result<Response, ApiError> {
    match programs::discard_draft(&state.pool, current.user.id, version_id).await? {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(refusal) => Err(author_refusal(refusal)),
    }
}

// -------------------------------------------------------- export/import

/// The export document, delivered as a download so a coordinator can save
/// it directly. The body is exactly the documented format's bytes.
async fn export_version(
    State(state): State<AppState>,
    _current: CurrentUser,
    Path(version_id): Path<i64>,
) -> Result<Response, ApiError> {
    let Some(document) = program_export::export_version(&state.pool, version_id).await? else {
        return Err(no_such_version());
    };
    let disposition = format!("attachment; filename=\"program-version-{version_id}.json\"");
    Ok((
        [
            (header::CONTENT_TYPE, "application/json".to_owned()),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        document,
    )
        .into_response())
}

#[derive(Deserialize)]
struct ImportRequest {
    /// The export document verbatim, as a string, so the documented bytes
    /// reach the importer untouched.
    document: String,
}

async fn import_new_program(
    State(state): State<AppState>,
    current: CurrentUser,
    Json(req): Json<ImportRequest>,
) -> Result<Response, ApiError> {
    match program_export::import_version(
        &state.pool,
        current.user.id,
        &req.document,
        ImportTarget::NewProgram,
    )
    .await?
    {
        Ok(version_id) => imported_body(&state, version_id).await,
        Err(refusal) => Err(import_refusal(refusal)),
    }
}

async fn import_next_version(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(program_id): Path<i64>,
    Json(req): Json<ImportRequest>,
) -> Result<Response, ApiError> {
    match program_export::import_version(
        &state.pool,
        current.user.id,
        &req.document,
        ImportTarget::VersionOf(program_id),
    )
    .await?
    {
        Ok(version_id) => imported_body(&state, version_id).await,
        Err(refusal) => Err(import_refusal(refusal)),
    }
}

/// The created draft's identity, including its program so the interface
/// can navigate to it.
async fn imported_body(state: &AppState, version_id: i64) -> Result<Response, ApiError> {
    let Some(summary) = programs::version_summary(&state.pool, version_id).await? else {
        // The import just created it inside a committed transaction.
        return Err(no_such_version());
    };
    let body = serde_json::json!({ "id": version_id, "program_id": summary.program_id });
    Ok((StatusCode::CREATED, Json(body)).into_response())
}
