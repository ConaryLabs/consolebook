# Architecture

This document describes the implemented system boundary and the remaining
design target. It is not an implementation receipt; tests and operator drills
prove runtime claims.

## Shape

Consolebook is a modular monolith deployed as one application instance per agency.

```text
browser
  |
  v
embedded static UI
  |
  v
HTTP API and application services
  |
  +-- training programs and enrollments
  +-- sessions and evaluation workflow
  +-- immutable record versions
  +-- acknowledgments and amendments
  +-- holds, retention, and lawful disposition (planned)
  +-- authorization and audit
  +-- in-app notifications
  +-- exports and recovery
  |
  v
SQLite database and local data directory
```

The application should remain useful without Redis, a message broker, a Node.js runtime, a hosted identity provider, or a network connection to the project maintainers.

## Implemented components

### Application

Rust owns the process lifecycle, configuration, HTTP API, migrations,
background maintenance, backups, exports, and embedded assets.

Axum serves versionless `/api/` routes and the embedded interface from one
listener. Application boundaries follow domain capabilities rather than mirror
web routes: handlers translate HTTP and services own policy. The detailed
source map lives in `docs/development.md`.

### Storage

SQLite is the operational database.

Connections must be created from one explicit options object that enables and verifies:

- foreign-key enforcement;
- WAL journaling;
- an intentional synchronous mode;
- a bounded busy timeout; and
- application-owned migrations.

Startup verifies these invariants and fails closed. ADR 0003 requires
`consolebook doctor` to inspect without changing state. It does not create or
migrate a database, but its current connection path can change a non-WAL
database's journal mode; [#56](https://github.com/FieldmouseWorks/consolebook/issues/56)
tracks restoring the read-only contract.

### User interface

The interface is a statically built SvelteKit single-page application embedded
in the Rust executable. Server-side rendering and a production Node.js runtime
are outside the design (ADR 0005).

The web interface is part of every vertical slice, not a post-API decoration. Setup, program configuration, training workflow, trainee review, retention administration, and recovery each require a usable interface before their milestone is complete.

### Records and portable documents

Finalization stores versioned canonical JSON bytes and SHA-256 content and chain
hashes. Acknowledgments, amendments, weekly summaries, and task signoffs remain
separate typed history. Structured record exports and trainee packets carry
stored bytes verbatim and verify from the archive alone (ADRs 0011–0015).

Typst is the planned renderer for stable PDF presentations. Templates and
redistribution-friendly fonts will ship with the application in a later
Milestone 5 slice.

A PDF is a presentation of a record version. The structured record remains independently exportable.

## Data directory

The layout is deliberately boring:

```text
data/
├── consolebook.db
├── backups/
├── exports/
└── instance/
```

`DataDir` owns these paths. SQLite and application state live under this one
root; runtime services do not require an external database, queue, cache, or
object store. Retention policies for records remain Milestone 5 work.

## Backups

Backups are automatic and default-on while the server runs.

The implemented pipeline produces a consistent SQLite snapshot with `VACUUM
INTO`, validates it, performs an explicit durability step, and prunes by
configured count (ADRs 0003 and 0006). Manual backup and stopped-server restore
use the same library paths as the CLI. Count-plus-age retention and clean-room
restore verification remain Milestone 5 work.

## Authentication

Local authentication implements:

- username;
- Argon2id password hashes;
- cryptographically random opaque session tokens;
- HttpOnly cookies;
- server-side session records;
- expiration and immediate revocation.

Password recovery in v1 is local and administrator-operated:

- an authorized administrator can issue a short-lived, single-use reset code;
- using the code forces a new password, revokes existing sessions, and creates an audit event; and
- a recovery command for administrator accounts requires operating-system
  access to the data directory and records an explicit recovery event. It is
  not restricted to installations with only one administrator.

Password reset does not depend on email or another external service.

OIDC may be added behind an authentication-provider boundary later.

## Authorization

Roles are convenient bundles of capabilities. Domain services authorize capabilities and assignment scope rather than scattering role-name comparisons.

The initial roles are Administrator, Coordinator, Trainer, and Trainee, but the
capability model is authoritative. Services combine capabilities with
assignment, session-membership, or own-record scope as the operation requires
(ADR 0010).

## First-run setup

An uninitialized installation emits a short-lived setup code. Creating the
first agency settings and administrator is one transaction that invalidates
the setup code.

After initialization, the setup operation is unavailable.

## Notifications

Workflow notices are persisted and shown in the application. Finalization, review requests, successor versions, acknowledgment requests, responses, refusals, and escalations cannot depend on email delivery.

SMTP may be added later as an optional delivery adapter. It mirrors an in-app notice; it does not become the record or the only way to act.

## Retention and disposition

Retention policy, record holds, and lawful disposition are the next Milestone 5
slice. They belong to application services with explicit capabilities and
audit events. Normal service methods and database triggers already reject
mutation or deletion of finalized content. The separate disposition path will
check applicable policy and holds, preview scope, record authority, and remove
only the approved material.

Disposition records have retention rules of their own. The architecture must not keep personal metadata forever merely to make an integrity chain convenient.

## Deployment boundary

The canonical artifact is one executable. Containers and service-manager examples may be provided, but neither defines the architecture.

Reverse proxies and external TLS termination are supported deployment choices. They are not required for local development.

The version/about interface will identify the running build, the AGPL-3.0-only license, and a source location. Deployments that modify Consolebook and make it available over a network must be able to point that interface at the Corresponding Source for the running version.
