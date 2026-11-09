-- What the user changed in a meeting's transcript, to undo and redo it:
-- the rows and the meeting's side records before and after each change
CREATE TABLE IF NOT EXISTS edit_journal (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    meeting_id TEXT NOT NULL,
    label TEXT NOT NULL,
    before_state TEXT NOT NULL,
    after_state TEXT NOT NULL,
    undone INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_edit_journal_meeting ON edit_journal(meeting_id, id);
