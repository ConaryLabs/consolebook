# ADR 0003: SQLite connection invariants and backup mechanics

- **Status:** Accepted
- **Date:** 2026-08-28
- **Amended by:** [ADR 0016](0016-read-only-diagnostics.md), which separates
  diagnostic connection options and defines WAL-sidecar and PRAGMA scope.

## Context

ADR 0001 committed Consolebook to one SQLite database per installation and
deferred durability settings to a follow-up decision. Training records are
consequential personnel records; silent misconfiguration of the database is
not an acceptable failure mode. The architecture document requires every
connection to come from one explicit options object whose guarantees are
verified, not assumed.

## Decision

Every writable connection is created from a single options object
(`storage::connect_options`) that sets:

- `foreign_keys = ON` — referential integrity is enforced by the database,
  not by application discipline;
- `journal_mode = WAL` — readers do not block the writer, and the write-ahead
  log gives crash consistency suitable for a long-running service;
- `synchronous = NORMAL` — with WAL, `NORMAL` syncs at checkpoint boundaries;
  a power loss can lose the most recent transactions but cannot corrupt the
  database. `FULL` remains an option if future record-finalization semantics
  demand a durability receipt per commit; and
- `busy_timeout = 5000 ms` — writers wait bounded time instead of failing
  immediately or hanging forever.

Startup re-reads the four PRAGMA values and **fails closed** if any does not
hold. `consolebook doctor` reports the same checks through the read-only path
in ADR 0016: it never creates or writes the database and never runs migrations.
SQLite may create WAL sidecars and update shared-memory coordination state.

Migrations are application-owned, embedded in the executable
(`sqlx::migrate!`), and applied on startup. Diagnostic and backup paths open
the database with `create_if_missing` disabled.

Backups are consistent `VACUUM INTO` snapshots taken while the database is
live, then validated (`PRAGMA integrity_check` on the snapshot as its own
database) and made durable with explicit fsync of the file and its
directory. A snapshot that fails validation is deleted rather than left
looking like a usable backup.

## Consequences

### Positive

- misconfigured deployments stop at startup instead of running quietly;
- `doctor` and startup share one verification code path;
- backups are consistent under concurrent use and are validated before they
  count; and
- the invariants are integration-tested.

### Costs

- `synchronous = NORMAL` accepts bounded loss of the newest transactions on
  power failure; the record-finalization design (Milestone 4) must revisit
  whether finalization requires `FULL` or an explicit checkpoint; and
- `VACUUM INTO` rewrites the whole database per snapshot, which is fine at
  expected sizes but worth re-measuring if databases grow large.

## Rejected alternatives

- **`journal_mode = DELETE` (default):** blocks readers during writes and
  offers no benefit for a long-running service.
- **File-copy backups:** copying a live SQLite database (plus `-wal`/`-shm`)
  is not consistent; `VACUUM INTO` produces a self-contained snapshot.
- **Trusting the options object without verification:** drivers and future
  refactors can silently drop a PRAGMA; verification is cheap and permanent.
