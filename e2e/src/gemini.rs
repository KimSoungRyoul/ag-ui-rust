//! A Gemini-backed [`Agent`], talking to the model over plain HTTP.
//!
//! # Why this exists
//!
//! Two reasons, and the second matters more.
//!
//! It proves the protocol plumbing survives a real streaming model rather than
//! a fixture. And it is the **architecture test**: `docs/DESIGN.md` claims
//! [`Agent`] *is* the LLM boundary and that no crate in this workspace depends
//! on a model library. This agent reaches Gemini with `reqwest`, a handful of
//! `serde` structs and nothing else, and implements nothing but [`Agent`]. That
//! it compiles and streams is what turns that claim into evidence — so keep
//! `rig`, `async-openai` and friends out of it.
//!
//! The `gemini_agent` example serves this over HTTP; `tests/live_gemini.rs`
//! drives that endpoint against the live API. Both use this one implementation,
//! so the smoke test exercises exactly the code a reader is pointed at.
//!
//! # The mapping, and the parts of it that bite
//!
//! `docs/QA.md` records the whole Gemini-to-AG-UI mapping. The awkward corners,
//! all of them handled below:
//!
//! - There is **no `[DONE]` sentinel**. The stream ends at body EOF, and the
//!   last frame carries `finishReason: "STOP"`.
//! - `responseId` is stable for the whole stream, so it is the `messageId`.
//! - Function calls arrive **atomically in one frame**, fully formed — unlike
//!   OpenAI there is no partial-JSON accumulation.
//! - `functionCall.args` is a JSON *object*; `TOOL_CALL_ARGS` wants a string.
//! - `gemini-2.5-flash-lite` supplies **no call id**, so ids are synthesized.
//! - A final frame may carry an empty text part alongside `finishReason`, which
//!   must not become an empty `TEXT_MESSAGE_CONTENT`.
//! - Parallel calls arrive as several `functionCall` parts in one frame.
//! - `v1beta` function schemas use **uppercase** JSON Schema types (`OBJECT`,
//!   `STRING`), where AG-UI tool definitions are lowercase.
//! - A 3.x model signs its calls and rejects the follow-up request unless the
//!   `thoughtSignature` comes back on the part it arrived on. 2.5 sends none.
//!   This one only shows up when something falls back to 3.x — see [`Call`].

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use ag_ui_core::{
    InputContent, Message, MessageId, RunOutcome, TextMessageRole, Tool, ToolCallId, UserContent,
};
use ag_ui_server::{Agent, Error, Result, RunContext};
use futures_util::stream::{Stream, StreamExt as _};
use serde::Deserialize;
use serde_json::{Map, Value, json};

/// The environment variable holding the API key.
pub const API_KEY_ENV: &str = "GEMINI_API_KEY";

/// The model this agent talks to by default.
///
/// Pinned, never a `*-latest` alias: those move — `gemini-flash-lite-latest`
/// currently resolves to a 3.x model — and the response shape changes under you
/// without a code change.
pub const DEFAULT_MODEL: &str = "gemini-2.5-flash-lite";

/// The tool this agent owns and executes itself.
pub const WEATHER_TOOL: &str = "get_weather";

/// Where the streaming endpoint lives. The key travels as the `x-goog-api-key`
/// header, never as a query parameter — query strings end up in logs.
const API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";

/// How many model round trips one run may spend before giving up. A model that
/// answers its own tool result with another tool call would otherwise loop.
const MAX_TURNS: usize = 4;

/// [`GeminiAgent::from_env`] found no API key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MissingApiKey;

impl fmt::Display for MissingApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{API_KEY_ENV} is not set")
    }
}

impl std::error::Error for MissingApiKey {}

/// An [`Agent`] that answers with Gemini, and can call one tool on the way.
///
/// ```no_run
/// # use ag_ui_e2e::gemini::GeminiAgent;
/// # use ag_ui_axum::RouterExt;
/// let agent = GeminiAgent::from_env().expect("GEMINI_API_KEY");
/// let app: axum::Router = axum::Router::new().route_agui("/agent", agent);
/// # let _ = app;
/// ```
pub struct GeminiAgent {
    client: reqwest::Client,
    api_key: String,
    model: String,
}

impl fmt::Debug for GeminiAgent {
    /// Redacts the key. A `#[derive(Debug)]` here would put it in the first log
    /// line that formats a router.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GeminiAgent")
            .field("model", &self.model)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl GeminiAgent {
    /// An agent authenticated with `api_key`, talking to [`DEFAULT_MODEL`].
    pub fn new(api_key: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            // Bounds a stalled stream without bounding a slow one: the timeout
            // is per read, not for the whole response.
            .read_timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        Self {
            client,
            api_key: api_key.into(),
            model: DEFAULT_MODEL.to_owned(),
        }
    }

    /// An agent keyed from [`API_KEY_ENV`].
    ///
    /// # Errors
    ///
    /// [`MissingApiKey`] when the variable is absent or empty.
    pub fn from_env() -> std::result::Result<Self, MissingApiKey> {
        match std::env::var(API_KEY_ENV) {
            Ok(key) if !key.trim().is_empty() => Ok(Self::new(key)),
            _ => Err(MissingApiKey),
        }
    }

    /// Talks to a different model.
    #[must_use]
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// POSTs one streaming request, mapping a non-2xx answer onto an error.
    ///
    /// The provider's error body is kept verbatim: a `429` carries
    /// `details[].RetryInfo.retryDelay`, and the live test reads it back out of
    /// the `RUN_ERROR` message to decide how long to wait. It contains no
    /// credential — the key is a header.
    async fn send(&self, body: &Value) -> Result<reqwest::Response> {
        let url = format!("{API_BASE}/{}:streamGenerateContent?alt=sse", self.model);
        let response = self
            .client
            .post(url)
            .header("x-goog-api-key", &self.api_key)
            .json(body)
            .send()
            .await
            .map_err(Error::agent)?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::agent(format!(
                "gemini returned HTTP {}: {}",
                status.as_u16(),
                body.trim()
            )));
        }
        Ok(response)
    }

    /// The request body for one turn.
    fn request(&self, contents: &[Value], tools: &[Value], system: Option<&str>) -> Value {
        let mut body = json!({
            "contents": contents,
            // Temperature 0 is not about quality here — it keeps the live smoke
            // test's assertions as stable as a live model allows.
            "generationConfig": {"temperature": 0},
        });
        if !tools.is_empty() {
            body["tools"] = json!([{"functionDeclarations": tools}]);
        }
        if let Some(system) = system {
            body["systemInstruction"] = json!({"parts": [{"text": system}]});
        }
        body
    }
}

impl Agent for GeminiAgent {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let system = system_instruction(ctx.messages());
        let mut contents = contents_of(ctx.messages());
        if contents.is_empty() {
            return Err(Error::agent("the run carried nothing to send to the model"));
        }
        let declarations = declarations_for(ctx);

        for _ in 0..MAX_TURNS {
            // Cheap early out. Past this point every emit fails once the client
            // disconnects, so `?` unwinds the run without any further help.
            ctx.check_cancelled()?;

            let request = self.request(&contents, &declarations, system.as_deref());
            let turn = stream_turn(ctx, self.send(&request).await?).await?;
            if turn.calls.is_empty() {
                return Ok(RunOutcome::Success);
            }

            // Gemini needs its own call parts echoed back, signatures and all,
            // before it will read the answers.
            contents.push(model_turn(&turn));

            let mut answers = Vec::new();
            for call in &turn.calls {
                // 2.5-flash-lite sends no id, so one is synthesized per call —
                // which is also what keeps parallel calls apart. 3.x does send
                // one, and then it is used as-is.
                let id = match &call.id {
                    Some(id) => ToolCallId::new(id.clone()),
                    None => ctx.new_tool_call_id(),
                };

                let mut handle = ctx.tool_call_with_id(id, &call.name)?;
                // `args` is an object on the wire; AG-UI wants the JSON text.
                handle.args_json(&call.args)?;

                match execute(call) {
                    Some(result) => {
                        handle.result_json(&result)?;
                        answers.push(function_response(call, &result));
                    }
                    // A tool the *client* offered: the front end runs it and
                    // sends the result back on the next request, so this run
                    // ends after TOOL_CALL_END.
                    None => handle.end()?,
                }
            }

            if answers.is_empty() {
                return Ok(RunOutcome::Success);
            }
            contents.push(json!({"role": "user", "parts": answers}));
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
    /// The signature the text arrived with, if a 3.x model wrote one.
    text_signature: Option<String>,
    /// The calls it asked for, in arrival order.
    calls: Vec<Call>,
}

/// Streams one Gemini response, emitting `TEXT_MESSAGE_*` as text arrives and
/// collecting the function calls for the caller to emit.
///
/// # Why the two phases
///
/// A [`MessageHandle`](ag_ui_server::MessageHandle) borrows the run context
/// mutably for as long as it lives, so it cannot be opened lazily inside a loop
/// that also uses the context. The first phase therefore reads frames with
/// nothing open, until text actually shows up; the second holds the message
/// open until the body ends. Calls collected in either phase go into a plain
/// `Vec`, which borrows nothing — that is what lets a turn mix text and calls.
///
/// Emitting the calls *after* the message closes is not just a borrow-checker
/// concession: Gemini sends them fully formed in the frame that ends the turn,
/// so there is nothing to stream in the meantime.
async fn stream_turn(ctx: &mut RunContext<()>, response: reqwest::Response) -> Result<Turn> {
    let mut frames = SseFrames::new(Box::pin(response.bytes_stream()));
    let mut turn = Turn::default();
    let mut opening = None;

    while let Some(frame) = frames.next_frame().await? {
        let text = frame.text();
        let id = frame.response_id.clone();
        turn.text_signature = frame
            .text_signature()
            .map(str::to_owned)
            .or(turn.text_signature);
        turn.calls.extend(frame.into_calls());
        if !text.is_empty() {
            // `responseId` is stable across the stream, so it identifies the
            // message directly.
            let id = match id {
                Some(id) => MessageId::new(id),
                None => ctx.new_message_id(),
            };
            opening = Some((id, text));
            break;
        }
    }

    if let Some((id, first)) = opening {
        turn.text.push_str(&first);
        let mut message = ctx.message_with_id(id, TextMessageRole::Assistant)?;
        message.delta(first)?;

        while let Some(frame) = frames.next_frame().await? {
            let text = frame.text();
            // Later parts win: the signature rides on the last part of a turn,
            // and the streamed fragments reassemble into that one part.
            turn.text_signature = frame
                .text_signature()
                .map(str::to_owned)
                .or(turn.text_signature);
            turn.calls.extend(frame.into_calls());
            // The last frame often carries an empty text part next to
            // `finishReason: STOP`. An empty delta is not an update.
            if !text.is_empty() {
                turn.text.push_str(&text);
                message.delta(text)?;
            }
        }
        message.end()?;
    }

    Ok(turn)
}

/// A signature worth sending back: present, and not the empty string.
fn signature(value: Option<&str>) -> Option<&str> {
    value.filter(|signature| !signature.is_empty())
}

/// The model's own turn, rebuilt so the next request carries it as context.
///
/// Two rules the provider enforces on this content: `thoughtSignature` goes on
/// the same part its `functionCall` arrived on, and the parts keep the order
/// they arrived in. The answers then follow as a separate turn — all the calls,
/// then all the responses, never interleaved.
fn model_turn(turn: &Turn) -> Value {
    let mut parts = Vec::with_capacity(turn.calls.len() + 1);

    if !turn.text.is_empty() {
        let mut part = json!({"text": turn.text});
        if let Some(signature) = &turn.text_signature {
            part["thoughtSignature"] = json!(signature);
        }
        parts.push(part);
    }

    for call in &turn.calls {
        let mut function_call = json!({"name": call.name, "args": call.args});
        // 3.x matches an answer to its call by id; 2.5 sends none, and then
        // there is nothing to match by but the name.
        if let Some(id) = &call.id {
            function_call["id"] = json!(id);
        }

        let mut part = json!({"functionCall": function_call});
        if let Some(signature) = &call.signature {
            part["thoughtSignature"] = json!(signature);
        }
        parts.push(part);
    }

    json!({"role": "model", "parts": parts})
}

/// One tool result, in the shape the model reads it back in.
fn function_response(call: &Call, result: &Value) -> Value {
    let mut response = json!({"name": call.name, "response": {"result": result}});
    if let Some(id) = &call.id {
        response["id"] = json!(id);
    }
    json!({"functionResponse": response})
}

/// Runs a call this agent owns. `None` means the tool belongs to the client.
fn execute(call: &Call) -> Option<Value> {
    if call.name != WEATHER_TOOL {
        return None;
    }
    let city = call.args.get("city").and_then(Value::as_str).unwrap_or("");
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
///
/// Spelled as an ordinary [`Tool`] — lowercase JSON Schema types and all — so
/// that it goes through the same translation a client-offered tool does.
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
fn declarations_for(ctx: &RunContext<()>) -> Vec<Value> {
    let builtin = weather_tool();
    let mut declarations = vec![declaration(&builtin)];
    declarations.extend(
        ctx.tools()
            .iter()
            .filter(|tool| tool.name != builtin.name)
            .map(declaration),
    );
    declarations
}

/// One AG-UI tool as a `v1beta` function declaration.
fn declaration(tool: &Tool) -> Value {
    let mut declaration = json!({"name": tool.name, "description": tool.description});
    if tool.parameters.is_object() {
        declaration["parameters"] = to_gemini_schema(&tool.parameters);
    }
    declaration
}

/// Translates a JSON Schema into the dialect `v1beta` accepts.
///
/// Two differences from the schema an AG-UI client sends: the type names are
/// uppercase (`OBJECT`, `STRING`), and the accepted keyword set is a small
/// OpenAPI subset that rejects anything else — `$schema` and
/// `additionalProperties` included. So this copies a whitelist rather than
/// passing the schema through.
fn to_gemini_schema(schema: &Value) -> Value {
    let Some(object) = schema.as_object() else {
        return json!({});
    };

    let mut out = Map::new();
    if let Some(name) = object.get("type").and_then(Value::as_str) {
        out.insert("type".to_owned(), json!(name.to_uppercase()));
    }
    for key in ["description", "enum", "required", "format", "nullable"] {
        if let Some(value) = object.get(key) {
            out.insert(key.to_owned(), value.clone());
        }
    }
    if let Some(properties) = object.get("properties").and_then(Value::as_object) {
        let translated = properties
            .iter()
            .map(|(name, schema)| (name.clone(), to_gemini_schema(schema)))
            .collect();
        out.insert("properties".to_owned(), Value::Object(translated));
    }
    if let Some(items) = object.get("items") {
        out.insert("items".to_owned(), to_gemini_schema(items));
    }
    Value::Object(out)
}

/// Folds the system and developer messages into one `systemInstruction`.
fn system_instruction(messages: &[Message]) -> Option<String> {
    let instructions: Vec<&str> = messages
        .iter()
        .filter_map(|message| match message {
            Message::System(message) => Some(message.content.as_str()),
            Message::Developer(message) => Some(message.content.as_str()),
            _ => None,
        })
        .collect();
    (!instructions.is_empty()).then(|| instructions.join("\n\n"))
}

/// Maps the AG-UI history onto Gemini `contents`.
///
/// Tool results are the only fiddly part: a `functionResponse` is matched to
/// its call by *name*, which AG-UI keeps on the assistant message rather than
/// on the tool message, so the assistant's calls are indexed on the way past.
fn contents_of(messages: &[Message]) -> Vec<Value> {
    let mut contents = Vec::new();
    let mut called: HashMap<&ToolCallId, &str> = HashMap::new();

    for message in messages {
        match message {
            // Hoisted into `systemInstruction` instead.
            Message::System(_) | Message::Developer(_) => {}

            Message::User(message) => {
                contents
                    .push(json!({"role": "user", "parts": [{"text": text_of(&message.content)}]}));
            }

            Message::Assistant(message) => {
                let mut parts = Vec::new();
                if let Some(text) = message.content.as_deref().filter(|text| !text.is_empty()) {
                    parts.push(json!({"text": text}));
                }
                for call in message.tool_calls.iter().flatten() {
                    called.insert(&call.id, call.function.name.as_str());
                    let args: Value = serde_json::from_str(&call.function.arguments)
                        .unwrap_or_else(|_| json!({}));
                    parts.push(json!({"functionCall": {"name": call.function.name, "args": args}}));
                }
                if !parts.is_empty() {
                    contents.push(json!({"role": "model", "parts": parts}));
                }
            }

            Message::Tool(message) => {
                let name = called.get(&message.tool_call_id).copied().unwrap_or("");
                // A tool that returned JSON is fed back as JSON; anything else
                // goes back as the string it is.
                let result = serde_json::from_str::<Value>(&message.content)
                    .unwrap_or_else(|_| json!(message.content));
                contents.push(json!({
                    "role": "user",
                    "parts": [{"functionResponse": {"name": name, "response": {"result": result}}}],
                }));
            }

            // Reasoning and activity are for the client, not for the model.
            _ => {}
        }
    }

    contents
}

/// The text of a user message. Non-text parts are dropped: this agent does not
/// claim to be multimodal.
fn text_of(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Parts(parts) => parts
            .iter()
            .filter_map(|part| match part {
                InputContent::Text(part) => Some(part.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// One decoded `data:` frame.
///
/// Everything is optional and unknown fields are ignored, so a field appearing
/// or moving on the provider's side degrades into missing data rather than a
/// failed run.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StreamFrame {
    #[serde(default)]
    response_id: Option<String>,
    #[serde(default)]
    candidates: Vec<Candidate>,
}

impl StreamFrame {
    /// The frame's text, thinking parts excluded.
    fn text(&self) -> String {
        let Some(candidate) = self.candidates.first() else {
            return String::new();
        };
        candidate
            .content
            .iter()
            .flat_map(|content| &content.parts)
            .filter(|part| part.thought != Some(true))
            .filter_map(|part| part.text.as_deref())
            .collect()
    }

    /// The signature carried by this frame's text, if it carried one.
    ///
    /// Only the last part of a turn has one, and echoing it back is recommended
    /// rather than enforced — unlike the one on a call.
    fn text_signature(&self) -> Option<&str> {
        self.candidates
            .first()?
            .content
            .iter()
            .flat_map(|content| &content.parts)
            .filter(|part| part.text.is_some() && part.thought != Some(true))
            .find_map(|part| signature(part.thought_signature.as_deref()))
    }

    /// The calls the frame asked for. Several arrive together when the model
    /// calls tools in parallel, and each keeps the signature of the part it
    /// came in.
    fn into_calls(self) -> Vec<Call> {
        self.candidates
            .into_iter()
            .next()
            .into_iter()
            .filter_map(|candidate| candidate.content)
            .flat_map(|content| content.parts)
            .filter_map(|part| {
                let Part {
                    function_call,
                    thought_signature,
                    ..
                } = part;
                let call = function_call?;
                Some(Call {
                    name: call.name,
                    args: call.args,
                    id: call.id,
                    signature: signature(thought_signature.as_deref()).map(str::to_owned),
                })
            })
            .collect()
    }
}

#[derive(Debug, Deserialize)]
struct Candidate {
    #[serde(default)]
    content: Option<Content>,
}

#[derive(Debug, Deserialize)]
struct Content {
    #[serde(default)]
    parts: Vec<Part>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Part {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    function_call: Option<FunctionCall>,
    /// Set on a reasoning part. `flash-lite` does not think by default, but a
    /// model that does would otherwise leak its scratchpad into the reply.
    #[serde(default)]
    thought: Option<bool>,
    /// The thinking a 3.x model did to produce this part, opaque and encrypted.
    ///
    /// Note where it lives: on the *part*, beside `functionCall` rather than
    /// inside it. It has to come back in the part it arrived in — see [`Call`].
    #[serde(default)]
    thought_signature: Option<String>,
}

/// The `functionCall` object itself.
#[derive(Debug, Deserialize)]
struct FunctionCall {
    name: String,
    /// A JSON *object*, not the argument string OpenAI-shaped providers stream.
    #[serde(default)]
    args: Value,
    /// Absent on 2.5; 3.x sends `"call_…"`.
    #[serde(default)]
    id: Option<String>,
}

/// One call, as Gemini sends it: fully formed, in a single frame, plus the
/// part-level metadata that has to travel back with it.
///
/// # The signature is not optional on 3.x
///
/// A 3.x model will not accept its own call back without the
/// `thoughtSignature` the part arrived with:
///
/// ```text
/// HTTP 400: Function call is missing a thought_signature in functionCall parts.
/// ```
///
/// 2.5 sends none and requires none, which is the trap: a client written and
/// tested against 2.5 looks finished, and then breaks the first time anything
/// routes it to 3.x. Absent therefore has to stay absent — never an empty
/// string, which is a signature the model did not write.
///
/// For parallel calls the provider attaches the signature to the **first**
/// `functionCall` part only, and the parts must go back in the order they
/// arrived, so this is carried per call rather than per turn.
#[derive(Debug)]
struct Call {
    name: String,
    args: Value,
    id: Option<String>,
    signature: Option<String>,
}

/// Frames an SSE body and decodes each `data:` payload.
///
/// Small enough to write out because the shape being consumed is narrow:
/// Gemini sends one JSON object per event, and — this is the part that surprises
/// — **no `[DONE]` sentinel**. The stream is over when the body is.
struct SseFrames<S> {
    stream: S,
    buffer: Vec<u8>,
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
            ended: false,
        }
    }

    /// The next decoded frame, or `None` at end of body.
    async fn next_frame(&mut self) -> Result<Option<StreamFrame>> {
        loop {
            if let Some(block) = take_block(&mut self.buffer) {
                match payload(&block) {
                    // Comments and keep-alives carry no `data:` line.
                    None => continue,
                    Some(data) => return Ok(Some(parse_frame(&data)?)),
                }
            }

            if self.ended {
                // A body that ends without its final blank line still has one
                // frame in it.
                let rest = std::mem::take(&mut self.buffer);
                return match payload(&rest) {
                    Some(data) => Ok(Some(parse_frame(&data)?)),
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
}

/// Splits off the bytes up to the next blank line, if there is one.
///
/// Gemini terminates frames with `\r\n\r\n`, so a separator search for `\n\n`
/// alone never matches and the whole stream buffers up until EOF. SSE allows
/// any of the three line endings, so all three blank lines are recognised here.
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

fn parse_frame(data: &str) -> Result<StreamFrame> {
    serde_json::from_str(data).map_err(|error| {
        Error::agent(format!(
            "gemini sent a frame this agent could not read: {error}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact bytes of a live tool-call turn, captured from the wire.
    const TOOL_CALL_FRAME: &[u8] = b"data: {\"candidates\": [{\"content\": {\"parts\": [{\"functionCall\": {\"name\": \"get_weather\",\"args\": {\"city\": \"Seoul\"}}}],\"role\": \"model\"},\"finishReason\": \"STOP\",\"index\": 0}],\"modelVersion\": \"gemini-2.5-flash-lite\",\"responseId\": \"igGDauC1F_6C0-kPwsTq-Ag\"}\r\n\r\n";

    fn frame(bytes: &[u8]) -> StreamFrame {
        let mut buffer = bytes.to_vec();
        let block = take_block(&mut buffer).expect("the frame should be terminated");
        let data = payload(&block).expect("the frame should carry a data line");
        parse_frame(&data).expect("the frame should decode")
    }

    #[test]
    fn a_function_call_arrives_whole_in_one_frame() {
        let frame = frame(TOOL_CALL_FRAME);
        assert_eq!(
            frame.response_id.as_deref(),
            Some("igGDauC1F_6C0-kPwsTq-Ag")
        );
        assert_eq!(frame.text(), "");

        let calls = frame.into_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, WEATHER_TOOL);
        assert_eq!(calls[0].args["city"], "Seoul");
        // The id AG-UI needs has to come from somewhere else, and 2.5 asks for
        // no signature back.
        assert!(calls[0].id.is_none());
        assert!(calls[0].signature.is_none());
    }

    #[test]
    fn the_empty_text_part_of_a_final_frame_is_not_an_update() {
        let bytes = b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"\"}],\"role\":\"model\"},\"finishReason\":\"STOP\"}],\"responseId\":\"r1\"}\n\n";
        assert_eq!(frame(bytes).text(), "");
    }

    #[test]
    fn parallel_calls_each_arrive_in_the_same_frame() {
        let bytes = b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"get_weather\",\"args\":{\"city\":\"Seoul\"}}},{\"functionCall\":{\"name\":\"get_weather\",\"args\":{\"city\":\"Oslo\"}}}]}}],\"responseId\":\"r1\"}\n\n";
        let calls = frame(bytes).into_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].args["city"], "Oslo");
    }

    /// A 3.x turn: an id and a part-level `thoughtSignature` on the call.
    ///
    /// Shaped from the documented contract rather than captured, because the
    /// wire run that would have captured it is the one that 400s without this
    /// handling.
    const SIGNED_CALL: &[u8] = b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"get_weather\",\"args\":{\"city\":\"Seoul\"},\"id\":\"call_1\"},\"thoughtSignature\":\"CvsBAdHtim8=\"}],\"role\":\"model\"},\"finishReason\":\"STOP\"}],\"responseId\":\"r1\"}\n\n";

    fn turn_of(bytes: &[u8]) -> Turn {
        let frame = frame(bytes);
        let text = frame.text();
        let text_signature = frame.text_signature().map(str::to_owned);
        Turn {
            text,
            text_signature,
            calls: frame.into_calls(),
        }
    }

    #[test]
    fn a_signature_goes_back_on_the_part_it_arrived_on() {
        let turn = turn_of(SIGNED_CALL);
        assert_eq!(turn.calls[0].signature.as_deref(), Some("CvsBAdHtim8="));

        let part = &model_turn(&turn)["parts"][0];
        // Beside the call, not inside it. A 3.x model rejects the whole request
        // with a 400 when this lands in the wrong place.
        assert_eq!(part["thoughtSignature"], "CvsBAdHtim8=");
        assert!(part["functionCall"]["thoughtSignature"].is_null());
        assert_eq!(part["functionCall"]["id"], "call_1");
    }

    #[test]
    fn a_model_that_sends_no_signature_is_echoed_without_one() {
        let turn = turn_of(TOOL_CALL_FRAME);
        assert!(turn.calls[0].signature.is_none());

        // Absent, not present-and-empty: an empty signature is one the model
        // never wrote.
        let part = &model_turn(&turn)["parts"][0];
        assert!(part.get("thoughtSignature").is_none(), "{part}");
    }

    #[test]
    fn an_empty_signature_counts_as_no_signature() {
        let bytes = b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"get_weather\",\"args\":{}},\"thoughtSignature\":\"\"}]}}],\"responseId\":\"r1\"}\n\n";
        let turn = turn_of(bytes);
        assert!(turn.calls[0].signature.is_none());
        assert!(
            model_turn(&turn)["parts"][0]
                .get("thoughtSignature")
                .is_none()
        );
    }

    /// The provider signs only the first of a parallel batch, and the parts have
    /// to go back in the order they arrived.
    #[test]
    fn parallel_calls_keep_the_one_signature_and_their_order() {
        let bytes = b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"get_weather\",\"args\":{\"city\":\"Seoul\"},\"id\":\"call_1\"},\"thoughtSignature\":\"first-only\"},{\"functionCall\":{\"name\":\"get_weather\",\"args\":{\"city\":\"Oslo\"},\"id\":\"call_2\"}}]}}],\"responseId\":\"r1\"}\n\n";
        let turn = turn_of(bytes);

        let parts = &model_turn(&turn)["parts"];
        assert_eq!(parts[0]["thoughtSignature"], "first-only");
        assert_eq!(parts[0]["functionCall"]["args"]["city"], "Seoul");
        assert!(parts[1].get("thoughtSignature").is_none(), "{}", parts[1]);
        assert_eq!(parts[1]["functionCall"]["args"]["city"], "Oslo");
    }

    #[test]
    fn text_alongside_a_call_keeps_its_own_signature_and_stays_first() {
        let bytes = b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Let me check.\",\"thoughtSignature\":\"text-sig\"},{\"functionCall\":{\"name\":\"get_weather\",\"args\":{\"city\":\"Seoul\"}},\"thoughtSignature\":\"call-sig\"}]}}],\"responseId\":\"r1\"}\n\n";
        let turn = turn_of(bytes);

        let parts = &model_turn(&turn)["parts"];
        assert_eq!(parts[0]["text"], "Let me check.");
        assert_eq!(parts[0]["thoughtSignature"], "text-sig");
        assert_eq!(parts[1]["thoughtSignature"], "call-sig");
    }

    #[test]
    fn an_answer_carries_the_id_its_call_arrived_with() {
        let turn = turn_of(SIGNED_CALL);
        let answer = function_response(&turn.calls[0], &json!({"temperatureC": 21}));
        assert_eq!(answer["functionResponse"]["id"], "call_1");
        assert_eq!(answer["functionResponse"]["name"], WEATHER_TOOL);
        assert_eq!(
            answer["functionResponse"]["response"]["result"]["temperatureC"],
            21
        );
    }

    /// Two frames, split across chunks mid-JSON, terminated the way Gemini
    /// terminates them — and ended by EOF rather than by a sentinel.
    #[tokio::test]
    async fn frames_survive_chunk_boundaries_and_end_at_eof() {
        let body: Vec<std::result::Result<&[u8], std::io::Error>> = vec![
            Ok(b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"It is \"}]}}],\"responseId\":\"r1\"}\r\n\r\ndata: {\"candida"),
            Ok(b"tes\":[{\"content\":{\"parts\":[{\"text\":\"sunny.\"}]},\"finishReason\":\"STOP\"}],\"responseId\":\"r1\"}\r\n\r\n"),
        ];

        let mut frames = SseFrames::new(futures_util::stream::iter(body));
        let mut texts = Vec::new();
        while let Some(frame) = frames.next_frame().await.expect("frames should decode") {
            assert_eq!(frame.response_id.as_deref(), Some("r1"));
            texts.push(frame.text());
        }
        assert_eq!(texts, ["It is ", "sunny."]);
    }

    #[test]
    fn a_thinking_part_is_not_reply_text() {
        let bytes = b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hmm\",\"thought\":true},{\"text\":\"Hello\"}]}}],\"responseId\":\"r1\"}\n\n";
        assert_eq!(frame(bytes).text(), "Hello");
    }

    #[test]
    fn schemas_are_translated_into_the_v1beta_dialect() {
        let schema = to_gemini_schema(&weather_tool().parameters);
        assert_eq!(schema["type"], "OBJECT");
        assert_eq!(schema["properties"]["city"]["type"], "STRING");
        assert_eq!(schema["required"], json!(["city"]));
    }

    #[test]
    fn keywords_v1beta_rejects_are_dropped() {
        let schema = to_gemini_schema(&json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "additionalProperties": false,
            "properties": {"tags": {"type": "array", "items": {"type": "string"}}},
        }));
        assert_eq!(schema["type"], "OBJECT");
        assert!(schema.get("$schema").is_none());
        assert!(schema.get("additionalProperties").is_none());
        assert_eq!(schema["properties"]["tags"]["items"]["type"], "STRING");
    }

    #[test]
    fn a_tool_result_is_matched_to_its_call_by_name() {
        let messages = vec![
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

        let contents = contents_of(&messages);
        assert_eq!(contents.len(), 3);
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(
            contents[1]["parts"][0]["functionCall"]["name"],
            WEATHER_TOOL
        );
        assert_eq!(
            contents[2]["parts"][0]["functionResponse"]["name"],
            WEATHER_TOOL
        );
        assert_eq!(
            contents[2]["parts"][0]["functionResponse"]["response"]["result"]["temperatureC"],
            21
        );
    }

    #[test]
    fn the_key_never_reaches_a_debug_line() {
        let rendered = format!("{:?}", GeminiAgent::new("super-secret-key"));
        assert!(!rendered.contains("super-secret-key"), "{rendered}");
    }
}
