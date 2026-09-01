# Record Export Format

Portable, verifiable exports of finalized evaluation versions (ADR 0014;
`docs/records-integrity.md`; #44 decision 1).
`consolebook-server/src/record_export.rs` produces exports and
`consolebook-server/src/export_verify.rs` verifies them;
`tests/record_export.rs` proves the round trip, determinism, and every
verification finding. This document is normative: the implementation
follows it, not the other way around.

An export never re-serializes a record. The stored canonical bytes of
each version (ADR 0011, ADR 0013) travel byte for byte, beside a
manifest that names them. Verifying an export needs nothing but the
export.

## Vocabulary

- **Unit** — one finalized version as exported: its canonical record
  bytes and its unit manifest.
- **Archive** — the container holding one archive manifest and one or
  more units. Every export, a single version included, is an archive,
  so one reader and one verifier cover every scope.
- **Scope** — what the archive claims to contain: one version, one
  record (every retained version, superseded originals included), one
  enrollment (every finalized version of its records), or the whole
  installation.

## Container

The container is a ZIP archive (APPNOTE 6.3):

- every entry uses compression method 0 (stored); nothing is encrypted;
  ZIP64 structures appear only when the container's size or entry count
  requires them (a record is kilobytes, so no single entry ever does);
- entries are files only — no directory entries — named with forward
  slashes and ASCII characters;
- entry order is the archive manifest first, then units in ascending
  (`record_id`, `version_number`) order, `record.json` before
  `manifest.json` within a unit;
- every entry's modification time is the export instant (DOS time, UTC
  as written, so seconds round down to even), and every entry carries
  Unix permissions `0644`;
- the archive holds nothing but what this document names; anything else
  is a verification finding.

Entry names:

```text
manifest.json                                   archive manifest
records/{record_id}/v{version_number}/record.json    canonical record bytes
records/{record_id}/v{version_number}/manifest.json  unit manifest
```

`{record_id}` and `{version_number}` are the decimal integers of the
unit's identity with no padding. The archive manifest states each
unit's `path` as the directory prefix (`records/12/v2`) and the path
must equal the derived form.

## Determinism

An archive is a pure function of its scope's stored rows and the
export instant: the same scope exported at the same instant produces
identical bytes. Manifests are canonical JSON under the record format's
JCS subset (ADR 0011): UTF-8, members sorted by code point, no
insignificant whitespace, integers only. Record bytes are copied from
storage unchanged.

## `record.json`

The version's `canonical_bytes` exactly as stored — a schema-1 or
schema-2 envelope under `jcs-v1`, presented under its own stored
`record_schema`. The export never upgrades, rewraps, or reformats them.
The envelope's own `record` and `instance` members are what the
verifier cross-checks against the manifests: the bytes commit to their
identity and lineage on their own (ADR 0011).

## Unit manifest (`records/…/manifest.json`)

```json
{
  "chain_hash": "…64 lowercase hex…",
  "content_hash": "…64 lowercase hex…",
  "exported_at": 1756753200,
  "format": "consolebook-record-unit",
  "format_version": 1,
  "installation_id": "…",
  "predecessor_content_hash": null,
  "record_id": 12,
  "record_schema": 2,
  "version_number": 1
}
```

| Member | Type | Meaning |
| --- | --- | --- |
| `format` | string | Always `consolebook-record-unit` |
| `format_version` | integer | `1`; bumped by any change to either manifest's shape |
| `installation_id` | string | The exporting installation's identity (`instance.installation_id`) |
| `record_id` | integer | The record's instance-local identity |
| `version_number` | integer | This version's number within the record (from 1) |
| `record_schema` | integer | The stored envelope schema of `record.json` |
| `content_hash` | string | The stored SHA-256 of `record.json`, lowercase hex |
| `chain_hash` | string | The stored integrity-chain hash (ADR 0011), lowercase hex |
| `predecessor_content_hash` | string or `null` | The prior version's content hash; `null` exactly when `version_number` is 1 |
| `exported_at` | integer | The export instant, UTC unix seconds; identical across the archive |

A unit manifest repeats what the archive manifest says about the unit
so that a unit directory stands on its own; the two must agree.

## Archive manifest (`manifest.json`)

```json
{
  "exported_at": 1756753200,
  "format": "consolebook-record-export",
  "format_version": 1,
  "installation_id": "…",
  "scope": { "kind": "record", "record_id": 12 },
  "units": [
    {
      "chain_hash": "…",
      "content_hash": "…",
      "path": "records/12/v1",
      "predecessor_content_hash": null,
      "record_id": 12,
      "record_schema": 2,
      "version_number": 1
    },
    {
      "chain_hash": "…",
      "content_hash": "…",
      "path": "records/12/v2",
      "predecessor_content_hash": "…",
      "record_id": 12,
      "record_schema": 2,
      "version_number": 2
    }
  ]
}
```

| Member | Type | Meaning |
| --- | --- | --- |
| `format` | string | Always `consolebook-record-export` |
| `format_version` | integer | `1` |
| `installation_id` | string | As in the unit manifest |
| `exported_at` | integer | The export instant, UTC unix seconds |
| `scope` | object | What the archive claims to contain (below) |
| `units` | array | Every unit, ascending by (`record_id`, `version_number`), no duplicates |

`units[]` members are `path`, `record_id`, `version_number`,
`record_schema`, `content_hash`, `chain_hash`, and
`predecessor_content_hash`, with the unit-manifest meanings.

### `scope`

`kind` is one of:

- `version` — with `record_id` and `version_number`; exactly one unit.
- `record` — with `record_id`; every retained version of that record.
- `enrollment` — with `enrollment_id`; every finalized version of every
  record of that enrollment.
- `installation` — no further members; every finalized version the
  installation holds.

The scope is the exporter's statement of intent. Verification checks
the archive against itself — a `version` scope must be exactly its one
unit, a `record` scope must hold only that record's versions — but it
cannot know whether an installation held versions the archive omits.

## Verification

A verifier reads only the archive. It reports, per unit and for the
archive as a whole, and its verdict is `verified` only when every check
below passes. Wording stays honest (ADR 0010, ADR 0011,
`docs/records-integrity.md`): a verified export is internally
consistent with its stated fingerprints. Without the future signed
mode nothing in the archive proves which installation produced it, and
the checks are not tamper-proofing against whoever produced the file.

Archive checks:

1. the container is a readable ZIP archive whose central directory names
   each entry once — a name written twice is a finding, because
   extraction tools disagree on which copy they take;
2. `manifest.json` exists, parses, and carries a known `format` and
   `format_version`;
3. `units` lists at least one unit, ascending by (`record_id`,
   `version_number`) with no duplicate identity, and each `path` equals
   the derived form;
4. the listed units fit the declared scope as far as the archive can
   tell: a `version` scope lists exactly one unit with that identity, a
   `record` scope lists only units of that record; an `enrollment` or
   `installation` scope states nothing the archive can confirm on its
   own;
5. every entry in the container is `manifest.json` or one of a listed
   unit's two files — an unlisted entry is a finding; and
6. both files of every listed unit exist.

Unit checks, for every listed unit:

1. `manifest.json` parses, carries the known `format` and
   `format_version`, and agrees with the archive entry and the archive
   manifest on every shared member (`installation_id`, `exported_at`,
   identity, schema, hashes);
2. `content_hash` equals SHA-256 over the bytes of `record.json`;
3. `record.json` parses as JSON and re-serializing it under the
   canonical subset reproduces the identical bytes — the bytes are
   canonical, so the hash is over the specified representation;
4. the envelope agrees with the manifest: `record.id`,
   `record.version_number`, `record.record_schema`,
   `record.predecessor_content_hash`, `instance`, and
   `canonicalization` (`jcs-v1`);
5. `chain_hash` equals
   `SHA-256("consolebook-version-v1" || 0x00 || predecessor || bytes)`
   with `predecessor` the raw 32 bytes of `predecessor_content_hash`,
   or 32 zero bytes when it is `null`;
6. `predecessor_content_hash` is `null` exactly when `version_number`
   is 1; and
7. when the archive also lists (`record_id`, `version_number - 1`),
   that unit's `content_hash` equals this unit's
   `predecessor_content_hash` — reported as *linked*; a predecessor the
   archive does not carry is reported as *not in export*, which is not
   a failure (a single-version scope is legitimate), and a first
   version reports *none*.

`consolebook-server export verify <archive>` runs exactly these checks,
prints one line per unit and every finding, and exits non-zero unless
the verdict is `verified`. It opens no data directory.

## What the archive does not carry

- **Drafts.** An unfinalized record is not a record; scopes contain
  finalized versions only, and a scope with none is refused, never
  exported empty.
- **Acknowledgments, amendments, and signoff history.** They are
  separate records bound to versions; the trainee packet (#44 decision
  3) is the artifact that gathers them. An amendment's existence is
  visible here only as a successor version's `predecessor_content_hash`.
- **Exporter identity.** Who exported what is the installation's
  audit trail (`record_exported`), not the artifact's business.
- **Signatures.** The future signed mode adds them beside the hashes
  without changing a record byte (`docs/records-integrity.md`).

## Authorization (behavior of the producing installation)

Export is a read in a portable shape, so it follows the read rules that
already exist rather than inventing parallel ones: a version or record
exports for whoever may read the record (workflow readers and the
trainee's own finalized record, ADR 0012); an enrollment exports for
whoever may read its training history; the whole installation exports
for holders of `export_records`. Every export is audited with actor and
subject and never with content.
