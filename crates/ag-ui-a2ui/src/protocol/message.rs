//! A2UI protocol envelopes.
//!
//! A2UI is a stream of JSON objects in two directions. Each object carries a
//! `version` discriminator plus exactly one payload key:
//!
//! | Direction | Payload keys |
//! |---|---|
//! | agent → renderer | `createSurface`, `updateComponents`, `updateDataModel`, `deleteSurface` |
//! | renderer → agent | `action`, `error` |
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
//!
//! # One set of types, two wire families
//!
//! `v0.9` and `v0.9.1` are one wire format and `1.0` is another. The types
//! here carry both, and an [`AgentMessage`] is written and read as its own
//! `version` spells it: nothing is translated from one family to the other.
//! What differs is asked of [`A2uiVersion`]:
//!
//! | | `v0.9`, `v0.9.1` | `1.0` |
//! |---|---|---|
//! | `createSurface` | `catalogId` required, `theme` | `catalogId` optional; `components`, `dataModel`, `metadata`; no `theme` |
//! | Removing from the data model | `value` left out; `null` is stored | `"value": null`; `value` is required |
//! | A binding, a call | `{"path": …}`, `{"call": …}` | `{"@path": …}`, `{"@call": …}` |
//! | A component's envelope | `accessibility`, `checks` | `@catalogId`, `@accessibility`, `@checks`, `@metadata` |
//!
//! A [`Component`] is a flat map in both, so a binding is whatever its
//! property holds; [`A2uiVersion::path`] and [`A2uiVersion::call`] write one
//! in a version's spelling.
//!
//! ```
//! use ag_ui_a2ui::message::{AgentMessage, AgentPayload, Component, CreateSurface};
//! use ag_ui_a2ui::A2uiVersion;
//! use serde_json::json;
//!
//! let version = A2uiVersion::V1_0;
//! let create = CreateSurface::new("profile")
//!     .with_catalog_id(version.basic_catalog_id())
//!     .with_components(vec![Component::new("root", "Text").with("text", version.path("/name"))])
//!     .with_data_model(json!({"name": "Ada"}));
//! let message = AgentMessage::new(AgentPayload::CreateSurface(create)).with_version(version);
//!
//! let wire = serde_json::to_value(&message)?;
//! assert_eq!(wire["version"], "1.0");
//! assert_eq!(wire["createSurface"]["components"][0]["text"], json!({"@path": "/name"}));
//! assert_eq!(serde_json::from_value::<AgentMessage>(wire)?, message);
//!
//! // Removing is `null` in 1.0 and an absent `value` before it.
//! let remove = AgentMessage::remove_data_model_value("profile", "/name");
//! assert_eq!(
//!     serde_json::to_value(remove.clone().with_version(version))?["updateDataModel"],
//!     json!({"surfaceId": "profile", "path": "/name", "value": null})
//! );
//! assert_eq!(
//!     serde_json::to_value(remove)?["updateDataModel"],
//!     json!({"surfaceId": "profile", "path": "/name"})
//! );
//! # Ok::<(), serde_json::Error>(())
//! ```

use serde::ser::SerializeMap as _;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::operation::Operation;
pub use super::version::A2uiVersion;
use crate::error::Result;

fn serialize_version<S: serde::Serializer>(
    version: &str,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    if A2uiVersion::parse(version).is_none() {
        return Err(serde::ser::Error::custom("unsupported A2UI version"));
    }
    serializer.serialize_str(version)
}
fn deserialize_version<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<String, D::Error> {
    let version = String::deserialize(deserializer)?;
    if A2uiVersion::parse(&version).is_none() {
        return Err(serde::de::Error::custom("unsupported A2UI version"));
    }
    Ok(version)
}
fn default_version() -> String {
    A2uiVersion::DEFAULT.as_str().to_string()
}

/// One agent → renderer message.
///
/// Written and read as its `version` spells it. Serializing fails for a
/// version this crate does not speak, and for a payload the version cannot
/// say: a `theme` in 1.0, or inline `components` before it. Reading drops what
/// the message's version does not have, as it drops any field it does not
/// know; a [`MessageProcessor`](crate::processor::MessageProcessor) is what
/// refuses such a message.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentMessage {
    /// Protocol version stamped on the wire; defaults to
    /// [`A2uiVersion::DEFAULT`].
    pub version: String,
    /// The single payload key that gives this message its type.
    pub payload: AgentPayload,
}

impl Serialize for AgentMessage {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::Error as _;

        let Some(version) = A2uiVersion::parse(&self.version) else {
            return Err(S::Error::custom("unsupported A2UI version"));
        };
        if let Some(reason) = self.payload.unsayable_in(version) {
            return Err(S::Error::custom(reason));
        }
        let mut message = serializer.serialize_map(Some(2))?;
        message.serialize_entry("version", &self.version)?;
        match &self.payload {
            AgentPayload::CreateSurface(create) => {
                message.serialize_entry(Operation::CreateSurface.as_str(), create)?;
            }
            AgentPayload::UpdateComponents(update) => {
                message.serialize_entry(Operation::UpdateComponents.as_str(), update)?;
            }
            AgentPayload::UpdateDataModel(update) if version.null_removes() => {
                message.serialize_entry(
                    Operation::UpdateDataModel.as_str(),
                    &RemovingWithNull(update),
                )?;
            }
            AgentPayload::UpdateDataModel(update) => {
                message.serialize_entry(Operation::UpdateDataModel.as_str(), update)?;
            }
            AgentPayload::DeleteSurface(delete) => {
                message.serialize_entry(Operation::DeleteSurface.as_str(), delete)?;
            }
            AgentPayload::CallRendererFunction(_) | AgentPayload::AgentFunctionResponse(_) => {
                return Err(S::Error::custom(
                    "function-call messages are not part of the wire this crate writes",
                ));
            }
        }
        message.end()
    }
}

impl<'de> Deserialize<'de> for AgentMessage {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        use serde::de::Error as _;

        let mut fields = Map::<String, Value>::deserialize(deserializer)?;
        let version = match fields.remove("version") {
            Some(Value::String(version)) => version,
            Some(_) => return Err(D::Error::custom("`version` must be a string")),
            None => return Err(D::Error::missing_field("version")),
        };
        let Some(spoken) = A2uiVersion::parse(&version) else {
            return Err(D::Error::custom("unsupported A2UI version"));
        };
        let mut payloads = fields.into_iter();
        let Some((key, payload)) = payloads.next() else {
            return Err(D::Error::custom("a message carries one payload key"));
        };
        if let Some((other, _)) = payloads.next() {
            return Err(D::Error::custom(format!(
                "a message carries one payload key, not `{key}` and `{other}`"
            )));
        }
        let payload = AgentPayload::read(spoken, &key, payload).map_err(D::Error::custom)?;
        Ok(Self { version, payload })
    }
}

/// An `updateDataModel` payload as v1.0 writes it: `value` is always there,
/// and `null` removes.
struct RemovingWithNull<'a>(&'a UpdateDataModel);

impl Serialize for RemovingWithNull<'_> {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        let mut payload = serializer.serialize_map(Some(3))?;
        payload.serialize_entry("surfaceId", &self.0.surface_id)?;
        payload.serialize_entry("path", &self.0.path)?;
        match &self.0.value {
            DataModelUpdate::Set(value) => payload.serialize_entry("value", value)?,
            DataModelUpdate::Remove => payload.serialize_entry("value", &Value::Null)?,
        }
        payload.end()
    }
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
    /// Re-creating a `surfaceId` that already exists is an error per spec: a
    /// surface that is live is changed with `updateComponents` and
    /// `updateDataModel` alone.
    ///
    /// For what else a v1.0 `createSurface` may carry, build the payload:
    /// [`CreateSurface::new`].
    pub fn create_surface(surface_id: impl Into<String>, catalog_id: impl Into<String>) -> Self {
        Self::new(AgentPayload::CreateSurface(
            CreateSurface::new(surface_id).with_catalog_id(catalog_id),
        ))
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
            value: DataModelUpdate::Set(value),
        }))
    }

    /// Removes a data-model location: the wire `value` is left out, or is
    /// `null` in a v1.0 message.
    pub fn remove_data_model_value(surface_id: impl Into<String>, path: impl Into<String>) -> Self {
        Self::new(AgentPayload::UpdateDataModel(UpdateDataModel {
            surface_id: surface_id.into(),
            path: path.into(),
            value: DataModelUpdate::Remove,
        }))
    }

    /// Selects the supported wire version explicitly.
    #[must_use]
    pub fn with_version(mut self, version: A2uiVersion) -> Self {
        self.version = version.as_str().into();
        self
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
///
/// The keys are the names [`Operation`] lists; [`AgentPayload::operation`]
/// says which one a payload is.
///
/// A payload on its own has no version. Serialized or deserialized by itself
/// it is spelled as the v0.9 family spells it; the message is what knows
/// better.
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
    /// Candidate RPC shape, excluded from v0.9-family serialization/deserialization.
    #[serde(skip)]
    CallRendererFunction(CallRendererFunction),
    /// Candidate RPC response, excluded from v0.9-family serialization/deserialization.
    #[serde(skip)]
    AgentFunctionResponse(FunctionResponse),
}

impl AgentPayload {
    /// The operation this payload is, by name.
    ///
    /// `None` for the candidate RPC shapes, which are not operations of the
    /// v0.9 family and never reach the wire.
    pub fn operation(&self) -> Option<Operation> {
        match self {
            Self::CreateSurface(_) => Some(Operation::CreateSurface),
            Self::UpdateComponents(_) => Some(Operation::UpdateComponents),
            Self::UpdateDataModel(_) => Some(Operation::UpdateDataModel),
            Self::DeleteSurface(_) => Some(Operation::DeleteSurface),
            Self::CallRendererFunction(_) | Self::AgentFunctionResponse(_) => None,
        }
    }

    /// Why this payload cannot be written in `version`, when it cannot.
    fn unsayable_in(&self, version: A2uiVersion) -> Option<String> {
        let Self::CreateSurface(create) = self else {
            return None;
        };
        if create.catalog_id.is_none() && !version.mixes_catalogs() {
            return Some(format!(
                "a {version} createSurface names its catalog: `catalogId` is required"
            ));
        }
        if create.theme.is_some() && !version.has_theme() {
            return Some(format!("a {version} createSurface has no `theme`"));
        }
        let inline = [
            ("components", !create.components.is_empty()),
            ("dataModel", create.data_model.is_some()),
            ("metadata", create.metadata.is_some()),
        ];
        inline
            .into_iter()
            .find(|(_, present)| *present && !version.creates_inline())
            .map(|(field, _)| format!("a {version} createSurface has no `{field}`"))
    }

    /// Reads the payload under `key` as `version` spells it.
    fn read(version: A2uiVersion, key: &str, payload: Value) -> std::result::Result<Self, String> {
        let decoded = |error: serde_json::Error| error.to_string();
        match Operation::parse(key) {
            Some(Operation::CreateSurface) => {
                let mut create: CreateSurface = serde_json::from_value(payload).map_err(decoded)?;
                if create.catalog_id.is_none() && !version.mixes_catalogs() {
                    return Err("missing field `catalogId`".to_string());
                }
                // What this version does not have is dropped, as an unknown
                // field is.
                if !version.has_theme() {
                    create.theme = None;
                }
                if !version.creates_inline() {
                    create.components.clear();
                    create.data_model = None;
                    create.metadata = None;
                }
                Ok(Self::CreateSurface(create))
            }
            Some(Operation::UpdateComponents) => serde_json::from_value(payload)
                .map(Self::UpdateComponents)
                .map_err(decoded),
            Some(Operation::UpdateDataModel) => {
                let stated = payload.get("value").is_some();
                let mut update: UpdateDataModel =
                    serde_json::from_value(payload).map_err(decoded)?;
                if version.null_removes() {
                    if !stated {
                        return Err("missing field `value`".to_string());
                    }
                    if update.value == DataModelUpdate::Set(Value::Null) {
                        update.value = DataModelUpdate::Remove;
                    }
                }
                Ok(Self::UpdateDataModel(update))
            }
            Some(Operation::DeleteSurface) => serde_json::from_value(payload)
                .map(Self::DeleteSurface)
                .map_err(decoded),
            None => {
                let known: Vec<String> = version
                    .operations()
                    .iter()
                    .map(|operation| format!("`{operation}`"))
                    .collect();
                Err(format!(
                    "unknown variant `{key}`, expected one of {}",
                    known.join(", ")
                ))
            }
        }
    }
}

/// Payload of a `createSurface` message.
///
/// Non-exhaustive: a later protocol version may give it more. Build one with
/// [`CreateSurface::new`] and the `with_` methods.
///
/// ```
/// use ag_ui_a2ui::message::{Component, CreateSurface};
/// use serde_json::json;
///
/// let create = CreateSurface::new("cart")
///     .with_catalog_id("https://example.com/catalogs/shop")
///     .with_components(vec![Component::new("root", "Text").with("text", json!("Cart"))]);
/// assert_eq!(create.catalog_id.as_deref(), Some("https://example.com/catalogs/shop"));
/// assert_eq!(create.components.len(), 1);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CreateSurface {
    /// Surface identifier, unique among active surfaces in this renderer context.
    pub surface_id: String,
    /// Opaque identifier of the component catalog this surface speaks.
    ///
    /// Fixed for the life of the surface: changing it means deleting and
    /// recreating the surface. Required before v1.0. From v1.0 it is the
    /// surface's default, and `None` leaves every component and function call
    /// to name its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_id: Option<String>,
    /// Catalog-defined theme parameters (`primaryColor`, `iconUrl`, ...).
    /// The v0.9 family only: v1.0 has no theme.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<Value>,
    /// Ask the renderer to echo this surface's whole data model back with every
    /// message it sends to the creating agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_data_model: Option<bool>,
    /// The surface's first components, from v1.0: what an `updateComponents`
    /// right after the creation would carry. Empty sends none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<Component>,
    /// The surface's first data model, from v1.0. The renderer applies it
    /// before `components`, so they bind against it at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_model: Option<Map<String, Value>>,
    /// Surface-level metadata, from v1.0: an object with an `extensions` map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

impl CreateSurface {
    /// A creation that names the surface and nothing else.
    pub fn new(surface_id: impl Into<String>) -> Self {
        Self {
            surface_id: surface_id.into(),
            catalog_id: None,
            theme: None,
            send_data_model: None,
            components: Vec::new(),
            data_model: None,
            metadata: None,
        }
    }

    /// Names the surface's catalog: the only one before v1.0, the default one
    /// from v1.0.
    #[must_use]
    pub fn with_catalog_id(mut self, catalog_id: impl Into<String>) -> Self {
        self.catalog_id = Some(catalog_id.into());
        self
    }

    /// Sets the theme. The v0.9 family only.
    #[must_use]
    pub fn with_theme(mut self, theme: Value) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Asks the renderer to send the surface's data model back with what it
    /// sends.
    #[must_use]
    pub fn with_send_data_model(mut self, send: bool) -> Self {
        self.send_data_model = Some(send);
        self
    }

    /// Carries the surface's first components. From v1.0.
    #[must_use]
    pub fn with_components(mut self, components: Vec<Component>) -> Self {
        self.components = components;
        self
    }

    /// Carries the surface's first data model. From v1.0. A value that is not
    /// an object sets none: the root of a data model is an object.
    #[must_use]
    pub fn with_data_model(mut self, data_model: Value) -> Self {
        self.data_model = match data_model {
            Value::Object(data_model) => Some(data_model),
            _ => None,
        };
        self
    }

    /// Carries surface-level metadata. From v1.0.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
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

/// What an `updateDataModel` does at its path: store a value, or remove what
/// is there.
///
/// The two families write a removal differently. Before v1.0 the `value` field
/// is left out, and an explicit `null` is a value that is stored. In v1.0
/// `"value": null` removes, so a v1.0 message cannot store a `null` at the
/// path itself. An [`AgentMessage`] reads and writes either; this type is the
/// same for both.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum DataModelUpdate {
    /// Store the exact JSON value. Before v1.0 that includes `null`.
    Set(Value),
    /// Remove the target.
    #[default]
    Remove,
}
impl DataModelUpdate {
    /// Whether the update removes a value.
    pub fn is_remove(&self) -> bool {
        matches!(self, Self::Remove)
    }
}
impl Serialize for DataModelUpdate {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Set(v) => v.serialize(serializer),
            Self::Remove => Err(serde::ser::Error::custom(
                "Remove must be serialized as an omitted updateDataModel value",
            )),
        }
    }
}
impl<'de> Deserialize<'de> for DataModelUpdate {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Ok(Self::Set(Value::deserialize(deserializer)?))
    }
}
/// Payload of an `updateDataModel` message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateDataModel {
    /// Surface whose data model is being updated.
    pub surface_id: String,
    /// Absolute JSON Pointer; `/` and omission denote the whole model.
    #[serde(default = "root_pointer")]
    pub path: String,
    /// A value to store, or the removal of what is at `path`.
    #[serde(default, skip_serializing_if = "DataModelUpdate::is_remove")]
    pub value: DataModelUpdate,
}
fn root_pointer() -> String {
    "/".to_string()
}
impl UpdateDataModel {
    /// Applies an update losslessly, including undefined roots and array slots.
    pub fn apply_model(&self, model: &mut crate::protocol::model::DataModel) -> Result<()> {
        model.apply(&self.path, &self.value)
    }
    /// Applies to plain JSON atomically. Undefined results are rejected; use
    /// `apply_model` when deletions can affect array slots or the root.
    pub fn apply(&self, model: &mut Value) -> Result<()> {
        let mut exact = crate::protocol::model::DataModel::from(model.clone());
        self.apply_model(&mut exact)?;
        *model = exact.to_json()?;
        Ok(())
    }
}
/// Stores an exact JSON value at a path. Null is stored rather than removed.
/// A failed pointer leaves the original model intact.
pub fn apply_data_model_update(model: &mut Value, path: &str, value: &Value) -> Result<()> {
    UpdateDataModel {
        surface_id: String::new(),
        path: path.into(),
        value: DataModelUpdate::Set(value.clone()),
    }
    .apply(model)
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
#[serde(deny_unknown_fields)]
pub struct RendererMessage {
    /// Protocol version stamped on the wire.
    #[serde(
        serialize_with = "serialize_version",
        deserialize_with = "deserialize_version"
    )]
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

    /// Validates the original client message against the pinned official schema
    /// before decoding. Serde alone permits omitted optional fields and defaults.
    #[cfg(feature = "schema-validation")]
    pub fn from_json(value: &Value) -> Result<Self> {
        crate::protocol::client_schema::ClientSchemaValidator::new()?.decode_message(value)
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
    #[serde(skip)]
    CallAgentFunction(CallAgentFunction),
    /// Result of an agent-initiated [`AgentPayload::CallRendererFunction`].
    #[serde(skip)]
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
    /// Additional action fields, including renderer-specific `userMessage` data.
    #[serde(flatten)]
    pub extensions: Map<String, Value>,
}

/// Payload of an `error` message from the renderer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RendererError {
    /// Machine-readable string code, as used by the reference renderer.
    /// Examples include `VALIDATION_FAILED` and `UNALLOWED_PARENT`.
    pub code: String,
    /// One or two sentences the agent (or its model) can act on.
    pub message: String,
    /// Surface the error relates to, for surface-scoped errors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_id: Option<String>,
    /// Error-specific fields such as `path`, `functionCallId`, and component IDs.
    /// Generic errors may attach arbitrary JSON here, including explicit null.
    #[serde(flatten)]
    pub extensions: Map<String, Value>,
}

impl RendererError {
    /// The JSON Pointer provided by a validation error, when it is a string.
    /// Generic errors retain other `path` values in [`Self::extensions`].
    pub fn path(&self) -> Option<&str> {
        self.extensions.get("path").and_then(Value::as_str)
    }
}

/// Renderer data-model snapshots carried as `a2uiClientDataModel` metadata.
/// Surfaces that enable `sendDataModel` supply their current JSON object here.
/// The application selects which authorized surface snapshots to apply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientDataModel {
    /// Supported v0.9-family wire version.
    #[serde(
        serialize_with = "serialize_version",
        deserialize_with = "deserialize_version"
    )]
    pub version: String,
    /// Current data-model objects keyed by renderer-scoped surface ID.
    pub surfaces: std::collections::BTreeMap<String, Map<String, Value>>,
}

impl ClientDataModel {
    /// Empty client metadata for the selected protocol version.
    pub fn new(version: A2uiVersion) -> Self {
        Self {
            version: version.as_str().into(),
            surfaces: Default::default(),
        }
    }

    /// Validates the original metadata against the pinned official schema.
    #[cfg(feature = "schema-validation")]
    pub fn from_json(value: &Value) -> Result<Self> {
        crate::protocol::client_schema::ClientSchemaValidator::new()?.decode_data_model(value)
    }
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
    fn a_v1_0_message_is_written_and_read_in_its_own_spelling() {
        let version = A2uiVersion::V1_0;
        let create = CreateSurface::new("s1")
            .with_components(vec![
                Component::new("root", "Text").with("text", version.path("/title")),
            ])
            .with_data_model(json!({"title": "Hello"}))
            .with_metadata(json!({"extensions": {"com_example": 1}}));
        let message = AgentMessage::new(AgentPayload::CreateSurface(create)).with_version(version);
        let wire = serde_json::to_value(&message).unwrap();
        assert_eq!(
            wire,
            json!({"version": "1.0", "createSurface": {
                "surfaceId": "s1",
                "components": [{"id": "root", "component": "Text", "text": {"@path": "/title"}}],
                "dataModel": {"title": "Hello"},
                "metadata": {"extensions": {"com_example": 1}}
            }})
        );
        assert_eq!(
            serde_json::from_value::<AgentMessage>(wire).unwrap(),
            message
        );

        // The same payload cannot be said in v0.9, and a theme cannot in 1.0.
        let v0_9 = AgentMessage {
            version: "v0.9".into(),
            ..message.clone()
        };
        let refused = serde_json::to_value(&v0_9).unwrap_err().to_string();
        assert!(refused.contains("`catalogId` is required"), "{refused}");
        let themed = AgentMessage::new(AgentPayload::CreateSurface(
            CreateSurface::new("s1")
                .with_catalog_id("c")
                .with_theme(json!({"primaryColor": "#000"})),
        ));
        assert!(serde_json::to_value(&themed).is_ok());
        let refused = serde_json::to_value(themed.with_version(version))
            .unwrap_err()
            .to_string();
        assert!(refused.contains("has no `theme`"), "{refused}");
    }

    #[test]
    fn reading_drops_what_the_version_of_the_message_does_not_have() {
        let v0_9: AgentMessage = serde_json::from_value(json!({
            "version": "v0.9",
            "createSurface": {
                "surfaceId": "s", "catalogId": "c", "theme": {"primaryColor": "#000"},
                "components": [{"id": "root", "component": "Text"}], "dataModel": {"a": 1}
            }
        }))
        .unwrap();
        let AgentPayload::CreateSurface(create) = &v0_9.payload else {
            panic!("expected createSurface");
        };
        assert!(create.theme.is_some());
        assert!(create.components.is_empty() && create.data_model.is_none());

        let v1_0: AgentMessage = serde_json::from_value(json!({
            "version": "1.0",
            "createSurface": {"surfaceId": "s", "theme": {"primaryColor": "#000"}}
        }))
        .unwrap();
        let AgentPayload::CreateSurface(create) = &v1_0.payload else {
            panic!("expected createSurface");
        };
        assert!(create.theme.is_none() && create.catalog_id.is_none());

        // Before v1.0 a surface names its catalog.
        let error = serde_json::from_value::<AgentMessage>(
            json!({"version": "v0.9.1", "createSurface": {"surfaceId": "s"}}),
        )
        .unwrap_err();
        assert!(error.to_string().contains("catalogId"), "{error}");
    }

    #[test]
    fn null_is_stored_before_v1_0_and_removes_from_it() {
        let update = |version: &str, payload: Value| {
            serde_json::from_value::<AgentMessage>(
                json!({"version": version, "updateDataModel": payload}),
            )
        };
        let value_of = |message: AgentMessage| match message.payload {
            AgentPayload::UpdateDataModel(update) => update.value,
            other => panic!("expected updateDataModel, got {other:?}"),
        };

        let stored = update(
            "v0.9",
            json!({"surfaceId": "s", "path": "/a", "value": null}),
        );
        assert_eq!(value_of(stored.unwrap()), DataModelUpdate::Set(Value::Null));
        let omitted = update("v0.9", json!({"surfaceId": "s", "path": "/a"}));
        assert_eq!(value_of(omitted.unwrap()), DataModelUpdate::Remove);

        let removed = update(
            "1.0",
            json!({"surfaceId": "s", "path": "/a", "value": null}),
        );
        assert_eq!(value_of(removed.unwrap()), DataModelUpdate::Remove);
        let nested = update(
            "1.0",
            json!({"surfaceId": "s", "path": "/a", "value": {"b": null}}),
        );
        assert_eq!(
            value_of(nested.unwrap()),
            DataModelUpdate::Set(json!({"b": null})),
            "only a null at the path itself removes"
        );
        let omitted = update("1.0", json!({"surfaceId": "s", "path": "/a"})).unwrap_err();
        assert!(
            omitted.to_string().contains("missing field `value`"),
            "{omitted}"
        );

        // Written back, each in its own spelling.
        let remove = AgentMessage::remove_data_model_value("s", "/a");
        assert_eq!(
            serde_json::to_string(&remove).unwrap(),
            r#"{"version":"v0.9","updateDataModel":{"surfaceId":"s","path":"/a"}}"#
        );
        assert_eq!(
            serde_json::to_string(&remove.with_version(A2uiVersion::V1_0)).unwrap(),
            r#"{"version":"1.0","updateDataModel":{"surfaceId":"s","path":"/a","value":null}}"#
        );
    }

    #[test]
    fn a_message_is_one_version_and_one_payload() {
        for (wire, reason) in [
            (
                json!({"createSurface": {"surfaceId": "s", "catalogId": "c"}}),
                "version",
            ),
            (
                json!({"version": "v0.8", "deleteSurface": {"surfaceId": "s"}}),
                "unsupported",
            ),
            (
                json!({"version": 1.0, "deleteSurface": {"surfaceId": "s"}}),
                "string",
            ),
            (json!({"version": "v0.9"}), "one payload key"),
            (
                json!({"version": "v0.9", "deleteSurface": {"surfaceId": "s"}, "extra": 1}),
                "one payload key",
            ),
            (
                json!({"version": "1.0", "callRendererFunction": {"functionCallId": "c"}}),
                "unknown variant `callRendererFunction`",
            ),
        ] {
            let error = serde_json::from_value::<AgentMessage>(wire.clone()).unwrap_err();
            assert!(error.to_string().contains(reason), "{wire}: {error}");
        }
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
    fn upsert_creates_missing_intermediates_and_stores_null() {
        let mut model = json!({});
        apply_data_model_update(&mut model, "/user/name", &json!("Ada")).unwrap();
        assert_eq!(model, json!({"user": {"name": "Ada"}}));

        apply_data_model_update(&mut model, "/user/name", &Value::Null).unwrap();
        assert_eq!(model, json!({"user": {"name":null}}));

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

        let err = apply_data_model_update(&mut model, "/items/nope", &json!(0)).unwrap_err();
        assert!(matches!(err, crate::Error::Pointer { .. }));
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
