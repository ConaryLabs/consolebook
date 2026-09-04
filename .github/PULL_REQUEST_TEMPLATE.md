## Primary Issue

<!-- Every non-trivial PR names one primary issue. Keep one linkage form. -->
Closes #
<!-- Use `Refs #` instead when this is one slice and the issue must remain open. -->

## Problem And Outcome

What problem does this solve, and what should be true after merge?

## Changes

-

## Scope

- In scope:
- Out of scope:

## Verification

- [ ] Listed the exact verification commands run below
- [ ] Added or updated tests when behavior changed
- [ ] Added or updated an ADR when a durable decision changed
- [ ] All fixtures and examples are invented; no real agency data

```text
- web/: npm ci, npm run check, npm run build
- cargo fmt --check
- cargo clippy --workspace --all-targets -- -D warnings
- cargo test --workspace
- cargo build -p consolebook-server
- web/: npm run e2e (state the browser used)
```

## Review And Merge Notes

- Review focus:
- User or operator impact:
