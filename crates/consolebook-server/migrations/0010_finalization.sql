-- Milestone 4 slice 1 (ADR 0011): canonical bytes, finalization, and
-- completion rules.
--
-- A finalized EvaluationVersion is the immutable record: its canonical
-- bytes (RFC 8785 subset, record schema 1), SHA-256 content hash, and
-- domain-separated chain hash, finalized instant and actor. The
-- database refuses every later mutation, the frozen derivation gains
-- "finalized records stay frozen", and the workflow gate — approval
-- required when the pinned version's policy says so — holds under raw
-- writes. Completion rules are versioned configuration per program
-- version (ADR 0007's pattern), frozen by publication like all
-- version content.
--
-- Instants are UTC unix seconds (INTEGER). Hashes are lowercase hex.

-- The closed v1 completion-rule set (#32 decision 2), all defaulting
-- on for newly authored versions. Existing versions are backfilled
-- before the freeze triggers below exist — with review approval on,
-- but the two content rules off: their drafts were authored under no
-- completeness contract, and imposing one retroactively could wedge an
-- already-approved copy (frozen since submission, no further decision
-- possible, finalization refusing) with no recovery. Retroactive
-- configuration rewriting workflow expectations is exactly what
-- versioned configuration exists to prevent.
CREATE TABLE finalization_policy (
    program_version_id INTEGER PRIMARY KEY REFERENCES program_version (id),
    review_approved INTEGER NOT NULL DEFAULT 1 CHECK (review_approved IN (0, 1)),
    required_narratives INTEGER NOT NULL DEFAULT 1
        CHECK (required_narratives IN (0, 1)),
    ratings_complete INTEGER NOT NULL DEFAULT 1
        CHECK (ratings_complete IN (0, 1))
) STRICT;

INSERT INTO finalization_policy
    (program_version_id, review_approved, required_narratives, ratings_complete)
SELECT id, 1, 0, 0 FROM program_version;

CREATE TRIGGER finalization_policy_published_no_insert
BEFORE INSERT ON finalization_policy
WHEN (SELECT published_at FROM program_version
      WHERE id = NEW.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER finalization_policy_published_no_update
BEFORE UPDATE ON finalization_policy
WHEN (SELECT published_at FROM program_version
      WHERE id = OLD.program_version_id) IS NOT NULL
    OR (SELECT published_at FROM program_version
        WHERE id = NEW.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

CREATE TRIGGER finalization_policy_published_no_delete
BEFORE DELETE ON finalization_policy
WHEN (SELECT published_at FROM program_version
      WHERE id = OLD.program_version_id) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'published program versions are immutable');
END;

-- The explicit not-observed marker (#32 decision 2): a typed column,
-- never a modifier-code heuristic. A rating carries a value or the
-- marker, never both; the shape is held by triggers because STRICT
-- ALTER cannot add a table CHECK.
ALTER TABLE draft_rating ADD COLUMN not_observed INTEGER NOT NULL DEFAULT 0;

CREATE TRIGGER draft_rating_not_observed_shape_insert
BEFORE INSERT ON draft_rating
WHEN NEW.not_observed NOT IN (0, 1)
    OR (NEW.not_observed = 1 AND NEW.value IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT,
        'a rating carries a value or an explicit not-observed, never both');
END;

CREATE TRIGGER draft_rating_not_observed_shape_update
BEFORE UPDATE ON draft_rating
WHEN NEW.not_observed NOT IN (0, 1)
    OR (NEW.not_observed = 1 AND NEW.value IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT,
        'a rating carries a value or an explicit not-observed, never both');
END;

-- The immutable finalized version (docs/records-integrity.md;
-- ADR 0011): canonical bytes and both fingerprints stored beside the
-- lineage slot slice 3's amendments will use. Slice 1 produces first
-- versions only.
CREATE TABLE evaluation_version (
    id INTEGER PRIMARY KEY,
    evaluation_record_id INTEGER NOT NULL REFERENCES evaluation_record (id),
    version_number INTEGER NOT NULL CHECK (version_number >= 1),
    record_schema INTEGER NOT NULL CHECK (record_schema >= 1),
    canonical_bytes BLOB NOT NULL CHECK (length(canonical_bytes) > 0),
    content_hash TEXT NOT NULL CHECK (
        length(content_hash) = 64 AND content_hash NOT GLOB '*[^0-9a-f]*'
    ),
    chain_hash TEXT NOT NULL CHECK (
        length(chain_hash) = 64 AND chain_hash NOT GLOB '*[^0-9a-f]*'
    ),
    predecessor_id INTEGER REFERENCES evaluation_version (id),
    finalized_at INTEGER NOT NULL,
    finalized_by INTEGER NOT NULL REFERENCES user (id),
    UNIQUE (evaluation_record_id, version_number),
    -- A first version has no predecessor; succession semantics are
    -- slice 3's (amendments).
    CHECK ((version_number = 1) = (predecessor_id IS NULL))
) STRICT;

CREATE INDEX evaluation_version_record
    ON evaluation_version (evaluation_record_id, version_number);

CREATE TRIGGER evaluation_version_no_update
BEFORE UPDATE ON evaluation_version
BEGIN
    SELECT RAISE(ABORT, 'a finalized version is immutable while retained');
END;

CREATE TRIGGER evaluation_version_no_delete
BEFORE DELETE ON evaluation_version
BEGIN
    SELECT RAISE(ABORT, 'a finalized version is immutable while retained');
END;

-- Until amendments (slice 3) define succession, only first versions
-- exist; slice 3 replaces this trigger with the amendment contract.
CREATE TRIGGER evaluation_version_first_versions_only
BEFORE INSERT ON evaluation_version
WHEN NEW.version_number != 1
    OR EXISTS (SELECT 1 FROM evaluation_version
               WHERE evaluation_record_id = NEW.evaluation_record_id)
BEGIN
    SELECT RAISE(ABORT, 'successor versions arrive with amendments');
END;

-- The workflow gate holds raw: when the pinned version's policy
-- requires review approval (missing policy fails closed), a version
-- lands only on a draft whose latest event is an approving decision.
-- The two content-completeness rules (required narratives, ratings
-- complete) are the service's typed contract: they evaluate authored
-- content against configuration, and duplicating that evaluation in
-- SQL would create a second owner of the content rules (ADR 0010).
CREATE TRIGGER evaluation_version_requires_approval
BEFORE INSERT ON evaluation_version
WHEN COALESCE((SELECT fp.review_approved FROM finalization_policy fp
               WHERE fp.program_version_id =
                     (SELECT r.program_version_id FROM evaluation_record r
                      WHERE r.id = NEW.evaluation_record_id)), 1) = 1
    AND NOT ((SELECT ce.kind FROM contributor_event ce
              WHERE ce.evaluation_record_id = NEW.evaluation_record_id
              ORDER BY ce.id DESC LIMIT 1) = 'review_decided'
             AND (SELECT rd.decision FROM review_decision rd
                  WHERE rd.evaluation_record_id = NEW.evaluation_record_id
                  ORDER BY rd.id DESC LIMIT 1) = 'approved')
BEGIN
    SELECT RAISE(ABORT, 'finalization takes an approved draft');
END;

-- Finalized records stay frozen: the one owner of the frozen state
-- (0009) gains the terminal case, and every existing freeze and
-- event-stream guard extends through it unchanged.
DROP VIEW evaluation_record_frozen;

CREATE VIEW evaluation_record_frozen AS
SELECT r.id AS evaluation_record_id,
       CASE
           WHEN EXISTS (SELECT 1 FROM evaluation_version v
                        WHERE v.evaluation_record_id = r.id) THEN 1
           ELSE
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
       END AS frozen
FROM evaluation_record r;
