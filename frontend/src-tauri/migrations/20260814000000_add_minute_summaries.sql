CREATE TABLE IF NOT EXISTS meeting_minute_summaries (
    meeting_id TEXT NOT NULL,
    minute_index INTEGER NOT NULL,
    start_seconds INTEGER NOT NULL,
    end_seconds INTEGER NOT NULL,
    summary TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (meeting_id, minute_index),
    FOREIGN KEY (meeting_id) REFERENCES meetings(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_minute_summaries_meeting
    ON meeting_minute_summaries(meeting_id, minute_index);
