//! The calls an agent makes to functions a renderer runs.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value};

use super::catalog::CatalogDocument;
use crate::error::{Error, Result};
use crate::protocol::catalog::{AllowedCallers, Catalog};
use crate::protocol::message::{
    AgentMessage, AgentPayload, CallRendererFunction, FunctionCall, FunctionResponse,
};
use crate::protocol::rpc::{PendingCall, PendingCalls, RpcError, SettledCall};
use crate::protocol::validate::is_identifier;
use crate::protocol::version::A2uiVersion;

/// One call to make: what [`RendererCalls::call`] is given.
///
/// ```
/// use std::time::Duration;
///
/// use ag_ui_a2ui::agent::RendererCall;
/// use serde_json::json;
///
/// let call = RendererCall::new("example.com:device", "getLocation")
///     .with_arg("accuracy", json!("city"))
///     .on_surface("checkout")
///     .with_timeout(Duration::from_secs(5));
/// # let _ = call;
/// ```
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct RendererCall {
    /// The catalog that declares the function. `callRendererFunction`
    /// requires it: there is no surface default to fall back on.
    pub catalog_id: String,
    /// The function to run.
    pub name: String,
    /// The arguments: an object, by name. A value may be a binding
    /// (`{"@path": …}`), which the renderer resolves against the surface.
    pub args: Option<Value>,
    /// The surface whose data model the arguments are resolved against. With
    /// none, or an empty one, the renderer resolves them against an empty
    /// data model.
    pub surface_id: Option<String>,
    /// The call's id. With none the table gives one.
    pub function_call_id: Option<String>,
    /// How long this call waits, in place of the default.
    pub timeout: Option<Duration>,
}

impl RendererCall {
    /// A call to `name` of catalog `catalog_id`, with no arguments.
    pub fn new(catalog_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            catalog_id: catalog_id.into(),
            name: name.into(),
            args: None,
            surface_id: None,
            function_call_id: None,
            timeout: None,
        }
    }

    /// The arguments, as a JSON object. Anything else is refused when the
    /// call is made.
    #[must_use]
    pub fn with_args(mut self, args: Value) -> Self {
        self.args = Some(args);
        self
    }

    /// One more argument.
    #[must_use]
    pub fn with_arg(mut self, name: impl Into<String>, value: Value) -> Self {
        let args = self.args.get_or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(args) = args {
            args.insert(name.into(), value);
        }
        self
    }

    /// The surface whose data model the arguments are resolved against.
    #[must_use]
    pub fn on_surface(mut self, surface_id: impl Into<String>) -> Self {
        self.surface_id = Some(surface_id.into());
        self
    }

    /// The `functionCallId` to use, in place of one the table gives.
    ///
    /// An id that is given is the giver's to keep unique for as long as an
    /// answer to it can arrive. The table refuses one that waits now; it does
    /// not remember the given ids of calls that ended, so an id used again
    /// takes the late answers to its first use. The ids the table gives
    /// itself are never ones it has had.
    #[must_use]
    pub fn with_id(mut self, function_call_id: impl Into<String>) -> Self {
        self.function_call_id = Some(function_call_id.into());
        self
    }

    /// How long this call waits for its response.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

/// The agent's `callRendererFunction` calls in one conversation: makes the
/// message for a call, and keeps track of the call until its
/// `rendererFunctionResponse` comes, or does not.
///
/// It is [`PendingCalls`] with what an agent's call needs checked first.
/// Nothing here waits: [`RendererCalls::call`] gives the message to send at
/// once, and how the call ended is handed over when
/// [`RendererCalls::resolve`], [`RendererCalls::expire`] or
/// [`RendererCalls::dispose`] ends it. Time is an argument, as it is for
/// [`PendingCalls`].
///
/// ```
/// use std::time::Duration;
///
/// use ag_ui_a2ui::agent::{RendererCall, RendererCalls};
/// use ag_ui_a2ui::message::FunctionResponse;
/// use serde_json::json;
///
/// let mut calls = RendererCalls::new();
/// let now = Duration::from_secs(0);
/// let message = calls.call(RendererCall::new("example.com:device", "getLocale"), now)?;
/// let sent = serde_json::to_value(&message).unwrap();
/// assert_eq!(sent["callRendererFunction"]["functionCallId"], "call-1");
/// assert_eq!(calls.pending_count(), 1);
///
/// // …the renderer answers:
/// let settled = calls.resolve(&FunctionResponse::value("call-1", json!("fr"))).unwrap();
/// assert_eq!(settled.outcome.value(), Some(&json!("fr")));
/// # Ok::<(), ag_ui_a2ui::rpc::RpcError>(())
/// ```
///
/// # A call the model wrote
///
/// In a v1.0 session the prompt shows `callRendererFunction`, so an answer may
/// carry one, under an id the model chose. The agent sends it with the rest of
/// the answer, and [`RendererCalls::track`] starts waiting on it, so that its
/// response has somewhere to land. The same rules hold as for a call of the
/// agent's own: an id that already waits is refused, and so is a call the
/// agent would not make.
///
/// # Ids
///
/// A call that gives no id gets the table's next one, `call-1`, `call-2`, …,
/// which is never one the table has had, whoever gave the earlier one. Keep
/// one table for a conversation; a host that makes more than one gives each a
/// prefix of its own ([`RendererCalls::with_id_prefix`]). The rule is
/// [`PendingCalls`]'s, and is told in full there.
#[derive(Debug, Clone, Default)]
pub struct RendererCalls {
    table: PendingCalls,
    /// The agent's catalogs, when calls are checked against them.
    catalogs: Option<BTreeMap<String, Target>>,
}

/// A catalog a call is checked against, and the functions of it that the
/// agent runs itself.
#[derive(Debug, Clone)]
struct Target {
    catalog: Arc<Catalog>,
    agent_functions: BTreeSet<String>,
}

impl RendererCalls {
    /// An empty table that checks a call's own shape and nothing of its
    /// catalog. Calls wait thirty seconds
    /// ([`DEFAULT_CALL_TIMEOUT`](crate::rpc::DEFAULT_CALL_TIMEOUT)), and at
    /// most [`DEFAULT_MAX_PENDING`](crate::rpc::DEFAULT_MAX_PENDING) at once.
    pub fn new() -> Self {
        Self::default()
    }

    /// Checks every call against the agent's catalogs, such as
    /// [`A2uiRequestProcessor::active_catalogs`](super::A2uiRequestProcessor::active_catalogs):
    /// the catalog is one of them, it declares the function for the renderer
    /// to run, and the agent may call it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Catalog`] when two catalogs share an id.
    pub fn with_catalogs(self, catalogs: &[CatalogDocument]) -> Result<Self> {
        self.checking(catalogs.iter().map(|catalog| Target {
            catalog: catalog.shared_catalog(),
            agent_functions: catalog.agent_functions().map(str::to_string).collect(),
        }))
    }

    /// [`RendererCalls::with_catalogs`], for catalogs held as summaries: ones
    /// built in code, which have no document. None of their functions is the
    /// agent's to run.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Catalog`] when two catalogs share an id.
    pub fn with_catalog_summaries(
        self,
        catalogs: impl IntoIterator<Item = Catalog>,
    ) -> Result<Self> {
        self.checking(catalogs.into_iter().map(|catalog| Target {
            catalog: Arc::new(catalog),
            agent_functions: BTreeSet::new(),
        }))
    }

    fn checking(mut self, targets: impl Iterator<Item = Target>) -> Result<Self> {
        let mut by_id = BTreeMap::new();
        for target in targets {
            let id = target.catalog.catalog_id.clone();
            if by_id.insert(id.clone(), target).is_some() {
                return Err(Error::catalog(format!(
                    "Duplicate catalog ID: '{id}'. Pass each catalog once."
                )));
            }
        }
        self.catalogs = Some(by_id);
        Ok(self)
    }

    /// How long a call waits unless it sets a timeout of its own. `None`
    /// waits without a limit.
    #[must_use]
    pub fn with_default_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.table = self.table.with_default_timeout(timeout);
        self
    }

    /// The most calls that may wait at once.
    #[must_use]
    pub fn with_max_pending(mut self, max_pending: usize) -> Self {
        self.table = self.table.with_max_pending(max_pending);
        self
    }

    /// What the ids this table gives start with, in place of `call-`
    /// ([`PendingCalls::with_id_prefix`]).
    ///
    /// A table counts its ids from one. Keep one table for the whole of a
    /// conversation with a renderer, and nothing more is needed. A host that
    /// makes a table for each request, or again after a restart, gives each a
    /// prefix no earlier table of that conversation used, the id of the run,
    /// say: without it the second table gives `call-1` again, and a late
    /// answer to the first table's call settles the second's.
    #[must_use]
    pub fn with_id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.table = self.table.with_id_prefix(prefix);
        self
    }

    /// Makes a call at `now`: checks it, starts waiting on it, and gives the
    /// `callRendererFunction` message to send.
    ///
    /// The checks run in this order, each before anything is kept: the
    /// timeout, disposal, the id, a duplicate id, the limit on waiting calls,
    /// the function name, the catalog id, the name as an identifier, the
    /// arguments, and last the catalogs. A call that is refused takes no
    /// place in the table and no id.
    ///
    /// A call that gives no id gets the table's next one, which the table has
    /// not had before. An id given with [`RendererCall::with_id`] is the
    /// caller's to keep unique: the table refuses one that waits now, and
    /// does not remember the given ids of calls that ended.
    ///
    /// # Errors
    ///
    /// Returns an [`RpcError`] and keeps nothing: `DISPOSED`, `DUPLICATE` and
    /// `TOO_MANY_PENDING` as [`PendingCalls::register`] gives them;
    /// `INVALID_FUNCTION_CALL` for a zero timeout, an empty id, name or
    /// catalog id, a name that is not an identifier or starts with `@`, and
    /// arguments that are not an object keyed by identifiers. With catalogs,
    /// `UNKNOWN_FUNCTION` when the catalog is not one of them or declares no
    /// such function for the renderer, and `INVALID_FUNCTION_CALL` when the
    /// function is the renderer's alone to call or needs a user's gesture.
    pub fn call(
        &mut self,
        call: RendererCall,
        now: Duration,
    ) -> std::result::Result<AgentMessage, RpcError> {
        if call.timeout.is_some_and(|timeout| timeout.is_zero()) {
            return Err(RpcError::invalid_call(format!(
                "The timeout of the call to '{}' must be longer than nothing.",
                call.name
            )));
        }
        // The table's own checks next, on the id the call will have.
        self.table.admits(call.function_call_id.as_deref())?;
        let args = match call.args {
            None => Map::new(),
            Some(Value::Object(args)) => args,
            Some(_) => {
                // The name comes first, here as for a call with arguments.
                Self::check_shape(&call.name, &call.catalog_id, &Map::new())?;
                return Err(RpcError::invalid_call(format!(
                    "Invalid arguments for '{}': the arguments must be an object, by name.",
                    call.name
                )));
            }
        };
        Self::check_shape(&call.name, &call.catalog_id, &args)?;
        self.check_target(&call.catalog_id, &call.name)?;

        let function_call_id = call
            .function_call_id
            .unwrap_or_else(|| self.table.next_id());
        let surface_id = call.surface_id.filter(|surface_id| !surface_id.is_empty());
        let mut pending = PendingCall::new(function_call_id.clone(), call.name.clone());
        pending.surface_id.clone_from(&surface_id);
        pending.timeout = call.timeout;
        self.table.register(pending, now)?;

        let mut payload = CallRendererFunction::new(
            function_call_id,
            FunctionCall::new(call.name)
                .with_catalog_id(call.catalog_id)
                .with_args(args),
        );
        payload.surface_id = surface_id;
        Ok(AgentMessage::call_renderer_function(payload))
    }

    /// What a call has to be before any catalog is asked: a name and a
    /// catalog, a name that is an identifier, arguments keyed by identifiers.
    fn check_shape(
        name: &str,
        catalog_id: &str,
        args: &Map<String, Value>,
    ) -> std::result::Result<(), RpcError> {
        if name.is_empty() {
            return Err(RpcError::invalid_call("The call needs a function name."));
        }
        if catalog_id.is_empty() {
            return Err(RpcError::invalid_call(format!(
                "The call to '{name}' needs a catalogId."
            )));
        }
        if name.starts_with('@') || !is_identifier(name) {
            return Err(RpcError::invalid_call(format!(
                "The function name '{name}' must be a UAX #31 identifier that doesn't start with \
                 '@'."
            )));
        }
        if let Some(key) = args.keys().find(|key| !is_identifier(key)) {
            return Err(RpcError::invalid_call(format!(
                "Invalid arguments for '{name}': the key '{key}' is not a UAX #31 identifier."
            )));
        }
        Ok(())
    }

    /// Starts waiting, at `now`, on a `callRendererFunction` that was written
    /// elsewhere and is about to be sent: one in a model's answer, which
    /// [`A2uiRequestProcessor::parse_response`](super::A2uiRequestProcessor::parse_response)
    /// has already checked. Gives whether the message is such a call; any
    /// other message is passed over.
    ///
    /// The call is held to what [`RendererCalls::call`] holds its own to: it
    /// is a message of a version that has function calls, so that it can be
    /// sent at all; it has a name, a catalog and arguments keyed by
    /// identifiers; and with catalogs, the function is one the renderer runs
    /// and the agent may call. A function the agent runs itself
    /// ([`AgentFunction`](super::AgentFunction)) is not one to ask the
    /// renderer for, whoever wrote the call.
    ///
    /// The id is the one the message carries, and is its writer's to keep
    /// unique: the table refuses one that waits now, and does not remember
    /// the given ids of calls that ended. An id of the form the table's own
    /// have moves the table's count past it, so the table never gives it
    /// again.
    ///
    /// # Errors
    ///
    /// As [`RendererCalls::call`], and as [`PendingCalls::register`]: the id
    /// already waits (`DUPLICATE`), the limit is reached
    /// (`TOO_MANY_PENDING`), or the table was disposed of (`DISPOSED`). The
    /// call is then one whose response nothing waits on, and is not to be
    /// sent.
    pub fn track(
        &mut self,
        message: &AgentMessage,
        now: Duration,
    ) -> std::result::Result<bool, RpcError> {
        let AgentPayload::CallRendererFunction(call) = &message.payload else {
            return Ok(false);
        };
        let sendable =
            A2uiVersion::parse(&message.version).is_some_and(A2uiVersion::carries_function_calls);
        if !sendable {
            return Err(RpcError::invalid_call(format!(
                "A message of version '{}' carries no function call, so this one cannot be \
                 sent.",
                message.version
            )));
        }
        self.table.admits(Some(&call.function_call_id))?;
        let name = call.call_function.call.as_str();
        let catalog_id = call.call_function.catalog_id.as_deref().unwrap_or_default();
        let none = Map::new();
        let args = call.call_function.args.as_ref().unwrap_or(&none);
        Self::check_shape(name, catalog_id, args)?;
        self.check_target(catalog_id, name)?;
        let mut pending = PendingCall::new(call.function_call_id.clone(), name);
        pending.surface_id.clone_from(&call.surface_id);
        self.table.register(pending, now)?;
        Ok(true)
    }

    /// Whether the agent may make this call, when it has catalogs to say.
    fn check_target(&self, catalog_id: &str, name: &str) -> std::result::Result<(), RpcError> {
        let Some(catalogs) = &self.catalogs else {
            return Ok(());
        };
        let Some(target) = catalogs.get(catalog_id) else {
            return Err(RpcError::unknown_function(format!(
                "The call to '{name}' names catalog '{catalog_id}', which isn't one of the \
                 agent's catalogs."
            )));
        };
        // A function the agent runs is not one the renderer can.
        let function = target
            .catalog
            .functions
            .get(name)
            .filter(|_| !target.agent_functions.contains(name));
        let Some(function) = function else {
            return Err(RpcError::unknown_function(format!(
                "Catalog '{catalog_id}' has no renderer function '{name}'."
            )));
        };
        if function.allowed_callers == AllowedCallers::RendererOnly {
            return Err(RpcError::invalid_call(format!(
                "Function '{name}' is rendererOnly, so the agent may not call it."
            )));
        }
        if function.requires_user_activation {
            return Err(RpcError::invalid_call(format!(
                "Function '{name}' requires user activation, and a call from the agent has no \
                 user gesture."
            )));
        }
        Ok(())
    }

    /// Ends the call a `rendererFunctionResponse` answers, and hands it over.
    /// `None` when no call under its id waits: it is unknown, late or a
    /// second response, and may be one for the model's turn.
    pub fn resolve(&mut self, response: &FunctionResponse) -> Option<SettledCall> {
        self.table.settle(response)
    }

    /// Fails one waiting call, as when its response could not be read
    /// (`UNKNOWN_ERROR`) or its transport was lost.
    pub fn fail(&mut self, function_call_id: &str, error: RpcError) -> Option<SettledCall> {
        self.table.fail(function_call_id, error)
    }

    /// Ends every call that has waited too long at `now`, with `TIMEOUT`.
    pub fn expire(&mut self, now: Duration) -> Vec<SettledCall> {
        self.table.expire(now)
    }

    /// When to call [`RendererCalls::expire`] next, if a waiting call has a
    /// limit.
    pub fn next_deadline(&self) -> Option<Duration> {
        self.table.next_deadline()
    }

    /// Ends the waiting calls of one surface with `CANCELLED`, as when the
    /// agent deletes the surface.
    pub fn cancel_surface(&mut self, surface_id: &str, reason: &str) -> Vec<SettledCall> {
        self.table.cancel_surface(surface_id, reason)
    }

    /// Ends every waiting call with `CANCELLED` and `reason`, refuses later
    /// calls with `DISPOSED`, and ignores later responses: the end of a
    /// conversation.
    pub fn dispose(&mut self, reason: &str) -> Vec<SettledCall> {
        self.table.dispose(reason)
    }

    /// Whether a call under this id waits for its response.
    pub fn is_pending(&self, function_call_id: &str) -> bool {
        self.table.is_pending(function_call_id)
    }

    /// How many calls wait.
    pub fn pending_count(&self) -> usize {
        self.table.pending_count()
    }

    /// Whether [`RendererCalls::dispose`] was called.
    pub fn is_disposed(&self) -> bool {
        self.table.is_disposed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::rpc::code;
    use serde_json::json;

    const NOW: Duration = Duration::from_secs(10);

    fn device() -> CatalogDocument {
        CatalogDocument::new(json!({
            "catalogId": "example.com:device",
            "protocolVersion": "1.0",
            "components": {},
            "functions": {
                "getLocation": {"returnType": "object", "allowedCallers": "rendererOrAgent"},
                "peek": {"returnType": "string", "allowedCallers": "agentOnly"},
                "formatLocal": {"returnType": "string"}
            }
        }))
        .unwrap()
    }

    #[test]
    fn a_call_is_a_message_to_send_and_a_call_that_waits() {
        let mut calls = RendererCalls::new();
        let message = calls
            .call(
                RendererCall::new("example.com:device", "getLocation")
                    .with_arg("accuracy", json!("city"))
                    .on_surface("s"),
                NOW,
            )
            .unwrap();
        assert_eq!(
            serde_json::to_value(&message).unwrap(),
            json!({"version": "1.0", "callRendererFunction": {
                "surfaceId": "s",
                "functionCallId": "call-1",
                "callFunction": {
                    "@call": "getLocation",
                    "catalogId": "example.com:device",
                    "args": {"accuracy": "city"}
                }
            }})
        );
        // No surface, or an empty one, and no arguments.
        let bare = calls
            .call(
                RendererCall::new("example.com:device", "peek").on_surface(""),
                NOW,
            )
            .unwrap();
        assert_eq!(
            serde_json::to_value(&bare).unwrap()["callRendererFunction"],
            json!({"functionCallId": "call-2", "callFunction": {
                "@call": "peek", "catalogId": "example.com:device", "args": {}
            }})
        );
        assert_eq!(calls.pending_count(), 2);
        assert!(calls.is_pending("call-1") && calls.is_pending("call-2"));
    }

    #[test]
    fn what_is_wrong_with_a_call_is_said_before_anything_is_kept() {
        let mut calls = RendererCalls::new().with_catalogs(&[device()]).unwrap();
        let refused = |calls: &mut RendererCalls, call: RendererCall| {
            let error = calls.call(call, NOW).unwrap_err();
            (error.code, error.message)
        };
        let device = "example.com:device";
        for (call, code, mentions) in [
            (
                RendererCall::new(device, "getLocation").with_id(""),
                code::INVALID_FUNCTION_CALL,
                "functionCallId",
            ),
            (
                RendererCall::new("", ""),
                code::INVALID_FUNCTION_CALL,
                "function name",
            ),
            (
                RendererCall::new("", "getLocation"),
                code::INVALID_FUNCTION_CALL,
                "catalogId",
            ),
            (
                RendererCall::new(device, "@index"),
                code::INVALID_FUNCTION_CALL,
                "@index",
            ),
            (
                RendererCall::new(device, "get-location"),
                code::INVALID_FUNCTION_CALL,
                "identifier",
            ),
            (
                RendererCall::new(device, "getLocation").with_args(json!(["city"])),
                code::INVALID_FUNCTION_CALL,
                "object",
            ),
            (
                RendererCall::new(device, "formatLocal").with_args(json!({"bad-key": 1})),
                code::INVALID_FUNCTION_CALL,
                "bad-key",
            ),
            (
                RendererCall::new(device, "formatLocal"),
                code::INVALID_FUNCTION_CALL,
                "rendererOnly",
            ),
            (
                RendererCall::new(device, "vibrate"),
                code::UNKNOWN_FUNCTION,
                "vibrate",
            ),
            (
                RendererCall::new("example.com:other", "getLocation"),
                code::UNKNOWN_FUNCTION,
                "example.com:other",
            ),
        ] {
            let (found, message) = refused(&mut calls, call.clone());
            assert_eq!(found, code, "{call:?}: {message}");
            assert!(message.contains(mentions), "{call:?}: {message}");
        }
        assert_eq!(calls.pending_count(), 0);
        // And nothing took an id on the way.
        let message = calls.call(RendererCall::new(device, "peek"), NOW).unwrap();
        assert_eq!(
            serde_json::to_value(&message).unwrap()["callRendererFunction"]["functionCallId"],
            "call-1"
        );
        assert!(matches!(
            RendererCalls::new().with_catalogs(&[self::device(), self::device()]),
            Err(Error::Catalog(_))
        ));
    }

    #[test]
    fn a_call_written_elsewhere_is_waited_on_under_its_own_id() {
        let mut calls = RendererCalls::new();
        let written = AgentMessage::call_renderer_function(CallRendererFunction::new(
            "from-the-model",
            FunctionCall::new("getLocation").with_catalog_id("example.com:device"),
        ));
        assert!(calls.track(&written, NOW).unwrap());
        assert!(
            !calls
                .track(&AgentMessage::delete_surface("s"), NOW)
                .unwrap()
        );
        // The same id again is one call too many.
        assert_eq!(
            calls.track(&written, NOW).unwrap_err().code,
            code::DUPLICATE
        );
        assert_eq!(
            calls
                .call(
                    RendererCall::new("example.com:device", "getLocation")
                        .with_id("from-the-model"),
                    NOW
                )
                .unwrap_err()
                .code,
            code::DUPLICATE
        );
        let settled = calls
            .resolve(&FunctionResponse::value(
                "from-the-model",
                json!({"city": "Paris"}),
            ))
            .unwrap();
        assert_eq!(settled.name, "getLocation");
        assert_eq!(calls.pending_count(), 0);
    }
}
