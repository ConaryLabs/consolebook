//! Diagnostic reads preserve database bytes and observe live WAL state.

use consolebook_server::data_dir::DataDir;
use consolebook_server::{doctor, storage};
use sqlx::Connection;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};

async fn initialize_stopped(data: &DataDir) {
    // A single explicitly closed connection makes WAL cleanup part of fixture
    // setup, independent of asynchronous pool returns and maintenance tasks.
    let mut connection = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(data.database())
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal),
    )
    .await
    .expect("fixture connection");
    storage::MIGRATOR
        .run(&mut connection)
        .await
        .expect("migrate fixture");
    sqlx::query("INSERT INTO instance (id, installation_id, created_at_utc) VALUES (1, 'invented-stopped-id', '2026-09-05T00:00:00Z')")
        .execute(&mut connection)
        .await
        .expect("fixture identity");
    connection.close().await.expect("stop fixture");
    assert!(!data.root().join("consolebook.db-wal").exists());
    assert!(!data.root().join("consolebook.db-shm").exists());
}

fn installation() -> (tempfile::TempDir, DataDir) {
    let tmp = tempfile::tempdir().expect("scratch directory");
    let data = DataDir::new(tmp.path().join("data"));
    data.ensure_layout().expect("layout");
    (tmp, data)
}

fn assert_healthy(findings: &[doctor::Finding]) {
    assert!(!doctor::has_failure(findings), "{findings:?}");
    assert!(findings.iter().any(|f| f.check == "pragma journal_mode"
        && f.verdict == doctor::Verdict::Ok
        && f.detail.starts_with("wal (observed database mode")));
}

#[tokio::test]
async fn diagnostic_connection_cannot_create_a_missing_database() {
    let (_tmp, data) = installation();
    assert!(storage::open_diagnostic(&data.database()).await.is_err());
    assert!(!data.database().exists());
    let findings = doctor::run(&data).await;
    assert!(doctor::has_failure(&findings));
    assert!(!data.database().exists());
}

#[tokio::test]
async fn doctor_reports_delete_mode_without_changing_any_files() {
    let (_tmp, data) = installation();
    // A migrated installation gives the remaining diagnostics real tables to
    // inspect; changing its mode must be reported, never silently repaired.
    initialize_stopped(&data).await;
    let mut connection =
        SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(data.database()))
            .await
            .expect("fixture connection");
    let mode: String = sqlx::query_scalar("PRAGMA journal_mode = DELETE")
        .fetch_one(&mut connection)
        .await
        .expect("misconfigure fixture");
    assert_eq!(mode, "delete");
    connection.close().await.expect("close fixture");
    let before = std::fs::read(data.database()).expect("database bytes");

    let findings = doctor::run(&data).await;
    let failures: Vec<_> = findings
        .iter()
        .filter(|f| f.verdict == doctor::Verdict::Fail)
        .collect();
    assert_eq!(failures.len(), 1, "{findings:?}");
    assert_eq!(failures[0].check, "pragma journal_mode");
    assert!(failures[0].detail.starts_with("expected wal, got delete"));
    assert_eq!(before, std::fs::read(data.database()).expect("after"));
    assert!(!data.root().join("consolebook.db-wal").exists());
    assert!(!data.root().join("consolebook.db-shm").exists());

    let pool = storage::open_diagnostic(&data.database())
        .await
        .expect("read-only connection");
    let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&pool)
        .await
        .expect("observed mode");
    assert_eq!(mode, "delete");
    pool.close().await;
}

#[tokio::test]
async fn diagnostic_connection_refuses_data_schema_and_journal_writes() {
    let (_tmp, data) = installation();
    initialize_stopped(&data).await;
    let before = std::fs::read(data.database()).expect("before");
    let pool = storage::open_diagnostic(&data.database())
        .await
        .expect("diagnostic connection");
    for statement in [
        "UPDATE instance SET installation_id = 'invented-write' WHERE id = 1",
        "CREATE TABLE invented_write (id INTEGER)",
        "PRAGMA journal_mode = DELETE",
    ] {
        assert!(sqlx::query(statement).execute(&pool).await.is_err());
    }
    pool.close().await;
    assert_eq!(before, std::fs::read(data.database()).expect("after"));
}

#[tokio::test]
async fn doctor_reads_live_wal_commits_and_preserves_database_and_wal_bytes() {
    let (_tmp, data) = installation();
    let writer = storage::open(&data.database()).await.expect("initialize");
    // Keep one writer connection and put the next identity exclusively in WAL.
    let mut connection = writer.acquire().await.expect("writer connection");
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(&mut *connection)
        .await
        .expect("checkpoint fixture");
    sqlx::query("UPDATE instance SET installation_id = 'invented-live-wal-id' WHERE id = 1")
        .execute(&mut *connection)
        .await
        .expect("commit to WAL");
    let before = std::fs::read(data.database()).expect("before");
    assert!(
        !before
            .windows(b"invented-live-wal-id".len())
            .any(|bytes| bytes == b"invented-live-wal-id")
    );
    let wal_path = data.root().join("consolebook.db-wal");
    let wal_before = std::fs::read(&wal_path).expect("WAL before");
    assert!(wal_before.len() > 32, "fixture needs committed WAL frames");

    let findings = doctor::run(&data).await;
    assert_healthy(&findings);
    assert!(
        findings
            .iter()
            .any(|f| f.check == "instance identity" && f.detail == "invented-live-wal-id")
    );
    assert_eq!(before, std::fs::read(data.database()).expect("after"));
    assert_eq!(wal_before, std::fs::read(wal_path).expect("WAL after"));
    drop(connection);
    writer.close().await;
}

#[tokio::test]
async fn doctor_reads_stopped_wal_installation_without_changing_database_bytes() {
    let (_tmp, data) = installation();
    initialize_stopped(&data).await;
    assert!(!data.root().join("consolebook.db-wal").exists());
    assert!(!data.root().join("consolebook.db-shm").exists());
    let before = std::fs::read(data.database()).expect("before");
    let findings = doctor::run(&data).await;
    assert_healthy(&findings);
    assert_eq!(before, std::fs::read(data.database()).expect("after"));
    for name in ["foreign_keys", "synchronous", "busy_timeout_ms"] {
        assert!(findings.iter().any(|f| f.check == format!("pragma {name}")
            && f.detail.contains("diagnostic connection only")));
    }
}

#[cfg(unix)]
mod permissions {
    use super::{initialize_stopped, installation, storage};
    use std::os::unix::fs::PermissionsExt;

    fn run_doctor(data: &consolebook_server::data_dir::DataDir) -> std::process::Output {
        // A separate process cannot reuse the fixture writer's writable SHM
        // mapping after chmod. Exercise the actual operator-facing command.
        std::process::Command::new(env!("CARGO_BIN_EXE_consolebook-server"))
            .arg("--data-dir")
            .arg(data.root())
            .arg("doctor")
            .output()
            .expect("doctor process")
    }

    // Restore permissions even if an assertion fails, so TempDir can clean up.
    struct ReadOnlyDirectory(std::path::PathBuf);

    impl ReadOnlyDirectory {
        fn new(path: &std::path::Path) -> Self {
            for entry in std::fs::read_dir(path).expect("directory") {
                let path = entry.expect("entry").path();
                if path.is_file() {
                    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o444))
                        .expect("read-only file");
                }
            }
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o555))
                .expect("read-only directory");
            Self(path.to_path_buf())
        }
    }

    impl Drop for ReadOnlyDirectory {
        fn drop(&mut self) {
            std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755))
                .expect("restore directory permissions");
            for entry in std::fs::read_dir(&self.0).expect("directory") {
                let path = entry.expect("entry").path();
                if path.is_file() {
                    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644))
                        .expect("restore file permissions");
                }
            }
        }
    }

    #[tokio::test]
    async fn read_only_directory_without_wal_sidecars_reports_failure() {
        let (_tmp, data) = installation();
        initialize_stopped(&data).await;
        assert!(!data.root().join("consolebook.db-wal").exists());
        assert!(!data.root().join("consolebook.db-shm").exists());
        let before = std::fs::read(data.database()).expect("before");
        let _permissions = ReadOnlyDirectory::new(data.root());
        // A privileged runner would make chmod-based filesystem proof false.
        assert!(
            std::fs::File::create(data.root().join("write-probe")).is_err(),
            "run permission tests as an unprivileged user"
        );
        let output = run_doctor(&data);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert_eq!(before, std::fs::read(data.database()).expect("after"));
        assert!(!data.root().join("consolebook.db-wal").exists());
        assert!(!data.root().join("consolebook.db-shm").exists());
    }

    #[tokio::test]
    async fn readable_live_wal_sidecars_allow_diagnosis_in_read_only_directory() {
        let (_tmp, data) = installation();
        let writer = storage::open(&data.database()).await.expect("initialize");
        let permissions = ReadOnlyDirectory::new(data.root());
        assert!(
            std::fs::File::create(data.root().join("write-probe")).is_err(),
            "run permission tests as an unprivileged user"
        );
        let paths = [
            data.database(),
            data.root().join("consolebook.db-wal"),
            data.root().join("consolebook.db-shm"),
        ];
        let before: Vec<_> = paths
            .iter()
            .map(|p| std::fs::read(p).expect("before"))
            .collect();
        let output = run_doctor(&data);
        assert!(output.status.success(), "{output:?}");
        for (path, bytes) in paths.iter().zip(before) {
            assert!(
                bytes == std::fs::read(path).expect("after"),
                "changed {}",
                path.display()
            );
        }
        drop(permissions);
        writer.close().await;
    }
}
