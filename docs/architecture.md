# Architecture

This document describes the current design target. It is not an implementation receipt.

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
  +-- authorization and audit
  +-- exports and recovery
  |
  v
SQLite database and local data directory
```

The application should remain useful without Redis, a message broker, a Node.js runtime, a hosted identity provider, or a network connection to the project maintainers.

## Planned components

### Application

Rust owns the process lifecycle, configuration, HTTP API, migrations, background maintenance, backups, exports, and embedded assets.

Axum is the planned HTTP framework. Application boundaries should follow domain capabilities rather than mirror web routes.

### Storage

SQLite is the default operational database.

Connections must be created from one explicit options object that enables and verifies:

- foreign-key enforcement;
- WAL journaling;
- an intentional synchronous mode;
- a bounded busy timeout; and
- application-owned migrations.

Startup and the future `consolebook doctor` command will verify these invariants.

### User interface

The planned interface is a statically built SvelteKit application embedded in the Rust executable. Server-side rendering and a production Node.js runtime are outside the design.

### Documents

Typst is the planned renderer for stable PDF exports. Templates and redistribution-friendly fonts will ship with the application.

A PDF is a presentation of a record version. The structured record remains independently exportable.

## Data directory

The intended layout is deliberately boring:

```text
data/
├── consolebook.db
├── backups/
├── exports/
└── instance/
```

Exact paths and retention policies remain undecided.

## Backups

Backups will be automatic and default-on.

The current design preference is a consistent SQLite snapshot produced with `VACUUM INTO`, followed by integrity validation, an explicit durability step, and retention management. Restore must be a tested product workflow.

## Authentication

Milestone one targets local authentication:

- username or email;
- Argon2id password hashes;
- cryptographically random opaque session tokens;
- HttpOnly cookies;
- server-side session records;
- expiration and immediate revocation.

OIDC may be added behind an authentication-provider boundary later.

## Authorization

Roles are convenient bundles of capabilities. Domain services authorize capabilities and assignment scope rather than scattering role-name comparisons.

The initial vocabulary is expected to include Administrator, Coordinator, Trainer, and Trainee, but the capability model is authoritative.

## First-run setup

An uninitialized installation will emit a short-lived setup code. Creating the first agency settings and administrator must be a single transaction that invalidates the setup code.

After initialization, the setup operation is unavailable.

## Deployment boundary

The canonical artifact is one executable. Containers and service-manager examples may be provided, but neither defines the architecture.

Reverse proxies and external TLS termination are supported deployment choices. They are not required for local development.
