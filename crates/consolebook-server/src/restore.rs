//! Restore from a validated snapshot.
//!
//! Restore is a product workflow, not file surgery: it validates the
//! snapshot as its own database, refuses while a server holds the
//! installation, sets the current database aside as a pre-restore snapshot,
//! moves the validated snapshot into place, and proves the result (connection
//! invariants, integrity, instance identity) before reporting success. The
//! restore is recorded as an audit event in the restored database.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use time::OffsetDateTime;
use time::macros::format_description;

use crate::audit::{self, EventKind};
use crate::data_dir::DataDir;
use crate::serve_lock::ServeLock;
use crate::{backup, storage};

/// Outcome of a completed restore.
#[derive(Debug)]
pub struct RestoreReport {
    pub restored_from: PathBuf,
    pub pre_restore_snapshot: Option<PathBuf>,
    pub installation_id: String,
}

/// Restores `snapshot` as the installation's database.
pub async fn run(data_dir: &DataDir, snapshot: &Path) -> Result<RestoreReport> {
    if !snapshot.exists() {
        bail!("snapshot {} does not exist", snapshot.display());
    }
    // Validate the incoming snapshot before touching anything.
    backup::validate_snapshot(snapshot)
        .await
        .with_context(|| format!("snapshot {} failed validation", snapshot.display()))?;

    // Refuse while a server holds the installation; hold the lock ourselves
    // for the duration so a server starting mid-restore is refused instead.
    let _lock = ServeLock::acquire(data_dir)
        .context("cannot restore while the installation is in use (stop the server first)")?;

    let database = data_dir.database();
    // Set the current database aside so a mistaken restore is itself
    // recoverable. A healthy database becomes a validated pre-restore
    // snapshot; one too damaged to snapshot must not block its own
    // replacement — its raw bytes are moved aside instead.
    let pre_restore_snapshot = if database.exists() {
        match set_aside_current(data_dir).await {
            Ok(snapshot) => Some(snapshot),
            Err(err) => {
                let stamp = timestamp()?;
                let aside = data_dir
                    .backups()
                    .join(format!("consolebook-prerestore-{stamp}.damaged"));
                std::fs::rename(&database, &aside).with_context(|| {
                    format!(
                        "current database cannot be snapshotted ({err:#}) and \
                         cannot be moved aside to {}",
                        aside.display()
                    )
                })?;
                backup::persist(&aside)?;
                tracing::warn!(
                    moved_to = %aside.display(),
                    "current database could not be snapshotted; raw bytes moved aside"
                );
                Some(aside)
            }
        }
    } else {
        None
    };

    // WAL sidecar files belong to the old database; a stale pair must not
    // be replayed into the restored one.
    for sidecar in ["-wal", "-shm"] {
        let mut path = database.as_os_str().to_owned();
        path.push(sidecar);
        let path = PathBuf::from(path);
        if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("removing stale {}", path.display()))?;
        }
    }
    std::fs::copy(snapshot, &database).with_context(|| {
        format!(
            "copying {} into place as {}",
            snapshot.display(),
            database.display()
        )
    })?;
    backup::persist(&database)?;

    // Prove the result the same way startup would, and record the restore
    // in the restored database's own audit record.
    let pool = storage::open(&database).await.context(
        "restored database failed startup verification; the pre-restore snapshot is untouched",
    )?;
    let verdict = storage::integrity_check(&pool).await?;
    if verdict != ["ok"] {
        bail!(
            "restored database failed integrity check: {}",
            verdict.join("; ")
        );
    }
    let installation_id = storage::installation_id(&pool).await?;
    audit::record(&pool, EventKind::RestoreCompleted, None, None).await?;
    pool.close().await;

    Ok(RestoreReport {
        restored_from: snapshot.to_path_buf(),
        pre_restore_snapshot,
        installation_id,
    })
}

fn timestamp() -> Result<String> {
    OffsetDateTime::now_utc()
        .format(format_description!(
            "[year][month][day]T[hour][minute][second]Z"
        ))
        .context("formatting pre-restore timestamp")
}

/// Snapshots the current database into `backups/` under a `prerestore`
/// name, so a mistaken restore is itself recoverable.
async fn set_aside_current(data_dir: &DataDir) -> Result<PathBuf> {
    let stamp = timestamp()?;
    let target = data_dir
        .backups()
        .join(format!("consolebook-prerestore-{stamp}.db"));
    let pool = storage::open_existing(&data_dir.database())
        .await
        .context("opening current database for the pre-restore snapshot")?;
    let target_str = target
        .to_str()
        .context("pre-restore path is not valid UTF-8")?;
    sqlx::query("VACUUM INTO ?1")
        .bind(target_str)
        .execute(&pool)
        .await
        .context("taking pre-restore snapshot")?;
    pool.close().await;
    backup::validate_snapshot(&target)
        .await
        .context("pre-restore snapshot failed validation; aborting restore")?;
    backup::persist(&target)?;
    Ok(target)
}
