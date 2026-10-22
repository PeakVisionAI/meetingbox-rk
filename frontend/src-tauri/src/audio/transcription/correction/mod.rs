mod archive;
mod client;
pub mod validation;

use archive::Archive;
use client::{CorrectionClient, RequestError};
use std::{collections::VecDeque, path::PathBuf, time::{Duration, Instant}};

pub struct CorrectionConfig {
    endpoint: String,
    model: String,
    timeout: Duration,
    enabled: bool,
}

impl Default for CorrectionConfig {
    fn default() -> Self {
        use crate::database::repositories::setting::{RK_SUMMARY_ENDPOINT, RK_SUMMARY_MODEL};
        let enabled = match std::env::var("MEETINGBOX_ASR_REWRITE").as_deref() {
            Ok("0") => false,
            Ok("1") | Err(_) => true,
            _ => { log::warn!("Invalid MEETINGBOX_ASR_REWRITE; correction disabled"); false }
        };
        Self { endpoint: RK_SUMMARY_ENDPOINT.into(), model: RK_SUMMARY_MODEL.into(),
            timeout: Duration::from_millis(1500), enabled }
    }
}

impl CorrectionConfig {
    /// A local endpoint override for isolated protocol tests and synthetic smoke checks.
    #[cfg(test)]
    pub fn local_for_test(endpoint: String, timeout: Duration) -> Self {
        Self { endpoint, timeout, ..Self::default() }
    }
}

pub struct Segment<'a> {
    pub sequence_id: u64,
    pub text: &'a str,
    pub audio_start_time: f64,
    pub audio_end_time: f64,
    pub timestamp: &'a str,
}

impl<'a> Segment<'a> {
    pub fn new(sequence_id: u64, text: &'a str, audio_start_time: f64,
        audio_end_time: f64, timestamp: &'a str) -> Self {
        Self { sequence_id, text, audio_start_time, audio_end_time, timestamp }
    }
}

pub struct Corrector {
    client: Option<CorrectionClient>,
    archive: Option<Archive>,
    config: CorrectionConfig,
    session: String,
    disabled_reason: Option<&'static str>,
    context: VecDeque<String>,
}

impl Corrector {
    pub fn new(session: String, folder: Option<PathBuf>, config: CorrectionConfig) -> Self {
        let client = CorrectionClient::new(&config.endpoint, &config.model).ok();
        let disabled_reason = if !config.enabled { Some("configured_disabled") }
            else if folder.is_none() { Some("missing_archive_folder") }
            else if client.is_none() { Some("client_initialization_failed") }
            else { None };
        Self {
            archive: folder.map(|folder| Archive::new(folder, session.clone())),
            client, config, session, disabled_reason, context: VecDeque::new(),
        }
    }

    #[cfg(test)]
    pub fn is_enabled(&self) -> bool { self.disabled_reason.is_none() }

    fn disable(&mut self, reason: &'static str) {
        if self.disabled_reason.is_none() {
            log::warn!("ASR rewrite disabled session={} reason={}", self.session, reason);
            self.disabled_reason = Some(reason);
        }
    }

    fn remember(&mut self, text: &str) {
        let tail: String = text.chars().rev().take(200).collect::<Vec<_>>()
            .into_iter().rev().collect();
        self.context.push_back(tail);
        while self.context.len() > 2 { self.context.pop_front(); }
    }

    pub async fn correct(&mut self, segment: Segment<'_>, pending_chunks: u64) -> String {
        let original = segment.text.to_string();
        let started = Instant::now();
        // The journal is also written when correction is disabled. Failure never changes text.
        let Some(archive) = self.archive.as_mut() else {
            self.remember(&original);
            return original;
        };
        if archive.append(&segment, 0, &original, "original", None, 0, &self.config.model).await.is_err() {
            log::warn!("ASR rewrite original archive failed session={} sequence={}", self.session, segment.sequence_id);
            self.disable("archive_failed");
            self.archive = None;
            self.remember(&original);
            return original;
        }
        // Avoid adding LLM latency when ASR already has work waiting.
        if pending_chunks > 0 { self.disable("asr_backlog"); }
        let (text, decision, reason): (String, &str, Option<String>) =
            if let Some(reason) = self.disabled_reason {
                (original.clone(), "disabled", Some(reason.into()))
            } else if original.chars().count() > 512 {
                (original.clone(), "rejected", Some("input_too_long".into()))
            } else {
                let context: Vec<_> = self.context.iter().cloned().collect();
                match tokio::time::timeout(self.config.timeout,
                    self.client.as_ref().unwrap().request(&original, &context)).await {
                    Err(_) => {
                        self.disable("timeout");
                        (original.clone(), "timeout", Some("timeout".into()))
                    }
                    Ok(Err(RequestError::Unavailable)) => {
                        self.disable("service_unavailable");
                        (original.clone(), "disabled", Some("service_unavailable".into()))
                    }
                    Ok(Err(RequestError::Invalid(reason))) =>
                        (original.clone(), "rejected", Some(reason.into())),
                    Ok(Ok(candidate)) => match validation::validate_correction(&original, &candidate) {
                        Err(reason) => (original.clone(), "rejected", Some(reason)),
                        Ok(()) => {
                            let decision = if candidate == original { "unchanged" } else { "changed" };
                            (candidate, decision, None)
                        }
                    }
                }
            };
        let archived = self.archive.as_mut().unwrap().append(&segment, 1, &text, decision,
            reason.as_deref(), started.elapsed().as_millis(), &self.config.model).await;
        self.remember(&original);
        if archived.is_err() {
            log::warn!("ASR rewrite decision archive failed session={} sequence={}", self.session, segment.sequence_id);
            self.disable("archive_failed");
            // A failed append may leave a partial line; never append behind it again.
            self.archive = None;
            return original;
        }
        log::debug!("ASR rewrite session={} sequence={} decision={} elapsed_ms={}",
            self.session, segment.sequence_id, decision, started.elapsed().as_millis());
        text
    }
}
