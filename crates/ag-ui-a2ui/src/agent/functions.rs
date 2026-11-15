//! The messages a renderer sends an agent, routed: a call to a function the
//! agent runs, the answer to a call the agent made, a user's action, an
//! error.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::catalog::CatalogDocument;
use crate::error::{Error, Result};
use crate::protocol::catalog::AllowedCallers;
use crate::protocol::message::{
    Action, AgentMessage, CallAgentFunction, FunctionResponse, RendererError,
};
use crate::protocol::rpc::RpcError;
use crate::protocol::validate::{ErrorCode, ValidationError};
use crate::protocol::version::A2uiVersion;

/// A function the agent runs for a renderer, as the agent declares it on its
/// copy of a catalog ([`CatalogDocument::with_agent_functions`]).
///
/// A renderer that meets a call to a function it has no implementation of
/// sends the call to the agent with `callAgentFunction`. Declaring the
/// function says what it takes and returns: the catalog document lists it
/// like any other function, so a prompt shows it and a validator accepts a
/// call to it, and a [`RendererMessageRouter`] knows a call to it is the
/// agent's to answer.
///
/// The declaration is all there is. This crate never holds the function
/// itself: a call comes back to the application as an [`AgentFunctionCall`],
/// and the application runs whatever it likes for it.
///
/// ```
/// use ag_ui_a2ui::A2uiVersion;
/// use ag_ui_a2ui::agent::{AgentFunction, CatalogDocument};
/// use serde_json::json;
///
/// let rate = AgentFunction::new("fetchFxRate", json!({
///     "type": "object",
///     "properties": {"base": {"type": "string"}, "target": {"type": "string", "default": "USD"}},
///     "required": ["base"],
///     "additionalProperties": false
/// }))
/// .returning("number")
/// .described("Current exchange rate from base to target.");
///
/// let catalog = CatalogDocument::basic(A2uiVersion::V1_0)?.with_agent_functions([rate])?;
/// assert!(catalog.is_agent_function("fetchFxRate"));
/// assert!(catalog.catalog().functions.contains_key("fetchFxRate"));
/// # Ok::<(), ag_ui_a2ui::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct AgentFunction {
    /// The function's name, which no function of the catalog has.
    pub name: String,
    /// What it does, for a model that reads the catalog.
    pub description: Option<String>,
    /// The type of what it returns, as a catalog states one (`string`,
    /// `number`, `object`, `void`, …).
    pub return_type: Option<String>,
    /// The JSON Schema of its arguments. It may declare `$defs` of its own,
    /// which join the catalog's.
    pub args: Value,
    /// Whether a renderer sends the call only in answer to a user's gesture.
    pub requires_user_activation: bool,
    /// Whether this is the catalog's own declaration of `name`, taken over.
    pub(crate) declared: bool,
}

impl AgentFunction {
    /// A function named `name` whose arguments are held to `args`, a JSON
    /// Schema for an object.
    pub fn new(name: impl Into<String>, args: Value) -> Self {
        Self {
            name: name.into(),
            description: None,
            return_type: None,
            args,
            requires_user_activation: false,
            declared: false,
        }
    }

    /// The function the catalog already declares under `name`, as one the
    /// agent runs: its arguments, return type and callers are the catalog's,
    /// and the catalog document stays as it is. For a function a catalog
    /// declares without any renderer implementing it.
    pub fn declared(name: impl Into<String>) -> Self {
        Self {
            declared: true,
            ..Self::new(name, Value::Null)
        }
    }

    /// The type of what the function returns.
    #[must_use]
    pub fn returning(mut self, return_type: impl Into<String>) -> Self {
        self.return_type = Some(return_type.into());
        self
    }

    /// What the function does.
    #[must_use]
    pub fn described(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Whether a renderer sends the call only after a user's gesture. The
    /// renderer enforces it; the agent answers such a call like any other.
    #[must_use]
    pub fn requiring_user_activation(mut self, requires: bool) -> Self {
        self.requires_user_activation = requires;
        self
    }
}

/// Which of the four messages a renderer sends a raw message is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RendererMessageKind {
    /// `action`: the user did something on a surface.
    Action,
    /// `error`: the renderer reports a problem.
    Error,
    /// `callAgentFunction`: the renderer asks the agent to run a function.
    CallAgentFunction,
    /// `rendererFunctionResponse`: the answer to a call the agent made.
    RendererFunctionResponse,
}

impl RendererMessageKind {
    const ALL: [Self; 4] = [
        Self::Action,
        Self::CallAgentFunction,
        Self::Error,
        Self::RendererFunctionResponse,
    ];

    /// The payload key, as it is on the wire.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Action => "action",
            Self::Error => "error",
            Self::CallAgentFunction => "callAgentFunction",
            Self::RendererFunctionResponse => "rendererFunctionResponse",
        }
    }

    /// The kind of a raw v1.0 message, without reading its body: for an
    /// application that hands some kinds on unread, an `action` to the
    /// model's turn, say.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Validation`] when `message` is not an object with
    /// `"version": "1.0"` and exactly one of the four payload keys.
    pub fn of(message: &Value) -> Result<Self> {
        let Some(fields) = message.as_object() else {
            return Err(invalid(
                "message",
                "A renderer-to-agent message must be an object.".to_string(),
            ));
        };
        let version = fields.get("version").and_then(Value::as_str);
        let speaks = version
            .and_then(A2uiVersion::parse)
            .is_some_and(A2uiVersion::carries_function_calls);
        if !speaks {
            return Err(invalid(
                "message.version",
                format!(
                    "A renderer-to-agent message must have \"version\": \"{}\", not {}.",
                    A2uiVersion::V1_0,
                    fields.get("version").unwrap_or(&Value::Null)
                ),
            ));
        }
        let names: Vec<&str> = Self::ALL.iter().map(|kind| kind.as_str()).collect();
        let mut keys = fields.keys().filter(|key| *key != "version");
        let (Some(key), None) = (keys.next(), keys.next()) else {
            return Err(invalid(
                "message",
                format!(
                    "A renderer-to-agent message must have exactly one of {names:?} beside \
                     its version."
                ),
            ));
        };
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == key)
            .ok_or_else(|| {
                invalid(
                    format!("message.{key}"),
                    format!(
                        "Unknown renderer-to-agent message '{key}'; expected one of {names:?}."
                    ),
                )
            })
    }
}

fn invalid(path: impl Into<String>, message: String) -> Error {
    Error::Validation {
        errors: vec![ValidationError::new(ErrorCode::InvalidValue, path, message)].into(),
    }
}

/// A call to a function the agent runs, read and found: what a
/// [`RendererMessageRouter`] hands the application for a `callAgentFunction`.
///
/// The application runs the function and hands the outcome back, and gets
/// the `agentFunctionResponse` to send:
///
/// ```
/// # use ag_ui_a2ui::agent::AgentFunctionCall;
/// # use ag_ui_a2ui::AgentMessage;
/// # use ag_ui_a2ui::rpc::RpcError;
/// # use serde_json::json;
/// fn answer(call: &AgentFunctionCall) -> AgentMessage {
///     match (call.name.as_str(), call.args.get("base").and_then(|base| base.as_str())) {
///         ("fetchFxRate", Some("EUR")) => call.value(json!(1.08)),
///         ("fetchFxRate", _) => call.error(RpcError::new("PROVIDER_NOT_FOUND", "no rate")),
///         _ => call.failed("this agent does not run that function"),
///     }
/// }
/// ```
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct AgentFunctionCall {
    /// The id the response copies.
    pub function_call_id: String,
    /// The surface the call was made on.
    pub surface_id: String,
    /// The catalog the call resolved to.
    pub catalog_id: String,
    /// The function to run.
    pub name: String,
    /// The arguments, as resolved values.
    ///
    /// From a router that checks schemas
    /// (`RendererMessageRouter::checking_schemas`, which the
    /// `schema-validation` feature has) they have been held to the
    /// function's `args` schema, and what the schema gives a default the
    /// call left out is filled in. From any other router they are as the
    /// renderer sent them, and checking them is the function's own business.
    pub args: Map<String, Value>,
}

impl AgentFunctionCall {
    /// The `agentFunctionResponse` for a function that returned `value`. A
    /// function that returns nothing returns `null`.
    pub fn value(&self, value: Value) -> AgentMessage {
        AgentMessage::agent_function_response(FunctionResponse::value(
            self.function_call_id.clone(),
            value,
        ))
    }

    /// The `agentFunctionResponse` for a function that failed with a code of
    /// its own choosing.
    pub fn error(&self, error: RpcError) -> AgentMessage {
        AgentMessage::agent_function_response(FunctionResponse::error(
            self.function_call_id.clone(),
            error,
        ))
    }

    /// The `agentFunctionResponse` for a function that failed:
    /// `EXECUTION_ERROR`, with `message`. What a renderer is told is the
    /// application's to choose; an internal error's text need not be it.
    pub fn failed(&self, message: impl Into<String>) -> AgentMessage {
        self.error(RpcError::execution(message))
    }
}

/// What a renderer's message is, and what there is to do with it.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Routed {
    /// A call to a function the agent declared. Run it, and send what
    /// [`AgentFunctionCall::value`] or [`AgentFunctionCall::error`] gives.
    Call(AgentFunctionCall),
    /// A call this crate answers itself, since there is nothing to run: the
    /// function is not the agent's (`UNKNOWN_FUNCTION`), or the call is not
    /// one it takes (`INVALID_FUNCTION_CALL`). Send it.
    Reply(AgentMessage),
    /// The user did something on a surface.
    Action(Action),
    /// The renderer reports a problem.
    Error(RendererError),
    /// The answer to a call the agent made:
    /// [`RendererCalls::resolve`](super::RendererCalls::resolve) takes it.
    FunctionResponse(FunctionResponse),
}

/// Reads the messages a renderer sends an agent, and says what each is for.
///
/// A `callAgentFunction` is always the agent's to answer, with exactly one
/// `agentFunctionResponse`: the router finds the function among those the
/// agent declared on its catalogs and hands the call over
/// ([`Routed::Call`]), or says what to answer when there is nothing to run
/// ([`Routed::Reply`]). The other three messages are read and handed over.
/// Nothing is run and nothing is kept: one router serves any number of
/// conversations.
///
/// [`A2uiRequestProcessor::renderer_messages`](super::A2uiRequestProcessor::renderer_messages)
/// makes one over a request's active catalogs.
///
/// ```
/// use ag_ui_a2ui::A2uiVersion;
/// use ag_ui_a2ui::agent::{AgentFunction, CatalogDocument, RendererMessageRouter, Routed};
/// use serde_json::json;
///
/// let catalog = CatalogDocument::basic(A2uiVersion::V1_0)?.with_agent_functions([
///     AgentFunction::new("fetchFxRate", json!({"type": "object"})).returning("number"),
/// ])?;
/// let router = RendererMessageRouter::new(&[catalog.clone()])?;
///
/// let message = json!({"version": "1.0", "callAgentFunction": {
///     "surfaceId": "checkout",
///     "functionCallId": "fx-1",
///     "callFunction": {"@call": "fetchFxRate", "catalogId": catalog.catalog_id(), "args": {"base": "EUR"}}
/// }});
/// let Routed::Call(call) = router.route(&message)? else { panic!("a call to run") };
/// assert_eq!((call.name.as_str(), call.surface_id.as_str()), ("fetchFxRate", "checkout"));
/// // The application runs it, and sends the answer.
/// let answer = serde_json::to_value(call.value(json!(1.08)))?;
/// assert_eq!(answer["agentFunctionResponse"], json!({"functionCallId": "fx-1", "value": 1.08}));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct RendererMessageRouter {
    catalogs: BTreeMap<String, CatalogDocument>,
    #[cfg(feature = "schema-validation")]
    schemas: Option<std::sync::Arc<Schemas>>,
}

/// What a router that checks schemas compiled: the `args` of each agent
/// function, and the renderer's messages.
#[cfg(feature = "schema-validation")]
#[derive(Debug)]
struct Schemas {
    /// By catalog id and function name: the compiled schema, and the schema
    /// with the catalog's definitions, which the defaults are read from.
    args: BTreeMap<(String, String), (jsonschema::Validator, Value)>,
    messages: crate::protocol::client_schema::ClientSchemaValidator,
}

impl RendererMessageRouter {
    /// A router over the agent's catalogs, such as
    /// [`A2uiRequestProcessor::active_catalogs`](super::A2uiRequestProcessor::active_catalogs).
    /// The functions it answers for are the agent functions of those catalogs.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Catalog`] when two catalogs share an id.
    pub fn new(catalogs: &[CatalogDocument]) -> Result<Self> {
        let mut by_id = BTreeMap::new();
        for catalog in catalogs {
            if by_id
                .insert(catalog.catalog_id().to_string(), catalog.clone())
                .is_some()
            {
                return Err(Error::catalog(format!(
                    "Duplicate catalog ID: '{}'. Pass each catalog once.",
                    catalog.catalog_id()
                )));
            }
        }
        Ok(Self {
            catalogs: by_id,
            #[cfg(feature = "schema-validation")]
            schemas: None,
        })
    }

    /// A router that also holds what it reads to the JSON Schemas: the
    /// arguments of a call to the function's `args` schema, with the
    /// defaults the schema gives filled in, and an `action`, an `error` and a
    /// `rendererFunctionResponse` to the protocol's own.
    ///
    /// It is asked for, not switched on by the feature: without it the
    /// arguments of a call arrive as the renderer sent them
    /// ([`AgentFunctionCall::args`]).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Catalog`] when the `args` schema of an agent function
    /// does not compile: it is not a schema, or it refers to a definition
    /// neither it nor its catalog has.
    #[cfg(feature = "schema-validation")]
    pub fn checking_schemas(mut self) -> Result<Self> {
        use crate::protocol::schema::SchemaBundle;
        use crate::protocol::schema_validation::compile_with;

        let mut args = BTreeMap::new();
        for (catalog_id, catalog) in &self.catalogs {
            let version = catalog.version();
            let common_types = SchemaBundle::texts(version)[1];
            // Beside the common types, so that a reference to them resolves.
            let base = serde_json::from_str::<Value>(common_types)?
                .get("$id")
                .and_then(Value::as_str)
                .and_then(|id| id.rsplit_once('/'))
                .map(|(base, _)| base.to_string())
                .unwrap_or_default();
            let definitions = catalog.document().get("$defs").cloned();
            for name in catalog.agent_functions() {
                let Some(schema) = catalog
                    .catalog()
                    .functions
                    .get(name)
                    .and_then(|function| function.args.clone())
                else {
                    continue;
                };
                let mut rooted = Map::new();
                rooted.insert(
                    "$id".into(),
                    Value::String(format!("{base}/agent_function_args.json")),
                );
                let mut defs = match &definitions {
                    Some(Value::Object(defs)) => defs.clone(),
                    _ => Map::new(),
                };
                defs.insert(ARGS_DEFINITION.into(), schema);
                rooted.insert("$defs".into(), Value::Object(defs));
                rooted.insert(
                    "$ref".into(),
                    Value::String(format!("#/$defs/{ARGS_DEFINITION}")),
                );
                let rooted = Value::Object(rooted);
                let compiled =
                    compile_with(version, &rooted, &[common_types]).map_err(|error| {
                        Error::catalog(format!(
                            "the args schema of agent function '{name}' of catalog '{catalog_id}' \
                         does not compile: {error}"
                        ))
                    })?;
                args.insert((catalog_id.clone(), name.to_string()), (compiled, rooted));
            }
        }
        let version = self
            .catalogs
            .values()
            .next()
            .map_or(A2uiVersion::V1_0, CatalogDocument::version);
        self.schemas = Some(std::sync::Arc::new(Schemas {
            args,
            messages: crate::protocol::client_schema::ClientSchemaValidator::for_version(
                if version.carries_function_calls() {
                    version
                } else {
                    A2uiVersion::V1_0
                },
            )?,
        }));
        Ok(self)
    }

    /// Reads one message a renderer sent, and says what it is for.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Validation`] when the message is not one of the four
    /// a v1.0 renderer sends ([`RendererMessageKind::of`]), when its body does
    /// not read, and when it is a `callAgentFunction` with no
    /// `functionCallId` to answer under. A `callAgentFunction` that has one
    /// is never an error: what is wrong with it is the answer
    /// ([`Routed::Reply`]).
    pub fn route(&self, message: &Value) -> Result<Routed> {
        let kind = RendererMessageKind::of(message)?;
        let body = &message[kind.as_str()];
        if kind == RendererMessageKind::CallAgentFunction {
            return self.route_call(body);
        }
        #[cfg(feature = "schema-validation")]
        if let Some(schemas) = &self.schemas {
            schemas.messages.validate_message(message)?;
        }
        let read = |error: serde_json::Error| {
            invalid(
                format!("message.{}", kind.as_str()),
                format!("Malformed {}: {error}.", kind.as_str()),
            )
        };
        Ok(match kind {
            RendererMessageKind::Action => {
                Routed::Action(serde_json::from_value(body.clone()).map_err(read)?)
            }
            RendererMessageKind::Error => {
                Routed::Error(serde_json::from_value(body.clone()).map_err(read)?)
            }
            _ => Routed::FunctionResponse(serde_json::from_value(body.clone()).map_err(read)?),
        })
    }

    /// A `callAgentFunction`: the call to run, or the answer to send.
    fn route_call(&self, body: &Value) -> Result<Routed> {
        let call_id = body
            .get("functionCallId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty());
        let Some(call_id) = call_id else {
            return Err(invalid(
                "message.callAgentFunction.functionCallId",
                "A callAgentFunction without a functionCallId cannot be answered: the response \
                 copies it."
                    .to_string(),
            ));
        };
        let refuse = |error: RpcError| {
            Ok(Routed::Reply(AgentMessage::agent_function_response(
                FunctionResponse::error(call_id, error),
            )))
        };
        let call: CallAgentFunction = match serde_json::from_value(body.clone()) {
            Ok(call) => call,
            Err(error) => {
                return refuse(RpcError::invalid_call(format!(
                    "Malformed callAgentFunction message: {error}."
                )));
            }
        };
        let name = call.call_function.call.as_str();
        if call.surface_id.is_empty() {
            return refuse(RpcError::invalid_call(format!(
                "The call to '{name}' has an empty surfaceId."
            )));
        }
        if name.is_empty() {
            return refuse(RpcError::invalid_call(
                "callAgentFunction.callFunction['@call'] must be a non-empty string.",
            ));
        }
        // A renderer names the catalog on every call. One that names none
        // runs the function of that name, when exactly one catalog has it.
        let catalog_id = match &call.call_function.catalog_id {
            Some(catalog_id) => catalog_id.clone(),
            None => {
                let mut holders = self
                    .catalogs
                    .iter()
                    .filter(|(_, catalog)| catalog.is_agent_function(name))
                    .map(|(id, _)| id);
                match (holders.next(), holders.next()) {
                    (Some(only), None) => only.clone(),
                    (None, _) => {
                        return refuse(RpcError::unknown_function(format!(
                            "No catalog of the agent has an agent function '{name}'."
                        )));
                    }
                    (Some(_), Some(_)) => {
                        return refuse(RpcError::invalid_call(format!(
                            "The call to '{name}' has no catalogId, and several catalogs have \
                             an agent function of that name."
                        )));
                    }
                }
            }
        };
        let Some(catalog) = self.catalogs.get(&catalog_id) else {
            return refuse(RpcError::unknown_function(format!(
                "The agent has no catalog '{catalog_id}'."
            )));
        };
        let function = catalog
            .catalog()
            .functions
            .get(name)
            .filter(|_| catalog.is_agent_function(name));
        let Some(function) = function else {
            return refuse(RpcError::unknown_function(format!(
                "Catalog '{catalog_id}' has no agent function '{name}'."
            )));
        };
        if function.allowed_callers == AllowedCallers::AgentOnly {
            return refuse(RpcError::invalid_call(format!(
                "Function '{name}' can't be called by the renderer (allowedCallers is \
                 agentOnly)."
            )));
        }
        #[allow(unused_mut)]
        let mut args = call.call_function.args.unwrap_or_default();
        #[cfg(feature = "schema-validation")]
        if let Some((compiled, rooted)) = self
            .schemas
            .as_ref()
            .and_then(|schemas| schemas.args.get(&(catalog_id.clone(), name.to_string())))
        {
            let given = Value::Object(args);
            if let Some(error) = compiled.iter_errors(&given).next() {
                return refuse(RpcError::invalid_call(format!(
                    "Invalid arguments for function '{name}': {}",
                    crate::protocol::schema_validation::message(&error)
                )));
            }
            let mut filled = given;
            apply_defaults(rooted, rooted, &mut filled, 0);
            args = match filled {
                Value::Object(args) => args,
                _ => Map::new(),
            };
        }
        Ok(Routed::Call(AgentFunctionCall {
            function_call_id: call.function_call_id,
            surface_id: call.surface_id,
            catalog_id,
            name: name.to_string(),
            args,
        }))
    }
}

/// The name the `args` schema of a function is compiled under, beside the
/// definitions of its catalog.
#[cfg(feature = "schema-validation")]
const ARGS_DEFINITION: &str = "a2ui.agentFunctionArgs";

/// How deep the defaults of a schema are followed: past any schema an author
/// writes, and short of a reference that leads back to itself for ever.
#[cfg(feature = "schema-validation")]
const MAX_DEFAULT_DEPTH: usize = 32;

/// Fills in what `schema` gives a default and `instance` leaves out, after
/// the instance has passed the schema: the properties of an object, of the
/// objects in it, and of the items of a list. A reference is followed within
/// `root`, and every member of an `allOf` has its say.
#[cfg(feature = "schema-validation")]
fn apply_defaults(root: &Value, schema: &Value, instance: &mut Value, depth: usize) {
    if depth > MAX_DEFAULT_DEPTH {
        return;
    }
    let Some(keywords) = schema.as_object() else {
        return;
    };
    if let Some(target) = keywords
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| reference.strip_prefix('#'))
        .and_then(|pointer| root.pointer(pointer))
    {
        apply_defaults(root, target, instance, depth + 1);
    }
    for member in keywords
        .get("allOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        apply_defaults(root, member, instance, depth + 1);
    }
    match instance {
        Value::Object(fields) => {
            let Some(Value::Object(properties)) = keywords.get("properties") else {
                return;
            };
            for (name, property) in properties {
                if !fields.contains_key(name) {
                    if let Some(default) = default_of(root, property, 0) {
                        fields.insert(name.clone(), default);
                    }
                }
                if let Some(held) = fields.get_mut(name) {
                    apply_defaults(root, property, held, depth + 1);
                }
            }
        }
        Value::Array(items) => {
            if let Some(item_schema) = keywords.get("items").filter(|items| items.is_object()) {
                for item in items {
                    apply_defaults(root, item_schema, item, depth + 1);
                }
            }
        }
        _ => {}
    }
}

/// The default a property's schema states, on itself or on what it refers to.
#[cfg(feature = "schema-validation")]
fn default_of(root: &Value, schema: &Value, depth: usize) -> Option<Value> {
    if depth > MAX_DEFAULT_DEPTH {
        return None;
    }
    if let Some(default) = schema.get("default") {
        return Some(default.clone());
    }
    let target = schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| reference.strip_prefix('#'))
        .and_then(|pointer| root.pointer(pointer))?;
    default_of(root, target, depth + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn catalog() -> CatalogDocument {
        CatalogDocument::new(json!({
            "catalogId": "example.com:shop",
            "protocolVersion": "1.0",
            "components": {},
            "functions": {
                "notify": {"returnType": "void", "allowedCallers": "rendererOrAgent"}
            },
            "$defs": {"Money": {"type": "object", "properties": {
                "amount": {"type": "number"},
                "currency": {"type": "string", "default": "USD"}
            }, "required": ["amount"]}}
        }))
        .unwrap()
        .with_agent_functions([AgentFunction::new(
            "pay",
            json!({
                "type": "object",
                "properties": {
                    "total": {"$ref": "#/$defs/Money"},
                    "memo": {"type": "string", "default": "standard"}
                },
                "required": ["total"],
                "additionalProperties": false
            }),
        )
        .returning("object")])
        .unwrap()
    }

    fn call(body: Value) -> Value {
        json!({"version": "1.0", "callAgentFunction": body})
    }

    fn reply_code(routed: &Routed) -> String {
        let Routed::Reply(reply) = routed else {
            panic!("expected a reply, got {routed:?}");
        };
        serde_json::to_value(reply).unwrap()["agentFunctionResponse"]["error"]["code"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn a_call_is_handed_over_or_answered_and_never_run() {
        let router = RendererMessageRouter::new(&[catalog()]).unwrap();
        let pay = |args: Value| {
            call(
                json!({"surfaceId": "s", "functionCallId": "c1", "callFunction": {
                    "@call": "pay", "catalogId": "example.com:shop", "args": args
                }}),
            )
        };
        // Without a schema check the arguments are as they were sent.
        let Routed::Call(handed) = router.route(&pay(json!({"total": "a lot"}))).unwrap() else {
            panic!("a call to run");
        };
        assert_eq!(handed.args, *json!({"total": "a lot"}).as_object().unwrap());
        assert_eq!(handed.catalog_id, "example.com:shop");

        // A function the renderer runs is not the agent's, and neither is one
        // nothing declares.
        for (name, catalog_id) in [
            ("notify", Some("example.com:shop")),
            ("refund", Some("example.com:shop")),
            ("pay", Some("example.com:other")),
            ("refund", None),
        ] {
            let mut function = json!({"@call": name, "args": {}});
            if let Some(catalog_id) = catalog_id {
                function["catalogId"] = json!(catalog_id);
            }
            let routed = router
                .route(&call(json!({
                    "surfaceId": "s", "functionCallId": "c2", "callFunction": function
                })))
                .unwrap();
            assert_eq!(
                reply_code(&routed),
                "UNKNOWN_FUNCTION",
                "{name} in {catalog_id:?}"
            );
        }
        // A call that does not read is answered when it can be, and an error
        // when it cannot.
        let malformed = router
            .route(&call(json!({"surfaceId": "s", "functionCallId": "c3"})))
            .unwrap();
        assert_eq!(reply_code(&malformed), "INVALID_FUNCTION_CALL");
        assert!(matches!(
            router.route(&call(json!({"surfaceId": "s"}))),
            Err(Error::Validation { .. })
        ));
    }

    #[cfg(feature = "schema-validation")]
    #[test]
    fn a_router_that_checks_schemas_validates_and_fills_in_defaults() {
        let router = RendererMessageRouter::new(&[catalog()])
            .unwrap()
            .checking_schemas()
            .unwrap();
        let pay = |args: Value| {
            call(
                json!({"surfaceId": "s", "functionCallId": "c1", "callFunction": {
                    "@call": "pay", "catalogId": "example.com:shop", "args": args
                }}),
            )
        };
        let Routed::Call(handed) = router
            .route(&pay(json!({"total": {"amount": 25}})))
            .unwrap()
        else {
            panic!("a call to run");
        };
        assert_eq!(
            Value::Object(handed.args),
            json!({"total": {"amount": 25, "currency": "USD"}, "memo": "standard"})
        );
        for refused in [
            json!({"total": "a lot"}),
            json!({"memo": "x"}),
            json!({"total": {"amount": 1}, "tip": 2}),
        ] {
            let routed = router.route(&pay(refused.clone())).unwrap();
            assert_eq!(reply_code(&routed), "INVALID_FUNCTION_CALL", "{refused}");
        }
    }

    #[test]
    fn the_other_messages_are_read_and_handed_over() {
        let router = RendererMessageRouter::new(&[catalog()]).unwrap();
        let action = json!({"version": "1.0", "action": {
            "name": "submit", "surfaceId": "s", "sourceComponentId": "button",
            "timestamp": "2026-01-01T00:00:00Z", "context": {"total": 3}
        }});
        assert_eq!(
            RendererMessageKind::of(&action).unwrap(),
            RendererMessageKind::Action
        );
        let Routed::Action(read) = router.route(&action).unwrap() else {
            panic!("an action");
        };
        assert_eq!(read.name, "submit");

        let response = json!({"version": "1.0", "rendererFunctionResponse": {
            "functionCallId": "call-1", "value": null
        }});
        let Routed::FunctionResponse(read) = router.route(&response).unwrap() else {
            panic!("a response");
        };
        assert_eq!(read, FunctionResponse::value("call-1", Value::Null));

        for not_one in [
            json!({"version": "v0.9.1", "action": {}}),
            json!({"version": "1.0"}),
            json!({"version": "1.0", "action": {}, "error": {}}),
            json!({"version": "1.0", "updateComponents": {}}),
            json!([]),
        ] {
            assert!(
                matches!(
                    RendererMessageKind::of(&not_one),
                    Err(Error::Validation { .. })
                ),
                "{not_one}"
            );
        }
    }
}
