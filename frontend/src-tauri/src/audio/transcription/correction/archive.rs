use serde::Serialize;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;

use super::Segment;

pub(crate) struct Archive {
    path: PathBuf,
    file: Option<tokio::fs::File>,
    session: String,
}

#[derive(Serialize)]
struct Record<'a> {
    schema_version: u8,
    recording_session: &'a str,
    sequence_id: u64,
    revision: u8,
    original_reference: String,
    original_text: &'a str,
    corrected_text: &'a str,
    audio_start_time: f64,
    audio_end_time: f64,
    timestamp: &'a str,
    recorded_at: String,
    model: &'a str,
    prompt_version: &'static str,
    decision: &'a str,
    reason: Option<&'a str>,
    elapsed_ms: u128,
    input_chars: usize,
    output_chars: usize,
}

impl Archive {
    pub fn new(folder: PathBuf, session: String) -> Self {
        Self { path: folder.join("transcript_corrections.jsonl"), file: None, session }
    }

    pub async fn append(
        &mut self,
        segment: &Segment<'_>,
        revision: u8,
        text: &str,
        decision: &str,
        reason: Option<&str>,
        elapsed_ms: u128,
        model: &str,
    ) -> std::io::Result<()> {
        if self.file.is_none() {
            self.file = Some(tokio::fs::OpenOptions::new()
                .create(true).append(true).open(&self.path).await?);
        }
        let record = Record {
            schema_version: 1,
            recording_session: &self.session,
            sequence_id: segment.sequence_id,
            revision,
            original_reference: format!("{}:{}:0", self.session, segment.sequence_id),
            original_text: segment.text,
            corrected_text: text,
            audio_start_time: segment.audio_start_time,
            audio_end_time: segment.audio_end_time,
            timestamp: segment.timestamp,
            recorded_at: chrono::Utc::now().to_rfc3339(),
            model,
            prompt_version: "asr-correction-v1",
            decision,
            reason,
            elapsed_ms,
            input_chars: segment.text.chars().count(),
            output_chars: text.chars().count(),
        };
        let mut bytes = serde_json::to_vec(&record)?;
        bytes.push(b'\n');
        let file = self.file.as_mut().unwrap();
        file.write_all(&bytes).await?;
        file.flush().await?;
        file.sync_data().await
    }
}
