use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

use super::validation::{build_user_prompt, parse_correction, SYSTEM_PROMPT};

const MAX_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub(crate) enum RequestError {
    Unavailable,
    Invalid(&'static str),
}

pub(crate) struct CorrectionClient {
    client: Client,
    endpoint: String,
    model: String,
}

#[derive(Deserialize)]
struct Envelope {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct Message {
    content: String,
}

impl CorrectionClient {
    pub fn new(endpoint: &str, model: &str) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: Client::builder()
                .connect_timeout(Duration::from_millis(500))
                .timeout(Duration::from_millis(1500))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            endpoint: format!("{}/chat/completions", endpoint.trim_end_matches('/')),
            model: model.into(),
        })
    }

    pub async fn request(&self, target: &str, context: &[String]) -> Result<String, RequestError> {
        let token_budget = (target.chars().count() * 2 + 64).clamp(128, 1024);
        let mut response = self.client.post(&self.endpoint)
            .json(&serde_json::json!({
                "model": self.model,
                "messages": [
                    {"role": "system", "content": SYSTEM_PROMPT},
                    {"role": "user", "content": build_user_prompt(target, context)}
                ],
                "max_tokens": token_budget,
                "stream": false
            }))
            .send().await.map_err(|_| RequestError::Unavailable)?;
        if !response.status().is_success() {
            return Err(RequestError::Unavailable);
        }
        if response.content_length().is_some_and(|n| n > MAX_RESPONSE_BYTES as u64) {
            return Err(RequestError::Invalid("response_too_large"));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| RequestError::Unavailable)? {
            if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(RequestError::Invalid("response_too_large"));
            }
            body.extend_from_slice(&chunk);
        }
        let envelope: Envelope = serde_json::from_slice(&body)
            .map_err(|_| RequestError::Invalid("invalid_envelope"))?;
        if envelope.choices.len() != 1 {
            return Err(RequestError::Invalid("ambiguous_choices"));
        }
        let choice = &envelope.choices[0];
        if choice.finish_reason.as_deref().is_some_and(|reason| reason != "stop") {
            return Err(RequestError::Invalid("incomplete_response"));
        }
        parse_correction(&choice.message.content)
            .map_err(|_| RequestError::Invalid("invalid_correction_json"))
    }
}
