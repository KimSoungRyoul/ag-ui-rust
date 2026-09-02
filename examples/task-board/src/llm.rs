//! Optional: let a model phrase the reply.
//!
//! Off unless `serve --llm` is asked for *and* a key is in the environment.
//! The board itself never goes near the model — ids, counts and state
//! transitions are computed in [`crate::agent`] and the model is handed the
//! finished sentence to rewrite. That is deliberate: a dogfood app whose
//! assertions depend on a model is a dogfood app that cannot be tested.
//!
//! Same wire format and the same environment variables as `e2e/src/llm.rs`, and
//! the same point: an OpenAI-compatible `/chat/completions` endpoint reached
//! with `reqwest` and two `serde` structs. There is no LLM crate here either.

use std::fmt;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

/// The environment variable holding the API key.
pub const API_KEY_ENV: &str = "AG_UI_LLM_API_KEY";
/// Read when [`API_KEY_ENV`] is unset — the default endpoint is Gemini's.
pub const FALLBACK_API_KEY_ENV: &str = "GEMINI_API_KEY";
/// The environment variable holding the base URL.
pub const BASE_URL_ENV: &str = "AG_UI_LLM_BASE_URL";
/// The environment variable holding the model id.
pub const MODEL_ENV: &str = "AG_UI_LLM_MODEL";
/// Qwen Cloud's OpenAI-compatible mode, read when [`BASE_URL_ENV`] is unset:
/// the base URL, its key, and its model, in that order.
pub const QWEN_BASE_URL_ENV: &str = "QWEN_BASE_URL";
/// The key that goes with [`QWEN_BASE_URL_ENV`].
pub const QWEN_API_KEY_ENV: &str = "QWEN_API_KEY";
/// The model that goes with [`QWEN_BASE_URL_ENV`], when [`MODEL_ENV`] is unset.
pub const QWEN_MODEL_ENV: &str = "QWEN_MODEL";
/// The Qwen model used when [`QWEN_MODEL_ENV`] is unset. Pinned, like the default.
pub const QWEN_DEFAULT_MODEL: &str = "qwen-plus";

/// Where requests go unless [`BASE_URL_ENV`] says otherwise.
pub const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/openai";

/// The model this talks to by default. Pinned, never a `*-latest` alias.
pub const DEFAULT_MODEL: &str = "gemini-2.5-flash-lite";

/// How the model is told to behave. Short on purpose: it is rewriting one
/// sentence, not running the board.
const SYSTEM: &str = "You are a workshop assistant reporting on a task board. \
Rewrite the given answer as one friendly sentence. Keep every id, number and \
task title exactly as they appear. Do not add tasks, invent state, or ask \
questions. Reply with the sentence and nothing else.";

/// [`Voice::from_env`] found no API key, and the endpoint needs one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MissingApiKey;

impl fmt::Display for MissingApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "neither {API_KEY_ENV} nor {FALLBACK_API_KEY_ENV} is set, and {DEFAULT_BASE_URL} needs a key \
             (set {BASE_URL_ENV} to a local server such as http://localhost:11434/v1 to run without one)"
        )
    }
}

impl std::error::Error for MissingApiKey {}

/// A model that rephrases the agent's replies.
pub struct Voice {
    client: reqwest::Client,
    base_url: String,
    model: String,
    /// Absent for a local server that wants no credential.
    api_key: Option<String>,
}

// Hand-written so the key cannot reach a log through `{:?}`.
impl fmt::Debug for Voice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Voice")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl Voice {
    /// Reads the endpoint, model and key from the environment.
    ///
    /// A custom [`BASE_URL_ENV`] is taken to mean a server the caller runs, so
    /// a missing key there is not an error.
    ///
    /// # Errors
    ///
    /// [`MissingApiKey`] when the default endpoint would be used without one.
    pub fn from_env() -> Result<Self, MissingApiKey> {
        // AG_UI_LLM_BASE_URL wins outright; QWEN_BASE_URL picks Qwen Cloud with
        // its own key and model; the default endpoint needs a key.
        let generic_key = var(API_KEY_ENV);
        let (base_url, model, api_key) = match (var(BASE_URL_ENV), var(QWEN_BASE_URL_ENV)) {
            (Some(base_url), _) => (
                base_url,
                var(MODEL_ENV).unwrap_or_else(|| DEFAULT_MODEL.to_owned()),
                generic_key
                    .or_else(|| var(FALLBACK_API_KEY_ENV))
                    .or_else(|| var(QWEN_API_KEY_ENV)),
            ),
            (None, Some(base_url)) => {
                let api_key = generic_key.or_else(|| var(QWEN_API_KEY_ENV));
                if api_key.is_none() {
                    return Err(MissingApiKey);
                }
                (
                    base_url,
                    var(MODEL_ENV)
                        .or_else(|| var(QWEN_MODEL_ENV))
                        .unwrap_or_else(|| QWEN_DEFAULT_MODEL.to_owned()),
                    api_key,
                )
            }
            (None, None) => {
                let api_key = generic_key.or_else(|| var(FALLBACK_API_KEY_ENV));
                if api_key.is_none() {
                    return Err(MissingApiKey);
                }
                (
                    DEFAULT_BASE_URL.to_owned(),
                    var(MODEL_ENV).unwrap_or_else(|| DEFAULT_MODEL.to_owned()),
                    api_key,
                )
            }
        };
        let base_url = base_url.trim_end_matches('/').to_owned();
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            base_url,
            model,
            api_key,
        })
    }

    /// The endpoint this is pointed at. Carries no credential — the key is a
    /// header.
    pub fn endpoint(&self) -> &str {
        &self.base_url
    }

    /// The model this is pointed at.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Rewrites `scripted` in the model's own words.
    ///
    /// The error is a plain string because the only caller formats it into a
    /// `REASONING_*` block and carries on with the scripted sentence — there is
    /// nothing to match on.
    pub async fn phrase(&self, said: &str, scripted: &str) -> Result<String, String> {
        let body = json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": SYSTEM},
                {"role": "user", "content": format!(
                    "The user typed: {said}\nThe board's answer: {scripted}"
                )},
            ],
            "temperature": 0,
        });

        let mut request = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .json(&body);
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }

        let response = request.send().await.map_err(|error| error.to_string())?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(format!("HTTP {}: {}", status.as_u16(), body.trim()));
        }

        let completion: Completion = response.json().await.map_err(|error| error.to_string())?;
        completion
            .choices
            .into_iter()
            .next()
            .map(|choice| choice.message.content.trim().to_owned())
            .ok_or_else(|| "the model returned no choices".to_owned())
    }
}

/// A set, non-blank environment variable.
fn var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Only the two fields this needs; everything else on the response is ignored.
#[derive(Debug, Deserialize)]
struct Completion {
    #[serde(default)]
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ChatMessage,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    #[serde(default)]
    content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_debug_rendering_carries_no_key() {
        let voice = Voice {
            client: reqwest::Client::new(),
            base_url: "http://localhost:11434/v1".to_owned(),
            model: "qwen3:4b".to_owned(),
            api_key: Some("sk-do-not-print-me".to_owned()),
        };
        let rendered = format!("{voice:?}");
        assert!(!rendered.contains("sk-do-not-print-me"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
    }

    #[test]
    fn a_response_with_no_choices_is_not_a_panic() {
        let completion: Completion = serde_json::from_str("{}").expect("an empty object");
        assert!(completion.choices.is_empty());
    }
}
