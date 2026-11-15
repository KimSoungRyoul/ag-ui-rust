//! Getting events from somewhere.
//!
//! Everything else in this crate is synchronous. This is the one layer that
//! talks to the outside world, and the only place `async` appears — which is
//! what lets the rest of the crate run under any executor, or none.
//!
//! [`Transport`] is deliberately small: hand it a [`RunAgentInput`], get back a
//! stream of [`Event`]s. Implementations shipped here:
//!
//! - [`sse`] — the `text/event-stream` decoder every HTTP transport needs.
#![cfg_attr(
    feature = "http",
    doc = "- [`http`] *(feature `http`)* — [`HttpTransport`], backed by `reqwest`."
)]
#![cfg_attr(
    not(feature = "http"),
    doc = "- `http` *(feature `http`, off in this build)* — `HttpTransport`, backed by `reqwest`."
)]
//! - [`replay`] — [`ReplayTransport`], which serves a scripted list of events
//!   and records what was sent to it. Tests use it; so does the doc example.
//!
//! A wasm frontend, an in-process agent, a websocket, a recorded fixture: each
//! is an `impl Transport`, and nothing above this module changes.

pub mod replay;
pub mod sse;

#[cfg(feature = "http")]
pub mod http;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::{Event, RunAgentInput};
use futures_core::Stream;
use futures_util::StreamExt;
use serde_json::Value;

use crate::client::error::{Error, Result};

pub use replay::ReplayTransport;
pub use sse::{SseDecoder, SseFrame, decode_events, decode_raw_events};

#[cfg(feature = "http")]
pub use http::{HttpTransport, HttpTransportBuilder};

/// A boxed stream of events, as a transport hands it over.
///
/// `Send` everywhere except wasm, where the browser APIs a transport would be
/// built on are single-threaded and not `Send` at all. Requiring it there would
/// make the wasm case — the reason this crate abstracts the transport in the
/// first place — impossible to satisfy.
#[cfg(not(target_family = "wasm"))]
pub type EventStream = Pin<Box<dyn Stream<Item = Result<Event>> + Send>>;

/// A boxed stream of events, as a transport hands it over.
#[cfg(target_family = "wasm")]
pub type EventStream = Pin<Box<dyn Stream<Item = Result<Event>>>>;

/// Raw JSON events before the common compatibility and enforcement boundary.
#[cfg(not(target_family = "wasm"))]
pub type RawEventStream = Pin<Box<dyn Stream<Item = Result<Value>> + Send>>;

/// Raw JSON events before the common compatibility and enforcement boundary.
#[cfg(target_family = "wasm")]
pub type RawEventStream = Pin<Box<dyn Stream<Item = Result<Value>>>>;

/// The future [`Transport::run`] returns: connecting, before any event arrives.
#[cfg(not(target_family = "wasm"))]
pub type TransportFuture = Pin<Box<dyn Future<Output = Result<EventStream>> + Send>>;

/// The future [`Transport::run`] returns: connecting, before any event arrives.
#[cfg(target_family = "wasm")]
pub type TransportFuture = Pin<Box<dyn Future<Output = Result<EventStream>>>>;

/// The future connecting to a raw event stream.
#[cfg(not(target_family = "wasm"))]
pub type RawTransportFuture = Pin<Box<dyn Future<Output = Result<RawEventStream>> + Send>>;

/// The future connecting to a raw event stream.
#[cfg(target_family = "wasm")]
pub type RawTransportFuture = Pin<Box<dyn Future<Output = Result<RawEventStream>>>>;

/// Somewhere an agent's events come from.
///
/// # Why the future is `'static`
///
/// A transport is usually held inside a [`Thread`](crate::client::Thread), which
/// mutates its own state as events arrive. If the returned future borrowed the
/// transport, that borrow would live as long as the run and the thread could
/// not touch itself while streaming. So `run` clones what it needs —
/// `reqwest::Client` is explicitly designed for exactly that — and the future
/// stands alone.
pub trait Transport {
    /// Starts a run and connects to its event stream.
    ///
    /// Failing to connect is an error from the future; failing mid-stream is an
    /// error item in the stream.
    fn run(&self, input: RunAgentInput) -> TransportFuture;

    /// Returns raw events so compatibility transforms can run before shared
    /// enforcement. Typed transports may use the default adapter; a transport
    /// receiving wire JSON should override this and preserve unknown fields.
    fn run_raw(&self, input: RunAgentInput) -> RawTransportFuture {
        let connecting = self.run(input);
        Box::pin(async move {
            let events = connecting.await?;
            Ok(boxed_raw_stream(events.map(|event| {
                event.and_then(|event| serde_json::to_value(event).map_err(Error::from))
            })))
        })
    }
}

impl<T: Transport + ?Sized> Transport for &T {
    fn run(&self, input: RunAgentInput) -> TransportFuture {
        (**self).run(input)
    }
    fn run_raw(&self, input: RunAgentInput) -> RawTransportFuture {
        (**self).run_raw(input)
    }
}

impl<T: Transport + ?Sized> Transport for Box<T> {
    fn run(&self, input: RunAgentInput) -> TransportFuture {
        (**self).run(input)
    }
    fn run_raw(&self, input: RunAgentInput) -> RawTransportFuture {
        (**self).run_raw(input)
    }
}

impl<T: Transport + ?Sized> Transport for Arc<T> {
    fn run(&self, input: RunAgentInput) -> TransportFuture {
        (**self).run(input)
    }
    fn run_raw(&self, input: RunAgentInput) -> RawTransportFuture {
        (**self).run_raw(input)
    }
}

/// Boxes a stream into the shape [`Transport::run`] returns.
#[cfg(not(target_family = "wasm"))]
pub fn boxed_stream(stream: impl Stream<Item = Result<Event>> + Send + 'static) -> EventStream {
    Box::pin(stream)
}

/// Boxes a stream into the shape [`Transport::run`] returns.
#[cfg(target_family = "wasm")]
pub fn boxed_stream(stream: impl Stream<Item = Result<Event>> + 'static) -> EventStream {
    Box::pin(stream)
}

/// Boxes a raw JSON stream before shared enforcement.
#[cfg(not(target_family = "wasm"))]
pub fn boxed_raw_stream(
    stream: impl Stream<Item = Result<Value>> + Send + 'static,
) -> RawEventStream {
    Box::pin(stream)
}

/// Boxes a raw JSON stream before shared enforcement.
#[cfg(target_family = "wasm")]
pub fn boxed_raw_stream(stream: impl Stream<Item = Result<Value>> + 'static) -> RawEventStream {
    Box::pin(stream)
}

/// Runs raw wire events through the compatibility and enforcement boundary
/// every transport shares, handing each of its warnings to `on_warning`.
///
/// A warning says what the boundary removed or translated on the way: an
/// unrecognised field, enum member or event, a retired `THINKING_*` event, a
/// deprecated optional `null`, a newer declared protocol version. It arrives
/// before the event it concerns. [`decode_events`], [`Transport::run`] and
/// [`RemoteAgent::run_events`](crate::client::RemoteAgent::run_events) apply
/// the same boundary and discard them; a [`Thread`](crate::client::Thread)
/// reports them as diagnostics. Nothing in this crate prints them.
///
/// ```
/// use ag_ui::client::transport::{decode_raw_events, enforce_events};
/// use futures_util::{StreamExt, stream};
///
/// # futures_util::FutureExt::now_or_never(async {
/// let body = "data: {\"type\":\"RUN_STARTED\",\"threadId\":\"t\",\"runId\":\"r\",\"extra\":1}\n\n";
/// let chunks = stream::iter([Ok::<_, std::io::Error>(body.as_bytes())]);
/// let mut warnings = Vec::new();
/// let events: Vec<_> = enforce_events(decode_raw_events(chunks), |warning| warnings.push(warning))
///     .collect()
///     .await;
/// assert!(events[0].is_ok());
/// assert_eq!(warnings, ["unrecognised property at /extra"]);
/// # }).unwrap();
/// ```
pub fn enforce_events<S, F>(raw: S, on_warning: F) -> impl Stream<Item = Result<Event>>
where
    S: Stream<Item = Result<Value>>,
    F: FnMut(String),
{
    crate::client::enforce::events(raw, on_warning)
}
