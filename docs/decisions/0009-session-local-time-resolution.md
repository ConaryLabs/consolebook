# ADR 0009: Local-time capture and resolution for training sessions

- **Status:** Accepted
- **Date:** 2026-08-28

## Context

PRINCIPLES.md 6 separates agency-local meaning from UTC instants, and
ADR 0008 gives training sessions both: a business date, timezone
snapshot, and local start/end representation beside UTC instants that
ordering, duration, and the overlap invariant reason about. Someone has
to turn "07:00 at this center" into an instant, and daylight-saving
transitions make some local times ambiguous (a fall-back fold) or
nonexistent (a spring-forward gap). Whatever rule resolves them becomes
part of what every historical session means, so it is fixed here
(#25; Milestone 3 slice 2).

## Decision

- The operator enters the business date, the local start and end, and
  an IANA timezone name; the interface defaults the timezone from the
  browser and the operator may correct it.
- The entered strings are stored verbatim — the local representation is
  never derived from the instants. The UTC instants are computed once,
  at entry, on the server, against the embedded IANA timezone database
  (the `jiff` crate), and never recomputed afterward.
- Disambiguation follows the RFC 5545 compatible rule: a local time
  inside a spring-forward gap resolves forward by the gap's length, and
  an ambiguous fall-back time takes the earlier offset (its first
  occurrence). Golden tests pin both cases.
- Unknown timezone names are refused, never defaulted.

## Consequences

### Positive

- historical sessions reproduce themselves with no live timezone data
  and survive later timezone-rule changes untouched;
- the overlap and ordering invariants reason about one honest timeline;
  and
- the client never computes an instant, so a local/UTC pair can never
  disagree by construction.

### Costs

- the embedded timezone database ages with the binary: a center in a
  jurisdiction that changes its rules needs an updated build before new
  entries resolve under the new rules (stored history is unaffected);
  and
- gap and fold times resolve silently instead of prompting the
  operator; revisit with an explicit confirmation step if real centers
  report surprise.

## Rejected alternatives

- **Client-computed UTC:** the server cannot verify the pair, so a
  buggy or hostile client could store a local representation and an
  instant that disagree.
- **Storing only UTC and deriving local display:** a timezone-rule
  change would rewrite what historical sessions present, violating
  PRINCIPLES.md 6.
- **Refusing ambiguous or nonexistent local times:** overnight shifts
  genuinely run through DST transitions; blocking entry punishes honest
  documentation for a rare edge the compatible rule handles
  deterministically.
