# Domain Model

Consolebook uses an opinionated training domain with versioned agency configuration. The names below are working vocabulary, not final table names.

## Configuration

### Program and ProgramVersion

A Program is the continuing identity of a training program. A ProgramVersion is an immutable configuration snapshot containing:

- phase definitions and allowed transitions;
- competencies and tasks;
- evaluation forms;
- rating scales and modifiers;
- narrative requirements;
- completion rules; and
- document presentation metadata.

Publishing a change creates a new ProgramVersion. Existing enrollments never float silently to it.

### EvaluationForm

An EvaluationForm defines the categories, competencies, task prompts, rating scale, required narratives, and summary sections for one record type.

Daily reports, weekly summaries, and phase evaluations are distinct record types even when they share components.

## People and access

### User

A person with a stable internal identity. Names, employee identifiers, titles, and contact details are mutable profile data.

Finalized records snapshot the presentation values they used.

### CapabilityGrant

Authorization is expressed as capabilities such as:

- `manage_users`;
- `manage_programs`;
- `assign_training`;
- `author_evaluation`;
- `review_evaluation`;
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
- attachments and their hashes;
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

An Amendment links an original finalized version to its successor and records the reason, authority, author, and timestamps. The original remains readable and exportable.

## Audit

### AuditEvent

Security- and record-sensitive actions produce append-only audit events, including authentication, authorization changes, finalization, acknowledgment, refusal, amendment, export, backup, and restore.

An audit event supplements the immutable domain record. It is not a substitute for version history.

## Database invariants to enforce

The initial schema is expected to enforce at least:

1. finalized versions cannot be updated or deleted through normal application writes;
2. acknowledgments reference a specific finalized version;
3. successor versions preserve a valid predecessor relationship;
4. published program versions cannot be edited;
5. all referenced configuration versions belong to the pinned program version;
6. UTC end time cannot precede UTC start time;
7. capability and assignment checks occur before sensitive reads and writes; and
8. no uniqueness constraint assumes one session or evaluation per trainee and calendar date.

The exact enforcement mechanism—constraints, triggers, or transactional application services—will be decided with migration `0001`.
