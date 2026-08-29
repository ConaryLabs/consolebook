-- Milestone 3 slice 4 (ADR 0008): the single-step review workflow.
--
-- A review decision (docs/domain-model.md ReviewDecision) is a permanent
-- record: reviewer, a decision from the closed set, a comment (required
-- when changes are requested), and the instant. The database holds the
-- workflow shape: decisions are append-only, decide only drafts whose
-- latest contributor event is submitted-for-review, and never come from
-- a contributor — the service refuses first with typed contracts, these
-- hold under races and raw writes.
--
-- The 0008 freeze triggers keyed on "latest event is submitted"; with
-- decisions in the stream the frozen state becomes derived from the
-- latest event plus the latest decision — submitted and approved drafts
-- are frozen, change-request and plain returns thaw the working copy.
-- The triggers are dropped and recreated onto that derivation (the 0006
-- drop-and-recreate precedent), with one owner: the view below.
--
-- Instants are UTC unix seconds (INTEGER).

CREATE TABLE review_decision (
    id INTEGER PRIMARY KEY,
    evaluation_record_id INTEGER NOT NULL REFERENCES evaluation_record (id),
    reviewer_user_id INTEGER NOT NULL REFERENCES user (id),
    decision TEXT NOT NULL CHECK (
        decision IN ('approved', 'changes_requested', 'returned')
    ),
    comment TEXT NOT NULL,
    decided_at INTEGER NOT NULL,
    -- A change request explains itself (ADR 0008): blank-equivalent
    -- comments are refused after trimming the full Unicode White_Space
    -- set, the same characters the service's trim removes.
    CHECK (
        decision != 'changes_requested'
        OR length(trim(comment, char(9, 10, 11, 12, 13, 32, 133, 160,
                                     5760, 8192, 8193, 8194, 8195, 8196,
                                     8197, 8198, 8199, 8200, 8201, 8202,
                                     8232, 8233, 8239, 8287, 12288))) > 0
    )
) STRICT;

CREATE INDEX review_decision_record
    ON review_decision (evaluation_record_id, id);

CREATE TRIGGER review_decision_no_update
BEFORE UPDATE ON review_decision
BEGIN
    SELECT RAISE(ABORT, 'review decisions are append-only');
END;

CREATE TRIGGER review_decision_no_delete
BEFORE DELETE ON review_decision
BEGIN
    SELECT RAISE(ABORT, 'review decisions are append-only');
END;

-- A decision lands while the draft is submitted — the service appends
-- the paired review_decided event in the same transaction, after this
-- row is accepted.
CREATE TRIGGER review_decision_decides_submitted
BEFORE INSERT ON review_decision
WHEN (SELECT kind FROM contributor_event
      WHERE evaluation_record_id = NEW.evaluation_record_id
      ORDER BY id DESC LIMIT 1) IS NOT 'submitted_for_review'
BEGIN
    SELECT RAISE(ABORT, 'reviews decide submitted drafts');
END;

-- ADR 0008's second snapshot is part of the transition: a change
-- request lands only when a change_request_return snapshot beyond
-- those earlier change requests consumed has been taken, so the
-- reviewed content is anchored before the copy thaws — under raw
-- writes exactly as through the service, which snapshots first in the
-- same transaction.
CREATE TRIGGER review_decision_change_request_snapshots
BEFORE INSERT ON review_decision
WHEN NEW.decision = 'changes_requested'
    AND (SELECT count(*) FROM draft_snapshot
         WHERE evaluation_record_id = NEW.evaluation_record_id
           AND reason = 'change_request_return')
        < (SELECT count(*) FROM review_decision
           WHERE evaluation_record_id = NEW.evaluation_record_id
             AND decision = 'changes_requested') + 1
BEGIN
    SELECT RAISE(ABORT, 'a change request snapshots what was reviewed');
END;

-- The decision and the workflow transition are one atomic write: the
-- paired review_decided event is generated here, never left to the
-- writer, so a decision can never land while the record still reads as
-- submitted — and a second decision on the same submission meets
-- 'reviews decide submitted drafts' above.
CREATE TRIGGER review_decision_advances_workflow
AFTER INSERT ON review_decision
BEGIN
    INSERT INTO contributor_event
        (evaluation_record_id, kind, actor_user_id, to_user_id, recorded_at)
    VALUES (NEW.evaluation_record_id, 'review_decided',
            NEW.reviewer_user_id, NULL, NEW.decided_at);
END;

-- Self-review is refused (ADR 0008): the current owner, an author or
-- submitter, and an ownership recipient are contributors. A coordinator
-- who only moved ownership between others stays eligible.
CREATE TRIGGER review_decision_refuses_contributors
BEFORE INSERT ON review_decision
WHEN NEW.reviewer_user_id = (SELECT owner_user_id FROM evaluation_record
                             WHERE id = NEW.evaluation_record_id)
    OR EXISTS (
        SELECT 1 FROM contributor_event
        WHERE evaluation_record_id = NEW.evaluation_record_id
          AND ((kind IN ('created', 'contributed', 'submitted_for_review')
                  AND actor_user_id = NEW.reviewer_user_id)
              OR (kind = 'ownership_transferred'
                  AND to_user_id = NEW.reviewer_user_id))
    )
BEGIN
    SELECT RAISE(ABORT, 'a contributor cannot review their own draft');
END;

-- The one owner of "is this working copy frozen": submitted drafts and
-- approved drafts are; a change-request or plain return thaws the copy
-- for revision.
CREATE VIEW evaluation_record_frozen AS
SELECT r.id AS evaluation_record_id,
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
       END AS frozen
FROM evaluation_record r;

-- The frozen state is derived from the event stream, so the stream
-- itself is guarded: while a draft is frozen, no event lands except
-- the review_decided a decision generates — a raw append can neither
-- thaw an approved copy nor slip past a submission.
CREATE TRIGGER contributor_event_frozen_insert
BEFORE INSERT ON contributor_event
WHEN NEW.kind IN ('created', 'contributed', 'ownership_transferred',
                  'submitted_for_review')
    AND (SELECT frozen FROM evaluation_record_frozen
         WHERE evaluation_record_id = NEW.evaluation_record_id) = 1
BEGIN
    SELECT RAISE(ABORT, 'a submitted or approved draft is frozen');
END;

-- And a review_decided event exists only as a decision's pair: the
-- generating trigger above runs after its decision row, so exactly one
-- unpaired decision precedes each legitimate event.
CREATE TRIGGER contributor_event_review_pairs_decision
BEFORE INSERT ON contributor_event
WHEN NEW.kind = 'review_decided'
    AND (SELECT count(*) FROM review_decision
         WHERE evaluation_record_id = NEW.evaluation_record_id)
        != (SELECT count(*) FROM contributor_event
            WHERE evaluation_record_id = NEW.evaluation_record_id
              AND kind = 'review_decided') + 1
BEGIN
    SELECT RAISE(ABORT, 'a review event pairs with its decision');
END;

DROP TRIGGER draft_rating_frozen_insert;
DROP TRIGGER draft_rating_frozen_update;
DROP TRIGGER draft_rating_frozen_delete;
DROP TRIGGER draft_rating_modifier_frozen_insert;
DROP TRIGGER draft_rating_modifier_frozen_delete;
DROP TRIGGER draft_narrative_frozen_insert;
DROP TRIGGER draft_narrative_frozen_update;
DROP TRIGGER draft_narrative_frozen_delete;

CREATE TRIGGER draft_rating_frozen_insert
BEFORE INSERT ON draft_rating
WHEN (SELECT frozen FROM evaluation_record_frozen
      WHERE evaluation_record_id = NEW.evaluation_record_id) = 1
BEGIN
    SELECT RAISE(ABORT, 'a submitted or approved draft is frozen');
END;

-- Updates guard both sides: content neither changes inside a frozen
-- copy nor gets re-pointed into one from an editable draft.
CREATE TRIGGER draft_rating_frozen_update
BEFORE UPDATE ON draft_rating
WHEN (SELECT frozen FROM evaluation_record_frozen
      WHERE evaluation_record_id = OLD.evaluation_record_id) = 1
    OR (SELECT frozen FROM evaluation_record_frozen
        WHERE evaluation_record_id = NEW.evaluation_record_id) = 1
BEGIN
    SELECT RAISE(ABORT, 'a submitted or approved draft is frozen');
END;

CREATE TRIGGER draft_rating_frozen_delete
BEFORE DELETE ON draft_rating
WHEN (SELECT frozen FROM evaluation_record_frozen
      WHERE evaluation_record_id = OLD.evaluation_record_id) = 1
BEGIN
    SELECT RAISE(ABORT, 'a submitted or approved draft is frozen');
END;

CREATE TRIGGER draft_rating_modifier_frozen_insert
BEFORE INSERT ON draft_rating_modifier
WHEN (SELECT frozen FROM evaluation_record_frozen
      WHERE evaluation_record_id = (SELECT evaluation_record_id
                                    FROM draft_rating
                                    WHERE id = NEW.draft_rating_id)) = 1
BEGIN
    SELECT RAISE(ABORT, 'a submitted or approved draft is frozen');
END;

CREATE TRIGGER draft_rating_modifier_frozen_delete
BEFORE DELETE ON draft_rating_modifier
WHEN (SELECT frozen FROM evaluation_record_frozen
      WHERE evaluation_record_id = (SELECT evaluation_record_id
                                    FROM draft_rating
                                    WHERE id = OLD.draft_rating_id)) = 1
BEGIN
    SELECT RAISE(ABORT, 'a submitted or approved draft is frozen');
END;

-- 0008 shipped no update guard for modifier rows; with both-sides
-- checks this closes re-pointing a modifier onto a frozen rating and
-- swapping which modifier a frozen rating carries.
CREATE TRIGGER draft_rating_modifier_frozen_update
BEFORE UPDATE ON draft_rating_modifier
WHEN (SELECT frozen FROM evaluation_record_frozen
      WHERE evaluation_record_id = (SELECT evaluation_record_id
                                    FROM draft_rating
                                    WHERE id = OLD.draft_rating_id)) = 1
    OR (SELECT frozen FROM evaluation_record_frozen
        WHERE evaluation_record_id = (SELECT evaluation_record_id
                                      FROM draft_rating
                                      WHERE id = NEW.draft_rating_id)) = 1
BEGIN
    SELECT RAISE(ABORT, 'a submitted or approved draft is frozen');
END;

CREATE TRIGGER draft_narrative_frozen_insert
BEFORE INSERT ON draft_narrative
WHEN (SELECT frozen FROM evaluation_record_frozen
      WHERE evaluation_record_id = NEW.evaluation_record_id) = 1
BEGIN
    SELECT RAISE(ABORT, 'a submitted or approved draft is frozen');
END;

CREATE TRIGGER draft_narrative_frozen_update
BEFORE UPDATE ON draft_narrative
WHEN (SELECT frozen FROM evaluation_record_frozen
      WHERE evaluation_record_id = OLD.evaluation_record_id) = 1
    OR (SELECT frozen FROM evaluation_record_frozen
        WHERE evaluation_record_id = NEW.evaluation_record_id) = 1
BEGIN
    SELECT RAISE(ABORT, 'a submitted or approved draft is frozen');
END;

CREATE TRIGGER draft_narrative_frozen_delete
BEFORE DELETE ON draft_narrative
WHEN (SELECT frozen FROM evaluation_record_frozen
      WHERE evaluation_record_id = OLD.evaluation_record_id) = 1
BEGIN
    SELECT RAISE(ABORT, 'a submitted or approved draft is frozen');
END;
