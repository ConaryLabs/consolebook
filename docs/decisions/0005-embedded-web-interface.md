# ADR 0005: Embedded web interface toolchain

- **Status:** Accepted
- **Date:** 2026-08-28

## Context

The architecture requires a statically built SvelteKit interface embedded
in the Rust executable, with no Node.js runtime and no server-side
rendering in production. Milestone 1 needs its first concrete slice —
setup, sign-in, recovery, and status — and with it the durable decisions
about toolchain, embedding, and build order.

## Decision

### Toolchain

SvelteKit 2 with Svelte 5 and TypeScript in `web/`, built by Vite through
`@sveltejs/adapter-static` with an `index.html` fallback and SSR disabled:
a single-page application. Node.js is a build-time tool only, pinned by
`web/package-lock.json`; nothing from `web/` ships except the static
files.

### API boundary

The interface consumes the same public HTTP API as any other client —
sessions ride the HttpOnly cookie, and the shell holds no secrets and no
privileged routes. Routing state comes from unauthenticated
`GET /api/instance` ({initialized, version, agency}); the client-side
guard lands visitors on setup, sign-in, or status accordingly. The guard
is convenience, not security: authorization stays server-side.

### Embedding and serving

`rust-embed` embeds `web/build` into release executables and reads it from
disk in debug builds (fast edit loop). The Axum router serves `/api/*`
first; the fallback handler serves static assets, hands unknown non-API
paths the SPA entry point, and keeps unknown `/api/*` paths as JSON 404s.
Content-hashed files under `_app/immutable/` get immutable cache headers;
everything else revalidates.

### Build order and honesty

`web/` builds before the Rust gates in CI so the embedded bytes are real
and tested. A bare `cargo build` without a web build still compiles (a
build script creates the empty asset folder) but the resulting executable
says so: it logs a startup warning and serves a plain-text 503 notice
instead of an interface. It never pretends.

### Verification

`svelte-check` and the web build run in the PR gate. Rust integration
tests cover asset serving, SPA fallback, and API 404 semantics. A
Playwright end-to-end spec (`web/e2e/`) drives the compiled binary through
setup → sign-in → status → reset-code issuance → password reset → sign-out
in a real browser; it runs on demand (`npm run e2e`) rather than in the PR
gate, keeping the gate fast until the UI surface grows enough to justify
browser time on every PR.

## Consequences

### Positive

- one executable still ships everything; deployment stays boring;
- the UI cannot drift from the API's authorization model because it has
  no other channel;
- debug builds pick up web changes without recompiling Rust; and
- CI proves the embedded interface, not a placeholder.

### Costs

- contributors touching the interface need Node.js locally;
- the SPA guard duplicates a small amount of routing logic that must
  track API semantics (kept honest by the E2E spec); and
- browser E2E runs on demand rather than on every PR — revisit when
  interface changes become frequent.

## Rejected alternatives

- **Server-side rendering / Node in production:** outside the design;
  doubles the runtime surface a small center must operate.
- **Rust-native templating:** the roadmap's interactive milestones
  (collaborative drafts, timelines) exceed what server templates handle
  well, and the architecture already commits to SvelteKit.
- **Committing built assets to the repository:** generated churn in
  review; CI builds them from source instead.
- **Failing the Rust build when web assets are missing:** would couple
  `cargo test` to a Node toolchain for every backend-only change; an
  honestly degraded binary plus a CI-enforced build order keeps both
  loops fast.
