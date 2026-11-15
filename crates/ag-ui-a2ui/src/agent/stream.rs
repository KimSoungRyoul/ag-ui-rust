//! Reading one answer as it streams, and applying what becomes ready.
//!
//! A [`ResponseStream`] is the same for every format. The format's reader
//! finds the blocks and says what the open one holds so far; the stream
//! decides what of that is ready, applies it to the renderer state, and hands
//! it on.

use serde_json::Value;

use super::format::{
    AnswerReader, Candidate, ReaderEvent, ResponseError, ResponsePart, StreamPart,
};
use super::state::RendererState;
use crate::error::{Error, Result};
use crate::protocol::message::AgentMessage;
use crate::protocol::operation::Operation;

/// What a stream does once it rejects a block.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AfterRejectedBlock {
    /// The stream ends: the call that met the block returns the error.
    #[default]
    End,
    /// The stream yields a [`StreamPart::Rejected`] in place of the block,
    /// skips the rest of it, and reads on. The messages the block emitted
    /// before it was rejected stay.
    Continue,
}

/// How a stream reads an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamOptions {
    /// False when the whole answer is one block, without tags: a tool call
    /// argument, say. Defaults to true.
    pub wrapped: bool,
    /// What to do once a block is rejected. Defaults to
    /// [`AfterRejectedBlock::End`].
    pub after_rejected_block: AfterRejectedBlock,
}

impl Default for StreamOptions {
    fn default() -> Self {
        Self {
            wrapped: true,
            after_rejected_block: AfterRejectedBlock::End,
        }
    }
}

/// Reads one streamed answer, chunk by chunk.
///
/// Opened by
/// [`A2uiRequestProcessor::open_stream`](super::A2uiRequestProcessor::open_stream).
/// The stream owns the text buffers of one answer. The renderer state is the
/// caller's, and is passed to every call: pass the state the stream was opened
/// on, each time. The stream is the same for every format: the format's reader
/// finds the blocks, and the stream decides what of them is ready.
///
/// # What is emitted, and when
///
/// - Text outside a block is emitted as it arrives.
/// - A message is emitted, and applied to the state, as soon as it reads and
///   the state accepts it. Only `updateComponents` and `updateDataModel` are
///   emitted while they are still arriving, and again, whole, each time a
///   chunk changes them: a renderer applying each in turn ends up holding what
///   the closed block holds. Any other message waits until its text has
///   closed, and the messages after it wait with it.
/// - A message that reads but that the state does not accept yet is not ready:
///   half a component can parse and still not be a component the catalog
///   describes. It is held, and the block is rejected when it closes if the
///   message is still not accepted.
/// - When a block closes, the messages it still holds are applied and the
///   trees of the surfaces it touched are checked in full.
///
/// # What a failure leaves behind
///
/// A streamed answer cannot be all or nothing: by the time a block is
/// rejected, the caller has been handed the parts before it, and a renderer
/// that was sent them holds them. So the stream keeps to a rule that can be
/// relied on instead: **the state holds exactly the messages that were
/// emitted**, in the order they were emitted, whether [`ResponseStream::feed`]
/// returned them or a [`ResponseError`] carried them. Closing a block that is
/// rejected applies none of the messages the block had not emitted yet.
///
/// A caller that did not forward the parts, or that can tell its renderer to
/// discard them, goes back with [`ResponseStream::rewind`], which puts the
/// state back as it was when the stream was opened.
#[derive(Debug)]
pub struct ResponseStream {
    reader: Box<dyn AnswerReader>,
    continues: bool,
    ended: bool,
    /// The state as it was when the stream was opened.
    opened: RendererState,
    started: bool,
    /// How many messages the blocks before the open one held: what the open
    /// block's first message is numbered in a locator.
    base: usize,
    /// How many messages the open block is known to hold.
    seen: usize,
    /// How many of the open block's messages have closed and been applied.
    done: usize,
    /// The message of the open block that is still arriving, as it was last
    /// applied and emitted.
    growing: Option<Value>,
    /// A message that has closed was not accepted. It will not be on a later
    /// chunk either, so nothing after it is emitted before the block closes.
    stalled: bool,
    /// The open block was rejected, so the rest of it is skipped.
    rejected: bool,
    /// The surfaces the open block touched, in the order it first named them.
    touched: Vec<String>,
}

impl ResponseStream {
    pub(crate) fn new(
        reader: Box<dyn AnswerReader>,
        opened: RendererState,
        after_rejected_block: AfterRejectedBlock,
    ) -> Self {
        Self {
            reader,
            continues: after_rejected_block == AfterRejectedBlock::Continue,
            ended: false,
            opened,
            started: false,
            base: 0,
            seen: 0,
            done: 0,
            growing: None,
            stalled: false,
            rejected: false,
            touched: Vec::new(),
        }
    }

    /// Whether the answer read so far ends inside a block.
    ///
    /// True after an opening tag and before its closing tag. An unwrapped
    /// answer is one block, so its stream is inside it until it ends. False
    /// once the stream has ended.
    ///
    /// A caller that joins the chunks of a paused and resumed answer reads
    /// this to tell whether it may add text between them, such as a space,
    /// which inside a block could break the block.
    pub fn is_in_block(&self) -> bool {
        !self.ended && self.reader.in_block()
    }

    /// Reads the next chunk and returns the parts that became ready. Every
    /// message returned has been applied to `state`.
    ///
    /// # Errors
    ///
    /// When a block is rejected and the stream was opened to end there, the
    /// stream has ended: the error carries the parts this chunk made ready
    /// before the failure, which have been applied to `state` like the parts
    /// of any earlier chunk. Feeding a stream that has ended is
    /// [`Error::Parse`].
    pub fn feed(
        &mut self,
        chunk: &str,
        state: &mut RendererState,
    ) -> std::result::Result<Vec<StreamPart>, ResponseError> {
        if self.ended {
            return Err(ResponseError::new(Error::parse(
                "the response stream has ended",
            )));
        }
        self.begin(state);
        let events = self.reader.feed(chunk);
        self.take(events, state)
    }

    /// Ends the answer and returns the parts still held back.
    ///
    /// Call it after the last chunk: text that could still have started a tag
    /// is held until then, and an unwrapped answer is checked as a whole only
    /// then. Closing a stream that has ended returns nothing.
    ///
    /// # Errors
    ///
    /// When the answer ended inside a block, or the messages it left behind
    /// are rejected, and the stream was opened to end at a rejected block. A
    /// stream that reads past rejected blocks returns a
    /// [`StreamPart::Rejected`] instead.
    pub fn close(
        &mut self,
        state: &mut RendererState,
    ) -> std::result::Result<Vec<StreamPart>, ResponseError> {
        if self.ended {
            return Ok(Vec::new());
        }
        self.begin(state);
        let events = self.reader.close();
        let parts = self.take(events, state);
        self.ended = true;
        parts
    }

    /// Puts `state` back as it was when the stream was opened.
    ///
    /// For a caller that did not send the stream's parts on, or that tells
    /// its renderer to discard them: the messages a stream emits are applied
    /// as they are emitted, and a block rejected later does not take them
    /// back.
    pub fn rewind(&self, state: &mut RendererState) {
        state.clone_from(&self.opened);
    }

    fn begin(&mut self, state: &mut RendererState) {
        if !self.started {
            self.started = true;
            state.bind(self.opened.processor());
            state.begin_answer();
        }
    }

    fn take(
        &mut self,
        events: Vec<ReaderEvent>,
        state: &mut RendererState,
    ) -> std::result::Result<Vec<StreamPart>, ResponseError> {
        let mut parts = Vec::new();
        for event in events {
            let block_ended = matches!(event, ReaderEvent::BlockEnd(_));
            let step = match event {
                ReaderEvent::Text(text) => {
                    parts.push(StreamPart::Text(text));
                    continue;
                }
                // The rest of a rejected block is skipped.
                ReaderEvent::Block(_) | ReaderEvent::BlockEnd(_) if self.rejected => Ok(()),
                ReaderEvent::Block(candidates) => self.emit_ready(&candidates, &mut parts, state),
                ReaderEvent::BlockEnd(messages) => self.finish_block(messages, &mut parts, state),
            };
            if let Err(error) = step {
                if !self.continues {
                    self.ended = true;
                    return Err(ResponseError {
                        error,
                        parts: parts
                            .into_iter()
                            .filter_map(|part| match part {
                                StreamPart::Text(text) => Some(ResponsePart::Text(text)),
                                StreamPart::A2ui(messages) => Some(ResponsePart::A2ui(messages)),
                                StreamPart::Rejected(_) => None,
                            })
                            .collect(),
                    });
                }
                parts.push(StreamPart::Rejected(error));
                self.rejected = true;
            }
            if block_ended {
                self.end_block();
            }
        }
        Ok(parts)
    }

    /// Forgets the block that closed, whether it was read or rejected.
    fn end_block(&mut self) {
        self.base += self.seen.max(self.done);
        self.seen = 0;
        self.done = 0;
        self.growing = None;
        self.stalled = false;
        self.rejected = false;
        self.touched.clear();
    }

    /// Emits the messages of the open block that read and are new or changed.
    fn emit_ready(
        &mut self,
        candidates: &[Candidate],
        parts: &mut Vec<StreamPart>,
        state: &mut RendererState,
    ) -> Result<()> {
        self.seen = self.seen.max(candidates.len());
        let mut ready = Vec::new();
        let outcome = self.apply_ready(candidates, &mut ready, state);
        // The messages applied before a rejected one are emitted with the
        // error, since the state holds them.
        if !ready.is_empty() {
            parts.push(StreamPart::A2ui(ready));
        }
        outcome
    }

    fn apply_ready(
        &mut self,
        candidates: &[Candidate],
        ready: &mut Vec<AgentMessage>,
        state: &mut RendererState,
    ) -> Result<()> {
        for (index, candidate) in candidates.iter().enumerate().skip(self.done) {
            if self.stalled || !candidate.message.is_object() {
                break;
            }
            if !candidate.closed && !may_grow(&candidate.message) {
                // Held until its text closes, or until it has something to
                // apply.
                break;
            }
            match self.check(index, &candidate.message, candidate.closed, state) {
                Ok(emitted) => ready.extend(emitted),
                Err(error) => {
                    if self.reader.components_are_whole() && is_settled(&candidate.message) {
                        // No later chunk can change what was refused.
                        return Err(error);
                    }
                    // Not ready. The block is rejected when it closes if the
                    // message still is not accepted.
                    self.stalled = candidate.closed;
                    break;
                }
            }
            if !candidate.closed {
                // Only the last message read so far can be open.
                break;
            }
            self.done = index + 1;
            self.growing = None;
        }
        Ok(())
    }

    /// Applies the rest of a closed block and checks the surfaces it touched.
    ///
    /// The messages not emitted yet are emitted only if every check passes,
    /// so a rejected block leaves the state holding what was emitted.
    fn finish_block(
        &mut self,
        messages: Result<Vec<Value>>,
        parts: &mut Vec<StreamPart>,
        state: &mut RendererState,
    ) -> Result<()> {
        let messages = messages?;
        self.seen = self.seen.max(messages.len());
        let base = self.base;
        let mut growing = self.growing.take();
        let mut touched = std::mem::take(&mut self.touched);
        let mut ready = Vec::new();
        state.transaction(|state| {
            for (index, message) in messages.iter().enumerate().skip(self.done) {
                // The message that was still arriving, if the renderer
                // already holds it as it now stands.
                if growing.take().is_some_and(|last| last == *message) {
                    continue;
                }
                let applied = state.apply_at(base + index, message)?;
                if !touched.contains(&applied.surface_id) {
                    touched.push(applied.surface_id);
                }
                ready.push(applied.message);
            }
            state.check_trees(&touched)
        })?;
        if !ready.is_empty() {
            parts.push(StreamPart::A2ui(ready));
        }
        Ok(())
    }

    /// Applies the next message of the open block, as it reads now.
    ///
    /// Returns the message to emit, or `None` if the renderer already holds
    /// it as it now stands.
    fn check(
        &mut self,
        index: usize,
        message: &Value,
        closed: bool,
        state: &mut RendererState,
    ) -> Result<Option<AgentMessage>> {
        if self.growing.as_ref() == Some(message) {
            return Ok(None);
        }
        let applied = state.apply_at(self.base + index, message)?;
        if !self.touched.contains(&applied.surface_id) {
            self.touched.push(applied.surface_id);
        }
        self.growing = (!closed).then(|| message.clone());
        Ok(Some(applied.message))
    }
}

/// The field a message grows by while it arrives, for the two operations a
/// renderer applies again as they grow.
fn growing_field(message: &Value) -> Option<&Value> {
    [
        (Operation::UpdateComponents, "components"),
        (Operation::UpdateDataModel, "value"),
    ]
    .into_iter()
    .find_map(|(operation, field)| message.get(operation.as_str())?.get(field))
}

/// Whether a message still arriving may be applied and emitted already: it
/// is one a renderer applies again as it grows, and it holds the field it
/// grows by, with at least one component when that is what it grows by.
fn may_grow(message: &Value) -> bool {
    match growing_field(message) {
        Some(Value::Array(components))
            if message.get(Operation::UpdateComponents.as_str()).is_some() =>
        {
            !components.is_empty()
        }
        Some(_) => message.get(Operation::UpdateDataModel.as_str()).is_some(),
        None => false,
    }
}

/// Whether what an `updateComponents` holds so far is what it will hold: its
/// version and surface are whole, and it has components. Said of a message
/// whose components are only read once they have closed.
fn is_settled(message: &Value) -> bool {
    let Some(body) = message.get(Operation::UpdateComponents.as_str()) else {
        return false;
    };
    message.get("version").is_some_and(Value::is_string)
        && body.get("surfaceId").is_some_and(Value::is_string)
        && body
            .get("components")
            .and_then(Value::as_array)
            .is_some_and(|components| !components.is_empty())
}
