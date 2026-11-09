-- Lines split or joined by hand, kept by time so a new speaker recognition
-- does the same again; lines the user checked («перевірено»)
CREATE TABLE IF NOT EXISTS segment_edits (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    meeting_id TEXT NOT NULL,
    kind TEXT NOT NULL,          -- 'split' | 'merge'
    at REAL NOT NULL,            -- the boundary, seconds
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_segment_edits_meeting ON segment_edits(meeting_id);
CREATE TABLE IF NOT EXISTS segment_reviews (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    meeting_id TEXT NOT NULL,
    start REAL NOT NULL,
    end REAL NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_segment_reviews_meeting ON segment_reviews(meeting_id);
ALTER TABLE transcripts ADD COLUMN reviewed_at TEXT;
