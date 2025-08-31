BEGIN;

-- Drop indexes
DROP INDEX IF EXISTS current_state_event_type_idx;
DROP INDEX IF EXISTS current_state_user_id_idx;
DROP INDEX IF EXISTS current_state_room_id_idx;

-- Drop the current_state table
DROP TABLE IF EXISTS current_state;

COMMIT;
