//! Consume a remote [AG-UI] agent: turn its event stream into messages and
//! state.
//!
//! An AG-UI run arrives as deltas — a message opens, text arrives a fragment at
//! a time, tool arguments accumulate as partial JSON, state moves by RFC 6902
//! patch, and the run may pause to ask a human something. This crate is the
//! consumer half of that protocol: the state machines that fold a stream back
//! into a conversation, the wire-format decoder that feeds them, and two levels
//! of API over the top.
//!
//! ```
//! use ag_ui_client::{RunEnd, Session, Update, transport::ReplayTransport};
//! use ag_ui_core::{Event, PatchOperation, TextMessageRole};
//! use futures_util::StreamExt;
//!
//! # let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
//! # rt.block_on(async {
//! // A transport that replays a scripted run, so this example needs no network.
//! let transport = ReplayTransport::new([
//!     Event::run_started("thread-1", "run-1"),
//!     Event::text_message_start("msg-1", TextMessageRole::Assistant),
//!     Event::text_message_content("msg-1", "It is "),
//!     Event::text_message_content("msg-1", "sunny."),
//!     Event::text_message_end("msg-1"),
//!     Event::state_delta(vec![PatchOperation::add("/checked", true)]),
//!     Event::run_finished_success("thread-1", "run-1"),
//! ]);
//!
//! let mut session = Session::<_>::new(transport, "thread-1");
//! let mut ended = None;
//!
//! let mut run = session.send("what is the weather?");
//! while let Some(update) = run.next().await {
//!     match update {
//!         Update::Message(message) => println!("{:?}", message.change),
//!         Update::State(state) => println!("state is now {state}"),
//!         Update::Error(error) => eprintln!("{error}"),
//!         Update::Done(end) => ended = Some(end),
//!         _ => {}
//!     }
//! }
//! drop(run);
//!
//! assert!(matches!(ended, Some(RunEnd::Success { .. })));
//! assert_eq!(session.messages().len(), 2);
//! assert_eq!(session.raw_state()["checked"], true);
//! # });
//! ```
//!
//! # Two levels
//!
//! [`Agent`] is the low level: [`agent.run(params)`](Agent::run) gives you the
//! events exactly as the agent sent them, unassembled. That is what a proxy, a
//! recorder or a bridge to another protocol wants.
//!
//! [`Session`] is the high level: a thread, its accumulated messages, and typed
//! state. [`session.send(text)`](Session::send) yields [`Update`]s — "this
//! message grew", "the state changed", "the agent is waiting on you" — with
//! chunk normalization, protocol verification and delta application already
//! done.
//!
//! # The pieces underneath
//!
//! - [`apply`] — the event applier. Deltas in, materialised messages and state
//!   out, plus a report of what changed so a view can redraw one row.
//! - [`chunks`] — normalizes `*_CHUNK` events into explicit start/content/end
//!   triples. Chunks carry their id only on the first one, so this stage
//!   remembers.
//! - [`verify`] — the ordering rules, checked client-side as the TypeScript SDK
//!   does. A malformed stream produces one clear error instead of a confused UI.
//! - [`interrupts`] — the human-in-the-loop round trip.
//! - [`transport`] — where events come from: [`Transport`], an SSE decoder, a
//!   `reqwest` client, and a replay transport for tests.
//!
//! # Executor-agnostic, transport-agnostic
//!
//! Only [`transport`] is async. Everything else — application, normalization,
//! verification — is a plain synchronous state machine you can drive from a
//! loop, a test, or an event handler.
//!
//! The one async layer is a trait, so a wasm frontend or a non-tokio runtime
//! substitutes its own. `cargo check --no-default-features` is a CI job
//! precisely to keep that true: it must not pull in `reqwest` or tokio.
//!
//! # Features
//!
//! - `http` *(default)* — [`HttpTransport`](transport::HttpTransport) and
//!   [`HttpAgent`], backed by `reqwest`. Disable it for wasm or for a custom
//!   transport, and the dependency disappears with it.
//!
//! [AG-UI]: https://docs.ag-ui.com

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod agent;
pub mod apply;
pub mod chunks;
pub mod error;
pub mod interrupts;
pub mod session;
pub mod transport;
pub mod verify;

pub use agent::{Agent, RunParams};
pub use apply::{
    Applier, Changed, MessageChange, MessageChangeKind, ReasoningChange, ReasoningChangeKind,
};
pub use chunks::{ChunkNormalizer, normalize_all};
pub use error::{Error, Result};
pub use interrupts::{InterruptExt, ResumeBuilder, interrupts_of, resume_run};
pub use session::{
    MessageUpdate, ReasoningUpdate, RunEnd, RunStream, Session, SessionBuilder, Update,
};
pub use transport::{EventStream, Transport};
pub use verify::{Verifier, verify_all};

#[cfg(feature = "http")]
pub use agent::{HttpAgent, HttpAgentBuilder};
