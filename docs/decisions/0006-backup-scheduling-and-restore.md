# ADR 0006: Backup scheduling, retention, and restore

- **Status:** Accepted
- **Date:** 2026-08-28

## Context

ADR 0003 fixed the backup mechanism — validated `VACUUM INTO` snapshots
with explicit durability — and deferred scheduling and retention. Milestone
1 requires backups that happen without operator memory and a restore path
that is a tested product workflow.

## Decision

### Scheduling

`serve` runs one background task that keeps the newest snapshot no older
than the configured interval (default 24 hours,
`--backup-interval-hours` / `CONSOLEBOOK_BACKUP_INTERVAL_HOURS`, minimum
1). The decision is a pure, tested function over the newest snapshot's
mtime; the task re-evaluates every minute, so missed ticks and slow
backups self-correct and a fresh installation gets its first snapshot
within a minute of starting. Failures are logged and retried on schedule;
they never crash the server. There is deliberately no flag to disable
automatic backups.

### Retention

After each validated snapshot, the oldest snapshots beyond the retention
count (default 14, `--backup-keep` / `CONSOLEBOOK_BACKUP_KEEP`, minimum 1)
are pruned. Pruning runs only after a new snapshot validates and never
removes the last remaining snapshot regardless of configuration.
Pre-restore snapshots live in the same directory and count toward
retention.

### The single-server lock

`serve` holds an exclusive OS file lock on `instance/serve.lock` for its
lifetime. Two servers cannot share a data directory, and `restore` refuses
to run while the lock is held — a check, not a guess. The lock releases on
process exit, crashes included.

### Restore

`consolebook restore <snapshot>` is the product workflow ADR 0003
promised:

1. validate the incoming snapshot as its own database;
2. acquire the serve lock (refusing while a server runs) and hold it for
   the duration;
3. set the current database aside into `backups/` — as a validated
   pre-restore snapshot when it is healthy, or as raw bytes moved aside
   when it is too damaged to snapshot (a corrupt database must not block
   its own replacement);
4. remove stale WAL sidecar files, copy the snapshot into place, fsync;
5. prove the result exactly as startup would (connection invariants,
   migrations, integrity check, instance identity); and
6. record a `restore_completed` audit event in the restored database.

Backups record `backup_completed` the same way; the domain model lists
backup and restore among audited actions.

## Consequences

### Positive

- a running installation always converges to a recent validated snapshot;
- recovery from corruption is one command with the mistake path itself
  recoverable;
- restore cannot race a running server; and
- the scheduling decision and every restore path are integration-tested,
  including the destroyed-database case.

### Costs

- `doctor` warns on staleness against the default interval, not a
  per-installation configured value (configuration is per-invocation
  flags for now; revisit when persistent instance settings exist);
- snapshots are plain SQLite files — encryption at rest for the backups
  directory needs its own decision before pilots handle it via disk
  encryption; and
- off-host replication of `backups/` remains the operator's
  responsibility, documented rather than automated.

## Rejected alternatives

- **Cron/systemd-timer driven backups:** breaks "one executable, no
  external services" and fails silently when the timer is missing.
- **Configurable backup disablement:** recovery is a product feature
  (PRINCIPLES.md 8); an installation that quietly stops backing up is a
  worse failure than a wasted daily snapshot.
- **In-place restore over the live file:** replacing a database under a
  running server invites torn state; the lock makes the precondition
  enforceable instead of documented.
