# ADR 0010: Database backstops hold history; services own authorization

- **Status:** Accepted
- **Date:** 2026-08-29

## Context

Milestones 3's draft and review schema carries database backstops that
hold under raw SQL writes: append-only streams, the derived frozen
state and its guards, event/decision/snapshot pairing, and the
self-review refusal. During review of #31, two proposals would have
extended those backstops beyond that line: checking `capability_grant`
inside the decision-insert triggers so an unqualified user cannot
insert a review decision raw, and validating snapshot content against
the working copy so a raw change request cannot carry a fabricated
anchor. Both were declined on the threads; this record makes the
boundary durable so it does not get re-litigated one table at a time.

## Decision

- Database backstops enforce properties of **immutable record
  history** — facts derivable from append-only streams and rows the
  same schema forbids rewriting. The self-review trigger is the model:
  a raw writer cannot legally satisfy it, because appending events
  only ever makes a user more of a contributor.
- **Authorization is the domain services' typed contract.**
  Capabilities are checked, refused typed, and audited in the service
  layer, and nowhere else. Triggers do not read `capability_grant`.
- **Content semantics are the domain services' contract.** The schema
  holds structure, ordering, pairing, and immutability of content
  rows; it does not attest that content is truthful or current.
- Milestone 4's finalization is where content becomes provable:
  canonical bytes and stored hashes (`docs/records-integrity.md`) bind
  finalized versions at a layer a verifier can actually check, which
  is the honest answer to the snapshot-content proposal (#32,
  decision 1).

## Consequences

### Positive

- one authority per rule: a capability rename or grant-model change
  touches the service and its tests, never a migration;
- backstop triggers stay provable — each enforces a property a test
  can force raw and a reviewer can reason about without simulating the
  service; and
- review discussions have a stated line: a proposed trigger is asked
  "is this derivable from immutable history?" before it is asked
  "is it possible?".

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
- **Validating snapshot content in triggers:** requires serializing
  the working copy in SQL to compare — a second owner of the snapshot
  format — and still proves nothing, because a writer who can
  fabricate the snapshot can fabricate the draft rows it snapshots.
  Content binding belongs to finalization hashes.
- **Refusing "raw" writes as such:** SQLite triggers cannot
  distinguish the service's statements from any other connection's;
  there is no expressible predicate.
