//! Function calls between an agent and a renderer, from A2UI v1.0: how a call
//! ends, and the table of the calls one side has made and waits on.
//!
//! An agent calls a function the renderer runs with `callRendererFunction`
//! and is answered with `rendererFunctionResponse`; a renderer calls a
//! function the agent runs with `callAgentFunction` and is answered with
//! `agentFunctionResponse`. The four messages are in
//! [`message`](crate::message). A response copies the `functionCallId` of its
//! call, which is all that ties the two together.
//!
//! # Where the line is
//!
//! This crate carries a call and routes it. It does not run a function: it
//! holds no handler, awaits nothing, starts no timer and listens to nothing.
//! A call the application has to run is handed to it as a value, and the
//! application hands the outcome back and gets the message to send. Time is
//! an argument: [`PendingCalls`] is told what the time is and says which calls
//! have waited too long.
//!
//! ```
//! use std::time::Duration;
//!
//! use ag_ui_a2ui::message::FunctionResponse;
//! use ag_ui_a2ui::rpc::{FunctionOutcome, PendingCall, PendingCalls, code};
//! use serde_json::json;
//!
//! let mut calls = PendingCalls::new();
//! let clock = Duration::from_secs(100); // any clock that does not go backwards
//! calls.register(PendingCall::new("call-1", "getLocale"), clock)?;
//! calls.register(PendingCall::new("call-2", "getLocation"), clock)?;
//!
//! // The response to the first arrives.
//! let settled = calls.settle(&FunctionResponse::value("call-1", json!("fr"))).unwrap();
//! assert_eq!(settled.outcome, FunctionOutcome::Value(json!("fr")));
//! // A response nothing waits on changes nothing.
//! assert!(calls.settle(&FunctionResponse::value("call-1", json!("fr"))).is_none());
//!
//! // The second waits past the default of thirty seconds.
//! let late = calls.expire(clock + Duration::from_secs(30));
//! let FunctionOutcome::Error(error) = &late[0].outcome else { panic!() };
//! assert_eq!(error.code, code::TIMEOUT);
//! assert_eq!(calls.pending_count(), 0);
//! # Ok::<(), ag_ui_a2ui::rpc::RpcError>(())
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::message::FunctionResponse;

/// The error codes of a function call that this crate writes itself. A
/// response may carry any other string: a function chooses its own
/// (`PROVIDER_NOT_FOUND`), and a later protocol version may add to these.
pub mod code {
    /// The call cannot be made: it names a function its caller may not call,
    /// one that needs a user gesture it does not have, a surface that is not
    /// there, or arguments the function does not take; or it is malformed.
    pub const INVALID_FUNCTION_CALL: &str = "INVALID_FUNCTION_CALL";
    /// Nothing by that name can be called in that catalog.
    pub const UNKNOWN_FUNCTION: &str = "UNKNOWN_FUNCTION";
    /// The function ran and failed.
    pub const EXECUTION_ERROR: &str = "EXECUTION_ERROR";
    /// No response came in the time the call was given.
    pub const TIMEOUT: &str = "TIMEOUT";
    /// The call was given up before a response came: its surface went, or
    /// the table was disposed of.
    pub const CANCELLED: &str = "CANCELLED";
    /// The table was disposed of before the call was made.
    pub const DISPOSED: &str = "DISPOSED";
    /// A call with this `functionCallId` is already waiting.
    pub const DUPLICATE: &str = "DUPLICATE";
    /// As many calls as the table allows are already waiting.
    pub const TOO_MANY_PENDING: &str = "TOO_MANY_PENDING";
    /// The response to the call could not be read.
    pub const UNKNOWN_ERROR: &str = "UNKNOWN_ERROR";
}

/// How long a call waits for its response unless it says otherwise: thirty
/// seconds, the default of the reference agent SDK.
pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// How many calls may wait at once unless the table is told otherwise.
pub const DEFAULT_MAX_PENDING: usize = 64;

/// Why a function call failed: the `error` of a response, and what this crate
/// refuses a call with.
///
/// `code` is a string and not an enum, because a function may answer with a
/// code of its own. The ones this crate writes are in [`code`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct RpcError {
    /// The machine-readable code, such as `INVALID_FUNCTION_CALL`.
    pub code: String,
    /// A sentence for whoever reads the failure.
    pub message: String,
}

impl RpcError {
    /// An error with this code and message.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    /// An [`INVALID_FUNCTION_CALL`](code::INVALID_FUNCTION_CALL).
    pub fn invalid_call(message: impl Into<String>) -> Self {
        Self::new(code::INVALID_FUNCTION_CALL, message)
    }

    /// An [`UNKNOWN_FUNCTION`](code::UNKNOWN_FUNCTION).
    pub fn unknown_function(message: impl Into<String>) -> Self {
        Self::new(code::UNKNOWN_FUNCTION, message)
    }

    /// An [`EXECUTION_ERROR`](code::EXECUTION_ERROR): the function ran and
    /// failed.
    pub fn execution(message: impl Into<String>) -> Self {
        Self::new(code::EXECUTION_ERROR, message)
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for RpcError {}

/// How a function call ended: with the value the function returned, which
/// may be `null`, or with an error.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum FunctionOutcome {
    /// The function returned this.
    Value(Value),
    /// The call failed.
    Error(RpcError),
}

impl FunctionOutcome {
    /// The value, of a call that returned one.
    pub fn value(&self) -> Option<&Value> {
        match self {
            Self::Value(value) => Some(value),
            Self::Error(_) => None,
        }
    }

    /// The error, of a call that failed.
    pub fn error(&self) -> Option<&RpcError> {
        match self {
            Self::Error(error) => Some(error),
            Self::Value(_) => None,
        }
    }
}

/// A call to wait on: what [`PendingCalls::register`] is given.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PendingCall {
    /// The `functionCallId` the response will copy.
    pub function_call_id: String,
    /// The function called, for the sentences of a failure.
    pub name: String,
    /// The surface the call is made on or for, when there is one: what
    /// [`PendingCalls::cancel_surface`] cancels by.
    pub surface_id: Option<String>,
    /// How long this call waits, in place of the table's default.
    pub timeout: Option<Duration>,
}

impl PendingCall {
    /// A call to `name` under this `functionCallId`.
    pub fn new(function_call_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            function_call_id: function_call_id.into(),
            name: name.into(),
            surface_id: None,
            timeout: None,
        }
    }

    /// The surface the call belongs to.
    #[must_use]
    pub fn on_surface(mut self, surface_id: impl Into<String>) -> Self {
        self.surface_id = Some(surface_id.into());
        self
    }

    /// How long this call waits for its response.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

/// A call that has ended, handed over when it ends. The table keeps nothing
/// of it.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SettledCall {
    /// The `functionCallId` of the call.
    pub function_call_id: String,
    /// The function that was called.
    pub name: String,
    /// The surface the call belonged to, when it had one.
    pub surface_id: Option<String>,
    /// How it ended.
    pub outcome: FunctionOutcome,
}

/// What the table holds of a call that waits.
#[derive(Debug, Clone)]
struct Waiting {
    name: String,
    surface_id: Option<String>,
    /// The time at which the call has waited too long, on the caller's clock.
    deadline: Option<Duration>,
    timeout: Option<Duration>,
}

/// The calls one side has made and waits on, by `functionCallId`.
///
/// It is the same table in both directions: an agent keeps one for its
/// `callRendererFunction` calls (`agent::RendererCalls`
/// is built on it), and a renderer host keeps one for its `callAgentFunction`
/// calls.
///
/// # Time
///
/// The table reads no clock. [`PendingCalls::register`] and
/// [`PendingCalls::expire`] are told the time as a [`Duration`] on a clock of
/// the caller's choosing that does not go backwards: the time since the
/// process or the conversation began, say. A call registered at `now` with a
/// timeout of `t` has waited too long once `expire` is called with `now + t`
/// or later. Nothing happens between calls to `expire`, so a caller that
/// wants a timeout to be noticed calls it when the time comes.
///
/// # What it keeps
///
/// Only the calls that wait, and no more than its limit of them
/// ([`DEFAULT_MAX_PENDING`] unless set). A call leaves the table when it
/// ends, and how it ended is handed to the caller at that moment as a
/// [`SettledCall`]. A response for an id nothing waits on, whether unknown,
/// late or sent twice, changes nothing and is not remembered.
#[derive(Debug, Clone)]
pub struct PendingCalls {
    default_timeout: Option<Duration>,
    max_pending: usize,
    waiting: BTreeMap<String, Waiting>,
    disposed: bool,
    /// The number of the next id [`PendingCalls::next_id`] gives.
    next: u64,
}

impl Default for PendingCalls {
    fn default() -> Self {
        Self::new()
    }
}

impl PendingCalls {
    /// An empty table: calls wait [`DEFAULT_CALL_TIMEOUT`], and at most
    /// [`DEFAULT_MAX_PENDING`] of them at once.
    pub fn new() -> Self {
        Self {
            default_timeout: Some(DEFAULT_CALL_TIMEOUT),
            max_pending: DEFAULT_MAX_PENDING,
            waiting: BTreeMap::new(),
            disposed: false,
            next: 1,
        }
    }

    /// How long a call waits unless it sets a timeout of its own. `None`
    /// waits without a limit; a call cannot turn the limit off by itself.
    #[must_use]
    pub fn with_default_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// The most calls that may wait at once. One past it is refused with
    /// [`TOO_MANY_PENDING`](code::TOO_MANY_PENDING).
    #[must_use]
    pub fn with_max_pending(mut self, max_pending: usize) -> Self {
        self.max_pending = max_pending;
        self
    }

    /// An id no waiting call has: `call-1`, `call-2`, and so on. The same
    /// calls made in the same order get the same ids, so a caller that needs
    /// ids no other conversation could guess gives its own.
    pub fn next_id(&mut self) -> String {
        loop {
            let id = format!("call-{}", self.next);
            self.next = self.next.checked_add(1).unwrap_or(1);
            if !self.waiting.contains_key(&id) {
                return id;
            }
        }
    }

    /// Why a call cannot be registered now, when it cannot: under this id,
    /// or with `None` under an id [`PendingCalls::next_id`] is yet to give.
    /// [`PendingCalls::register`] makes the same checks; this is for a caller
    /// that has more to check before it registers.
    ///
    /// # Errors
    ///
    /// As [`PendingCalls::register`].
    pub fn admits(&self, function_call_id: Option<&str>) -> Result<(), RpcError> {
        if self.disposed {
            return Err(RpcError::new(
                code::DISPOSED,
                "The calls were disposed of; no further call can be made.",
            ));
        }
        if function_call_id == Some("") {
            return Err(RpcError::invalid_call("functionCallId must not be empty."));
        }
        if let Some(function_call_id) = function_call_id.filter(|id| self.waiting.contains_key(*id))
        {
            return Err(RpcError::new(
                code::DUPLICATE,
                format!("A call with functionCallId '{function_call_id}' is already pending."),
            ));
        }
        if self.waiting.len() >= self.max_pending {
            return Err(RpcError::new(
                code::TOO_MANY_PENDING,
                format!(
                    "{} function calls are already pending, the most allowed.",
                    self.waiting.len()
                ),
            ));
        }
        Ok(())
    }

    /// Starts waiting on a call made at `now`.
    ///
    /// # Errors
    ///
    /// Returns an [`RpcError`], and registers nothing, when the table was
    /// disposed of ([`DISPOSED`](code::DISPOSED)), the id is empty
    /// ([`INVALID_FUNCTION_CALL`](code::INVALID_FUNCTION_CALL)) or already
    /// waits ([`DUPLICATE`](code::DUPLICATE)), the limit is reached
    /// ([`TOO_MANY_PENDING`](code::TOO_MANY_PENDING)), or the call's timeout is
    /// zero.
    pub fn register(&mut self, call: PendingCall, now: Duration) -> Result<(), RpcError> {
        if call.timeout.is_some_and(|timeout| timeout.is_zero()) {
            return Err(RpcError::invalid_call(format!(
                "The timeout of the call to '{}' must be longer than nothing.",
                call.name
            )));
        }
        self.admits(Some(&call.function_call_id))?;
        let timeout = call.timeout.or(self.default_timeout);
        self.waiting.insert(
            call.function_call_id,
            Waiting {
                name: call.name,
                surface_id: call.surface_id,
                // A deadline past the end of the clock is no deadline.
                deadline: timeout.and_then(|timeout| now.checked_add(timeout)),
                timeout,
            },
        );
        Ok(())
    }

    /// Whether a call under this id waits for its response. Never, once the
    /// table is disposed of.
    pub fn is_pending(&self, function_call_id: &str) -> bool {
        self.waiting.contains_key(function_call_id)
    }

    /// How many calls wait.
    pub fn pending_count(&self) -> usize {
        self.waiting.len()
    }

    /// Whether [`PendingCalls::dispose`] was called.
    pub fn is_disposed(&self) -> bool {
        self.disposed
    }

    fn ended(&mut self, function_call_id: &str, outcome: FunctionOutcome) -> Option<SettledCall> {
        let waiting = self.waiting.remove(function_call_id)?;
        Some(SettledCall {
            function_call_id: function_call_id.to_string(),
            name: waiting.name,
            surface_id: waiting.surface_id,
            outcome,
        })
    }

    /// Ends the call a response answers, and hands it over. `None` when no
    /// call under its `functionCallId` waits: the response is unknown, late or
    /// a second one, and nothing changes.
    pub fn settle(&mut self, response: &FunctionResponse) -> Option<SettledCall> {
        self.ended(&response.function_call_id, response.outcome.clone())
    }

    /// Fails one waiting call with `error`, as when its response could not be
    /// read or the transport lost it. `None` when no such call waits.
    pub fn fail(&mut self, function_call_id: &str, error: RpcError) -> Option<SettledCall> {
        self.ended(function_call_id, FunctionOutcome::Error(error))
    }

    /// Ends every call whose time has come at `now`, each with
    /// [`TIMEOUT`](code::TIMEOUT), in the order of their ids.
    pub fn expire(&mut self, now: Duration) -> Vec<SettledCall> {
        let late: Vec<String> = self
            .waiting
            .iter()
            .filter(|(_, waiting)| waiting.deadline.is_some_and(|deadline| deadline <= now))
            .map(|(id, _)| id.clone())
            .collect();
        late.into_iter()
            .filter_map(|id| {
                let waiting = self.waiting.get(&id)?;
                let error = RpcError::new(
                    code::TIMEOUT,
                    format!(
                        "Function '{}' timed out after {:?}.",
                        waiting.name,
                        waiting.timeout.unwrap_or_default()
                    ),
                );
                self.fail(&id, error)
            })
            .collect()
    }

    /// The time at which the next waiting call has waited too long, if any
    /// has a limit: when to call [`PendingCalls::expire`] next.
    pub fn next_deadline(&self) -> Option<Duration> {
        self.waiting
            .values()
            .filter_map(|waiting| waiting.deadline)
            .min()
    }

    /// Ends the calls of one surface with [`CANCELLED`](code::CANCELLED), as
    /// when the surface is deleted. The table stays in use.
    pub fn cancel_surface(&mut self, surface_id: &str, reason: &str) -> Vec<SettledCall> {
        let of_surface: Vec<String> = self
            .waiting
            .iter()
            .filter(|(_, waiting)| waiting.surface_id.as_deref() == Some(surface_id))
            .map(|(id, _)| id.clone())
            .collect();
        of_surface
            .into_iter()
            .filter_map(|id| self.fail(&id, RpcError::new(code::CANCELLED, reason)))
            .collect()
    }

    /// Ends every waiting call with [`CANCELLED`](code::CANCELLED) and takes
    /// the table out of use: a later [`PendingCalls::register`] is refused
    /// with [`DISPOSED`](code::DISPOSED), and a later response is ignored.
    pub fn dispose(&mut self, reason: &str) -> Vec<SettledCall> {
        self.disposed = true;
        let all: Vec<String> = self.waiting.keys().cloned().collect();
        all.into_iter()
            .filter_map(|id| self.fail(&id, RpcError::new(code::CANCELLED, reason)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const START: Duration = Duration::from_secs(1_000);

    fn code_of(settled: &SettledCall) -> &str {
        &settled.outcome.error().expect("the call failed").code
    }

    #[test]
    fn a_response_ends_the_call_it_names_and_no_other() {
        let mut calls = PendingCalls::new();
        calls.register(PendingCall::new("a", "one"), START).unwrap();
        calls
            .register(PendingCall::new("b", "two").on_surface("s"), START)
            .unwrap();
        let settled = calls
            .settle(&FunctionResponse::value("b", Value::Null))
            .unwrap();
        assert_eq!(
            (settled.name.as_str(), settled.surface_id.as_deref()),
            ("two", Some("s"))
        );
        assert_eq!(settled.outcome, FunctionOutcome::Value(Value::Null));
        assert!(calls.is_pending("a") && !calls.is_pending("b"));

        let failed = calls
            .settle(&FunctionResponse::error(
                "a",
                RpcError::new("PERMISSION_DENIED", "no"),
            ))
            .unwrap();
        assert_eq!(code_of(&failed), "PERMISSION_DENIED");
        assert_eq!(calls.pending_count(), 0);
    }

    #[test]
    fn a_response_nothing_waits_on_changes_and_keeps_nothing() {
        let mut calls = PendingCalls::new();
        calls.register(PendingCall::new("a", "one"), START).unwrap();
        for _ in 0..1_000 {
            assert!(
                calls
                    .settle(&FunctionResponse::value("unknown", json!(1)))
                    .is_none()
            );
        }
        assert_eq!(calls.pending_count(), 1);
        calls
            .settle(&FunctionResponse::value("a", json!(1)))
            .unwrap();
        // A second response to the same call is one nothing waits on.
        assert!(
            calls
                .settle(&FunctionResponse::value("a", json!(2)))
                .is_none()
        );
        // And the id is free again.
        calls.register(PendingCall::new("a", "one"), START).unwrap();
    }

    #[test]
    fn a_call_is_refused_before_anything_is_kept() {
        let mut calls = PendingCalls::new().with_max_pending(2);
        calls.register(PendingCall::new("a", "one"), START).unwrap();
        let refused = |calls: &mut PendingCalls, call: PendingCall| {
            calls.register(call, START).unwrap_err().code
        };
        assert_eq!(
            refused(&mut calls, PendingCall::new("a", "again")),
            code::DUPLICATE
        );
        assert_eq!(
            refused(&mut calls, PendingCall::new("", "unnamed")),
            code::INVALID_FUNCTION_CALL
        );
        assert_eq!(
            refused(
                &mut calls,
                PendingCall::new("z", "now").with_timeout(Duration::ZERO)
            ),
            code::INVALID_FUNCTION_CALL
        );
        calls.register(PendingCall::new("b", "two"), START).unwrap();
        assert_eq!(
            refused(&mut calls, PendingCall::new("c", "three")),
            code::TOO_MANY_PENDING
        );
        assert_eq!(calls.pending_count(), 2);
    }

    #[test]
    fn a_call_times_out_when_the_clock_reaches_its_deadline() {
        let mut calls = PendingCalls::new().with_default_timeout(Some(Duration::from_secs(5)));
        calls
            .register(PendingCall::new("a", "slow"), START)
            .unwrap();
        calls
            .register(
                PendingCall::new("b", "slower").with_timeout(Duration::from_secs(60)),
                START,
            )
            .unwrap();
        assert_eq!(calls.next_deadline(), Some(START + Duration::from_secs(5)));
        assert!(
            calls
                .expire(START + Duration::from_millis(4_999))
                .is_empty()
        );
        let late = calls.expire(START + Duration::from_secs(5));
        assert_eq!(late.len(), 1);
        assert_eq!(
            (late[0].function_call_id.as_str(), code_of(&late[0])),
            ("a", code::TIMEOUT)
        );
        // The response comes after all: nothing waits on it.
        assert!(
            calls
                .settle(&FunctionResponse::value("a", json!(1)))
                .is_none()
        );
        assert!(calls.is_pending("b"));

        // Without a limit a call waits for good.
        let mut patient = PendingCalls::new().with_default_timeout(None);
        patient
            .register(PendingCall::new("a", "slow"), START)
            .unwrap();
        assert_eq!(patient.next_deadline(), None);
        assert!(patient.expire(Duration::MAX).is_empty());
    }

    #[test]
    fn a_surface_takes_its_calls_with_it_and_disposal_takes_all() {
        let mut calls = PendingCalls::new();
        calls
            .register(PendingCall::new("a", "one").on_surface("s"), START)
            .unwrap();
        calls
            .register(PendingCall::new("b", "two").on_surface("t"), START)
            .unwrap();
        calls
            .register(PendingCall::new("c", "three"), START)
            .unwrap();

        let cancelled = calls.cancel_surface("s", "surface 's' was deleted");
        assert_eq!(cancelled.len(), 1);
        assert_eq!(code_of(&cancelled[0]), code::CANCELLED);
        // The table is still in use.
        calls
            .register(PendingCall::new("d", "four").on_surface("s"), START)
            .unwrap();

        let dropped = calls.dispose("the session ended");
        let ids: Vec<&str> = dropped
            .iter()
            .map(|call| call.function_call_id.as_str())
            .collect();
        assert_eq!(ids, ["b", "c", "d"]);
        assert!(dropped.iter().all(|call| code_of(call) == code::CANCELLED));
        assert!(calls.is_disposed());
        assert_eq!(
            calls
                .register(PendingCall::new("e", "five"), START)
                .unwrap_err()
                .code,
            code::DISPOSED
        );
        assert!(
            calls
                .settle(&FunctionResponse::value("b", json!(1)))
                .is_none()
        );
    }

    #[test]
    fn generated_ids_are_told_in_order_and_pass_over_ones_in_use() {
        let mut calls = PendingCalls::new();
        calls
            .register(PendingCall::new("call-2", "taken"), START)
            .unwrap();
        assert_eq!(calls.next_id(), "call-1");
        assert_eq!(calls.next_id(), "call-3");
    }
}
