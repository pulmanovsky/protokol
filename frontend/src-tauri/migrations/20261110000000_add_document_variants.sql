-- One document per template for a meeting: switching templates keeps each
-- version; source_hash tells whether the transcript changed since
CREATE TABLE IF NOT EXISTS document_variants (
    meeting_id TEXT NOT NULL,
    template_id TEXT NOT NULL,
    result TEXT NOT NULL,
    source_hash TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (meeting_id, template_id)
);

-- The template of the document shown now
ALTER TABLE summary_processes ADD COLUMN template_id TEXT;
