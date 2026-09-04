# Repository Guidelines

Consolebook is pre-alpha training-record software for emergency communications
centers: one Rust executable, one SQLite data directory, and a static SvelteKit
UI. No required external services, telemetry, or production Node.js runtime.

## Find the relevant context

Start with `git status --short --branch` and the task's issue or PR, including
unresolved review feedback. Preserve unrelated work. Do not assume a previous
session's branch, milestone status, or verification still describes the head.

Read only the context needed for the task:

- Build, source ownership, or local development: `docs/development.md`.
- Next work and milestone status: `docs/roadmap.md`, then the linked GitHub issue.
- Product constraints: `PRINCIPLES.md`; boundaries: `docs/architecture.md`.
- Domain terms: `docs/domain-model.md`; record integrity: `docs/records-integrity.md`.
- Contribution lifecycle, verification, and refactor rules: `CONTRIBUTING.md`.
- Decisions and formats: the task index in `docs/development.md` links the
  relevant ADRs and specifications. Do not load the entire corpus by default.
- Preview deployment: `docs/preview.md`. Host configuration and deployed
  binaries are separate from this checkout.

The server is in `crates/consolebook-server/`: `src/` owns services and the thin
CLI, `migrations/` owns persisted constraints, and `tests/` owns integration
proof. `web/src/` owns the UI and typed API client; `web/e2e/` tests the binary
through a browser. `sessions.rs` means login sessions; `training_sessions.rs`
means training periods.

## Non-negotiable contracts

- Finalized records are immutable while retained. Corrections create successor
  versions or amendments. Lawful disposition is a separate authorized workflow.
- Agency variation is versioned configuration. Finalized records pin configuration
  and presentation snapshots; mutable reference data must not rewrite history.
- Operational dates are agency-local; duration and ordering use UTC instants.
- Capabilities and scope are enforced by domain services. HTTP and UI adapt
  those decisions. Authorization, integrity, retention, disposition, and exports
  require typed contracts and tests; heuristics cannot establish them.
- Startup verifies storage invariants and fails closed. `doctor` must diagnose
  without creating, migrating, or changing state (ADR 0003).
- Use invented agencies, people, incidents, identifiers, and narratives. Real
  records, operational material, personal data, and credentials never enter the
  repository, tests, logs, or issues. Security reports follow `SECURITY.md` privately.

## Work and verification

Non-trivial work requires one primary issue, an issue-linked branch, and a PR.
Search existing issues first; never push directly to `main`. `Closes #...` means
all acceptance criteria are satisfied; otherwise use `Refs #...`.

Build `web/` before Rust when UI or embedding matters. The command sequence and
browser prerequisites are in `CONTRIBUTING.md`. Required checks: `npm ci`,
`npm run check`, `npm run build` in `web/`; `cargo fmt --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, and
`cargo test --workspace`. Browser tests need the compiled debug binary.
Keep the pinned Rust toolchain in `rust-toolchain.toml`.

Report only verification actually run, with exact failures. Investigate
intermittent failures. Design documents are not runtime proof; unit tests are
not recovery drills. Fix causes and in-scope defects; file exact-evidence
issues for separate work.

Follow `CONTRIBUTING.md` before adding behavior to large Rust modules: over
1,000 lines requires an ownership-based reorganization, over 1,500 requires
naming the boundary first, and over 2,500 requires a reviewed decomposition
path before major feature work unless urgent. Refactors state the new owner,
persisted/public impact, and focused proof.

Durable behavior changes require an ADR; changes to `PRINCIPLES.md` require one.
Use forward migrations, standard Rust formatting, `thiserror` for library
errors as they emerge, `anyhow` at application boundaries, and no `unsafe`.
Use short imperative Conventional Commit subjects. Logs exclude sensitive
content; existing first-run setup-code output is the documented exception
(ADR 0004), not permission for additional secret logging.

This file owns the concise shared contract; `CONTRIBUTING.md` owns workflow,
`docs/development.md` owns the source map, and ADRs own decisions. Tool-specific
entrypoints (`CLAUDE.md`, `.agents/rules/`, `.github/copilot-instructions.md`)
only point here. Keep machine preferences and session handoffs out of this file.
