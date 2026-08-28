//! Integration tests for retention, scheduling decisions, and restore.
//! All fixtures are invented.

use consolebook_server::data_dir::DataDir;
use consolebook_server::serve_lock::ServeLock;
use consolebook_server::{backup, restore, scheduler, setup, storage};

fn temp_data_dir() -> (tempfile::TempDir, DataDir) {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let data_dir = DataDir::new(tmp.path().join("data"));
    data_dir.ensure_layout().expect("create layout");
    (tmp, data_dir)
}

async fn initialized_db(data_dir: &DataDir) -> String {
    let pool = storage::open(&data_dir.database()).await.expect("open");
    let code = setup::issue_setup_code(&pool)
        .await
        .expect("issue")
        .expect("uninitialized")
        .0;
    setup::initialize(
        &pool,
        &code.raw,
        "Example County Communications",
        "avery.admin",
        "Avery Admin",
        "invented-passphrase-1",
    )
    .await
    .expect("initialize")
    .expect("accepted");
    let id = storage::installation_id(&pool).await.expect("id");
    pool.close().await;
    id
}

fn fake_snapshot(data_dir: &DataDir, name: &str) {
    std::fs::write(data_dir.backups().join(name), b"placeholder").expect("write");
}

#[tokio::test]
async fn retention_keeps_newest_and_never_the_last() {
    let (_tmp, data_dir) = temp_data_dir();
    for i in 0..5 {
        fake_snapshot(&data_dir, &format!("consolebook-2026010{i}T000000Z.db"));
    }

    let pruned = backup::prune(&data_dir, 2).expect("prune");
    assert_eq!(pruned.len(), 3);
    let remaining = backup::snapshots(&data_dir).expect("list");
    let names: Vec<String> = remaining
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        [
            "consolebook-20260103T000000Z.db",
            "consolebook-20260104T000000Z.db"
        ]
    );

    // keep = 0 still refuses to delete the final snapshot.
    let pruned = backup::prune(&data_dir, 0).expect("prune");
    assert_eq!(pruned.len(), 1);
    assert_eq!(backup::snapshots(&data_dir).expect("list").len(), 1);
    let pruned = backup::prune(&data_dir, 0).expect("prune");
    assert!(pruned.is_empty());
}

#[tokio::test]
async fn backup_runs_prune_and_audits() {
    let (_tmp, data_dir) = temp_data_dir();
    initialized_db(&data_dir).await;

    for i in 0..3 {
        fake_snapshot(&data_dir, &format!("consolebook-2026010{i}T000000Z.db"));
    }
    let report = backup::run(&data_dir, 2).await.expect("backup");
    assert!(report.snapshot.exists());
    // Old fakes pruned down to keep=2 including the new snapshot.
    assert_eq!(backup::snapshots(&data_dir).expect("list").len(), 2);
    assert_eq!(report.pruned.len(), 2);

    let pool = storage::open_existing(&data_dir.database())
        .await
        .expect("open");
    let kinds: Vec<String> = sqlx::query_scalar("SELECT kind FROM audit_event ORDER BY id")
        .fetch_all(&pool)
        .await
        .expect("audit");
    assert!(kinds.iter().any(|k| k == "backup_completed"));
}

#[test]
fn scheduling_decision() {
    let day = 86_400;
    // No snapshot: due immediately.
    assert!(scheduler::backup_due(None, 1_000_000, day));
    // Fresh snapshot: not due.
    assert!(!scheduler::backup_due(Some(1_000_000 - 10), 1_000_000, day));
    // Stale snapshot: due.
    assert!(scheduler::backup_due(
        Some(1_000_000 - day - 1),
        1_000_000,
        day
    ));
    // Clock skew (snapshot in the future): not due, no panic.
    assert!(!scheduler::backup_due(
        Some(1_000_000 + day),
        1_000_000,
        day
    ));
}

#[tokio::test]
async fn restore_round_trip_recovers_a_destroyed_database() {
    let (_tmp, data_dir) = temp_data_dir();
    let original_id = initialized_db(&data_dir).await;

    let report = backup::run(&data_dir, backup::DEFAULT_KEEP)
        .await
        .expect("backup");

    // Catastrophe: the live database is destroyed and replaced by garbage.
    std::fs::write(data_dir.database(), b"not a database at all").expect("corrupt");

    let restored = restore::run(&data_dir, &report.snapshot)
        .await
        .expect("restore");
    assert_eq!(restored.installation_id, original_id);
    // A database too damaged to snapshot is moved aside raw, marked so it
    // can never be mistaken for a usable backup.
    let aside = restored.pre_restore_snapshot.expect("set aside");
    assert_eq!(aside.extension().unwrap(), "damaged");
    assert!(aside.exists());
    let pool = storage::open_existing(&data_dir.database())
        .await
        .expect("open restored");
    let verdict = storage::integrity_check(&pool).await.expect("integrity");
    assert_eq!(verdict, ["ok"]);
    let kinds: Vec<String> = sqlx::query_scalar("SELECT kind FROM audit_event ORDER BY id")
        .fetch_all(&pool)
        .await
        .expect("audit");
    assert!(kinds.iter().any(|k| k == "restore_completed"));
    assert_eq!(
        setup::agency_name(&pool).await.expect("agency").as_deref(),
        Some("Example County Communications")
    );
}

#[tokio::test]
async fn restore_refuses_invalid_snapshot() {
    let (_tmp, data_dir) = temp_data_dir();
    initialized_db(&data_dir).await;
    let bogus = data_dir.backups().join("consolebook-bogus.db");
    std::fs::write(&bogus, b"not a database").expect("write");

    assert!(restore::run(&data_dir, &bogus).await.is_err());
    // The live database is untouched.
    let pool = storage::open_existing(&data_dir.database())
        .await
        .expect("open");
    assert_eq!(
        storage::integrity_check(&pool).await.expect("integrity"),
        ["ok"]
    );
}

#[tokio::test]
async fn restore_refuses_while_installation_is_locked() {
    let (_tmp, data_dir) = temp_data_dir();
    initialized_db(&data_dir).await;
    let report = backup::run(&data_dir, backup::DEFAULT_KEEP)
        .await
        .expect("backup");

    let _held = ServeLock::acquire(&data_dir).expect("lock");
    let refused = restore::run(&data_dir, &report.snapshot).await;
    assert!(refused.is_err(), "restore must refuse while locked");
}

#[tokio::test]
async fn serve_lock_is_exclusive() {
    let (_tmp, data_dir) = temp_data_dir();
    let first = ServeLock::acquire(&data_dir).expect("first lock");
    assert!(ServeLock::acquire(&data_dir).is_err());
    drop(first);
    assert!(ServeLock::acquire(&data_dir).is_ok());
}
