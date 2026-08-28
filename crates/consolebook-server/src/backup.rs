//! Automatic-backup building blocks: consistent snapshot, validation,
//! explicit durability.
//!
//! A backup is a `VACUUM INTO` snapshot (consistent even while the database is
//! in use), validated with `SQLite`'s integrity check, then fsynced along with
//! its directory. Scheduling and retention management arrive later in
//! Milestone 1; this module is the mechanism they will call.

use std::fs::File;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use time::OffsetDateTime;
use time::format_description::BorrowedFormatItem;
use time::macros::format_description;

use crate::data_dir::DataDir;
use crate::storage;

const SNAPSHOT_STAMP: &[BorrowedFormatItem<'_>] =
    format_description!("[year][month][day]T[hour][minute][second]Z");

/// Outcome of one completed, validated backup.
#[derive(Debug)]
pub struct BackupReport {
    pub snapshot: PathBuf,
    pub size_bytes: u64,
}

/// Takes a consistent snapshot of the live database into `backups/`,
/// validates it, and makes it durable. Fails without leaving a snapshot
/// behind if validation fails.
pub async fn run(data_dir: &DataDir) -> Result<BackupReport> {
    let pool = storage::open_existing(&data_dir.database()).await?;
    let stamp = OffsetDateTime::now_utc()
        .format(SNAPSHOT_STAMP)
        .context("formatting snapshot timestamp")?;
    let snapshot = data_dir.backups().join(format!("consolebook-{stamp}.db"));
    if snapshot.exists() {
        bail!(
            "snapshot {} already exists; refusing to overwrite a backup",
            snapshot.display()
        );
    }

    let snapshot_str = snapshot
        .to_str()
        .context("backup path is not valid UTF-8")?;
    sqlx::query("VACUUM INTO ?1")
        .bind(snapshot_str)
        .execute(&pool)
        .await
        .context("taking VACUUM INTO snapshot")?;
    pool.close().await;

    if let Err(err) = validate_snapshot(&snapshot).await {
        // A snapshot that fails validation must not look like a usable backup.
        let _ = std::fs::remove_file(&snapshot);
        return Err(err.context("snapshot failed validation and was removed"));
    }

    persist(&snapshot)?;
    let size_bytes = std::fs::metadata(&snapshot)
        .with_context(|| format!("reading snapshot metadata {}", snapshot.display()))?
        .len();
    Ok(BackupReport {
        snapshot,
        size_bytes,
    })
}

/// Opens the snapshot as its own database and requires a clean integrity check.
async fn validate_snapshot(snapshot: &Path) -> Result<()> {
    let pool = storage::open_existing(snapshot).await?;
    let verdict = storage::integrity_check(&pool).await?;
    pool.close().await;
    if verdict != ["ok"] {
        bail!("integrity check reported: {}", verdict.join("; "));
    }
    Ok(())
}

/// Explicit durability step: fsync the snapshot and its directory entry.
fn persist(snapshot: &Path) -> Result<()> {
    File::open(snapshot)
        .and_then(|f| f.sync_all())
        .with_context(|| format!("syncing snapshot {}", snapshot.display()))?;
    let dir = snapshot
        .parent()
        .context("snapshot has no parent directory")?;
    File::open(dir)
        .and_then(|f| f.sync_all())
        .with_context(|| format!("syncing backup directory {}", dir.display()))?;
    Ok(())
}
