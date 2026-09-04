# Development Guide

This map routes contributors and agents to the implementation and its
authorities. [AGENTS.md](../AGENTS.md) owns repository rules;
[CONTRIBUTING.md](../CONTRIBUTING.md) owns build gates and contribution workflow.

## Choose context by task

Read the relevant row, then the specific service, migration, and test involved.
ADRs record decisions; format documents specify portable bytes; source and
tests show what is implemented. [Roadmap](roadmap.md) owns milestone status.

| Task | Start in `crates/consolebook-server/src/` | Supporting context |
| --- | --- | --- |
| Process, storage, diagnostics | `main.rs`, `data_dir.rs`, `storage.rs`, `doctor.rs` | [Architecture](architecture.md), [ADR 0003](decisions/0003-sqlite-connection-invariants.md), [ADR 0016](decisions/0016-read-only-diagnostics.md) |
| Backups and restore | `backup.rs`, `scheduler.rs`, `restore.rs`, `serve_lock.rs` | [ADR 0006](decisions/0006-backup-scheduling-and-restore.md) |
| Setup, login, recovery | `setup.rs`, `users.rs`, `sessions.rs`, `secrets.rs` | [ADR 0004](decisions/0004-local-authentication.md) |
| Capabilities and assignments | `capabilities.rs`, `assignments.rs`, `draft_access.rs` | [ADR 0010](decisions/0010-service-owned-authorization-boundary.md), [Domain model](domain-model.md) |
| Program configuration | `programs.rs`, `program_export.rs` | [ADR 0007](decisions/0007-program-version-configuration-model.md), [Program format](formats/program-version-export.md) |
| Enrollment and training sessions | `enrollments.rs`, `lifecycle.rs`, `training_sessions.rs`, `session_membership.rs`, `session_time.rs` | [ADR 0008](decisions/0008-session-draft-and-attribution-model.md), [ADR 0009](decisions/0009-session-local-time-resolution.md) |
| Drafts and review | `evaluation_drafts.rs`, `draft_content.rs`, `draft_review.rs` | [ADR 0008](decisions/0008-session-draft-and-attribution-model.md), [ADR 0010](decisions/0010-service-owned-authorization-boundary.md) |
| Finalization and canonical bytes | `finalization.rs`, `canonical.rs`, `record_envelope.rs` | [Integrity](records-integrity.md), [ADR 0011](decisions/0011-canonical-record-format-and-finalization.md) |
| Acknowledgments and amendments | `acknowledgments.rs`, `amendments.rs` | [Domain model](domain-model.md), [ADR 0012](decisions/0012-amendment-reopening-state-machine.md) |
| Summaries and signoffs | `summaries.rs`, `task_signoffs.rs` | [ADR 0013](decisions/0013-weekly-summaries-and-task-signoffs.md) |
| Record exports | `record_export.rs`, `export_verify.rs`, `zip_container.rs` | [ADR 0014](decisions/0014-record-export-format.md), [Export format](formats/record-export.md) |
| Trainee packets | `trainee_packet.rs`, `packet_verify.rs` | [ADR 0015](decisions/0015-trainee-packet.md), [Packet format](formats/trainee-packet.md) |
| Retention, holds, disposition (planned) | No implemented service yet | [Integrity](records-integrity.md), [Milestone 5 decisions](https://github.com/FieldmouseWorks/consolebook/issues/44) |
| Web shell and HTTP | `http.rs`, `web_assets.rs`, `notices.rs`, domain `*_http.rs` modules | [ADR 0005](decisions/0005-embedded-web-interface.md), web map below |
| Preview operations | Separate host installation | [Preview runbook](preview.md) |

`lib.rs` exposes the library modules. Integration tests in
`crates/consolebook-server/tests/` are named by capability; migration files in
`crates/consolebook-server/migrations/` own schema, constraints, and triggers.
Read both when changing a persisted contract.

## Runtime flow

```text
Svelte route -> web/src/lib/api.ts -> /api/* HTTP adapter
             -> domain service -> SQLx -> SQLite constraints and triggers
```

`main.rs` owns the CLI. `serve` resolves the data directory, acquires the serve
lock, opens and migrates SQLite, verifies connection invariants, starts the
backup scheduler, and serves Axum. `http.rs` owns router registration, the
current-user extractor, and error translation. Larger handler groups live in
`programs_http.rs`, `training_http.rs`, `drafts_http.rs`, and `exports_http.rs`.
Policy belongs in services; persisted constraints also have database backstops.
`audit.rs` owns typed audit events; `notices.rs` owns recipient-scoped notices.

`sessions.rs` owns login sessions; `training_sessions.rs` owns periods of
training. Do not infer policy from a role name or a UI guard.

New write paths use `storage::write_tx` and await rollback on refusal through
`storage::refuse`. Earlier deferred write transactions still need the
[#27 retrofit](https://github.com/FieldmouseWorks/consolebook/issues/27).
A transaction's presence alone does not prove authorization shares its
snapshot; check where the decision is evaluated. See the
[domain-model qualification](domain-model.md#application-service-invariants).

## Web map

The UI is a client-routed SPA. `web/src/routes/+layout.ts` guards setup and
authentication; `+layout.svelte` owns navigation and shared styling.
`web/src/lib/api.ts` owns typed same-origin HTTP calls.
`web/src/lib/editor/` contains program-authoring components.

`web/e2e/fixtures.ts` supplies each scenario's server, base URL, and setup code.
`server.ts` owns process startup and scratch-data cleanup; `server.spec.ts`
checks startup failures, listener ownership, and shutdown. Keep scenario data
and assertions in their own specs.

| Route | Ownership |
| --- | --- |
| `/setup`, `/login`, `/reset` | Installation and authentication entry |
| `/` | Capability-sensitive status, notices, administration, session/review queues, installation exports |
| `/programs/**` | Program authoring, comparison, publishing, enrollment |
| `/enrollments/[id]` | Lifecycle, assignments, sessions, summaries, signoffs, exports |
| `/drafts/[id]` | Authoring, review, finalized presentation, acknowledgment, amendments |
| `/records` | Trainee's own timeline and packet downloads |

## Local workflow

From the repository root:

```sh
(cd web && npm ci && npm run build)
cargo run -p consolebook-server -- --data-dir ./data serve
```

Open <http://127.0.0.1:7770> on the same machine. For a fresh installation,
use the setup code printed by the server, create invented agency/admin data,
then sign in. Use a separate empty data directory for a disposable preview.
The published preview already occupies port 7770 on its host; choose another
`serve --bind 127.0.0.1:PORT` there.

For live UI editing, run `npm run dev` in `web/` with a local Rust server.
`vite.config.ts` proxies `/api` to port 7770; adjust the target when using a
different local port. Check that target before using the dev UI on a shared
host. `npm run preview` alone serves static files without that API proxy.

`web_assets.rs` uses the build in `web/build/`: release builds embed it;
debug builds read it from disk. A missing build serves the explicit
"interface not embedded" notice. Rebuild web then Rust for release packaging.
Node.js is never required by the deployed binary.

See [CONTRIBUTING.md](../CONTRIBUTING.md#build-and-verification) for the full
verification sequence and browser prerequisites.
`cargo run -p consolebook-server -- --help` lists CLI operations;
`export verify <archive>` reads a file without opening an installation.
