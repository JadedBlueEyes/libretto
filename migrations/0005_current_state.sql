BEGIN;

CREATE TABLE current_state (
	user_id     TEXT    NOT NULL,
	room_id     TEXT    NOT NULL,
	event_type  TEXT    NOT NULL,
	state_key   TEXT    NOT NULL,
	event_rowid INTEGER NOT NULL,

	PRIMARY KEY (room_id, user_id, event_type, state_key),
	CONSTRAINT current_state_room_fkey FOREIGN KEY (room_id, user_id) REFERENCES room (room_id, user_id),
	CONSTRAINT current_state_event_fkey FOREIGN KEY (event_rowid) REFERENCES event (rowid),
	CONSTRAINT current_state_rowid_unique UNIQUE (event_rowid)
);

CREATE INDEX current_state_room_id_idx ON current_state (room_id);
CREATE INDEX current_state_user_id_idx ON current_state (user_id);
CREATE INDEX current_state_event_type_idx ON current_state (event_type);

COMMIT;
