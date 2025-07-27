BEGIN;

CREATE TABLE event (
    rowid               SERIAL PRIMARY KEY,

    -- User who saw the event
    user_id             TEXT    NOT NULL,
    -- Room the event was in
    room_id             TEXT    NOT NULL,
    event_id            TEXT    NOT NULL,

    sender              TEXT NOT NULL,
    timestamp           BIGINT NOT NULL,

    transaction_id      TEXT,
    unsigned            JSONB NOT NULL DEFAULT '{}',

    -- We don't store encrypted and decrypted separately as
    -- the Rust SDK doesn't expose that, and we don't want
    -- to inadvertently become a map of ciphertext ->
    -- cleartext that could be used in some kind of attack.
    -- If the event type is m.room.encrypted, when the message is UTD and
    content             JSONB NOT NULL,
    event_type          TEXT NOT NULL,

    -- State events
    state_key           TEXT,

    -- Relation columns
    redacted_by         TEXT,
    relates_to          TEXT,
    relation_type       TEXT,
    last_edit_rowid     INTEGER,

    -- For encryption retries
    megolm_session_id   TEXT,

    -- Unique constraint to prevent duplicate events
    UNIQUE (room_id, event_id)
);

CREATE INDEX event_room_id_idx ON event (room_id);
CREATE INDEX event_redacted_by_idx ON event (room_id, redacted_by);
CREATE INDEX event_relates_to_idx ON event (room_id, relates_to);
CREATE INDEX event_megolm_session_id_idx ON event (room_id, megolm_session_id);

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

CREATE TRIGGER event_update_redacted_by
AFTER INSERT ON event
FOR EACH ROW EXECUTE FUNCTION event_update_redacted_by_fn();

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

CREATE TRIGGER event_update_last_edit_when_redacted
AFTER UPDATE ON event
FOR EACH ROW EXECUTE FUNCTION event_update_last_edit_when_redacted_fn();

-- Trigger: event_insert_update_last_edit
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

CREATE TRIGGER event_insert_update_last_edit
AFTER INSERT ON event
FOR EACH ROW EXECUTE FUNCTION event_insert_update_last_edit_fn();

CREATE TABLE media (
    mxc TEXT PRIMARY KEY
);

CREATE TABLE media_reference (
    event_rowid INTEGER NOT NULL,
    media_mxc TEXT NOT NULL,

    PRIMARY KEY (event_rowid, media_mxc),
    CONSTRAINT media_reference_event_fkey FOREIGN KEY (event_rowid) REFERENCES event (rowid) ON DELETE CASCADE,
    CONSTRAINT media_reference_media_fkey FOREIGN KEY (media_mxc) REFERENCES media (mxc) ON DELETE CASCADE
);

COMMIT;
