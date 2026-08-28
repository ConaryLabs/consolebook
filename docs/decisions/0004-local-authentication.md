# ADR 0004: Local authentication, sessions, and recovery

- **Status:** Accepted
- **Date:** 2026-08-28

## Context

ADR 0001 deferred authentication to a follow-up decision. Milestone 1
requires local authentication with no email, cloud, or identity-provider
dependency: small centers must be able to initialize, sign in, and recover
a locked-out administrator with nothing but the executable and the data
directory.

## Decision

### Passwords

Argon2id (v19) with the RustCrypto defaults — 19 MiB memory, 2 iterations,
1 lane — stored as PHC strings that record their own parameters, so cost
changes apply to new hashes without migrations. Policy: 12–512 bytes, not
equal to the username. Login verifies unknown usernames against a fixed
dummy hash so response timing does not reveal whether an account exists,
and failure responses are identical for unknown user and wrong password.

### Sessions

256-bit random tokens from OS entropy, delivered in an `HttpOnly`,
`SameSite=Strict`, path-`/` cookie, stored server-side as SHA-256 digests
with a 12-hour absolute lifetime. Logout and password reset revoke
immediately; validation always checks expiry and revocation, and expired
rows are pruned opportunistically. The `Secure` cookie attribute is not
yet set: pre-alpha deployments are local or behind an operator's TLS
proxy, and the flag joins the deployment-hardening work before any pilot.

### Authorization

Capabilities, checked by domain services (`manage_users` gates user
administration). Roles are grant bundles applied at creation time — the
first administrator receives `manage_users`, `manage_programs`,
`assign_training`, and `export_records` — never a name compared at
decision time. Capabilities added to the product later reach existing
administrators by explicit migration.

### First-run setup

An uninitialized installation (no agency row) issues a 128-bit setup code,
stored as a digest with a 15-minute expiry, printed by `serve` at startup
and by `consolebook setup-code` on demand. `POST /api/setup` creates the
agency settings, the first administrator, and the administrator's grants,
and consumes the code — one transaction. After initialization the
operation returns 409 and no further codes are issued.

### Reset and recovery

A user holding `manage_users` can issue a 15-minute, single-use reset code
for any account. Using a code sets the new password, marks the code used,
revokes every session for the account, and records an audit event — one
transaction. `consolebook recover --username ...` issues the same kind of
code for an administrator account without any credentials; it requires
operating-system access to the data directory, refuses non-administrator
targets, and records a distinct recovery audit event.

### Audit

Authentication actions (setup, login success and failure, logout, reset
issued and used, recovery) insert append-only `audit_event` rows carrying
kind, instant, actor, and subject — no free text, no secrets. UPDATE and
DELETE on the table are rejected by database triggers.

## Consequences

### Positive

- an installation is operable with no external services;
- stolen database contents reveal no usable tokens or codes;
- lockout recovery has a tested, audited path that does not weaken the
  network surface (it requires filesystem access);
- authorization decisions are already capability-shaped before
  multi-role work starts in Milestone 3.

### Costs

- no rate limiting or lockout yet: failed logins are cheap for an
  attacker with network access, and failed-login audit rows are
  unbounded — both need a deliberate decision before any deployment
  outside a trusted network;
- absolute 12-hour sessions have no idle timeout or renewal; revisit with
  the web interface work;
- the administrator bundle must be extended by migration as new
  capabilities gain behavior, which is explicit but easy to forget — the
  capability's introducing slice owns that migration.

## Rejected alternatives

- **Email-based reset:** requires SMTP, which the architecture keeps
  optional; a small center may have no outbound mail at all.
- **Storing session tokens raw:** a database read (backup theft, SQL
  injection elsewhere) must not yield live credentials.
- **Role-name checks:** the domain model makes capabilities the
  authority; role names as code conditions rot into agency-specific
  branches.
- **JWT / stateless sessions:** immediate revocation is a requirement;
  server-side opaque sessions make it trivial and keep no signing keys.
