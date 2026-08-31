-- Milestone 4 slice 4 (ADR 0013): weekly-summary daily links and task
-- signoffs (docs/domain-model.md WeeklySummary, TaskSignoff; #32
-- decisions 5 and 6).
--
-- A weekly summary is an ordinary evaluation record whose working copy
-- also carries links to the exact finalized daily-report versions it
-- covers. A link pins one immutable version: a later amendment of the
-- daily never rewrites what the summary summarized. Links are authored
-- while the copy is editable, freeze with it, and seal into the
-- record-schema-2 envelope.
--
-- A task signoff is a versioned record that a configured task was
-- observed or demonstrated: append-only rows per (enrollment, task)
-- where the latest row answers the current state. Overrides — any row
-- after the first — require explicit authority and a recorded reason;
-- the authority is the service's typed contract (ADR 0010), the reason
-- and ordering hold raw here.
--
-- Instants are UTC unix seconds (INTEGER).

CREATE TABLE summary_daily_link (
    id INTEGER PRIMARY KEY,
    summary_record_id INTEGER NOT NULL REFERENCES evaluation_record (id),
    daily_version_id INTEGER NOT NULL REFERENCES evaluation_version (id),
    UNIQUE (summary_record_id, daily_version_id)
) STRICT;

CREATE INDEX summary_daily_link_version
    ON summary_daily_link (daily_version_id);

-- Links are added and removed while the copy is editable, never edited
-- in place: an UPDATE could re-point a reviewed link.
CREATE TRIGGER summary_daily_link_no_update
BEFORE UPDATE ON summary_daily_link
BEGIN
    SELECT RAISE(ABORT, 'summary links are added and removed, never edited');
END;

-- Only weekly-summary records carry daily links.
CREATE TRIGGER summary_daily_link_takes_a_summary
BEFORE INSERT ON summary_daily_link
WHEN (SELECT f.record_type FROM evaluation_record r
      JOIN evaluation_form f ON f.id = r.evaluation_form_id
      WHERE r.id = NEW.summary_record_id) IS NOT 'weekly_summary'
BEGIN
    SELECT RAISE(ABORT, 'daily links belong to weekly summaries');
END;

-- A summary links finalized daily reports of its own enrollment: the
-- linked version's record shares the summary's enrollment and is typed
-- daily_report. The version row itself is finality (it exists only
-- once sealed).
CREATE TRIGGER summary_daily_link_stays_home
BEFORE INSERT ON summary_daily_link
WHEN (SELECT r.enrollment_id FROM evaluation_version v
      JOIN evaluation_record r ON r.id = v.evaluation_record_id
      WHERE v.id = NEW.daily_version_id)
    IS NOT (SELECT enrollment_id FROM evaluation_record
            WHERE id = NEW.summary_record_id)
BEGIN
    SELECT RAISE(ABORT,
        'a summary links its own enrollment''s daily reports');
END;

CREATE TRIGGER summary_daily_link_takes_dailies
BEFORE INSERT ON summary_daily_link
WHEN (SELECT f.record_type FROM evaluation_version v
      JOIN evaluation_record r ON r.id = v.evaluation_record_id
      JOIN evaluation_form f ON f.id = r.evaluation_form_id
      WHERE v.id = NEW.daily_version_id) IS NOT 'daily_report'
BEGIN
    SELECT RAISE(ABORT, 'a summary links finalized daily reports');
END;

-- The links are part of what a submission or approval attests: they
-- freeze and thaw with the working copy (0009's one owner).
CREATE TRIGGER summary_daily_link_frozen_insert
BEFORE INSERT ON summary_daily_link
WHEN (SELECT frozen FROM evaluation_record_frozen
      WHERE evaluation_record_id = NEW.summary_record_id) = 1
BEGIN
    SELECT RAISE(ABORT, 'a submitted or approved draft is frozen');
END;

CREATE TRIGGER summary_daily_link_frozen_delete
BEFORE DELETE ON summary_daily_link
WHEN (SELECT frozen FROM evaluation_record_frozen
      WHERE evaluation_record_id = OLD.summary_record_id) = 1
BEGIN
    SELECT RAISE(ABORT, 'a submitted or approved draft is frozen');
END;

CREATE TABLE task_signoff (
    id INTEGER PRIMARY KEY,
    enrollment_id INTEGER NOT NULL REFERENCES enrollment (id),
    task_id INTEGER NOT NULL REFERENCES task (id),
    kind TEXT NOT NULL CHECK (kind IN ('observed', 'demonstrated', 'revoked')),
    -- Empty for a first signoff; an override explains itself (trigger
    -- below, the 0009 White_Space set).
    reason TEXT NOT NULL,
    signed_by INTEGER NOT NULL REFERENCES user (id),
    -- Presentation-name snapshot (the 0011/0012 precedent): the
    -- permanent signoff displays its authority as named at the act.
    signed_by_display_name TEXT NOT NULL
        CHECK (length(signed_by_display_name) > 0),
    signed_at INTEGER NOT NULL
) STRICT;

CREATE INDEX task_signoff_enrollment
    ON task_signoff (enrollment_id, task_id, id);

CREATE TRIGGER task_signoff_no_update
BEFORE UPDATE ON task_signoff
BEGIN
    SELECT RAISE(ABORT, 'task signoffs are versioned, never edited');
END;

CREATE TRIGGER task_signoff_no_delete
BEFORE DELETE ON task_signoff
BEGIN
    SELECT RAISE(ABORT, 'task signoffs are versioned, never edited');
END;

-- The signed task belongs to the enrollment's pinned version
-- (invariant 5's spirit): agency vocabulary never crosses enrollments.
CREATE TRIGGER task_signoff_takes_a_pinned_task
BEFORE INSERT ON task_signoff
WHEN (SELECT t.program_version_id FROM task t WHERE t.id = NEW.task_id)
    IS NOT (SELECT e.program_version_id FROM enrollment e
            WHERE e.id = NEW.enrollment_id)
BEGIN
    SELECT RAISE(ABORT,
        'a signoff takes a task of the enrollment''s pinned version');
END;

-- Any row after the first supersedes it: an override records its
-- reason, and a revocation has something to revoke.
CREATE TRIGGER task_signoff_overrides_explain
BEFORE INSERT ON task_signoff
WHEN EXISTS (SELECT 1 FROM task_signoff
             WHERE enrollment_id = NEW.enrollment_id
               AND task_id = NEW.task_id)
    AND length(trim(NEW.reason, char(9, 10, 11, 12, 13, 32, 133, 160,
                                     5760, 8192, 8193, 8194, 8195, 8196,
                                     8197, 8198, 8199, 8200, 8201, 8202,
                                     8232, 8233, 8239, 8287, 12288))) = 0
BEGIN
    SELECT RAISE(ABORT, 'a signoff override records its reason');
END;

CREATE TRIGGER task_signoff_revocations_supersede
BEFORE INSERT ON task_signoff
WHEN NEW.kind = 'revoked'
    AND NOT EXISTS (SELECT 1 FROM task_signoff
                    WHERE enrollment_id = NEW.enrollment_id
                      AND task_id = NEW.task_id)
BEGIN
    SELECT RAISE(ABORT, 'a revocation supersedes a signoff');
END;
