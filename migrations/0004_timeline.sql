BEGIN;

-- FIRST: fixing a mistake in the last schema migration

-- Drop the existing unique constraint on (room_id, event_id)
ALTER TABLE event DROP CONSTRAINT event_room_id_event_id_key;

-- Add new unique constraint on (user_id, room_id, event_id)
ALTER TABLE event ADD CONSTRAINT event_user_id_room_id_event_id_key UNIQUE (user_id, room_id, event_id);

-- Update stored procedures to account for user_id

-- Update redaction function to only affect events for the same user
CREATE OR REPLACE FUNCTION event_update_redacted_by_fn() RETURNS TRIGGER AS $$
BEGIN
    IF NEW.event_type = 'm.room.redaction' THEN
        UPDATE event
        SET redacted_by = NEW.event_id
        WHERE room_id = NEW.room_id
          AND user_id = NEW.user_id
          AND event_id = NEW.content->>'redacts';
    END IF;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

-- Update last edit when redacted function to consider user_id
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
               AND edit.user_id = event.user_id
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
          AND user_id = NEW.user_id
          AND last_edit_rowid = NEW.rowid
          AND state_key IS NULL
          AND (relation_type IS NULL OR relation_type NOT IN ('m.replace', 'm.annotation'));
    END IF;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

-- Update insert/update last edit function to consider user_id
CREATE OR REPLACE FUNCTION event_insert_update_last_edit_fn() RETURNS TRIGGER AS $$
BEGIN
    IF NEW.relation_type = 'm.replace'
       AND NEW.redacted_by IS NULL
       AND NEW.state_key IS NULL THEN
        UPDATE event
        SET last_edit_rowid = NEW.rowid
        WHERE event_id = NEW.relates_to
          AND user_id = NEW.user_id
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

-- SECOND: Adding a timeline

CREATE TABLE timeline (
	rowid       SERIAL  PRIMARY KEY,
	room_id     TEXT    NOT NULL,
	user_id     TEXT    NOT NULL,
	event_rowid INTEGER NOT NULL,

	CONSTRAINT timeline_event_fkey FOREIGN KEY (event_rowid) REFERENCES event (rowid) ON DELETE CASCADE,
	CONSTRAINT timeline_event_unique_key UNIQUE (event_rowid) -- events are already unique per user/room
);
CREATE INDEX timeline_room_id_idx ON timeline (room_id);

COMMIT;
