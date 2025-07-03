CREATE TABLE account (
	user_id         TEXT NOT NULL PRIMARY KEY,
	device_id       TEXT NOT NULL,
	access_token    TEXT NOT NULL,
	refresh_token   TEXT,
	db_passphrase   TEXT NOT NULL,
	-- e.g. https://matrix.ellis.link
	homeserver_url  TEXT NOT NULL,
	-- Relative to configured data dir
	db_path         TEXT NOT NULL,

	-- Sync token
	next_batch      TEXT
);
