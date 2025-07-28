BEGIN;

-- Drop the unique constraint on (user_id, room_id, event_id)
ALTER TABLE event DROP CONSTRAINT event_user_id_room_id_event_id_key;

-- Add back the original unique constraint on (room_id, event_id)
ALTER TABLE event ADD CONSTRAINT event_room_id_event_id_key UNIQUE (room_id, event_id);

-- Revert stored procedures to original versions

-- Revert redaction function to original version
CREATE OR REPLACE FUNCTION event_update_redacted_by_fn() RETURNS TRIGGER AS $$
BEGIN
    IF NEW.event_type = 'm.room.redaction' THEN
        UPDATE event
        SET redacted_by = NEW.event_id
        WHERE room_id = NEW.room_id
          AND event_id = NEW.content->>'redacts';
    END IF;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

-- Revert last edit when redacted function to original version
CREATE OR REPLACE FUNCTION event_update_last_edit_when_redacted_fn() RETURNS TRIGGER AS $$
BEGIN
    IF OLD.redacted_by IS NULL
       AND NEW.redacted_by IS NOT NULL
       AND NEW.relation_type = 'm.replace'
       AND NEW.state_key IS NULL THEN
        UPDATE event
        SET last_edit_rowid = COALESCE(
            (SELECT rowid FROM event edit
             WHERE edit.room_id = event.room_id
               AND edit.relates_to = event.event_id
               AND edit.relation_type = 'm.replace'
               AND edit.event_type = event.event_type
               AND edit.sender = event.sender
               AND edit.redacted_by IS NULL
               AND edit.state_key IS NULL
             ORDER BY edit.timestamp DESC
             LIMIT 1),
            0)
        WHERE event_id = NEW.relates_to
          AND last_edit_rowid = NEW.rowid
          AND state_key IS NULL
          AND (relation_type IS NULL OR relation_type NOT IN ('m.replace', 'm.annotation'));
    END IF;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

-- Revert insert/update last edit function to original version
CREATE OR REPLACE FUNCTION event_insert_update_last_edit_fn() RETURNS TRIGGER AS $$
BEGIN
    IF NEW.relation_type = 'm.replace'
       AND NEW.redacted_by IS NULL
       AND NEW.state_key IS NULL THEN
        UPDATE event
        SET last_edit_rowid = NEW.rowid
        WHERE event_id = NEW.relates_to
          AND event_type = NEW.event_type
          AND sender = NEW.sender
          AND state_key IS NULL
          AND (relation_type IS NULL OR relation_type NOT IN ('m.replace', 'm.annotation'))
          AND NEW.timestamp >
              COALESCE((SELECT prev_edit.timestamp FROM event prev_edit WHERE prev_edit.rowid = event.last_edit_rowid), 0);
    END IF;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

DROP TABLE IF EXISTS timeline;
DROP INDEX IF EXISTS timeline_room_id_idx;

COMMIT;
