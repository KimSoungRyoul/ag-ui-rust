//! An [`Agent`] backed by any OpenAI-compatible `/chat/completions` endpoint.
//!
//! # Why this exists
//!
//! Two reasons, and the second matters more.
//!
//! It proves the protocol plumbing survives a real streaming model rather than
//! a fixture. And it is the **architecture test**: `docs/DESIGN.md` claims
//! [`Agent`] *is* the LLM boundary and that no crate in this workspace depends
//! on a model library. This agent reaches the model with `reqwest`, a handful of
//! `serde` structs and nothing else, and implements nothing but [`Agent`]. That
//! it compiles and streams is what turns that claim into evidence — so keep
//! `rig`, `async-openai` and friends out of it.
//!
//! # Why the OpenAI wire format rather than a vendor's own
//!
//! This used to speak Gemini's native `:streamGenerateContent` dialect, and
//! being bound to one vendor cost real time: the free tier ran out, the harness
//! fell back to a sibling model, the sibling was a 3.x model that requires
//! `thoughtSignature` echoed back in tool loops, and the run died on
//! `HTTP 400: Function call is missing a thought_signature in functionCall
//! parts.` — a failure that was invisible until the fallback fired.
//!
//! `/chat/completions` is the one shape nearly everything serves: Gemini's
//! compatibility endpoint, Ollama, llama.cpp, LM Studio, vLLM, Groq, Together.
//! Pointing this agent at a different provider is now an env var, thought
//! signatures are the compatibility layer's problem rather than ours, and the
//! whole vendor-schema translation this file used to carry is gone — the
//! request takes an AG-UI [`Tool`]'s JSON Schema through unchanged.
//!
//! # The mapping, and the parts of it that bite
//!
//! `docs/QA.md` records the whole mapping. The awkward corners, all handled
//! below and all covered by the tests at the bottom of this file:
//!
//! - Tool-call arguments arrive as **partial JSON accumulated across frames**,
//!   keyed by `tool_calls[].index`. A fragment can split anywhere, including
//!   mid-string and between a backslash and the character it escapes, so
//!   nothing may parse a fragment on its own.
//! - **`tool_calls[].index` is not always there.** The spec says it is, and
//!   OpenAI, Ollama and Groq send it — but Gemini's compatibility endpoint
//!   omits it entirely and puts two parallel calls in one frame, distinguished
//!   only by `id`. Keying on `index` alone merges parallel calls into JSON
//!   soup, so `Calls` falls back to `id`, then to array position.
//! - The stream ends at a **`data: [DONE]` sentinel**, unlike the native API,
//!   which just EOFs.
//! - `finish_reason` may arrive on a frame carrying no content, which must not
//!   become an empty `TEXT_MESSAGE_CONTENT`.
//! - `tool_calls[].id` comes from the server. It is used as the AG-UI
//!   `toolCallId` as-is; one is synthesized only for a server that sends none.
//! - Line terminators differ **between endpoints of the same vendor**: Gemini's
//!   native SSE frames end `\r\n\r\n` and its OpenAI-compatible ones end
//!   `\n\n`. Both are accepted — see `take_block`.

use std::fmt;
use std::time::Duration;

use ag_ui_core::{Message, MessageId, RunOutcome, TextMessageRole, Tool, ToolCallId};
use ag_ui_server::{Agent, Error, Result, RunContext};
use futures_util::stream::{Stream, StreamExt as _};
use serde::Deserialize;
use serde_json::{Value, json};

/// The environment variable holding the API key.
pub const API_KEY_ENV: &str = "AG_UI_LLM_API_KEY";

/// Read when [`API_KEY_ENV`] is unset, because the default endpoint is Gemini's
/// and a contributor who has used this repo before already has this one set.
pub const FALLBACK_API_KEY_ENV: &str = "GEMINI_API_KEY";

/// The environment variable holding the base URL.
pub const BASE_URL_ENV: &str = "AG_UI_LLM_BASE_URL";

/// The environment variable holding the model id.
pub const MODEL_ENV: &str = "AG_UI_LLM_MODEL";

/// Where requests go unless [`BASE_URL_ENV`] says otherwise.
///
/// Gemini's OpenAI-compatible endpoint: the free tier needs no credential we do
/// not already have. `/chat/completions` is appended to this.
pub const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/openai";

/// The model this agent talks to by default.
///
/// Pinned, never a `*-latest` alias: those move — `gemini-flash-lite-latest`
/// currently resolves to a 3.x model — and behaviour changes under you without
/// a code change. Verified present on the default endpoint's model listing.
pub const DEFAULT_MODEL: &str = "gemini-2.5-flash-lite";

/// The tool this agent owns and executes itself.
pub const WEATHER_TOOL: &str = "get_weather";

/// How many model round trips one run may spend before giving up. A model that
/// answers its own tool result with another tool call would otherwise loop.
const MAX_TURNS: usize = 4;

/// [`LlmAgent::from_env`] found no API key, and the endpoint it would have
/// talked to needs one.
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

/// An [`Agent`] that answers with an OpenAI-compatible model, and can call one
/// tool on the way.
///
/// ```no_run
/// # use ag_ui_e2e::llm::LlmAgent;
/// # use ag_ui_axum::RouterExt;
/// let agent = LlmAgent::from_env().expect("AG_UI_LLM_API_KEY");
/// let app: axum::Router = axum::Router::new().route_agui("/agent", agent);
/// # let _ = app;
/// ```
pub struct LlmAgent {
    client: reqwest::Client,
    base_url: String,
    model: String,
    /// Absent for a local server that wants no credential. Absent stays absent:
    /// an empty `Authorization: Bearer` header is a rejected request, not an
    /// anonymous one.
    api_key: Option<String>,
}

impl fmt::Debug for LlmAgent {
    /// Redacts the key. A `#[derive(Debug)]` here would put it in the first log
    /// line that formats a router.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LlmAgent")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl LlmAgent {
    /// An agent pointed at `base_url`, talking to `model`.
    ///
    /// `api_key` is [`None`] for an endpoint that wants no credential, which is
    /// the usual case for a model served on localhost.
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: Option<String>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            // Bounds a stalled stream without bounding a slow one: the timeout
            // is per read, not for the whole response.
            .read_timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        Self {
            client,
            // A trailing slash here would produce `//chat/completions`, which
            // some servers route and some 404.
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            model: model.into(),
            api_key: api_key.filter(|key| !key.trim().is_empty()),
        }
    }

    /// An agent configured from the environment.
    ///
    /// | Variable | Default |
    /// | --- | --- |
    /// | [`BASE_URL_ENV`] | [`DEFAULT_BASE_URL`] |
    /// | [`MODEL_ENV`] | [`DEFAULT_MODEL`] |
    /// | [`API_KEY_ENV`], then [`FALLBACK_API_KEY_ENV`] | none |
    ///
    /// # Errors
    ///
    /// [`MissingApiKey`] when no key is set *and* the endpoint is the default
    /// one, which needs one. A custom [`BASE_URL_ENV`] is taken to mean a
    /// server the caller runs, so a missing key there is not an error — it is
    /// sent as absent.
    pub fn from_env() -> std::result::Result<Self, MissingApiKey> {
        let base_url = var(BASE_URL_ENV).unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
        let model = var(MODEL_ENV).unwrap_or_else(|| DEFAULT_MODEL.to_owned());
        let api_key = var(API_KEY_ENV).or_else(|| var(FALLBACK_API_KEY_ENV));

        if api_key.is_none() && base_url.trim_end_matches('/') == DEFAULT_BASE_URL {
            return Err(MissingApiKey);
        }
        Ok(Self::new(base_url, model, api_key))
    }

    /// Talks to a different model.
    #[must_use]
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// The model this agent is pointed at.
    #[must_use]
    pub fn model_name(&self) -> &str {
        &self.model
    }

    /// The endpoint this agent is pointed at. Carries no credential — the key
    /// is a header.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// POSTs one streaming request, mapping a non-2xx answer onto an error.
    ///
    /// The provider's error body is kept verbatim: a `429` from Gemini carries
    /// `details[].RetryInfo.retryDelay`, and the live test reads it back out of
    /// the `RUN_ERROR` message to decide how long to wait. It contains no
    /// credential — the key is a header, and never a query parameter, because
    /// query strings end up in logs.
    async fn send(&self, body: &Value) -> Result<reqwest::Response> {
        let mut request = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .json(body);
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }

        let response = request.send().await.map_err(Error::agent)?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::agent(format!(
                "the model returned HTTP {}: {}",
                status.as_u16(),
                body.trim()
            )));
        }
        Ok(response)
    }

    /// The request body for one turn.
    fn request(&self, messages: &[Value], tools: &[Value]) -> Value {
        let mut body = json!({
            "model": self.model,
            "messages": messages,
            "stream": true,
            // Temperature 0 is not about quality here — it keeps the live smoke
            // test's assertions as stable as a live model allows.
            "temperature": 0,
        });
        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        body
    }
}

/// A set, non-blank environment variable.
fn var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

impl Agent for LlmAgent {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let mut messages = messages_of(ctx.messages());
        if messages.is_empty() {
            return Err(Error::agent("the run carried nothing to send to the model"));
        }
        let tools = tools_for(ctx);

        for _ in 0..MAX_TURNS {
            // Cheap early out. Past this point every emit fails once the client
            // disconnects, so `?` unwinds the run without any further help.
            ctx.check_cancelled()?;

            let request = self.request(&messages, &tools);
            let response = self.send(&request).await?;
            let turn = stream_turn(ctx, Box::pin(response.bytes_stream())).await?;
            if turn.calls.is_empty() {
                return Ok(RunOutcome::Success);
            }

            let phase = tool_phase(ctx, &turn)?;
            messages.extend(phase.messages);
            if !phase.answered {
                // Every call belonged to the *client*: the front end runs them
                // and sends the results on the next request, so this run ends
                // after `TOOL_CALL_END`.
                return Ok(RunOutcome::Success);
            }
        }

        Err(Error::agent(format!(
            "the model asked for tools {MAX_TURNS} turns running"
        )))
    }
}

/// What one model turn produced.
#[derive(Debug, Default)]
struct Turn {
    /// Everything the turn said, already streamed to the client.
    text: String,
    /// The calls it asked for, fully accumulated, in arrival order.
    calls: Vec<Call>,
}

/// One tool call, reassembled from however many frames carried pieces of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Call {
    /// The server's own id. Absent only for a server that sends none.
    id: Option<String>,
    name: String,
    /// JSON text, concatenated from the fragments. Never parsed before the
    /// stream ends — a fragment is not valid JSON on its own.
    arguments: String,
    /// The provider's own `extra_content`, carried back untouched.
    ///
    /// # Why an opaque blob and not a parsed field
    ///
    /// Because the harness does not need to understand it, and every provider
    /// puts something different there. The case that forced it: a Gemini 3.x
    /// model signs its tool calls, and rejects the follow-up request unless the
    /// signature comes back with the call it arrived on —
    ///
    /// ```text
    /// HTTP 400: Function call is missing a thought_signature in functionCall parts.
    /// ```
    ///
    /// — which the compatibility endpoint expresses as
    /// `{"google": {"thought_signature": "EnEKbwER…"}}`. Round-tripping the
    /// whole object keeps that working without this file knowing what a thought
    /// signature is, and does the same for the next vendor extension.
    ///
    /// Absent stays absent. 2.5 sends none and needs none, and an empty
    /// extension is not the same as no extension.
    extra: Option<Value>,
}

impl Call {
    /// The arguments as JSON text.
    ///
    /// A model calling a no-argument tool sends `""`. AG-UI carries arguments as
    /// a string the client is expected to parse, so an empty one becomes an
    /// empty object here rather than a parse error somewhere downstream.
    fn arguments(&self) -> &str {
        if self.arguments.trim().is_empty() {
            "{}"
        } else {
            &self.arguments
        }
    }
}

/// Streams one response, emitting `TEXT_MESSAGE_*` as text arrives and
/// accumulating the tool calls for [`tool_phase`] to emit.
///
/// # Why text streams and calls do not
///
/// A [`MessageHandle`](ag_ui_server::MessageHandle) borrows the run context
/// mutably for as long as it lives, so it cannot be opened lazily inside a loop
/// that also uses the context. The first phase therefore reads frames with
/// nothing open, until text actually shows up; the second holds the message
/// open until the stream ends. Calls accumulate in a plain [`Calls`], which
/// borrows nothing — that is what lets a turn mix text and calls.
///
/// The calls are then emitted *after* the message closes, fully formed rather
/// than streamed. That is forced by the same typestate rule: parallel calls
/// arrive interleaved by `index`, and two open [`ToolCallHandle`]s at once is a
/// borrow-check error by design. Accumulating first is the only mapping that
/// keeps interleaved arguments from being spliced into each other.
///
/// [`ToolCallHandle`]: ag_ui_server::ToolCallHandle
async fn stream_turn<S, B, E>(ctx: &mut RunContext<()>, stream: S) -> Result<Turn>
where
    S: Stream<Item = std::result::Result<B, E>> + Unpin,
    B: AsRef<[u8]>,
    E: std::error::Error + Send + Sync + 'static,
{
    let mut frames = SseFrames::new(stream);
    let mut calls = Calls::default();
    let mut turn = Turn::default();
    let mut opening = None;

    while let Some(frame) = frames.next_frame().await? {
        let content = frame.content().to_owned();
        let id = frame.id.clone();
        calls.merge(frame.into_tool_calls());
        // A frame carrying only `finish_reason`, or only a role, or only a
        // fragment of a tool call, says nothing about the message text.
        if !content.is_empty() {
            // The completion id is stable across the stream, so it identifies
            // the message directly.
            let id = match id.filter(|id| !id.is_empty()) {
                Some(id) => MessageId::new(id),
                None => ctx.new_message_id(),
            };
            opening = Some((id, content));
            break;
        }
    }

    if let Some((id, first)) = opening {
        turn.text.push_str(&first);
        let mut message = ctx.message_with_id(id, TextMessageRole::Assistant)?;
        message.delta(first)?;

        while let Some(frame) = frames.next_frame().await? {
            let content = frame.content().to_owned();
            calls.merge(frame.into_tool_calls());
            // The final frame usually carries `finish_reason` and nothing else.
            // An empty delta is not an update.
            if !content.is_empty() {
                turn.text.push_str(&content);
                message.delta(content)?;
            }
        }
        message.end()?;
    }

    turn.calls = calls.finish();
    Ok(turn)
}

/// What [`tool_phase`] produced.
struct ToolPhase {
    /// The assistant turn echoed back, plus one `role: "tool"` message per
    /// answer — ready to append to the next request.
    messages: Vec<Value>,
    /// Whether this agent answered anything at all. When it did not, every call
    /// belonged to the client and there is nothing to ask the model about.
    answered: bool,
}

/// Emits each accumulated call as `TOOL_CALL_START` / `ARGS` / `END`, runs the
/// ones this agent owns, and builds the messages the next request carries.
///
/// The model needs its own tool calls echoed back before it will read the
/// answers, and each answer is matched to its call by `tool_call_id` — which is
/// why the ids are resolved here, once, and used for both.
fn tool_phase(ctx: &mut RunContext<()>, turn: &Turn) -> Result<ToolPhase> {
    let mut echoed = Vec::with_capacity(turn.calls.len());
    let mut answers = Vec::new();

    for call in &turn.calls {
        // The server supplies the id, so it is used as-is; synthesizing one
        // would break the match between the echo and its answer. Only a server
        // that sends none gets a made-up id.
        let id = match &call.id {
            Some(id) => ToolCallId::new(id.clone()),
            None => ctx.new_tool_call_id(),
        };

        let mut echo = json!({
            "id": id.as_str(),
            "type": "function",
            "function": {"name": call.name, "arguments": call.arguments()},
        });
        // Whatever the provider attached to this call goes back on it,
        // untouched. Absent stays absent — see [`Call::extra`].
        if let Some(extra) = &call.extra {
            echo["extra_content"] = extra.clone();
        }
        echoed.push(echo);

        let mut handle = ctx.tool_call_with_id(id.clone(), &call.name)?;
        // Already a string on this wire format, so it goes straight through —
        // no re-serialization, and no chance of reordering the model's keys.
        handle.args(call.arguments())?;

        match execute(call) {
            Some(result) => {
                handle.result_json(&result)?;
                answers.push(json!({
                    "role": "tool",
                    "tool_call_id": id.as_str(),
                    "content": serde_json::to_string(&result).unwrap_or_default(),
                }));
            }
            // A tool the *client* offered: it runs there, not here.
            None => handle.end()?,
        }
    }

    let mut assistant = json!({"role": "assistant", "tool_calls": echoed});
    if !turn.text.is_empty() {
        assistant["content"] = json!(turn.text);
    }

    let answered = !answers.is_empty();
    let mut messages = vec![assistant];
    messages.append(&mut answers);
    Ok(ToolPhase { messages, answered })
}

/// Runs a call this agent owns. `None` means the tool belongs to the client.
fn execute(call: &Call) -> Option<Value> {
    if call.name != WEATHER_TOOL {
        return None;
    }
    let arguments: Value = serde_json::from_str(call.arguments()).unwrap_or(Value::Null);
    let city = arguments.get("city").and_then(Value::as_str).unwrap_or("");
    Some(json!({
        "city": city,
        "temperatureC": 21,
        "conditions": "clear",
        // Said plainly, because it is: the round trip is the point, not the
        // weather, and a fixed answer keeps the live test's assertions honest.
        "source": "synthetic",
    }))
}

/// The AG-UI definition of the tool this agent owns.
pub fn weather_tool() -> Tool {
    Tool::new(
        WEATHER_TOOL,
        "Current weather for a city.",
        json!({
            "type": "object",
            "properties": {
                "city": {"type": "string", "description": "City name, for example Seoul."},
            },
            "required": ["city"],
        }),
    )
}

/// Everything this run may call: the built-in tool, plus whatever the client
/// offered under a different name.
fn tools_for(ctx: &RunContext<()>) -> Vec<Value> {
    let builtin = weather_tool();
    let mut tools = vec![function_tool(&builtin)];
    tools.extend(
        ctx.tools()
            .iter()
            .filter(|tool| tool.name != builtin.name)
            .map(function_tool),
    );
    tools
}

/// One AG-UI tool as an OpenAI function definition.
///
/// The parameters go through **unchanged**. That is the quiet payoff of this
/// wire format: an AG-UI [`Tool`] already carries ordinary lowercase JSON
/// Schema, which is exactly what this endpoint wants. The native Gemini dialect
/// wanted uppercase type names and an OpenAPI keyword subset, so this used to be
/// a recursive translation with a keyword whitelist.
fn function_tool(tool: &Tool) -> Value {
    let mut function = json!({"name": tool.name, "description": tool.description});
    if tool.parameters.is_object() {
        function["parameters"] = tool.parameters.clone();
    }
    json!({"type": "function", "function": function})
}

/// Maps the AG-UI history onto `messages`.
///
/// Also simpler than the native dialect: a tool result is matched to its call by
/// `tool_call_id`, which AG-UI carries on the tool message already, so nothing
/// has to index the assistant's calls on the way past to recover a name.
fn messages_of(messages: &[Message]) -> Vec<Value> {
    messages
        .iter()
        .filter_map(|message| match message {
            // `developer` is a newer role that not every compatible server
            // accepts; `system` is understood everywhere.
            Message::System(message) => Some(json!({"role": "system", "content": message.content})),
            Message::Developer(message) => {
                Some(json!({"role": "system", "content": message.content}))
            }

            Message::User(message) => {
                Some(json!({"role": "user", "content": message.content.to_text()}))
            }

            Message::Assistant(message) => {
                let text = message.content.as_deref().filter(|text| !text.is_empty());
                let calls: Vec<Value> = message
                    .tool_calls
                    .iter()
                    .flatten()
                    .map(|call| {
                        json!({
                            "id": call.id.as_str(),
                            "type": "function",
                            "function": {
                                "name": call.function.name,
                                // Already a string in AG-UI, and already a
                                // string on this wire. Nothing to convert.
                                "arguments": call.function.arguments,
                            },
                        })
                    })
                    .collect();

                let mut out = json!({"role": "assistant"});
                if let Some(text) = text {
                    out["content"] = json!(text);
                }
                if !calls.is_empty() {
                    out["tool_calls"] = json!(calls);
                }
                // An assistant turn with neither text nor calls is not a turn.
                (text.is_some() || !calls.is_empty()).then_some(out)
            }

            Message::Tool(message) => Some(json!({
                "role": "tool",
                "tool_call_id": message.tool_call_id.as_str(),
                "content": message.content,
            })),

            // Reasoning and activity are for the client, not for the model.
            _ => None,
        })
        .collect()
}

/// The text of a user message. Non-text parts are dropped: this agent does not
/// claim to be multimodal.
/// Tool calls being reassembled from the fragments of a stream.
///
/// # Why this is not just a `HashMap<u64, _>`
///
/// The obvious implementation keys on `tool_calls[].index`, which the OpenAI
/// streaming format says is always present. It is not: **Gemini's compatibility
/// endpoint omits `index` entirely** and delivers parallel calls as several
/// entries of one frame's array, told apart only by `id`. Captured from the
/// wire, abridged:
///
/// ```text
/// "tool_calls":[{"function":{"arguments":"{\"city\":\"Seoul\"}","name":"get_weather"},
///                "id":"function-call-7026415214984972976","type":"function"},
///               {"function":{"arguments":"{\"city\":\"Oslo\"}","name":"get_weather"},
///                "id":"function-call-7026415214984972901","type":"function"}]
/// ```
///
/// Defaulting a missing `index` to `0` would concatenate those two into
/// `{"city":"Seoul"}{"city":"Oslo"}`. So the slot is resolved by `index` when
/// there is one, by `id` when there is not, and by position in the array as a
/// last resort — which is what a server sending neither leaves to work with.
#[derive(Debug, Default)]
struct Calls {
    /// Arrival order, which is the order the calls are emitted in.
    slots: Vec<Call>,
    /// The `index` each slot was opened under, parallel to `slots`.
    keys: Vec<Option<u64>>,
}

impl Calls {
    /// Folds one frame's `tool_calls` into the calls being built.
    fn merge(&mut self, deltas: Vec<ToolCallDelta>) {
        for (position, delta) in deltas.into_iter().enumerate() {
            let at = self.slot_for(&delta, position);
            let slot = &mut self.slots[at];

            if slot.id.is_none() {
                slot.id = delta.id.filter(|id| !id.is_empty());
            }
            // First one wins: the provider attaches this to the frame that
            // opens the call, and a later frame carrying none is not a
            // retraction.
            if slot.extra.is_none() {
                slot.extra = delta.extra.filter(|extra| !extra.is_null());
            }
            if let Some(function) = delta.function {
                // Sent once, on the frame that opens the call — but some servers
                // repeat it on every fragment, so this sets rather than appends.
                if let Some(name) = function.name.filter(|name| !name.is_empty()) {
                    if slot.name.is_empty() {
                        slot.name = name;
                    }
                }
                // The one field that really is a delta.
                if let Some(arguments) = function.arguments {
                    slot.arguments.push_str(&arguments);
                }
            }
        }
    }

    /// Which slot this fragment belongs to, opening one if it is new.
    ///
    /// The three keys cascade rather than being exclusive, so a server that
    /// sends `index` on some fragments and only `id` on others still lands them
    /// in one slot.
    fn slot_for(&mut self, delta: &ToolCallDelta, position: usize) -> usize {
        let id = delta.id.as_deref().filter(|id| !id.is_empty());
        let by_index = delta
            .index
            .and_then(|index| self.keys.iter().position(|key| *key == Some(index)));
        let by_id = id.and_then(|id| self.slots.iter().position(|s| s.id.as_deref() == Some(id)));
        // Position is the last resort, and only for a fragment that identifies
        // itself no other way — an unmatched `index` means a *new* call, not
        // whichever call happens to sit at the same offset.
        let by_position = (delta.index.is_none() && id.is_none() && position < self.slots.len())
            .then_some(position);

        let at = by_index.or(by_id).or(by_position).unwrap_or_else(|| {
            self.slots.push(Call::default());
            self.keys.push(None);
            self.slots.len() - 1
        });
        // Remember an index the slot did not already have, so later fragments
        // can find it that way too.
        if self.keys[at].is_none() {
            self.keys[at] = delta.index;
        }
        at
    }

    /// The finished calls.
    ///
    /// A nameless slot is dropped: it means a server opened a call and the
    /// stream ended before the name arrived, and a `TOOL_CALL_START` with an
    /// empty name is worse than no event at all.
    fn finish(self) -> Vec<Call> {
        self.slots
            .into_iter()
            .filter(|call| !call.name.is_empty())
            .collect()
    }
}

/// One decoded `data:` frame.
///
/// Everything is optional and unknown fields are ignored, so a field appearing
/// or moving on the provider's side degrades into missing data rather than a
/// failed run.
#[derive(Debug, Deserialize)]
struct ChatFrame {
    /// The completion id, stable for the whole stream.
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    choices: Vec<Choice>,
}

impl ChatFrame {
    /// This frame's text, empty when it carried none.
    fn content(&self) -> &str {
        self.choices
            .first()
            .and_then(|choice| choice.delta.as_ref())
            .and_then(|delta| delta.content.as_deref())
            .unwrap_or_default()
    }

    /// This frame's tool-call fragments, in array order.
    fn into_tool_calls(self) -> Vec<ToolCallDelta> {
        self.choices
            .into_iter()
            .next()
            .and_then(|choice| choice.delta)
            .map(|delta| delta.tool_calls)
            .unwrap_or_default()
    }
}

#[derive(Debug, Deserialize)]
struct Choice {
    /// Absent on a frame that carries only usage, which some servers append
    /// after the last content frame.
    #[serde(default)]
    delta: Option<Delta>,
}

#[derive(Debug, Deserialize)]
struct Delta {
    /// `null` on a frame that carries only a role, a tool-call fragment, or a
    /// finish reason.
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ToolCallDelta>,
}

/// One frame's worth of one tool call.
#[derive(Debug, Deserialize)]
struct ToolCallDelta {
    /// Which call this fragment belongs to. The OpenAI format says this is
    /// always present; Gemini's compatibility endpoint disagrees — see
    /// [`Calls`].
    #[serde(default)]
    index: Option<u64>,
    /// The server's call id, sent on the frame that opens the call.
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<FunctionDelta>,
    /// A provider extension riding along with the call — see [`Call::extra`].
    /// Deliberately untyped: it is echoed, never inspected.
    #[serde(default, rename = "extra_content")]
    extra: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct FunctionDelta {
    #[serde(default)]
    name: Option<String>,
    /// A fragment of the JSON arguments — not JSON itself. It can end anywhere,
    /// including inside a string literal or between a backslash and the
    /// character it escapes.
    #[serde(default)]
    arguments: Option<String>,
}

/// Frames an SSE body and decodes each `data:` payload.
///
/// Small enough to write out because the shape being consumed is narrow: one
/// JSON object per event, terminated by a `data: [DONE]` sentinel.
struct SseFrames<S> {
    stream: S,
    buffer: Vec<u8>,
    /// The sentinel arrived. Anything after it is not ours to read.
    done: bool,
    /// The body ended.
    ended: bool,
}

impl<S, B, E> SseFrames<S>
where
    S: Stream<Item = std::result::Result<B, E>> + Unpin,
    B: AsRef<[u8]>,
    E: std::error::Error + Send + Sync + 'static,
{
    fn new(stream: S) -> Self {
        Self {
            stream,
            buffer: Vec::new(),
            done: false,
            ended: false,
        }
    }

    /// The next decoded frame, or `None` at `[DONE]` or end of body.
    ///
    /// Both endings are handled because both happen: the sentinel is what a
    /// compatible endpoint promises, and a body that is cut short still has to
    /// end the loop rather than hang.
    async fn next_frame(&mut self) -> Result<Option<ChatFrame>> {
        loop {
            if self.done {
                return Ok(None);
            }

            if let Some(block) = take_block(&mut self.buffer) {
                match payload(&block) {
                    // Comments and keep-alives carry no `data:` line.
                    None => continue,
                    Some(data) => return self.decode(&data),
                }
            }

            if self.ended {
                // A body that ends without its final blank line still has one
                // frame in it.
                let rest = std::mem::take(&mut self.buffer);
                return match payload(&rest) {
                    Some(data) => self.decode(&data),
                    None => Ok(None),
                };
            }

            match self.stream.next().await {
                Some(Ok(chunk)) => self.buffer.extend_from_slice(chunk.as_ref()),
                Some(Err(error)) => return Err(Error::agent(error)),
                None => self.ended = true,
            }
        }
    }

    /// One `data:` payload, as a frame or as the end of the stream.
    fn decode(&mut self, data: &str) -> Result<Option<ChatFrame>> {
        if data.trim() == DONE {
            self.done = true;
            return Ok(None);
        }
        serde_json::from_str(data).map(Some).map_err(|error| {
            Error::agent(format!(
                "the model sent a frame this agent could not read: {error}"
            ))
        })
    }
}

/// The sentinel that ends an OpenAI-compatible stream.
const DONE: &str = "[DONE]";

/// Splits off the bytes up to the next blank line, if there is one.
///
/// The terminator is not the same everywhere, and not even the same across one
/// vendor's endpoints: Gemini's native SSE ends frames with `\r\n\r\n` and its
/// OpenAI-compatible endpoint with `\n\n`. A decoder that scans for only one of
/// them never finds a boundary, buffers the whole response and emits everything
/// at EOF — which reads as "streaming is broken" rather than as a parse error.
/// SSE allows all three line endings, so all three blank lines end a frame here.
fn take_block(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let (end, separator) = (0..buffer.len()).find_map(|index| {
        let rest = &buffer[index..];
        if rest.starts_with(b"\r\n\r\n") {
            Some((index, 4))
        } else if rest.starts_with(b"\n\n") || rest.starts_with(b"\r\r") {
            Some((index, 2))
        } else {
            None
        }
    })?;

    let mut block: Vec<u8> = buffer.drain(..end + separator).collect();
    block.truncate(end);
    Some(block)
}

/// The `data:` lines of one block, joined as the SSE spec asks.
fn payload(block: &[u8]) -> Option<String> {
    // Frames are split on a line boundary, so this never cuts a code point.
    let block = String::from_utf8_lossy(block);
    let mut data = String::new();
    for line in block.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
        }
    }
    (!data.trim().is_empty()).then_some(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ag_ui_core::{Event, RunAgentInput};

    /// A run context and its event stream, for driving the mapping without a
    /// network. `RunContext::new` exists for exactly this.
    fn context() -> (RunContext<()>, ag_ui_server::EventReceiver) {
        RunContext::new(RunAgentInput::new("t1", "r1")).expect("an empty state decodes")
    }

    /// Feeds `chunks` through the mapping and returns the AG-UI events it
    /// emitted, plus the turn it reassembled.
    async fn map(chunks: &[&'static [u8]]) -> (Vec<Event>, Turn) {
        let (mut ctx, mut events) = context();
        let body = chunks
            .iter()
            .map(|chunk| Ok::<&[u8], std::io::Error>(chunk))
            .collect::<Vec<_>>();
        let turn = stream_turn(&mut ctx, futures_util::stream::iter(body))
            .await
            .expect("the frames should decode");
        (events.drain(), turn)
    }

    /// Every `TEXT_MESSAGE_CONTENT` delta, in order.
    fn deltas(events: &[Event]) -> Vec<&str> {
        events
            .iter()
            .filter_map(|event| match event {
                Event::TextMessageContent(payload) => Some(payload.delta.as_str()),
                _ => None,
            })
            .collect()
    }

    /// The exact bytes of a live parallel tool call, captured from Gemini's
    /// OpenAI-compatible endpoint. Note what is *not* in it: no `index` on
    /// either call, and both calls in a single frame.
    const RECORDED_PARALLEL: &[u8] = b"data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"tool_calls\":[{\"function\":{\"arguments\":\"{\\\"city\\\":\\\"Seoul\\\"}\",\"name\":\"get_weather\"},\"id\":\"function-call-7026415214984972976\",\"type\":\"function\"},{\"function\":{\"arguments\":\"{\\\"city\\\":\\\"Oslo\\\"}\",\"name\":\"get_weather\"},\"id\":\"function-call-7026415214984972901\",\"type\":\"function\"}]},\"finish_reason\":\"tool_calls\",\"index\":0}],\"created\":1786978672,\"id\":\"byGDariPGvbS1e8PuviF8QE\",\"model\":\"gemini-2.5-flash-lite\",\"object\":\"chat.completion.chunk\"}\n\ndata: [DONE]\n\n";

    /// The exact bytes of a live text turn, same endpoint.
    const RECORDED_TEXT: &[u8] = b"data: {\"choices\":[{\"delta\":{\"content\":\"One,\",\"role\":\"assistant\"},\"index\":0}],\"created\":1786978761,\"id\":\"iiGDaurJKvnE0-kPrsjvuA8\",\"model\":\"gemini-2.5-flash-lite\",\"object\":\"chat.completion.chunk\"}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\" two, three.\",\"role\":\"assistant\"},\"finish_reason\":\"stop\",\"index\":0}],\"created\":1786978761,\"id\":\"iiGDaurJKvnE0-kPrsjvuA8\",\"model\":\"gemini-2.5-flash-lite\",\"object\":\"chat.completion.chunk\"}\n\ndata: [DONE]\n\n";

    #[tokio::test]
    async fn recorded_text_streams_as_one_message() {
        let (events, turn) = map(&[RECORDED_TEXT]).await;

        assert_eq!(deltas(&events), ["One,", " two, three."]);
        assert_eq!(turn.text, "One, two, three.");
        assert!(turn.calls.is_empty());

        // The completion id is stable across frames, so the whole turn is one
        // message under that id.
        let ids: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                Event::TextMessageStart(payload) => Some(payload.message_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, ["iiGDaurJKvnE0-kPrsjvuA8"]);
        assert_eq!(
            events.last().map(Event::event_type),
            Some(ag_ui_core::EventType::TextMessageEnd)
        );
    }

    /// The capture that broke the obvious implementation: no `index` anywhere,
    /// two calls, one frame.
    #[tokio::test]
    async fn recorded_parallel_calls_without_an_index_stay_apart() {
        let (events, turn) = map(&[RECORDED_PARALLEL]).await;

        // No text in this turn, so nothing should have been said.
        assert!(deltas(&events).is_empty(), "{events:?}");
        assert_eq!(turn.calls.len(), 2, "{:?}", turn.calls);
        assert_eq!(turn.calls[0].arguments, r#"{"city":"Seoul"}"#);
        assert_eq!(turn.calls[1].arguments, r#"{"city":"Oslo"}"#);
        assert_eq!(
            turn.calls[0].id.as_deref(),
            Some("function-call-7026415214984972976")
        );
        assert!(turn.calls.iter().all(|call| call.name == WEATHER_TOOL));
    }

    /// The single biggest difference from the native dialect: arguments are a
    /// stream of fragments, and a fragment can end anywhere at all.
    #[tokio::test]
    async fn arguments_split_mid_string_and_mid_escape_reassemble() {
        // The split points are deliberate: after the opening quote of a value,
        // between a backslash and the `"` it escapes, and inside a multi-byte
        // character's own escape sequence. Nothing here parses on its own.
        let chunks: &[&[u8]] = &[
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"{\\\"cit\"}}]}}]}\n\n",
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"y\\\":\\\"Se\"}}]}}]}\n\n",
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"oul \\\\\"}}]}}]}\n\n",
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"quoted\\\\\\\" \\\\u00e9\\\"}\"}}]}}]}\n\n",
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
            b"data: [DONE]\n\n",
        ];

        let (_, turn) = map(chunks).await;
        assert_eq!(turn.calls.len(), 1, "{:?}", turn.calls);

        let call = &turn.calls[0];
        assert_eq!(call.name, WEATHER_TOOL);
        assert_eq!(call.id.as_deref(), Some("call_1"));
        // Only now, with every fragment in hand, is it JSON.
        let arguments: Value =
            serde_json::from_str(call.arguments()).expect("the fragments reassemble into JSON");
        assert_eq!(arguments["city"], "Seoul \"quoted\" é");
    }

    /// A live Gemini **3.x** parallel tool call, captured from the same
    /// OpenAI-compatible endpoint. The base64 signature is truncated — it is
    /// opaque and its length is not the point; everything else is verbatim.
    ///
    /// Three things in here that the 2.5 capture does not have: the calls are in
    /// **separate frames**, still with no `index` on either (so array position
    /// is 0 for both, and only `id` tells them apart), the first carries an
    /// `extra_content` signature and the second does not, and the turn ends with
    /// a frame that has a `delta` containing nothing but a role.
    const RECORDED_SIGNED_PARALLEL: &[u8] = b"data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"tool_calls\":[{\"extra_content\":{\"google\":{\"thought_signature\":\"EnEKbwERTTIP0Zk3tjLvi9mRksxP\"}},\"function\":{\"arguments\":\"{\\\"city\\\":\\\"Seoul\\\"}\",\"name\":\"get_weather\"},\"id\":\"call_272732\",\"type\":\"function\"}]},\"index\":0}],\"created\":1786979368,\"id\":\"JySDarX1H6-w1e8PlI7z6QU\",\"model\":\"gemini-3.1-flash-lite\",\"object\":\"chat.completion.chunk\"}\n\ndata: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"tool_calls\":[{\"function\":{\"arguments\":\"{\\\"city\\\":\\\"Oslo\\\"}\",\"name\":\"get_weather\"},\"id\":\"call_272740\",\"type\":\"function\"}]},\"index\":0}],\"created\":1786979368,\"id\":\"JySDarX1H6-w1e8PlI7z6QU\",\"model\":\"gemini-3.1-flash-lite\",\"object\":\"chat.completion.chunk\"}\n\ndata: {\"choices\":[{\"delta\":{\"role\":\"assistant\"},\"finish_reason\":\"stop\",\"index\":0}],\"created\":1786979368,\"id\":\"JySDarX1H6-w1e8PlI7z6QU\",\"model\":\"gemini-3.1-flash-lite\",\"object\":\"chat.completion.chunk\"}\n\ndata: [DONE]\n\n";

    /// Recorded proof of the `id`-keyed path: two calls, separate frames, and
    /// array position 0 for both.
    #[tokio::test]
    async fn recorded_signed_parallel_calls_stay_apart_and_keep_their_signature() {
        let (events, turn) = map(&[RECORDED_SIGNED_PARALLEL]).await;

        assert!(deltas(&events).is_empty(), "{events:?}");
        assert_eq!(turn.calls.len(), 2, "{:?}", turn.calls);
        assert_eq!(turn.calls[0].arguments, r#"{"city":"Seoul"}"#);
        assert_eq!(turn.calls[1].arguments, r#"{"city":"Oslo"}"#);
        assert_eq!(turn.calls[0].id.as_deref(), Some("call_272732"));
        assert_eq!(turn.calls[1].id.as_deref(), Some("call_272740"));

        // The provider signs the first call of a batch and only that one.
        assert_eq!(
            turn.calls[0].extra.as_ref().and_then(|extra| extra
                .pointer("/google/thought_signature")
                .and_then(Value::as_str)),
            Some("EnEKbwERTTIP0Zk3tjLvi9mRksxP")
        );
        assert!(turn.calls[1].extra.is_none(), "{:?}", turn.calls[1]);
    }

    /// The half that the live run actually failed on: the signature has to go
    /// back on the call it arrived with, or the *next* request is a 400.
    #[test]
    fn a_provider_extension_is_echoed_back_on_the_call_it_arrived_on() {
        let (mut ctx, mut events) = context();
        let signature = json!({"google": {"thought_signature": "EnEKbwER"}});
        let turn = Turn {
            text: String::new(),
            calls: vec![
                Call {
                    id: Some("call_a".to_owned()),
                    name: WEATHER_TOOL.to_owned(),
                    arguments: r#"{"city":"Seoul"}"#.to_owned(),
                    extra: Some(signature.clone()),
                },
                Call {
                    id: Some("call_b".to_owned()),
                    name: WEATHER_TOOL.to_owned(),
                    arguments: r#"{"city":"Oslo"}"#.to_owned(),
                    extra: None,
                },
            ],
        };

        let phase = tool_phase(&mut ctx, &turn).expect("the calls should emit");
        let echoed = &phase.messages[0]["tool_calls"];
        assert_eq!(echoed[0]["extra_content"], signature);
        assert_eq!(echoed[0]["id"], "call_a");
        // Absent stays absent: an unsigned call must not go back carrying
        // `null`, or an empty object the model never wrote.
        assert!(echoed[1].get("extra_content").is_none(), "{}", echoed[1]);

        // And none of it leaks into the AG-UI stream — the protocol carries the
        // call, not the provider's reasoning about it.
        let rendered = format!("{:?}", events.drain());
        assert!(!rendered.contains("thought_signature"), "{rendered}");
    }

    /// The case that only `id` keying survives: no `index` anywhere, and the
    /// two calls' fragments arrive in separate frames. Keying on array position
    /// would put every one of these at position 0 and splice both calls into a
    /// single slot; keying on a defaulted `index` would do the same.
    #[tokio::test]
    async fn parallel_calls_without_an_index_are_kept_apart_by_id() {
        let chunks: &[&[u8]] = &[
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"id\":\"call_a\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"{\\\"city\\\":\"}}]}}]}\n\n",
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"id\":\"call_b\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"{\\\"city\\\":\"}}]}}]}\n\n",
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"id\":\"call_a\",\"function\":{\"arguments\":\"\\\"Seoul\\\"}\"}}]}}]}\n\n",
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"id\":\"call_b\",\"function\":{\"arguments\":\"\\\"Oslo\\\"}\"}}]}}]}\n\n",
            b"data: [DONE]\n\n",
        ];

        let (_, turn) = map(chunks).await;
        assert_eq!(turn.calls.len(), 2, "{:?}", turn.calls);
        assert_eq!(turn.calls[0].arguments, r#"{"city":"Seoul"}"#);
        assert_eq!(turn.calls[1].arguments, r#"{"city":"Oslo"}"#);
    }

    /// A server that sends `index` on the opening fragment and only `id`
    /// afterwards — or the reverse. Both keys have to reach the same slot.
    #[tokio::test]
    async fn a_call_identified_two_different_ways_stays_one_call() {
        let chunks: &[&[u8]] = &[
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"id\":\"call_a\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"{\\\"ci\"}}]}}]}\n\n",
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_a\",\"function\":{\"arguments\":\"ty\\\":\"}}]}}]}\n\n",
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"Seoul\\\"}\"}}]}}]}\n\n",
            b"data: [DONE]\n\n",
        ];

        let (_, turn) = map(chunks).await;
        assert_eq!(turn.calls.len(), 1, "{:?}", turn.calls);
        assert_eq!(turn.calls[0].arguments, r#"{"city":"Seoul"}"#);
        assert_eq!(turn.calls[0].id.as_deref(), Some("call_a"));
    }

    /// The other providers' shape: parallel calls arrive interleaved across
    /// frames and are told apart only by `index`.
    #[tokio::test]
    async fn parallel_calls_interleaved_by_index_do_not_bleed_into_each_other() {
        let chunks: &[&[u8]] = &[
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_a\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"\"}}]}}]}\n\n",
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"call_b\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"\"}}]}}]}\n\n",
            // Now the two argument streams alternate, one fragment at a time.
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"city\\\":\"}}]}}]}\n\n",
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1,\"function\":{\"arguments\":\"{\\\"city\\\":\"}}]}}]}\n\n",
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1,\"function\":{\"arguments\":\"\\\"Oslo\\\"}\"}}]}}]}\n\n",
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"Seoul\\\"}\"}}]}}]}\n\n",
            b"data: [DONE]\n\n",
        ];

        let (_, turn) = map(chunks).await;
        assert_eq!(turn.calls.len(), 2, "{:?}", turn.calls);
        assert_eq!(turn.calls[0].id.as_deref(), Some("call_a"));
        assert_eq!(turn.calls[0].arguments, r#"{"city":"Seoul"}"#);
        assert_eq!(turn.calls[1].id.as_deref(), Some("call_b"));
        assert_eq!(turn.calls[1].arguments, r#"{"city":"Oslo"}"#);
    }

    /// Both endings, because both happen. The sentinel is the promise; a body
    /// that just stops is what a proxy or a crash actually delivers.
    #[tokio::test]
    async fn a_done_sentinel_ends_the_stream_and_nothing_after_it_is_read() {
        let chunks: &[&[u8]] = &[
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
            b"data: [DONE]\n\n",
            // A server that keeps talking past its own sentinel, or a proxy
            // that appends something. Reading this would be a parse error.
            b"data: not json at all\n\n",
        ];

        let (events, turn) = map(chunks).await;
        assert_eq!(deltas(&events), ["hi"]);
        assert_eq!(turn.text, "hi");
    }

    #[tokio::test]
    async fn a_stream_that_ends_without_a_sentinel_still_ends() {
        let chunks: &[&[u8]] = &[
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
            // No trailing blank line either: the last frame is all there is.
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"content\":\" there\"}}]}",
        ];

        let (events, _) = map(chunks).await;
        assert_eq!(deltas(&events), ["hi", " there"]);
    }

    /// `[DONE]` with no `finish_reason` anywhere: a well-formed turn all the
    /// same, and the calls in it still have to come out.
    #[tokio::test]
    async fn a_done_with_no_finish_reason_still_yields_its_call() {
        let chunks: &[&[u8]] = &[
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"{\\\"city\\\":\\\"Oslo\\\"}\"}}]}}]}\n\n",
            b"data: [DONE]\n\n",
        ];

        let (_, turn) = map(chunks).await;
        assert_eq!(turn.calls.len(), 1);
        assert_eq!(turn.calls[0].arguments, r#"{"city":"Oslo"}"#);
    }

    /// The final frame usually carries `finish_reason` and nothing else. An
    /// empty `TEXT_MESSAGE_CONTENT` is not an update, and a client that renders
    /// one shows a flicker for it.
    #[tokio::test]
    async fn a_contentless_final_frame_emits_no_empty_delta() {
        let chunks: &[&[u8]] = &[
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\n",
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"content\":\"done\"}}]}\n\n",
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"content\":null},\"finish_reason\":\"stop\"}]}\n\n",
            // Some servers append a usage-only frame with no `delta` at all.
            b"data: {\"id\":\"c1\",\"choices\":[],\"usage\":{\"total_tokens\":9}}\n\n",
            b"data: [DONE]\n\n",
        ];

        let (events, _) = map(chunks).await;
        assert_eq!(deltas(&events), ["done"]);
        assert!(
            deltas(&events).iter().all(|delta| !delta.is_empty()),
            "{events:?}"
        );
    }

    /// Gemini's compatible endpoint sends `\n\n`, its native one `\r\n\r\n`, and
    /// a frame can be cut anywhere by the chunking underneath.
    #[tokio::test]
    async fn frames_survive_chunk_boundaries_and_either_terminator() {
        let chunks: &[&[u8]] = &[
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"content\":\"It is \"}}]}\r\n\r\ndata: {\"id\":\"c1\",\"cho",
            b"ices\":[{\"delta\":{\"content\":\"sunny.\"}}]}\n\ndata: [DONE]\n\n",
        ];

        let (events, turn) = map(chunks).await;
        assert_eq!(deltas(&events), ["It is ", "sunny."]);
        assert_eq!(turn.text, "It is sunny.");
    }

    /// A turn that says something *and* calls a tool. Text streams as it
    /// arrives; the call is emitted whole, after the message closes.
    #[tokio::test]
    async fn text_and_a_call_in_one_turn_both_survive() {
        let chunks: &[&[u8]] = &[
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"content\":\"Let me check.\"}}]}\n\n",
            b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"{\\\"city\\\":\\\"Seoul\\\"}\"}}]}}]}\n\n",
            b"data: [DONE]\n\n",
        ];

        let (events, turn) = map(chunks).await;
        assert_eq!(deltas(&events), ["Let me check."]);
        assert_eq!(turn.calls.len(), 1);
        assert_eq!(turn.text, "Let me check.");
    }

    /// The tool half of the mapping, on the real emit path.
    #[test]
    fn a_call_maps_onto_start_args_end_and_result() {
        let (mut ctx, mut events) = context();
        let turn = Turn {
            text: String::new(),
            calls: vec![Call {
                id: Some("call_1".to_owned()),
                name: WEATHER_TOOL.to_owned(),
                arguments: r#"{"city":"Seoul"}"#.to_owned(),
                extra: None,
            }],
        };

        let phase = tool_phase(&mut ctx, &turn).expect("the call should emit");
        let events = events.drain();

        let types: Vec<_> = events.iter().map(Event::event_type).collect();
        assert_eq!(
            types,
            [
                ag_ui_core::EventType::ToolCallStart,
                ag_ui_core::EventType::ToolCallArgs,
                ag_ui_core::EventType::ToolCallEnd,
                ag_ui_core::EventType::ToolCallResult,
            ],
            "{types:?}"
        );

        // The server's id is carried through, never replaced.
        for event in &events {
            let id = match event {
                Event::ToolCallStart(payload) => &payload.tool_call_id,
                Event::ToolCallArgs(payload) => &payload.tool_call_id,
                Event::ToolCallEnd(payload) => &payload.tool_call_id,
                Event::ToolCallResult(payload) => &payload.tool_call_id,
                _ => continue,
            };
            assert_eq!(id.as_str(), "call_1", "{event:?}");
        }

        let arguments: String = events
            .iter()
            .filter_map(|event| match event {
                Event::ToolCallArgs(payload) => Some(payload.delta.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(arguments, r#"{"city":"Seoul"}"#);

        // And the model gets its own call back, plus the answer, matched by id.
        assert!(phase.answered);
        assert_eq!(phase.messages.len(), 2);
        assert_eq!(phase.messages[0]["tool_calls"][0]["id"], "call_1");
        assert_eq!(phase.messages[1]["role"], "tool");
        assert_eq!(phase.messages[1]["tool_call_id"], "call_1");
        assert!(
            phase.messages[1]["content"]
                .as_str()
                .is_some_and(|content| content.contains("21")),
            "{}",
            phase.messages[1]
        );
    }

    /// A tool the client offered: streamed, never executed, and the run has
    /// nothing further to ask the model.
    #[test]
    fn a_client_owned_tool_is_streamed_but_not_answered() {
        let (mut ctx, mut events) = context();
        let turn = Turn {
            text: String::new(),
            calls: vec![Call {
                id: Some("call_1".to_owned()),
                name: "open_dialog".to_owned(),
                arguments: r#"{"kind":"confirm"}"#.to_owned(),
                extra: None,
            }],
        };

        let phase = tool_phase(&mut ctx, &turn).expect("the call should emit");
        let types: Vec<_> = events.drain().iter().map(Event::event_type).collect();
        assert_eq!(
            types,
            [
                ag_ui_core::EventType::ToolCallStart,
                ag_ui_core::EventType::ToolCallArgs,
                ag_ui_core::EventType::ToolCallEnd,
            ],
            "{types:?}"
        );
        assert!(!phase.answered);
        assert_eq!(phase.messages.len(), 1);
    }

    /// A server that sends no id at all still has to produce a usable stream:
    /// AG-UI needs one on all four events.
    #[test]
    fn a_call_without_a_server_id_gets_one_synthesized() {
        let (mut ctx, mut events) = context();
        let turn = Turn {
            text: String::new(),
            calls: vec![Call {
                id: None,
                name: WEATHER_TOOL.to_owned(),
                arguments: String::new(),
                extra: None,
            }],
        };

        let phase = tool_phase(&mut ctx, &turn).expect("the call should emit");
        let events = events.drain();

        let id = events
            .iter()
            .find_map(|event| match event {
                Event::ToolCallStart(payload) => Some(payload.tool_call_id.clone()),
                _ => None,
            })
            .expect("a start event");
        assert!(!id.is_empty());
        assert_eq!(phase.messages[0]["tool_calls"][0]["id"], id.as_str());

        // And the empty argument string became parseable JSON rather than
        // being forwarded as `""`.
        let arguments: String = events
            .iter()
            .filter_map(|event| match event {
                Event::ToolCallArgs(payload) => Some(payload.delta.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(arguments, "{}");
    }

    #[test]
    fn a_tool_schema_goes_to_the_model_unchanged() {
        let tool = weather_tool();
        let sent = function_tool(&tool);
        assert_eq!(sent["type"], "function");
        assert_eq!(sent["function"]["name"], WEATHER_TOOL);
        // Lowercase, verbatim — no dialect translation on this wire format.
        assert_eq!(sent["function"]["parameters"], tool.parameters);
        assert_eq!(sent["function"]["parameters"]["type"], "object");
        assert_eq!(
            sent["function"]["parameters"]["properties"]["city"]["type"],
            "string"
        );
    }

    #[test]
    fn a_tool_result_is_matched_to_its_call_by_id() {
        let messages = vec![
            Message::system("m0", "Be brief."),
            Message::user("m1", "weather in Seoul?"),
            Message::Assistant(ag_ui_core::AssistantMessage {
                id: MessageId::new("m2"),
                tool_calls: Some(vec![ag_ui_core::ToolCall::new(
                    "c1",
                    WEATHER_TOOL,
                    r#"{"city":"Seoul"}"#,
                )]),
                ..Default::default()
            }),
            Message::tool("m3", "c1", r#"{"temperatureC":21}"#),
        ];

        let sent = messages_of(&messages);
        assert_eq!(sent.len(), 4);
        assert_eq!(sent[0]["role"], "system");
        assert_eq!(sent[1]["role"], "user");
        assert_eq!(sent[2]["role"], "assistant");
        assert_eq!(sent[2]["tool_calls"][0]["id"], "c1");
        assert_eq!(sent[2]["tool_calls"][0]["function"]["name"], WEATHER_TOOL);
        // A string on both sides, so it is passed through rather than reparsed.
        assert_eq!(
            sent[2]["tool_calls"][0]["function"]["arguments"],
            r#"{"city":"Seoul"}"#
        );
        assert_eq!(sent[3]["role"], "tool");
        assert_eq!(sent[3]["tool_call_id"], "c1");
    }

    #[test]
    fn the_key_never_reaches_a_debug_line() {
        let agent = LlmAgent::new(DEFAULT_BASE_URL, DEFAULT_MODEL, Some("s3cret".to_owned()));
        let rendered = format!("{agent:?}");
        assert!(!rendered.contains("s3cret"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
    }

    /// A local server usually wants no credential, and an empty `Bearer` is a
    /// rejected request rather than an anonymous one.
    #[test]
    fn a_blank_key_is_absent_rather_than_empty() {
        let agent = LlmAgent::new("http://localhost:11434/v1", "qwen3", Some("  ".to_owned()));
        assert!(agent.api_key.is_none());
    }

    #[test]
    fn a_trailing_slash_does_not_double_up_the_path() {
        let agent = LlmAgent::new("http://localhost:1234/v1/", "local", None);
        assert_eq!(agent.base_url(), "http://localhost:1234/v1");
    }
}
