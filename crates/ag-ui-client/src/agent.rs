//! The low-level API: start a run, get its events.
//!
//! [`RemoteAgent`] adds almost nothing to [`Transport`] — a request builder, and
//! a stream that flattens connecting into streaming. That is the point. Anything
//! that wants the events *as they were sent* — a proxy, a recorder, a bridge to
//! another protocol, a test — should stay at this level.
//!
//! For a UI, [`Session`](crate::Session) sits on top of this and does the
//! assembling.
//!
//! ```no_run
//! # #[cfg(feature = "http")]
//! # async fn example() -> Result<(), ag_ui_client::Error> {
//! use ag_ui_client::{HttpAgent, RunParams};
//! use futures_util::StreamExt;
//!
//! let agent = HttpAgent::builder("https://example.com/agent")
//!     .header("authorization", "Bearer …")
//!     .build()?;
//!
//! let mut events = agent.run(
//!     RunParams::new("thread-1", "run-1").user("msg-1", "What is the weather?"),
//! );
//!
//! while let Some(event) = events.next().await {
//!     println!("{:?}", event?.event_type());
//! }
//! # Ok(())
//! # }
//! ```

use ag_ui_core::{
    Context, Message, MessageId, ResumeEntry, RunAgentInput, RunId, ThreadId, Tool, UserContent,
};
use futures_util::TryStreamExt;
use serde_json::Value;

use crate::transport::{EventStream, Transport, boxed_stream};

#[cfg(feature = "http")]
use crate::error::Result;
#[cfg(feature = "http")]
use crate::transport::{HttpTransport, HttpTransportBuilder};

/// What to send when starting a run.
///
/// A builder over [`RunAgentInput`]: the two ids are required, everything else
/// has a sensible empty default. `agent.run(…)` takes anything that converts
/// into the input, so a hand-built [`RunAgentInput`] works just as well.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunParams {
    input: RunAgentInput,
}

impl RunParams {
    /// A run in `thread_id`, identified by `run_id`.
    pub fn new(thread_id: impl Into<ThreadId>, run_id: impl Into<RunId>) -> Self {
        Self {
            input: RunAgentInput::new(thread_id, run_id),
        }
    }

    /// Sets the conversation history, oldest first.
    #[must_use]
    pub fn messages(mut self, messages: impl Into<Vec<Message>>) -> Self {
        self.input.messages = messages.into();
        self
    }

    /// Appends one message.
    #[must_use]
    pub fn message(mut self, message: impl Into<Message>) -> Self {
        self.input.messages.push(message.into());
        self
    }

    /// Appends a user message — the common case, spelled short.
    #[must_use]
    pub fn user(self, id: impl Into<MessageId>, content: impl Into<UserContent>) -> Self {
        self.message(Message::user(id, content))
    }

    /// Sets the shared state the agent starts from.
    #[must_use]
    pub fn state(mut self, state: impl Into<Value>) -> Self {
        self.input.state = state.into();
        self
    }

    /// Offers tools for this run.
    #[must_use]
    pub fn tools(mut self, tools: impl Into<Vec<Tool>>) -> Self {
        self.input.tools = tools.into();
        self
    }

    /// Sets the ambient context entries.
    #[must_use]
    pub fn context(mut self, context: impl Into<Vec<Context>>) -> Self {
        self.input.context = context.into();
        self
    }

    /// Sets the passthrough properties, which the protocol never interprets.
    #[must_use]
    pub fn forwarded_props(mut self, props: impl Into<Value>) -> Self {
        self.input.forwarded_props = props.into();
        self
    }

    /// Answers the interrupts a previous run paused on. See
    /// [`crate::interrupts`].
    #[must_use]
    pub fn resume(mut self, entries: impl Into<Vec<ResumeEntry>>) -> Self {
        self.input.resume = Some(entries.into());
        self
    }

    /// Records the run that spawned this one, for nested agents.
    #[must_use]
    pub fn parent_run_id(mut self, run_id: impl Into<RunId>) -> Self {
        self.input.parent_run_id = Some(run_id.into());
        self
    }

    /// The request this describes.
    pub fn into_input(self) -> RunAgentInput {
        self.input
    }
}

impl From<RunParams> for RunAgentInput {
    fn from(params: RunParams) -> Self {
        params.input
    }
}

impl From<RunAgentInput> for RunParams {
    fn from(input: RunAgentInput) -> Self {
        Self { input }
    }
}

/// A remote agent, over any [`Transport`].
///
/// [`HttpAgent`] is this over HTTP; the type is generic so that a wasm
/// transport, an in-process agent, or a recorded fixture substitutes without
/// anything above noticing.
///
/// # Not [`ag_ui_server::Agent`]
///
/// The two crates sit on opposite ends of the same wire, and the word "agent"
/// means the opposite thing at each end, so they do not share a name.
/// [`ag_ui_server::Agent`] is a *trait you implement* to be an agent;
/// `RemoteAgent` is a *handle you hold* onto someone else's. An agent that calls
/// another agent — the composition case — needs both in one file, and
/// `impl Agent for X { … self.upstream: RemoteAgent<_> … }` reads correctly
/// only because they are spelled differently.
///
/// [`ag_ui_server::Agent`]: https://docs.rs/ag-ui-server/latest/ag_ui_server/trait.Agent.html
#[derive(Clone, Debug, Default)]
pub struct RemoteAgent<T> {
    transport: T,
}

impl<T> RemoteAgent<T> {
    /// An agent reached through `transport`.
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    /// The transport underneath.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Unwraps the transport.
    pub fn into_transport(self) -> T {
        self.transport
    }
}

impl<T: Transport> RemoteAgent<T> {
    /// Starts a run and streams its events, exactly as the agent sent them.
    ///
    /// Nothing is normalized, verified or assembled here — chunk events arrive
    /// as chunk events. That is what a proxy wants; a UI wants
    /// [`Session`](crate::Session).
    ///
    /// Connecting is folded into the stream: a transport that cannot reach the
    /// agent yields one error item and ends.
    pub fn run(&self, params: impl Into<RunAgentInput>) -> EventStream {
        let connecting = self.transport.run(params.into());
        boxed_stream(futures_util::stream::once(connecting).try_flatten())
    }
}

/// An agent reached over HTTP.
#[cfg(feature = "http")]
pub type HttpAgent = RemoteAgent<HttpTransport>;

#[cfg(feature = "http")]
impl RemoteAgent<HttpTransport> {
    /// A builder for an agent at `url`.
    pub fn builder(url: impl AsRef<str>) -> HttpAgentBuilder {
        HttpAgentBuilder {
            transport: HttpTransport::builder(url),
        }
    }

    /// An agent at `url`, with default settings.
    ///
    /// # Errors
    ///
    /// [`Error::Config`](crate::Error::Config) when the URL does not parse.
    pub fn http(url: impl AsRef<str>) -> Result<Self> {
        Ok(Self::new(HttpTransport::new(url)?))
    }
}

/// Builds an [`HttpAgent`]: base URL, headers, timeouts.
#[cfg(feature = "http")]
#[derive(Clone, Debug)]
pub struct HttpAgentBuilder {
    transport: HttpTransportBuilder,
}

#[cfg(feature = "http")]
impl HttpAgentBuilder {
    /// Adds a header to every request.
    #[must_use]
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.transport = self.transport.header(name, value);
        self
    }

    /// Adds several headers.
    #[must_use]
    pub fn headers<K, V>(mut self, headers: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        self.transport = self.transport.headers(headers);
        self
    }

    /// Bounds the whole run, streaming included.
    #[must_use]
    pub fn timeout(mut self, timeout: std::time::Duration) -> Self {
        self.transport = self.transport.timeout(timeout);
        self
    }

    /// Bounds connection setup only, leaving the stream unbounded.
    #[must_use]
    pub fn connect_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.transport = self.transport.connect_timeout(timeout);
        self
    }

    /// Uses a caller-supplied `reqwest` client.
    #[must_use]
    pub fn client(mut self, client: reqwest::Client) -> Self {
        self.transport = self.transport.client(client);
        self
    }

    /// Builds the agent.
    ///
    /// # Errors
    ///
    /// [`Error::Config`](crate::Error::Config) when the URL or a header is not
    /// valid.
    pub fn build(self) -> Result<HttpAgent> {
        Ok(RemoteAgent::new(self.transport.build()?))
    }
}
