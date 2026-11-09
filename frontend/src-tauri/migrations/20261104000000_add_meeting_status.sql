-- Protocol review status: 'draft' (Чернетка) until a person approves it ('approved', Затверджено)
ALTER TABLE meetings ADD COLUMN status TEXT NOT NULL DEFAULT 'draft';
