//! Pure audio framing and utterance replacement rules for the optional preview.

pub(super) const SAMPLE_RATE: usize = 16_000;
pub(super) const CHUNK_SAMPLES: usize = SAMPLE_RATE * 600 / 1000;

pub(super) struct AudioChunk {
    pub samples: Vec<f32>,
    pub start_time: f64,
    pub end_time: f64,
    pub revision: u64,
}

pub(super) struct AudioBuffer {
    samples: Vec<f32>,
    start_time: f64,
    end_time: f64,
}

impl AudioBuffer {
    pub fn new() -> Self {
        Self {
            samples: Vec::with_capacity(CHUNK_SAMPLES),
            start_time: 0.0,
            end_time: 0.0,
        }
    }

    /// Append at most the space remaining in this 600 ms window.
    pub fn append(&mut self, samples: &[f32], timestamp: f64) -> usize {
        let count = samples.len().min(CHUNK_SAMPLES - self.samples.len());
        if count == 0 {
            return 0;
        }
        if self.samples.is_empty() {
            self.start_time = timestamp;
        }
        self.samples.extend_from_slice(&samples[..count]);
        self.end_time = timestamp + count as f64 / SAMPLE_RATE as f64;
        count
    }

    pub fn is_full(&self) -> bool {
        self.samples.len() == CHUNK_SAMPLES
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }

    pub fn take(&mut self, revision: u64) -> Option<AudioChunk> {
        if self.samples.is_empty() {
            return None;
        }
        Some(AudioChunk {
            samples: std::mem::replace(&mut self.samples, Vec::with_capacity(CHUNK_SAMPLES)),
            start_time: self.start_time,
            end_time: self.end_time,
            revision,
        })
    }
}

/// Mono, 16 kHz, signed PCM16 little-endian WAV, including a valid empty header.
pub(super) fn pcm16_wav(samples: &[f32]) -> Vec<u8> {
    let data_bytes = (samples.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_bytes as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE as u32).to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE as u32 * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_bytes.to_le_bytes());
    for &sample in samples {
        let sample = if sample.is_finite() { sample.clamp(-1.0, 1.0) } else { 0.0 };
        let pcm = (sample * if sample < 0.0 { 32768.0 } else { 32767.0 }) as i16;
        wav.extend_from_slice(&pcm.to_le_bytes());
    }
    wav
}

#[derive(Debug, PartialEq)]
pub(super) struct Preview {
    pub text: String,
    pub start_time: f64,
    pub end_time: f64,
    pub is_final: bool,
}

#[derive(Default)]
pub(super) struct Utterance {
    start_time: Option<f64>,
    end_time: f64,
    speech_observed: bool,
    text: String,
}

impl Utterance {
    /// Keep speech bounds even when the model returns text only after silence.
    pub fn observe_speech(&mut self, start: f64, end: f64) {
        self.start_time.get_or_insert(start);
        self.end_time = end;
        self.speech_observed = true;
    }

    /// The sidecar sends the whole current utterance, never a token delta.
    /// An empty final still commits the last visible hypothesis, if any.
    /// Text returned during silence must not extend the last speech boundary.
    pub fn update(&mut self, text: &str, start: f64, end: f64, is_final: bool) -> Option<Preview> {
        let text = text.trim();
        let changed = !text.is_empty() && text != self.text;
        if !text.is_empty() {
            self.start_time.get_or_insert(start);
            // Without a VAD observation, text is the only timing evidence. Keep
            // the last partial's end on final; final-only text uses this window.
            if !self.speech_observed && (!is_final || self.text.is_empty()) {
                self.end_time = end;
            }
            self.text = text.to_owned();
        }
        let preview = if !self.text.is_empty() && (changed || is_final) {
            Some(Preview {
                text: self.text.clone(),
                start_time: self.start_time.unwrap_or(start),
                end_time: self.end_time,
                is_final,
            })
        } else {
            None
        };
        if is_final {
            *self = Self::default();
        }
        preview
    }

    /// An empty non-final replacement tells the UI to discard a broken preview.
    pub fn discard(&mut self) -> Option<Preview> {
        let preview = self.start_time.map(|start| Preview {
            text: String::new(),
            start_time: start,
            end_time: self.end_time,
            is_final: false,
        });
        *self = Self::default();
        preview
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twelve_mix_windows_include_silence_and_keep_capture_times() {
        let mut buffer = AudioBuffer::new();
        for window in 0..12 {
            assert_eq!(buffer.append(&[0.0; 800], 5.0 + window as f64 * 0.05), 800);
            assert_eq!(buffer.is_full(), window == 11);
        }
        let chunk = buffer.take(7).unwrap();
        assert_eq!(chunk.samples.len(), 9600);
        assert_eq!(chunk.start_time, 5.0);
        assert!((chunk.end_time - 5.6).abs() < 1e-9);
        assert_eq!(chunk.revision, 7);
        assert!(buffer.take(7).is_none());
    }

    #[test]
    fn partial_stop_window_and_overflow_input_keep_their_boundaries() {
        let mut buffer = AudioBuffer::new();
        assert_eq!(buffer.append(&vec![0.1; 10_000], 2.0), 9600);
        assert_eq!(buffer.take(1).unwrap().samples.len(), 9600);
        buffer.append(&[0.0; 400], 2.6);
        let tail = buffer.take(1).unwrap();
        assert!((tail.end_time - 2.625).abs() < 1e-9);
        buffer.append(&[0.5; 800], 3.0);
        buffer.clear();
        assert!(buffer.take(2).is_none());
    }

    #[test]
    fn wav_has_pcm16_header_and_clips_without_wrapping() {
        let wav = pcm16_wav(&[-2.0, -1.0, 0.0, 1.0, 2.0, f32::NAN]);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16000);
        assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16);
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 12);
        let pcm: Vec<_> = wav[44..].chunks_exact(2)
            .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]])).collect();
        assert_eq!(pcm, [-32768, -32768, 0, 32767, 32767, 0]);
        assert_eq!(pcm16_wav(&[]).len(), 44);
    }

    #[test]
    fn cumulative_text_replaces_and_silence_final_starts_a_new_utterance() {
        let mut utterance = Utterance::default();
        assert_eq!(utterance.update("hello", 1.0, 1.6, false).unwrap().text, "hello");
        let replacement = utterance.update("hello world", 1.6, 2.2, false).unwrap();
        assert_eq!(replacement.text, "hello world");
        assert_eq!(replacement.start_time, 1.0);
        // vad.speech=false must not prevent the final response's text being used.
        let final_preview = utterance.update("hello world!", 2.2, 2.8, true).unwrap();
        assert!(final_preview.is_final);
        assert_eq!(final_preview.text, "hello world!");
        assert_eq!(final_preview.end_time, 2.2);
        assert_eq!(utterance.update("next", 3.0, 3.6, false).unwrap().start_time, 3.0);
    }

    #[test]
    fn first_speech_time_survives_model_latency() {
        let mut utterance = Utterance::default();
        utterance.observe_speech(4.0, 4.6);
        assert!(utterance.update("", 4.0, 4.6, false).is_none());
        let preview = utterance.update("delayed", 4.6, 5.2, false).unwrap();
        assert_eq!(preview.start_time, 4.0);
        assert_eq!(preview.end_time, 4.6);
        // Text can arrive on a later silent window; only VAD advances bounds.
        utterance.observe_speech(5.2, 5.8);
        let next = utterance.update("delayed words", 5.2, 5.8, false).unwrap();
        assert_eq!(next.end_time, 5.8);
    }

    #[test]
    fn silence_final_and_repeated_silence_do_not_extend_speech_coverage() {
        let mut utterance = Utterance::default();
        utterance.observe_speech(1.0, 1.6);
        utterance.update("partial", 1.0, 1.6, false);
        utterance.observe_speech(1.6, 2.2);
        utterance.update("partial words", 1.6, 2.2, false);
        assert!(utterance.update("partial words", 2.2, 2.8, false).is_none());
        let corrected = utterance.update("corrected words", 2.8, 3.4, false).unwrap();
        assert_eq!(corrected.end_time, 2.2);
        let final_preview = utterance.update("corrected words!", 3.4, 4.0, true).unwrap();
        assert_eq!(final_preview.start_time, 1.0);
        assert_eq!(final_preview.end_time, 2.2);
        assert!(final_preview.is_final);
        assert!(utterance.update("", 4.0, 4.6, true).is_none());
        utterance.observe_speech(5.0, 5.6);
        let next = utterance.update("next", 5.0, 5.6, false).unwrap();
        assert_eq!(next.start_time, 5.0);
        assert_eq!(next.end_time, 5.6);
    }

    #[test]
    fn first_text_on_silence_final_uses_observed_speech_bounds() {
        let mut utterance = Utterance::default();
        utterance.observe_speech(7.0, 7.6);
        assert!(utterance.update("", 7.0, 7.6, false).is_none());
        let final_preview = utterance.update("late final", 7.6, 8.2, true).unwrap();
        assert_eq!(final_preview.start_time, 7.0);
        assert_eq!(final_preview.end_time, 7.6);
        assert_eq!(final_preview.text, "late final");
        assert!(final_preview.is_final);
    }

    #[test]
    fn explicit_silence_flush_preserves_speech_end_with_or_without_final_text() {
        for final_text in ["", "final correction"] {
            let mut utterance = Utterance::default();
            utterance.observe_speech(2.0, 2.6);
            utterance.update("pending", 2.0, 2.6, false);
            let final_preview = utterance.update(final_text, 3.2, 3.2, true).unwrap();
            assert_eq!(final_preview.end_time, 2.6);
            assert!(final_preview.is_final);
        }
    }

    #[test]
    fn voiced_final_advances_bounds_and_final_only_text_has_a_fallback() {
        let mut utterance = Utterance::default();
        utterance.observe_speech(1.0, 1.6);
        utterance.update("partial", 1.0, 1.6, false);
        utterance.observe_speech(1.6, 2.2);
        let voiced_final = utterance.update("final", 1.6, 2.2, true).unwrap();
        assert_eq!(voiced_final.end_time, 2.2);
        let fallback = utterance.update("final only", 3.0, 3.6, true).unwrap();
        assert_eq!(fallback.start_time, 3.0);
        assert_eq!(fallback.end_time, 3.6);
    }

    #[test]
    fn empty_flush_finalizes_visible_text_and_fault_discards_it() {
        let mut utterance = Utterance::default();
        utterance.update("pending", 0.0, 0.6, false);
        let final_preview = utterance.update("", 0.6, 0.6, true).unwrap();
        assert_eq!(final_preview.text, "pending");
        assert!(final_preview.is_final);
        assert!(utterance.update("", 1.0, 1.6, true).is_none());
        utterance.update("broken", 2.0, 2.6, false);
        let clear = utterance.discard().unwrap();
        assert_eq!(clear.text, "");
        assert!(!clear.is_final);
        assert!(utterance.discard().is_none());
    }
}
