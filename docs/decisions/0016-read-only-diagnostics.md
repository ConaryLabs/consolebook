# ADR 0016: Read-only SQLite diagnostics

- **Status:** Accepted
- **Date:** 2026-09-05
- **Issue:** [#56](https://github.com/FieldmouseWorks/consolebook/issues/56)
- **Amends:** [ADR 0003](0003-sqlite-connection-invariants.md)

## Context

`doctor` used the writable backup connection options, which set journal mode
to WAL before checking it. Diagnosing a DELETE-mode database therefore changed
its header and reported the newly applied mode as healthy. Disabling creation
and migrations alone does not make a connection read-only.

SQLite also uses filesystem state to coordinate WAL readers. A promise that a
live diagnostic never touches any file in the data directory is too broad for
ordinary SQLite locking. We need to distinguish retained database content from
WAL coordination, and connection settings from persisted database settings.

## Decision

`storage::open_diagnostic` owns an explicit read-only options path with database
creation disabled and no journal-mode setter. SQLx leaves journal mode unset by
default. Startup and backup continue using their existing writable options;
diagnostics never migrate, repair, checkpoint, or change journal mode.

The diagnostic pool has one connection. It sets the same connection-local
foreign-key, synchronous, and busy-timeout values as startup and shares
`verify_invariants`, which reads all four values on one acquired connection.
The report labels their scope:

| PRAGMA | What the diagnostic observes |
| --- | --- |
| `journal_mode` | Database journaling mode; WAL persists across connections. A newly opened non-WAL database normally reports DELETE; this does not recover another connection's transient TRUNCATE, PERSIST, MEMORY, or OFF setting. A non-WAL result fails the expected-WAL check. |
| `foreign_keys` | Enforcement enabled on this diagnostic connection. It cannot inspect another connection's enforcement. |
| `synchronous` | NORMAL on this diagnostic connection. It does not measure another connection's durability setting. |
| `busy_timeout` | 5000 ms on this diagnostic connection. It does not inspect another process's timeout. |

The read-only SQLite open flag rejects database writes even when the operating
system user has write permission. The diagnostic does not alter existing main
database or WAL content. Normal SQLite locking remains enabled so committed
changes still in a live WAL are visible.

### WAL sidecars and read-only filesystems

SQLite may create absent `-wal` and `-shm` sidecars in a writable directory and
update shared-memory reader coordination. These are an explicit exception to
the former literal "never writes to the data directory" description. The
diagnostic does not append transactions, checkpoint, remove sidecars itself,
or rewrite the main database. A live writer may of course change its own files
while diagnosis runs; several diagnostic queries are not one installation-wide
snapshot.

On read-only storage, WAL diagnosis requires existing readable, usable sidecars.
If SQLite needs to create or recover coordination state and cannot, `doctor`
reports failure and leaves repair to the operator. A cleanly stopped WAL
installation without sidecars can be diagnosed in a writable directory; the
same installation on read-only storage may fail. File readability alone is
not proof that a WAL index is usable.

Never use `immutable=1`, disable locking, or fall back to a raw file copy to
force success. An installation can be live or retain uncheckpointed WAL
commits; bypassing SQLite's concurrency protocol cannot establish a correct
view. Do not automatically change mode or remove sidecars to accommodate
read-only media.

These semantics follow SQLite's [read-only WAL rules](https://www.sqlite.org/wal.html#read_only_databases)
and [PRAGMA documentation](https://www.sqlite.org/pragma.html).

## Consequences and proof

`doctor` can honestly report a journal mismatch while preserving the database.
Its connection-local checks validate its own configured connection, not the
configuration of a separately running server. No schema, backup format, or
startup durability setting changes.

`tests/doctor_read_only.rs` covers absent databases, a migrated DELETE-mode
database remaining byte-identical, rejected data/schema/journal writes,
stopped WAL diagnosis, and reading an identity committed only to a live WAL
without changing main database or WAL bytes. Unix permission tests cover a
read-only directory with usable live sidecars and failure without sidecars;
they require an unprivileged runner and assert that writes are denied. These
permission tests are not a mounted read-only filesystem or crash-recovery drill.
Existing operable-shell and backup/restore tests cover the unchanged writable
paths.
