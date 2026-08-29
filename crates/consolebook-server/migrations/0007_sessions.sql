-- Milestone 3 slice 2 (ADR 0008, ADR 0009): training sessions with
-- explicit time semantics, and per-session trainer membership.
--
-- Operational dates and shifts carry agency-local meaning; duration and
-- ordering use UTC instants (PRINCIPLES.md 6). The business date,
-- timezone name, and local start/end are stored verbatim as entered —
-- never derived — so a later timezone-database change cannot rewrite what
-- a historical session said. The UTC instants are computed once at entry
-- (ADR 0009) and are the only columns ordering and overlap reason about.
--
-- Domain invariants enforced here: UTC end cannot precede start (6);
-- active training intervals for one trainee cannot overlap (7), where an
-- open session is unbounded, contiguous sessions are legal, and a
-- cancelled session does not occupy its interval; and no uniqueness
-- assumes one session per trainee and calendar date (8) — deliberately no
-- such constraint exists. Session phases belong to the enrollment's
-- currently pinned version (invariant 5).
--
-- Instants are UTC unix seconds (INTEGER).

CREATE TABLE training_session (
    id INTEGER PRIMARY KEY,
    enrollment_id INTEGER NOT NULL REFERENCES enrollment (id),
    business_date TEXT NOT NULL CHECK (length(business_date) > 0),
    timezone TEXT NOT NULL CHECK (length(timezone) > 0),
    local_start TEXT NOT NULL CHECK (length(local_start) > 0),
    local_end TEXT,
    utc_start INTEGER NOT NULL,
    utc_end INTEGER,
    phase_id INTEGER REFERENCES phase (id),
    -- Open until closed with a disposition: completed and interrupted
    -- carry an end instant; a cancelled session never happened and
    -- carries none.
    disposition TEXT CHECK (disposition IN ('completed', 'interrupted', 'cancelled')),
    created_at INTEGER NOT NULL,
    created_by INTEGER REFERENCES user (id),
    closed_at INTEGER,
    closed_by INTEGER REFERENCES user (id),
    CHECK (utc_end IS NULL OR utc_end >= utc_start),
    CHECK ((local_end IS NULL) = (utc_end IS NULL)),
    CHECK ((closed_at IS NULL) = (disposition IS NULL)),
    CHECK (closed_by IS NULL OR closed_at IS NOT NULL),
    CHECK (
        CASE
            WHEN disposition IN ('completed', 'interrupted') THEN utc_end IS NOT NULL
            ELSE utc_end IS NULL
        END
    )
) STRICT;

CREATE INDEX training_session_enrollment
    ON training_session (enrollment_id, utc_start);

-- A session belongs to its enrollment permanently.
CREATE TRIGGER training_session_keeps_enrollment
BEFORE UPDATE OF enrollment_id ON training_session
WHEN OLD.enrollment_id != NEW.enrollment_id
BEGIN
    SELECT RAISE(ABORT, 'a session belongs to its enrollment');
END;

-- Invariant 7: active intervals for one trainee cannot overlap, across
-- every enrollment of that trainee. An open session is unbounded on the
-- right; interval ends are exclusive, so contiguous sessions (handoffs,
-- holdovers, callbacks) are legal; cancelled sessions release their
-- interval.
CREATE TRIGGER training_session_no_overlap_on_insert
BEFORE INSERT ON training_session
WHEN (NEW.disposition IS NULL OR NEW.disposition != 'cancelled')
    AND EXISTS (
        SELECT 1 FROM training_session ts
        JOIN enrollment other ON other.id = ts.enrollment_id
        WHERE other.user_id
                = (SELECT user_id FROM enrollment WHERE id = NEW.enrollment_id)
          AND (ts.disposition IS NULL OR ts.disposition != 'cancelled')
          AND ts.utc_start < COALESCE(NEW.utc_end, 9223372036854775807)
          AND NEW.utc_start < COALESCE(ts.utc_end, 9223372036854775807)
    )
BEGIN
    SELECT RAISE(ABORT, 'active training intervals for one trainee cannot overlap');
END;

CREATE TRIGGER training_session_no_overlap_on_update
BEFORE UPDATE OF utc_start, utc_end, disposition ON training_session
WHEN (NEW.disposition IS NULL OR NEW.disposition != 'cancelled')
    AND EXISTS (
        SELECT 1 FROM training_session ts
        JOIN enrollment other ON other.id = ts.enrollment_id
        WHERE ts.id != OLD.id
          AND other.user_id
                = (SELECT user_id FROM enrollment WHERE id = OLD.enrollment_id)
          AND (ts.disposition IS NULL OR ts.disposition != 'cancelled')
          AND ts.utc_start < COALESCE(NEW.utc_end, 9223372036854775807)
          AND NEW.utc_start < COALESCE(ts.utc_end, 9223372036854775807)
    )
BEGIN
    SELECT RAISE(ABORT, 'active training intervals for one trainee cannot overlap');
END;

-- Invariant 5 for sessions: a phase reference belongs to the enrollment's
-- currently pinned version. Sessions recorded under an earlier pin keep
-- their phases; only setting a phase is checked.
CREATE TRIGGER training_session_phase_pins_version_on_insert
BEFORE INSERT ON training_session
WHEN NEW.phase_id IS NOT NULL
    AND (SELECT program_version_id FROM phase WHERE id = NEW.phase_id)
        != (SELECT program_version_id FROM enrollment WHERE id = NEW.enrollment_id)
BEGIN
    SELECT RAISE(ABORT, 'session phases reference the enrollment''s pinned version');
END;

CREATE TRIGGER training_session_phase_pins_version_on_update
BEFORE UPDATE OF phase_id ON training_session
WHEN NEW.phase_id IS NOT NULL
    AND NEW.phase_id IS NOT OLD.phase_id
    AND (SELECT program_version_id FROM phase WHERE id = NEW.phase_id)
        != (SELECT program_version_id FROM enrollment WHERE id = NEW.enrollment_id)
BEGIN
    SELECT RAISE(ABORT, 'session phases reference the enrollment''s pinned version');
END;

-- One or more trainers per session (docs/domain-model.md TrainingSession).
-- Members hold author_evaluation, enforced by the service and audited;
-- the database keeps the floor.
CREATE TABLE session_trainer (
    id INTEGER PRIMARY KEY,
    session_id INTEGER NOT NULL REFERENCES training_session (id),
    trainer_user_id INTEGER NOT NULL REFERENCES user (id),
    added_at INTEGER NOT NULL,
    added_by INTEGER REFERENCES user (id),
    UNIQUE (session_id, trainer_user_id)
) STRICT;

CREATE INDEX session_trainer_trainer ON session_trainer (trainer_user_id);

CREATE TRIGGER session_trainer_keeps_one
BEFORE DELETE ON session_trainer
WHEN (SELECT COUNT(*) FROM session_trainer WHERE session_id = OLD.session_id) = 1
BEGIN
    SELECT RAISE(ABORT, 'a training session keeps at least one trainer');
END;
