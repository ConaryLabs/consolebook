//! Command-line entry point for the Consolebook server.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use consolebook_server::data_dir::DataDir;
use consolebook_server::doctor::Verdict;
use consolebook_server::{VERSION, backup, doctor, http, storage};

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
    },
    /// Diagnose an existing installation without modifying it.
    Doctor,
    /// Take a validated, consistent snapshot of the database into backups/.
    Backup,
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
        Command::Serve { bind } => serve(&cli.data_dir, bind).await,
        Command::Doctor => run_doctor(&cli.data_dir).await,
        Command::Backup => run_backup(&cli.data_dir).await,
    };
    match result {
        Ok(code) => code,
        Err(err) => {
            tracing::error!("{err:#}");
            ExitCode::FAILURE
        }
    }
}

async fn serve(data_dir: &std::path::Path, bind: SocketAddr) -> Result<ExitCode> {
    let data_dir = DataDir::new(data_dir);
    data_dir.ensure_layout()?;
    let pool = storage::open(&data_dir.database()).await?;
    let installation_id = storage::installation_id(&pool).await?;
    tracing::info!(
        version = VERSION,
        installation_id,
        data_dir = %data_dir.root().display(),
        "starting Consolebook (pre-alpha)"
    );

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("binding {bind}"))?;
    tracing::info!(addr = %listener.local_addr()?, "listening");
    http::serve(listener, http::AppState { pool }).await?;
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

async fn run_backup(data_dir: &std::path::Path) -> Result<ExitCode> {
    let data_dir = DataDir::new(data_dir);
    data_dir.ensure_layout()?;
    let report = backup::run(&data_dir).await?;
    tracing::info!(
        snapshot = %report.snapshot.display(),
        size_bytes = report.size_bytes,
        "backup complete and validated"
    );
    println!("{}", report.snapshot.display());
    Ok(ExitCode::SUCCESS)
}
