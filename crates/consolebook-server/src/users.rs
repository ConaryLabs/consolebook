//! User accounts and password reset.
//!
//! Usernames are unique case-insensitively; the stored spelling is what the
//! user typed. Password reset codes are short-lived, single-use, stored
//! hashed, and using one revokes every session for the account.

use anyhow::{Context, Result};
use sqlx::{Row, SqliteConnection, SqlitePool};
use time::OffsetDateTime;

use crate::audit::{self, EventKind};
use crate::capabilities::{self, Capability};
use crate::secrets::{self, OpaqueSecret};
use crate::sessions;

/// Lifetime of setup and password-reset codes.
pub const CODE_TTL_SECONDS: i64 = 15 * 60;

#[derive(Debug, Clone)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub password_hash: String,
}

fn user_from_row(row: &sqlx::sqlite::SqliteRow) -> User {
    User {
        id: row.get("id"),
        username: row.get("username"),
        display_name: row.get("display_name"),
        password_hash: row.get("password_hash"),
    }
}

/// Looks a user up by username, case-insensitively.
pub async fn find_by_username(pool: &SqlitePool, username: &str) -> Result<Option<User>> {
    let row = sqlx::query(
        "SELECT id, username, display_name, password_hash
         FROM user WHERE username = ?1 COLLATE NOCASE",
    )
    .bind(username)
    .fetch_optional(pool)
    .await
    .context("looking up user")?;
    Ok(row.as_ref().map(user_from_row))
}

pub async fn find_by_id(pool: &SqlitePool, id: i64) -> Result<Option<User>> {
    let row =
        sqlx::query("SELECT id, username, display_name, password_hash FROM user WHERE id = ?1")
            .bind(id)
            .fetch_optional(pool)
            .await
            .context("looking up user")?;
    Ok(row.as_ref().map(user_from_row))
}

/// Creates a user inside a caller-owned transaction. The caller is
/// responsible for policy checks and capability grants.
pub async fn create(
    conn: &mut SqliteConnection,
    username: &str,
    display_name: &str,
    password_hash: &str,
) -> Result<i64> {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let result = sqlx::query(
        "INSERT INTO user (username, display_name, password_hash, created_at)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(username)
    .bind(display_name)
    .bind(password_hash)
    .bind(now)
    .execute(conn)
    .await
    .context("creating user")?;
    Ok(result.last_insert_rowid())
}

/// A user as listed for rosters; never carries the password hash.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UserSummary {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub created_at: i64,
}

/// Every user, ordered by display name. Capability checks are the
/// caller's responsibility.
pub async fn list(pool: &SqlitePool) -> Result<Vec<UserSummary>> {
    let rows = sqlx::query(
        "SELECT id, username, display_name, created_at FROM user
         ORDER BY display_name COLLATE NOCASE, username COLLATE NOCASE",
    )
    .fetch_all(pool)
    .await
    .context("listing users")?;
    Ok(rows
        .iter()
        .map(|row| UserSummary {
            id: row.get("id"),
            username: row.get("username"),
            display_name: row.get("display_name"),
            created_at: row.get("created_at"),
        })
        .collect())
}

/// Why creating a user was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum CreateUserRefusal {
    UsernameInvalid(&'static str),
    UsernameTaken,
}

/// A newly created user and the one-time reset code that lets them set
/// their first password through the standard reset flow.
#[derive(Debug)]
pub struct CreatedUser {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub reset_code: OpaqueSecret,
    pub reset_expires_at: i64,
}

/// Creates a user with no capability grants and no usable password — the
/// stored credential is the hash of a random secret nobody sees — then
/// issues a standard administrator-origin reset code so the person's first
/// sign-in goes through the existing reset flow. Full user administration
/// is a later milestone; this exists so training can be assigned.
pub async fn create_with_reset_code(
    pool: &SqlitePool,
    actor_user_id: i64,
    username: &str,
    display_name: &str,
) -> Result<std::result::Result<CreatedUser, CreateUserRefusal>> {
    let username = username.trim();
    let display_name = {
        let trimmed = display_name.trim();
        if trimmed.is_empty() {
            username
        } else {
            trimmed
        }
    };
    if username.is_empty() {
        return Ok(Err(CreateUserRefusal::UsernameInvalid(
            "a username is required",
        )));
    }
    if username.len() > 64 {
        return Ok(Err(CreateUserRefusal::UsernameInvalid(
            "usernames are at most 64 characters",
        )));
    }
    if find_by_username(pool, username).await?.is_some() {
        return Ok(Err(CreateUserRefusal::UsernameTaken));
    }
    // Hash outside the transaction: Argon2 work should not hold a write
    // transaction open. The plain value is dropped unrecorded.
    let unusable = secrets::generate_one_time_code()?;
    let password_hash = secrets::hash_password(&unusable.raw)?;

    let mut tx = pool.begin().await?;
    let user_id = create(&mut tx, username, display_name, &password_hash).await?;
    audit::record(
        &mut *tx,
        EventKind::UserCreated,
        Some(actor_user_id),
        Some(user_id),
    )
    .await?;
    tx.commit().await?;

    let issued = issue_reset_code(
        pool,
        username,
        ResetOrigin::Administrator {
            issued_by: actor_user_id,
        },
    )
    .await?
    .map_err(|refusal| {
        anyhow::anyhow!("issuing the initial reset code was refused: {refusal:?}")
    })?;
    Ok(Ok(CreatedUser {
        id: user_id,
        username: issued.user.username,
        display_name: issued.user.display_name,
        reset_code: issued.code,
        reset_expires_at: issued.expires_at,
    }))
}

/// How a reset code came to exist; recorded on the code and in audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetOrigin {
    /// Issued by a user holding `manage_users` through the API.
    Administrator { issued_by: i64 },
    /// Issued by the `recover` command with OS access to the data directory.
    Recovery,
}

/// Why a reset code was not issued.
#[derive(Debug, PartialEq, Eq)]
pub enum IssueRefusal {
    NoSuchUser,
    /// Recovery target does not hold `manage_users`.
    NotAnAdministrator,
}

/// A successfully issued reset code.
#[derive(Debug)]
pub struct IssuedResetCode {
    pub user: User,
    pub code: OpaqueSecret,
    pub expires_at: i64,
}

/// Issues a short-lived, single-use password reset code for `username`.
///
/// For [`ResetOrigin::Recovery`], the target must hold `manage_users`: the
/// recovery command exists to rescue a locked-out administrator, not to
/// bypass administrator-operated reset for everyone else.
pub async fn issue_reset_code(
    pool: &SqlitePool,
    username: &str,
    origin: ResetOrigin,
) -> Result<std::result::Result<IssuedResetCode, IssueRefusal>> {
    let Some(user) = find_by_username(pool, username).await? else {
        return Ok(Err(IssueRefusal::NoSuchUser));
    };
    if matches!(origin, ResetOrigin::Recovery)
        && !capabilities::user_has(pool, user.id, Capability::ManageUsers).await?
    {
        return Ok(Err(IssueRefusal::NotAnAdministrator));
    }

    let code = secrets::generate_one_time_code()?;
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let expires_at = now + CODE_TTL_SECONDS;
    let (issued_via, issued_by, event) = match origin {
        ResetOrigin::Administrator { issued_by } => {
            ("administrator", Some(issued_by), EventKind::ResetCodeIssued)
        }
        ResetOrigin::Recovery => ("recovery", None, EventKind::RecoveryCodeIssued),
    };

    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO password_reset_code
         (user_id, code_hash, issued_via, issued_by, issued_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(user.id)
    .bind(&code.digest_hex)
    .bind(issued_via)
    .bind(issued_by)
    .bind(now)
    .bind(expires_at)
    .execute(&mut *tx)
    .await
    .context("storing reset code")?;
    audit::record(&mut *tx, event, issued_by, Some(user.id)).await?;
    tx.commit().await?;

    Ok(Ok(IssuedResetCode {
        user,
        code,
        expires_at,
    }))
}

/// Outcome of a reset attempt. `Invalid` deliberately does not reveal
/// which check failed (unknown user, wrong code, expired, already used).
#[derive(Debug, PartialEq, Eq)]
pub enum ResetOutcome {
    Done,
    Invalid,
    PasswordPolicy(&'static str),
}

/// Consumes a reset code: verifies it, sets the new password, marks the
/// code used, revokes every session for the account, and records the audit
/// event — all in one transaction.
pub async fn use_reset_code(
    pool: &SqlitePool,
    username: &str,
    code: &str,
    new_password: &str,
) -> Result<ResetOutcome> {
    if let Err(reason) = secrets::check_password_policy(new_password, username) {
        return Ok(ResetOutcome::PasswordPolicy(reason));
    }
    let Some(user) = find_by_username(pool, username).await? else {
        return Ok(ResetOutcome::Invalid);
    };
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let digest = secrets::digest_hex(code);
    // Hash outside the transaction: Argon2 work should not hold a write
    // transaction open.
    let password_hash = secrets::hash_password(new_password)?;

    let mut tx = pool.begin().await?;
    let code_id: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM password_reset_code
         WHERE user_id = ?1 AND code_hash = ?2 AND used_at IS NULL AND expires_at > ?3",
    )
    .bind(user.id)
    .bind(&digest)
    .bind(now)
    .fetch_optional(&mut *tx)
    .await
    .context("looking up reset code")?;
    let Some(code_id) = code_id else {
        return Ok(ResetOutcome::Invalid);
    };
    sqlx::query("UPDATE password_reset_code SET used_at = ?1 WHERE id = ?2")
        .bind(now)
        .bind(code_id)
        .execute(&mut *tx)
        .await
        .context("marking reset code used")?;
    sqlx::query("UPDATE user SET password_hash = ?1 WHERE id = ?2")
        .bind(&password_hash)
        .bind(user.id)
        .execute(&mut *tx)
        .await
        .context("updating password")?;
    sessions::revoke_all_for_user(&mut tx, user.id).await?;
    audit::record(
        &mut *tx,
        EventKind::ResetCodeUsed,
        Some(user.id),
        Some(user.id),
    )
    .await?;
    tx.commit().await?;
    Ok(ResetOutcome::Done)
}
