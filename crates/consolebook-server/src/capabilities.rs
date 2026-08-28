//! Capability-based authorization.
//!
//! Authorization is expressed as capabilities evaluated by domain services
//! (docs/domain-model.md). Roles are convenient bundles applied when grants
//! are created; nothing checks a role name at decision time.

use anyhow::{Context, Result};
use sqlx::{SqliteConnection, SqlitePool};
use time::OffsetDateTime;

/// Capabilities with behavior today. The wider vocabulary from the domain
/// model joins as its features are implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    ManageUsers,
    ManagePrograms,
    AssignTraining,
    ExportRecords,
    AuthorEvaluation,
    ReviewEvaluation,
    ViewAssignedRecords,
}

impl Capability {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ManageUsers => "manage_users",
            Self::ManagePrograms => "manage_programs",
            Self::AssignTraining => "assign_training",
            Self::ExportRecords => "export_records",
            Self::AuthorEvaluation => "author_evaluation",
            Self::ReviewEvaluation => "review_evaluation",
            Self::ViewAssignedRecords => "view_assigned_records",
        }
    }
}

/// The Administrator bundle: what the first (and any later) administrator
/// is granted. A bundle is applied once at grant time; new capabilities
/// added to the product reach existing administrators through explicit
/// migrations, not through re-evaluating a role name.
pub const ADMINISTRATOR_BUNDLE: [Capability; 4] = [
    Capability::ManageUsers,
    Capability::ManagePrograms,
    Capability::AssignTraining,
    Capability::ExportRecords,
];

/// The Trainer bundle: authors evaluations for assigned trainees and reads
/// their assigned trainees' training history.
pub const TRAINER_BUNDLE: [Capability; 2] = [
    Capability::AuthorEvaluation,
    Capability::ViewAssignedRecords,
];

/// The Coordinator bundle: assigns training, reviews evaluations, and
/// reads assigned training history. Broader administration stays explicit
/// authority (PRINCIPLES.md 10).
pub const COORDINATOR_BUNDLE: [Capability; 3] = [
    Capability::AssignTraining,
    Capability::ReviewEvaluation,
    Capability::ViewAssignedRecords,
];

/// Role bundles selectable at user creation. A role is consumed when the
/// grants are created; nothing checks a role name at decision time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleBundle {
    Administrator,
    Coordinator,
    Trainer,
    /// No capabilities today; trainees receive `view_own_records` with the
    /// Milestone 4 trainee timeline.
    #[default]
    Trainee,
}

impl RoleBundle {
    #[must_use]
    pub fn capabilities(self) -> &'static [Capability] {
        match self {
            Self::Administrator => &ADMINISTRATOR_BUNDLE,
            Self::Coordinator => &COORDINATOR_BUNDLE,
            Self::Trainer => &TRAINER_BUNDLE,
            Self::Trainee => &[],
        }
    }
}

/// Grants every capability in a bundle to `user_id`.
pub async fn grant_bundle(
    conn: &mut SqliteConnection,
    user_id: i64,
    bundle: &[Capability],
    granted_by: Option<i64>,
) -> Result<()> {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    for capability in bundle {
        sqlx::query(
            "INSERT INTO capability_grant (user_id, capability, granted_at, granted_by)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(user_id)
        .bind(capability.as_str())
        .bind(now)
        .bind(granted_by)
        .execute(&mut *conn)
        .await
        .context("granting capability")?;
    }
    Ok(())
}

/// Whether `user_id` holds `capability`.
pub async fn user_has(pool: &SqlitePool, user_id: i64, capability: Capability) -> Result<bool> {
    let held: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM capability_grant WHERE user_id = ?1 AND capability = ?2")
            .bind(user_id)
            .bind(capability.as_str())
            .fetch_optional(pool)
            .await
            .context("checking capability")?;
    Ok(held.is_some())
}

/// Every capability held by `user_id`, for session introspection.
pub async fn list_for_user(pool: &SqlitePool, user_id: i64) -> Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT capability FROM capability_grant WHERE user_id = ?1 ORDER BY capability",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .context("listing capabilities")
}
