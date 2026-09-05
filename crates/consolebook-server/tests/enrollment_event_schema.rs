//! Enrollment-event shape is enforced by migrations, including upgrades from
//! the schema that admitted a single version reference on other event kinds.
//! All rows and names are invented; no retained history is rewritten to seed tests.

use std::borrow::Cow;
use std::path::Path;

use consolebook_server::{export_verify, storage, trainee_packet};
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Connection, SqlitePool};

const LEGACY_VERSION: i64 = 13;
const SHAPE_VERSION: i64 = 14;
const EXPORTED_AT: i64 = 1_788_289_200;

async fn connect(path: &Path) -> SqliteConnection {
    SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal),
    )
    .await
    .expect("fixture connection")
}

async fn legacy_database(path: &Path) -> SqliteConnection {
    let mut connection = connect(path).await;
    let legacy = sqlx::migrate::Migrator {
        migrations: Cow::Owned(
            storage::MIGRATOR
                .iter()
                .filter(|migration| migration.version <= LEGACY_VERSION)
                .cloned()
                .collect(),
        ),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    legacy
        .run(&mut connection)
        .await
        .expect("legacy migrations");
    seed(&mut connection).await;
    connection
}

async fn seed(connection: &mut SqliteConnection) {
    sqlx::raw_sql(
        "INSERT INTO instance (id, installation_id, created_at_utc)
         VALUES (1, 'invented-schema-installation', '2026-09-01T00:00:00Z');
         INSERT INTO user (id, username, display_name, password_hash, created_at)
         VALUES (1, 'casey.schema', 'Casey Example', 'unused-invented-fixture', 1);
         INSERT INTO capability_grant (user_id, capability, granted_at)
         VALUES (1, 'export_records', 1);
         INSERT INTO program (id, name, created_at) VALUES (1, 'Invented Schema Program', 1);
         INSERT INTO program_version
           (id, program_id, version_number, label, name, description, created_at)
         VALUES (1, 1, 1, 'rev A', 'Invented Schema Program', '', 1),
                (2, 1, 2, 'rev B', 'Invented Schema Program', '', 1),
                (3, 1, 3, 'draft C', 'Invented Schema Program', '', 1);
         INSERT INTO phase (id, program_version_id, name, description, presentation_number)
         VALUES (1, 1, 'Phase One', '', 1), (2, 2, 'Phase Two', '', 1);
         UPDATE program_version SET published_at = 1 WHERE id IN (1, 2);
         INSERT INTO enrollment (id, user_id, program_version_id, enrolled_at)
         VALUES (1, 1, 1, 1);",
    )
    .execute(connection)
    .await
    .expect("invented base rows");
}

async fn insert_event(
    connection: &mut SqliteConnection,
    kind: &str,
    from: Option<i64>,
    to: Option<i64>,
) -> sqlx::Result<sqlx::sqlite::SqliteQueryResult> {
    sqlx::query(
        "INSERT INTO enrollment_event
         (enrollment_id, kind, occurred_at, actor_user_id, reason,
          from_program_version_id, to_program_version_id)
         VALUES (1, ?1, 20, 1, 'Invented lifecycle event.', ?2, ?3)",
    )
    .bind(kind)
    .bind(from)
    .bind(to)
    .execute(connection)
    .await
}

async fn reference_matrix(connection: &mut SqliteConnection) {
    for kind in ["version_change", "withdraw", "complete", "reinstate"] {
        for (from, to) in [
            (None, None),
            (Some(1), None),
            (None, Some(2)),
            (Some(1), Some(2)),
        ] {
            let valid = if kind == "version_change" {
                from.is_some() && to.is_some()
            } else {
                from.is_none() && to.is_none()
            };
            let result = insert_event(connection, kind, from, to).await;
            assert_eq!(
                result.is_ok(),
                valid,
                "{kind} from={from:?} to={to:?}: {result:?}"
            );
            if kind != "version_change" && !valid {
                let message = result.expect_err("shape refused").to_string();
                assert!(
                    message.contains("enrollment events name both versions"),
                    "{message}"
                );
            }
        }
    }
}

#[tokio::test]
async fn fresh_schema_enforces_the_complete_reference_matrix() {
    let tmp = tempfile::tempdir().expect("scratch");
    let mut connection = connect(&tmp.path().join("consolebook.db")).await;
    storage::MIGRATOR
        .run(&mut connection)
        .await
        .expect("migrate");
    seed(&mut connection).await;
    reference_matrix(&mut connection).await;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM enrollment_event")
        .fetch_one(&mut connection)
        .await
        .expect("count accepted rows");
    assert_eq!(count, 4, "only the four valid shapes append rows");
    connection.close().await.expect("close");
}

type EventRow = (
    i64,
    i64,
    String,
    i64,
    Option<i64>,
    String,
    Option<i64>,
    Option<i64>,
);
type PhaseRow = (
    i64,
    i64,
    String,
    Option<i64>,
    Option<i64>,
    i64,
    i64,
    Option<i64>,
    String,
    Option<i64>,
);
type SchemaRow = (String, String, String, String);

#[derive(Debug, PartialEq, Eq)]
struct History {
    events: Vec<EventRow>,
    phases: Vec<PhaseRow>,
    pin: i64,
}

async fn history(connection: &mut SqliteConnection) -> History {
    History {
        events: sqlx::query_as(
            "SELECT id, enrollment_id, kind, occurred_at, actor_user_id, reason,
                    from_program_version_id, to_program_version_id
             FROM enrollment_event ORDER BY id",
        )
        .fetch_all(&mut *connection)
        .await
        .expect("event history"),
        phases: sqlx::query_as(
            "SELECT id, enrollment_id, kind, from_phase_id, to_phase_id, effective_at,
                    recorded_at, actor_user_id, reason, version_change_event_id
             FROM phase_event ORDER BY id",
        )
        .fetch_all(&mut *connection)
        .await
        .expect("phase history"),
        pin: sqlx::query_scalar("SELECT program_version_id FROM enrollment WHERE id = 1")
            .fetch_one(connection)
            .await
            .expect("pin"),
    }
}

async fn schema(connection: &mut SqliteConnection) -> Vec<SchemaRow> {
    sqlx::query_as(
        "SELECT type, name, tbl_name, sql FROM sqlite_schema
         WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' ORDER BY type, name",
    )
    .fetch_all(connection)
    .await
    .expect("schema objects")
}

async fn seed_history(connection: &mut SqliteConnection) {
    sqlx::raw_sql(
        "INSERT INTO phase_event
           (id, enrollment_id, kind, to_phase_id, effective_at, recorded_at, actor_user_id, reason)
         VALUES (100, 1, 'advance', 1, 10, 10, 1, 'Invented original phase.');
         INSERT INTO enrollment_event (id, enrollment_id, kind, occurred_at, actor_user_id, reason)
         VALUES (200, 1, 'withdraw', 11, 1, 'Invented withdrawal.'),
                (201, 1, 'reinstate', 12, 1, 'Invented reinstatement.');
         INSERT INTO enrollment_event
           (id, enrollment_id, kind, occurred_at, actor_user_id, reason,
            from_program_version_id, to_program_version_id)
         VALUES (202, 1, 'version_change', 20, 1, 'Invented revision.', 1, 2);
         UPDATE enrollment SET program_version_id = 2 WHERE id = 1;
         INSERT INTO phase_event
           (id, enrollment_id, kind, to_phase_id, effective_at, recorded_at,
            actor_user_id, reason, version_change_event_id)
         VALUES (101, 1, 'advance', 2, 20, 21, 1, 'Invented new phase.', 202);
         INSERT INTO enrollment_event (id, enrollment_id, kind, occurred_at, actor_user_id, reason)
         VALUES (203, 1, 'complete', 30, 1, 'Invented completion.');",
    )
    .execute(connection)
    .await
    .expect("append legacy history");
}

async fn packet(pool: &SqlitePool) -> Vec<u8> {
    let packet = trainee_packet::export_at(pool, 1, 1, EXPORTED_AT)
        .await
        .expect("export")
        .expect("authorized")
        .bytes;
    let report = export_verify::verify_archive(&packet);
    assert!(report.verified(), "{report:?}");
    packet
}

#[tokio::test]
async fn upgrade_preserves_history_references_schema_objects_and_packet_bytes() {
    let tmp = tempfile::tempdir().expect("scratch");
    let path = tmp.path().join("consolebook.db");
    let mut connection = legacy_database(&path).await;
    seed_history(&mut connection).await;
    let before = history(&mut connection).await;
    let before_schema = schema(&mut connection).await;
    connection.close().await.expect("close legacy fixture");
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .foreign_keys(true),
        )
        .await
        .expect("legacy export connection");
    let before_packet = packet(&pool).await;
    pool.close().await;

    // Exercise the real startup path, including migration checksum validation.
    let pool = storage::open(&path).await.expect("upgrade at startup");
    let after_packet = packet(&pool).await;
    assert_eq!(
        before_packet, after_packet,
        "fixed-instant packets are byte-identical"
    );
    let mut connection = pool.acquire().await.expect("inspect upgraded storage");
    assert_eq!(before, history(&mut connection).await);
    let after_schema = schema(&mut connection).await;
    let retained_schema: Vec<_> = after_schema
        .into_iter()
        .filter(|(_, name, _, _)| name != "enrollment_event_version_reference_shape")
        .collect();
    assert_eq!(
        before_schema, retained_schema,
        "existing tables, indexes, and triggers stay intact"
    );
    let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&mut *connection)
        .await
        .expect("foreign keys");
    assert_eq!(foreign_keys, 1);
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&mut *connection)
            .await
            .expect("foreign key check")
            .is_empty()
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(&mut *connection)
            .await
            .expect("integrity"),
        "ok"
    );
    for statement in [
        "UPDATE enrollment_event SET from_program_version_id = 1 WHERE id = 200",
        "DELETE FROM enrollment_event WHERE id = 200",
        "UPDATE enrollment SET program_version_id = 1 WHERE id = 1",
        "INSERT INTO phase_event (enrollment_id, kind, to_phase_id, effective_at, recorded_at, reason)
         VALUES (1, 'advance', 2, 31, 31, '')",
        "INSERT INTO enrollment_event
         (enrollment_id, kind, occurred_at, reason, from_program_version_id, to_program_version_id)
         VALUES (1, 'version_change', 31, '', 2, 1)",
    ] {
        assert!(sqlx::query(statement).execute(&mut *connection).await.is_err(), "{statement}");
    }
    // Existing target-publication, different-version, and reason rules survive.
    assert!(
        insert_event(&mut connection, "version_change", Some(2), Some(2))
            .await
            .is_err()
    );
    assert!(
        insert_event(&mut connection, "version_change", Some(2), Some(999))
            .await
            .is_err()
    );
    let draft_target = insert_event(&mut connection, "version_change", Some(2), Some(3))
        .await
        .expect_err("draft targets remain refused");
    assert!(
        draft_target
            .to_string()
            .contains("published program versions"),
        "{draft_target}"
    );
    reference_matrix(&mut connection).await;
    drop(connection);
    storage::MIGRATOR
        .run(&pool)
        .await
        .expect("already upgraded is idempotent");
    pool.close().await;
}

#[tokio::test]
async fn malformed_legacy_rows_refuse_upgrade_without_rewriting_history() {
    for kind in ["withdraw", "complete", "reinstate"] {
        for (from, to) in [(Some(1), None), (None, Some(2))] {
            let tmp = tempfile::tempdir().expect("scratch");
            let mut connection = legacy_database(&tmp.path().join("consolebook.db")).await;
            seed_history(&mut connection).await;
            // This is the exact #51 reproduction: the old schema accepts it.
            insert_event(&mut connection, kind, from, to)
                .await
                .expect("legacy loophole");
            let before = history(&mut connection).await;
            let before_schema = schema(&mut connection).await;
            for _ in 0..2 {
                let error = storage::MIGRATOR
                    .run(&mut connection)
                    .await
                    .expect_err("refuse malformed history");
                assert!(
                    error
                        .to_string()
                        .contains("enrollment_event_legacy_version_references_invalid"),
                    "{error}"
                );
                assert_eq!(before, history(&mut connection).await);
                assert_eq!(before_schema, schema(&mut connection).await);
                let guard_count: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM sqlite_temp_schema WHERE name = 'enrollment_event_shape_upgrade_guard'",
                ).fetch_one(&mut connection).await.expect("no temporary guard remains");
                assert_eq!(guard_count, 0);
                let latest: i64 = sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations")
                    .fetch_one(&mut connection)
                    .await
                    .expect("migration ledger");
                assert_eq!(latest, LEGACY_VERSION);
            }
            connection.close().await.expect("close");
        }
    }
}

#[tokio::test]
async fn startup_fails_closed_on_malformed_legacy_history() {
    let tmp = tempfile::tempdir().expect("scratch");
    let path = tmp.path().join("consolebook.db");
    let mut connection = legacy_database(&path).await;
    insert_event(&mut connection, "withdraw", Some(1), None)
        .await
        .expect("legacy loophole");
    let before = history(&mut connection).await;
    connection.close().await.expect("stop fixture");
    let error = storage::open(&path).await.expect_err("startup must stop");
    assert!(
        format!("{error:#}").contains("enrollment_event_legacy_version_references_invalid"),
        "{error:#}"
    );
    let pool = storage::open_diagnostic(&path)
        .await
        .expect("read-only inspection");
    let mut connection = pool.acquire().await.expect("inspect");
    assert_eq!(before, history(&mut connection).await);
    let applied: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations WHERE version = ?1")
            .bind(SHAPE_VERSION)
            .fetch_one(&mut *connection)
            .await
            .expect("ledger");
    assert_eq!(applied, 0);
    drop(connection);
    pool.close().await;
}
