# Repository Guidelines

## Start With The Smallest Useful Context

Consolebook is a small Rust workspace plus one web app: the server lives
in `crates/consolebook-server/` (library modules plus a thin CLI in
`main.rs`, integration tests in `tests/`, embedded migrations in
`migrations/`), and the embedded SvelteKit interface lives in `web/`
(built statically, embedded by the Rust build; Node.js is build-time
only).

Durable truth lives in a few files; read only what the task needs:

- `PRINCIPLES.md` — non-negotiable product constraints
- `docs/development.md` — runtime flow, source ownership, and local workflow
- `docs/architecture.md` — system boundaries and design targets
- `docs/domain-model.md` — domain vocabulary and invariants
- `docs/records-integrity.md` — immutability, hashes, provenance
- `docs/roadmap.md` — milestone sequence and current position
- `docs/decisions/` — architecture decision records

## Build And Verification

- `cargo build -p consolebook-server` builds the server.
- `cargo test --workspace` runs the tests.
- `cargo fmt --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` are repository
  gates; clippy pedantic is enabled workspace-wide.
- In `web/`: `npm ci`, `npm run check`, and `npm run build` are gates;
  build `web/` before cargo when interface behavior matters (a bare cargo
  build compiles but serves an honest "not embedded" notice).
- `npm run e2e` in `web/` drives the compiled binary through the shell in
  a real browser (set `CONSOLEBOOK_E2E_CHROMIUM` to a Chromium path when
  Playwright's own download is unavailable).
- The toolchain is pinned in `rust-toolchain.toml`; do not float it.

Verification means the reported command actually ran. Preserve exact failure
evidence and explain the causal leaf failure. A design document is not runtime
proof, and a passing unit test is not recovery proof.

## Issue, Branch, And Pull-Request Workflow

Follow `CONTRIBUTING.md`. Non-trivial implementation, bug, refactor,
documentation, operations, and maintenance work uses one primary GitHub
issue, an issue-linked branch, and a pull request; never push repository
changes directly to `main`. Search existing issues first. `Closes #...`
means the PR satisfies the issue's acceptance criteria; otherwise use
`Refs #...` and leave the issue open.

Security reports use private advisories per `SECURITY.md`, never public
issues.

## Product And Authority Contract

Consolebook is pre-alpha training-record software for emergency
communications centers. `PRINCIPLES.md` is the authority; the load-bearing
consequences for code:

- Finalized records are immutable while retained. Corrections are successor
  versions or amendments; deletion is lawful disposition, a separate
  authorized workflow. Never an in-place edit.
- Agency variation is versioned configuration, never agency-specific code
  paths or hidden conditionals.
- Finalized records pin the exact configuration versions used to create
  them; mutable reference data must not rewrite history.
- Operational dates carry agency-local meaning; duration and ordering use
  UTC instants. Do not conflate them.
- One executable, one data directory, SQLite. No required external
  services, telemetry, or cloud dependencies.

Engineer solutions, not band-aids. Heuristics, regexes, substring matching,
and silent defaults may aid diagnostics, discovery, or presentation; they
may not establish record integrity, authorization, retention, disposition,
or export behavior. Those are typed contracts with tests. A failure in a
typed check is a defect to engineer, not a class of input to route around.

## Defect And Maintainability Discipline

- Fix a defect, duplicated authority, or half-implementation found in
  scope. File an exact-evidence issue when it belongs elsewhere; do not
  silently route around it.
- Fix causes and prove the contract or property, not only the observed
  input.
- Treat intermittent or unexplained failures as evidence of a defect, not
  as a reason to retry until green.
- A slice adding behavior to a Rust source file over 1,000 lines must
  include an ownership-based reorganization in the same issue or plan.
  Thin dispatch, registration, and re-export wiring may remain in a large
  hub.
- Before changing behavior in a Rust file over 1,500 lines, name the
  ownership boundary being preserved or improved. Files over 2,500 lines
  need a reviewed decomposition path before major feature work unless the
  fix is urgent.
- Refactors name what moves, its new owner, persisted/public impact, and
  the focused proof.
- Decisions that change durable system behavior get an ADR in
  `docs/decisions/`; changes to `PRINCIPLES.md` require one.

## Rust And CLI Conventions

Use standard Rust formatting and naming, `thiserror` for library errors as
they emerge, and `anyhow` at application boundaries. `unsafe` is forbidden
workspace-wide. Keep modules focused on one domain capability. Use short
imperative Conventional Commit subjects such as
`storage(backup): validate snapshot before fsync`.

Structured `tracing` logs never contain record content, personal data, or
credentials. Startup verifies database connection invariants and fails
closed; `doctor` diagnoses read-only and never creates or migrates state
(ADR 0003).

## Documentation And Safety

`AGENTS.md` is the concise repo-wide contract; `CONTRIBUTING.md` owns the
full contribution lifecycle; `docs/development.md` owns the implementation
map; ADRs own decisions. Tool entrypoints
(`CLAUDE.md`, `.agents/rules/`, `.github/copilot-instructions.md`) stay
thin and point back here.

All fixtures, examples, tests, and seed data use invented agencies, people,
incidents, narratives, and identifiers. Real training records, operational
material, credentials, or personal information never enter this repository
in any form — code, docs, tests, logs, or issue text.
