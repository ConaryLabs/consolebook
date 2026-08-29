//! `SQLite` storage with explicit, verified connection invariants.
//!
//! `docs/architecture.md` requires every connection to come from one explicit
//! options object that enables and verifies foreign-key enforcement, WAL
//! journaling, an intentional synchronous mode, a bounded busy timeout, and
//! application-owned migrations. Startup fails closed if any invariant does
//! not hold; `doctor` reports the same checks without failing the process.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use sqlx::Row;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

/// Busy timeout applied to every connection.
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Begins an immediate (write) transaction: the write lock is taken up
/// front, so a check-then-write path validates against the committed
/// state and a concurrent writer waits out the busy timeout instead of
/// failing its read snapshot mid-transaction — typed refusals stay typed
/// under concurrency. New write paths use this; #27 tracks retrofitting
/// the earlier deferred ones.
pub async fn write_tx(pool: &SqlitePool) -> sqlx::Result<sqlx::Transaction<'static, sqlx::Sqlite>> {
    pool.begin_with("BEGIN IMMEDIATE").await
}

/// Ends a write transaction on a refusal path with the rollback awaited,
/// so the write lock never outlives the decision. A dropped transaction
/// only queues its rollback on the connection's worker thread; until that
/// runs, the lock lingers — and a deferred writer elsewhere meets it as
/// an immediate `SQLITE_BUSY`, because `SQLite` does not consult the busy
/// timeout when promoting an open read transaction to a write (#27 tracks
/// converting those deferred paths themselves).
pub async fn refuse<T, E>(
    tx: sqlx::Transaction<'static, sqlx::Sqlite>,
    refusal: E,
) -> Result<std::result::Result<T, E>> {
    tx.rollback().await.context("rolling back refused write")?;
    Ok(Err(refusal))
}

/// Embedded, application-owned migrations.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// One verified connection invariant, for `doctor` output and startup checks.
#[derive(Debug, Clone)]
pub struct InvariantCheck {
    pub name: &'static str,
    pub expected: String,
    pub actual: String,
}

impl InvariantCheck {
    #[must_use]
    pub fn holds(&self) -> bool {
        self.expected.eq_ignore_ascii_case(&self.actual)
    }
}

/// The single options object every connection is created from.
///
/// `create_if_missing` is only enabled by [`open`]; diagnostic paths use
/// [`open_existing`] so `doctor` never creates a database as a side effect.
fn connect_options(db_path: &Path, create: bool) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(create)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(BUSY_TIMEOUT)
}

async fn connect(db_path: &Path, create: bool) -> Result<SqlitePool> {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(connect_options(db_path, create))
        .await
        .with_context(|| format!("opening database {}", db_path.display()))?;
    Ok(pool)
}

/// Opens (creating if necessary) the database, runs migrations, verifies the
/// connection invariants, and ensures the instance identity row exists.
pub async fn open(db_path: &Path) -> Result<SqlitePool> {
    let pool = connect(db_path, true).await?;

    MIGRATOR
        .run(&pool)
        .await
        .context("running database migrations")?;

    let failed: Vec<InvariantCheck> = verify_invariants(&pool)
        .await?
        .into_iter()
        .filter(|check| !check.holds())
        .collect();
    if !failed.is_empty() {
        let summary: Vec<String> = failed
            .iter()
            .map(|c| format!("{} (expected {}, got {})", c.name, c.expected, c.actual))
            .collect();
        bail!("database invariants violated: {}", summary.join(", "));
    }

    ensure_instance_identity(&pool).await?;
    Ok(pool)
}

/// Opens an existing database without creating one and without migrating.
/// Used by diagnostics and backup so they never mutate schema state.
pub async fn open_existing(db_path: &Path) -> Result<SqlitePool> {
    if !db_path.exists() {
        bail!("database {} does not exist", db_path.display());
    }
    connect(db_path, false).await
}

/// Reads back the PRAGMA state the options object is supposed to guarantee.
pub async fn verify_invariants(pool: &SqlitePool) -> Result<Vec<InvariantCheck>> {
    let foreign_keys: i64 = sqlx::query("PRAGMA foreign_keys")
        .fetch_one(pool)
        .await?
        .get(0);
    let journal_mode: String = sqlx::query("PRAGMA journal_mode")
        .fetch_one(pool)
        .await?
        .get(0);
    // 1 = NORMAL. SQLite reports synchronous numerically.
    let synchronous: i64 = sqlx::query("PRAGMA synchronous")
        .fetch_one(pool)
        .await?
        .get(0);
    let busy_timeout_ms: i64 = sqlx::query("PRAGMA busy_timeout")
        .fetch_one(pool)
        .await?
        .get(0);

    Ok(vec![
        InvariantCheck {
            name: "foreign_keys",
            expected: "1".into(),
            actual: foreign_keys.to_string(),
        },
        InvariantCheck {
            name: "journal_mode",
            expected: "wal".into(),
            actual: journal_mode,
        },
        InvariantCheck {
            name: "synchronous",
            expected: "1".into(),
            actual: synchronous.to_string(),
        },
        InvariantCheck {
            name: "busy_timeout_ms",
            expected: i64::try_from(BUSY_TIMEOUT.as_millis())
                .expect("busy timeout fits in i64")
                .to_string(),
            actual: busy_timeout_ms.to_string(),
        },
    ])
}

/// Inserts the single instance identity row on first initialization.
async fn ensure_instance_identity(pool: &SqlitePool) -> Result<()> {
    let created_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .context("formatting instance creation instant")?;
    sqlx::query(
        "INSERT INTO instance (id, installation_id, created_at_utc)
         VALUES (1, ?1, ?2)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(created_at)
    .execute(pool)
    .await
    .context("initializing instance identity")?;
    Ok(())
}

/// The opaque, stable identifier of this installation.
pub async fn installation_id(pool: &SqlitePool) -> Result<String> {
    let row = sqlx::query("SELECT installation_id FROM instance WHERE id = 1")
        .fetch_one(pool)
        .await
        .context("reading instance identity")?;
    Ok(row.get(0))
}

/// Runs `SQLite`'s own integrity check and returns its verdict lines.
pub async fn integrity_check(pool: &SqlitePool) -> Result<Vec<String>> {
    let rows = sqlx::query("PRAGMA integrity_check")
        .fetch_all(pool)
        .await
        .context("running integrity check")?;
    Ok(rows.into_iter().map(|row| row.get(0)).collect())
}
