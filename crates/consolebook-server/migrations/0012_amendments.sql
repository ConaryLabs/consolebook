-- Milestone 4 slice 3: amendments and successor versions
-- (docs/domain-model.md Amendment; docs/records-integrity.md; #32
-- decision 4; ADR 0011's chain rule).
--
-- A correction never edits a finalized version in place: an amendment
-- reopens the record's one working copy, the correction travels the
-- ordinary workflow under the pinned version's policy, and sealing
-- produces the successor EvaluationVersion linked to its predecessor.
-- The amendment row is the permanent record of reason and authority —
-- and it is the open-state marker: an amendment whose target is still
-- the record's latest version is in progress; once the successor
-- lands, that target is no longer latest and the record refreezes, by
-- derivation rather than by a mutable status column.
--
-- Because the contributor-event stream is append-only and its kind set
-- closed (0008), the reopened cycle is scoped by high-water marks: the
-- amendment records the last event and decision ids that existed when
-- it opened, and every workflow derivation for an amended record reads
-- only what came after. The marks are facts about the reopening
-- instant, held truthful by triggers below.
--
-- Instants are UTC unix seconds (INTEGER).

CREATE TABLE amendment (
    id INTEGER PRIMARY KEY,
    evaluation_record_id INTEGER NOT NULL REFERENCES evaluation_record (id),
    -- The version being corrected. UNIQUE: a version takes one
    -- successor, so it is amended at most once.
    predecessor_version_id INTEGER NOT NULL UNIQUE
        REFERENCES evaluation_version (id),
    -- An amendment explains itself (#32 decision 4): blank-equivalent
    -- reasons are refused after trimming the full Unicode White_Space
    -- set (the 0009 precedent).
    reason TEXT NOT NULL CHECK (
        length(trim(reason, char(9, 10, 11, 12, 13, 32, 133, 160,
                                 5760, 8192, 8193, 8194, 8195, 8196,
                                 8197, 8198, 8199, 8200, 8201, 8202,
                                 8232, 8233, 8239, 8287, 12288))) > 0
    ),
    opened_by INTEGER NOT NULL REFERENCES user (id),
    -- Presentation-name snapshot (the 0011 acknowledgment precedent):
    -- the permanent amendment displays its authority as named at the
    -- act; a later profile rename never rewrites it.
    opened_by_display_name TEXT NOT NULL
        CHECK (length(opened_by_display_name) > 0),
    opened_at INTEGER NOT NULL,
    -- The workflow position reopened from: the reopened cycle derives
    -- its state from events and decisions strictly after these marks.
    opened_after_event_id INTEGER NOT NULL,
    opened_after_decision_id INTEGER NOT NULL
) STRICT;

CREATE INDEX amendment_record ON amendment (evaluation_record_id, id);

CREATE TRIGGER amendment_no_update
BEFORE UPDATE ON amendment
BEGIN
    SELECT RAISE(ABORT, 'amendments are permanent records');
END;

CREATE TRIGGER amendment_no_delete
BEFORE DELETE ON amendment
BEGIN
    SELECT RAISE(ABORT, 'amendments are permanent records');
END;

-- An amendment reopens the record's latest finalized version — its own
-- record's, never another's, never a superseded one, and never an
-- unfinalized record (no version, no target).
CREATE TRIGGER amendment_amends_the_latest
BEFORE INSERT ON amendment
WHEN NEW.predecessor_version_id IS NOT (
    SELECT v.id FROM evaluation_version v
    WHERE v.evaluation_record_id = NEW.evaluation_record_id
    ORDER BY v.version_number DESC LIMIT 1
)
BEGIN
    SELECT RAISE(ABORT,
        'an amendment reopens the record''s latest finalized version');
END;

-- The high-water marks are the stream positions at opening, exactly:
-- a forged low mark would resurrect the superseded cycle's approval as
-- if it belonged to the reopened one.
CREATE TRIGGER amendment_marks_are_truthful
BEFORE INSERT ON amendment
WHEN NEW.opened_after_event_id IS NOT COALESCE(
        (SELECT MAX(ce.id) FROM contributor_event ce
         WHERE ce.evaluation_record_id = NEW.evaluation_record_id), 0)
    OR NEW.opened_after_decision_id IS NOT COALESCE(
        (SELECT MAX(rd.id) FROM review_decision rd
         WHERE rd.evaluation_record_id = NEW.evaluation_record_id), 0)
BEGIN
    SELECT RAISE(ABORT,
        'an amendment records the workflow position it reopened from');
END;

-- Succession replaces 0010's first-versions-only placeholder: a
-- version carrying a predecessor extends its own record's latest
-- version, next in order, and arrives only under that version's
-- recorded amendment. (A first version still has no predecessor by the
-- 0010 CHECK, and a duplicate number meets the UNIQUE constraint.)
DROP TRIGGER evaluation_version_first_versions_only;

CREATE TRIGGER evaluation_version_successors_in_order
BEFORE INSERT ON evaluation_version
WHEN NEW.predecessor_id IS NOT NULL
    AND (NEW.predecessor_id IS NOT (
             SELECT v.id FROM evaluation_version v
             WHERE v.evaluation_record_id = NEW.evaluation_record_id
             ORDER BY v.version_number DESC LIMIT 1)
         OR NEW.version_number IS NOT (
             SELECT MAX(v.version_number) + 1 FROM evaluation_version v
             WHERE v.evaluation_record_id = NEW.evaluation_record_id))
BEGIN
    SELECT RAISE(ABORT, 'a successor version extends the latest version');
END;

CREATE TRIGGER evaluation_version_successors_take_amendments
BEFORE INSERT ON evaluation_version
WHEN NEW.predecessor_id IS NOT NULL
    AND NOT EXISTS (SELECT 1 FROM amendment a
                    WHERE a.predecessor_version_id = NEW.predecessor_id)
BEGIN
    SELECT RAISE(ABORT, 'a successor version arrives with its amendment');
END;

-- The frozen derivation gains the reopened cycle: an unfinalized
-- record derives from its whole stream (0009); a finalized record with
-- no open amendment stays frozen (0010's terminal case); a finalized
-- record whose latest version carries an amendment derives from the
-- events and decisions after the reopening marks — draft until
-- submitted, frozen through approval, sealed by the successor (which
-- makes a newer version latest and ends the open amendment by
-- derivation).
DROP VIEW evaluation_record_frozen;

CREATE VIEW evaluation_record_frozen AS
SELECT r.id AS evaluation_record_id,
       CASE
           WHEN NOT EXISTS (SELECT 1 FROM evaluation_version v
                            WHERE v.evaluation_record_id = r.id) THEN
               CASE (SELECT ce.kind FROM contributor_event ce
                     WHERE ce.evaluation_record_id = r.id
                     ORDER BY ce.id DESC LIMIT 1)
                   WHEN 'submitted_for_review' THEN 1
                   WHEN 'review_decided' THEN
                       CASE (SELECT rd.decision FROM review_decision rd
                             WHERE rd.evaluation_record_id = r.id
                             ORDER BY rd.id DESC LIMIT 1)
                           WHEN 'approved' THEN 1
                           ELSE 0
                       END
                   ELSE 0
               END
           WHEN (SELECT a.id FROM amendment a
                 WHERE a.predecessor_version_id = (
                     SELECT v.id FROM evaluation_version v
                     WHERE v.evaluation_record_id = r.id
                     ORDER BY v.version_number DESC LIMIT 1)) IS NULL THEN 1
           ELSE
               CASE (SELECT ce.kind FROM contributor_event ce
                     WHERE ce.evaluation_record_id = r.id
                       AND ce.id > (SELECT a.opened_after_event_id
                                    FROM amendment a
                                    WHERE a.predecessor_version_id = (
                                        SELECT v.id FROM evaluation_version v
                                        WHERE v.evaluation_record_id = r.id
                                        ORDER BY v.version_number DESC LIMIT 1))
                     ORDER BY ce.id DESC LIMIT 1)
                   WHEN 'submitted_for_review' THEN 1
                   WHEN 'review_decided' THEN
                       CASE (SELECT rd.decision FROM review_decision rd
                             WHERE rd.evaluation_record_id = r.id
                               AND rd.id > (SELECT a.opened_after_decision_id
                                            FROM amendment a
                                            WHERE a.predecessor_version_id = (
                                                SELECT v.id
                                                FROM evaluation_version v
                                                WHERE v.evaluation_record_id = r.id
                                                ORDER BY v.version_number DESC
                                                LIMIT 1))
                             ORDER BY rd.id DESC LIMIT 1)
                           WHEN 'approved' THEN 1
                           ELSE 0
                       END
                   ELSE 0
               END
       END AS frozen
FROM evaluation_record r;

-- The workflow gate holds raw for successors too: when the pinned
-- version's policy requires review approval (missing policy fails
-- closed), a version lands only on a draft whose latest event within
-- its own cycle is an approving decision. For a first version the
-- cycle is the whole stream (marks coalesce to zero); for a successor
-- it is everything after its amendment's reopening marks.
DROP TRIGGER evaluation_version_requires_approval;

CREATE TRIGGER evaluation_version_requires_approval
BEFORE INSERT ON evaluation_version
WHEN COALESCE((SELECT fp.review_approved FROM finalization_policy fp
               WHERE fp.program_version_id =
                     (SELECT r.program_version_id FROM evaluation_record r
                      WHERE r.id = NEW.evaluation_record_id)), 1) = 1
    -- An empty reopened cycle has no latest event; COALESCE keeps the
    -- gate closed rather than letting NULL slip past the comparison.
    AND NOT (COALESCE((SELECT ce.kind FROM contributor_event ce
                       WHERE ce.evaluation_record_id = NEW.evaluation_record_id
                         AND ce.id > COALESCE(
                             (SELECT a.opened_after_event_id FROM amendment a
                              WHERE a.predecessor_version_id = NEW.predecessor_id),
                             0)
                       ORDER BY ce.id DESC LIMIT 1), '') = 'review_decided'
             AND COALESCE((SELECT rd.decision FROM review_decision rd
                           WHERE rd.evaluation_record_id = NEW.evaluation_record_id
                             AND rd.id > COALESCE(
                                 (SELECT a.opened_after_decision_id FROM amendment a
                                  WHERE a.predecessor_version_id = NEW.predecessor_id),
                                 0)
                           ORDER BY rd.id DESC LIMIT 1), '') = 'approved')
BEGIN
    SELECT RAISE(ABORT, 'finalization takes an approved draft');
END;
