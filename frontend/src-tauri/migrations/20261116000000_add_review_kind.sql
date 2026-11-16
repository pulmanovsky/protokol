-- How a line came to count as checked: 'read' on screen, 'heard' (listened
-- through), 'edited' (fixed or a word in it confirmed) or 'marked' by hand.
-- The measurements and the training trust only these. 'all' is what
-- «Позначити все перевіреним» (removed in 0.4.1) left on lines nobody read.
ALTER TABLE transcripts ADD COLUMN review_kind TEXT;
ALTER TABLE segment_reviews ADD COLUMN kind TEXT;

-- The moments of «Позначити все перевіреним» before: each press gave every
-- line it marked one and the same moment. Found in the history of changes
-- where it is still kept, and as many checked spans made at one moment
-- (reading marks a line at a time). Lines alone sharing a moment are not
-- enough: a new recognition cuts one checked line into several.
CREATE TEMP TABLE bulk_marks AS
  SELECT DISTINCT j.meeting_id AS meeting_id, json_extract(l.value, '$.reviewed_at') AS at
  FROM edit_journal j, json_each(CASE WHEN json_valid(j.after_state) THEN j.after_state ELSE '{}' END, '$.lines') l
  WHERE j.label = 'усе позначено перевіреним' AND json_extract(l.value, '$.reviewed_at') IS NOT NULL;
INSERT INTO bulk_marks
  SELECT meeting_id, created_at FROM segment_reviews
  GROUP BY meeting_id, created_at HAVING COUNT(*) >= 3;

UPDATE transcripts SET review_kind = 'all'
WHERE reviewed_at IS NOT NULL AND (meeting_id, reviewed_at) IN (SELECT meeting_id, at FROM bulk_marks);
UPDATE transcripts
SET review_kind = CASE WHEN original_transcript IS NOT NULL AND original_transcript <> transcript THEN 'edited' ELSE 'read' END
WHERE reviewed_at IS NOT NULL AND review_kind IS NULL;

UPDATE segment_reviews SET kind = 'all' WHERE (meeting_id, created_at) IN (SELECT meeting_id, at FROM bulk_marks);
UPDATE segment_reviews SET kind = 'read' WHERE kind IS NULL;

DROP TABLE bulk_marks;
