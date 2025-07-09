CREATE TABLE room (
	room_id              TEXT    NOT NULL,
	user_id              TEXT    NOT NULL,
	PRIMARY KEY (room_id, user_id),

	-- The room type, if there is one
	room_type            TEXT,
	-- The m.room.create state event’s content of this room if one has been received.
	creation_content     JSONB,
	-- The m.room.tombstone state event’s content of this room if one has been received.
	tombstone_content    JSONB,

	-- Display name of the room, JSON from the SDK
	name                 JSONB,
	-- MXC URI for avatar
	avatar               TEXT,

	-- Plain text room topic (HTML topics are not in the Rust SDK yet)
	topic                TEXT,
	canonical_alias      TEXT,

	-- either unknown, enabled or disabled
	encryption_state     TEXT,

	last_event_timestamp INTEGER,

	unread_highlight_count    INTEGER NOT NULL DEFAULT 0,
	unread_notification_count INTEGER NOT NULL DEFAULT 0,

	-- Pagination token for backfill
	prev_batch           TEXT
);
