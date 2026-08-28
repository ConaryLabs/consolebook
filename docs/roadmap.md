# Roadmap

The roadmap is ordered by risk. Dates come later; fake schedules are how software projects begin lying to themselves.

## Milestone 0 — Foundation

- establish principles and architecture decisions;
- define public domain vocabulary;
- create a buildable Rust workspace;
- select a license;
- define contribution and security-reporting paths; and
- specify canonical record bytes.

**Exit:** the project can explain what it is building and what it refuses to become.

## Milestone 1 — Operable shell

- command-line configuration;
- first-run setup code;
- local administrator creation;
- SQLite migrations and explicit connection invariants;
- health and `doctor` commands;
- structured logging without sensitive record content; and
- automatic validated backups with a tested restore path.

**Exit:** an empty installation can initialize, restart, back up, restore, and diagnose itself.

## Milestone 2 — Versioned program configuration

- programs and immutable program versions;
- phases and non-linear transitions;
- competencies, tasks, forms, and rating scales;
- publishing and enrollment pinning; and
- configuration export/import.

**Exit:** a complete invented training program can be published, enrolled, exported, and reproduced.

## Milestone 3 — Sessions and drafts

- users, capabilities, and assignment-scoped access;
- enrollments and phase history;
- training sessions with explicit time semantics;
- daily evaluation drafts;
- contributor and ownership-transfer history;
- manual and automatic draft persistence; and
- review/change-request workflow.

**Exit:** trainers can document an invented session collaboratively without losing attribution.

## Milestone 4 — Defensible records

- canonical record format;
- immutable finalized versions;
- rating and narrative rules;
- weekly summaries linked to daily versions;
- acknowledgments, responses, and refusals;
- amendments and successor versions;
- append-only audit events; and
- database-enforced immutability.

**Exit:** the full lifecycle is reproducible and mutation attempts fail closed.

## Milestone 5 — Exports and recovery

- deterministic PDF records;
- complete structured exports;
- trainee packet generation;
- attachment integrity;
- scheduled backup retention;
- restore verification; and
- operator documentation.

**Exit:** a center can leave with all of its data and can prove recovery from a clean installation.

## Milestone 6 — Pilot hardening

- accessibility and usability review;
- threat modeling;
- privacy review;
- performance and concurrency tests;
- migration compatibility policy;
- deployment packages for common environments; and
- external security assessment.

**Exit:** explicit owner acceptance and real recovery evidence support a production-readiness decision.

## Held until justified

- OIDC;
- installation-level signing keys;
- hosted instance management;
- advanced analytics;
- integrations with external personnel systems; and
- configurable custom roles.

These are valid future capabilities. None gets to infect migration `0001` merely because it sounds enterprise-shaped.
