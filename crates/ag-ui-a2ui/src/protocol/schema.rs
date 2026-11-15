//! The three JSON Schema documents that describe a surface.
//!
//! A2UI v0.9 splits its schema into three files that travel together:
//!
//! - **server-to-client** — the message envelopes;
//! - **common types** — `ComponentId`, `ChildList`, the `Dynamic*` bindings;
//! - **catalog** — the components and functions themselves.
//!
//! [`SchemaBundle`] holds all three, because a validator compiles all three.
//!
//! # Pruning
//!
//! The v0.9 design puts the schema into the model's context. That context is
//! finite, and an agent usually wants the model to use ten components rather
//! than the catalog's eighty. The surgery that says so is here, and it is the
//! agent layer that offers it: an allowlist on a registered catalog, and the
//! operations a format describes. Keeping some components or messages also
//! drops every type that nothing kept still refers to, transitively, so a
//! shared type survives exactly as long as something still points at it.

use std::collections::BTreeSet;

#[cfg(feature = "agent")]
use serde_json::Map;
use serde_json::Value;

use super::version::A2uiVersion;
use crate::error::{Error, Result};

/// Property names whose value is prose a model writes, in the basic catalog.
///
/// The set to hand
#[cfg_attr(
    feature = "agent",
    doc = "[`DirectJson::with_progressive_keys`](crate::agent::DirectJson::with_progressive_keys),"
)]
#[cfg_attr(
    not(feature = "agent"),
    doc = "`DirectJson::with_progressive_keys` (feature `agent`),"
)]
/// so that such a string shows while it is still being written. Everything
/// absent from this list is structural or atomic: ending `"id"` or `"path"`
/// mid-token would fabricate an identifier or a data binding that the model
/// never wrote, so those wait for more input instead.
pub const DEFAULT_CUTTABLE_KEYS: [&str; 7] = [
    "literalString",
    "valueString",
    "label",
    "hint",
    "caption",
    "altText",
    "text",
];

/// Markers around the schema block in a generated prompt.
///
/// Fixed strings rather than prose: the reference toolkits emit exactly these,
/// and evaluation harnesses key on them to find the block.
pub const SCHEMA_BLOCK_START: &str = "---BEGIN A2UI JSON SCHEMA---";
/// Closing marker of the prompt's schema block.
pub const SCHEMA_BLOCK_END: &str = "---END A2UI JSON SCHEMA---";

/// The three schema documents describing one surface contract.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SchemaBundle {
    /// The server-to-client envelope schema.
    pub s2c: Value,
    /// Shared type definitions the other two reference.
    pub common_types: Value,
    /// The component and function catalog.
    pub catalog: Value,
}

impl SchemaBundle {
    /// The unmodified official v0.9.1 schemas (which accept both wire versions
    /// of the v0.9 family): [`SchemaBundle::for_version`] with
    /// [`A2uiVersion::V0_9_1`]. No network or filesystem access is performed.
    pub fn basic() -> Result<Self> {
        Self::for_version(A2uiVersion::V0_9_1)
    }

    /// The unmodified official schemas of `version`, with the basic catalog
    /// published for it. No network or filesystem access is performed.
    ///
    /// The three sets are three registries, and a message is held to the one
    /// of its own version:
    ///
    /// - v0.9: the envelope accepts `"version": "v0.9"` alone, and the basic
    ///   catalog has `List.listStyle`.
    /// - v0.9.1: the envelope accepts `v0.9` and `v0.9.1`, and the basic
    ///   catalog has no `listStyle`.
    /// - v1.0: `agent_to_renderer.json`, whose `createSurface` may carry
    ///   components and data, with the v1 basic catalog.
    ///
    /// ```
    /// use ag_ui_a2ui::A2uiVersion;
    /// use ag_ui_a2ui::schema::SchemaBundle;
    ///
    /// let list_style = |version| -> ag_ui_a2ui::Result<bool> {
    ///     let catalog = SchemaBundle::for_version(version)?.catalog;
    ///     Ok(serde_json::to_string(&catalog["components"]["List"])?.contains("listStyle"))
    /// };
    /// assert!(list_style(A2uiVersion::V0_9)?);
    /// assert!(!list_style(A2uiVersion::V0_9_1)?);
    ///
    /// let v1_0 = SchemaBundle::for_version(A2uiVersion::V1_0)?;
    /// assert_eq!(v1_0.s2c["$id"], "https://a2ui.org/specification/v1_0/agent_to_renderer.json");
    /// assert_eq!(v1_0.catalog_id(), Some(A2uiVersion::V1_0.basic_catalog_id()));
    /// # Ok::<(), ag_ui_a2ui::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error only if a document this crate ships does not parse.
    pub fn for_version(version: A2uiVersion) -> Result<Self> {
        let [s2c, common_types, catalog] = Self::texts(version);
        Ok(Self {
            s2c: serde_json::from_str(s2c)?,
            common_types: serde_json::from_str(common_types)?,
            catalog: serde_json::from_str(catalog)?,
        })
    }

    /// The documents of [`SchemaBundle::for_version`] as their text: the
    /// envelope, the common types, the basic catalog. A prompt writes them in
    /// the order their authors did, which a parsed value does not keep.
    pub(crate) fn texts(version: A2uiVersion) -> [&'static str; 3] {
        match version {
            A2uiVersion::V0_9 => [
                include_str!("../../schemas/v0_9/server_to_client.json"),
                include_str!("../../schemas/v0_9/common_types.json"),
                include_str!("../../schemas/v0_9/catalog.json"),
            ],
            A2uiVersion::V0_9_1 => [
                include_str!("../../schemas/v0_9_1/server_to_client.json"),
                include_str!("../../schemas/v0_9_1/common_types.json"),
                include_str!("../../schemas/v0_9_1/catalog.json"),
            ],
            A2uiVersion::V1_0 => [
                include_str!("../../schemas/v1_0/agent_to_renderer.json"),
                include_str!("../../schemas/v1_0/common_types.json"),
                include_str!("../../schemas/v1_0/catalog.json"),
            ],
        }
    }

    /// Builds a complete v0.9-family bundle from an inline capabilities catalog.
    /// The selected document retains its ID, components, function array, and
    /// theme. Local definitions supply the envelope's component/function unions;
    /// external references must still be registered explicitly by the caller.
    pub fn from_inline_catalog(catalog: Value) -> Result<Self> {
        let mut bundle = Self::basic()?;
        let mut catalog = catalog
            .as_object()
            .cloned()
            .ok_or_else(|| Error::catalog("inline catalog must be an object"))?;
        if catalog.get("catalogId").and_then(Value::as_str).is_none() {
            return Err(Error::catalog("inline catalogId is required"));
        }
        let components = catalog
            .get("components")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let component_union:Vec<_>=components.keys().map(|name|serde_json::json!({"allOf":[
            {"$ref":"https://a2ui.org/specification/v0_9/common_types.json#/$defs/ComponentCommon"},
            {"$ref":format!("#/components/{}",name.replace('~',"~0").replace('/',"~1"))},
            {"properties":{"component":{"const":name}},"required":["component"]}
        ]})).collect();
        let mut function_union = Vec::new();
        let mut seen = BTreeSet::new();
        if let Some(functions) = catalog.get("functions") {
            let functions = functions
                .as_array()
                .ok_or_else(|| Error::catalog("inline functions must be an array"))?;
            for function in functions {
                let name = function
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::catalog("inline function needs a name"))?;
                if !seen.insert(name) {
                    return Err(Error::catalog("duplicate inline function"));
                }
                let parameters = function
                    .get("parameters")
                    .filter(|v| v.is_object())
                    .ok_or_else(|| {
                        Error::catalog("inline function parameters must be a schema object")
                    })?;
                let result = function
                    .get("returnType")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::catalog("inline function returnType is required"))?;
                function_union.push(serde_json::json!({"type":"object","properties":{"call":{"const":name},"args":parameters,"returnType":{"const":result}},"required":["call","args"],"additionalProperties":false}));
            }
        }
        let component_schema = if component_union.is_empty() {
            Value::Bool(false)
        } else {
            serde_json::json!({"oneOf":component_union})
        };
        let function_schema = if function_union.is_empty() {
            Value::Bool(false)
        } else {
            serde_json::json!({"oneOf":function_union})
        };
        let theme = catalog
            .get("theme")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        catalog.insert(
            "$schema".into(),
            Value::String("https://json-schema.org/draft/2020-12/schema".into()),
        );
        catalog.insert(
            "$id".into(),
            Value::String("https://a2ui.invalid/local/inline-catalog.json".into()),
        );
        catalog.insert("$defs".into(),serde_json::json!({"anyComponent":component_schema,"anyFunction":function_schema,"theme":{"type":"object","properties":theme,"additionalProperties":false}}));
        bundle.catalog = Value::Object(catalog);
        Ok(bundle)
    }

    /// The `catalogId` this bundle's catalog declares.
    pub fn catalog_id(&self) -> Option<&str> {
        self.catalog.get("catalogId").and_then(Value::as_str)
    }
}

/// Keeps only the named components, and the `anyComponent` branches that reach
/// them.
#[cfg(feature = "agent")]
pub(crate) fn prune_components(catalog: &mut Value, allowed: &[String]) {
    let allowed: BTreeSet<&str> = allowed.iter().map(String::as_str).collect();
    let Some(object) = catalog.as_object_mut() else {
        return;
    };
    if let Some(Value::Object(components)) = object.get_mut("components") {
        components.retain(|name, _| allowed.contains(name.as_str()));
    }
    // The catalog's discriminated union must lose the same branches, or it
    // still admits components the document no longer defines.
    if let Some(Value::Object(defs)) = object.get_mut("$defs") {
        if let Some(Value::Object(any_component)) = defs.get_mut("anyComponent") {
            for key in ["oneOf", "anyOf"] {
                if let Some(Value::Array(branches)) = any_component.get_mut(key) {
                    branches.retain(|branch| match branch.get("$ref").and_then(Value::as_str) {
                        Some(reference) => reference
                            .rsplit('/')
                            .next()
                            .is_some_and(|name| allowed.contains(name)),
                        None => true,
                    });
                }
            }
        }
    }
}

/// Keeps only the named functions, and the `anyFunction` branches that reach
/// them.
///
/// A catalog document lists its functions as a map keyed by name; the inline
/// catalogs of a renderer's capabilities list them as an array of objects that
/// each carry a `name`. Both are read.
#[cfg(feature = "agent")]
pub(crate) fn prune_functions(catalog: &mut Value, allowed: &[String]) {
    let allowed: BTreeSet<&str> = allowed.iter().map(String::as_str).collect();
    let Some(object) = catalog.as_object_mut() else {
        return;
    };
    match object.get_mut("functions") {
        Some(Value::Object(functions)) => {
            functions.retain(|name, _| allowed.contains(name.as_str()));
        }
        Some(Value::Array(functions)) => functions.retain(|function| {
            function
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| allowed.contains(name))
        }),
        _ => {}
    }
    if let Some(Value::Object(defs)) = object.get_mut("$defs") {
        if let Some(Value::Object(any_function)) = defs.get_mut("anyFunction") {
            for key in ["oneOf", "anyOf"] {
                if let Some(Value::Array(branches)) = any_function.get_mut(key) {
                    branches.retain(|branch| match branch.get("$ref").and_then(Value::as_str) {
                        Some(reference) => reference
                            .rsplit('/')
                            .next()
                            .is_some_and(|name| allowed.contains(name)),
                        None => true,
                    });
                }
            }
        }
    }
}

/// Keeps only the named messages, then drops definitions nothing reaches.
///
/// Handles both envelope shapes: a `oneOf` of `$ref`s, and a flat `properties`
/// map keyed by message name.
#[cfg(feature = "agent")]
pub(crate) fn prune_messages(s2c: &mut Value, allowed: &[String]) {
    let allowed: BTreeSet<&str> = allowed.iter().map(String::as_str).collect();
    let Some(object) = s2c.as_object_mut() else {
        return;
    };

    let mut pruned_union = false;
    for key in ["oneOf", "anyOf"] {
        if let Some(Value::Array(branches)) = object.get_mut(key) {
            branches.retain(|branch| match branch.get("$ref").and_then(Value::as_str) {
                Some(reference) => reference
                    .rsplit('/')
                    .next()
                    .is_some_and(|name| allowed.contains(name)),
                None => true,
            });
            pruned_union = true;
        }
    }
    if let Some(Value::Object(properties)) = object.get_mut("properties") {
        if !pruned_union {
            properties.retain(|name, _| allowed.contains(name.as_str()));
        }
    }

    if pruned_union {
        let roots: Vec<Value> = object
            .iter()
            .filter(|(key, _)| *key != "$defs")
            .map(|(_, value)| value.clone())
            .collect();
        let roots: Vec<&Value> = roots.iter().collect();
        retain_reachable_defs(object, &roots, "#/$defs/");
    }
}

/// Keeps only the common types still referenced by the given documents.
#[cfg(feature = "agent")]
pub(crate) fn prune_common_types(common_types: &mut Value, referenced_by: &[&Value]) {
    let Some(object) = common_types.as_object_mut() else {
        return;
    };
    if object.get("$defs").and_then(Value::as_object).is_none() {
        return;
    }
    retain_reachable_defs(object, referenced_by, "common_types.json#/$defs/");
}

/// Retains `$defs` entries reachable from `roots`, following refs transitively.
///
/// `external_prefix` is how another document names these definitions; the
/// document's own `#/$defs/` form is always followed as well.
#[cfg(feature = "agent")]
fn retain_reachable_defs(object: &mut Map<String, Value>, roots: &[&Value], external_prefix: &str) {
    let Some(defs) = object.get("$defs").and_then(Value::as_object).cloned() else {
        return;
    };

    let mut keep: BTreeSet<String> = BTreeSet::new();
    let mut queue: Vec<Value> = roots.iter().map(|root| (*root).clone()).collect();

    while let Some(value) = queue.pop() {
        for reference in collect_refs(&value) {
            let name = reference
                .strip_prefix(external_prefix)
                .or_else(|| reference.strip_prefix("#/$defs/"))
                .map(str::to_string);
            let Some(name) = name else { continue };
            if !defs.contains_key(&name) || !keep.insert(name.clone()) {
                continue;
            }
            if let Some(definition) = defs.get(&name) {
                queue.push(definition.clone());
            }
        }
    }

    if let Some(Value::Object(target)) = object.get_mut("$defs") {
        target.retain(|name, _| keep.contains(name));
    }
}

/// Every `$ref` string anywhere inside a value.
#[cfg(feature = "agent")]
fn collect_refs(value: &Value) -> Vec<String> {
    let mut out = Vec::new();
    walk_refs(value, &mut out);
    out
}

#[cfg(feature = "agent")]
fn walk_refs(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if key == "$ref" {
                    if let Some(reference) = child.as_str() {
                        out.push(reference.to_string());
                    }
                }
                walk_refs(child, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk_refs(item, out);
            }
        }
        _ => {}
    }
}

#[cfg(all(test, feature = "agent"))]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pruning_components_also_prunes_the_union() {
        let mut catalog = json!({
            "catalogId": "basic",
            "$defs": {"anyComponent": {"oneOf": [
                {"$ref": "#/components/Text"},
                {"$ref": "#/components/Button"},
                {"$ref": "#/components/Image"}
            ]}},
            "components": {"Text": {}, "Button": {}, "Image": {}}
        });
        prune_components(&mut catalog, &["Text".to_string()]);
        assert_eq!(
            catalog,
            json!({
                "catalogId": "basic",
                "$defs": {"anyComponent": {"oneOf": [{"$ref": "#/components/Text"}]}},
                "components": {"Text": {}}
            })
        );
    }

    #[test]
    fn pruning_messages_drops_unreachable_definitions() {
        let mut s2c = json!({
            "oneOf": [{"$ref": "#/$defs/MessageA"}],
            "$defs": {
                "MessageA": {"type": "object", "properties": {"shared": {"$ref": "#/$defs/Shared"}}},
                "Shared": {"type": "string"},
                "Unused": {"type": "number"}
            }
        });
        prune_messages(&mut s2c, &["MessageA".to_string()]);
        let defs = s2c["$defs"].as_object().unwrap();
        assert!(defs.contains_key("MessageA"));
        assert!(
            defs.contains_key("Shared"),
            "a referenced type must survive"
        );
        assert!(!defs.contains_key("Unused"));
    }

    #[test]
    fn a_flat_message_map_prunes_by_property_name() {
        let mut s2c = json!({
            "properties": {
                "beginRendering": {"type": "object"},
                "surfaceUpdate": {"type": "object"},
                "deleteSurface": {"type": "object"}
            },
            "required": ["surfaceId"]
        });
        prune_messages(
            &mut s2c,
            &["beginRendering".to_string(), "deleteSurface".to_string()],
        );
        assert_eq!(
            s2c,
            json!({
                "properties": {
                    "beginRendering": {"type": "object"},
                    "deleteSurface": {"type": "object"}
                },
                "required": ["surfaceId"]
            })
        );
    }

    #[test]
    fn common_types_follow_what_still_references_them() {
        let mut common_types = json!({"$defs": {
            "TypeForA": {"type": "string", "$ref": "#/$defs/SubtypeForA"},
            "TypeForB": {"type": "number"},
            "SubtypeForA": {"type": "boolean"}
        }});
        let mut catalog = json!({"catalogId": "basic", "components": {
            "CompA": {"$ref": "common_types.json#/$defs/TypeForA"},
            "CompB": {"$ref": "common_types.json#/$defs/TypeForB"}
        }});
        prune_components(&mut catalog, &["CompA".to_string()]);
        prune_common_types(&mut common_types, &[&catalog]);
        assert_eq!(
            common_types,
            json!({"$defs": {
                "TypeForA": {"type": "string", "$ref": "#/$defs/SubtypeForA"},
                "SubtypeForA": {"type": "boolean"}
            }})
        );
    }
}
