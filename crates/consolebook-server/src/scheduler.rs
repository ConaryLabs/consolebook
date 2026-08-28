//! Automatic backup scheduling.
//!
//! `serve` spawns one background task that keeps the newest snapshot no
//! older than the configured interval. The decision function is pure and
//! tested; the task just applies it on a coarse tick, so a missed tick
//! (sleep, clock jump, slow backup) self-corrects on the next one.

use std::time::Duration;

use sqlx::SqlitePool;
use time::OffsetDateTime;

use crate::backup;
use crate::capabilities::Capability;
use crate::data_dir::DataDir;
use crate::notices::{self, NoticeKind};

/// Default interval between automatic backups.
pub const DEFAULT_INTERVAL_HOURS: u64 = 24;

/// How often the scheduler re-evaluates whether a backup is due.
const TICK: Duration = Duration::from_secs(60);

/// Whether a backup is due now, given the newest snapshot's mtime.
/// No snapshot at all means due immediately.
#[must_use]
pub fn backup_due(latest_snapshot_mtime: Option<i64>, now: i64, interval_secs: i64) -> bool {
    match latest_snapshot_mtime {
        None => true,
        Some(latest) => now.saturating_sub(latest) >= interval_secs,
    }
}

/// Runs the automatic-backup loop until the process exits. Failures are
/// logged, surfaced to administrators as in-app notices, and retried on
/// the schedule; they never crash the server.
pub async fn run(data_dir: DataDir, pool: SqlitePool, interval: Duration, keep: usize) {
    let interval_secs = i64::try_from(interval.as_secs()).unwrap_or(i64::MAX);
    loop {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let due = match backup::latest_snapshot_mtime(&data_dir) {
            Ok(latest) => backup_due(latest, now, interval_secs),
            Err(err) => {
                tracing::error!("cannot inspect backups directory: {err:#}");
                false
            }
        };
        if due {
            match backup::run(&data_dir, keep).await {
                Ok(report) => tracing::info!(
                    snapshot = %report.snapshot.display(),
                    size_bytes = report.size_bytes,
                    pruned = report.pruned.len(),
                    "automatic backup complete and validated"
                ),
                Err(err) => {
                    tracing::error!("automatic backup failed: {err:#}");
                    report_backup_failure(&pool, &err).await;
                }
            }
        }
        tokio::time::sleep(TICK).await;
    }
}

/// Surfaces a backup failure to every administrator as a persisted notice.
/// Deduplication in the notices service keeps a repeatedly failing backup
/// at one unread notice per administrator.
async fn report_backup_failure(pool: &SqlitePool, err: &anyhow::Error) {
    let message = format!(
        "Automatic backup failed: {err:#}. Backups retry on schedule; \
         check disk space and `consolebook doctor`, and treat repeated \
         failures as urgent."
    );
    match notices::notify_capability_holders(
        pool,
        Capability::ManageUsers,
        NoticeKind::BackupFailed,
        &message,
    )
    .await
    {
        Ok(created) if created > 0 => {
            tracing::info!(created, "backup failure surfaced as in-app notices");
        }
        Ok(_) => {}
        Err(notify_err) => {
            tracing::error!("could not create backup-failure notices: {notify_err:#}");
        }
    }
}
