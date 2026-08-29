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
use crate::session_membership;
use crate::training_sessions::{self, Disposition, SessionInput, SessionRefusal, SessionUpdate};

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
        .route(
            "/api/enrollments/{id}/sessions",
            get(list_sessions).post(create_session),
        )
        .route("/api/sessions/{id}", get(get_session).put(update_session))
        .route("/api/sessions/{id}/close", post(close_session))
        .route("/api/sessions/{id}/trainers", post(add_session_trainer))
        .route(
            "/api/sessions/{id}/trainers/{user_id}",
            axum::routing::delete(remove_session_trainer),
        )
        .route("/api/sessions/mine", get(my_sessions))
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

// ------------------------------------------------------------- sessions

#[allow(clippy::too_many_lines)]
fn session_refusal(refusal: &SessionRefusal) -> ApiError {
    match refusal {
        SessionRefusal::CapabilityRequired => ApiError::new(
            StatusCode::FORBIDDEN,
            "capability_required",
            "session work requires assign_training, session membership, or an assignment with the needed capability",
        ),
        SessionRefusal::NoSuchEnrollment => ApiError::new(
            StatusCode::NOT_FOUND,
            "no_such_enrollment",
            "no such enrollment",
        ),
        SessionRefusal::EnrollmentInactive => ApiError::new(
            StatusCode::CONFLICT,
            "enrollment_inactive",
            "sessions are created on active enrollments",
        ),
        SessionRefusal::NoSuchSession => ApiError::new(
            StatusCode::NOT_FOUND,
            "no_such_session",
            "no such training session",
        ),
        SessionRefusal::SessionClosed => ApiError::new(
            StatusCode::CONFLICT,
            "session_closed",
            "the session already carries a disposition",
        ),
        SessionRefusal::InvalidBusinessDate => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_business_date",
            "the business date is not a real calendar date (YYYY-MM-DD)",
        ),
        SessionRefusal::UnknownTimezone => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "unknown_timezone",
            "the timezone is not an IANA timezone name",
        ),
        SessionRefusal::InvalidLocalTime => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_local_time",
            "a local time failed to parse or resolve",
        ),
        SessionRefusal::EndBeforeStart => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "end_before_start",
            "the end instant cannot precede the start instant",
        ),
        SessionRefusal::EndRequired => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "end_required",
            "completed and interrupted sessions carry an end time",
        ),
        SessionRefusal::EndNotAllowed => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "end_not_allowed",
            "cancelled sessions carry no end time",
        ),
        SessionRefusal::DispositionRequired => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "disposition_required",
            "an end time at creation needs a completed or interrupted disposition",
        ),
        SessionRefusal::InvalidDisposition => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_disposition",
            "sessions are cancelled after creation, never created cancelled",
        ),
        SessionRefusal::Overlap => ApiError::new(
            StatusCode::CONFLICT,
            "interval_overlap",
            "active training intervals for one trainee cannot overlap",
        ),
        SessionRefusal::NoSuchPhase => ApiError::new(
            StatusCode::NOT_FOUND,
            "no_such_phase",
            "no such phase in the pinned program version",
        ),
        SessionRefusal::NoSuchUser => ApiError::new(
            StatusCode::NOT_FOUND,
            "no_such_user",
            "no user with that id",
        ),
        SessionRefusal::TrainerLacksCapability => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "trainer_lacks_capability",
            "session trainers need the author_evaluation capability",
        ),
        SessionRefusal::AlreadyMember => ApiError::new(
            StatusCode::CONFLICT,
            "already_member",
            "that trainer is already on this session",
        ),
        SessionRefusal::NotMember => ApiError::new(
            StatusCode::NOT_FOUND,
            "not_a_member",
            "that trainer is not on this session",
        ),
        SessionRefusal::LastTrainer => ApiError::new(
            StatusCode::CONFLICT,
            "last_trainer",
            "a training session keeps at least one trainer",
        ),
        SessionRefusal::NoTrainers => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "no_trainers",
            "a training session needs at least one trainer",
        ),
    }
}

#[derive(Serialize)]
struct SessionsBody {
    sessions: Vec<training_sessions::SessionRow>,
}

async fn list_sessions(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(enrollment_id): Path<i64>,
) -> Result<Response, ApiError> {
    match training_sessions::list_for_enrollment(&state.pool, current.user.id, enrollment_id)
        .await?
    {
        Ok(sessions) => Ok(Json(SessionsBody { sessions }).into_response()),
        Err(refusal) => Err(session_refusal(&refusal)),
    }
}

async fn create_session(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(enrollment_id): Path<i64>,
    Json(input): Json<SessionInput>,
) -> Result<Response, ApiError> {
    match training_sessions::create(&state.pool, current.user.id, enrollment_id, &input).await? {
        Ok(id) => {
            let body = serde_json::json!({ "id": id });
            Ok((StatusCode::CREATED, Json(body)).into_response())
        }
        Err(refusal) => Err(session_refusal(&refusal)),
    }
}

async fn get_session(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(session_id): Path<i64>,
) -> Result<Response, ApiError> {
    match training_sessions::get(&state.pool, current.user.id, session_id).await? {
        Ok(detail) => Ok(Json(detail).into_response()),
        Err(refusal) => Err(session_refusal(&refusal)),
    }
}

async fn update_session(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(session_id): Path<i64>,
    Json(update): Json<SessionUpdate>,
) -> Result<Response, ApiError> {
    match training_sessions::update_open(&state.pool, current.user.id, session_id, &update).await? {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(refusal) => Err(session_refusal(&refusal)),
    }
}

#[derive(Deserialize)]
struct CloseSessionRequest {
    disposition: Disposition,
    #[serde(default)]
    local_end: Option<String>,
}

async fn close_session(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(session_id): Path<i64>,
    Json(req): Json<CloseSessionRequest>,
) -> Result<Response, ApiError> {
    match training_sessions::close(
        &state.pool,
        current.user.id,
        session_id,
        req.disposition,
        req.local_end.as_deref(),
    )
    .await?
    {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(refusal) => Err(session_refusal(&refusal)),
    }
}

#[derive(Deserialize)]
struct AddSessionTrainerRequest {
    trainer_user_id: i64,
}

async fn add_session_trainer(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(session_id): Path<i64>,
    Json(req): Json<AddSessionTrainerRequest>,
) -> Result<Response, ApiError> {
    match session_membership::add_trainer(
        &state.pool,
        current.user.id,
        session_id,
        req.trainer_user_id,
    )
    .await?
    {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(refusal) => Err(session_refusal(&refusal)),
    }
}

async fn remove_session_trainer(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((session_id, user_id)): Path<(i64, i64)>,
) -> Result<Response, ApiError> {
    match session_membership::remove_trainer(&state.pool, current.user.id, session_id, user_id)
        .await?
    {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(refusal) => Err(session_refusal(&refusal)),
    }
}

#[derive(Serialize)]
struct MySessionsBody {
    sessions: Vec<training_sessions::MySession>,
}

/// The caller's own sessions — the trainer's working list.
async fn my_sessions(
    State(state): State<AppState>,
    current: CurrentUser,
) -> Result<Json<MySessionsBody>, ApiError> {
    match training_sessions::list_mine(&state.pool, current.user.id).await? {
        Ok(sessions) => Ok(Json(MySessionsBody { sessions })),
        Err(refusal) => Err(session_refusal(&refusal)),
    }
}
