-- ADR 0018; #51: only a version change may name program versions.
-- 0006's CHECK rejects two references on other kinds but admits one.
-- Preserve that migration's checksum and the referenced, append-only table.
-- Its existing no-update trigger and this insert trigger enforce the full
-- shape without rebuilding history or disabling foreign-key enforcement.

-- Refuse an upgrade over malformed retained history. Do not infer which
-- reference was intended or silently discard it. SQLx runs this migration
-- and its ledger entry in one transaction; failure rolls back this guard.
CREATE TEMP TABLE enrollment_event_shape_upgrade_guard (
    valid INTEGER NOT NULL,
    CONSTRAINT enrollment_event_legacy_version_references_invalid CHECK (valid = 1)
) STRICT;

INSERT INTO enrollment_event_shape_upgrade_guard (valid)
SELECT 0 FROM enrollment_event
WHERE NOT CASE kind
    WHEN 'version_change'
        THEN from_program_version_id IS NOT NULL AND to_program_version_id IS NOT NULL
    ELSE from_program_version_id IS NULL AND to_program_version_id IS NULL
END
LIMIT 1;

DROP TABLE enrollment_event_shape_upgrade_guard;

CREATE TRIGGER enrollment_event_version_reference_shape
BEFORE INSERT ON enrollment_event
WHEN NOT CASE NEW.kind
    WHEN 'version_change'
        THEN NEW.from_program_version_id IS NOT NULL AND NEW.to_program_version_id IS NOT NULL
    ELSE NEW.from_program_version_id IS NULL AND NEW.to_program_version_id IS NULL
END
BEGIN
    SELECT RAISE(ABORT, 'enrollment events name both versions for a version change and neither otherwise');
END;
