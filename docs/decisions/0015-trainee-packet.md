# ADR 0015: Trainee packets

- **Status:** Accepted
- **Date:** 2026-09-01
- **Amended by:** [ADR 0017](0017-packet-pin-timeline-verification.md), which
  binds signoffs and phase events to the pin timeline at Unix-second precision.

## Context

Milestone 5's exit is that a center can leave with all of its data;
its roadmap names trainee packet generation. #44 decision 3 settled the
owner choice: a packet is everything retained about one enrollment —
every retained version of every record, superseded originals included,
the acknowledgments, the amendment records, the signoff history,
structured units plus (later) their PDFs, and one packet manifest — with
nothing summarized-only. ADR 0014 fixed the export unit and its
verifier; ADR 0013 left the signoff's portable representation to
exports. This ADR fixes the packet's composition, its documents, who
may produce it, and how it verifies (#48; Milestone 5 slice 2).

## Decision

### A packet is an export plus typed documents, never a re-serialization

- A packet's units are the record export's units, byte for byte: the
  same `record.json` and unit manifests, written by the same code. The
  packet adds a root manifest of its own format
  (`consolebook-trainee-packet`) and four canonical-JSON documents
  under `packet/`: the enrollment's lifecycle and phase history, every
  acknowledgment, every amendment, and the full task signoff history
  (`docs/formats/trainee-packet.md`).
- Documents name every person by stable identity beside the name
  shown — the stored snapshot where the act stored one (acknowledgment
  names, amendment authority, signoff authority). The enrollment
  document and the manifest's `enrollment` member present the trainee,
  program version, and event actors as of the export instant, and the
  format says so; they are presentation of history, not records, and
  the records inside the units remain the authority.
- Document shapes mirror the stored tables' own constraints — closed
  kinds, mandated orders carrying the row identity that fixes recorded
  order, speaker and response rules, version and phase references by
  kind — so a document that could not have come from the tables does
  not verify, and the shape check is the format's contract rather than
  a spot check of member types.
- The document set is a closed, ordered list of kinds carried exactly
  once. Rendered PDFs (decision 2, slice 4) arrive as a new kind under a
  new format version; a version-1 verifier refuses a kind it does not
  know rather than skipping it.

### Completeness is a property, not a summary

- Every retained version of every record of the enrollment is present,
  superseded originals included, and the unit list may be empty: an
  enrollment with no finalized version still has a history, and the
  packet is the truthful account of it. Sessions and drafts that
  produced no finalized version are operational data and stay out;
  the sessions a record covered are inside that record's bytes.
- The signoff history travels in full — first signoffs and overrides
  alike — so the packet answers "what was signed off, by whom, and what
  changed" without the installation.
- Every component is read from one database snapshot (one read
  transaction), so the manifest, the units, and the documents describe
  the same committed state; a packet never carries a version its
  acknowledgment document has not seen, or the reverse.
- Every `kind` a document carries is a closed vocabulary shared with the
  module that owns the table — lifecycle event kinds, phase event kinds,
  acknowledgment kinds, signoff kinds — parsed on production and on
  verification, never passed through as text.

### One verifier, dispatching on the declared format

- `export verify` reads the root manifest's `format` and applies the
  packet's checks: the record export's unit checks unchanged, the
  documents present and hashing to the manifest, canonical and typed,
  references from acknowledgments and amendments resolving to carried
  units, the amendments agreeing with the lineage the units establish
  both ways, every program version the documents name belonging to the
  pin history the lifecycle events define, every unit's predecessor
  carried, every unit's envelope naming the packet's trainee, and
  nothing unlisted. The verdict keeps
  ADR 0014's honest meaning: consistency with the stated fingerprints.

### Production follows read rules that already exist

- A packet is produced for whoever may read the enrollment's training
  history, for the trainee on their own enrollment
  (`view_own_records`), and for `export_records` holders (ADR 0010: the
  contracts exist; none is invented here). The trainee's own packet
  therefore carries their signoff history, which the interface does not
  yet show them outside the packet — the packet is the trainee leaving
  with their records, and the record is theirs; the interface gap is
  tracked separately, not resolved by withholding it from the packet.
- The read rules are evaluated inside the packet's own read
  transaction, so permission and contents describe one committed state,
  and that transaction's connection is the only one a packet request
  holds while it runs.
- Every packet is audited (`trainee_packet_exported`) with actor and
  trainee and never with content.

## Consequences

### Positive

- a trainee leaves with one archive that carries every record about
  them exactly as sealed, plus the acts and history around those
  records, verifiable anywhere with the same tool as a record export;
- the unit machinery, the envelope reader, and the verifier are shared,
  so the packet inherits every integrity property already proven for
  exports and adds only document checks; and
- the format's closed document set means a reader knows exactly what a
  version-1 packet contains and what it cannot.

### Costs

- two manifests of two formats share the container and the unit
  layout, so the verifier dispatches on `format` and both format
  documents must stay consistent about units;
- presentation-as-of-export members (trainee, program, event actors)
  can differ between two packets of the same enrollment taken after a
  rename, unlike the records inside — the format labels them, and
  determinism holds per instant, not across renames; and
- the packet is assembled in memory like the record export (#47
  covers streaming both).

## Rejected alternatives

- **A packet as a record export with extra entries under the same
  format:** the export format's verifier rightly flags anything
  unlisted; a packet is a different artifact with a different manifest
  and should say so by name rather than by exception.
- **Summarizing acknowledgments and signoffs into the enrollment
  document:** decision 3 says nothing summarized-only; each act is
  carried as its own row with its snapshots.
- **Withholding the signoff history from the trainee's own packet:**
  the packet exists so a trainee leaves with their records; a record
  about them that other readers can see but they cannot would make the
  packet incomplete by design.
- **Including sessions and drafts:** they are not records; a packet of
  records that also carried operational scheduling data would blur what
  the trainee is leaving with.
