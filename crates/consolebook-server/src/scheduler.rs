//! Automatic backup scheduling.
//!
//! `serve` spawns one background task that keeps the newest snapshot no
//! older than the configured interval. The decision function is pure and
//! tested; the task just applies it on a coarse tick, so a missed tick
//! (sleep, clock jump, slow backup) self-corrects on the next one.

use std::time::Duration;

use time::OffsetDateTime;

use crate::backup;
use crate::data_dir::DataDir;

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
/// logged and retried on the schedule; they never crash the server.
pub async fn run(data_dir: DataDir, interval: Duration, keep: usize) {
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
                Err(err) => tracing::error!("automatic backup failed: {err:#}"),
            }
        }
        tokio::time::sleep(TICK).await;
    }
}
