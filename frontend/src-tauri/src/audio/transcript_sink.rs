use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::{Arc, Mutex}, time::{Duration, Instant}};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub id: String,
    pub text: String,
    pub audio_start_time: f64,
    pub audio_end_time: f64,
    pub duration: f64,
    pub display_time: String,
    pub confidence: f32,
    pub sequence_id: u64,
}

/// Session-owned persistence survives moving the recording manager during shutdown.
#[derive(Clone)]
pub struct TranscriptSink {
    state: Arc<Mutex<State>>,
}

struct State {
    folder: Option<PathBuf>,
    segments: Vec<TranscriptSegment>,
    last_write: Option<Instant>,
}

impl TranscriptSink {
    pub fn new(folder: Option<PathBuf>) -> Self {
        Self { state: Arc::new(Mutex::new(State { folder, segments: Vec::new(), last_write: None })) }
    }

    pub fn set_folder(&self, folder: PathBuf) -> Result<()> {
        self.state.lock().map_err(|_| anyhow!("Transcript sink lock poisoned"))?.folder = Some(folder);
        Ok(())
    }

    pub fn upsert(&self, segment: TranscriptSegment) -> Result<()> {
        let mut state = self.state.lock().map_err(|_| anyhow!("Transcript sink lock poisoned"))?;
        if let Some(existing) = state.segments.iter_mut().find(|s| s.sequence_id == segment.sequence_id) {
            *existing = segment;
        } else {
            state.segments.push(segment);
        }
        if state.last_write.map_or(true, |last| last.elapsed() >= Duration::from_secs(5)) {
            Self::write(&mut state)?;
        }
        Ok(())
    }

    pub fn flush(&self) -> Result<()> {
        let mut state = self.state.lock().map_err(|_| anyhow!("Transcript sink lock poisoned"))?;
        Self::write(&mut state)
    }

    pub fn snapshot(&self) -> Vec<TranscriptSegment> {
        self.state.lock().map(|state| state.segments.clone()).unwrap_or_default()
    }

    pub fn clear(&self) {
        if let Ok(mut state) = self.state.lock() { state.segments.clear(); }
    }

    fn write(state: &mut State) -> Result<()> {
        if let Some(folder) = &state.folder {
            let bytes = serde_json::to_vec_pretty(&serde_json::json!({
                "version": "1.0",
                "segments": state.segments,
                "last_updated": chrono::Utc::now().to_rfc3339(),
                "total_segments": state.segments.len()
            }))?;
            let temp = folder.join(".transcripts.json.tmp");
            std::fs::write(&temp, bytes)?;
            std::fs::rename(temp, folder.join("transcripts.json"))?;
            state.last_write = Some(Instant::now());
        }
        Ok(())
    }
}
