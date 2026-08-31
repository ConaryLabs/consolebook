# ADR 0013: Weekly summary links, record schema 2, and task signoffs

- **Status:** Accepted
- **Date:** 2026-08-31

## Context

Milestone 4's last slice (#42) delivers the WeeklySummary and
TaskSignoff of `docs/domain-model.md` on the finished record substrate
(ADR 0011's canonical format and finalization, #39's version-bound
acknowledgments, ADR 0012's amendments). A weekly summary "references
the exact finalized daily-report versions included in the summary and
carries independent narrative, finalization, acknowledgment, and
amendment history"; #32 decision 5 settled that it carries both its
own authored content and validated links. A task signoff is "a
versioned record that a configured task was observed or demonstrated"
whose overrides "require explicit authority and a recorded reason";
decision 6 put it in this slice. The open questions are durable: how
links enter the record bytes, and what shape the signoff's versioning
takes.

## Decision

### Weekly summaries are ordinary records plus pinned links

- A weekly summary is an `evaluation_record` whose pinned form has
  `record_type = weekly_summary`, created on an enrollment by a
  coordinator or an assigned `author_evaluation` holder, with no
  covered sessions. Everything already built applies unchanged —
  working copy, completion policy, review, finalization,
  acknowledgment, amendment — because it is a record, not a parallel
  pipeline.
- The working copy additionally carries `summary_daily_link` rows,
  each pinning one finalized daily-report `evaluation_version` of the
  same enrollment. The pin is exact: a later amendment of the daily
  produces a new version and never rewrites what the summary
  summarized. Links are authored while the copy is editable, freeze
  and thaw with it, and are validated typed with the shape held raw
  (weekly records only, own enrollment only, daily reports only,
  add/remove but never edit).
- Superseded daily versions remain linkable: the author chooses the
  exact version the summary speaks about, and every retained version
  is a legitimate referent while retained.

### Record schema 2

- The envelope gains one canonically ordered member, `daily_reports`:
  an array of `{content_hash, record_id, version_number}` objects,
  present in every schema-2 envelope and empty for records that link
  nothing. The summary's bytes thereby commit to exactly what it
  covered — identity by id and version, content by hash.
- All new finalizations emit `record_schema` 2. Existing schema-1
  bytes are never rewritten and never reinterpreted (ADR 0011:
  evolution is a bump, history is untouched); readers present each
  version under its own stored schema.
- Presentation of a link resolves through the pinned immutable version
  — its stored envelope, addressed by id and checkable against the
  recorded hash — which is a read of immutable data, not a mutable
  join.

### Task signoffs are versioned state per (enrollment, task)

- `task_signoff` rows are append-only: enrollment, a task of the
  enrollment's pinned version (held raw), a kind from the closed set
  `observed | demonstrated | revoked`, the signing actor with a
  presentation-name snapshot, and the instant. The latest row answers
  the current state; the full history stays readable.
- The first signoff for a task takes authoring scope (a coordinator,
  or an assigned `author_evaluation` holder). Every later row is an
  override: it takes `review_evaluation` and a non-blank recorded
  reason, and a revocation exists only where there is something to
  revoke. Authority is the service's typed contract (ADR 0010); reason
  shape, ordering, pinning, and permanence hold under raw writes.
- Signoffs are independent of evaluation records: they attach to the
  enrollment's pinned vocabulary, not to any draft or version, so they
  neither freeze with records nor enter envelopes in this slice.
  Exports (Milestone 5) decide their portable representation.

## Consequences

### Positive

- summaries inherit every integrity property already proven for
  records — one lifecycle, one freeze authority, one amendment model;
- the bytes of a summary commit to its coverage, so "which dailies did
  this summarize" is answerable from the record alone, offline,
  forever; and
- signoff history is complete and orderly: no state is overwritten,
  and every change of position names its authority and reason.

### Costs

- a schema bump means two envelope shapes coexist permanently; every
  future reader keys on `record_schema` (the envelope carries it for
  exactly this reason);
- pinned links can point at superseded daily versions, which readers
  must present honestly (the version history makes supersession
  visible) rather than silently upgrading; and
- signoff state is per pinned version: an enrollment version change
  (Milestone 3's modeled event) starts a fresh vocabulary, and any
  carry-over policy is future work, not an implicit copy.

## Rejected alternatives

- **Links outside the bytes (a side table only):** the sealed record
  must commit to what it covered; a summary whose coverage lives in
  mutable rows fails PRINCIPLES.md's snapshot rule the moment a link
  row is touched.
- **Linking records rather than versions:** "the exact finalized
  daily-report versions" is the domain contract; record-level links
  would silently follow amendments and rewrite what the summary said.
- **Auto-linking by business-date window:** date heuristics choosing
  record contents would put a heuristic in charge of record integrity
  (AGENTS.md forbids exactly this); authors choose, validation
  refuses.
- **Mutable signoff state (update-in-place with history elsewhere):**
  one more place where an UPDATE rewrites a record; append-only rows
  with a derived current state match the repository's every other
  permanent structure.
