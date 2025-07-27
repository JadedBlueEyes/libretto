BEGIN;

-- Drop tables in reverse order (respecting foreign key constraints)
DROP TABLE IF EXISTS media_reference;
DROP TABLE IF EXISTS media;

-- Drop triggers
DROP TRIGGER IF EXISTS event_insert_update_last_edit ON event;
DROP TRIGGER IF EXISTS event_update_last_edit_when_redacted ON event;
DROP TRIGGER IF EXISTS event_update_redacted_by ON event;

-- Drop trigger functions
DROP FUNCTION IF EXISTS event_insert_update_last_edit_fn();
DROP FUNCTION IF EXISTS event_update_last_edit_when_redacted_fn();
DROP FUNCTION IF EXISTS event_update_redacted_by_fn();

-- Drop indexes
DROP INDEX IF EXISTS event_megolm_session_id_idx;
DROP INDEX IF EXISTS event_relates_to_idx;
DROP INDEX IF EXISTS event_redacted_by_idx;
DROP INDEX IF EXISTS event_room_id_idx;

-- Drop the event table
DROP TABLE IF EXISTS event;

COMMIT;
