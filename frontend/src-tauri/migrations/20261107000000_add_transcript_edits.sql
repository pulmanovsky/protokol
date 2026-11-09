-- Lines the user corrected by hand keep the recognised text (to undo)
ALTER TABLE transcripts ADD COLUMN original_transcript TEXT;
