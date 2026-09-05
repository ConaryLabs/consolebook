# ADR 0008: Sessions, drafts, and attribution model

- **Status:** Accepted
- **Date:** 2026-08-28
- **Amended by:** [ADR 0018](0018-enrollment-event-reference-shape.md), which
  completes database enforcement of enrollment-event version-reference shape.

## Context

Milestone 3 is where training work gets documented: assignment-scoped
access, enrollment lifecycle and phase history, training sessions with
explicit time semantics, collaborative daily evaluation drafts, and the
review workflow. Every shape here is what Milestone 4 freezes — drafts
become immutable `EvaluationVersion`s, contributor events become the
attribution the milestone exit promises, and session time semantics
become permanent record content (PRINCIPLES.md 6).

Issue #22 recorded the proposed shape and five open decisions; the owner
adopted all five recommendations. This ADR encodes that direction. It
builds directly on ADR 0007: versioned configuration stays the vocabulary
(phases, transition graphs, forms, scales), and this milestone adds the
history that pins to it.

## Decision

### Capabilities, bundles, and assignments

`author_evaluation`, `review_evaluation`, and `view_assigned_records`
join the implemented vocabulary, with Trainer (author + view-assigned)
and Coordinator (assign + review + view-assigned) bundles consumed at
grant time. Broader administration stays explicit authority
(PRINCIPLES.md 10).

A `training_assignment` connects a trainer to an enrollment with start
and end instants and attribution. It is the durable grant behind
assignment-scoped access: a trainer holding `view_assigned_records`
reads exactly the enrollments they hold an active assignment for.
Because an assignment exists to grant scoped reads — and its notice
names the trainee — only holders of `view_assigned_records` are
assignable.
Session trainer membership (slice 2) will additionally grant access to
that session's records, and an unassigned `author_evaluation` holder
may be added to a session ad hoc — holdover and coverage are real — with
the addition audited. Assignments are access grants, not records: ending
one closes the interval in place, attributably.

### Enrollment lifecycle is an append-only event stream

`enrollment_event` records version change (with actor and reason),
withdraw, complete, and reinstate. The database refuses UPDATE and
DELETE — the same enforcement class as `audit_event` — and status is
derived from the stream, never stored beside it.

Changing an enrollment's pinned version is a modeled event: the pin
UPDATE is accepted by the database only when the latest event for that
enrollment records exactly that change, replacing migration 0005's
blanket refusal. A version change stays within the enrollment's
continuing program — moving programs is a new enrollment — and is
refused with a typed conflict when the trainee already has an enrollment
pinning the target version. Re-enrollment is expressed as reinstate; the
one-row-per-(user, version) uniqueness stays until a real center
demonstrates the need to relax it.

### Phase history: effective-dated events validated against the graph

`phase_event` is the domain model's phase history stream — advance,
return for remediation, restart, pause, resume, complete — non-monotonic
by design. Each event carries an effective instant (when the transition
took effect) and a recorded instant (when it was written). Backfill is
honest: both instants are kept and visible, ordering uses the effective
instant, and effective never postdates recorded. Events append in
effective order — an event that would land between two already-recorded
events is refused rather than silently reordering history; the
correction path is recording forward, as on paper. The boundary applies
to epochs too: a phase event cannot take effect before the version
change that opened its epoch was recorded.

Enforcement splits by what each layer can honestly express. The database
enforces the append-only property, per-kind shape, effective-before-
recorded, and domain invariant 5 (referenced phases belong to the
enrollment's currently pinned version). Domain services enforce the
pinned transition graph — advance follows advance or skip edges, return
follows remediation, restart follows restart, entry may target any phase
of the pinned version — plus the pause state machine, required reasons
for return and restart, and `assign_training` gating, all covered by
tests. Every version change opens a fresh pin epoch, stamped on each
phase event by the database: current phase and pause derive only from
the current epoch, so state never resurrects across a version change —
even back to a previously pinned version — while history recorded under
earlier pins keeps its phases.

### Training sessions (slice 2)

A `training_session` carries the enrollment, the agency-local business
date, an IANA timezone snapshot, the stored local start/end
representation, UTC start/end instants (end open while in progress),
one-or-more trainer membership rows, phase context, and a disposition
from a closed set (completed, cancelled, interrupted; created open,
closed explicitly). The local representation is stored, never derived,
so a timezone-database change cannot rewrite history. Database-enforced:
UTC end cannot precede start, active intervals for one trainee cannot
overlap, and no uniqueness assumes one session per trainee and date
(domain invariants 6–8).

### Drafts and attribution (slice 3)

An `evaluation_record` is the continuing identity, typed by the pinned
form's record type, with sessions attached through a join table — one
daily draft per training session in v1, with multi-session or
per-business-date coverage left open as later policy, not schema. The
draft is one mutable working copy; ratings validate against the pinned
scale kind.

Attribution is metadata-only contributor events — created, contributed,
ownership transferred, submitted for review, review decided — append-only
at the database. Full content snapshots are taken at exactly two
workflow points: submission and change-request return, so a review is
anchored to what was reviewed. Autosave persists content; consecutive
saves by the same contributor coalesce into one contributed event per
working stretch, so attribution stays honest without keystroke noise.

### Review workflow (slice 4)

Single-step and capability-gated: any `review_evaluation` holder who is
not a contributor to the draft may approve, request changes (with a
required comment), or return it. Self-review is refused. Configured
multi-step chains follow ADR 0007's pattern — demonstrated need from a
real center plus their own ADR. Whether review is required before
finalization becomes versioned configuration in Milestone 4's completion
rules, not code. Workflow notices reuse the 0003 per-recipient notice
table with new kinds targeted at the affected user.

## Consequences

### Positive

- assignment-scoped access is a database fact, not handler discipline,
  and every grant and ending is attributable;
- version changes, withdrawal, and phase movement are permanent history
  that Milestone 4 records can cite;
- backdated paperwork is representable without falsified
  contemporaneity, and the refusal to interleave keeps derived state
  reproducible;
- the graph rules make "can this trainee move there" a configuration
  question, never a code change; and
- attribution arrives before drafts do, so collaborative documentation
  never has an unattributed era.

### Costs

- deriving status and current phase from streams costs queries a status
  column would not — accepted to avoid a second writable authority;
- interleaved backfill (inserting between recorded events) is refused;
  centers that discover deep paperwork errors must record correcting
  events forward;
- metadata-only draft history means intermediate draft content between
  snapshots is unrecoverable by design; and
- pause state resets with a version change, matching phase re-entry —
  a paused trainee changed to a new version must be re-paused if still
  out.

## Rejected alternatives

- **Content snapshot per save:** discoverable draft archaeology and a
  disposition burden with no exit-criterion payoff; the two workflow
  snapshots anchor review without it.
- **Strict assignment-only draft access:** punishes holdover and
  coverage staffing; audited ad-hoc session membership handles reality
  without opening broad access.
- **Configured review chains now:** configuration quietly becomes
  workflow code before any center needs it; single-step review with
  capability gating covers v1.
- **Stored enrollment status beside the stream:** a second writable
  authority that can disagree with the history; deriving keeps the
  stream authoritative.
- **Free interleaving of backdated events:** silently rewrites what
  derived state meant at recording time; refused in favor of honest
  forward corrections.
