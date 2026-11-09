-- Speakers recognised by voice in a meeting, and saved voice profiles.
-- Voice data is a short numeric vector per speaker; it stays in this database.
CREATE TABLE IF NOT EXISTS meeting_speakers (
    meeting_id TEXT NOT NULL,
    label TEXT NOT NULL,            -- «Спікер N» as first assigned
    name TEXT,                      -- name given by the user or matched from a profile
    centroid BLOB NOT NULL,         -- voice embedding (f32 little-endian)
    speech_seconds REAL NOT NULL DEFAULT 0,
    PRIMARY KEY (meeting_id, label),
    FOREIGN KEY (meeting_id) REFERENCES meetings(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS voice_profiles (
    name TEXT PRIMARY KEY NOT NULL,
    embedding BLOB NOT NULL,        -- mean voice embedding (f32 little-endian)
    model TEXT NOT NULL,            -- embedding model the vector comes from
    meetings INTEGER NOT NULL DEFAULT 1,
    updated_at TEXT NOT NULL
);
