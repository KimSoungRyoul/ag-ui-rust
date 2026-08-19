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
//! use ag_ui::client::{RunEnd, Session, Update, transport::ReplayTransport};
//! use ag_ui::{Event, PatchOperation, TextMessageRole};
//! use futures_util::StreamExt;
//! use serde::Deserialize;
//!
//! /// The agent's state, in your own type.
//! #[derive(Clone, Debug, Deserialize, PartialEq)]
//! struct Weather {
//!     checked: bool,
//! }
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
//! let mut session = Session::new(transport, "thread-1");
//! let mut weather = None;
//! let mut ended = None;
//!
//! let mut run = session.send("what is the weather?");
//! while let Some(update) = run.next().await {
//!     match update {
//!         Update::Message(message) => println!("{:?}", message.change),
//!         // The state arrives already typed — and this is where the type
//!         // comes from, so `Session` needs no turbofish.
//!         Update::State(state) => weather = Some(state),
//!         Update::Error(error) => eprintln!("{error}"),
//!         Update::Done(end) => ended = Some(end),
//!         _ => {}
//!     }
//! }
//! drop(run);
//!
//! assert!(matches!(ended, Some(RunEnd::Success { .. })));
//! assert_eq!(session.messages().len(), 2);
//! assert_eq!(weather, Some(Weather { checked: true }));
//! // The raw JSON is always there too, whether or not it fits the type.
//! assert_eq!(session.raw_state()["checked"], true);
//! # });
//! ```
//!
//! # Two levels
//!
//! [`RemoteAgent`] is the low level:
//! [`agent.run(params)`](RemoteAgent::run) gives you the events exactly as the
//! agent sent them, unassembled. That is what a proxy, a recorder or a bridge
//! to another protocol wants.
//!
//! It is called `RemoteAgent` and not `Agent` because the other half of this
//! SDK already owns that word from the other side:
//! [`crate::serve::Agent`] is the trait you implement to *be* an agent, and an
//! agent that calls another agent imports both.
//!
//! [`crate::serve::Agent`]: https://kimsoungryoul.github.io/ag-ui-rust/api/ag_ui/serve/agent/trait.Agent.html
//!
//! [`Session`] is the high level: a thread, its accumulated messages, and typed
//! state. [`session.send(text)`](Session::send) yields [`Update`]s — "this
//! message grew", "the state changed", "the agent is waiting on you" — with
//! chunk normalization, protocol verification and delta application already
//! done.
//!
//! # Tools are yours to offer
//!
//! AG-UI has no tool discovery and no negotiation. The tool set travels on
//! every request, from the client, and an agent cannot ask for one it was not
//! sent — so offering none to an agent that needs one does not produce a
//! missing-tool error from this crate. It produces the *agent's* own error
//! ("the client offered no add_task tool", or whatever that agent says),
//! arriving as an ordinary failed run, which reads like a bug in the agent and
//! is not one. A client written against no particular agent therefore has to be
//! configured with a tool set the way it is configured with a URL:
//! [`SessionBuilder::tools`], or [`Session::set_tools`] from the next run on.
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
// Gated because a feature list names items the current build may not have, and
// a link to one of those is a rustdoc error rather than a dead link. See
// `doc-features` in CI.
#![cfg_attr(
    feature = "http",
    doc = "- `http` *(default)* — [`HttpTransport`](transport::HttpTransport) and",
    doc = "  [`HttpAgent`], backed by `reqwest`. Disable it for wasm or for a custom",
    doc = "  transport, and the dependency disappears with it."
)]
#![cfg_attr(
    not(feature = "http"),
    doc = "- `http` *(default, off in this build)* — `transport::HttpTransport` and",
    doc = "  `HttpAgent`, backed by `reqwest`. Off, as here, the dependency disappears",
    doc = "  with it and you bring your own transport."
)]
//!
//! [AG-UI]: https://docs.ag-ui.com

pub mod agent;
pub mod apply;
pub mod chunks;
pub mod error;
pub mod interrupts;
pub mod session;
pub mod transport;
pub mod verify;

pub use agent::{RemoteAgent, RunParams};
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
