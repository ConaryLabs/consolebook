-- Milestone 1: agency identity, users, capability grants, sessions,
-- setup and reset codes, and append-only audit events.
--
-- Instants in these tables are UTC unix seconds (INTEGER). Human-facing
-- dates with agency-local meaning are a separate concept and arrive with
-- training sessions (PRINCIPLES.md section 6).
--
-- Secrets are never stored raw: password_hash is an Argon2id PHC string;
-- token_hash and code_hash columns hold SHA-256 hex of the opaque value.

CREATE TABLE agency (
    -- Single-row table: one installation serves one agency.
    id INTEGER PRIMARY KEY CHECK (id = 1),
    name TEXT NOT NULL CHECK (length(name) > 0),
    created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE user (
    id INTEGER PRIMARY KEY,
    username TEXT NOT NULL CHECK (length(username) > 0),
    display_name TEXT NOT NULL CHECK (length(display_name) > 0),
    password_hash TEXT NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;

CREATE UNIQUE INDEX user_username_unique ON user (username COLLATE NOCASE);

-- Authorization is expressed as capabilities (docs/domain-model.md).
-- Roles are convenient bundles applied at grant time, never a parallel
-- authority checked by name.
CREATE TABLE capability_grant (
    user_id INTEGER NOT NULL REFERENCES user (id),
    capability TEXT NOT NULL,
    granted_at INTEGER NOT NULL,
    granted_by INTEGER REFERENCES user (id),
    PRIMARY KEY (user_id, capability)
) STRICT;

CREATE TABLE session (
    token_hash TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES user (id),
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER
) STRICT;

CREATE INDEX session_user ON session (user_id);

CREATE TABLE setup_code (
    -- Single-row table: at most one live setup code, and none after
    -- initialization (the setup transaction deletes it).
    id INTEGER PRIMARY KEY CHECK (id = 1),
    code_hash TEXT NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT;

CREATE TABLE password_reset_code (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES user (id),
    code_hash TEXT NOT NULL UNIQUE,
    issued_via TEXT NOT NULL CHECK (issued_via IN ('administrator', 'recovery')),
    issued_by INTEGER REFERENCES user (id),
    issued_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    used_at INTEGER
) STRICT;

CREATE TABLE audit_event (
    id INTEGER PRIMARY KEY,
    occurred_at INTEGER NOT NULL,
    kind TEXT NOT NULL,
    actor_user_id INTEGER REFERENCES user (id),
    subject_user_id INTEGER REFERENCES user (id)
) STRICT;

-- Append-only, enforced by the database rather than application manners.
CREATE TRIGGER audit_event_no_update
BEFORE UPDATE ON audit_event
BEGIN
    SELECT RAISE(ABORT, 'audit events are append-only');
END;

CREATE TRIGGER audit_event_no_delete
BEFORE DELETE ON audit_event
BEGIN
    SELECT RAISE(ABORT, 'audit events are append-only');
END;
