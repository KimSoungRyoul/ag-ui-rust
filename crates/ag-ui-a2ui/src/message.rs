//! A2UI protocol envelopes.
//!
//! A2UI is a stream of JSON objects in two directions. Each object carries a
//! `version` discriminator plus exactly one payload key:
//!
//! | Direction | Payload keys |
//! |---|---|
//! | agent → renderer | `createSurface`, `updateComponents`, `updateDataModel`, `deleteSurface`, `callRendererFunction`, `agentFunctionResponse` |
//! | renderer → agent | `action`, `callAgentFunction`, `rendererFunctionResponse`, `error` |
//!
//! # The adjacency-list component model
//!
//! Components are sent as a **flat list**. Parent/child links are ID references,
//! never nesting — a `Card` names its child by id, a `Column` holds an array of
//! ids. The renderer stores every component in a map and rebuilds the tree at
//! render time, which is what lets the agent stream definitions in any order and
//! lets the renderer start painting as soon as `root` arrives.
//!
//! ```
//! use ag_ui_a2ui::message::{AgentMessage, Component};
//! use serde_json::json;
//!
//! let msg = AgentMessage::update_components(
//!     "profile",
//!     vec![
//!         Component::new("root", "Column").with("children", json!(["name"])),
//!         Component::new("name", "Text").with("text", json!("Ada")),
//!     ],
//! );
//! let wire = serde_json::to_value(&msg).unwrap();
//! assert_eq!(wire["version"], "v0.9");
//! assert_eq!(wire["updateComponents"]["components"][0]["component"], "Column");
//! ```

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::constants::PROTOCOL_VERSION;
use crate::error::{Error, Result};

fn default_version() -> String {
    PROTOCOL_VERSION.to_string()
}

/// One agent → renderer message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentMessage {
    /// Protocol version stamped on the wire; defaults to
    /// [`PROTOCOL_VERSION`].
    #[serde(default = "default_version")]
    pub version: String,
    /// The single payload key that gives this message its type.
    #[serde(flatten)]
    pub payload: AgentPayload,
}

impl AgentMessage {
    /// Wraps a payload with the current protocol version.
    pub fn new(payload: AgentPayload) -> Self {
        Self {
            version: default_version(),
            payload,
        }
    }

    /// `createSurface`: allocate a surface and fix its `catalogId`.
    ///
    /// Re-creating a `surfaceId` that already exists is an error per spec; see
    #[cfg_attr(
        feature = "toolkit",
        doc = "[`crate::toolkit::ops::assemble_ops`], which omits this message when the"
    )]
    #[cfg_attr(
        not(feature = "toolkit"),
        doc = "`toolkit::ops::assemble_ops` (behind the `toolkit` feature), which omits this message when the"
    )]
    /// intent is to update an existing surface.
    pub fn create_surface(surface_id: impl Into<String>, catalog_id: impl Into<String>) -> Self {
        Self::new(AgentPayload::CreateSurface(CreateSurface {
            surface_id: surface_id.into(),
            catalog_id: catalog_id.into(),
            theme: None,
            send_data_model: None,
        }))
    }

    /// `updateComponents`: add or replace components on an existing surface.
    pub fn update_components(surface_id: impl Into<String>, components: Vec<Component>) -> Self {
        Self::new(AgentPayload::UpdateComponents(UpdateComponents {
            surface_id: surface_id.into(),
            components,
        }))
    }

    /// `updateDataModel`: upsert `value` at `path` (JSON Pointer, `/` = whole model).
    pub fn update_data_model(
        surface_id: impl Into<String>,
        path: impl Into<String>,
        value: Value,
    ) -> Self {
        Self::new(AgentPayload::UpdateDataModel(UpdateDataModel {
            surface_id: surface_id.into(),
            path: path.into(),
            value,
        }))
    }

    /// `deleteSurface`: drop a surface and everything under it.
    pub fn delete_surface(surface_id: impl Into<String>) -> Self {
        Self::new(AgentPayload::DeleteSurface(DeleteSurface {
            surface_id: surface_id.into(),
        }))
    }

    /// The `surfaceId` this message targets, if it targets one.
    ///
    /// Function-call messages are addressed by `functionCallId` rather than by
    /// surface, so they return `None`.
    pub fn surface_id(&self) -> Option<&str> {
        match &self.payload {
            AgentPayload::CreateSurface(m) => Some(&m.surface_id),
            AgentPayload::UpdateComponents(m) => Some(&m.surface_id),
            AgentPayload::UpdateDataModel(m) => Some(&m.surface_id),
            AgentPayload::DeleteSurface(m) => Some(&m.surface_id),
            AgentPayload::CallRendererFunction(_) | AgentPayload::AgentFunctionResponse(_) => None,
        }
    }
}

/// The payload of an [`AgentMessage`], externally tagged by its wire key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentPayload {
    /// Create a surface and fix its catalog.
    CreateSurface(CreateSurface),
    /// Add or replace components on a surface.
    UpdateComponents(UpdateComponents),
    /// Upsert part of a surface's data model.
    UpdateDataModel(UpdateDataModel),
    /// Remove a surface entirely.
    DeleteSurface(DeleteSurface),
    /// Ask the renderer to run one of its local functions.
    CallRendererFunction(CallRendererFunction),
    /// Answer a renderer-initiated [`RendererPayload::CallAgentFunction`].
    AgentFunctionResponse(FunctionResponse),
}

/// Payload of a `createSurface` message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSurface {
    /// Globally unique surface identifier, for the renderer's lifetime.
    pub surface_id: String,
    /// Opaque identifier of the component catalog this surface speaks.
    ///
    /// Fixed for the life of the surface: changing it means deleting and
    /// recreating the surface.
    pub catalog_id: String,
    /// Catalog-defined theme parameters (`primaryColor`, `iconUrl`, ...).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<Value>,
    /// Ask the renderer to echo this surface's whole data model back with every
    /// message it sends to the creating agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_data_model: Option<bool>,
}

/// Payload of an `updateComponents` message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateComponents {
    /// Surface to update. It must already have been created.
    pub surface_id: String,
    /// Flat adjacency list of component definitions.
    pub components: Vec<Component>,
}

/// Payload of an `updateDataModel` message.
///
/// Upsert semantics: an existing path is replaced, a missing path is created,
/// and a `null` value deletes the key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDataModel {
    /// Surface whose data model is being updated.
    pub surface_id: String,
    /// JSON Pointer into the data model. Defaults to `/`, the whole model.
    #[serde(default = "root_pointer")]
    pub path: String,
    /// The new value. `null` deletes the key at `path`.
    #[serde(default)]
    pub value: Value,
}

fn root_pointer() -> String {
    "/".to_string()
}

impl UpdateDataModel {
    /// Applies this update to a surface data model in place.
    ///
    /// - `path` of `/` or `""` replaces the whole model (or clears it to `null`).
    /// - Missing intermediate objects are created.
    /// - A `null` value removes the key (or, in an array, sets the slot to
    ///   `null` so the array keeps its length, per spec).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Pointer`] when the pointer is malformed, when it walks
    /// through a scalar, or when an array index is not a number within bounds.
    pub fn apply(&self, model: &mut Value) -> Result<()> {
        apply_data_model_update(model, &self.path, &self.value)
    }
}

/// Applies one `updateDataModel` operation to `model`.
///
/// Split out from [`UpdateDataModel::apply`] so callers holding a loose
/// path/value pair (a replayed history entry, say) can reuse the semantics.
///
/// # Errors
///
/// See [`UpdateDataModel::apply`].
pub fn apply_data_model_update(model: &mut Value, path: &str, value: &Value) -> Result<()> {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        *model = value.clone();
        return Ok(());
    }
    if !trimmed.starts_with('/') {
        return Err(Error::pointer(
            trimmed,
            "updateDataModel path must be an absolute JSON Pointer starting with '/'",
        ));
    }

    let tokens = crate::binding::pointer_tokens(trimmed);
    let Some((last, parents)) = tokens.split_last() else {
        *model = value.clone();
        return Ok(());
    };

    let mut cursor = model;
    for token in parents {
        cursor = descend_or_create(cursor, token, trimmed)?;
    }

    match cursor {
        Value::Object(map) => {
            if value.is_null() {
                map.remove(last.as_str());
            } else {
                map.insert(last.clone(), value.clone());
            }
        }
        Value::Array(items) => {
            let idx = parse_index(last, trimmed)?;
            if idx < items.len() {
                // A null clears the slot without shortening the array.
                items[idx] = value.clone();
            } else if idx == items.len() {
                items.push(value.clone());
            } else {
                return Err(Error::pointer(
                    trimmed,
                    format!("array index {idx} is out of bounds (len {})", items.len()),
                ));
            }
        }
        Value::Null => {
            let mut map = Map::new();
            if !value.is_null() {
                map.insert(last.clone(), value.clone());
            }
            *cursor = Value::Object(map);
        }
        _ => {
            return Err(Error::pointer(trimmed, "path walks through a scalar value"));
        }
    }
    Ok(())
}

fn descend_or_create<'a>(
    cursor: &'a mut Value,
    token: &str,
    full_path: &str,
) -> Result<&'a mut Value> {
    if cursor.is_null() {
        *cursor = Value::Object(Map::new());
    }
    match cursor {
        Value::Object(map) => Ok(map.entry(token.to_string()).or_insert(Value::Null)),
        Value::Array(items) => {
            let idx = parse_index(token, full_path)?;
            let len = items.len();
            if idx == len {
                items.push(Value::Null);
            }
            items.get_mut(idx).ok_or_else(|| {
                Error::pointer(
                    full_path,
                    format!("array index {idx} is out of bounds (len {len})"),
                )
            })
        }
        _ => Err(Error::pointer(
            full_path,
            "path walks through a scalar value",
        )),
    }
}

fn parse_index(token: &str, full_path: &str) -> Result<usize> {
    token.parse::<usize>().map_err(|_| {
        Error::pointer(
            full_path,
            format!("expected an array index, found segment {token:?}"),
        )
    })
}

/// Payload of a `deleteSurface` message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteSurface {
    /// Surface to remove.
    pub surface_id: String,
}

/// Payload of a `callRendererFunction` message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallRendererFunction {
    /// Correlation id; the renderer copies it into its response.
    pub function_call_id: String,
    /// The function to invoke, with its arguments.
    pub call_function: FunctionCall,
}

/// Payload of a `callAgentFunction` message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallAgentFunction {
    /// Surface the call originated from.
    pub surface_id: String,
    /// Correlation id; the agent copies it into its response.
    pub function_call_id: String,
    /// The function to invoke, with its arguments.
    pub call_function: FunctionCall,
}

/// A named function invocation, used both in component properties and in the
/// two `call*Function` messages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionCall {
    /// Function name, e.g. `formatString`, `required`, `@index`.
    pub call: String,
    /// Named arguments. Values may themselves be bindings or nested calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Map<String, Value>>,
    /// Catalog the function is drawn from, when it is not the surface default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_id: Option<String>,
    /// Expected return type, used to disambiguate overloads on the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_type: Option<String>,
}

/// The result of a `call*Function`, sent back by whichever side ran it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionResponse {
    /// Correlation id copied from the originating call.
    pub function_call_id: String,
    /// Whatever the function returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Set instead of `result` when the call failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One renderer → agent message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RendererMessage {
    /// Protocol version stamped on the wire.
    #[serde(default = "default_version")]
    pub version: String,
    /// The single payload key that gives this message its type.
    #[serde(flatten)]
    pub payload: RendererPayload,
}

impl RendererMessage {
    /// Wraps a payload with the current protocol version.
    pub fn new(payload: RendererPayload) -> Self {
        Self {
            version: default_version(),
            payload,
        }
    }

    /// The `surfaceId` this message relates to, if any.
    pub fn surface_id(&self) -> Option<&str> {
        match &self.payload {
            RendererPayload::Action(a) => Some(&a.surface_id),
            RendererPayload::CallAgentFunction(c) => Some(&c.surface_id),
            RendererPayload::RendererFunctionResponse(_) => None,
            RendererPayload::Error(e) => e.surface_id.as_deref(),
        }
    }
}

/// The payload of a [`RendererMessage`], externally tagged by its wire key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RendererPayload {
    /// A user interacted with a component that declares an `action`.
    Action(Action),
    /// The renderer wants the agent to run a function on its behalf.
    CallAgentFunction(CallAgentFunction),
    /// Result of an agent-initiated [`AgentPayload::CallRendererFunction`].
    RendererFunctionResponse(FunctionResponse),
    /// The renderer is reporting a problem, typically a failed validation.
    Error(RendererError),
}

/// Payload of an `action` message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    /// Action name, taken from the component's `action.event.name`.
    pub name: String,
    /// Surface the interaction happened on.
    pub surface_id: String,
    /// Component that triggered it.
    pub source_component_id: String,
    /// ISO 8601 timestamp of the interaction.
    pub timestamp: String,
    /// The component's `action.event.context` with all bindings resolved.
    #[serde(default)]
    pub context: Map<String, Value>,
    /// Human-readable description of what the user did, if the component
    /// supplied one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_message: Option<String>,
}

/// Payload of an `error` message from the renderer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RendererError {
    /// Machine-readable code, e.g. `VALIDATION_FAILED`, `UNALLOWED_PARENT`.
    pub code: String,
    /// One or two sentences the agent (or its model) can act on.
    pub message: String,
    /// Surface the error relates to, for surface-scoped errors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_id: Option<String>,
    /// JSON Pointer to the offending field, for validation errors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Correlation id, for errors raised while running a function call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function_call_id: Option<String>,
}

/// One node of the flat component adjacency list.
///
/// `id` and `component` are the only fixed fields; everything else is
/// catalog-defined and lives in [`Component::props`] as raw JSON. The struct
/// flattens back to the wire shape `{"id": ..., "component": "Text", "text": ...}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Component {
    /// Unique id within the surface; other components reference it by this.
    pub id: String,
    /// Component type name, resolved against the surface's catalog.
    pub component: String,
    /// Catalog-defined properties, verbatim.
    #[serde(flatten)]
    pub props: Map<String, Value>,
}

impl Component {
    /// Creates a component with no properties yet.
    pub fn new(id: impl Into<String>, component: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            component: component.into(),
            props: Map::new(),
        }
    }

    /// Sets a property, builder style.
    #[must_use]
    pub fn with(mut self, key: impl Into<String>, value: Value) -> Self {
        self.props.insert(key.into(), value);
        self
    }

    /// Borrows a property by name.
    pub fn prop(&self, key: &str) -> Option<&Value> {
        self.props.get(key)
    }
}

/// How a component declares its children.
///
/// Either a static list of ids, or a template: one component id instantiated
/// once per item of the array at `path`. The template form is what creates a
/// collection scope for relative data bindings (see [`crate::binding`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ChildList {
    /// A fixed set of child component ids.
    Ids(Vec<String>),
    /// A template instantiated once per element of a bound array.
    Template(ChildTemplate),
}

/// The template form of a [`ChildList`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildTemplate {
    /// Id of the component to instantiate per item.
    pub component_id: String,
    /// JSON Pointer to the array to iterate.
    pub path: String,
}

impl ChildList {
    /// Parses a raw `children` property value, if it is a well-formed child list.
    pub fn from_value(value: &Value) -> Option<Self> {
        serde_json::from_value(value.clone()).ok()
    }

    /// Every component id this child list references.
    pub fn referenced_ids(&self) -> Vec<&str> {
        match self {
            ChildList::Ids(ids) => ids.iter().map(String::as_str).collect(),
            ChildList::Template(t) => vec![t.component_id.as_str()],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn agent_message_round_trips_through_the_wire_shape() {
        let msg = AgentMessage::create_surface("s1", "cat");
        let wire = serde_json::to_value(&msg).unwrap();
        assert_eq!(
            wire,
            json!({"version": "v0.9", "createSurface": {"surfaceId": "s1", "catalogId": "cat"}})
        );
        let back: AgentMessage = serde_json::from_value(wire).unwrap();
        assert_eq!(back, msg);
    }

    #[test]
    fn components_keep_catalog_props_flat() {
        let wire = json!({"id": "t", "component": "Text", "text": "hi", "variant": "h1"});
        let c: Component = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(c.prop("variant"), Some(&json!("h1")));
        assert_eq!(serde_json::to_value(&c).unwrap(), wire);
    }

    #[test]
    fn update_data_model_defaults_path_to_root() {
        let msg: AgentMessage = serde_json::from_value(json!({
            "version": "v0.9",
            "updateDataModel": {"surfaceId": "s", "value": {"a": 1}}
        }))
        .unwrap();
        let AgentPayload::UpdateDataModel(m) = msg.payload else {
            panic!("expected updateDataModel");
        };
        assert_eq!(m.path, "/");
    }

    #[test]
    fn upsert_creates_missing_intermediates_and_null_deletes() {
        let mut model = json!({});
        apply_data_model_update(&mut model, "/user/name", &json!("Ada")).unwrap();
        assert_eq!(model, json!({"user": {"name": "Ada"}}));

        apply_data_model_update(&mut model, "/user/name", &Value::Null).unwrap();
        assert_eq!(model, json!({"user": {}}));

        apply_data_model_update(&mut model, "/", &json!({"replaced": true})).unwrap();
        assert_eq!(model, json!({"replaced": true}));
    }

    #[test]
    fn upsert_into_arrays_preserves_length_on_delete() {
        let mut model = json!({"items": [1, 2, 3]});
        apply_data_model_update(&mut model, "/items/1", &Value::Null).unwrap();
        assert_eq!(model, json!({"items": [1, null, 3]}));

        apply_data_model_update(&mut model, "/items/3", &json!(4)).unwrap();
        assert_eq!(model, json!({"items": [1, null, 3, 4]}));

        let err = apply_data_model_update(&mut model, "/items/9", &json!(0)).unwrap_err();
        assert!(matches!(err, Error::Pointer { .. }));
    }

    #[test]
    fn escaped_pointer_tokens_are_decoded() {
        let mut model = json!({});
        apply_data_model_update(&mut model, "/a~1b", &json!(1)).unwrap();
        assert_eq!(model, json!({"a/b": 1}));
    }

    #[test]
    fn child_list_parses_both_forms() {
        assert_eq!(
            ChildList::from_value(&json!(["a", "b"]))
                .unwrap()
                .referenced_ids(),
            vec!["a", "b"]
        );
        assert_eq!(
            ChildList::from_value(&json!({"componentId": "tpl", "path": "/items"}))
                .unwrap()
                .referenced_ids(),
            vec!["tpl"]
        );
        assert!(ChildList::from_value(&json!("nope")).is_none());
    }

    #[test]
    fn renderer_messages_round_trip() {
        let wire = json!({
            "version": "v0.9",
            "action": {
                "name": "submit",
                "surfaceId": "s1",
                "sourceComponentId": "btn",
                "timestamp": "2026-01-01T00:00:00Z",
                "context": {"email": "a@b.c"}
            }
        });
        let msg: RendererMessage = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(msg.surface_id(), Some("s1"));
        assert_eq!(serde_json::to_value(&msg).unwrap(), wire);
    }
}
