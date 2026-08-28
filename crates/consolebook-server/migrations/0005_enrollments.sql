-- Milestone 2: enrollment pinning (ADR 0007; docs/domain-model.md
-- Enrollment).
--
-- An enrollment connects one trainee to one published program_version —
-- never a program, never a draft. Lifecycle history (phase transitions,
-- version changes with actor and reason, withdrawal, concurrent-enrollment
-- policy) arrives with Milestone 3; until then one row per (user, version)
-- is the honest minimal contract, and the Milestone 3 lifecycle work may
-- relax the uniqueness when re-enrollment becomes a modeled event.
--
-- Instants are UTC unix seconds (INTEGER).

CREATE TABLE enrollment (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES user (id),
    program_version_id INTEGER NOT NULL REFERENCES program_version (id),
    enrolled_at INTEGER NOT NULL,
    enrolled_by INTEGER REFERENCES user (id),
    UNIQUE (user_id, program_version_id)
) STRICT;

CREATE INDEX enrollment_version ON enrollment (program_version_id);

-- Enrollments pin published configuration, enforced by the database
-- rather than application manners.
CREATE TRIGGER enrollment_requires_published_version
BEFORE INSERT ON enrollment
WHEN (SELECT published_at FROM program_version WHERE id = NEW.program_version_id) IS NULL
BEGIN
    SELECT RAISE(ABORT, 'enrollments pin published program versions');
END;

-- Changing an enrollment's version is an explicit modeled event
-- (docs/domain-model.md); until Milestone 3 models it, the database
-- refuses silent repointing.
CREATE TRIGGER enrollment_version_change_is_an_event
BEFORE UPDATE OF program_version_id ON enrollment
WHEN OLD.program_version_id != NEW.program_version_id
BEGIN
    SELECT RAISE(ABORT, 'enrollment version changes are explicit events and are not yet modeled');
END;
