//! Training lifecycle HTTP handlers (Milestone 3 slice 1).
//!
//! `http.rs` remains the hub; this module owns the enrollment-detail,
//! lifecycle-event, phase-event, and assignment endpoints. Reads are gated
//! by capability plus assignment scope inside the domain services;
//! handlers translate refusal enums into stable error codes and never
//! restate policy.

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};

use crate::assignments::{self, AssignRefusal};
use crate::capabilities::{self, Capability};
use crate::http::{ApiError, AppState, CurrentUser};
use crate::lifecycle::{self, EnrollmentEventKind, LifecycleRefusal, PhaseEventKind};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/enrollments/{id}", get(enrollment_detail))
        .route(
            "/api/enrollments/{id}/events",
            post(record_enrollment_event),
        )
        .route(
            "/api/enrollments/{id}/phase-events",
            post(record_phase_event),
        )
        .route("/api/enrollments/{id}/assignments", post(create_assignment))
        .route("/api/assignments/{id}/end", post(end_assignment))
        .route("/api/assignments/mine", get(my_assignments))
}

// ------------------------------------------------------ refusal mapping

fn lifecycle_refusal(refusal: &LifecycleRefusal) -> ApiError {
    match refusal {
        LifecycleRefusal::CapabilityRequired => ApiError::new(
            StatusCode::FORBIDDEN,
            "capability_required",
            "this requires the assign_training capability, or an active assignment with view_assigned_records for reads",
        ),
        LifecycleRefusal::NoSuchEnrollment => ApiError::new(
            StatusCode::NOT_FOUND,
            "no_such_enrollment",
            "no such enrollment",
        ),
        LifecycleRefusal::NotActive => ApiError::new(
            StatusCode::CONFLICT,
            "enrollment_inactive",
            "the enrollment is withdrawn or completed",
        ),
        LifecycleRefusal::AlreadyActive => ApiError::new(
            StatusCode::CONFLICT,
            "already_active",
            "the enrollment is already active",
        ),
        LifecycleRefusal::ReasonRequired => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "reason_required",
            "this event requires a reason",
        ),
        LifecycleRefusal::NoSuchVersion => ApiError::new(
            StatusCode::NOT_FOUND,
            "no_such_version",
            "no such program version",
        ),
        LifecycleRefusal::NotPublished => ApiError::new(
            StatusCode::CONFLICT,
            "not_published",
            "enrollments pin published program versions",
        ),
        LifecycleRefusal::SameVersion => ApiError::new(
            StatusCode::CONFLICT,
            "same_version",
            "the enrollment already pins that version",
        ),
        LifecycleRefusal::DifferentProgram => ApiError::new(
            StatusCode::CONFLICT,
            "different_program",
            "a version change stays within the enrollment's program; changing programs is a new enrollment",
        ),
        LifecycleRefusal::TargetAlreadyEnrolled => ApiError::new(
            StatusCode::CONFLICT,
            "already_enrolled",
            "the trainee already has an enrollment pinning that version",
        ),
        LifecycleRefusal::NoSuchPhase => ApiError::new(
            StatusCode::NOT_FOUND,
            "no_such_phase",
            "no such phase in the pinned program version",
        ),
        LifecycleRefusal::TransitionNotAllowed => ApiError::new(
            StatusCode::CONFLICT,
            "transition_not_allowed",
            "the pinned version's transition graph has no matching edge",
        ),
        LifecycleRefusal::NoCurrentPhase => ApiError::new(
            StatusCode::CONFLICT,
            "no_current_phase",
            "the trainee has not entered a phase of the pinned version",
        ),
        LifecycleRefusal::AlreadyPaused => ApiError::new(
            StatusCode::CONFLICT,
            "already_paused",
            "training is already paused",
        ),
        LifecycleRefusal::NotPaused => {
            ApiError::new(StatusCode::CONFLICT, "not_paused", "training is not paused")
        }
        LifecycleRefusal::Paused => ApiError::new(
            StatusCode::CONFLICT,
            "paused",
            "training is paused; resume before recording phase changes",
        ),
        LifecycleRefusal::OutOfOrder => ApiError::new(
            StatusCode::CONFLICT,
            "out_of_order",
            "phase events append in effective order; this instant precedes an already-recorded event",
        ),
        LifecycleRefusal::EffectiveInFuture => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "effective_in_future",
            "the effective instant cannot be in the future",
        ),
    }
}

fn assign_refusal(refusal: &AssignRefusal) -> ApiError {
    match refusal {
        AssignRefusal::CapabilityRequired => ApiError::new(
            StatusCode::FORBIDDEN,
            "capability_required",
            "managing assignments requires the assign_training capability",
        ),
        AssignRefusal::NoSuchEnrollment => ApiError::new(
            StatusCode::NOT_FOUND,
            "no_such_enrollment",
            "no such enrollment",
        ),
        AssignRefusal::EnrollmentInactive => ApiError::new(
            StatusCode::CONFLICT,
            "enrollment_inactive",
            "assignments attach to active enrollments",
        ),
        AssignRefusal::NoSuchUser => ApiError::new(
            StatusCode::NOT_FOUND,
            "no_such_user",
            "no user with that id",
        ),
        AssignRefusal::TrainerLacksCapability => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "trainer_lacks_capability",
            "the assigned trainer needs the view_assigned_records capability",
        ),
        AssignRefusal::AlreadyAssigned => ApiError::new(
            StatusCode::CONFLICT,
            "already_assigned",
            "that trainer already holds an active assignment on this enrollment",
        ),
        AssignRefusal::NoSuchAssignment => ApiError::new(
            StatusCode::NOT_FOUND,
            "no_such_assignment",
            "no such assignment",
        ),
        AssignRefusal::AlreadyEnded => ApiError::new(
            StatusCode::CONFLICT,
            "already_ended",
            "that assignment is already ended",
        ),
    }
}

// ----------------------------------------------------------- enrollment

async fn enrollment_detail(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(enrollment_id): Path<i64>,
) -> Result<Response, ApiError> {
    match lifecycle::enrollment_detail(&state.pool, current.user.id, enrollment_id).await? {
        Ok(detail) => Ok(Json(detail).into_response()),
        Err(refusal) => Err(lifecycle_refusal(&refusal)),
    }
}

#[derive(Deserialize)]
struct EnrollmentEventRequest {
    kind: EnrollmentEventKind,
    #[serde(default)]
    reason: String,
    to_version_id: Option<i64>,
}

async fn record_enrollment_event(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(enrollment_id): Path<i64>,
    Json(req): Json<EnrollmentEventRequest>,
) -> Result<Response, ApiError> {
    match lifecycle::record_enrollment_event(
        &state.pool,
        current.user.id,
        enrollment_id,
        req.kind,
        &req.reason,
        req.to_version_id,
    )
    .await?
    {
        Ok(id) => {
            let body = serde_json::json!({ "id": id });
            Ok((StatusCode::CREATED, Json(body)).into_response())
        }
        Err(refusal) => Err(lifecycle_refusal(&refusal)),
    }
}

#[derive(Deserialize)]
struct PhaseEventRequest {
    kind: PhaseEventKind,
    to_phase_id: Option<i64>,
    effective_at: Option<i64>,
    #[serde(default)]
    reason: String,
}

async fn record_phase_event(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(enrollment_id): Path<i64>,
    Json(req): Json<PhaseEventRequest>,
) -> Result<Response, ApiError> {
    match lifecycle::record_phase_event(
        &state.pool,
        current.user.id,
        enrollment_id,
        req.kind,
        req.to_phase_id,
        req.effective_at,
        &req.reason,
    )
    .await?
    {
        Ok(id) => {
            let body = serde_json::json!({ "id": id });
            Ok((StatusCode::CREATED, Json(body)).into_response())
        }
        Err(refusal) => Err(lifecycle_refusal(&refusal)),
    }
}

// ---------------------------------------------------------- assignments

#[derive(Deserialize)]
struct AssignRequest {
    trainer_user_id: i64,
}

async fn create_assignment(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(enrollment_id): Path<i64>,
    Json(req): Json<AssignRequest>,
) -> Result<Response, ApiError> {
    match assignments::create(
        &state.pool,
        current.user.id,
        enrollment_id,
        req.trainer_user_id,
    )
    .await?
    {
        Ok(id) => {
            let body = serde_json::json!({ "id": id });
            Ok((StatusCode::CREATED, Json(body)).into_response())
        }
        Err(refusal) => Err(assign_refusal(&refusal)),
    }
}

async fn end_assignment(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(assignment_id): Path<i64>,
) -> Result<Response, ApiError> {
    match assignments::end(&state.pool, current.user.id, assignment_id).await? {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(refusal) => Err(assign_refusal(&refusal)),
    }
}

#[derive(Serialize)]
struct MyAssignmentsBody {
    assignments: Vec<assignments::AssignedTrainee>,
}

/// The caller's own active assignments — the trainer's "my trainees"
/// view. Being assigned grants nothing by itself: reading trainee
/// identities takes `view_assigned_records`, the same capability the
/// enrollment detail requires (PRINCIPLES.md 10).
async fn my_assignments(
    State(state): State<AppState>,
    current: CurrentUser,
) -> Result<Json<MyAssignmentsBody>, ApiError> {
    if !capabilities::user_has(
        &state.pool,
        current.user.id,
        Capability::ViewAssignedRecords,
    )
    .await?
    {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "capability_required",
            "listing your assignments requires the view_assigned_records capability",
        ));
    }
    let assignments = assignments::list_for_trainer(&state.pool, current.user.id).await?;
    Ok(Json(MyAssignmentsBody { assignments }))
}
