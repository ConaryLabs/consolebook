//! Validated backups: consistent snapshot, validation, explicit
//! durability, retention.
//!
//! A backup is a `VACUUM INTO` snapshot (consistent even while the database
//! is in use), validated with `SQLite`'s integrity check, then fsynced along
//! with its directory, audited, and finally subject to retention pruning.
//! The scheduler in `scheduler.rs` drives this automatically from `serve`;
//! the `backup` command drives it manually.

use std::fs::File;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use time::OffsetDateTime;
use time::format_description::BorrowedFormatItem;
use time::macros::format_description;

use crate::audit::{self, EventKind};
use crate::data_dir::DataDir;
use crate::storage;

const SNAPSHOT_STAMP: &[BorrowedFormatItem<'_>] =
    format_description!("[year][month][day]T[hour][minute][second]Z");

/// Default number of snapshots retention keeps.
pub const DEFAULT_KEEP: usize = 14;

/// Outcome of one completed, validated backup.
#[derive(Debug)]
pub struct BackupReport {
    pub snapshot: PathBuf,
    pub size_bytes: u64,
    pub pruned: Vec<PathBuf>,
}

/// Takes a consistent snapshot of the live database into `backups/`,
/// validates it, makes it durable, records the audit event, and prunes to
/// the retention count. Fails without leaving a snapshot behind if
/// validation fails.
pub async fn run(data_dir: &DataDir, keep: usize) -> Result<BackupReport> {
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

    if let Err(err) = validate_snapshot(&snapshot).await {
        pool.close().await;
        // A snapshot that fails validation must not look like a usable backup.
        let _ = std::fs::remove_file(&snapshot);
        return Err(err.context("snapshot failed validation and was removed"));
    }

    persist(&snapshot)?;
    audit::record(&pool, EventKind::BackupCompleted, None, None).await?;
    pool.close().await;

    let size_bytes = std::fs::metadata(&snapshot)
        .with_context(|| format!("reading snapshot metadata {}", snapshot.display()))?
        .len();
    let pruned = prune(data_dir, keep)?;
    Ok(BackupReport {
        snapshot,
        size_bytes,
        pruned,
    })
}

/// The snapshots currently in `backups/`, oldest first. The timestamped
/// names sort chronologically; this listing is discovery only — nothing
/// about a backup's validity is inferred from its name.
pub fn snapshots(data_dir: &DataDir) -> Result<Vec<PathBuf>> {
    let dir = data_dir.backups();
    let mut found = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(found),
        Err(err) => {
            return Err(err).with_context(|| format!("listing backups in {}", dir.display()));
        }
    };
    for entry in entries {
        let entry = entry.context("reading backup directory entry")?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("consolebook-") && name.ends_with(".db") {
            found.push(entry.path());
        }
    }
    found.sort();
    Ok(found)
}

/// Unix mtime of the newest snapshot, for scheduling and diagnostics.
pub fn latest_snapshot_mtime(data_dir: &DataDir) -> Result<Option<i64>> {
    let mut latest: Option<i64> = None;
    for snapshot in snapshots(data_dir)? {
        let modified = std::fs::metadata(&snapshot)
            .and_then(|m| m.modified())
            .with_context(|| format!("reading snapshot mtime {}", snapshot.display()))?;
        let unix = modified
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
        latest = Some(latest.map_or(unix, |current| current.max(unix)));
    }
    Ok(latest)
}

/// Deletes the oldest snapshots beyond `keep`. Never deletes the last
/// remaining snapshot, whatever `keep` says. Returns what was removed.
pub fn prune(data_dir: &DataDir, keep: usize) -> Result<Vec<PathBuf>> {
    let keep = keep.max(1);
    let all = snapshots(data_dir)?;
    let excess = all.len().saturating_sub(keep);
    let mut pruned = Vec::with_capacity(excess);
    for snapshot in all.into_iter().take(excess) {
        std::fs::remove_file(&snapshot)
            .with_context(|| format!("pruning snapshot {}", snapshot.display()))?;
        pruned.push(snapshot);
    }
    Ok(pruned)
}

/// Opens the snapshot as its own database and requires a clean integrity check.
pub async fn validate_snapshot(snapshot: &Path) -> Result<()> {
    let pool = storage::open_existing(snapshot).await?;
    let verdict = storage::integrity_check(&pool).await?;
    pool.close().await;
    if verdict != ["ok"] {
        bail!("integrity check reported: {}", verdict.join("; "));
    }
    Ok(())
}

/// Explicit durability step: fsync a file and its directory entry.
pub fn persist(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|f| f.sync_all())
        .with_context(|| format!("syncing {}", path.display()))?;
    let dir = path.parent().context("path has no parent directory")?;
    File::open(dir)
        .and_then(|f| f.sync_all())
        .with_context(|| format!("syncing directory {}", dir.display()))?;
    Ok(())
}
