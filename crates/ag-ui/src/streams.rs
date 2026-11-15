//! The open-stream model: what a `*_CHUNK` event continues, and what ends it.
//!
//! `TEXT_MESSAGE_CHUNK`, `TOOL_CALL_CHUNK` and `REASONING_MESSAGE_CHUNK` are a
//! compact spelling of start, content and end. They carry their id and the
//! fields that open an item **only on the first chunk**, and they have no end
//! event, so two things have to be remembered: which item a chunk that names
//! none continues, and when the item it was assembling is over. AG-UI 1.0
//! states both (`basic/patterns/streaming`, "The chunked form"), and this
//! module is the one place they are implemented. Three callers share it:
//!
//! - the client's `ChunkNormalizer`, which expands chunks before verification;
//! - the ordering rules, which judge a chunk as the events it stands for when
//!   one reaches a verifier unexpanded;
//! - the server's transformers, which have to know which call or message a
//!   chunk that names no id continues before they filter or re-attribute it.
//!
//! # Lanes
//!
//! A *lane* is the parent agent's, or one subagent's. A lane assembles at
//! most one chunk stream. That stream ends when:
//!
//! - a chunk opens a different item in the same lane;
//! - a message, tool-call, step, state, custom or reasoning event arrives in
//!   the lane. `RAW`, `ACTIVITY_SNAPSHOT`, `ACTIVITY_DELTA`,
//!   `REASONING_ENCRYPTED_VALUE` and `SUBAGENT_STARTED` stand aside and end
//!   nothing;
//! - `RUN_STARTED`, `RUN_FINISHED`, `RUN_ERROR` or `MESSAGES_SNAPSHOT`
//!   arrives, which ends every lane's;
//! - `SUBAGENT_FINISHED` or `SUBAGENT_ERROR` arrives, which ends that
//!   subagent's.
//!
//! # The two spellings do not mix
//!
//! Only chunks open and continue a chunk stream. An explicit event that names
//! the id a chunk stream is assembling is not a continuation of it: it ends
//! the stream like any other event in the lane, and is then judged by the
//! rules of the explicit form. So an explicit `TEXT_MESSAGE_END` for a
//! chunk-opened message finds the message already closed, and a chunk that
//! names an explicitly opened message opens it a second time. Both are for the
//! ordering rules to reject; this module only reports what the chunk stood
//! for.

// Nothing calls this module yet. The chunk normalizer, the transformers and
// the ordering rules move onto it next, and this goes with them.
#![allow(dead_code)]

use std::fmt;

use crate::{
    Event, MessageId, ReasoningMessageChunkEvent, ReasoningMessageContentEvent,
    ReasoningMessageStartEvent, SubagentRunId, TextMessageChunkEvent, TextMessageContentEvent,
    TextMessageRole, TextMessageStartEvent, ToolCallArgsEvent, ToolCallChunkEvent, ToolCallId,
    ToolCallStartEvent,
};

/// Whose lane: a subagent's, or the parent agent's when `None`.
pub(crate) type Owner = Option<SubagentRunId>;

/// Which family of item a chunk stream assembles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Kind {
    Text,
    Tool,
    Reasoning,
}

impl Kind {
    const fn chunk_name(self) -> &'static str {
        match self {
            Self::Text => "TEXT_MESSAGE_CHUNK",
            Self::Tool => "TOOL_CALL_CHUNK",
            Self::Reasoning => "REASONING_MESSAGE_CHUNK",
        }
    }

    const fn id_name(self) -> &'static str {
        match self {
            Self::Text | Self::Reasoning => "messageId",
            Self::Tool => "toolCallId",
        }
    }

    const fn noun(self) -> &'static str {
        match self {
            Self::Text => "message",
            Self::Tool => "call",
            Self::Reasoning => "reasoning message",
        }
    }
}

/// What a chunk stream was opened with: the fields a continuation may repeat,
/// with the same values only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Opener {
    Text {
        role: TextMessageRole,
        name: Option<String>,
    },
    Tool {
        name: String,
        parent_message_id: Option<MessageId>,
    },
    Reasoning,
}

/// A chunk stream being assembled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Stream {
    pub(crate) kind: Kind,
    /// The message id or tool call id, as a plain string.
    pub(crate) id: String,
    /// The lane the stream is in.
    pub(crate) owner: Owner,
    pub(crate) opener: Opener,
}

impl Stream {
    /// The end event the chunked form never sends, carrying the stream's
    /// attribution.
    fn end(self) -> Event {
        let mut end = match self.kind {
            Kind::Text => Event::text_message_end(MessageId::new(self.id)),
            Kind::Tool => Event::tool_call_end(ToolCallId::new(self.id)),
            Kind::Reasoning => Event::reasoning_message_end(MessageId::new(self.id)),
        };
        if let Some(owner) = self.owner {
            end.set_subagent_run_id(owner);
        }
        end
    }
}

/// Why a chunk cannot be read as the events it stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ChunkError {
    /// It names no id and no chunk stream of its kind is there to continue:
    /// none at all, or none in the lane of the subagent it is attributed to.
    NothingOpen {
        kind: Kind,
        subagent: Option<SubagentRunId>,
    },
    /// It names neither an id nor a subagent, and several subagents are
    /// assembling a stream of its kind.
    Ambiguous { kind: Kind },
    /// It opens a tool call without naming the tool.
    Unnamed { id: String },
    /// It repeats a field its stream was opened with, with another value.
    Conflict {
        kind: Kind,
        id: String,
        field: &'static str,
        repeated: String,
        established: Option<String>,
    },
}

impl fmt::Display for ChunkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NothingOpen { kind, subagent } => {
                write!(
                    f,
                    "{} carries no {} and ",
                    kind.chunk_name(),
                    kind.id_name()
                )?;
                match subagent {
                    Some(id) => write!(f, "subagent {:?} has no {}", id.as_str(), kind.noun())?,
                    None => write!(f, "no {} is", kind.noun())?,
                }
                f.write_str(" open as a chunk stream")
            }
            Self::Ambiguous { kind } => write!(
                f,
                "{} carries no {} and several subagents have a {} open; attribute the chunk",
                kind.chunk_name(),
                kind.id_name(),
                kind.noun()
            ),
            Self::Unnamed { id } => write!(
                f,
                "TOOL_CALL_CHUNK opens tool call {id:?} without a toolCallName"
            ),
            Self::Conflict {
                kind,
                id,
                field,
                repeated,
                established,
            } => {
                write!(
                    f,
                    "{} for {} {id:?} repeats {field} {repeated:?}, which does not match the \
                     open stream's {field} ",
                    kind.chunk_name(),
                    kind.noun()
                )?;
                match established {
                    Some(value) => write!(f, "{value:?}"),
                    None => f.write_str("(absent)"),
                }
            }
        }
    }
}

/// A chunk, reduced to what placing it needs.
struct Chunk<'a> {
    kind: Kind,
    id: Option<&'a str>,
    tag: &'a Owner,
    fields: Fields<'a>,
}

/// The opener fields a chunk carries, each of which may be absent.
enum Fields<'a> {
    Text {
        role: Option<TextMessageRole>,
        name: Option<&'a str>,
    },
    Tool {
        name: Option<&'a str>,
        parent_message_id: Option<&'a MessageId>,
    },
    Reasoning,
}

impl<'a> Chunk<'a> {
    fn text(chunk: &'a TextMessageChunkEvent) -> Self {
        Self {
            kind: Kind::Text,
            id: chunk.message_id.as_deref(),
            tag: &chunk.subagent_run_id,
            fields: Fields::Text {
                role: chunk.role,
                name: chunk.name.as_deref(),
            },
        }
    }

    fn tool(chunk: &'a ToolCallChunkEvent) -> Self {
        Self {
            kind: Kind::Tool,
            id: chunk.tool_call_id.as_deref(),
            tag: &chunk.subagent_run_id,
            fields: Fields::Tool {
                name: chunk.tool_call_name.as_deref(),
                parent_message_id: chunk.parent_message_id.as_ref(),
            },
        }
    }

    fn reasoning(chunk: &'a ReasoningMessageChunkEvent) -> Self {
        Self {
            kind: Kind::Reasoning,
            id: chunk.message_id.as_deref(),
            tag: &chunk.subagent_run_id,
            fields: Fields::Reasoning,
        }
    }
}

/// Where a chunk landed.
struct Placed {
    id: String,
    owner: Owner,
    /// Whether the chunk opened the stream, and so stands for a start event.
    opened: bool,
}

/// Rejects a continuation chunk that repeats an opener's field with another
/// value. One that leaves the field out agrees with anything.
fn agree(
    chunk: &Chunk<'_>,
    id: &str,
    field: &'static str,
    repeated: Option<&str>,
    established: Option<&str>,
) -> Result<(), ChunkError> {
    match repeated {
        Some(repeated) if Some(repeated) != established => Err(ChunkError::Conflict {
            kind: chunk.kind,
            id: id.to_owned(),
            field,
            repeated: repeated.to_owned(),
            established: established.map(str::to_owned),
        }),
        _ => Ok(()),
    }
}

/// The chunk streams being assembled: at most one per lane, in the order they
/// opened.
#[derive(Clone, Debug, Default)]
pub(crate) struct Streams {
    open: Vec<Stream>,
}

impl Streams {
    /// Whether no chunk stream is being assembled.
    pub(crate) fn is_idle(&self) -> bool {
        self.open.is_empty()
    }

    /// Expands one event into the explicit form, appending the result to
    /// `out`: the ends it implies, then for a chunk the start it stands for
    /// and its content, and for anything else the event itself.
    pub(crate) fn expand(&mut self, event: Event, out: &mut Vec<Event>) -> Result<(), ChunkError> {
        match event {
            Event::TextMessageChunk(chunk) => self.text_chunk(chunk, out),
            Event::ToolCallChunk(chunk) => self.tool_chunk(chunk, out),
            Event::ReasoningMessageChunk(chunk) => self.reasoning_chunk(chunk, out),
            other => {
                self.end_for(&other, out);
                out.push(other);
                Ok(())
            }
        }
    }

    /// Appends to `out` the end of every chunk stream an event that is not a
    /// chunk closes. The event itself is not appended.
    pub(crate) fn end_for(&mut self, event: &Event, out: &mut Vec<Event>) {
        self.end(event, &mut |stream| out.push(stream.end()));
    }

    /// Ends whatever is still being assembled. For the end of the transport
    /// stream: the last item of a run has nothing after it to imply its end.
    pub(crate) fn finish(&mut self, out: &mut Vec<Event>) {
        self.close_all(&mut |stream| out.push(stream.end()));
    }

    /// Folds an event into the model without expanding it. A chunk that
    /// cannot be placed changes nothing.
    pub(crate) fn track(&mut self, event: &Event) {
        let mut ignore = |_| {};
        let _ = match event {
            Event::TextMessageChunk(chunk) => self.place(&Chunk::text(chunk), &mut ignore),
            Event::ToolCallChunk(chunk) => self.place(&Chunk::tool(chunk), &mut ignore),
            Event::ReasoningMessageChunk(chunk) => {
                self.place(&Chunk::reasoning(chunk), &mut ignore)
            }
            other => return self.end(other, &mut ignore),
        };
    }

    /// The stream a chunk that names no id continues: the one in the lane it
    /// is attributed to; unattributed, the parent's, else the only one of its
    /// kind.
    pub(crate) fn continued(&self, kind: Kind, tag: &Owner) -> Result<&Stream, ChunkError> {
        let of_kind = |stream: &&Stream| stream.kind == kind;
        if tag.is_some() {
            return self
                .lane(tag)
                .filter(of_kind)
                .ok_or(ChunkError::NothingOpen {
                    kind,
                    subagent: tag.clone(),
                });
        }
        if let Some(stream) = self.lane(&None).filter(of_kind) {
            return Ok(stream);
        }
        let mut candidates = self.open.iter().filter(of_kind);
        match (candidates.next(), candidates.next()) {
            (Some(only), None) => Ok(only),
            (None, _) => Err(ChunkError::NothingOpen {
                kind,
                subagent: None,
            }),
            (Some(_), Some(_)) => Err(ChunkError::Ambiguous { kind }),
        }
    }

    /// The stream assembling this id, whichever lane it is in.
    pub(crate) fn holder(&self, kind: Kind, id: &str) -> Option<&Stream> {
        self.open
            .iter()
            .find(|stream| stream.kind == kind && stream.id == id)
    }

    // ---- chunks ---------------------------------------------------------

    fn text_chunk(
        &mut self,
        chunk: TextMessageChunkEvent,
        out: &mut Vec<Event>,
    ) -> Result<(), ChunkError> {
        let placed = self.place(&Chunk::text(&chunk), &mut |stream| out.push(stream.end()))?;
        let id = MessageId::new(placed.id);
        if placed.opened {
            let mut start = TextMessageStartEvent::new(id.clone(), chunk.role.unwrap_or_default());
            start.name = chunk.name;
            start.base = chunk.base.clone();
            start.subagent_run_id = placed.owner.clone();
            out.push(start.into());
        }
        if let Some(delta) = chunk.delta {
            let mut content = TextMessageContentEvent::new(id, delta);
            content.base = chunk.base;
            content.subagent_run_id = placed.owner;
            out.push(content.into());
        }
        Ok(())
    }

    fn tool_chunk(
        &mut self,
        chunk: ToolCallChunkEvent,
        out: &mut Vec<Event>,
    ) -> Result<(), ChunkError> {
        let placed = self.place(&Chunk::tool(&chunk), &mut |stream| out.push(stream.end()))?;
        let id = ToolCallId::new(placed.id);
        if placed.opened {
            // `place` opened the stream, so the chunk named its tool.
            let name = chunk.tool_call_name.unwrap_or_default();
            let mut start = ToolCallStartEvent::new(id.clone(), name);
            start.parent_message_id = chunk.parent_message_id;
            start.base = chunk.base.clone();
            start.subagent_run_id = placed.owner.clone();
            out.push(start.into());
        }
        if let Some(delta) = chunk.delta {
            let mut args = ToolCallArgsEvent::new(id, delta);
            args.base = chunk.base;
            args.subagent_run_id = placed.owner;
            out.push(args.into());
        }
        Ok(())
    }

    fn reasoning_chunk(
        &mut self,
        chunk: ReasoningMessageChunkEvent,
        out: &mut Vec<Event>,
    ) -> Result<(), ChunkError> {
        let placed = self.place(&Chunk::reasoning(&chunk), &mut |stream| {
            out.push(stream.end());
        })?;
        let id = MessageId::new(placed.id);
        if placed.opened {
            let mut start = ReasoningMessageStartEvent::new(id.clone());
            start.base = chunk.base.clone();
            start.subagent_run_id = placed.owner.clone();
            out.push(start.into());
        }
        if let Some(delta) = chunk.delta {
            let mut content = ReasoningMessageContentEvent::new(id, delta);
            content.base = chunk.base;
            content.subagent_run_id = placed.owner;
            out.push(content.into());
        }
        Ok(())
    }

    /// Finds the stream a chunk continues, or opens the one it names, handing
    /// `ended` whatever its lane was assembling before.
    ///
    /// Nothing changes when the chunk is rejected: a call that cannot be
    /// opened does not close the stream before it on the way to finding out.
    fn place(
        &mut self,
        chunk: &Chunk<'_>,
        ended: &mut impl FnMut(Stream),
    ) -> Result<Placed, ChunkError> {
        let (id, owner) = match chunk.id {
            Some(id) => (id.to_owned(), self.lane_of(chunk.kind, id, chunk.tag)),
            None => {
                let stream = self.continued(chunk.kind, chunk.tag)?;
                (stream.id.clone(), stream.owner.clone())
            }
        };
        let assembling = self
            .lane(&owner)
            .filter(|stream| stream.kind == chunk.kind && stream.id == id);
        if let Some(stream) = assembling {
            match (&stream.opener, &chunk.fields) {
                (
                    Opener::Text { role, name },
                    Fields::Text {
                        role: again,
                        name: named,
                    },
                ) => {
                    let again = again.map(|role| role.as_str());
                    agree(chunk, &id, "role", again, Some(role.as_str()))?;
                    agree(chunk, &id, "name", *named, name.as_deref())?;
                }
                (
                    Opener::Tool {
                        name,
                        parent_message_id,
                    },
                    Fields::Tool {
                        name: again,
                        parent_message_id: parent,
                    },
                ) => {
                    agree(chunk, &id, "toolCallName", *again, Some(name))?;
                    let parent = parent.map(MessageId::as_str);
                    let established = parent_message_id.as_deref();
                    agree(chunk, &id, "parentMessageId", parent, established)?;
                }
                _ => {}
            }
            return Ok(Placed {
                id,
                owner,
                opened: false,
            });
        }
        let opener = match &chunk.fields {
            Fields::Text { role, name } => Opener::Text {
                // An absent role opens an assistant message.
                role: role.unwrap_or_default(),
                name: name.map(str::to_owned),
            },
            Fields::Tool {
                name,
                parent_message_id,
            } => Opener::Tool {
                name: name
                    .ok_or_else(|| ChunkError::Unnamed { id: id.clone() })?
                    .to_owned(),
                parent_message_id: parent_message_id.cloned(),
            },
            Fields::Reasoning => Opener::Reasoning,
        };
        self.close_lane(&owner, ended);
        self.open.push(Stream {
            kind: chunk.kind,
            id: id.clone(),
            owner: owner.clone(),
            opener,
        });
        Ok(Placed {
            id,
            owner,
            opened: true,
        })
    }

    // ---- everything else ------------------------------------------------

    /// Hands `ended` every stream an event that is not a chunk closes: the
    /// module docs list which events close what.
    fn end(&mut self, event: &Event, ended: &mut impl FnMut(Stream)) {
        // An explicit event of one of the three families ends the stream in
        // its lane, even when it names the id that stream is assembling: the
        // two spellings do not mix.
        if let Some((kind, id, tag)) = item(event) {
            let lane = self.lane_of(kind, id, tag);
            return self.close_lane(&lane, ended);
        }
        match event {
            // A result answers a call, so the call is over, whoever was
            // assembling it: a chunk-streamed call has no end of its own, and
            // without this the result would overtake the end it is owed. The
            // result's own lane is the one it is attributed to, since whoever
            // executes a call can differ from whoever requested it.
            Event::ToolCallResult(e) => {
                self.close_id(Kind::Tool, &e.tool_call_id, ended);
                self.close_lane(&e.subagent_run_id, ended);
            }
            // A reasoning block closing implies the message inside it has.
            Event::ReasoningEnd(e) => {
                self.close_id(Kind::Reasoning, &e.message_id, ended);
                self.close_lane(&e.subagent_run_id, ended);
            }
            Event::ReasoningStart(e) => self.close_lane(&e.subagent_run_id, ended),

            // The standalone events that end their lane's stream.
            Event::StateSnapshot(e) => self.close_lane(&e.subagent_run_id, ended),
            Event::StateDelta(e) => self.close_lane(&e.subagent_run_id, ended),
            Event::Custom(e) => self.close_lane(&e.subagent_run_id, ended),
            Event::StepStarted(e) => self.close_lane(&e.subagent_run_id, ended),
            Event::StepFinished(e) => self.close_lane(&e.subagent_run_id, ended),

            // A stream does not outlive the subagent it belongs to.
            Event::SubagentFinished(e) => {
                self.close_lane(&Some(e.subagent_run_id.clone()), ended);
            }
            Event::SubagentError(e) => self.close_lane(&Some(e.subagent_run_id.clone()), ended),

            Event::RunStarted(_)
            | Event::RunFinished(_)
            | Event::RunError(_)
            | Event::MessagesSnapshot(_) => self.close_all(ended),

            // These stand aside from assembly and end nothing.
            Event::Raw(_)
            | Event::ActivitySnapshot(_)
            | Event::ActivityDelta(_)
            | Event::ReasoningEncryptedValue(_)
            | Event::SubagentStarted(_) => {}

            // The explicit events of the three families were handled above,
            // chunks are placed rather than ended for, and the retired
            // THINKING_* family is translated before this stage.
            _ => {}
        }
    }

    // ---- bookkeeping ----------------------------------------------------

    /// The stream a lane is assembling.
    fn lane(&self, owner: &Owner) -> Option<&Stream> {
        self.open.iter().find(|stream| &stream.owner == owner)
    }

    /// The lane an event for `id` is in: the one it is attributed to;
    /// unattributed, the one assembling that id, else the parent's. An
    /// untagged continuation of a subagent's stream is legal on the wire and
    /// must not be read as the parent's.
    fn lane_of(&self, kind: Kind, id: &str, tag: &Owner) -> Owner {
        if tag.is_some() {
            return tag.clone();
        }
        self.holder(kind, id)
            .and_then(|stream| stream.owner.clone())
    }

    fn close_lane(&mut self, owner: &Owner, ended: &mut impl FnMut(Stream)) {
        if let Some(index) = self.open.iter().position(|stream| &stream.owner == owner) {
            ended(self.open.remove(index));
        }
    }

    fn close_id(&mut self, kind: Kind, id: &str, ended: &mut impl FnMut(Stream)) {
        let holds = |stream: &Stream| stream.kind == kind && stream.id == id;
        if let Some(index) = self.open.iter().position(holds) {
            ended(self.open.remove(index));
        }
    }

    fn close_all(&mut self, ended: &mut impl FnMut(Stream)) {
        std::mem::take(&mut self.open).into_iter().for_each(ended);
    }
}

/// The item an explicit start, content or end event belongs to, and the
/// attribution the event carries.
fn item(event: &Event) -> Option<(Kind, &str, &Owner)> {
    Some(match event {
        Event::TextMessageStart(e) => (Kind::Text, e.message_id.as_str(), &e.subagent_run_id),
        Event::TextMessageContent(e) => (Kind::Text, e.message_id.as_str(), &e.subagent_run_id),
        Event::TextMessageEnd(e) => (Kind::Text, e.message_id.as_str(), &e.subagent_run_id),
        Event::ToolCallStart(e) => (Kind::Tool, e.tool_call_id.as_str(), &e.subagent_run_id),
        Event::ToolCallArgs(e) => (Kind::Tool, e.tool_call_id.as_str(), &e.subagent_run_id),
        Event::ToolCallEnd(e) => (Kind::Tool, e.tool_call_id.as_str(), &e.subagent_run_id),
        Event::ReasoningMessageStart(e) => {
            (Kind::Reasoning, e.message_id.as_str(), &e.subagent_run_id)
        }
        Event::ReasoningMessageContent(e) => {
            (Kind::Reasoning, e.message_id.as_str(), &e.subagent_run_id)
        }
        Event::ReasoningMessageEnd(e) => {
            (Kind::Reasoning, e.message_id.as_str(), &e.subagent_run_id)
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EventType;

    fn text(id: Option<&str>, delta: &str) -> Event {
        Event::text_message_chunk(id.map(MessageId::new), Some(delta.to_owned()))
    }

    fn expand(events: impl IntoIterator<Item = Event>) -> Result<Vec<EventType>, ChunkError> {
        let mut streams = Streams::default();
        let mut out = Vec::new();
        for event in events {
            streams.expand(event, &mut out)?;
        }
        streams.finish(&mut out);
        Ok(out.iter().map(Event::event_type).collect())
    }

    #[test]
    fn an_explicit_event_for_the_assembled_id_ends_the_stream_rather_than_joining_it() {
        // The end the chunked form owes comes first; the explicit event that
        // follows is the ordering rules' to judge.
        for explicit in [
            Event::text_message_content("m1", "b"),
            Event::text_message_end("m1"),
            Event::text_message_start("m1", TextMessageRole::Assistant),
        ] {
            let kind = explicit.event_type();
            assert_eq!(
                expand([text(Some("m1"), "a"), explicit]).unwrap(),
                [
                    EventType::TextMessageStart,
                    EventType::TextMessageContent,
                    EventType::TextMessageEnd,
                    kind,
                ]
            );
        }
    }

    #[test]
    fn an_explicitly_opened_item_is_not_a_stream_a_chunk_can_continue() {
        let start = || Event::text_message_start("m1", TextMessageRole::Assistant);
        // A chunk that names no id has no chunk stream to continue.
        assert_eq!(
            expand([start(), text(None, "a")]),
            Err(ChunkError::NothingOpen {
                kind: Kind::Text,
                subagent: None
            })
        );
        // One that names the id opens the item again.
        assert_eq!(
            expand([start(), text(Some("m1"), "a")]).unwrap(),
            [
                EventType::TextMessageStart,
                EventType::TextMessageStart,
                EventType::TextMessageContent,
                EventType::TextMessageEnd,
            ]
        );
    }

    #[test]
    fn a_rejected_chunk_leaves_the_model_as_it_was() {
        let mut streams = Streams::default();
        let mut out = Vec::new();
        streams.expand(text(Some("m1"), "a"), &mut out).unwrap();
        let before = out.len();
        let unnamed = Event::tool_call_chunk(Some("c1".into()), None, Some("{}".into()));
        assert_eq!(
            streams.expand(unnamed, &mut out),
            Err(ChunkError::Unnamed { id: "c1".into() })
        );
        assert_eq!(out.len(), before, "nothing was synthesized on the way");
        assert_eq!(
            streams.holder(Kind::Text, "m1").map(|s| &s.owner),
            Some(&None)
        );
    }

    #[test]
    fn tracking_follows_the_same_model_as_expanding() {
        let events = [
            text(Some("m1"), "a").with_subagent_run_id("s1"),
            Event::custom("note", serde_json::json!(1)),
            Event::tool_call_chunk(Some("c1".into()), Some("search".into()), None),
            Event::custom("note", serde_json::json!(1)).with_subagent_run_id("s1"),
        ];
        let mut tracked = Streams::default();
        let mut expanded = Streams::default();
        for event in events {
            tracked.track(&event);
            expanded.expand(event, &mut Vec::new()).unwrap();
            assert_eq!(tracked.open, expanded.open);
        }
        assert_eq!(tracked.open.len(), 1, "the parent's call is still open");
        assert!(tracked.holder(Kind::Tool, "c1").is_some());
    }

    #[test]
    fn the_errors_read_as_sentences() {
        let nothing = ChunkError::NothingOpen {
            kind: Kind::Text,
            subagent: None,
        };
        assert_eq!(
            nothing.to_string(),
            "TEXT_MESSAGE_CHUNK carries no messageId and no message is open as a chunk stream"
        );
        let subagent = ChunkError::NothingOpen {
            kind: Kind::Tool,
            subagent: Some("s3".into()),
        };
        assert_eq!(
            subagent.to_string(),
            r#"TOOL_CALL_CHUNK carries no toolCallId and subagent "s3" has no call open as a chunk stream"#
        );
    }
}
