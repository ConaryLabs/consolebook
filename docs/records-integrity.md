# Records Integrity

Training records may be used in personnel decisions, grievances, audits, and litigation. The data model must preserve what was documented, by whom, under which rules, and what happened afterward.

## Draft and finalized states

Draft content is mutable. Draft edits and ownership transfers remain attributable.

Finalization creates an immutable EvaluationVersion. Application code must never update finalized content in place.

A correction produces a successor version linked to the original with an explicit reason and authority. Both versions remain available.

## Canonical bytes

The canonical byte representation is part of the record format and must be specified before records are produced.

The current design target is canonical JSON using RFC 8785 JSON Canonicalization Scheme semantics:

- UTF-8 encoding;
- deterministic object-member ordering;
- deterministic number serialization;
- no insignificant whitespace; and
- a versioned envelope identifying the canonicalization and record-schema versions.

Hashes must be calculated over the specified canonical bytes, never over incidental serializer output.

## Stable fingerprints

Every finalized version receives a SHA-256 content hash.

A version relationship may also carry an integrity-chain hash using a domain-separated construction such as:

```text
SHA-256(
  "consolebook-version-v1" ||
  0x00 ||
  previous_version_hash ||
  canonical_record_bytes
)
```

Exact byte lengths and treatment of a missing predecessor must be fixed in the format specification and covered by golden vectors.

This chain detects corruption, incomplete history, buggy writes, and lazy tampering. Someone with arbitrary database-write access can recompute a database-local chain, so the product must not describe the chain alone as strong tamper evidence.

## Signatures

A future stronger mode may sign version hashes with an installation Ed25519 key stored outside SQLite with operating-system access controls.

The public key and signature metadata would accompany structured exports and PDFs. Key creation, rotation, backup, recovery, and compromise handling require a separate design and are outside milestone one.

Canonicalization is included now so signatures can be added without redefining a record.

## Historical presentation snapshots

Finalized versions cannot depend on mutable joins for their meaning. They preserve the values required to reproduce the record, including:

- displayed names and identifiers;
- role or title when relevant;
- program and phase labels;
- competency and task text;
- rating labels and definitions;
- form instructions;
- timezone and local-time representation; and
- template and font versions used for rendered output.

Stable IDs preserve identity. Snapshots preserve what the record said.

## Attachments

Attachments included in a finalized record receive cryptographic hashes and immutable metadata. Replacing an attachment creates a successor record version.

Malware scanning, content-type validation, size limits, and export behavior remain to be designed.

## Acknowledgments

Acknowledgments bind to one immutable EvaluationVersion. They are separate records so receipt, disagreement, refusal, and escalation do not alter the authored evaluation.

An amendment never inherits acknowledgment silently.

## Audit trail

The audit trail records actions around the record: viewing where policy requires it, submission, finalization, acknowledgment, refusal, export, authorization changes, backup, and restore.

Audit events need their own retention and integrity rules. They do not replace immutable record versions.

## Verification

Before the first production-capable release, Consolebook must have:

- canonicalization golden vectors;
- hash and predecessor-chain vectors;
- database tests proving finalized rows reject mutation;
- export round-trip tests;
- deterministic PDF fixtures within defined tolerances;
- backup validation and restore drills; and
- tests proving amendments and acknowledgments bind to exact versions.
