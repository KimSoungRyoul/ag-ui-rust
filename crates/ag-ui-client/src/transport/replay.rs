//! A transport that replays a scripted list of events.
//!
//! Testing a client against a live agent is slow, flaky, and needs a model. It
//! is also unnecessary: the agent's half of the conversation is just a list of
//! events. [`ReplayTransport`] serves one — and records the
//! [`RunAgentInput`](https://kimsoungryoul.github.io/ag-ui-rust/api/ag_ui_core/input/struct.RunAgentInput.html)s it was handed, which is how a test asserts that a resume
//! carried the right answers.
//!
//! ```
//! use ag_ui_client::transport::ReplayTransport;
//! use ag_ui_core::Event;
//!
//! let transport = ReplayTransport::new([
//!     Event::run_started("thread-1", "run-1"),
//!     Event::run_finished_success("thread-1", "run-1"),
//! ]);
//! ```

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use ag_ui_core::{Event, RunAgentInput};

use crate::error::Error;
use crate::transport::{EventStream, Transport, TransportFuture};

/// A [`Transport`] that answers each run from a script.
///
/// Cloning shares the script and the recording, so a test can keep a handle
/// after handing one to a [`Session`](crate::Session).
#[derive(Clone, Debug, Default)]
pub struct ReplayTransport {
    inner: Arc<Mutex<Script>>,
}

#[derive(Debug, Default)]
struct Script {
    runs: VecDeque<Vec<Event>>,
    requests: Vec<RunAgentInput>,
}

impl ReplayTransport {
    /// A transport that answers the first run with these events, and every
    /// later run with an error.
    pub fn new(events: impl IntoIterator<Item = Event>) -> Self {
        Self::with_runs([events.into_iter().collect::<Vec<_>>()])
    }

    /// A transport that answers each run with the next list in the script.
    ///
    /// This is what a human-in-the-loop round trip needs: the first run pauses
    /// on an interrupt, the second — the resume — carries on.
    pub fn with_runs(runs: impl IntoIterator<Item = Vec<Event>>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Script {
                runs: runs.into_iter().collect(),
                requests: Vec::new(),
            })),
        }
    }

    /// Every request this transport has been handed, in order.
    pub fn requests(&self) -> Vec<RunAgentInput> {
        self.lock().requests.clone()
    }

    /// The most recent request, if there has been one.
    pub fn last_request(&self) -> Option<RunAgentInput> {
        self.lock().requests.last().cloned()
    }

    /// How many runs are left in the script.
    pub fn remaining(&self) -> usize {
        self.lock().runs.len()
    }

    /// A poisoned lock still holds a perfectly good script — a test that
    /// panicked mid-assert should fail on that panic, not on this mutex.
    fn lock(&self) -> MutexGuard<'_, Script> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }
}

impl Transport for ReplayTransport {
    fn run(&self, input: RunAgentInput) -> TransportFuture {
        let mut script = self.lock();
        script.requests.push(input);
        let next = script.runs.pop_front();
        drop(script);

        Box::pin(async move {
            let Some(events) = next else {
                return Err(Error::Transport(
                    "the replay script has no runs left".into(),
                ));
            };
            let stream = futures_util::stream::iter(events.into_iter().map(Ok));
            Ok(Box::pin(stream) as EventStream)
        })
    }
}
