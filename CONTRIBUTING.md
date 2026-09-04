# Contributing

Consolebook is pre-alpha. Follow [AGENTS.md](AGENTS.md) and the product
constraints in [PRINCIPLES.md](PRINCIPLES.md). The task index and source map in
[docs/development.md](docs/development.md) route to additional context.

## Issue, branch, and pull-request workflow

Non-trivial implementation, bug, refactor, documentation, operations, and
maintenance work uses one primary GitHub issue, an issue-linked branch, and a
pull request. Never push repository changes directly to `main`.

- Inspect the checkout and existing PR reviews; preserve unrelated edits.
- Search open and closed issues before filing a new one.
- Discuss broad domain or architecture changes before writing a large patch.
- `Closes #...` means the PR satisfies the issue's acceptance criteria; use
  `Refs #...` for a slice that leaves the issue open.
- Use short imperative Conventional Commit subjects, such as
  `storage(backup): validate snapshot before fsync`.
- Security reports use private advisories per [SECURITY.md](SECURITY.md).

## Engineering discipline

Fix causes and prove the contract, including in-scope defects and duplicated
authority. File exact-evidence issues for separate work. Heuristics may assist
discovery or presentation; authorization, integrity, retention, disposition,
and exports require typed contracts. Investigate intermittent failures rather
than retrying until green.

Keep modules focused on a domain capability. For Rust source files:

- Adding behavior to a file over 1,000 lines requires an ownership-based
  reorganization in the same issue or plan. Thin dispatch, registration, and
  re-export wiring may remain in a large hub.
- Before changing behavior in a file over 1,500 lines, name the ownership
  boundary being preserved or improved.
- Files over 2,500 lines require a reviewed decomposition path before major
  feature work unless the fix is urgent.
- Refactors name what moves, its new owner, persisted/public impact, and the
  focused proof. Avoid unrelated rewrites.

Decisions changing durable behavior get an ADR in `docs/decisions/`; changes
to `PRINCIPLES.md` require one. Schema changes use forward migrations. Portable
format changes update the specification, producer, verifier, and fixtures
together, with versioning governed by that format's contract.

## Build and verification

Use the Rust toolchain pinned in `rust-toolchain.toml` and npm's committed
lockfile. The Vite 7 dependency declares Node.js `^20.19.0 || >=22.12.0`;
use a compatible supported Node.js release. Node.js is build-time only.

From the repository root, run in this order:

```sh
(cd web && npm ci && npm run check && npm run build)
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p consolebook-server
(cd web && npx playwright install chromium && npm run e2e)
```

The explicit build supplies `target/debug/consolebook-server` for the browser
tests. The shared fixture starts a scratch installation per test on an
OS-assigned loopback port and waits for shutdown before deleting its data.
`npm run check` checks both Svelte code and browser-test TypeScript.
If a system Chromium is available, skip the browser download and run:

```sh
(cd web && CONSOLEBOOK_E2E_CHROMIUM=/path/to/chromium npm run e2e)
```

Linux browser dependencies can be installed with
`npx playwright install --with-deps chromium` from `web/`.
Run Unix diagnostic permission tests as an unprivileged user; they assert that
filesystem permissions deny writes, which a privileged runner can bypass.
The [pr-gate workflow](.github/workflows/pr-gate.yml) runs the web, Rust, and
browser checks on pull requests. Focused local tests help development but do
not replace required gates. Preserve exact failure evidence and report only
commands actually run; an unexplained failure is not a reason to retry.

## Documentation and fixtures

State what is implemented, what is proposed, and what was verified. Link to
the owning specification or decision instead of duplicating it. Update the
roadmap when milestone state changes and the development map when ownership
moves. Do not put session transcripts, transient branch inventories, or
verification claims without a named revision into agent entrypoints.

All fixtures, screenshots, examples, and seed data use invented agencies,
people, incidents, identifiers, narratives, and schedules. Real operational
material and credentials never enter the repository or public issue text.
Remove sensitive values from diagnostic output before sharing it.

## Licensing contributions

Consolebook is licensed under AGPL-3.0-only. Contributions use the same terms.
The project requires neither a separate contributor license agreement nor a
broad relicensing grant.
