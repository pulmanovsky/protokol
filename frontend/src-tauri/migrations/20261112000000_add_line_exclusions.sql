-- Lines the secretary left out of the document («Не включати в документ»),
-- kept by time so a new speaker recognition leaves them out again
CREATE TABLE IF NOT EXISTS segment_exclusions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    meeting_id TEXT NOT NULL,
    start REAL NOT NULL,
    end REAL NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_segment_exclusions_meeting ON segment_exclusions(meeting_id);
