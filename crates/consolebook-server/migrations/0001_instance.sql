-- Milestone 1: instance identity and layout metadata.
--
-- Domain tables (programs, sessions, records) arrive with their own
-- milestones. This migration only gives an installation a stable identity
-- so backups, exports, and diagnostics can name the instance they belong to.

CREATE TABLE instance (
    -- Single-row table: one installation is one agency instance.
    id INTEGER PRIMARY KEY CHECK (id = 1),
    installation_id TEXT NOT NULL,
    created_at_utc TEXT NOT NULL
) STRICT;
