-- Google Meet in Protokol's own window: the participant behind each voice.

-- The Meet participant (the name on the tile) a meeting's voice came from
ALTER TABLE meeting_speakers ADD COLUMN meet_name TEXT;
-- Where the shown name came from: voice, meet_link, meet_link_doubt,
-- meet_same_name, meet
ALTER TABLE meeting_speakers ADD COLUMN name_source TEXT;

-- Meet accounts seen in recordings and the saved person each one is.
-- shared = 1: several people speak from it, it is not linked to anyone.
CREATE TABLE IF NOT EXISTS meet_accounts (
    meet_name TEXT PRIMARY KEY NOT NULL,
    person TEXT,
    shared INTEGER NOT NULL DEFAULT 0,
    linked_at TEXT,
    last_seen TEXT
);
