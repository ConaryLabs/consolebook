# Development Guide

This is the source-ownership map for contributors and coding agents. Product
rules live in `PRINCIPLES.md`, domain terms in `docs/domain-model.md`, integrity
rules in `docs/records-integrity.md`, and durable decisions in
`docs/decisions/`. This guide says where the implementation lives and how its
pieces call one another.

## Runtime and request flow

Consolebook is one Rust process serving a static single-page application and a
versionless JSON API from the same origin:

```text
browser route
  -> web/src/lib/api.ts
  -> /api/* handler in http.rs or a *_http.rs module
  -> domain service in a capability-owned Rust module
  -> SQLx transaction
  -> SQLite constraints and triggers in migrations/
```

`crates/consolebook-server/src/main.rs` is the CLI boundary. `serve` resolves
one `DataDir`, acquires the installation lock, opens and migrates SQLite,
verifies connection invariants, starts automatic backups, and hands the pool
to the Axum router. The router falls back to `web_assets.rs`, which serves the
SvelteKit build and sends `index.html` for client-side routes.

Policy belongs in domain services, not HTTP handlers or Svelte components.
Handlers translate transport shapes and typed refusals. Database constraints
and triggers are the final authority for persisted invariants. The web client
does not establish authorization or record integrity.

## Repository map

```text
crates/consolebook-server/
├── src/          library modules and the thin CLI entry point
├── migrations/   embedded, ordered SQLite schema and trigger authority
└── tests/        integration tests against public/runtime paths
web/
├── src/lib/      the typed same-origin API client and shared UI helpers
├── src/routes/   SvelteKit pages and the client-side routing guard
└── e2e/          browser tests against the compiled Rust executable
docs/
├── decisions/    ADRs for durable behavior
└── formats/      normative portable-format specifications
```

The Rust modules are organized by ownership rather than by table or route:

- Process and storage: `main.rs`, `data_dir.rs`, `serve_lock.rs`,
  `storage.rs`, and the embedded migrations.
- Operations: `backup.rs`, `scheduler.rs`, `restore.rs`, and `doctor.rs`.
- Local identity and access: `setup.rs`, `users.rs`, `secrets.rs`,
  `sessions.rs`, `capabilities.rs`, `assignments.rs`, and `audit.rs`.
- Program configuration: `programs.rs`, `program_export.rs`, and
  `programs_http.rs`.
- Training lifecycle: `enrollments.rs`, `lifecycle.rs`,
  `training_sessions.rs`, `session_time.rs`, `session_membership.rs`, and
  `training_http.rs`.
- Draft workflow: `evaluation_drafts.rs`, `draft_access.rs`,
  `draft_content.rs`, `draft_review.rs`, and `drafts_http.rs`.
- Defensible records: `canonical.rs`, `finalization.rs`,
  `record_envelope.rs`, `acknowledgments.rs`, `amendments.rs`, `summaries.rs`,
  and `task_signoffs.rs`.
- Portable records: `record_export.rs`, `trainee_packet.rs`,
  `export_verify.rs`, `packet_verify.rs`, `zip_container.rs`, and
  `exports_http.rs`.
- HTTP and interface shell: `http.rs`, the three domain `*_http.rs` modules,
  `exports_http.rs`, `notices.rs`, and `web_assets.rs`.

Two names are easy to confuse: `sessions.rs` owns authenticated login sessions;
`training_sessions.rs` owns periods of on-the-job training. Likewise,
`programs.rs` owns program behavior while `programs_http.rs` only adapts it to
HTTP.

`http.rs` is intentionally a router/error/authentication hub. Larger route
families register through `programs_http.rs`, `training_http.rs`,
`drafts_http.rs`, and `exports_http.rs`. New domain behavior should not make
the hub its owner.

## Web ownership

`web/src/routes/+layout.ts` is the client-side setup and authentication guard;
`+layout.svelte` owns the shared shell and primary navigation.
`web/src/lib/api.ts` is the typed boundary for every HTTP call. Page ownership
is:

- `/setup`, `/login`, and `/reset`: installation and local-authentication entry;
- `/`: capability-sensitive status, notices, user administration, session and
  review queues, and installation exports;
- `/programs/**`: program authoring, comparison, publishing, and enrollment;
- `/enrollments/[id]`: one enrollment's lifecycle and training workflow;
- `/drafts/[id]`: draft authoring, review, and finalization;
- `/records`: the signed-in trainee's records and packet downloads.

There is no production Node.js server. SvelteKit builds a static SPA into
`web/build`; Rust embeds those files. Build the web app before Rust whenever
interface behavior or asset serving matters.

## Run a fresh local installation

From the repository root:

```sh
(cd web && npm ci && npm run build)
cargo run -p consolebook-server -- --data-dir ./data serve
```

The server binds `127.0.0.1:7770` by default. On a new data directory it prints
a short-lived setup code. Open <http://127.0.0.1:7770>, enter that code, and
create the invented local agency and first administrator. Never use real
agency or personnel data in a development installation or screenshot.

Use a separate empty `--data-dir` for disposable previews. `serve`, unlike
`doctor`, may create the directory, migrate the database, issue setup material,
and start the backup scheduler.

Useful operator commands are:

```sh
cargo run -p consolebook-server -- --data-dir ./data doctor
cargo run -p consolebook-server -- --data-dir ./data backup
cargo run -p consolebook-server -- --data-dir ./data restore <snapshot>
cargo run -p consolebook-server -- export verify <archive>
```

`doctor` is read-only and never creates or migrates an installation. Restore
requires the server to be stopped. Export verification reads the archive and
does not open an installation.

## Verification map

Run the repository gates in build order:

```sh
(cd web && npm ci && npm run check && npm run build)
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Rust integration tests in `crates/consolebook-server/tests/` mirror the major
domain capabilities and exercise the public service, HTTP, CLI, or storage
boundary appropriate to the contract. `web/e2e/` drives the compiled binary in
a real browser and supplies a scratch data directory; build both web and Rust
first, then run from `web/`:

```sh
CONSOLEBOOK_E2E_CHROMIUM=/path/to/chromium npm run e2e
```

Do not report a command as verified unless it actually ran. A format document
does not prove serialization, a unit test does not prove recovery, and a UI
check does not prove service-layer authorization. Apparently software remains
stubbornly unimpressed by good intentions.

## Choosing authority before editing

- Change a product invariant only through `PRINCIPLES.md` plus an ADR.
- Change durable behavior with the relevant ADR and focused contract tests.
- Change a portable archive by updating its `docs/formats/` specification,
  producer, file-only verifier, and fixtures together.
- Change persisted behavior through a forward migration; never rewrite an
  applied migration.
- Change a capability-sensitive operation in the domain service first, then
  adapt HTTP and web layers.
- Check `docs/roadmap.md` and the primary milestone issue before starting the
  next slice; GitHub issue state is more transient than tracked architecture.

