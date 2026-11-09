-- Optional role of a voice («керівник», «керівник групи»): meeting types
-- can use it to find the parts of a meeting.
ALTER TABLE voice_profiles ADD COLUMN role TEXT;
ALTER TABLE meeting_speakers ADD COLUMN role TEXT;
