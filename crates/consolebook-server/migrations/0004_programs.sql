-- Milestone 2: versioned program configuration (ADR 0007).
--
-- A program is the stable identity; a program_version is the publishable
-- unit, mutable while draft and frozen by publishing. Version contents are
-- owned typed rows keyed to the owning program_version. Every owned table
-- carries program_version_id and every cross-reference uses a composite
-- foreign key including it, so domain invariant 5 — all referenced
-- configuration belongs to the pinned version — is enforced by the
-- database, not application discipline.
--
-- Once published_at is set, the version row and every owned row refuse
-- INSERT, UPDATE, and DELETE at the database: the same enforcement class
-- as audit_event. Draft rows remain freely editable.
--
-- Instants are UTC unix seconds (INTEGER). Phases are optional: a version
-- with no phases is a valid shape (annual and in-service programs have
-- topics, not progressions).

CREATE TABLE program (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL CHECK (length(name) > 0),
    created_at INTEGER NOT NULL,
    created_by INTEGER REFERENCES user (id)
) STRICT;

CREATE UNIQUE INDEX program_name_unique ON program (name COLLATE NOCASE);

CREATE TABLE program_version (
    id INTEGER PRIMARY KEY,
    program_id INTEGER NOT NULL REFERENCES program (id),
    -- Internal monotonic number: identity and ordering authority.
    version_number INTEGER NOT NULL CHECK (version_number >= 1),
    -- Agency-visible free-text label ("2026 CTO Program rev B"):
    -- presentation, never identity.
    label TEXT NOT NULL,
    -- Snapshot of the program name as authored for this version. A later
    -- program rename never rewrites what a pinned version presented.
    name TEXT NOT NULL CHECK (length(name) > 0),
    description TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    created_by INTEGER REFERENCES user (id),
    published_at INTEGER,
    published_by INTEGER REFERENCES user (id),
    UNIQUE (program_id, version_number)
) STRICT;

-- Owned configuration rows. Each declares UNIQUE (id, program_version_id)
-- so referencing tables can require, by foreign key, that both sides of a
-- reference belong to the same version.

CREATE TABLE phase (
    id INTEGER PRIMARY KEY,
    program_version_id INTEGER NOT NULL REFERENCES program_version (id),
    name TEXT NOT NULL CHECK (length(name) > 0),
    description TEXT NOT NULL,
    -- Presentation data (docs/domain-model.md): ordering, never progress.
    presentation_number INTEGER NOT NULL,
    UNIQUE (id, program_version_id)
) STRICT;

CREATE UNIQUE INDEX phase_name_unique
    ON phase (program_version_id, name COLLATE NOCASE);

-- The allowed-transition graph: explicit directed edges, no rules engine
-- (ADR 0007). Kind is presentation semantics for the edge.
CREATE TABLE phase_transition (
    id INTEGER PRIMARY KEY,
    program_version_id INTEGER NOT NULL REFERENCES program_version (id),
    from_phase_id INTEGER NOT NULL,
    to_phase_id INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('advance', 'remediation', 'skip', 'restart')),
    FOREIGN KEY (from_phase_id, program_version_id)
        REFERENCES phase (id, program_version_id),
    FOREIGN KEY (to_phase_id, program_version_id)
        REFERENCES phase (id, program_version_id),
    UNIQUE (program_version_id, from_phase_id, to_phase_id)
) STRICT;

CREATE TABLE competency (
    id INTEGER PRIMARY KEY,
    program_version_id INTEGER NOT NULL REFERENCES program_version (id),
    -- Free-text grouping label; empty means uncategorized.
    category TEXT NOT NULL,
    name TEXT NOT NULL CHECK (length(name) > 0),
    description TEXT NOT NULL,
    sort_order INTEGER NOT NULL,
    UNIQUE (id, program_version_id)
) STRICT;

CREATE UNIQUE INDEX competency_name_unique
    ON competency (program_version_id, name COLLATE NOCASE);

CREATE TABLE task (
    id INTEGER PRIMARY KEY,
    program_version_id INTEGER NOT NULL REFERENCES program_version (id),
    competency_id INTEGER NOT NULL,
    prompt TEXT NOT NULL CHECK (length(prompt) > 0),
    sort_order INTEGER NOT NULL,
    FOREIGN KEY (competency_id, program_version_id)
        REFERENCES competency (id, program_version_id),
    UNIQUE (id, program_version_id)
) STRICT;

CREATE UNIQUE INDEX task_prompt_unique
    ON task (competency_id, prompt COLLATE NOCASE);

-- Rating scales are a closed set of product-defined kinds with
-- agency-defined content (ADR 0007). Numeric bounds exist exactly when the
-- kind is anchored_numeric.
CREATE TABLE rating_scale (
    id INTEGER PRIMARY KEY,
    program_version_id INTEGER NOT NULL REFERENCES program_version (id),
    name TEXT NOT NULL CHECK (length(name) > 0),
    kind TEXT NOT NULL CHECK (kind IN ('anchored_numeric', 'pass_fail', 'narrative_only')),
    min_value INTEGER,
    max_value INTEGER,
    CHECK ((kind = 'anchored_numeric') = (min_value IS NOT NULL AND max_value IS NOT NULL)),
    CHECK (min_value IS NULL OR max_value IS NULL OR min_value < max_value),
    UNIQUE (id, program_version_id)
) STRICT;

CREATE UNIQUE INDEX rating_scale_name_unique
    ON rating_scale (program_version_id, name COLLATE NOCASE);

CREATE TABLE rating_anchor (
    id INTEGER PRIMARY KEY,
    program_version_id INTEGER NOT NULL REFERENCES program_version (id),
    rating_scale_id INTEGER NOT NULL,
    value INTEGER NOT NULL,
    label TEXT NOT NULL CHECK (length(label) > 0),
    definition TEXT NOT NULL,
    FOREIGN KEY (rating_scale_id, program_version_id)
        REFERENCES rating_scale (id, program_version_id),
    UNIQUE (rating_scale_id, value)
) STRICT;

CREATE TABLE rating_modifier (
    id INTEGER PRIMARY KEY,
    program_version_id INTEGER NOT NULL REFERENCES program_version (id),
    code TEXT NOT NULL CHECK (length(code) > 0),
    label TEXT NOT NULL CHECK (length(label) > 0),
    description TEXT NOT NULL,
    UNIQUE (id, program_version_id)
) STRICT;

CREATE UNIQUE INDEX rating_modifier_code_unique
    ON rating_modifier (program_version_id, code COLLATE NOCASE);

-- The product owns the form skeleton per record type; these rows are the
-- agency-configured content that populates it (ADR 0007). There are no
-- configurable sections.
CREATE TABLE evaluation_form (
    id INTEGER PRIMARY KEY,
    program_version_id INTEGER NOT NULL REFERENCES program_version (id),
    record_type TEXT NOT NULL CHECK (record_type IN ('daily_report', 'weekly_summary', 'phase_evaluation')),
    name TEXT NOT NULL CHECK (length(name) > 0),
    instructions TEXT NOT NULL,
    UNIQUE (id, program_version_id)
) STRICT;

CREATE UNIQUE INDEX evaluation_form_name_unique
    ON evaluation_form (program_version_id, name COLLATE NOCASE);

CREATE TABLE form_competency (
    id INTEGER PRIMARY KEY,
    program_version_id INTEGER NOT NULL REFERENCES program_version (id),
    evaluation_form_id INTEGER NOT NULL,
    competency_id INTEGER NOT NULL,
    rating_scale_id INTEGER NOT NULL,
    sort_order INTEGER NOT NULL,
    FOREIGN KEY (evaluation_form_id, program_version_id)
        REFERENCES evaluation_form (id, program_version_id),
    FOREIGN KEY (competency_id, program_version_id)
        REFERENCES competency (id, program_version_id),
    FOREIGN KEY (rating_scale_id, program_version_id)
        REFERENCES rating_scale (id, program_version_id),
    UNIQUE (evaluation_form_id, competency_id)
) STRICT;

CREATE TABLE form_narrative (
    id INTEGER PRIMARY KEY,
    program_version_id INTEGER NOT NULL REFERENCES program_version (id),
    evaluation_form_id INTEGER NOT NULL,
    prompt TEXT NOT NULL CHECK (length(prompt) > 0),
    required INTEGER NOT NULL CHECK (required IN (0, 1)),
    sort_order INTEGER NOT NULL,
    FOREIGN KEY (evaluation_form_id, program_version_id)
        REFERENCES evaluation_form (id, program_version_id)
) STRICT;

CREATE INDEX form_narrative_form ON form_narrative (evaluation_form_id);

-- Agency-entered citations to external standards (CALEA, APCO ANS, state
-- continuing-education requirements). The product stores the agency's
-- mapping, never the standard's text (ADR 0007). A citation attaches to
-- the whole version (both targets NULL), one competency, or one task.
CREATE TABLE standards_citation (
    id INTEGER PRIMARY KEY,
    program_version_id INTEGER NOT NULL REFERENCES program_version (id),
    competency_id INTEGER,
    task_id INTEGER,
    body TEXT NOT NULL CHECK (length(body) > 0),
    edition TEXT NOT NULL,
    clause TEXT NOT NULL CHECK (length(clause) > 0),
    note TEXT NOT NULL,
    CHECK (competency_id IS NULL OR task_id IS NULL),
    FOREIGN KEY (competency_id, program_version_id)
        REFERENCES competency (id, program_version_id),
    FOREIGN KEY (task_id, program_version_id)
        REFERENCES task (id, program_version_id)
) STRICT;

CREATE INDEX standards_citation_version ON standards_citation (program_version_id);

-- Audit events gain a generic domain subject so lifecycle events can name
-- what they acted on. Deliberately no foreign key: audit rows are
-- append-only and must never block lawful disposition of their subjects.
-- Existing rows keep NULLs.
ALTER TABLE audit_event ADD COLUMN subject_kind TEXT;
ALTER TABLE audit_event ADD COLUMN subject_id INTEGER;

-- Publish freeze, enforced by the database rather than application
-- manners. The version row rejects UPDATE and DELETE once published_at is
-- set (the publishing UPDATE itself sees OLD.published_at IS NULL and
-- passes). Every owned table rejects INSERT, UPDATE, and DELETE whenever
-- the owning version — old or new parent, so rows cannot be moved into or
-- out of a published version — is published.

CREATE TRIGGER program_version_published_no_update
BEFORE UPDATE ON program_version
WHEN OLD.published_at IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER program_version_published_no_delete
BEFORE DELETE ON program_version
WHEN OLD.published_at IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER phase_published_no_insert
BEFORE INSERT ON phase
WHEN (SELECT published_at FROM program_version WHERE id = NEW.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER phase_published_no_update
BEFORE UPDATE ON phase
WHEN (SELECT published_at FROM program_version WHERE id = OLD.program_version_id) IS NOT NULL
    OR (SELECT published_at FROM program_version WHERE id = NEW.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER phase_published_no_delete
BEFORE DELETE ON phase
WHEN (SELECT published_at FROM program_version WHERE id = OLD.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER phase_transition_published_no_insert
BEFORE INSERT ON phase_transition
WHEN (SELECT published_at FROM program_version WHERE id = NEW.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER phase_transition_published_no_update
BEFORE UPDATE ON phase_transition
WHEN (SELECT published_at FROM program_version WHERE id = OLD.program_version_id) IS NOT NULL
    OR (SELECT published_at FROM program_version WHERE id = NEW.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER phase_transition_published_no_delete
BEFORE DELETE ON phase_transition
WHEN (SELECT published_at FROM program_version WHERE id = OLD.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER competency_published_no_insert
BEFORE INSERT ON competency
WHEN (SELECT published_at FROM program_version WHERE id = NEW.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER competency_published_no_update
BEFORE UPDATE ON competency
WHEN (SELECT published_at FROM program_version WHERE id = OLD.program_version_id) IS NOT NULL
    OR (SELECT published_at FROM program_version WHERE id = NEW.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER competency_published_no_delete
BEFORE DELETE ON competency
WHEN (SELECT published_at FROM program_version WHERE id = OLD.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER task_published_no_insert
BEFORE INSERT ON task
WHEN (SELECT published_at FROM program_version WHERE id = NEW.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER task_published_no_update
BEFORE UPDATE ON task
WHEN (SELECT published_at FROM program_version WHERE id = OLD.program_version_id) IS NOT NULL
    OR (SELECT published_at FROM program_version WHERE id = NEW.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER task_published_no_delete
BEFORE DELETE ON task
WHEN (SELECT published_at FROM program_version WHERE id = OLD.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER rating_scale_published_no_insert
BEFORE INSERT ON rating_scale
WHEN (SELECT published_at FROM program_version WHERE id = NEW.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER rating_scale_published_no_update
BEFORE UPDATE ON rating_scale
WHEN (SELECT published_at FROM program_version WHERE id = OLD.program_version_id) IS NOT NULL
    OR (SELECT published_at FROM program_version WHERE id = NEW.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER rating_scale_published_no_delete
BEFORE DELETE ON rating_scale
WHEN (SELECT published_at FROM program_version WHERE id = OLD.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER rating_anchor_published_no_insert
BEFORE INSERT ON rating_anchor
WHEN (SELECT published_at FROM program_version WHERE id = NEW.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER rating_anchor_published_no_update
BEFORE UPDATE ON rating_anchor
WHEN (SELECT published_at FROM program_version WHERE id = OLD.program_version_id) IS NOT NULL
    OR (SELECT published_at FROM program_version WHERE id = NEW.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER rating_anchor_published_no_delete
BEFORE DELETE ON rating_anchor
WHEN (SELECT published_at FROM program_version WHERE id = OLD.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER rating_modifier_published_no_insert
BEFORE INSERT ON rating_modifier
WHEN (SELECT published_at FROM program_version WHERE id = NEW.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER rating_modifier_published_no_update
BEFORE UPDATE ON rating_modifier
WHEN (SELECT published_at FROM program_version WHERE id = OLD.program_version_id) IS NOT NULL
    OR (SELECT published_at FROM program_version WHERE id = NEW.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER rating_modifier_published_no_delete
BEFORE DELETE ON rating_modifier
WHEN (SELECT published_at FROM program_version WHERE id = OLD.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER evaluation_form_published_no_insert
BEFORE INSERT ON evaluation_form
WHEN (SELECT published_at FROM program_version WHERE id = NEW.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER evaluation_form_published_no_update
BEFORE UPDATE ON evaluation_form
WHEN (SELECT published_at FROM program_version WHERE id = OLD.program_version_id) IS NOT NULL
    OR (SELECT published_at FROM program_version WHERE id = NEW.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER evaluation_form_published_no_delete
BEFORE DELETE ON evaluation_form
WHEN (SELECT published_at FROM program_version WHERE id = OLD.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER form_competency_published_no_insert
BEFORE INSERT ON form_competency
WHEN (SELECT published_at FROM program_version WHERE id = NEW.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER form_competency_published_no_update
BEFORE UPDATE ON form_competency
WHEN (SELECT published_at FROM program_version WHERE id = OLD.program_version_id) IS NOT NULL
    OR (SELECT published_at FROM program_version WHERE id = NEW.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER form_competency_published_no_delete
BEFORE DELETE ON form_competency
WHEN (SELECT published_at FROM program_version WHERE id = OLD.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER form_narrative_published_no_insert
BEFORE INSERT ON form_narrative
WHEN (SELECT published_at FROM program_version WHERE id = NEW.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER form_narrative_published_no_update
BEFORE UPDATE ON form_narrative
WHEN (SELECT published_at FROM program_version WHERE id = OLD.program_version_id) IS NOT NULL
    OR (SELECT published_at FROM program_version WHERE id = NEW.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER form_narrative_published_no_delete
BEFORE DELETE ON form_narrative
WHEN (SELECT published_at FROM program_version WHERE id = OLD.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER standards_citation_published_no_insert
BEFORE INSERT ON standards_citation
WHEN (SELECT published_at FROM program_version WHERE id = NEW.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER standards_citation_published_no_update
BEFORE UPDATE ON standards_citation
WHEN (SELECT published_at FROM program_version WHERE id = OLD.program_version_id) IS NOT NULL
    OR (SELECT published_at FROM program_version WHERE id = NEW.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER standards_citation_published_no_delete
BEFORE DELETE ON standards_citation
WHEN (SELECT published_at FROM program_version WHERE id = OLD.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;
