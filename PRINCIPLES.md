# Consolebook Principles

Consolebook is training-record software for emergency communications centers. These principles exist before the implementation and constrain it.

## 1. Programs are configuration

The product implements stable domain concepts: programs, phases, competencies, tasks, training sessions, evaluations, summaries, acknowledgments, and amendments.

An agency's terminology, rating scales, phase structure, forms, and workflow rules are versioned configuration. Agency-specific branches and hidden conditionals are design failures.

## 2. One installation serves one agency

The core application is single-agency software. It does not carry a multi-tenant schema or require shared infrastructure.

Hosted deployments may run many isolated Consolebook instances.

## 3. Finalized records are immutable while retained

Drafts may change. Finalized records may not.

Corrections create successor versions or amendments that retain the original content, author, timestamps, reason, and acknowledgment history for as long as the applicable retention policy keeps them.

Lawful disposition is a separate, authorized workflow, never an edit disguised as cleanup. Holds block disposition. The system preserves only the destruction metadata that the applicable policy permits or requires.

## 4. Acknowledgment means receipt

A trainee's acknowledgment records receipt of a version. It does not imply agreement.

Responses, refusals, supervisor attestations, and escalation events are preserved as part of the record.

## 5. Historical records reproduce themselves

A finalized record is pinned to the exact versions of the program, form, competency text, rating definitions, and rules used to create it.

Mutable reference data must not silently rewrite history. Finalized records preserve the presentation snapshot needed to reproduce exports later.

## 6. Human dates and actual instants are separate

Operational dates and shifts carry agency-local meaning. Duration and ordering use UTC instants.

Historical sessions preserve their timezone and local representation instead of depending on current installation settings.

## 7. Integrity claims stay honest

Canonical record bytes and cryptographic hashes provide stable fingerprints and detect corruption, bugs, incomplete history, and casual tampering.

A database-local hash chain is an integrity chain. Stronger tamper evidence requires signatures backed by keys stored outside the database.

## 8. Recovery is a product feature

Backups are automatic, validated, retained, and restorable. Operators receive recovery tools, not homework.

## 9. Data remains portable

Agencies can export complete records and configuration in documented formats without vendor assistance.

Generated PDFs are presentation artifacts. Structured exports remain available for migration and independent verification.

## 10. Access follows responsibility

Permissions are expressed as capabilities. Assignment-scoped access is the default for trainers; broader review and administration require explicit authority.

Every sensitive action is attributable.

## 11. Deployment stays boring

The default deployment uses one executable and one data directory. External services are optional.

The application must remain practical for small centers with limited technical staff.

## 12. Private operational data stays private

Consolebook does not require telemetry or cloud services. Development fixtures use invented people, agencies, incidents, narratives, and identifiers.

Real training records never belong in the source repository.

---

Changes to these principles require an explicit architecture decision record explaining the reason and consequences.
