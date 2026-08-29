-- Milestone 3 slice 3 (ADR 0008): daily evaluation drafts with
-- metadata-only contributor attribution.
--
-- An evaluation record is the continuing identity of an evaluation
-- (docs/domain-model.md EvaluationRecord): it stamps the version its
-- covered session was recorded under and is typed by a pinned form —
-- invariant 5 as composite foreign keys against that stamp. Sessions
-- attach through a join table; one daily draft per training session is
-- v1 policy in the service, deliberately not schema, so multi-session
-- and per-business-date coverage stay open (ADR 0008).
--
-- Attribution is an append-only stream of metadata-only contributor
-- events — the same enforcement class as audit_event — and the current
-- owner moves only when the latest event records exactly that transfer.
-- The draft content is one mutable working copy; once the latest event
-- says the draft is submitted for review, the database freezes it, and
-- full content snapshots (taken at submission and change-request return)
-- anchor a review to what was reviewed.
--
-- Instants are UTC unix seconds (INTEGER).

-- Composite identity indexes the 0004 vocabulary tables did not need
-- until now: draft content pins form rows by (id, version).
CREATE UNIQUE INDEX form_competency_version_identity
    ON form_competency (id, program_version_id);
CREATE UNIQUE INDEX form_narrative_version_identity
    ON form_narrative (id, program_version_id);

CREATE TABLE evaluation_record (
    id INTEGER PRIMARY KEY,
    enrollment_id INTEGER NOT NULL REFERENCES enrollment (id),
    -- The stamp: the covered session's version, so the draft's
    -- vocabulary stays what training was recorded under.
    program_version_id INTEGER NOT NULL REFERENCES program_version (id),
    evaluation_form_id INTEGER NOT NULL,
    owner_user_id INTEGER NOT NULL REFERENCES user (id),
    -- Optimistic concurrency for the working copy: every content save
    -- carries the revision it read and bumps it, so a stale full
    -- replacement is a typed refusal, never a silent overwrite of
    -- another contributor's work.
    revision INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    created_by INTEGER REFERENCES user (id),
    -- Invariant 5: the form belongs to the stamped version.
    FOREIGN KEY (evaluation_form_id, program_version_id)
        REFERENCES evaluation_form (id, program_version_id),
    UNIQUE (id, program_version_id)
) STRICT;

CREATE INDEX evaluation_record_enrollment
    ON evaluation_record (enrollment_id);

-- A record keeps what it is; only ownership moves, and only through its
-- recorded event (below).
CREATE TRIGGER evaluation_record_keeps_identity
BEFORE UPDATE OF enrollment_id, program_version_id, evaluation_form_id
    ON evaluation_record
WHEN OLD.enrollment_id != NEW.enrollment_id
    OR OLD.program_version_id != NEW.program_version_id
    OR OLD.evaluation_form_id != NEW.evaluation_form_id
BEGIN
    SELECT RAISE(ABORT, 'a record keeps its enrollment, version, and form');
END;

-- Sessions covered by a record. The schema is many-to-many on purpose;
-- v1's one-daily-draft-per-session rule lives in the service.
CREATE TABLE evaluation_session (
    id INTEGER PRIMARY KEY,
    evaluation_record_id INTEGER NOT NULL REFERENCES evaluation_record (id),
    training_session_id INTEGER NOT NULL REFERENCES training_session (id),
    UNIQUE (evaluation_record_id, training_session_id)
) STRICT;

CREATE INDEX evaluation_session_session
    ON evaluation_session (training_session_id);

-- A record documents its own enrollment's training, in the vocabulary
-- that training was recorded under.
CREATE TRIGGER evaluation_session_agrees
BEFORE INSERT ON evaluation_session
WHEN (SELECT enrollment_id FROM evaluation_record
      WHERE id = NEW.evaluation_record_id)
        != (SELECT enrollment_id FROM training_session
            WHERE id = NEW.training_session_id)
    OR (SELECT program_version_id FROM evaluation_record
        WHERE id = NEW.evaluation_record_id)
        != (SELECT program_version_id FROM training_session
            WHERE id = NEW.training_session_id)
BEGIN
    SELECT RAISE(ABORT, 'a record covers sessions of its own enrollment and version');
END;

CREATE TRIGGER evaluation_session_keeps_identity
BEFORE UPDATE ON evaluation_session
BEGIN
    SELECT RAISE(ABORT, 'coverage changes are inserts and deletes, never edits');
END;

-- A cancelled session never happened, so it takes no coverage. The
-- service refuses first with its typed contract; this holds under races
-- and raw writes.
CREATE TRIGGER evaluation_session_covers_live_sessions
BEFORE INSERT ON evaluation_session
WHEN (SELECT disposition FROM training_session
      WHERE id = NEW.training_session_id) = 'cancelled'
BEGIN
    SELECT RAISE(ABORT, 'a cancelled session takes no evaluation');
END;

-- Metadata-only attribution (docs/domain-model.md ContributorEvent): who
-- touched the draft and how, never the content itself. The closed kind
-- set covers the whole workflow; review_decided is written by the review
-- slice. Append-only, like audit_event.
CREATE TABLE contributor_event (
    id INTEGER PRIMARY KEY,
    evaluation_record_id INTEGER NOT NULL REFERENCES evaluation_record (id),
    kind TEXT NOT NULL CHECK (
        kind IN ('created', 'contributed', 'ownership_transferred',
                 'submitted_for_review', 'review_decided')
    ),
    actor_user_id INTEGER NOT NULL REFERENCES user (id),
    -- The recipient, exactly for ownership transfers.
    to_user_id INTEGER REFERENCES user (id),
    recorded_at INTEGER NOT NULL,
    CHECK ((kind = 'ownership_transferred') = (to_user_id IS NOT NULL))
) STRICT;

CREATE INDEX contributor_event_record
    ON contributor_event (evaluation_record_id, id);

CREATE TRIGGER contributor_event_no_update
BEFORE UPDATE ON contributor_event
BEGIN
    SELECT RAISE(ABORT, 'contributor events are append-only');
END;

CREATE TRIGGER contributor_event_no_delete
BEFORE DELETE ON contributor_event
BEGIN
    SELECT RAISE(ABORT, 'contributor events are append-only');
END;

-- Ownership moves only with its recorded event: the owner update is
-- accepted exactly when the latest contributor event records this
-- transfer (the 0006 version-change pattern).
CREATE TRIGGER evaluation_record_owner_change_requires_event
BEFORE UPDATE OF owner_user_id ON evaluation_record
WHEN OLD.owner_user_id != NEW.owner_user_id
    AND NOT EXISTS (
        SELECT 1 FROM contributor_event ce
        WHERE ce.evaluation_record_id = OLD.id
          AND ce.id = (SELECT MAX(id) FROM contributor_event
                       WHERE evaluation_record_id = OLD.id)
          AND ce.kind = 'ownership_transferred'
          AND ce.to_user_id = NEW.owner_user_id
    )
BEGIN
    SELECT RAISE(ABORT, 'ownership moves only with its recorded event');
END;

-- The mutable working copy: one rating per pinned form competency, its
-- modifiers, and one text per pinned narrative prompt. Scale-kind
-- validation (anchored bounds, pass/fail, narrative-only takes no value)
-- is the service's typed contract; the composite foreign keys hold the
-- vocabulary to the record's stamped version.
CREATE TABLE draft_rating (
    id INTEGER PRIMARY KEY,
    evaluation_record_id INTEGER NOT NULL,
    program_version_id INTEGER NOT NULL,
    form_competency_id INTEGER NOT NULL,
    value INTEGER,
    FOREIGN KEY (evaluation_record_id, program_version_id)
        REFERENCES evaluation_record (id, program_version_id),
    FOREIGN KEY (form_competency_id, program_version_id)
        REFERENCES form_competency (id, program_version_id),
    UNIQUE (evaluation_record_id, form_competency_id),
    UNIQUE (id, program_version_id)
) STRICT;

CREATE TABLE draft_rating_modifier (
    id INTEGER PRIMARY KEY,
    draft_rating_id INTEGER NOT NULL,
    program_version_id INTEGER NOT NULL,
    rating_modifier_id INTEGER NOT NULL,
    FOREIGN KEY (draft_rating_id, program_version_id)
        REFERENCES draft_rating (id, program_version_id),
    FOREIGN KEY (rating_modifier_id, program_version_id)
        REFERENCES rating_modifier (id, program_version_id),
    UNIQUE (draft_rating_id, rating_modifier_id)
) STRICT;

CREATE TABLE draft_narrative (
    id INTEGER PRIMARY KEY,
    evaluation_record_id INTEGER NOT NULL,
    program_version_id INTEGER NOT NULL,
    form_narrative_id INTEGER NOT NULL,
    text TEXT NOT NULL,
    FOREIGN KEY (evaluation_record_id, program_version_id)
        REFERENCES evaluation_record (id, program_version_id),
    FOREIGN KEY (form_narrative_id, program_version_id)
        REFERENCES form_narrative (id, program_version_id),
    UNIQUE (evaluation_record_id, form_narrative_id)
) STRICT;

-- A submitted draft is frozen: while the latest contributor event says
-- submitted_for_review, the working copy refuses every write, so a
-- review is anchored to what was reviewed. A later review decision
-- (slice 4) becomes the latest event and the copy thaws.
CREATE TRIGGER draft_rating_frozen_insert
BEFORE INSERT ON draft_rating
WHEN (SELECT kind FROM contributor_event
      WHERE evaluation_record_id = NEW.evaluation_record_id
      ORDER BY id DESC LIMIT 1) = 'submitted_for_review'
BEGIN
    SELECT RAISE(ABORT, 'a submitted draft is frozen until review');
END;

CREATE TRIGGER draft_rating_frozen_update
BEFORE UPDATE ON draft_rating
WHEN (SELECT kind FROM contributor_event
      WHERE evaluation_record_id = OLD.evaluation_record_id
      ORDER BY id DESC LIMIT 1) = 'submitted_for_review'
BEGIN
    SELECT RAISE(ABORT, 'a submitted draft is frozen until review');
END;

CREATE TRIGGER draft_rating_frozen_delete
BEFORE DELETE ON draft_rating
WHEN (SELECT kind FROM contributor_event
      WHERE evaluation_record_id = OLD.evaluation_record_id
      ORDER BY id DESC LIMIT 1) = 'submitted_for_review'
BEGIN
    SELECT RAISE(ABORT, 'a submitted draft is frozen until review');
END;

CREATE TRIGGER draft_rating_modifier_frozen_insert
BEFORE INSERT ON draft_rating_modifier
WHEN (SELECT kind FROM contributor_event
      WHERE evaluation_record_id = (SELECT evaluation_record_id
                                    FROM draft_rating
                                    WHERE id = NEW.draft_rating_id)
      ORDER BY id DESC LIMIT 1) = 'submitted_for_review'
BEGIN
    SELECT RAISE(ABORT, 'a submitted draft is frozen until review');
END;

CREATE TRIGGER draft_rating_modifier_keeps_identity
BEFORE UPDATE ON draft_rating_modifier
BEGIN
    SELECT RAISE(ABORT, 'modifier changes are inserts and deletes, never edits');
END;

CREATE TRIGGER draft_rating_modifier_frozen_delete
BEFORE DELETE ON draft_rating_modifier
WHEN (SELECT kind FROM contributor_event
      WHERE evaluation_record_id = (SELECT evaluation_record_id
                                    FROM draft_rating
                                    WHERE id = OLD.draft_rating_id)
      ORDER BY id DESC LIMIT 1) = 'submitted_for_review'
BEGIN
    SELECT RAISE(ABORT, 'a submitted draft is frozen until review');
END;

CREATE TRIGGER draft_narrative_frozen_insert
BEFORE INSERT ON draft_narrative
WHEN (SELECT kind FROM contributor_event
      WHERE evaluation_record_id = NEW.evaluation_record_id
      ORDER BY id DESC LIMIT 1) = 'submitted_for_review'
BEGIN
    SELECT RAISE(ABORT, 'a submitted draft is frozen until review');
END;

CREATE TRIGGER draft_narrative_frozen_update
BEFORE UPDATE ON draft_narrative
WHEN (SELECT kind FROM contributor_event
      WHERE evaluation_record_id = OLD.evaluation_record_id
      ORDER BY id DESC LIMIT 1) = 'submitted_for_review'
BEGIN
    SELECT RAISE(ABORT, 'a submitted draft is frozen until review');
END;

CREATE TRIGGER draft_narrative_frozen_delete
BEFORE DELETE ON draft_narrative
WHEN (SELECT kind FROM contributor_event
      WHERE evaluation_record_id = OLD.evaluation_record_id
      ORDER BY id DESC LIMIT 1) = 'submitted_for_review'
BEGIN
    SELECT RAISE(ABORT, 'a submitted draft is frozen until review');
END;

-- Full content snapshots at exactly two workflow points (ADR 0008):
-- submission, and the change-request return the review slice writes.
-- Append-only; the canonical serialization is the service's contract.
CREATE TABLE draft_snapshot (
    id INTEGER PRIMARY KEY,
    evaluation_record_id INTEGER NOT NULL REFERENCES evaluation_record (id),
    reason TEXT NOT NULL CHECK (reason IN ('submission', 'change_request_return')),
    content TEXT NOT NULL,
    taken_at INTEGER NOT NULL,
    taken_by INTEGER REFERENCES user (id)
) STRICT;

CREATE INDEX draft_snapshot_record
    ON draft_snapshot (evaluation_record_id, id);

CREATE TRIGGER draft_snapshot_no_update
BEFORE UPDATE ON draft_snapshot
BEGIN
    SELECT RAISE(ABORT, 'snapshots are append-only');
END;

CREATE TRIGGER draft_snapshot_no_delete
BEFORE DELETE ON draft_snapshot
BEGIN
    SELECT RAISE(ABORT, 'snapshots are append-only');
END;
