//! First-run setup.
//!
//! An uninitialized installation (no agency row) carries a short-lived,
//! hashed setup code. Creating the agency settings and the first
//! administrator is a single transaction that consumes the code; after
//! initialization the setup operation is unavailable (docs/architecture.md).

use anyhow::{Context, Result};
use sqlx::SqlitePool;
use time::OffsetDateTime;

use crate::audit::{self, EventKind};
use crate::capabilities::{self, ADMINISTRATOR_BUNDLE};
use crate::secrets::{self, OpaqueSecret};
use crate::users;

/// Whether the installation has completed first-run setup.
pub async fn is_initialized(pool: &SqlitePool) -> Result<bool> {
    Ok(agency_name(pool).await?.is_some())
}

/// The configured agency name; `None` until first-run setup completes.
pub async fn agency_name(pool: &SqlitePool) -> Result<Option<String>> {
    sqlx::query_scalar("SELECT name FROM agency WHERE id = 1")
        .fetch_optional(pool)
        .await
        .context("checking initialization state")
}

/// Issues a fresh setup code for an uninitialized installation, replacing
/// any previous one. Returns `None` when the installation is already
/// initialized. The raw code is shown once (server log or command output)
/// and only its digest is stored.
pub async fn issue_setup_code(pool: &SqlitePool) -> Result<Option<(OpaqueSecret, i64)>> {
    if is_initialized(pool).await? {
        return Ok(None);
    }
    let code = secrets::generate_one_time_code()?;
    let expires_at = OffsetDateTime::now_utc().unix_timestamp() + users::CODE_TTL_SECONDS;
    sqlx::query(
        "INSERT INTO setup_code (id, code_hash, expires_at) VALUES (1, ?1, ?2)
         ON CONFLICT (id) DO UPDATE SET code_hash = ?1, expires_at = ?2",
    )
    .bind(&code.digest_hex)
    .bind(expires_at)
    .execute(pool)
    .await
    .context("storing setup code")?;
    Ok(Some((code, expires_at)))
}

/// Why an initialization attempt was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum SetupRefusal {
    AlreadyInitialized,
    InvalidCode,
    PasswordPolicy(&'static str),
}

/// Completes first-run setup in one transaction: verifies the code, creates
/// the agency row and the first administrator with the Administrator
/// capability bundle, consumes the code, and records the audit event.
pub async fn initialize(
    pool: &SqlitePool,
    setup_code: &str,
    agency_name: &str,
    username: &str,
    display_name: &str,
    password: &str,
) -> Result<std::result::Result<i64, SetupRefusal>> {
    let agency_name = agency_name.trim();
    let username = username.trim();
    let display_name = {
        let trimmed = display_name.trim();
        if trimmed.is_empty() {
            username
        } else {
            trimmed
        }
    };
    if agency_name.is_empty() || username.is_empty() || username.len() > 64 {
        return Ok(Err(SetupRefusal::InvalidCode));
    }
    if let Err(reason) = secrets::check_password_policy(password, username) {
        return Ok(Err(SetupRefusal::PasswordPolicy(reason)));
    }
    // Hash before opening the transaction: Argon2 work should not hold a
    // write transaction open.
    let password_hash = secrets::hash_password(password)?;
    let now = OffsetDateTime::now_utc().unix_timestamp();

    let mut tx = pool.begin().await?;
    let initialized: Option<i64> = sqlx::query_scalar("SELECT 1 FROM agency WHERE id = 1")
        .fetch_optional(&mut *tx)
        .await?;
    if initialized.is_some() {
        return Ok(Err(SetupRefusal::AlreadyInitialized));
    }
    let valid: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM setup_code WHERE id = 1 AND code_hash = ?1 AND expires_at > ?2",
    )
    .bind(secrets::digest_hex(setup_code))
    .bind(now)
    .fetch_optional(&mut *tx)
    .await?;
    if valid.is_none() {
        return Ok(Err(SetupRefusal::InvalidCode));
    }

    sqlx::query("INSERT INTO agency (id, name, created_at) VALUES (1, ?1, ?2)")
        .bind(agency_name)
        .bind(now)
        .execute(&mut *tx)
        .await
        .context("creating agency settings")?;
    let user_id = users::create(&mut tx, username, display_name, "", "", &password_hash).await?;
    capabilities::grant_bundle(&mut tx, user_id, &ADMINISTRATOR_BUNDLE, None).await?;
    sqlx::query("DELETE FROM setup_code WHERE id = 1")
        .execute(&mut *tx)
        .await
        .context("consuming setup code")?;
    audit::record(
        &mut *tx,
        EventKind::SetupCompleted,
        Some(user_id),
        Some(user_id),
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(user_id))
}
