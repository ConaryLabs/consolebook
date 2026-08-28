# Program-Version Export Format

Versioned JSON for whole program versions (ADR 0007; PRINCIPLES.md 9).
`consolebook-server/src/program_export.rs` implements it; round-trip
tests in `tests/program_export.rs` prove it. This document is normative:
the implementation follows it, not the other way around.

## Envelope

```json
{
  "format": "consolebook-program-version",
  "format_version": 1,
  "content": { ... }
}
```

- `format` identifies the document family and never changes.
- `format_version` is an integer, bumped by any change to the document
  shape. Import accepts exactly the versions the running build
  understands and refuses others by name.

## Determinism

Following the canonical-bytes principles in `docs/records-integrity.md`,
an export is deterministic: the same configuration always produces the
same bytes.

- UTF-8, no insignificant whitespace, no trailing newline.
- Object members appear in exactly the order this document lists them.
- Arrays are ordered: phases by (`presentation_number`, `name`);
  transitions by (`from_phase`, `to_phase`); competencies, tasks, form
  competency bindings, and narratives in authored order; rating scales
  and evaluation forms by `name`; anchors by `value`; modifiers by
  `code`; citations by (`body`, `edition`, `clause`, `note`).
- All string values are stored and emitted verbatim. Nothing trims,
  case-folds, or re-wraps content.
- Optional numeric fields are always present, `null` when absent.

Import accepts any member and array order; export order is what is
normative. Full RFC 8785 canonicalization and content hashing arrive
with finalized records (Milestone 4); this format is designed so that
adding them does not change a single exported byte.

## `content`

| Member | Type | Meaning |
| --- | --- | --- |
| `name` | string | Program name as presented by this version (snapshot; non-blank) |
| `label` | string | Agency-visible free-text version label; may be empty |
| `description` | string | May be empty |
| `phases` | array | Zero or more; a phase-less version is a valid annual/in-service shape |
| `phase_transitions` | array | Explicit allowed edges between named phases |
| `competencies` | array | Competencies with nested tasks and citations |
| `rating_scales` | array | Closed-kind scales with agency content |
| `rating_modifiers` | array | Version-wide rating modifiers |
| `evaluation_forms` | array | Agency content populating product-owned form skeletons |
| `citations` | array | Version-level standards citations |

Names are unique within their collection, ASCII-case-insensitively.
References (`from_phase`, `to_phase`, `competency`, `rating_scale`) match
their target's name exactly, case-sensitively.

### `phases[]`

`name` (non-blank), `description`, `presentation_number` (integer;
presentation ordering, never progress).

### `phase_transitions[]`

`from_phase`, `to_phase` (names of phases in this document), `kind`
(one of `advance`, `remediation`, `skip`, `restart`). At most one edge
per (`from_phase`, `to_phase`) pair.

### `competencies[]`

`category` (may be empty), `name` (non-blank), `description`,
`tasks` (array of `prompt` (non-blank, unique within the competency) and
`citations`), `citations`.

### `rating_scales[]`

`name` (non-blank), `kind`, `min_value`, `max_value`, `anchors` (array
of `value` (integer, unique within the scale), `label` (non-blank),
`definition`).

Kind rules:

- `anchored_numeric`: `min_value` and `max_value` integers with
  `min_value < max_value`; at least one anchor; every anchor value
  within the bounds.
- `pass_fail`: bounds `null`; exactly two anchors with values 0 and 1
  (labels are the agency's).
- `narrative_only`: bounds `null`; no anchors.

### `rating_modifiers[]`

`code` (non-blank), `label` (non-blank), `description`.

### `evaluation_forms[]`

`record_type` (one of `daily_report`, `weekly_summary`,
`phase_evaluation`), `name` (non-blank), `instructions`,
`competencies` (array of `competency` and `rating_scale` name
references; each competency bound at most once per form), `narratives`
(array of `prompt` (non-blank) and `required` boolean).

### `citations[]` (all levels)

`body` (non-blank; the standards body, e.g. an accreditation program),
`edition` (may be empty), `clause` (non-blank), `note` (may be empty).
The document never embeds standard text — citations are the agency's
mapping, the standard itself is not product content (ADR 0007).

## What the document does not carry

- **Version numbers.** They are instance-local identity, assigned
  monotonically by the importing installation.
- **Publication state.** Import always creates a draft; publishing is
  the normal explicit workflow.
- **Row identifiers or instance identity.** References are by name, so
  a document is portable between installations.

## Import semantics

Import validates the envelope, then the content (same rules as
authoring), then creates either a new program named by `name` (refused
when the name is taken) or the next draft version of an existing
program. A successful import followed by an export of the new version
reproduces the exported document byte for byte.
