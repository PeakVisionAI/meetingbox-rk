CREATE TABLE IF NOT EXISTS speaker_contacts (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    note TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS meeting_speaker_labels (
    meeting_id TEXT NOT NULL,
    speaker_key TEXT NOT NULL,
    display_name TEXT NOT NULL,
    contact_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (meeting_id, speaker_key),
    FOREIGN KEY (meeting_id) REFERENCES meetings(id) ON DELETE CASCADE,
    FOREIGN KEY (contact_id) REFERENCES speaker_contacts(id) ON DELETE SET NULL
);

CREATE TABLE IF NOT EXISTS hotwords (
    id TEXT PRIMARY KEY NOT NULL,
    phrase TEXT NOT NULL,
    replacement TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_hotwords_phrase ON hotwords(phrase);

CREATE TABLE IF NOT EXISTS meeting_actions (
    id TEXT PRIMARY KEY NOT NULL,
    meeting_id TEXT NOT NULL,
    content TEXT NOT NULL,
    assignee TEXT,
    due_date TEXT,
    status TEXT NOT NULL DEFAULT 'open',
    source_transcript_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (meeting_id) REFERENCES meetings(id) ON DELETE CASCADE,
    FOREIGN KEY (source_transcript_id) REFERENCES transcripts(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_meeting_actions_meeting ON meeting_actions(meeting_id, created_at);
CREATE UNIQUE INDEX IF NOT EXISTS idx_meeting_actions_source ON meeting_actions(meeting_id, source_transcript_id) WHERE source_transcript_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_transcripts_speaker ON transcripts(meeting_id, speaker);
