-- Lines the user gave to another speaker by hand: kept by time, so a new
-- speaker recognition applies them again
CREATE TABLE IF NOT EXISTS speaker_overrides (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    meeting_id TEXT NOT NULL,
    start REAL NOT NULL,
    end REAL NOT NULL,
    speaker TEXT NOT NULL,
    previous TEXT,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_speaker_overrides_meeting ON speaker_overrides(meeting_id);
ALTER TABLE transcripts ADD COLUMN speaker_manual INTEGER NOT NULL DEFAULT 0;
