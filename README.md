# Consolebook

Consolebook is an early-stage training-record system for emergency communications centers. The initial focus is structured training programs, Daily Observation Reports, weekly summaries, task signoffs, acknowledgments, amendments, and defensible exports.

> **Status:** pre-alpha design and scaffolding. There is no usable application yet. Do not put production or personnel data into this repository or any current build.

## Product goals

- Programs and agency terminology are versioned configuration.
- Training sessions and evaluation records remain separate concepts.
- Daily reports, weekly summaries, and phase evaluations have explicit provenance.
- Finalized records are immutable while retained; corrections create successor versions.
- Retention policies, holds, and lawful disposition are explicit workflows.
- Acknowledgment records receipt rather than agreement.
- Assignment-scoped access is the default.
- Backups, recovery, and complete exports are product features.
- A small center can operate one installation without a pile of external services.

## Non-goals

- A multi-tenant schema in the core application.
- A generic drag-and-drop form builder.
- Agency-specific code paths.
- Cloud services, telemetry, or containers as runtime requirements.
- Editing finalized records in place.

## Architecture baseline

The current design target is a modular monolith:

- Rust and Axum for the application and HTTP API
- SQLx with SQLite in WAL mode for storage
- a statically built SvelteKit interface embedded in the executable
- Typst for deterministic PDF output
- opaque server-side sessions with Argon2id password hashes
- one executable and one data directory per installation

The server crate implements the beginnings of the Milestone 1 operable shell: a CLI, SQLite storage with verified connection invariants and embedded migrations, a health endpoint, diagnostics, and validated backups. There is no authentication, web interface, or domain model yet.

## Repository map

- [PRINCIPLES.md](PRINCIPLES.md) — non-negotiable product constraints
- [AGENTS.md](AGENTS.md) — repo-wide contract for contributors and agents
- [docs/architecture.md](docs/architecture.md) — proposed system boundaries
- [docs/domain-model.md](docs/domain-model.md) — domain vocabulary and invariants
- [docs/records-integrity.md](docs/records-integrity.md) — immutability, hashes, and provenance
- [docs/roadmap.md](docs/roadmap.md) — milestone sequence
- [docs/decisions/](docs/decisions/) — architecture decision records
- [crates/consolebook-server/](crates/consolebook-server/) — the server crate

## Build and run

```sh
cargo run -p consolebook-server -- serve          # initialize ./data and serve the API
cargo run -p consolebook-server -- doctor         # diagnose an installation, read-only
cargo run -p consolebook-server -- backup         # validated snapshot into ./data/backups
```

`serve` binds `127.0.0.1:7770` by default and answers `GET /api/health`. The
data directory defaults to `./data` and can be set with `--data-dir` or
`CONSOLEBOOK_DATA_DIR`. See [ADR 0003](docs/decisions/0003-sqlite-connection-invariants.md)
for the database durability decisions.

## Privacy

Examples and test fixtures must be invented. Do not commit real agency names, employee information, operational narratives, credentials, exports, screenshots, or training records.

## License

Consolebook is licensed under the [GNU Affero General Public License v3.0 only](LICENSE). If you modify Consolebook and let users interact with it over a network, AGPLv3 requires you to offer those users the Corresponding Source for the running modified version.

See [ADR 0002](docs/decisions/0002-license.md) for the decision and trade-offs.
