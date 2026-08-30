# ADR 0010: Database backstops hold data invariants; services own authorization

- **Status:** Accepted
- **Date:** 2026-08-29

## Context

Milestone 3's draft and review schema carries database backstops that
hold under raw SQL writes: append-only streams, the derived frozen
state and its guards, event/decision/snapshot pairing, the session
overlap and agreement invariants, and the self-review refusal. During
review of #31, two proposals would have extended those backstops
further: checking `capability_grant` inside the decision-insert
triggers so an unqualified user cannot insert a review decision raw,
and validating snapshot content against the working copy so a raw
change request cannot carry a fabricated anchor. Both were declined on
the threads; this record makes the boundary durable so it does not get
re-litigated one table at a time.

## Decision

- Database backstops enforce the record system's **data invariants** —
  properties stated over the schema's own rows. That includes both
  append-only history (events, decisions, snapshots, audit) and the
  declared domain invariants over operational state that
  `docs/domain-model.md` assigns to the database, such as session
  overlap (invariant 7) and evaluation-session agreement. The
  self-review refusal is the model for eligibility held by history: a
  raw writer cannot legally satisfy it, because appending events only
  ever makes a user more of a contributor.
- **Authorization is the domain services' typed contract.**
  Capabilities are checked, refused typed, and audited in the service
  layer, and nowhere else. Triggers do not read `capability_grant`:
  whether an actor was allowed is not a data invariant of the record
  rows, and grant rows are mutable reference data a direct writer
  controls.
- **Content truthfulness is not provable at any local layer against a
  direct writer.** The schema holds structure, ordering, pairing, and
  immutability of content rows; the services validate content against
  the pinned vocabulary. Neither can attest that stored content
  matches what a person actually saw or wrote.
- Milestone 4's canonical bytes and stored hashes
  (`docs/records-integrity.md`; #32, decision 1) make finalized
  versions reproducible and their byte/hash consistency verifiable —
  and, as records-integrity.md itself states, database-local hashes
  prove internal consistency only: a writer with direct database
  access can fabricate bytes and recompute their hashes. Stronger
  binding waits on that document's future signed mode. The
  snapshot-content proposal is therefore declined as unprovable at
  this trust level, not relocated to finalization.

## Consequences

### Positive

- one authority per authorization rule: a capability rename or
  grant-model change touches the service, its tests, and the explicit
  data migration that rewrites persisted grant strings
  (`capability_grant` stores names; bundles apply once, per
  `capabilities.rs`) — never trigger logic scattered across tables;
- backstop triggers stay provable — each enforces a property a test
  can force raw and a reviewer can reason about without simulating the
  service; and
- review discussions have a stated line: a proposed trigger is asked
  "does this hold a data invariant of the record system's own rows,
  or does it re-check who was allowed or attest that content is
  truthful?" before it is asked "is it possible?".

### Costs

- a writer with direct SQLite access can insert rows the service
  would have refused for authorization; the schema documents this
  honestly instead of pretending otherwise; and
- the boundary must be restated in review from time to time — this
  record exists to make that cheap.

## Rejected alternatives

- **Capability checks in triggers:** `capability_grant` is mutable
  reference data the same raw writer controls; one self-granted row
  precedes the guarded insert, so the trigger establishes nothing
  while creating a second authorization authority that must track the
  service's. To mean anything it would need protective triggers on
  `capability_grant` itself and a schema-owned grant model — a
  deliberate redesign that would get its own ADR, not a per-table
  patch.
- **Validating snapshot content in triggers:** while a draft is
  frozen its content rows cannot change, so a decision-time comparison
  of the return snapshot against the frozen rows is expressible and
  would hold a real, narrow property — snapshot-to-row consistency at
  that instant, never what any person saw. It is declined as a poor
  trade, not as impossible: the comparison reimplements the snapshot
  format in SQL, a second owner that must track every format change,
  and it guards one snapshot kind while the submission snapshot has
  no pairing at all and a consistent wholesale fabrication stays open
  at the same trust level. ADR 0008's anchor is guaranteed where it
  is produced — the service writes snapshot and decision in one
  transaction from the frozen rows — and the defensible-record anchor
  is Milestone 4's canonical bytes and hashes, with binding against a
  hostile local writer waiting on the signed mode
  `docs/records-integrity.md` sketches. An owner who weighs this
  trade differently reopens it by ADR, with the comparison scoped to
  every snapshot kind at once.
- **Refusing "raw" writes as such:** SQLite triggers cannot
  distinguish the service's statements from any other connection's;
  there is no expressible predicate.
