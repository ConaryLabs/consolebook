-- Milestone 4 slice 2: acknowledgments and trainee capabilities
-- (docs/domain-model.md Acknowledgment; #32 decision 3).
--
-- An acknowledgment binds one person to one finalized
-- EvaluationVersion (invariant 2) and records receipt, not agreement,
-- with one kind from the closed domain set. It is a permanent record:
-- append-only, one per version per person, and shaped by the database
-- so raw writes cannot forge who spoke — trainee kinds are recorded by
-- the trainee themselves, attested kinds never are, and the bound
-- person is always the version's own trainee.
--
-- Instants are UTC unix seconds (INTEGER).

CREATE TABLE acknowledgment (
    id INTEGER PRIMARY KEY,
    evaluation_version_id INTEGER NOT NULL REFERENCES evaluation_version (id),
    -- The person bound to the version: always its trainee (trigger
    -- below). Kinds split by who speaks, not by who is bound.
    user_id INTEGER NOT NULL REFERENCES user (id),
    kind TEXT NOT NULL CHECK (
        kind IN ('acknowledged', 'acknowledged_with_response', 'refused',
                 'supervisor_attested_refusal', 'unavailable')
    ),
    -- The trainee's response, a refusal reason, or an attestation's
    -- explanation. A plain acknowledgment carries none; every other
    -- kind explains itself — blank-equivalent text is refused after
    -- trimming the full Unicode White_Space set (the 0009 precedent,
    -- the same characters the service's trim removes).
    response TEXT NOT NULL,
    recorded_by INTEGER NOT NULL REFERENCES user (id),
    recorded_at INTEGER NOT NULL,
    -- One acknowledgment per version per person; a successor version
    -- (slice 3) requires a new acknowledgment because it is a new row
    -- against a new version.
    UNIQUE (evaluation_version_id, user_id),
    CHECK (
        CASE WHEN kind = 'acknowledged'
             THEN response = ''
             ELSE length(trim(response, char(9, 10, 11, 12, 13, 32, 133, 160,
                                             5760, 8192, 8193, 8194, 8195,
                                             8196, 8197, 8198, 8199, 8200,
                                             8201, 8202, 8232, 8233, 8239,
                                             8287, 12288))) > 0
        END
    ),
    -- Who speaks: the trainee kinds are the trainee's own act; the
    -- attested kinds are someone else's statement about the trainee
    -- and are never self-recorded.
    CHECK (
        (kind IN ('acknowledged', 'acknowledged_with_response', 'refused'))
        = (recorded_by = user_id)
    )
) STRICT;

CREATE INDEX acknowledgment_user ON acknowledgment (user_id);

CREATE TRIGGER acknowledgment_no_update
BEFORE UPDATE ON acknowledgment
BEGIN
    SELECT RAISE(ABORT, 'acknowledgments are permanent records');
END;

CREATE TRIGGER acknowledgment_no_delete
BEFORE DELETE ON acknowledgment
BEGIN
    SELECT RAISE(ABORT, 'acknowledgments are permanent records');
END;

-- The bound person is the version's trainee — the enrollment's user on
-- the record the version seals. A missing version falls through to the
-- foreign key ('!=' against NULL never fires this trigger).
CREATE TRIGGER acknowledgment_binds_the_trainee
BEFORE INSERT ON acknowledgment
WHEN NEW.user_id != (SELECT e.user_id
                     FROM evaluation_version v
                     JOIN evaluation_record r ON r.id = v.evaluation_record_id
                     JOIN enrollment e ON e.id = r.enrollment_id
                     WHERE v.id = NEW.evaluation_version_id)
BEGIN
    SELECT RAISE(ABORT, 'an acknowledgment binds the version''s trainee');
END;

-- Trainee capabilities (#32 decision 3): trainees read their own
-- finalized records and acknowledge them. New trainees receive both
-- through the Trainee bundle; existing users reach them through this
-- explicit migration (the capabilities.rs contract), identified by
-- their enrollments — enrollment is what makes someone a trainee.
INSERT INTO capability_grant (user_id, capability, granted_at, granted_by)
SELECT DISTINCT e.user_id, 'view_own_records', unixepoch(), NULL
FROM enrollment e
WHERE NOT EXISTS (
    SELECT 1 FROM capability_grant g
    WHERE g.user_id = e.user_id AND g.capability = 'view_own_records'
);

INSERT INTO capability_grant (user_id, capability, granted_at, granted_by)
SELECT DISTINCT e.user_id, 'acknowledge_own_record', unixepoch(), NULL
FROM enrollment e
WHERE NOT EXISTS (
    SELECT 1 FROM capability_grant g
    WHERE g.user_id = e.user_id AND g.capability = 'acknowledge_own_record'
);
