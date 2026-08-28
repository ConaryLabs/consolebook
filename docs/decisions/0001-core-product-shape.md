# ADR 0001: Core product shape

- **Status:** Accepted
- **Date:** 2026-08-27

## Context

Emergency communications centers need structured training records that remain understandable and exportable for years. Centers vary in terminology, program length, phases, rating scales, and review workflow.

Small centers may have limited technical staff. The product must be deployable without requiring a distributed platform or a permanent vendor relationship.

Finalized evaluations can become consequential personnel records. Immutability, provenance, acknowledgment, amendment, export, and recovery therefore shape the initial schema.

## Decision

Consolebook will be a single-agency, self-contained modular monolith.

The core domain will be opinionated rather than a generic form builder. Agency variation will be represented as immutable, versioned program configuration.

The default deployment will use:

- one Rust executable;
- an embedded static web interface;
- one SQLite database in an application-owned data directory;
- in-process document rendering; and
- automatic local backups.

Each hosted customer, if hosting is offered later, will receive an isolated Consolebook instance rather than rows in a shared multi-tenant schema.

Finalized record versions will be immutable. Corrections will create successor versions, and acknowledgments will bind to exact versions.

## Consequences

### Positive

- deployment and recovery remain understandable;
- agency data has a clean isolation boundary;
- the schema avoids speculative tenancy fields;
- operational dependencies are minimal;
- historical program definitions remain reproducible; and
- the same executable can support self-hosted and isolated hosted deployments.

### Costs

- operating many hosted instances may require later orchestration;
- cross-agency analytics are outside the instance boundary;
- configuration-version tooling must be designed early; and
- SQLite concurrency and backup behavior require deliberate engineering and proof.

## Rejected alternatives

### Shared multi-tenant application

Rejected because it adds pervasive tenancy concerns before the product has one working agency installation and weakens the default isolation story.

### Generic form builder

Rejected because it pushes essential training concepts into arbitrary fields and makes invariants, exports, and migrations unreliable.

### Cloud-first service decomposition

Rejected because it increases deployment and recovery burden without serving the initial domain.

## Follow-up decisions

Separate ADRs will cover canonical record bytes, authentication, database durability settings, backup/restore mechanics, PDF determinism, and signing-key lifecycle if signatures are introduced.
