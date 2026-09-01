# Trainee Packet Format

Everything retained about one enrollment, as one verifiable archive
(ADR 0015; #44 decision 3; `docs/records-integrity.md`).
`consolebook-server/src/trainee_packet.rs` produces packets and
`consolebook-server/src/export_verify.rs` verifies them;
`tests/trainee_packet.rs` proves the contents, determinism, and every
verification finding. This document is normative: the implementation
follows it, not the other way around.

A packet is a record export (`docs/formats/record-export.md`) plus what
the record bytes do not carry. Its units are byte-identical to record-
export units, so everything that document says about `record.json`,
unit manifests, determinism, and unit verification holds here without
restatement. What a packet adds is a different root manifest and four
typed documents.

## Container

The container rules of the record export format apply unchanged: a ZIP
archive, stored entries, files only, ASCII names, the export instant as
every entry's modification time, `0644`, nothing the manifest does not
name. Entry order is the packet manifest, then units in ascending
(`record_id`, `version_number`) order, then documents in ascending path
order.

```text
manifest.json                       packet manifest
records/{record_id}/v{n}/record.json     canonical record bytes (as the record export)
records/{record_id}/v{n}/manifest.json   unit manifest (as the record export)
packet/acknowledgments.json         every acknowledgment of every version
packet/amendments.json              every amendment
packet/enrollment.json              the enrollment's own history
packet/signoffs.json                the full task signoff history
```

## Packet manifest (`manifest.json`)

```json
{
  "documents": [
    { "kind": "acknowledgments", "path": "packet/acknowledgments.json", "sha256": "…" },
    { "kind": "amendments", "path": "packet/amendments.json", "sha256": "…" },
    { "kind": "enrollment", "path": "packet/enrollment.json", "sha256": "…" },
    { "kind": "signoffs", "path": "packet/signoffs.json", "sha256": "…" }
  ],
  "enrollment": {
    "id": 4,
    "program": { "name": "…", "version_number": 1, "label": "…" },
    "trainee": { "id": 7, "username": "…", "display_name": "…", "employee_id": "…", "title": "…" }
  },
  "exported_at": 1788289200,
  "format": "consolebook-trainee-packet",
  "format_version": 1,
  "installation_id": "…",
  "units": [ … ]
}
```

| Member | Type | Meaning |
| --- | --- | --- |
| `format` | string | Always `consolebook-trainee-packet` |
| `format_version` | integer | `1`; bumped by any change to the manifest or a document's shape, and by any new document kind |
| `installation_id` | string | The exporting installation's identity |
| `exported_at` | integer | The export instant, UTC unix seconds; identical in every unit manifest |
| `enrollment` | object | The enrollment's identity, its pinned program version, and its trainee, each *as presented at export* |
| `units` | array | Every retained version of every record of the enrollment, exactly as the record export lists units; may be empty; every unit's predecessor is carried |
| `documents` | array | The four documents, ascending by `path`, each with the SHA-256 of its bytes (lowercase hex) |

The manifest is canonical JSON under the record format's JCS subset,
as every manifest and document in the packet is.

### `documents[].kind`

Version 1 defines exactly four kinds, each present exactly once, at the
path `packet/{kind}.json`: `acknowledgments`, `amendments`,
`enrollment`, `signoffs`. Rendered PDFs (#44 decision 2) arrive as a
new kind with a format-version bump; a version-1 verifier refuses a
kind it does not know rather than skipping it.

## Documents

Every document is canonical JSON with a typed shape: every member
present (nullable members as `null`, never absent), no member the shape
does not name, every `kind` one of a closed vocabulary — the set the
stored table constrains — so a value outside it fails the shape check
rather than passing through as text, and the order and cross-member
rules each shape states below, which mirror the stored tables' own
constraints. Every person a document names is an object
`{id, display_name}`: the stable user identity beside the name shown
(`docs/records-integrity.md`: stable ids preserve identity, snapshots
preserve what the record said). The name is the stored snapshot for
acknowledgments, amendments, and signoffs, and the export-time name for
enrollment and phase events, and it is never empty: every stored
snapshot and every user's name is constrained non-empty.

### `packet/enrollment.json`

```json
{
  "enrolled_at": 1780000000,
  "enrollment_id": 4,
  "events": [
    { "actor": { "display_name": "…", "id": 3 }, "event_id": 12, "from_version": null,
      "kind": "withdraw", "occurred_at": 1780500000, "reason": "…", "to_version": null }
  ],
  "phase_events": [
    { "actor": { "display_name": "…", "id": 3 }, "effective_at": 1780100000, "event_id": 7,
      "from_phase": null, "kind": "advance", "reason": "", "recorded_at": 1780100000,
      "to_phase": "Phase One" }
  ]
}
```

`events` are the enrollment lifecycle events in recorded order —
strictly ascending `event_id`, the installation's row identity — with
`kind` one of `version_change`, `withdraw`, `complete`, `reinstate`.
`from_version` and `to_version` are `{version_number, label}` objects
for a version change, which names two different versions and gives a
reason, and `null` for every other kind. `phase_events` are the phase
history in effective order — strictly ascending (`effective_at`,
`event_id`) — with `kind` one of `advance`, `return`, `restart`,
`pause`, `resume`, `complete`, phases by name: an advance names
`to_phase`, a return or restart names both phases, and a pause, resume,
or completion names only `from_phase`, the phase it happened in; nothing
is effective after it was recorded. `actor` is `null` when no person
recorded the event. Actor names here are resolved at export, unlike the
snapshots inside records and the other documents; the packet manifest's
`exported_at` is the instant they describe.

### `packet/acknowledgments.json`

An array, strictly ascending by (`record_id`, `version_number`) — one
acknowledgment binds one version — of `{record_id, version_number,
kind, response, user, recorded_by, recorded_at}`: every acknowledgment
bound to any version the packet carries, from the stored snapshots.
`kind` is one of the acknowledgment kinds (`docs/domain-model.md`).
`user` is the person bound, always the packet's trainee. `recorded_by`
is who spoke: the trainee themselves for `acknowledged`,
`acknowledged_with_response`, and `refused`, and never the trainee for
`supervisor_attested_refusal` and `unavailable`. A plain `acknowledged`
carries an empty `response`; every other kind explains itself with a
non-blank one.

### `packet/amendments.json`

An array, strictly ascending by (`record_id`,
`predecessor_version_number`) — a version is amended at most once — of
`{record_id, predecessor_version_number, successor_version_number,
reason, opened_by, opened_at}`. `reason` is non-blank.
`successor_version_number` is `null` while the correction is still in
progress; a sealed correction names the version it produced. The
document and the carried lineage agree both ways: a named successor is
the carried version whose predecessor is the amended version, an
amendment in progress has no carried successor, and every carried
version that succeeds another has its amendment recorded.

### `packet/signoffs.json`

An array in recorded order — strictly ascending `signoff_id`, the
installation's row identity — of `{signoff_id, task_id,
competency_category, competency_name, prompt, kind, reason, signed_by,
signed_at}`: every task signoff row, first signoffs and overrides alike,
so the history is complete (ADR 0013). `kind` is one of `observed`,
`demonstrated`, `revoked`. Any signoff after the first for a task
supersedes it and records a non-blank `reason`, and a revocation never
opens a task's history: it has something to revoke.

## Determinism

A packet is a pure function of the enrollment's rows and the export
instant: the same enrollment packed at the same instant is byte-
identical, for the same reasons the record export is. The producing
installation reads the enrollment, its units, and every document inside
one database transaction, so a packet describes one committed state: a
finalization, acknowledgment, or signoff that lands while a packet is
being produced is wholly in it or wholly absent, never listed by the
manifest and missing from a document.

## Verification

The record export's verifier reads the root manifest's `format` and
applies this format's checks when it names a packet. Its verdict has
the same honest meaning: consistency with the stated fingerprints,
never tamper-proofing or provenance. What it does not check (container
metadata) is the same as for the record export.

Archive checks are those of the record export, with three differences:
the unit list may be empty (an enrollment with no finalized version
still has a history to leave with); the scope checked is the packet's
trainee — every unit's envelope must name `enrollment.trainee.id`, or
the unit is *outside scope*; and every unit's predecessor is carried —
a packet carries every retained version, so a version whose predecessor
is absent is a hole in the lineage (*predecessor not carried*), not a
scope choice as it is for a record export.

Unit checks are exactly the record export's.

Document checks:

1. the manifest lists each of the four kinds exactly once, ascending by
   path, each at its derived path;
2. every listed document exists, and the SHA-256 of its bytes equals
   the manifest's `sha256`, itself 64 lowercase hex characters;
3. the bytes are canonical JSON and parse as the kind's shape — every
   named member, typed, closed kinds included, and no other;
4. the rows are in the kind's mandated order — strictly ascending by
   the key its shape names above — so a reordered or duplicated row is
   a finding;
5. the cross-member rules each shape states above hold: every named
   person has a non-empty name, an acknowledgment's response and
   speaker match its kind and its `user`
   is the packet's trainee, an amendment gives a reason, a lifecycle
   event carries version references exactly when it is a version
   change, a phase event names the phases its kind requires and is not
   effective after it was recorded, a signoff override records its
   reason, and a revocation is never a task's first signoff;
6. every acknowledgment names a (`record_id`, `version_number`) the
   packet carries, and every amendment's predecessor, and successor
   where present, does too — a reference the packet cannot resolve is
   a finding;
7. the amendments agree with the carried lineage both ways, as their
   shape states above — an amendment contradicting the carried
   successor, or a carried successor without its amendment, is a
   finding; and
8. `enrollment.json` names the manifest's `enrollment.id`.

`consolebook-server export verify <packet>` prints one line per unit
and per document and exits non-zero unless the verdict is `verified`.

## Authorization (behavior of the producing installation)

A packet is produced for whoever may read the enrollment's training
history, for the trainee themselves on their own enrollment
(`view_own_records`), and for `export_records` holders. An unknown
enrollment is refused. Every packet is audited
(`trainee_packet_exported`) with actor and trainee and never with
content.

## What the packet does not carry

- **Sessions and drafts.** The sessions a record covered are inside
  that record's bytes; sessions and drafts that produced no finalized
  version are operational data, not records.
- **Rendered PDFs.** They arrive with #44's slice 4 as a further
  document kind under a new format version.
- **Exporter identity.** As for the record export, that is the audit
  trail's business.
