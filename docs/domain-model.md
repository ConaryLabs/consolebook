# Domain Model

Consolebook uses a training domain with versioned agency configuration. These
are domain concepts, not a table inventory. Retention/disposition, attachments,
and PDF presentation remain design targets; see [roadmap.md](roadmap.md).

## Configuration

### Program and ProgramVersion

A Program is the continuing identity of a training program. A ProgramVersion
is editable while a draft and immutable after publication. It contains:

- phase definitions and allowed transitions;
- competencies and tasks;
- evaluation forms;
- rating scales and modifiers;
- narrative requirements;
- completion rules.

PDF template/font metadata remains part of the planned rendering work.

Changes to published configuration require a new draft ProgramVersion, then
publication. Existing enrollments never float silently to it.

### EvaluationForm

An EvaluationForm defines the categories, competencies, task prompts, rating scale, required narratives, and summary sections for one record type.

Daily reports, weekly summaries, and phase evaluations are distinct record types even when they share components.

## People and access

### User

A person with a stable internal identity. Names, employee identifiers, and
titles are profile data. Contact details are not currently modeled.

Finalized records snapshot the presentation values they used.

### CapabilityGrant

Authorization is expressed as capabilities such as:

- `manage_users`;
- `manage_programs`;
- `assign_training`;
- `author_evaluation`;
- `review_evaluation`;
- `view_own_records`;
- `acknowledge_own_record`;
- `view_assigned_records`; and
- `export_records`.

Assignment scope and capability are evaluated together.

## Training

### Enrollment

An Enrollment connects one trainee to one ProgramVersion and records lifecycle transitions.

Concurrent active enrollment is an agency policy, not a universal database assumption. A configuration may limit a trainee to one active operational program.

Changing an enrollment to another ProgramVersion is an explicit event with actor, time, and reason.

### PhaseTransition

Phase history is an event stream. Transitions may advance, return for remediation, restart, pause, resume, or complete.

A phase number is presentation data. The model must not assume progress is strictly monotonic.

### TrainingSession

A TrainingSession describes an actual period of training:

- business or shift date;
- timezone snapshot;
- local representation and UTC start/end instants;
- trainee;
- one or more assigned trainers;
- program and phase context; and
- session disposition.

More than one session may share the same trainee and business date. A session may exist before any evaluation is finalized.

Active training intervals for the same trainee may not overlap. Holdovers, callbacks, and trainer handoffs remain valid as separate or contiguous sessions.

### TaskSignoff

A versioned record that a configured task was observed or demonstrated. Overrides require explicit authority and a recorded reason.

## Evaluations

### EvaluationRecord

The continuing identity of an evaluation. A draft is mutable and may collect contributor and ownership-transfer events.

A record may refer to one or more training sessions. Multiple evaluation records may refer to the same session when policy permits.

### EvaluationVersion

An immutable finalized snapshot containing the complete historical presentation:

- author and contributors;
- trainee identity as presented;
- program, phase, form, competency, and rating definitions;
- observations, ratings, modifiers, and narratives;
- covered sessions;
- an attachments member, currently empty pending attachment support;
- timestamps and local-time representation;
- canonicalization version; and
- integrity metadata.

Corrections create a successor EvaluationVersion.

### WeeklySummary

A weekly summary is its own EvaluationRecord type. It references the exact finalized daily-report versions included in the summary and carries independent narrative, finalization, acknowledgment, and amendment history.

### ContributorEvent

Draft authorship is explicit. Events record creation, edits, ownership transfer, review, and submission without pretending that the final submitter wrote every word.

## Review and acknowledgment

### ReviewDecision

A reviewer may approve, request changes, or return a draft according to configured workflow. Change requests occur before finalization.

### Acknowledgment

An Acknowledgment binds a person to one EvaluationVersion and records one of:

- acknowledged;
- acknowledged with response;
- refused;
- supervisor-attested refusal; or
- unavailable.

Acknowledgment means receipt, not agreement. A successor version requires a new acknowledgment.

### Amendment

An Amendment links an original finalized version to its successor and records the reason, authority, author, and timestamps. The original remains readable and exportable while retained.

## Exports

### RecordExport

A RecordExport is an archive of finalized EvaluationVersions as stored: each version's canonical bytes, unchanged, beside a manifest carrying the installation identity, record and version identity, record schema, both hashes, the predecessor's content hash, and the export instant (`docs/formats/record-export.md`, ADR 0014). Scopes are one version, one record, one enrollment, or the installation. Verifying an export needs nothing but the export; the verdict reports consistency with the stated fingerprints, never tamper-proofing. Every export is audited without record content.

### TraineePacket

A TraineePacket is everything retained about one enrollment as one archive (`docs/formats/trainee-packet.md`, ADR 0015): the record export's units for every retained version of every record, plus typed documents for the enrollment's lifecycle and phase history, every acknowledgment, every amendment, and the full task signoff history, named with hashes by one packet manifest. The trainee may produce their own; so may whoever reads the enrollment's training history and `export_records` holders. It verifies with the same verifier as a record export.

## Retention and disposition (planned)

### RetentionPolicy

A versioned RetentionPolicy maps record classes to an approved disposition authority, trigger, minimum retention period, action, and rules for any destruction log. Installations configure policy; Consolebook does not pretend one jurisdiction's schedule is universal.

### RecordHold

A RecordHold suspends disposition for an explicit scope. Holds may represent litigation, anticipated litigation, audit, investigation, public-records request, or another configured authority. Creating, changing, and releasing a hold requires attribution and a reason.

### DispositionEvent

A DispositionEvent records an authorized disposition attempt and its result. Where policy permits or requires a retained tombstone, it may contain only the minimum approved fields, such as:

- opaque record and version identifiers;
- the former version hash;
- record class and covered date range;
- disposition-authority identifier;
- actor, approver, and timestamp;
- method and result; and
- non-sensitive reason code.

The event does not preserve destroyed narratives, attachments, presentation snapshots, or personal profile data. Disposition events have their own retention policy and may themselves become eligible for disposition.

## Audit

### AuditEvent

Security- and record-sensitive actions produce append-only audit events. The
implemented vocabulary covers authentication and recovery, assignments and
enrollment lifecycle, draft and review workflow, finalization,
acknowledgments, amendments, exports, and backup or restore operations. Hold
and disposition events join that vocabulary with the Milestone 5 retention
slice.

An audit event supplements the immutable domain record. It is not a substitute for version history.

## Database invariants

The embedded migrations, constraints, and triggers enforce the persisted
contract. Among the current invariants:

1. finalized versions cannot be updated or deleted through normal application writes;
2. acknowledgments reference a specific finalized version;
3. successor versions preserve a valid predecessor relationship;
4. published program versions cannot be edited;
5. all referenced configuration versions belong to the pinned program version;
6. UTC end time cannot precede UTC start time;
7. active training intervals for one trainee cannot overlap; and
8. no uniqueness constraint assumes one session or evaluation per trainee and calendar date.

Application services add transactional checks where the invariant spans
authorization, workflow state, or several tables. Migrations are forward-only;
the migration that introduced an invariant remains its historical authority.

## Application-service invariants

1. capability and assignment scope are checked before sensitive reads and writes;
2. `view_own_records` grants a trainee access only to their own retained timeline;
3. all trainers assigned to the same trainee can share the records allowed by policy without receiving broad access to unrelated trainees.

Snapshot-bound authorization is implemented for trainee packets (ADR 0015),
not a universal service guarantee. Other read/write paths need their transaction
boundaries checked before claiming the same property. A write transaction alone
does not ensure an earlier authorization decision belongs to its snapshot.

The Milestone 5 retention slice adds two further invariants: any applicable
hold blocks disposition, and disposition requires explicit capability, policy
authority, scope confirmation, attribution, and a recorded result.
