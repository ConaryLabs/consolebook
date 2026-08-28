//! Command-line entry point for the Consolebook server.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use consolebook_server::data_dir::DataDir;
use consolebook_server::doctor::Verdict;
use consolebook_server::serve_lock::ServeLock;
use consolebook_server::users::{IssueRefusal, ResetOrigin};
use consolebook_server::{
    VERSION, backup, doctor, http, restore, scheduler, setup, storage, users,
};

#[derive(Parser)]
#[command(name = "consolebook", version = VERSION, about = "Training-record system for emergency communications centers (pre-alpha)")]
struct Cli {
    /// Installation data directory (one directory per agency installation).
    #[arg(
        long,
        global = true,
        env = "CONSOLEBOOK_DATA_DIR",
        default_value = "data"
    )]
    data_dir: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize the data directory if needed and serve the HTTP API.
    Serve {
        /// Address to bind. Local by default; put a reverse proxy in front
        /// for external TLS if you expose it.
        #[arg(long, env = "CONSOLEBOOK_BIND", default_value = "127.0.0.1:7770")]
        bind: SocketAddr,
        /// Hours between automatic validated backups.
        #[arg(
            long,
            env = "CONSOLEBOOK_BACKUP_INTERVAL_HOURS",
            default_value_t = scheduler::DEFAULT_INTERVAL_HOURS,
            value_parser = clap::value_parser!(u64).range(1..)
        )]
        backup_interval_hours: u64,
        /// Snapshots retention keeps in backups/.
        #[arg(
            long,
            env = "CONSOLEBOOK_BACKUP_KEEP",
            default_value_t = backup::DEFAULT_KEEP as u64,
            value_parser = clap::value_parser!(u64).range(1..)
        )]
        backup_keep: u64,
    },
    /// Diagnose an existing installation without modifying it.
    Doctor,
    /// Take a validated, consistent snapshot of the database into backups/.
    Backup {
        /// Snapshots retention keeps in backups/.
        #[arg(
            long,
            env = "CONSOLEBOOK_BACKUP_KEEP",
            default_value_t = backup::DEFAULT_KEEP as u64,
            value_parser = clap::value_parser!(u64).range(1..)
        )]
        keep: u64,
    },
    /// Replace the database with a validated snapshot (server must be stopped).
    ///
    /// The current database is set aside as a pre-restore snapshot in
    /// backups/ before anything is replaced.
    Restore {
        /// Path to the snapshot to restore, usually in backups/.
        snapshot: PathBuf,
    },
    /// Print a fresh first-run setup code for an uninitialized installation.
    SetupCode,
    /// Issue a password reset code for a locked-out administrator.
    ///
    /// Requires operating-system access to the data directory and records
    /// an explicit recovery audit event.
    Recover {
        /// Username of the administrator account to recover.
        #[arg(long)]
        username: String,
    },
}

fn init_logging() {
    use tracing_subscriber::EnvFilter;
    // Structured logs, no record content: log events describe operations and
    // outcomes, never evaluation narratives or personal data.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
}

#[tokio::main]
async fn main() -> ExitCode {
    init_logging();
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Serve {
            bind,
            backup_interval_hours,
            backup_keep,
        } => serve(&cli.data_dir, bind, backup_interval_hours, backup_keep).await,
        Command::Doctor => run_doctor(&cli.data_dir).await,
        Command::Backup { keep } => run_backup(&cli.data_dir, keep).await,
        Command::Restore { snapshot } => run_restore(&cli.data_dir, &snapshot).await,
        Command::SetupCode => run_setup_code(&cli.data_dir).await,
        Command::Recover { username } => run_recover(&cli.data_dir, &username).await,
    };
    match result {
        Ok(code) => code,
        Err(err) => {
            tracing::error!("{err:#}");
            ExitCode::FAILURE
        }
    }
}

async fn serve(
    data_dir: &std::path::Path,
    bind: SocketAddr,
    backup_interval_hours: u64,
    backup_keep: u64,
) -> Result<ExitCode> {
    let data_dir = DataDir::new(data_dir);
    data_dir.ensure_layout()?;
    let lock = ServeLock::acquire(&data_dir)?;
    tracing::debug!(lock = %lock.path().display(), "holding installation lock");
    let pool = storage::open(&data_dir.database()).await?;
    let installation_id = storage::installation_id(&pool).await?;
    tracing::info!(
        version = VERSION,
        installation_id,
        data_dir = %data_dir.root().display(),
        "starting Consolebook (pre-alpha)"
    );

    if !consolebook_server::web_assets::embedded() {
        tracing::warn!(
            "this build does not embed the web interface; build web/ (npm ci && npm run build) before cargo for a complete executable"
        );
    }
    if let Some((code, expires_at)) = setup::issue_setup_code(&pool).await? {
        // The one deliberate secret in the log: the operator needs it to
        // complete first-run setup, and it is useless once setup completes
        // or the code expires.
        tracing::warn!(
            setup_code = code.raw,
            expires_in_minutes = (expires_at - now_unix()) / 60,
            "installation is not initialized; complete setup at POST /api/setup with this code"
        );
    }

    let scheduler = tokio::spawn(scheduler::run(
        data_dir.clone(),
        pool.clone(),
        std::time::Duration::from_secs(backup_interval_hours * 3600),
        usize::try_from(backup_keep).unwrap_or(usize::MAX),
    ));

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("binding {bind}"))?;
    tracing::info!(addr = %listener.local_addr()?, "listening");
    http::serve(listener, http::AppState { pool }).await?;
    scheduler.abort();
    drop(lock);
    tracing::info!("stopped");
    Ok(ExitCode::SUCCESS)
}

async fn run_doctor(data_dir: &std::path::Path) -> Result<ExitCode> {
    let data_dir = DataDir::new(data_dir);
    let findings = doctor::run(&data_dir).await;
    for finding in &findings {
        let mark = match finding.verdict {
            Verdict::Ok => "ok  ",
            Verdict::Warn => "warn",
            Verdict::Fail => "FAIL",
        };
        println!("{mark}  {:<22} {}", finding.check, finding.detail);
    }
    if doctor::has_failure(&findings) {
        Ok(ExitCode::FAILURE)
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

fn now_unix() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

async fn run_setup_code(data_dir: &std::path::Path) -> Result<ExitCode> {
    let data_dir = DataDir::new(data_dir);
    let pool = storage::open(&data_dir.database()).await?;
    match setup::issue_setup_code(&pool).await? {
        None => {
            eprintln!("this installation is already initialized; setup is unavailable");
            Ok(ExitCode::FAILURE)
        }
        Some((code, expires_at)) => {
            println!("{}", code.raw);
            eprintln!(
                "setup code valid for {} minutes; complete setup at POST /api/setup",
                (expires_at - now_unix()) / 60
            );
            Ok(ExitCode::SUCCESS)
        }
    }
}

async fn run_recover(data_dir: &std::path::Path, username: &str) -> Result<ExitCode> {
    let data_dir = DataDir::new(data_dir);
    let pool = storage::open(&data_dir.database()).await?;
    match users::issue_reset_code(&pool, username, ResetOrigin::Recovery).await? {
        Ok(issued) => {
            println!("{}", issued.code.raw);
            eprintln!(
                "reset code for {} valid for {} minutes; use POST /api/auth/reset with a new password. All sessions revoke when it is used.",
                issued.user.username,
                (issued.expires_at - now_unix()) / 60
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(IssueRefusal::NoSuchUser) => {
            eprintln!("no user named {username}");
            Ok(ExitCode::FAILURE)
        }
        Err(IssueRefusal::NotAnAdministrator) => {
            eprintln!(
                "{username} is not an administrator; ask an administrator to issue a reset code instead"
            );
            Ok(ExitCode::FAILURE)
        }
    }
}

async fn run_backup(data_dir: &std::path::Path, keep: u64) -> Result<ExitCode> {
    let data_dir = DataDir::new(data_dir);
    data_dir.ensure_layout()?;
    let report = backup::run(&data_dir, usize::try_from(keep).unwrap_or(usize::MAX)).await?;
    tracing::info!(
        snapshot = %report.snapshot.display(),
        size_bytes = report.size_bytes,
        pruned = report.pruned.len(),
        "backup complete and validated"
    );
    println!("{}", report.snapshot.display());
    Ok(ExitCode::SUCCESS)
}

async fn run_restore(data_dir: &std::path::Path, snapshot: &std::path::Path) -> Result<ExitCode> {
    let data_dir = DataDir::new(data_dir);
    data_dir.ensure_layout()?;
    let report = restore::run(&data_dir, snapshot).await?;
    if let Some(pre) = &report.pre_restore_snapshot {
        eprintln!("previous database set aside as {}", pre.display());
    }
    eprintln!(
        "restored {} (installation {})",
        report.restored_from.display(),
        report.installation_id
    );
    Ok(ExitCode::SUCCESS)
}
