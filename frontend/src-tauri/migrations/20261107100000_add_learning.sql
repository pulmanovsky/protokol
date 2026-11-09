-- What the app wrote and what the secretary approved, side by side
CREATE TABLE IF NOT EXISTS protocol_revisions (
    meeting_id TEXT PRIMARY KEY NOT NULL,
    meeting_type TEXT NOT NULL,
    draft_md TEXT NOT NULL,
    final_md TEXT,
    generated_at TEXT NOT NULL,
    approved_at TEXT
);

-- A turn and how the secretary wrote it up: examples for the next meetings of that type
CREATE TABLE IF NOT EXISTS learned_examples (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    meeting_id TEXT NOT NULL,
    meeting_type TEXT NOT NULL,
    speaker TEXT NOT NULL,
    role TEXT,
    turn_text TEXT NOT NULL,
    approved TEXT NOT NULL,
    edits TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_learned_examples_type ON learned_examples(meeting_type);
