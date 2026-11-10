-- «Спікери → Усі збережені»: what is done with each saved person.

-- Off: the voice is kept but no longer names anyone in new recordings
ALTER TABLE voice_profiles ADD COLUMN recognize INTEGER NOT NULL DEFAULT 1;

-- Names not offered in hints any more. forgotten = 1: the person was
-- forgotten and is not listed either; 0: only «Пропонувати ім'я» is off.
-- Naming someone so by hand again takes the name out of here.
CREATE TABLE IF NOT EXISTS hidden_speaker_names (
    name TEXT PRIMARY KEY NOT NULL,
    forgotten INTEGER NOT NULL DEFAULT 0,
    hidden_at TEXT NOT NULL
);

-- Recordings left out of a person's voice («Це не цей голос»)
CREATE TABLE IF NOT EXISTS voice_profile_exclusions (
    name TEXT NOT NULL,
    meeting_id TEXT NOT NULL,
    PRIMARY KEY (name, meeting_id)
);

-- The last forgotten people, kept for «Скасувати»
CREATE TABLE IF NOT EXISTS forgotten_speakers (
    batch TEXT NOT NULL,
    name TEXT NOT NULL,
    embedding BLOB,
    model TEXT,
    meetings INTEGER,
    updated_at TEXT,
    role TEXT,
    recognize INTEGER,
    hidden_before INTEGER NOT NULL DEFAULT 0,
    forgotten_before INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (batch, name)
);
