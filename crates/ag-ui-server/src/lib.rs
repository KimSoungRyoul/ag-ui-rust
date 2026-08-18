//! Host an [AG-UI] agent in Rust.
//!
//! AG-UI is the protocol between a user-facing application and an agent
//! backend: a POST carrying [`RunAgentInput`](ag_ui_core::RunAgentInput),
//! answered by a stream of typed events. This crate is the server half —
//! implement [`Agent`], hand it to [`run()`], and you have a stream a transport
//! can serialize. [`ag-ui-axum`] mounts it on a router; nothing here depends on
//! a web framework, an executor or an LLM client.
//!
//! ```
//! use ag_ui_core::{Event, EventType, RunAgentInput, RunOutcome};
//! use ag_ui_server::{Agent, Result, RunContext, run};
//! use futures_util::StreamExt;
//! use serde::{Deserialize, Serialize};
//!
//! /// State the client mirrors and the agent updates.
//! #[derive(Default, Serialize, Deserialize)]
//! struct Draft {
//!     revision: u32,
//!     title: String,
//! }
//!
//! struct Editor;
//!
//! impl Agent for Editor {
//!     type State = Draft;
//!
//!     async fn run(&self, ctx: &mut RunContext<Draft>) -> Result<RunOutcome> {
//!         // A step brackets a phase of the run. Its guard emits
//!         // STEP_FINISHED on drop, so an early `?` cannot skip it.
//!         let mut step = ctx.step("draft")?;
//!
//!         // Reasoning the client can render, in its own REASONING_* block.
//!         step.think("The user wants a title.")?;
//!
//!         // A message streams as TEXT_MESSAGE_START / _CONTENT* / _END.
//!         let mut message = step.assistant_message()?;
//!         message.delta("Naming it ")?;
//!         message.delta("\"Q3 plan\".")?;
//!         message.end()?;
//!
//!         // Publishing state diffs against the last snapshot and sends
//!         // whichever of STATE_SNAPSHOT / STATE_DELTA is smaller.
//!         step.update_state(|draft| {
//!             draft.revision += 1;
//!             draft.title = "Q3 plan".into();
//!         })?;
//!
//!         drop(step); // or just let it fall out of scope
//!         Ok(RunOutcome::Success)
//!     }
//! }
//!
//! # let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
//! # rt.block_on(async {
//! let input = RunAgentInput::new("thread-1", "run-1");
//! let events: Vec<Event> = run(Editor, input)
//!     .map(|event| event.expect("the stream should not break"))
//!     .collect()
//!     .await;
//!
//! let types: Vec<EventType> = events.iter().map(Event::event_type).collect();
//! assert_eq!(
//!     types,
//!     [
//!         EventType::RunStarted,
//!         EventType::StepStarted,
//!         EventType::ReasoningStart,
//!         EventType::ReasoningMessageStart,
//!         EventType::ReasoningMessageContent,
//!         EventType::ReasoningMessageEnd,
//!         EventType::ReasoningEnd,
//!         EventType::TextMessageStart,
//!         EventType::TextMessageContent,
//!         EventType::TextMessageContent,
//!         EventType::TextMessageEnd,
//!         EventType::StateSnapshot,
//!         EventType::StepFinished,
//!         EventType::RunFinished,
//!     ]
//! );
//! # });
//! ```
//!
//! # The four things that shape this API
//!
//! **Protocol misuse should not compile.** Event ordering is enforced by
//! [typestate handles](emit) that borrow the run context mutably, so
//! interleaving two messages is a borrow-check error. The handles emit their
//! terminating event on `Drop`, so it cannot be forgotten. What the borrow
//! checker cannot catch — raw [`emit`](RunContext::emit) calls — a runtime
//! [ordering verifier](verify) catches, on by default.
//!
//! **The emit path is synchronous.** `Drop` cannot be async, so a handle cannot
//! `await` while emitting its terminator: `msg.delta(text)?` takes no `.await`.
//! Emitters push into an unbounded channel and the transport drains it.
//!
//! **Executor-agnostic.** `futures` primitives throughout, no tokio in the
//! dependency list, no `spawn`. [`CancellationToken`] is an `AtomicBool` and a
//! waker list rather than `tokio_util`'s. Polling the stream is what runs the
//! agent.
//!
//! **One extension point.** Everything that observes or rewrites the stream is
//! a [`StreamTransformer`]. There is no parallel builder of callbacks; the
//! hooks other SDKs expose that way are built-in transformers here —
//! [`FilterToolCalls`], [`ToolResultToState`].
//!
//! # Human in the loop
//!
//! Return [`RunOutcome::Interrupt`](ag_ui_core::RunOutcome::Interrupt) to pause
//! a run. The client answers, and the next request carries the answers in
//! [`RunContext::resume`]:
//!
//! ```
//! # use ag_ui_core::{Interrupt, ResumeStatus, RunOutcome};
//! # use ag_ui_server::{Agent, Result, RunContext};
//! # struct Approver;
//! impl Agent for Approver {
//!     type State = ();
//!
//!     async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
//!         match ctx.resume_for("delete-everything") {
//!             None => Ok(RunOutcome::interrupt(vec![Interrupt::new(
//!                 "delete-everything",
//!                 "tool_approval",
//!             )])),
//!             Some(answer) if answer.status == ResumeStatus::Resolved => {
//!                 ctx.say("Done.")?;
//!                 Ok(RunOutcome::Success)
//!             }
//!             Some(_) => {
//!                 ctx.say("Cancelled.")?;
//!                 Ok(RunOutcome::Success)
//!             }
//!         }
//!     }
//! }
//! ```
//!
//! # Features
//!
//! - `verify` *(default)* — the ordering state machine. Off, the whole
//!   verifier is a zero-sized type whose checks compile away.
//!
//! [AG-UI]: https://docs.ag-ui.com
//! [`ag-ui-axum`]: https://kimsoungryoul.github.io/ag-ui-rust/api/ag_ui_axum/index.html

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
// See `ag_ui_core`'s lib.rs: marks feature-gated items in the rendered docs.
#![cfg_attr(docsrs, feature(doc_cfg))]

// `readme = "README.md"` in Cargo.toml makes that file the crate's front page
// wherever the package is presented, so its examples are doctested: a stale one
// is a red build rather than a bad first impression. `cfg(doctest)` is what
// keeps this module out of the rendered docs — it compiles the examples rather
// than publishing them.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
mod readme {}

pub mod agent;
pub mod cancel;
pub mod context;
pub mod emit;
pub mod error;
pub mod run;
pub mod state;
pub mod transform;
pub mod verify;

pub use agent::{Agent, AgentState, BoxAgent, DynAgent};
pub use cancel::{CancellationToken, Cancelled};
pub use context::RunContext;
pub use emit::{EventReceiver, MessageHandle, ReasoningHandle, StepGuard, ToolCallHandle};
pub use error::{Error, Result, Rule, VerificationError};
pub use run::{Runner, run};
pub use state::{StateManager, StatePublish};
pub use transform::{FilterToolCalls, StreamTransformer, ToolResultToState, TransformerChain};
