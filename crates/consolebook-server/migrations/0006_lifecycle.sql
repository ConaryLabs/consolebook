-- Milestone 3 slice 1 (ADR 0008): training assignments, enrollment
-- lifecycle events, and phase history.
--
-- Enrollment lifecycle and phase history are append-only event streams,
-- enforced by the database like audit_event. Changing an enrollment's
-- pinned version is now a modeled event: the repoint UPDATE is accepted
-- only when the latest enrollment_event records exactly that change, which
-- replaces migration 0005's blanket refusal. Phase events must reference
-- phases of the enrollment's currently pinned version (domain invariant 5).
--
-- Instants are UTC unix seconds (INTEGER). Phase events carry both the
-- effective instant (when the transition took effect) and the recorded
-- instant (when it was written): honest backfill, never falsified
-- contemporaneity, and effective_at can never postdate recorded_at.

-- Mutable profile data (docs/domain-model.md User). Finalized records
-- snapshot the presentation values they used; these columns are the live
-- values, empty when unknown.
ALTER TABLE user ADD COLUMN employee_id TEXT NOT NULL DEFAULT '';
ALTER TABLE user ADD COLUMN title TEXT NOT NULL DEFAULT '';

-- The durable grant behind assignment-scoped access (PRINCIPLES.md 10):
-- an assigned trainer holding view_assigned_records may read the
-- trainee's training history. Ending an assignment closes the interval in
-- place with attribution; assignments are access grants, not records.
CREATE TABLE training_assignment (
    id INTEGER PRIMARY KEY,
    enrollment_id INTEGER NOT NULL REFERENCES enrollment (id),
    trainer_user_id INTEGER NOT NULL REFERENCES user (id),
    assigned_at INTEGER NOT NULL,
    assigned_by INTEGER REFERENCES user (id),
    ended_at INTEGER,
    ended_by INTEGER REFERENCES user (id),
    CHECK (ended_at IS NULL OR ended_at >= assigned_at),
    CHECK (ended_by IS NULL OR ended_at IS NOT NULL)
) STRICT;

CREATE UNIQUE INDEX training_assignment_active_unique
    ON training_assignment (enrollment_id, trainer_user_id)
    WHERE ended_at IS NULL;

CREATE INDEX training_assignment_trainer ON training_assignment (trainer_user_id);

-- Enrollment lifecycle: version change with reason, withdraw, complete,
-- reinstate (docs/domain-model.md Enrollment). Status is derived from the
-- stream, never stored beside it.
CREATE TABLE enrollment_event (
    id INTEGER PRIMARY KEY,
    enrollment_id INTEGER NOT NULL REFERENCES enrollment (id),
    kind TEXT NOT NULL CHECK (kind IN ('version_change', 'withdraw', 'complete', 'reinstate')),
    occurred_at INTEGER NOT NULL,
    actor_user_id INTEGER REFERENCES user (id),
    reason TEXT NOT NULL,
    from_program_version_id INTEGER REFERENCES program_version (id),
    to_program_version_id INTEGER REFERENCES program_version (id),
    CHECK ((kind = 'version_change')
        = (from_program_version_id IS NOT NULL AND to_program_version_id IS NOT NULL)),
    CHECK (from_program_version_id IS NULL
        OR from_program_version_id != to_program_version_id),
    CHECK (kind != 'version_change' OR length(reason) > 0)
) STRICT;

CREATE INDEX enrollment_event_enrollment ON enrollment_event (enrollment_id);

CREATE TRIGGER enrollment_event_no_update
BEFORE UPDATE ON enrollment_event
BEGIN
    SELECT RAISE(ABORT, 'enrollment events are append-only');
END;

CREATE TRIGGER enrollment_event_no_delete
BEFORE DELETE ON enrollment_event
BEGIN
    SELECT RAISE(ABORT, 'enrollment events are append-only');
END;

-- A version-change event pins published configuration, like the
-- enrollment itself.
CREATE TRIGGER enrollment_event_requires_published_version
BEFORE INSERT ON enrollment_event
WHEN NEW.kind = 'version_change'
    AND (SELECT published_at FROM program_version
         WHERE id = NEW.to_program_version_id) IS NULL
BEGIN
    SELECT RAISE(ABORT, 'enrollments pin published program versions');
END;

-- Migration 0005 refused every repoint because the modeled event did not
-- exist yet. It does now: a repoint is accepted exactly when the latest
-- event for the enrollment records this change (the service inserts the
-- event and updates the pin in one transaction).
DROP TRIGGER enrollment_version_change_is_an_event;

CREATE TRIGGER enrollment_version_change_requires_event
BEFORE UPDATE OF program_version_id ON enrollment
WHEN OLD.program_version_id != NEW.program_version_id
    AND NOT EXISTS (
        SELECT 1 FROM enrollment_event
        WHERE id = (SELECT MAX(id) FROM enrollment_event
                    WHERE enrollment_id = OLD.id)
          AND kind = 'version_change'
          AND from_program_version_id = OLD.program_version_id
          AND to_program_version_id = NEW.program_version_id
    )
BEGIN
    SELECT RAISE(ABORT, 'enrollment version changes require a recorded event');
END;

CREATE TRIGGER enrollment_update_requires_published_version
BEFORE UPDATE OF program_version_id ON enrollment
WHEN OLD.program_version_id != NEW.program_version_id
    AND (SELECT published_at FROM program_version
         WHERE id = NEW.program_version_id) IS NULL
BEGIN
    SELECT RAISE(ABORT, 'enrollments pin published program versions');
END;

-- Phase history (docs/domain-model.md PhaseTransition): an event stream
-- that may advance, return for remediation, restart, pause, resume, or
-- complete. Phase numbers stay presentation; nothing here assumes
-- monotonic progress. Phase-changing kinds carry a target; pause, resume,
-- and complete record the phase they happened in.
CREATE TABLE phase_event (
    id INTEGER PRIMARY KEY,
    enrollment_id INTEGER NOT NULL REFERENCES enrollment (id),
    kind TEXT NOT NULL CHECK (kind IN ('advance', 'return', 'restart', 'pause', 'resume', 'complete')),
    from_phase_id INTEGER REFERENCES phase (id),
    to_phase_id INTEGER REFERENCES phase (id),
    effective_at INTEGER NOT NULL,
    recorded_at INTEGER NOT NULL,
    actor_user_id INTEGER REFERENCES user (id),
    reason TEXT NOT NULL,
    -- The version-change event that opened the pin epoch this event was
    -- recorded under; NULL for the enrollment's original pin. Derived
    -- state (current phase, pause) reads only the current epoch, so a
    -- version change always resets it — even back to a previously pinned
    -- version.
    version_change_event_id INTEGER REFERENCES enrollment_event (id),
    CHECK (effective_at <= recorded_at),
    CHECK (
        CASE kind
            WHEN 'advance' THEN to_phase_id IS NOT NULL
            WHEN 'return' THEN from_phase_id IS NOT NULL AND to_phase_id IS NOT NULL
            WHEN 'restart' THEN from_phase_id IS NOT NULL AND to_phase_id IS NOT NULL
            ELSE from_phase_id IS NOT NULL AND to_phase_id IS NULL
        END
    )
) STRICT;

CREATE INDEX phase_event_enrollment ON phase_event (enrollment_id, effective_at);

CREATE TRIGGER phase_event_no_update
BEFORE UPDATE ON phase_event
BEGIN
    SELECT RAISE(ABORT, 'phase events are append-only');
END;

CREATE TRIGGER phase_event_no_delete
BEFORE DELETE ON phase_event
BEGIN
    SELECT RAISE(ABORT, 'phase events are append-only');
END;

-- Every phase event stamps the pin epoch it was recorded under: the
-- enrollment's latest version-change event, or NULL before any.
CREATE TRIGGER phase_event_stamps_current_epoch
BEFORE INSERT ON phase_event
WHEN NEW.version_change_event_id IS NOT (
    SELECT MAX(id) FROM enrollment_event
    WHERE enrollment_id = NEW.enrollment_id AND kind = 'version_change')
BEGIN
    SELECT RAISE(ABORT, 'phase events record the enrollment''s current pin epoch');
END;

-- Domain invariant 5: referenced phases belong to the enrollment's
-- currently pinned version. History recorded under an earlier pin keeps
-- its old-version phases; new events re-enter the current version's graph.
CREATE TRIGGER phase_event_pins_enrollment_version
BEFORE INSERT ON phase_event
WHEN (NEW.from_phase_id IS NOT NULL
        AND (SELECT program_version_id FROM phase WHERE id = NEW.from_phase_id)
            != (SELECT program_version_id FROM enrollment WHERE id = NEW.enrollment_id))
    OR (NEW.to_phase_id IS NOT NULL
        AND (SELECT program_version_id FROM phase WHERE id = NEW.to_phase_id)
            != (SELECT program_version_id FROM enrollment WHERE id = NEW.enrollment_id))
BEGIN
    SELECT RAISE(ABORT, 'phase events reference the enrollment''s pinned version');
END;
