//! Evaluation draft HTTP handlers (Milestone 3 slice 3).
//!
//! `http.rs` remains the hub; this module owns the draft endpoints.
//! Gates live in the domain services; handlers translate refusal enums
//! into stable error codes and never restate policy.

use anyhow::Context;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post, put};
use serde::{Deserialize, Serialize};

use crate::draft_content::{self, DraftContent, FormSkeleton};
use crate::evaluation_drafts::{self, DraftDetail, DraftRefusal};
use crate::http::{ApiError, AppState, CurrentUser};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/sessions/{id}/draft", post(create_draft))
        .route("/api/sessions/{id}/daily-forms", get(list_daily_forms))
        .route("/api/drafts/{id}", get(get_draft))
        .route("/api/drafts/{id}/content", put(save_content))
        .route("/api/drafts/{id}/transfer", post(transfer_draft))
        .route("/api/drafts/{id}/submit", post(submit_draft))
}

// ------------------------------------------------------ refusal mapping

#[allow(clippy::too_many_lines)]
fn draft_refusal(refusal: &DraftRefusal) -> ApiError {
    match refusal {
        DraftRefusal::CapabilityRequired => ApiError::new(
            StatusCode::FORBIDDEN,
            "capability_required",
            "this requires the assign_training capability, or author_evaluation within the record's scope",
        ),
        DraftRefusal::NoSuchSession => ApiError::new(
            StatusCode::NOT_FOUND,
            "no_such_session",
            "no such training session",
        ),
        DraftRefusal::SessionCancelled => ApiError::new(
            StatusCode::CONFLICT,
            "session_cancelled",
            "a cancelled session never happened and takes no draft",
        ),
        DraftRefusal::DraftAlreadyExists => ApiError::new(
            StatusCode::CONFLICT,
            "draft_already_exists",
            "this session already has its daily draft",
        ),
        DraftRefusal::NoDailyForm => ApiError::new(
            StatusCode::CONFLICT,
            "no_daily_form",
            "the pinned version defines no daily report form",
        ),
        DraftRefusal::FormRequired => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "form_required",
            "the pinned version defines several daily report forms; name one",
        ),
        DraftRefusal::NoSuchForm => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "no_such_form",
            "the named form is not a daily report form of the pinned version",
        ),
        DraftRefusal::NoSuchRecord => {
            ApiError::new(StatusCode::NOT_FOUND, "no_such_record", "no such draft")
        }
        DraftRefusal::DraftSubmitted => ApiError::new(
            StatusCode::CONFLICT,
            "draft_submitted",
            "the draft is submitted for review and frozen",
        ),
        DraftRefusal::NoSuchUser => {
            ApiError::new(StatusCode::NOT_FOUND, "no_such_user", "no such user")
        }
        DraftRefusal::NotEligible => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "not_eligible",
            "recipients author evaluations within the record's scope",
        ),
        DraftRefusal::AlreadyOwner => ApiError::new(
            StatusCode::CONFLICT,
            "already_owner",
            "that user already owns the draft",
        ),
        DraftRefusal::NoSuchFormCompetency => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "no_such_form_competency",
            "a rated competency is not on the pinned form",
        ),
        DraftRefusal::NoSuchFormNarrative => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "no_such_form_narrative",
            "a narrative is not on the pinned form",
        ),
        DraftRefusal::NoSuchModifier => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "no_such_modifier",
            "a modifier is not in the pinned vocabulary",
        ),
        DraftRefusal::ValueOutOfRange => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "value_out_of_range",
            "a rating value violates its pinned scale",
        ),
        DraftRefusal::ValueNotAllowed => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "value_not_allowed",
            "narrative-only scales take no value",
        ),
        DraftRefusal::DuplicateEntry => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "duplicate_entry",
            "a competency or narrative appears twice in one save",
        ),
        DraftRefusal::StaleSave => ApiError::new(
            StatusCode::CONFLICT,
            "stale_save",
            "another contributor saved first; reload the draft and reapply",
        ),
    }
}

// ------------------------------------------------------------- handlers

#[derive(Deserialize)]
struct CreateDraftRequest {
    #[serde(default)]
    evaluation_form_id: Option<i64>,
}

#[derive(Serialize)]
struct CreatedBody {
    id: i64,
}

async fn create_draft(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(session_id): Path<i64>,
    Json(req): Json<CreateDraftRequest>,
) -> Result<Response, ApiError> {
    match evaluation_drafts::create(
        &state.pool,
        current.user.id,
        session_id,
        req.evaluation_form_id,
    )
    .await?
    {
        Ok(id) => Ok((StatusCode::CREATED, Json(CreatedBody { id })).into_response()),
        Err(refusal) => Err(draft_refusal(&refusal)),
    }
}

#[derive(Serialize)]
struct DraftView {
    #[serde(flatten)]
    detail: DraftDetail,
    form: FormSkeleton,
    content: DraftContent,
}

async fn get_draft(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(record_id): Path<i64>,
) -> Result<Response, ApiError> {
    let detail = match evaluation_drafts::detail(&state.pool, current.user.id, record_id).await? {
        Ok(detail) => detail,
        Err(refusal) => return Err(draft_refusal(&refusal)),
    };
    let mut conn = state.pool.acquire().await.context("acquiring connection")?;
    let form = draft_content::skeleton(
        &mut conn,
        detail.program_version_id,
        detail.evaluation_form_id,
    )
    .await?;
    let content = draft_content::content(&mut conn, record_id).await?;
    Ok(Json(DraftView {
        detail,
        form,
        content,
    })
    .into_response())
}

#[derive(Deserialize)]
struct SaveContentRequest {
    /// The revision this save was based on.
    revision: i64,
    #[serde(flatten)]
    content: DraftContent,
}

#[derive(Serialize)]
struct SavedBody {
    revision: i64,
}

async fn save_content(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(record_id): Path<i64>,
    Json(req): Json<SaveContentRequest>,
) -> Result<Response, ApiError> {
    match draft_content::save(
        &state.pool,
        current.user.id,
        record_id,
        req.revision,
        &req.content,
    )
    .await?
    {
        Ok(revision) => Ok(Json(SavedBody { revision }).into_response()),
        Err(refusal) => Err(draft_refusal(&refusal)),
    }
}

#[derive(Serialize)]
struct DailyFormsBody {
    forms: Vec<evaluation_drafts::DailyForm>,
}

async fn list_daily_forms(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(session_id): Path<i64>,
) -> Result<Response, ApiError> {
    match evaluation_drafts::list_daily_forms(&state.pool, current.user.id, session_id).await? {
        Ok(forms) => Ok(Json(DailyFormsBody { forms }).into_response()),
        Err(refusal) => Err(draft_refusal(&refusal)),
    }
}

#[derive(Deserialize)]
struct TransferRequest {
    to_user_id: i64,
}

async fn transfer_draft(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(record_id): Path<i64>,
    Json(req): Json<TransferRequest>,
) -> Result<Response, ApiError> {
    match evaluation_drafts::transfer(&state.pool, current.user.id, record_id, req.to_user_id)
        .await?
    {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(refusal) => Err(draft_refusal(&refusal)),
    }
}

#[derive(Deserialize)]
struct SubmitRequest {
    /// The revision the submitter viewed.
    revision: i64,
}

async fn submit_draft(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(record_id): Path<i64>,
    Json(req): Json<SubmitRequest>,
) -> Result<Response, ApiError> {
    match evaluation_drafts::submit(&state.pool, current.user.id, record_id, req.revision).await? {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(refusal) => Err(draft_refusal(&refusal)),
    }
}
