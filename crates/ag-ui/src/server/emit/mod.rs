//! Typestate handles that make protocol misuse a compile error.
//!
//! Every streaming construct in AG-UI is bracketed: `TEXT_MESSAGE_START` …
//! `TEXT_MESSAGE_END`, `TOOL_CALL_START` … `TOOL_CALL_END`, `STEP_STARTED` …
//! `STEP_FINISHED`. Handing an agent three raw `emit` calls per construct means
//! trusting it to close what it opened, in order, on every path including the
//! early return. This module hands out RAII handles instead:
//!
//! - creating a handle emits the opening event;
//! - the handle borrows the [`RunContext`](crate::server::RunContext) mutably, so a
//!   second overlapping handle is a **borrow-check error**, not a runtime
//!   protocol violation;
//! - `Drop` emits the terminator, so forgetting `end()` — or returning `Err`
//!   through a `?` halfway through a message — still produces a well-formed
//!   stream.
//!
//! ```compile_fail,E0499
//! use ag_ui::server::RunContext;
//!
//! fn interleave(ctx: &mut RunContext<()>) {
//!     let mut first = ctx.assistant_message().unwrap();
//!     // error[E0499]: cannot borrow `*ctx` as mutable more than once at a time
//!     let mut second = ctx.assistant_message().unwrap();
//!     first.delta("a").unwrap();
//!     second.delta("b").unwrap();
//! }
//! ```
//!
//! # Why the emit path is synchronous
//!
//! `Drop` cannot be async, so a handle cannot `await` while emitting its
//! terminator. `msg.delta(text)?` therefore does not take `.await`: emitters
//! push into an unbounded channel and the transport drains it. An earlier draft
//! copied `await`-ing emitters from the TypeScript and .NET SDKs; it cannot
//! coexist with the `Drop` guarantee.
//!
//! # The escape hatch
//!
//! [`StepGuard`] dereferences to the run context — a step is a scope, and
//! everything else nests inside it. The three streaming handles deliberately do
//! not, because that is exactly what would let a second message open inside the
//! first. They expose [`emit`](MessageHandle::emit) instead, for the unordered
//! events (state, activity, custom) that may legally interleave with a message.
//!
//! # What an open handle can still reach
//!
//! A handle borrows two *fields* of the run context — the event sink and the
//! state — rather than the context itself. So the state is reachable through
//! the handle ([`state`](ToolCallHandle::state),
//! [`state_mut`](ToolCallHandle::state_mut),
//! [`publish_state`](ToolCallHandle::publish_state)) and a tool call can do its
//! work between its arguments and its result: `STATE_*` is unordered, so a
//! publish inside the brackets is a legal stream.
//!
//! Widening reach, not weakening the rule. The context stays exclusively
//! borrowed for as long as the handle lives, so a second block is still a
//! borrow-check error — including from inside an open call:
//!
//! ```compile_fail,E0499
//! use ag_ui::server::RunContext;
//!
//! fn narrate(ctx: &mut RunContext<()>) {
//!     let mut call = ctx.tool_call("search").unwrap();
//!     // error[E0499]: cannot borrow `*ctx` as mutable more than once at a time
//!     let mut message = ctx.assistant_message().unwrap();
//!     call.args("{}").unwrap();
//! }
//! ```
//!
//! # What has no handle, and why that is the answer
//!
//! Two things an agent may legitimately put on the wire are
//! [`RunContext::emit`](crate::server::RunContext::emit) territory, and the escape
//! hatch is the supported path for both rather than a gap waiting for an API.
//!
//! The `*_CHUNK` family is unbracketed by definition: a chunk carries its own
//! id and needs no start and no end, which is the point — it exists for
//! provider adapters that cannot know a message ended until the next one
//! begins. There is nothing for an RAII handle to close, and wrapping one
//! around a self-contained event would only add a way to get it wrong.
//!
//! Interleaved parallel tool calls are the other. Two open [`ToolCallHandle`]s
//! at once is a borrow-check error *by design*, so a provider streaming
//! `args(a) args(b) args(a) end(a) end(b)` cannot be mirrored handle-for-call.
//! Either accumulate each call and emit it whole once its arguments are
//! complete — what `e2e/src/llm.rs` does, and the only mapping that cannot
//! splice two calls' arguments into each other — or emit the interleaving
//! yourself. The verifier keys everything by id, so it accepts the interleaved
//! stream; what it will not let you do is close a call you never opened.

mod message;
mod reasoning;
mod step;
mod tool;

use crate::Event;
use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures_core::Stream;
use futures_util::StreamExt as _;

use crate::server::cancel::CancellationToken;
use crate::server::error::{Error, Result};
use crate::server::transform::TransformerChain;
use crate::server::verify::Verifier;

pub use message::MessageHandle;
pub use reasoning::ReasoningHandle;
pub use step::StepGuard;
pub use tool::ToolCallHandle;

/// The write end of a run's event stream: transformers, then verification,
/// then the channel.
///
/// Not public: the only way to reach one is through a
/// [`RunContext`](crate::server::RunContext) or a handle, which is what keeps the
/// ordering guarantees intact.
pub(crate) struct EventSink {
    tx: UnboundedSender<Event>,
    chain: TransformerChain,
    verifier: Verifier,
    cancel: CancellationToken,
    /// Whether a terminal event has gone out. Tracked here as well as in the
    /// verifier so that turning the `verify` feature off cannot make the driver
    /// emit a second `RUN_FINISHED`.
    terminated: bool,
}

impl std::fmt::Debug for EventSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventSink")
            .field("transformers", &self.chain.len())
            .field("cancelled", &self.cancel.is_cancelled())
            .finish()
    }
}

impl EventSink {
    pub(crate) fn new(
        tx: UnboundedSender<Event>,
        chain: TransformerChain,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            tx,
            chain,
            verifier: Verifier::new(),
            cancel,
            terminated: false,
        }
    }

    /// Emits one event, unless the run was cancelled.
    ///
    /// Failing every emit after cancellation is what makes cancellation work
    /// without any cooperation from the agent: the next `?` unwinds the run.
    pub(crate) fn emit(&mut self, event: Event) -> Result<()> {
        if self.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        self.emit_forced(event)
    }

    /// Emits one event even after cancellation — used by the run driver for
    /// `RUN_FINISHED` and `RUN_ERROR`, which must go out regardless.
    pub(crate) fn emit_forced(&mut self, event: Event) -> Result<()> {
        if self.chain.is_empty() {
            return self.send(event);
        }
        for event in self.chain.transform(event) {
            self.send(event)?;
        }
        Ok(())
    }

    fn send(&mut self, event: Event) -> Result<()> {
        self.verifier.observe(&event)?;
        self.terminated |= matches!(event, Event::RunFinished(_) | Event::RunError(_));
        self.tx
            .unbounded_send(event)
            .map_err(|_| Error::Disconnected)
    }

    /// Whether a terminal event has already gone out.
    pub(crate) fn is_terminated(&self) -> bool {
        self.terminated
    }

    pub(crate) fn cancel_token(&self) -> &CancellationToken {
        &self.cancel
    }
}

/// The read end of a run's event stream.
///
/// Yielded by [`RunContext::new`](crate::server::RunContext::new) for agents under
/// test. Transports get a [`Stream`] from
/// [`Runner::run`](crate::server::Runner::run) instead.
#[derive(Debug)]
pub struct EventReceiver {
    rx: UnboundedReceiver<Event>,
}

impl EventReceiver {
    pub(crate) fn new(rx: UnboundedReceiver<Event>) -> Self {
        Self { rx }
    }

    /// Takes every event emitted so far without waiting.
    ///
    /// The emit path is synchronous, so after calling an agent's code
    /// everything it emitted is already queued. That makes this the whole
    /// assertion story for a unit test:
    ///
    /// ```
    /// # use ag_ui::{Event, RunAgentInput, TextMessageRole};
    /// # use ag_ui::server::RunContext;
    /// let (mut ctx, mut events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;
    /// ctx.say("hello")?;
    /// assert_eq!(events.drain(), vec![
    ///     Event::text_message_start("r-msg-1", TextMessageRole::Assistant),
    ///     Event::text_message_content("r-msg-1", "hello"),
    ///     Event::text_message_end("r-msg-1"),
    /// ]);
    /// # Ok::<(), ag_ui::server::Error>(())
    /// ```
    pub fn drain(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        while let Ok(event) = self.rx.try_recv() {
            events.push(event);
        }
        events
    }

    /// Closes the channel, so the next emit fails with
    /// [`Error::Disconnected`].
    ///
    /// [`Error::Disconnected`]: crate::server::Error::Disconnected
    pub fn close(&mut self) {
        self.rx.close();
    }
}

impl Stream for EventReceiver {
    type Item = Event;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Event>> {
        self.rx.poll_next_unpin(cx)
    }
}
