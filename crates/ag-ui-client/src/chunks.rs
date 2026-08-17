//! Normalizing `*_CHUNK` events into explicit start/content/end triples.
//!
//! A producer that cannot bracket its output — most provider adapters, because
//! the upstream API does not tell them a message has ended until the next one
//! begins — sends `TEXT_MESSAGE_CHUNK`, `TOOL_CALL_CHUNK` and
//! `REASONING_MESSAGE_CHUNK` instead. Those events fold start, content and end
//! into one, and they carry their id and name **only on the first chunk**:
//!
//! ```text
//! TEXT_MESSAGE_CHUNK { messageId: "msg-1", delta: "Hel" }
//! TEXT_MESSAGE_CHUNK { delta: "lo" }
//! TEXT_MESSAGE_CHUNK { messageId: "msg-2", delta: "Bye" }   <- msg-1 just ended
//! ```
//!
//! So the id has to be remembered, and the end of one stream is only knowable
//! from the start of the next — or from the end of the run. That bookkeeping is
//! this module.
//!
//! ```
//! use ag_ui_client::chunks::normalize_all;
//! use ag_ui_core::{Event, EventType, MessageId};
//!
//! let events = normalize_all([
//!     Event::text_message_chunk(Some(MessageId::new("msg-1")), Some("Hel".into())),
//!     Event::text_message_chunk(None, Some("lo".into())),
//! ])?;
//!
//! let types: Vec<EventType> = events.iter().map(Event::event_type).collect();
//! assert_eq!(types, [
//!     EventType::TextMessageStart,
//!     EventType::TextMessageContent,
//!     EventType::TextMessageContent,
//!     EventType::TextMessageEnd,
//! ]);
//! # Ok::<(), ag_ui_client::Error>(())
//! ```

use ag_ui_core::{
    Event, MessageId, ReasoningMessageChunkEvent, TextMessageChunkEvent, TextMessageStartEvent,
    ToolCallChunkEvent, ToolCallId, ToolCallStartEvent,
};

use crate::error::{Error, Result};

/// Which family of stream is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Text,
    Tool,
    Reasoning,
}

/// The stream currently in flight.
#[derive(Clone, Debug)]
struct Open {
    kind: Kind,
    /// The message id or tool call id, as a plain string.
    id: String,
    /// Whether this normalizer emitted the opening event and therefore owes the
    /// closing one. Streams the producer opened explicitly close themselves.
    owed: bool,
}

/// Expands chunk events into the explicit events the rest of the protocol is
/// written in.
///
/// Feed every event of a run through it, in order, and apply what comes out.
/// Events that are not chunks pass through untouched — but they are still
/// *observed*, so that an explicitly opened message can absorb a following
/// bare chunk, and so an explicit end is not duplicated.
#[derive(Clone, Debug, Default)]
pub struct ChunkNormalizer {
    open: Option<Open>,
}

impl ChunkNormalizer {
    /// A normalizer with nothing open.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a stream is currently open.
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Expands one event, appending the result to `out`.
    ///
    /// Appends between zero and three events: the synthesized end of the
    /// previous stream, the synthesized start of this one, and the content.
    ///
    /// # Errors
    ///
    /// A chunk that names no stream and has none open, or a tool call chunk
    /// that opens a call without naming the tool, is a protocol violation:
    /// there is no id to attach the payload to, and guessing one would put
    /// text in the wrong message.
    pub fn normalize(&mut self, event: Event, out: &mut Vec<Event>) -> Result<()> {
        match event {
            Event::TextMessageChunk(chunk) => self.text_chunk(chunk, out),
            Event::ToolCallChunk(chunk) => self.tool_chunk(chunk, out),
            Event::ReasoningMessageChunk(chunk) => self.reasoning_chunk(chunk, out),
            other => {
                self.observe(&other, out);
                out.push(other);
                Ok(())
            }
        }
    }

    /// Closes whatever the normalizer opened but never got to end.
    ///
    /// Call this when the transport stream ends. A chunk stream that never
    /// terminates is the normal case, not an error: the last message of a run
    /// has nothing after it to imply its end.
    pub fn finish(&mut self, out: &mut Vec<Event>) {
        self.close(out);
    }

    // ---- chunk families -------------------------------------------------

    /// The stream a chunk belongs to: the one it names, or the open one of its
    /// kind.
    ///
    /// `missing` is the message for the third case — no id on the chunk and
    /// nothing open — which is a protocol violation rather than something to
    /// guess at.
    fn stream_id(&self, kind: Kind, named: Option<&str>, missing: &str) -> Result<String> {
        match named {
            Some(id) => Ok(id.to_owned()),
            None => self
                .current(kind)
                .ok_or_else(|| Error::protocol(missing.to_owned())),
        }
    }

    /// Closes whatever was open, emits `start`, and takes on the debt for the
    /// matching end event.
    fn begin(&mut self, kind: Kind, id: &str, start: Event, out: &mut Vec<Event>) {
        self.close(out);
        out.push(start);
        self.open = Some(Open {
            kind,
            id: id.to_owned(),
            owed: true,
        });
    }

    fn text_chunk(&mut self, chunk: TextMessageChunkEvent, out: &mut Vec<Event>) -> Result<()> {
        let id = MessageId::new(self.stream_id(
            Kind::Text,
            chunk.message_id.as_ref().map(MessageId::as_str),
            "TEXT_MESSAGE_CHUNK carries no messageId and no message is open",
        )?);

        if self.current(Kind::Text).as_deref() != Some(id.as_str()) {
            let mut start = TextMessageStartEvent::new(id.clone(), chunk.role.unwrap_or_default());
            start.name = chunk.name;
            start.base = chunk.base.clone();
            self.begin(Kind::Text, id.as_str(), start.into(), out);
        }

        if let Some(delta) = chunk.delta {
            let mut content = ag_ui_core::TextMessageContentEvent::new(id, delta);
            content.base = chunk.base;
            out.push(content.into());
        }
        Ok(())
    }

    fn tool_chunk(&mut self, chunk: ToolCallChunkEvent, out: &mut Vec<Event>) -> Result<()> {
        let id = ToolCallId::new(self.stream_id(
            Kind::Tool,
            chunk.tool_call_id.as_ref().map(ToolCallId::as_str),
            "TOOL_CALL_CHUNK carries no toolCallId and no call is open",
        )?);

        if self.current(Kind::Tool).as_deref() != Some(id.as_str()) {
            // Checked before anything is emitted: a call with no name cannot be
            // opened, and the previous stream should not be closed on the way
            // to finding that out.
            let Some(name) = chunk.tool_call_name else {
                return Err(Error::protocol(format!(
                    "TOOL_CALL_CHUNK opens tool call {id:?} without a toolCallName"
                )));
            };
            let mut start = ToolCallStartEvent::new(id.clone(), name);
            start.parent_message_id = chunk.parent_message_id;
            start.base = chunk.base.clone();
            self.begin(Kind::Tool, id.as_str(), start.into(), out);
        }

        if let Some(delta) = chunk.delta {
            let mut args = ag_ui_core::ToolCallArgsEvent::new(id, delta);
            args.base = chunk.base;
            out.push(args.into());
        }
        Ok(())
    }

    fn reasoning_chunk(
        &mut self,
        chunk: ReasoningMessageChunkEvent,
        out: &mut Vec<Event>,
    ) -> Result<()> {
        let id = MessageId::new(self.stream_id(
            Kind::Reasoning,
            chunk.message_id.as_ref().map(MessageId::as_str),
            "REASONING_MESSAGE_CHUNK carries no messageId and no reasoning message is open",
        )?);

        if self.current(Kind::Reasoning).as_deref() != Some(id.as_str()) {
            let mut start = ag_ui_core::ReasoningMessageStartEvent::new(id.clone());
            start.base = chunk.base.clone();
            self.begin(Kind::Reasoning, id.as_str(), start.into(), out);
        }

        if let Some(delta) = chunk.delta {
            let mut content = ag_ui_core::ReasoningMessageContentEvent::new(id, delta);
            content.base = chunk.base;
            out.push(content.into());
        }
        Ok(())
    }

    // ---- explicit events ------------------------------------------------

    /// Tracks what an explicit (non-chunk) event does to the open stream.
    ///
    /// Only events that belong to a stream, the one that answers a tool call,
    /// and the two that end a run touch it: a `STATE_DELTA` between two chunks
    /// of one message must not split that message in half.
    fn observe(&mut self, event: &Event, out: &mut Vec<Event>) {
        match event {
            Event::TextMessageStart(e) => {
                self.open_explicit(Kind::Text, e.message_id.as_str(), out)
            }
            Event::TextMessageContent(e) => {
                self.open_explicit(Kind::Text, e.message_id.as_str(), out);
            }
            Event::TextMessageEnd(e) => self.close_explicit(Kind::Text, e.message_id.as_str(), out),

            Event::ToolCallStart(e) => {
                self.open_explicit(Kind::Tool, e.tool_call_id.as_str(), out);
            }
            Event::ToolCallArgs(e) => self.open_explicit(Kind::Tool, e.tool_call_id.as_str(), out),
            Event::ToolCallEnd(e) => self.close_explicit(Kind::Tool, e.tool_call_id.as_str(), out),
            // A result answers a call, so the call is over — and the protocol
            // puts `TOOL_CALL_END` before it. A chunk-streamed call has no end
            // of its own, so without this the result overtakes the terminator
            // this normalizer still owes.
            Event::ToolCallResult(_) => self.close(out),

            Event::ReasoningMessageStart(e) => {
                self.open_explicit(Kind::Reasoning, e.message_id.as_str(), out);
            }
            Event::ReasoningMessageContent(e) => {
                self.open_explicit(Kind::Reasoning, e.message_id.as_str(), out);
            }
            Event::ReasoningMessageEnd(e) => {
                self.close_explicit(Kind::Reasoning, e.message_id.as_str(), out);
            }
            // A reasoning block closing implies its messages have closed.
            Event::ReasoningEnd(_) | Event::RunFinished(_) | Event::RunError(_) => self.close(out),
            _ => {}
        }
    }

    /// An explicit event for a stream: closes a different open stream, then
    /// takes ownership of this one. Nothing is owed — a stream the producer
    /// opened, the producer ends.
    ///
    /// An explicit event for the stream *this* normalizer opened leaves the
    /// debt in place: the end is still owed until the producer sends one.
    fn open_explicit(&mut self, kind: Kind, id: &str, out: &mut Vec<Event>) {
        if self.current(kind).as_deref() != Some(id) {
            self.close(out);
            self.open = Some(Open {
                kind,
                id: id.to_owned(),
                owed: false,
            });
        }
    }

    fn close_explicit(&mut self, kind: Kind, id: &str, out: &mut Vec<Event>) {
        if self.current(kind).as_deref() == Some(id) {
            self.open = None;
        } else {
            self.close(out);
        }
    }

    // ---- bookkeeping ----------------------------------------------------

    /// The id of the open stream, if it is of this kind.
    fn current(&self, kind: Kind) -> Option<String> {
        self.open
            .as_ref()
            .filter(|open| open.kind == kind)
            .map(|open| open.id.clone())
    }

    /// Emits the end event for the open stream, if this normalizer owes one.
    fn close(&mut self, out: &mut Vec<Event>) {
        let Some(open) = self.open.take() else {
            return;
        };
        if !open.owed {
            return;
        }
        out.push(match open.kind {
            Kind::Text => Event::text_message_end(MessageId::new(open.id)),
            Kind::Tool => Event::tool_call_end(ToolCallId::new(open.id)),
            Kind::Reasoning => Event::reasoning_message_end(MessageId::new(open.id)),
        });
    }
}

/// Normalizes a whole run in one call, closing anything left open at the end.
///
/// The streaming form is [`ChunkNormalizer`]; this is the convenience for
/// tests, recorded streams, and anything else that has all the events already.
pub fn normalize_all(events: impl IntoIterator<Item = Event>) -> Result<Vec<Event>> {
    let mut normalizer = ChunkNormalizer::new();
    let mut out = Vec::new();
    for event in events {
        normalizer.normalize(event, &mut out)?;
    }
    normalizer.finish(&mut out);
    Ok(out)
}
