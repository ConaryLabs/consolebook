# Contributing

Consolebook is pre-alpha. Design changes are welcome, but the domain and record-integrity constraints in [PRINCIPLES.md](PRINCIPLES.md) come first.

## Before contributing

- Read the principles and relevant architecture decision records.
- Discuss broad domain or architecture changes before writing a large patch.
- Add or update an ADR when a decision changes durable system behavior.
- Keep changes narrow enough to review and verify.

## Issue, branch, and pull-request workflow

Non-trivial implementation, bug, refactor, documentation, operations, and
maintenance work uses one primary GitHub issue, an issue-linked branch, and a
pull request. Never push repository changes directly to `main`.

- Search open and closed issues before filing a new one.
- `Closes #...` in a pull request means it satisfies the issue's acceptance
  criteria; use `Refs #...` when the issue must remain open across slices.
- The `pr-gate` workflow enforces the Rust checks below on every pull
  request; run them locally first.
- Use short imperative Conventional Commit subjects such as
  `storage(backup): validate snapshot before fsync`.
- Security reports use private advisories per [SECURITY.md](SECURITY.md),
  never public issues.

## Engineering discipline

Engineer solutions, not band-aids. Heuristics, regexes, substring matching,
and silent defaults may aid diagnostics, discovery, or presentation; they may
not establish record integrity, authorization, retention, disposition, or
export behavior. Fix a defect found in scope, or file an exact-evidence issue
when it belongs elsewhere; do not silently route around it.

## Privacy and fixtures

Never submit real training records or operational material.

Examples, screenshots, tests, and seed data must use invented:

- agencies;
- people and identifiers;
- incidents and addresses;
- narratives;
- schedules; and
- program content.

Remove credentials and personal information from logs and bug reports.

## Rust checks

Once implementation begins, changes should pass:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Documentation

State what is implemented, what is proposed, and what has been verified. A design document is not runtime proof, and a passing unit test is not recovery proof.

## Licensing contributions

Consolebook is licensed under AGPL-3.0-only. By submitting a contribution, you agree to license it under the same terms. The project does not require a separate contributor license agreement or a broad relicensing grant.
