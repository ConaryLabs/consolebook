# ADR 0012: Amendment reopening and the successor state machine

- **Status:** Accepted
- **Date:** 2026-08-31

## Context

ADR 0011 fixed the canonical format, the chain rule, and the
predecessor treatment before the first record existed, deliberately
leaving succession mechanics to the amendment slice (#40; #32
decision 4: amendments are made by the finalizing capability set,
always carry a required reason, and always require re-acknowledgment).
Delivering them requires durable decisions ADR 0011 does not cover:
how a sealed record's one working copy reopens, what state the
append-only streams derive during a correction, and how the reopened
cycle ends. `docs/records-integrity.md` constrains the answer: a
correction is a successor version linked to the original with an
explicit reason and authority; both versions remain available while
retained; an amendment never inherits acknowledgment silently.

## Decision

- **The amendment row is both the permanent record and the open-state
  marker.** An `amendment` binds one predecessor `evaluation_version`
  (at most once — the version takes at most one successor) and records
  the required non-blank reason, the authority (id plus a
  presentation-name snapshot), and the instant, append-only. A record
  is *reopened* exactly while an amendment targets its current latest
  version; the successor's arrival makes a newer version latest and
  ends the reopening by derivation. There is no mutable status column
  and no second authority for "is this record open".
- **The correction travels the ordinary workflow.** Opening an
  amendment thaws the record's one working copy — still holding the
  sealed content — and the correction is contributed, submitted, and
  reviewed under the pinned version's `finalization_policy` exactly
  like original content. Sealing produces version N+1 with
  `predecessor_id`, the envelope's `predecessor_content_hash`, and the
  chain hash of ADR 0011.
- **Reopened-cycle state is scoped by high-water marks.** The
  contributor-event kind set stays closed (ADR 0011's reasoning for
  deriving Finalized) and history is never rewritten; instead the
  amendment records the last event and decision ids that existed at
  opening, held truthful by triggers, and every workflow derivation for
  a reopened record — frozen state, draft status, the raw approval
  gate, contribution coalescing — reads only what came after the
  marks. A superseded cycle's approval can never leak into the new
  one, and the correction's edits are attributed within their own
  cycle.
- **Opening advances the working copy's revision**, so a save or
  finalization carrying a prior cycle's optimistic-concurrency token
  resolves as a typed stale refusal, never a silent overwrite of the
  new cycle's copy.
- **Own-record readers see the sealed self throughout.** A trainee
  admitted on the own-record basis reads the latest sealed version, its
  acknowledgment, and the history; during a reopening the workspace
  still presents finalized status with no working copy, no
  reopened-cycle events or decisions, and no amendment internals — a
  draft in progress about them is not theirs to see until it seals and
  awaits their acknowledgment.
- **Every retained version stays readable and verifiable by number**
  (`/api/drafts/{id}/versions/{n}` and its verification), the
  superseded original presented behind an explicit banner. The
  successor starts unacknowledged by construction: acknowledgments bind
  to exact versions (#39).
- **There is no amendment withdrawal in v1.** A reopening ends only by
  sealing the successor; a mistaken opening is closed by re-sealing,
  which is explicit, audited, and re-acknowledged. Withdrawal can be
  modeled later as its own recorded event if the burden proves real.

## Consequences

### Positive

- one derivation answers "reopened or sealed", with the database
  refusing forged marks, out-of-order successors, and successors
  without amendments under raw writes;
- the whole event and decision history survives every correction —
  nothing is cleared, rewound, or duplicated to make room for a new
  cycle; and
- amendments reuse the existing workflow, gates, snapshots, and
  review machinery rather than growing a parallel correction pipeline.

### Costs

- workflow derivations carry mark-scoping complexity (`id >` the
  reopening marks) that every future stream consumer must respect;
- an abandoned reopening leaves the record editable until someone
  seals it — visible in the interface and audit trail, but not
  self-expiring; and
- the amendment table's dual role means its rows participate in state
  derivation, so slice-4-and-later features must not repurpose or
  soft-delete them.

## Rejected alternatives

- **New contributor-event kinds (`amendment_opened`) in the stream:**
  the 0008 kind set is a closed CHECK on a STRICT table; extending it
  rebuilds the table, and ADR 0011 already chose deriving state over
  reshaping the stream.
- **A mutable `state` column on the record or amendment:** a second
  authority beside the derivations, updatable by raw writes into
  contradictions the triggers could not referee.
- **Clearing or snapshot-resetting the working copy at opening:** the
  sealed content is the natural starting point for a correction, and
  destroying contributions to make attribution simpler inverts the
  priority — attribution adapts (mark-scoped coalescing), content
  survives.
- **A separate amendment-draft table:** duplicates the working-copy
  machinery (content, freeze triggers, snapshots, review anchoring)
  for no invariant the marks do not already hold.
