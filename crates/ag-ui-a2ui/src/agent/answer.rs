//! One answer, read as it streams, for an app that is both the agent and the
//! renderer.
//!
//! Such an app does not forward messages: it draws from the renderer state
//! itself. What it wants from an answer is where each surface sits among the
//! text, so it can lay the two out in the order the model wrote them.

use super::format::StreamPart;
use super::processor::A2uiRequestProcessor;
use super::state::RendererState;
use super::stream::{AfterRejectedBlock, ResponseStream, StreamOptions};
use crate::error::{Error, Result};
use crate::protocol::message::AgentPayload;

/// A part of a [`StreamedAnswer`].
#[derive(Debug)]
pub enum AnswerPart {
    /// Text the model wrote between two surfaces, or before or after them.
    Text(String),
    /// A surface the answer created, where the model wrote it: its id. The
    /// surface itself is [`RendererState::surface`] of that id.
    Surface(String),
    /// A block the answer rejected, where the model wrote it.
    Rejected(Error),
}

/// One model answer, read as it streams into the state an app renders from.
///
/// The answer reads past a rejected block. It keeps its parts in the order the
/// model wrote them: the text, one part per surface it creates, and an
/// [`AnswerPart::Rejected`] where a block was rejected, followed by the text
/// and surfaces after it. A surface the answer deletes leaves the parts. Text
/// that arrives in pieces is joined into one part until a block comes between.
#[derive(Debug)]
pub struct StreamedAnswer {
    stream: ResponseStream,
    parts: Vec<AnswerPart>,
    /// Whether the last part the stream returned was text, so more text joins
    /// it.
    in_text: bool,
}

impl StreamedAnswer {
    /// Opens the answer to the prompt of `processor`, to be read into `state`.
    ///
    /// # Errors
    ///
    /// As [`A2uiRequestProcessor::open_stream`]: [`Error::Catalog`] when
    /// `state` holds other catalogs than the processor's active ones, and
    /// [`Error::Parse`] when the processor's format cannot be read chunk by
    /// chunk.
    pub fn open(processor: &A2uiRequestProcessor, state: &RendererState) -> Result<Self> {
        let stream = processor.open_stream(
            state,
            StreamOptions {
                after_rejected_block: AfterRejectedBlock::Continue,
                ..StreamOptions::default()
            },
        )?;
        Ok(Self {
            stream,
            parts: Vec::new(),
            in_text: false,
        })
    }

    /// The parts of the answer so far, in the order the model wrote them.
    pub fn parts(&self) -> &[AnswerPart] {
        &self.parts
    }

    /// Whether the answer so far ends inside a block. See
    /// [`ResponseStream::is_in_block`].
    pub fn is_in_block(&self) -> bool {
        self.stream.is_in_block()
    }

    /// Reads the next chunk of the answer into `state`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Parse`] if the answer has been closed. A rejected
    /// block is not an error here: it becomes a part.
    pub fn feed(&mut self, chunk: &str, state: &mut RendererState) -> Result<()> {
        let parts = self.stream.feed(chunk, state)?;
        self.show(parts, state);
        Ok(())
    }

    /// Ends the answer, adding the text held back as a possible tag, and a
    /// part for a block that was still open.
    ///
    /// # Errors
    ///
    /// None today; the signature leaves room for a format whose close can
    /// fail for a reason other than a rejected block.
    pub fn close(&mut self, state: &mut RendererState) -> Result<()> {
        let parts = self.stream.close(state)?;
        self.show(parts, state);
        Ok(())
    }

    fn show(&mut self, parts: Vec<StreamPart>, state: &RendererState) {
        for part in parts {
            match part {
                StreamPart::Text(text) => {
                    match self.parts.last_mut() {
                        Some(AnswerPart::Text(before)) if self.in_text => before.push_str(&text),
                        _ => self.parts.push(AnswerPart::Text(text)),
                    }
                    self.in_text = true;
                }
                StreamPart::Rejected(error) => {
                    self.in_text = false;
                    self.parts.push(AnswerPart::Rejected(error));
                }
                StreamPart::A2ui(messages) => {
                    self.in_text = false;
                    for message in messages {
                        match message.payload {
                            AgentPayload::CreateSurface(create) => {
                                if state.surface(&create.surface_id).is_some() {
                                    self.parts.push(AnswerPart::Surface(create.surface_id));
                                }
                            }
                            AgentPayload::DeleteSurface(delete) => self.parts.retain(|part| {
                                !matches!(part, AnswerPart::Surface(id) if *id == delete.surface_id)
                            }),
                            _ => {}
                        }
                    }
                }
            }
        }
    }
}
