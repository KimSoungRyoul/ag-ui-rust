//! Streaming one tool call.

use ag_ui_core::{Event, MessageId, ReasoningEncryptedValueSubtype, ToolCallId};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::emit::EventSink;
use crate::error::Result;

/// One open tool call.
///
/// Created by [`RunContext::tool_call`](crate::RunContext::tool_call).
/// `TOOL_CALL_START` has already gone out; `Drop` emits `TOOL_CALL_END`.
///
/// Arguments stream as text because providers stream them as text — a partial
/// delta is usually not valid JSON. The handle keeps what it emitted, so
/// [`parse_args`](Self::parse_args) can hand you the finished struct to execute
/// against:
///
/// ```
/// # use ag_ui_core::RunAgentInput;
/// # use ag_ui_server::RunContext;
/// # use serde::Deserialize;
/// #[derive(Deserialize)]
/// struct Query { city: String }
///
/// # let (mut ctx, _events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;
/// let mut call = ctx.tool_call("get_weather")?;
/// call.args(r#"{"city":"#)?;          // as the provider streams them
/// call.args(r#""Seoul"}"#)?;
/// let query: Query = call.parse_args()?;
/// assert_eq!(query.city, "Seoul");
/// call.result(r#"{"tempC":21}"#)?;    // emits TOOL_CALL_END then TOOL_CALL_RESULT
/// # Ok::<(), ag_ui_server::Error>(())
/// ```
#[derive(Debug)]
pub struct ToolCallHandle<'a> {
    sink: &'a mut EventSink,
    id: ToolCallId,
    result_message_id: MessageId,
    args: String,
    ended: bool,
}

impl<'a> ToolCallHandle<'a> {
    /// Emits `TOOL_CALL_START`.
    ///
    /// `result_message_id` is allocated up front so the handle can emit
    /// `TOOL_CALL_RESULT` without reaching back into the run context — which is
    /// what keeps a second overlapping handle a borrow-check error.
    pub(crate) fn start(
        sink: &'a mut EventSink,
        id: ToolCallId,
        name: &str,
        parent_message_id: Option<MessageId>,
        result_message_id: MessageId,
    ) -> Result<Self> {
        let mut start = ag_ui_core::ToolCallStartEvent::new(id.clone(), name);
        start.parent_message_id = parent_message_id;
        sink.emit(start.into())?;
        Ok(Self {
            sink,
            id,
            result_message_id,
            args: String::new(),
            ended: false,
        })
    }

    /// The id every event of this call carries.
    pub fn id(&self) -> &ToolCallId {
        &self.id
    }

    /// The id the result message will carry.
    pub fn result_message_id(&self) -> &MessageId {
        &self.result_message_id
    }

    /// Appends a fragment of the argument JSON — `TOOL_CALL_ARGS`.
    pub fn args(&mut self, delta: impl AsRef<str>) -> Result<()> {
        let delta = delta.as_ref();
        self.args.push_str(delta);
        self.sink
            .emit(Event::tool_call_args(self.id.clone(), delta))
    }

    /// Serializes `value` and emits it as the call's arguments in one delta.
    pub fn args_json<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.args(serde_json::to_string(value)?)
    }

    /// The argument JSON emitted so far, unparsed.
    pub fn raw_args(&self) -> &str {
        &self.args
    }

    /// Parses everything emitted through [`args`](Self::args) into `T`.
    ///
    /// Fails while the arguments are still partial, which is the point: call it
    /// once the provider has finished streaming them.
    pub fn parse_args<T: DeserializeOwned>(&self) -> Result<T> {
        Ok(serde_json::from_str(&self.args)?)
    }

    /// Attaches the provider's opaque reasoning signature for this call —
    /// `REASONING_ENCRYPTED_VALUE`.
    pub fn encrypted_value(&mut self, value: impl Into<String>) -> Result<()> {
        self.sink.emit(Event::reasoning_encrypted_value(
            ReasoningEncryptedValueSubtype::ToolCall,
            self.id.clone(),
            value,
        ))
    }

    /// Emits an unrelated event without closing the call. See
    /// [`MessageHandle::emit`](crate::MessageHandle::emit).
    pub fn emit(&mut self, event: Event) -> Result<()> {
        self.sink.emit(event)
    }

    /// Emits `TOOL_CALL_END` and consumes the handle, leaving the call
    /// unanswered.
    ///
    /// Use it when the client executes the tool — a front-end tool's result
    /// arrives as a message on the next request, not from here.
    pub fn end(mut self) -> Result<()> {
        self.ended = true;
        self.sink.emit(Event::tool_call_end(self.id.clone()))
    }

    /// Emits `TOOL_CALL_END` then `TOOL_CALL_RESULT`, and consumes the handle.
    ///
    /// Returns the id of the tool message carrying the result.
    pub fn result(mut self, content: impl Into<String>) -> Result<MessageId> {
        self.ended = true;
        self.sink.emit(Event::tool_call_end(self.id.clone()))?;
        let mut result = ag_ui_core::ToolCallResultEvent::new(
            self.result_message_id.clone(),
            self.id.clone(),
            content,
        );
        result.role = Some(ag_ui_core::ToolResultRole::Tool);
        self.sink.emit(result.into())?;
        Ok(self.result_message_id.clone())
    }

    /// Serializes `value` and reports it as the call's result.
    pub fn result_json<T: Serialize + ?Sized>(self, value: &T) -> Result<MessageId> {
        let content = serde_json::to_string(value)?;
        self.result(content)
    }
}

impl Drop for ToolCallHandle<'_> {
    fn drop(&mut self) {
        if !self.ended {
            let _ = self.sink.emit(Event::tool_call_end(self.id.clone()));
        }
    }
}
