//! Streaming one text message.

use ag_ui_core::{Event, MessageId, TextMessageRole};

use crate::emit::EventSink;
use crate::error::Result;

/// One open text message.
///
/// Created by [`RunContext::assistant_message`](crate::RunContext::assistant_message).
/// `TEXT_MESSAGE_START` has already gone out by the time you hold one; `Drop`
/// emits `TEXT_MESSAGE_END` if [`end`](Self::end) was not called.
///
/// ```
/// # use ag_ui_core::RunAgentInput;
/// # use ag_ui_server::RunContext;
/// # let (mut ctx, mut events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;
/// let mut message = ctx.assistant_message()?;
/// for word in ["Hello", ", ", "world"] {
///     message.delta(word)?;
/// }
/// message.end()?;
/// assert_eq!(events.drain().len(), 5);
/// # Ok::<(), ag_ui_server::Error>(())
/// ```
#[derive(Debug)]
pub struct MessageHandle<'a> {
    sink: &'a mut EventSink,
    id: MessageId,
    ended: bool,
}

impl<'a> MessageHandle<'a> {
    /// Emits `TEXT_MESSAGE_START` and takes the message.
    ///
    /// Returns `Err` without producing a handle when the start could not be
    /// emitted, so a failed open never leaves a terminator to be emitted for a
    /// message that does not exist.
    pub(crate) fn start(
        sink: &'a mut EventSink,
        id: MessageId,
        role: TextMessageRole,
    ) -> Result<Self> {
        sink.emit(Event::text_message_start(id.clone(), role))?;
        Ok(Self {
            sink,
            id,
            ended: false,
        })
    }

    /// The id every event of this message carries.
    pub fn id(&self) -> &MessageId {
        &self.id
    }

    /// Appends text — `TEXT_MESSAGE_CONTENT`.
    ///
    /// No `.await`: see the [module docs](crate::emit) for why the emit path is
    /// synchronous.
    pub fn delta(&mut self, text: impl Into<String>) -> Result<()> {
        self.sink
            .emit(Event::text_message_content(self.id.clone(), text))
    }

    /// Emits an unrelated event without closing the message.
    ///
    /// For the unordered families — `STATE_*`, `ACTIVITY_*`, `CUSTOM`, `RAW` —
    /// which may legally interleave with a message. Opening a second message
    /// through here is a protocol violation the verifier will reject.
    pub fn emit(&mut self, event: Event) -> Result<()> {
        self.sink.emit(event)
    }

    /// Emits `TEXT_MESSAGE_END` and consumes the handle.
    ///
    /// Only worth calling over letting the handle drop when you want to see the
    /// error: `Drop` cannot report one.
    pub fn end(mut self) -> Result<()> {
        self.ended = true;
        self.sink.emit(Event::text_message_end(self.id.clone()))
    }
}

impl Drop for MessageHandle<'_> {
    fn drop(&mut self) {
        if !self.ended {
            // Nowhere to report a failure to; a dead channel or a cancelled run
            // makes the terminator moot anyway.
            let _ = self.sink.emit(Event::text_message_end(self.id.clone()));
        }
    }
}
