# ADR 0017: Packet pin timeline verification

- **Status:** Accepted
- **Date:** 2026-09-05
- **Issue:** [#52](https://github.com/FieldmouseWorks/consolebook/issues/52)
- **Amends:** [ADR 0015](0015-trainee-packet.md)

## Context

Packet verification checked whether a document's program version appeared
anywhere in the enrollment's pin history. A rehashed packet could therefore
attribute an early signoff to a version pinned only later, or place phase
activity before the version-change event that opened its named epoch.

The producer already carries all required instants: version changes have
`occurred_at`, signoffs have `signed_at`, and phase events have `effective_at`,
`recorded_at`, and an epoch identity. All are UTC Unix seconds. Separate tables
do not establish the order of a change and an act within the same second.

## Decision

`packet_verify::PinHistory` owns the temporal interpretation alongside its
existing version and label checks. Every epoch retains its pinned version,
opening instant, and the next version change's closing instant. The original
pin has no version-change opening boundary; the final epoch has no closing
boundary. Epochs remain distinct when the enrollment returns to an earlier
version.

- A signoff must name a version pinned at `signed_at`. Each epoch includes
  both boundary seconds. Equivalently, at second `t`, allow the pin after all
  changes strictly before `t`, plus every target reached by a change at `t`.
  Multiple changes within one second may therefore allow several versions;
  a pin that exists only in that second cannot explain an adjacent second.
- A phase event must name the version its explicit epoch reached. Both
  `effective_at` and `recorded_at` must be at or after its epoch's opening;
  `recorded_at` must be at or before its closing. Under the original pin, only
  the closing boundary applies. The existing `effective_at <= recorded_at`
  shape rule still applies, and backdating within the epoch remains valid.
- Version-change instants must be nondecreasing in recorded event order, so
  an epoch cannot close before it opens. A contradictory timestamp history
  fails verification, even if its origin was a clock regression rather than
  an edited archive. The verifier does not reorder or repair that history.
- All timeline contradictions use `DocumentPinHistory`. Existing shape,
  canonical-byte, hash, label, and reference checks remain in force.

These are stricter consistency checks on version-1 fields, not a new document
shape. The packet format remains version 1 under its existing versioning
rule. The producer and schema need no changes: they already preserve the
instants and epoch references verbatim. The normative packet specification
states the temporal rules and the same-second limitation.

## Ownership and proof

`tests/trainee_packet/pin_history.rs` owns membership and temporal packet
verification tests, extracted from the large packet integration-test module.
The parent retains shared fixtures and archive-editing helpers and registers
the child module. This test reorganization has no persisted or public impact.

The real producer exports deterministic, constrained database fixtures that
visit versions 1, 2, 3, then 1 again. They include acts on both sides of changes
within the same second and an intermediate epoch opened and closed in one
second. Forged documents are canonicalized and their manifest hashes updated;
the negative assertions require pin-history findings, demonstrating that a
checksum or shape error is not doing the timeline check's work.
An additional round trip records pin changes, phase entries, and signoffs
through the domain services and verifies the resulting exported packet.

## Limits

The result still means consistency with the stated fingerprints and history,
not proof of provenance or of a timestamp's truth. An attacker who rewrites a
whole history consistently is outside this verifier's guarantee. The packet
cannot resolve subsecond ordering that was never stored. This change adds no
clock synchronization, signing, epoch column for signoffs, or live-write
concurrency policy; those require their own storage and service decisions.
