//! The notation a model writes A2UI in, and the parts an answer is read into.
//!
//! An [`OutputFormat`] is one way of asking a model for A2UI: the instructions
//! that go into the prompt, and how the answer is read back. Everything a
//! format does is about notation. It finds its blocks in an answer, and turns
//! one block into raw A2UI messages. It never checks a message against a
//! catalog or a renderer state; the request processor does that, the same way
//! for every format.
//!
//! This crate implements one format, [`DirectJson`](super::DirectJson): the
//! model writes the messages themselves, as JSON, between `<a2ui-json>` tags.

use std::fmt;

use serde_json::Value;

use super::catalog::CatalogDocument;
use crate::error::{Error, Result};
use crate::protocol::message::AgentMessage;

/// A piece of an answer as the model wrote it, before any block is compiled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawPart {
    /// Conversational text between blocks, trimmed.
    Text(String),
    /// The body of one block, without its tags.
    Block {
        /// The body.
        raw: String,
        /// Whether the block was closed. False for a block the answer ended
        /// inside.
        closed: bool,
    },
}

impl RawPart {
    /// A closed block.
    pub fn block(raw: impl Into<String>) -> Self {
        Self::Block {
            raw: raw.into(),
            closed: true,
        }
    }
}

/// A piece of an answer once it is read: text, or the messages of one block.
///
/// The messages have been checked and applied to the renderer state, in
/// order. They are what the agent sends to the renderer.
#[derive(Debug, Clone, PartialEq)]
pub enum ResponsePart {
    /// Conversational text.
    Text(String),
    /// The A2UI messages of one block.
    A2ui(Vec<AgentMessage>),
}

/// A piece of a streamed answer: a [`ResponsePart`], or a block the stream
/// rejected and read past.
///
/// A later version may add a kind of part, so a `match` on one needs a
/// wildcard arm.
#[derive(Debug)]
#[non_exhaustive]
pub enum StreamPart {
    /// Conversational text.
    Text(String),
    /// A2UI messages that became ready. A message that grows as it arrives is
    /// emitted again, whole, each time a chunk changes it.
    A2ui(Vec<AgentMessage>),
    /// A block that was rejected, in place of the block. Only a stream opened
    /// with [`AfterRejectedBlock::Continue`](super::AfterRejectedBlock) yields
    /// one.
    Rejected(Error),
}

/// Why an answer could not be read, and what was read before the failure.
///
/// `parts` holds the parts made ready before the failing block. For an answer
/// read whole, none of them was applied to the state: it is as it was. For a
/// streamed answer, they were applied and yielded as they arrived, so `parts`
/// holds what the failing call made ready before it failed.
#[derive(Debug)]
pub struct ResponseError {
    /// The failure.
    pub error: Error,
    /// The parts made ready before it.
    pub parts: Vec<ResponsePart>,
}

impl ResponseError {
    pub(crate) fn new(error: Error) -> Self {
        Self {
            error,
            parts: Vec::new(),
        }
    }
}

impl fmt::Display for ResponseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for ResponseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

impl From<ResponseError> for Error {
    fn from(error: ResponseError) -> Self {
        error.error
    }
}

/// One message of a block that is still arriving, as it reads now.
///
/// A later version may add a field, so a reader builds one with
/// [`Candidate::new`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Candidate {
    /// The message as far as it can be read.
    pub message: Value,
    /// Whether the message's text has closed. False for a message the reader
    /// healed: its brackets closed for it, a trailing comma dropped, a string
    /// under a progressive key ended early.
    pub closed: bool,
}

impl Candidate {
    /// A message as far as it can be read, and whether its text has closed.
    pub fn new(message: Value, closed: bool) -> Self {
        Self { message, closed }
    }
}

/// What a reader makes of a chunk.
///
/// A later version may add an event, so a `match` on one needs a wildcard
/// arm. A reader builds the events it yields from the variants as they are.
#[derive(Debug)]
#[non_exhaustive]
pub enum ReaderEvent {
    /// A piece of conversational text, already trimmed as a run.
    Text(String),
    /// The open block as it reads now: its messages, in order.
    Block(Vec<Candidate>),
    /// The open block closed, or the answer ended inside it: its messages, or
    /// why it cannot be read.
    BlockEnd(Result<Vec<Value>>),
}

/// Reads one answer of a format as it arrives, chunk by chunk.
///
/// A reader only reads: it finds the blocks and says what the open one holds
/// so far. It holds the text buffers of one answer, and nothing else.
pub trait AnswerReader: fmt::Debug + Send + Sync {
    /// Reads the next chunk.
    fn feed(&mut self, chunk: &str) -> Vec<ReaderEvent>;

    /// Ends the answer: yields the text held back because it could have
    /// started a tag, and a [`ReaderEvent::BlockEnd`] for a block still open.
    fn close(&mut self) -> Vec<ReaderEvent>;

    /// Whether the answer read so far ends inside a block.
    fn in_block(&self) -> bool;

    /// Whether a component in a [`Candidate`] is whole: its own text has
    /// closed, so no later chunk can change it.
    ///
    /// When it is, a component the catalog refuses is refused for good, and
    /// the stream rejects the block on the chunk that closes the component.
    /// When it is not, the component may still be arriving, and a refusal is
    /// only final once the block closes.
    fn components_are_whole(&self) -> bool {
        false
    }
}

/// One way of asking a model for A2UI, and of reading its answer.
///
/// Everything a format does is about notation: the instructions that go into
/// the prompt, where its blocks begin and end in an answer, and what one block
/// says as raw A2UI messages. It never checks a message against a catalog or
/// a renderer state. The request processor does that, the same way for every
/// format, so a new format adds no second validation path and no second retry
/// loop.
///
/// A format holds configuration only. The buffers of an answer that is still
/// arriving belong to the [`AnswerReader`] it opens, so one format serves any
/// number of answers at once.
///
/// This crate implements one format, [`DirectJson`](super::DirectJson).
pub trait OutputFormat: fmt::Debug + Send + Sync {
    /// A short name for the format, for diagnostics.
    fn name(&self) -> &'static str;

    /// The instructions that tell a model how to write for `catalogs` in this
    /// format, with `examples` rendered the way the model is to write them.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Catalog`] when the catalogs cannot share one prompt:
    /// none was given, or they target different protocol families.
    fn prompt_snippet(
        &self,
        catalogs: &[CatalogDocument],
        examples: &[Vec<AgentMessage>],
    ) -> Result<String>;

    /// Splits a finished answer into its text and its blocks.
    ///
    /// Only the format's framing is looked at: a block whose body is not
    /// readable still comes back as a block.
    fn unwrap(&self, answer: &str) -> Vec<RawPart>;

    /// Writes parts back out as a model would have written them.
    fn wrap(&self, parts: &[RawPart]) -> String;

    /// Whether `answer` holds a block of this format. With `complete`, whether
    /// it holds one that was closed.
    fn has_content(&self, answer: &str, complete: bool) -> bool {
        self.unwrap(answer).iter().any(|part| match part {
            RawPart::Block { closed, .. } => *closed || !complete,
            RawPart::Text(_) => false,
        })
    }

    /// Turns the body of one block into raw A2UI messages.
    ///
    /// Compiling translates the notation and reports what the notation itself
    /// gets wrong. The messages are not checked against a catalog or a
    /// renderer state here.
    ///
    /// `closed` is false for a block the answer ended inside.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Parse`] when the block cannot be read.
    fn compile(&self, block: &str, closed: bool) -> Result<Vec<Value>>;

    /// Writes messages back in the format's notation: the body of one block.
    ///
    /// # Errors
    ///
    /// Returns an error when a message cannot be written in this notation.
    fn decompile(&self, messages: &[AgentMessage]) -> Result<String>;

    /// Opens a reader for one answer as it arrives, or `None` when the format
    /// can only be read whole.
    ///
    /// `wrapped` is false when the whole answer is one block, without tags.
    fn reader(&self, wrapped: bool) -> Option<Box<dyn AnswerReader>> {
        let _ = wrapped;
        None
    }
}
