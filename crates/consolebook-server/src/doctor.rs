//! `consolebook doctor`: diagnose an installation without changing it.
//!
//! Doctor never creates the database, never migrates, and never writes to the
//! data directory. It reports what it finds; the operator decides what to do.

use anyhow::Result;
use sqlx::SqlitePool;

use crate::data_dir::DataDir;
use crate::storage;

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug)]
pub struct Finding {
    pub check: String,
    pub verdict: Verdict,
    pub detail: String,
}

impl Finding {
    fn ok(check: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            check: check.into(),
            verdict: Verdict::Ok,
            detail: detail.into(),
        }
    }
    fn warn(check: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            check: check.into(),
            verdict: Verdict::Warn,
            detail: detail.into(),
        }
    }
    fn fail(check: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            check: check.into(),
            verdict: Verdict::Fail,
            detail: detail.into(),
        }
    }
}

/// Runs every diagnostic and returns the findings. The caller decides how to
/// render them and what exit code the process deserves.
pub async fn run(data_dir: &DataDir) -> Vec<Finding> {
    let mut findings = Vec::new();

    check_layout(data_dir, &mut findings);

    let db_path = data_dir.database();
    if !db_path.exists() {
        findings.push(Finding::fail(
            "database",
            format!(
                "{} does not exist; run `consolebook serve` to initialize",
                db_path.display()
            ),
        ));
        return findings;
    }

    match storage::open_existing(&db_path).await {
        Err(err) => findings.push(Finding::fail("database", format!("cannot open: {err:#}"))),
        Ok(pool) => {
            findings.push(Finding::ok("database", format!("{}", db_path.display())));
            check_invariants(&pool, &mut findings).await;
            check_migrations(&pool, &mut findings).await;
            check_integrity(&pool, &mut findings).await;
            check_identity(&pool, &mut findings).await;
            check_initialization(&pool, &mut findings).await;
            pool.close().await;
        }
    }

    check_backups(data_dir, &mut findings);
    findings
}

#[must_use]
pub fn has_failure(findings: &[Finding]) -> bool {
    findings.iter().any(|f| f.verdict == Verdict::Fail)
}

fn check_layout(data_dir: &DataDir, findings: &mut Vec<Finding>) {
    for (name, path) in [
        ("data directory", data_dir.root().to_path_buf()),
        ("backups directory", data_dir.backups()),
        ("exports directory", data_dir.exports()),
        ("instance directory", data_dir.instance()),
    ] {
        if path.is_dir() {
            findings.push(Finding::ok(name, format!("{}", path.display())));
        } else {
            findings.push(Finding::fail(name, format!("missing: {}", path.display())));
        }
    }
}

async fn check_invariants(pool: &SqlitePool, findings: &mut Vec<Finding>) {
    match storage::verify_invariants(pool).await {
        Err(err) => findings.push(Finding::fail(
            "connection invariants",
            format!("could not verify: {err:#}"),
        )),
        Ok(checks) => {
            for check in checks {
                let name = format!("pragma {}", check.name);
                if check.holds() {
                    findings.push(Finding::ok(name, check.actual));
                } else {
                    findings.push(Finding::fail(
                        name,
                        format!("expected {}, got {}", check.expected, check.actual),
                    ));
                }
            }
        }
    }
}

async fn check_migrations(pool: &SqlitePool, findings: &mut Vec<Finding>) {
    let applied: Result<Vec<i64>, sqlx::Error> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(pool)
            .await;
    match applied {
        Err(err) => findings.push(Finding::fail(
            "migrations",
            format!("cannot read migration history: {err}"),
        )),
        Ok(applied) => {
            let expected: Vec<i64> = storage::MIGRATOR.iter().map(|m| m.version).collect();
            let missing: Vec<String> = expected
                .iter()
                .filter(|v| !applied.contains(v))
                .map(ToString::to_string)
                .collect();
            let unknown: Vec<String> = applied
                .iter()
                .filter(|v| !expected.contains(v))
                .map(ToString::to_string)
                .collect();
            if missing.is_empty() && unknown.is_empty() {
                findings.push(Finding::ok(
                    "migrations",
                    format!("{} applied, up to date", applied.len()),
                ));
            } else if !missing.is_empty() {
                findings.push(Finding::fail(
                    "migrations",
                    format!("pending: {}", missing.join(", ")),
                ));
            } else {
                findings.push(Finding::fail(
                    "migrations",
                    format!(
                        "database has migrations this build does not know: {}",
                        unknown.join(", ")
                    ),
                ));
            }
        }
    }
}

async fn check_integrity(pool: &SqlitePool, findings: &mut Vec<Finding>) {
    match storage::integrity_check(pool).await {
        Err(err) => findings.push(Finding::fail("integrity", format!("{err:#}"))),
        Ok(verdict) if verdict == ["ok"] => findings.push(Finding::ok("integrity", "ok")),
        Ok(verdict) => findings.push(Finding::fail("integrity", verdict.join("; "))),
    }
}

async fn check_identity(pool: &SqlitePool, findings: &mut Vec<Finding>) {
    match storage::installation_id(pool).await {
        Err(err) => findings.push(Finding::fail("instance identity", format!("{err:#}"))),
        Ok(id) => findings.push(Finding::ok("instance identity", id)),
    }
}

async fn check_initialization(pool: &SqlitePool, findings: &mut Vec<Finding>) {
    match crate::setup::is_initialized(pool).await {
        Err(err) => findings.push(Finding::fail(
            "initialization",
            format!("cannot determine: {err:#}"),
        )),
        Ok(false) => findings.push(Finding::warn(
            "initialization",
            "not initialized; start the server and complete first-run setup",
        )),
        Ok(true) => {
            let admins: Result<i64, sqlx::Error> = sqlx::query_scalar(
                "SELECT COUNT(DISTINCT user_id) FROM capability_grant WHERE capability = ?1",
            )
            .bind(crate::capabilities::Capability::ManageUsers.as_str())
            .fetch_one(pool)
            .await;
            match admins {
                Err(err) => findings.push(Finding::fail(
                    "initialization",
                    format!("cannot count administrators: {err}"),
                )),
                Ok(0) => findings.push(Finding::fail(
                    "initialization",
                    "initialized but no user holds manage_users; restore from a backup taken before the grants were lost",
                )),
                Ok(n) => findings.push(Finding::ok(
                    "initialization",
                    format!("initialized, {n} administrator(s)"),
                )),
            }
        }
    }
}

fn check_backups(data_dir: &DataDir, findings: &mut Vec<Finding>) {
    if !data_dir.backups().is_dir() {
        return; // Layout check already reported the missing directory.
    }
    let snapshots = match crate::backup::snapshots(data_dir) {
        Ok(snapshots) => snapshots,
        Err(err) => {
            findings.push(Finding::fail("backups", format!("{err:#}")));
            return;
        }
    };
    if snapshots.is_empty() {
        findings.push(Finding::warn(
            "backups",
            "no snapshots yet; the server takes one automatically, or run `consolebook backup`",
        ));
        return;
    }
    let latest = snapshots.last().expect("non-empty");
    let latest_name = latest
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let age = crate::backup::latest_snapshot_mtime(data_dir)
        .ok()
        .flatten()
        .map(|mtime| time::OffsetDateTime::now_utc().unix_timestamp() - mtime);
    let stale_after = i64::try_from(crate::scheduler::DEFAULT_INTERVAL_HOURS * 3600)
        .expect("interval fits in i64");
    match age {
        Some(age) if age > stale_after => findings.push(Finding::warn(
            "backups",
            format!(
                "{} snapshot(s), newest is {} hours old (older than the default {}-hour interval)",
                snapshots.len(),
                age / 3600,
                crate::scheduler::DEFAULT_INTERVAL_HOURS,
            ),
        )),
        _ => findings.push(Finding::ok(
            "backups",
            format!("{} snapshot(s), latest {latest_name}", snapshots.len()),
        )),
    }
}
