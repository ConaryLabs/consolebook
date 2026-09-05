# ADR 0018: Enrollment-event version-reference shape

- **Status:** Accepted
- **Date:** 2026-09-05
- **Issue:** [#51](https://github.com/FieldmouseWorks/consolebook/issues/51)
- **Amends:** [ADR 0008](0008-session-draft-and-attribution-model.md)

## Context

Only a version-change event names program versions. Migration 0006 compared
`kind = 'version_change'` with both references being non-null. For other kinds,
that comparison requires at least one null reference, allowing a withdrawal,
completion, or reinstatement with exactly one version reference. The domain
service already writes both references for a version change and neither for
other kinds, and the packet verifier already refuses the malformed shape.
The database boundary must enforce the same rule.

## Decision

Forward migration 0014 adds `enrollment_event_version_reference_shape`, a
`BEFORE INSERT` trigger enforcing:

```sql
CASE kind
    WHEN 'version_change'
        THEN from_program_version_id IS NOT NULL AND to_program_version_id IS NOT NULL
    ELSE from_program_version_id IS NULL AND to_program_version_id IS NULL
END
```

The existing unconditional no-update trigger covers later changes. The
original CHECK constraints still enforce different version identities and a
reason for a version change; target-publication and append-only triggers
remain intact. No service, packet shape, packet format version, or persisted
event value changes. Migration 0006 and its checksum remain unchanged.

### Upgrade over existing history

Before installing the new trigger, migration 0014 evaluates the full shape
against existing events. A malformed row fails a temporary guard's named
constraint, `enrollment_event_legacy_version_references_invalid`. SQLx applies
the script and its migration-ledger entry in one transaction, so failure
leaves the history, existing schema, and ledger at their prior state; the
temporary guard rolls back too. Startup reports the migration failure and
does not serve the installation.

The migration does not clear the reference, infer a new event kind, delete a
row, or mark the migration applied despite malformed history. The operator
must preserve the source installation and resolve its history through a
separately authorized repair decision. This migration provides no automatic
repair command. Retrying without resolving the malformed history fails again.
Valid installations, including those with phase events referencing recorded
version-change IDs, upgrade without copying their event tables.

### Why a trigger instead of rebuilding the table

The issue proposed replacing the CHECK by rebuilding `enrollment_event`.
This is unnecessary for an append-only table whose database contract already
uses triggers. A trigger adds the missing enforcement while preserving table
identity, incoming `phase_event` foreign keys, indexes, and existing triggers.
The temporary guard supplies the legacy-row validation that adding a trigger
alone would omit.

SQLite's [generalized table-rebuild procedure](https://www.sqlite.org/lang_altertable.html#making_other_kinds_of_table_schema_changes)
requires care with incoming references and foreign-key enforcement. SQLx's
SQLite migration implementation wraps each script in a transaction, and
SQLite [cannot toggle foreign-key enforcement inside that transaction](https://www.sqlite.org/foreignkeys.html#fk_enable).
We retain the normal migration runner and keep foreign keys enabled throughout.

## Proof and consequences

`tests/enrollment_event_schema.rs` owns direct storage and upgrade proof:

- all four event kinds against all four null/non-null reference combinations,
  on both fresh and upgraded schemas;
- an installation migrated only through 0013 reproduces each of the six
  one-reference loopholes, then refuses 0014 without changing event or phase
  rows, schema objects, or the applied-migration version;
- repeated failure leaves no temporary guard behind, and the real startup
  path fails closed on malformed legacy history;
- a valid upgrade preserves event IDs, phase epoch references, enrollment
  pins, existing tables/indexes/triggers, and fixed-instant packet bytes;
- foreign-key and integrity checks pass, prior write restrictions still
  refuse invalid operations, and rerunning the migrator is idempotent.

The upgrade scans the event stream until it finds an invalid row or reaches
the end. This is validation cost, not a table rewrite. The predicate appears
in the historical CHECK, migration guard, insert trigger, and packet verifier;
the schema matrix and packet tests keep their intended contract aligned.
