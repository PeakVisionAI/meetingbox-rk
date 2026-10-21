CREATE TABLE IF NOT EXISTS transcript_correction_history (
    id TEXT PRIMARY KEY NOT NULL,
    transcript_id TEXT NOT NULL,
    meeting_id TEXT NOT NULL,
    original_text TEXT NOT NULL,
    corrected_text TEXT NOT NULL,
    original_speaker TEXT,
    corrected_speaker TEXT,
    created_at TEXT NOT NULL,
    FOREIGN KEY (transcript_id) REFERENCES transcripts(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_transcript_correction_history_transcript
ON transcript_correction_history(transcript_id, created_at);
