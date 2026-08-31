# ADR 0011: Canonical record format and finalization

- **Status:** Accepted
- **Date:** 2026-08-30

## Context

Milestone 4 turns approved drafts into defensible records.
`docs/records-integrity.md` requires the canonical byte representation
to be specified before any record is produced (the one Milestone 0
deliverable held open), and #32 settled the owner decisions: content
and chain hashes from the first finalized record, completion rules as
versioned per-program-version configuration with conservative
defaults, and finalized presentation that never depends on mutable
joins. This ADR fixes the format, the hashes, the finalization model,
and the completion rules (#36; Milestone 4 slice 1).

## Decision

### Canonical bytes

- A finalized version's content is one JSON document, serialized under
  RFC 8785 (JSON Canonicalization Scheme) semantics: UTF-8, object
  members sorted by code point, no insignificant whitespace, JCS
  string escaping.
- The document is restricted to a closed subset that keeps JCS number
  rules trivial and portable: values are objects, arrays, strings,
  booleans, `null`, and integers with magnitude below 2^53. Floats,
  exponent forms, and larger magnitudes are refused at serialization —
  a defect, never silently rounded. Strings are stored as authored,
  with no Unicode normalization.
- The serializer is the one owner of the format and is pinned by
  golden vectors; hashes are computed only over these specified bytes,
  never incidental serializer output.

### The envelope (record schema 1)

The document's top level identifies the format and carries the
complete historical presentation of `docs/domain-model.md`'s
EvaluationVersion. Fields, canonically ordered:

- `attachments` — always present; empty until attachments exist.
- `attribution` — the full contributor-event stream: kind, actor
  (id, username, display name), transfer recipient where present,
  recorded instant.
- `canonicalization` — `"jcs-v1"`.
- `content` — `ratings` (competency category/name/description and its
  task prompts; scale name, kind, bounds, and anchors with labels and
  definitions; the value or the explicit `not_observed` marker;
  modifiers as code and description) and `narratives` (prompt,
  required flag, authored text).
- `finalization` — instant, finalizing user (id, username, display
  name), and the completion-policy values that were in force.
- `form` — name, instructions, record type.
- `instance` — the installation identifier, so identity survives
  export.
- `program` — pinned version's program-name snapshot, version number,
  label.
- `record` — record id, this version's number, `record_schema` (1),
  and `predecessor_content_hash` (`null` for a first version), so the
  bytes themselves commit to lineage.
- `review` — the decision rows: reviewer identity, decision, comment,
  instant.
- `sessions` — each covered session's business date, timezone, local
  start/end as stored, UTC instants, disposition, phase context, and
  trainer identities.
- `trainee` — id, username, display name, employee identifier, title,
  as presented at finalization.

Schema evolution is a `record_schema` bump; existing bytes are never
rewritten.

### Fingerprints

- Every finalized version stores its canonical bytes and their SHA-256
  content hash (lowercase hex).
- Every version also stores the integrity-chain hash:
  `SHA-256("consolebook-version-v1" || 0x00 || predecessor || bytes)`
  where `predecessor` is the raw 32-byte content hash of the prior
  version, or 32 zero bytes for a first version. Golden vectors pin
  both cases.
- Honest limits (ADR 0010, `docs/records-integrity.md`): these hashes
  make records reproducible and their byte/hash consistency
  verifiable. Against a writer with direct database access they prove
  internal consistency, not provenance; verification wording never
  claims tamper-proofing. Stronger binding is the future signed mode.

### Finalization

- Finalizing produces the `evaluation_version` row — bytes, hashes,
  version number, instant, actor — inside one immediate write
  transaction that rechecks state and completion rules against
  committed data. The database refuses every later mutation of a
  finalized version, in the 0009 backstop style.
- A record with any finalized version derives the `Finalized` status
  and stays frozen; the contributor-event kind set is unchanged — the
  version row itself is the durable fact, and finalization is audited
  (`draft_finalized`) and notifies the owner and trainee's side later
  (acknowledgments are slice 2).
- `review_evaluation` holders finalize. Review approval already gated
  content through an independent reviewer, so finalization has no
  separate contributor exclusion; it is the sealing act, refused typed
  when the record is already finalized, when required approval is
  missing, or when any enabled completion rule fails.
- The finalized record presents from its stored envelope only. Later
  renames, config edits, or session changes alter nothing it shows.

### Completion rules (versioned configuration)

- `finalization_policy`, one row per program version, authored with
  the version and frozen by publication like all version content:
  `review_approved`, `required_narratives`, `ratings_complete` — each
  on or off, all defaulting on; existing published versions are
  backfilled all-on.
- `review_approved` on: only an approved draft finalizes. Off: a
  record may finalize from any unfinalized state — review remains
  available but optional, which is configuration, not a code path.
- The two content rules are enforced at submission as well as at
  finalization: a draft never enters review missing what finalization
  will demand, so an approved draft — frozen until sealed — can never
  wedge between an uneditable copy and a failing rule. With
  `review_approved` off the rules answer at the finalization attempt,
  where the copy is still editable; no state is a dead end.
- `required_narratives` on: every narrative prompt marked required
  carries non-blank text.
- `ratings_complete` on: every form competency whose scale is not
  narrative-only carries a value or the explicit `not_observed`
  marker. The marker is a typed column on the rating (mutually
  exclusive with a value at the schema); completion never keys on
  modifier codes or other heuristics.

## Consequences

### Positive

- records reproduce byte-for-byte from what is stored, with no live
  join in the presentation path;
- the chain rule and predecessor treatment are fixed before the first
  record exists, so slice 3's amendments extend a defined chain
  instead of defining one retroactively;
- signatures can be added later without redefining a record, as
  `docs/records-integrity.md` intended; and
- agencies vary finalization strictness by configuration, pinned per
  program version, never by code path.

### Costs

- the envelope duplicates presentation data that also lives in
  configuration tables — deliberately, that is what a snapshot is;
- the integer-only subset means any future fractional value must be
  modeled explicitly (fixed-point integers or strings) rather than
  serialized as a float; and
- a `record_schema` bump is the only way to change the field set, so
  envelope mistakes are permanent for records already finalized.

## Rejected alternatives

- **A general-purpose JCS dependency over arbitrary JSON:** the
  closed subset makes the specification testable and removes the
  float-serialization corner cases that make JCS implementations
  subtle; our serializer refuses what the format forbids instead of
  handling it.
- **Deriving finalized presentation from live tables with pinned
  ids:** a rename or config edit would rewrite what a record presents
  (PRINCIPLES.md 6 and 7 both fall); ids preserve identity, snapshots
  preserve what the record said.
- **A `finalized` contributor-event kind:** the 0008 kind set is a
  closed CHECK; the version row already carries actor and instant, and
  deriving status from it avoids rebuilding the event table while
  keeping one authority for "is this finalized".
- **Finalization restricted to non-contributors:** the independent
  eligibility gate is review approval (ADR 0008); duplicating it at
  the sealing step adds a second authority without adding protection
  when review is enabled, and blocks single-coordinator centers when
  it is not.
