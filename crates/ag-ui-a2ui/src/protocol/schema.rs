//! The three JSON Schema documents that describe a surface, and surgery on them.
//!
//! A2UI v0.9 splits its schema into three files that travel together:
//!
//! - **server-to-client** — the message envelopes;
//! - **common types** — `ComponentId`, `ChildList`, the `Dynamic*` bindings;
//! - **catalog** — the components and functions themselves.
//!
//! [`SchemaBundle`] holds all three, because everything you do with one you do
//! with all three: put them in a prompt, restrict them to a subset of
//! components, hand them to a validator.
//!
//! # Why prune
//!
//! The whole point of the v0.9 "prompt-first" design is that the schema goes
//! into the model's context. That context is finite, and an agent usually wants
//! the model to use ten components rather than the catalog's eighty. Pruning is
//! how you say so: [`SchemaBundle::prune`] keeps the components (or messages)
//! you allow and then walks `$ref`s to drop every type that nothing kept still
//! refers to — including transitively, so a shared type survives exactly as long
//! as something still points at it.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use crate::error::{Error, Result};

/// Keys whose string values may be closed early when a chunk cuts them.
///
/// Everything absent from this list is structural or atomic: healing `"id"` or
/// `"path"` mid-token would fabricate an identifier or a data binding that the
/// model never wrote, so those fragments wait for more input instead.
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
    /// Keys whose string values may be closed early while streaming.
    ///
    /// `None` uses [`DEFAULT_CUTTABLE_KEYS`]. Override it when a catalog puts
    /// long free text under a name the defaults do not cover.
    pub custom_cuttable_keys: Option<Vec<String>>,
}

impl SchemaBundle {
    /// The unmodified official v0.9.1 schemas (which accept both wire versions).
    /// No network or filesystem access is performed.
    pub fn basic() -> Result<Self> {
        Ok(Self {
            s2c: serde_json::from_str(include_str!("../../schemas/v0_9_1/server_to_client.json"))?,
            common_types: serde_json::from_str(include_str!(
                "../../schemas/v0_9_1/common_types.json"
            ))?,
            catalog: serde_json::from_str(include_str!("../../schemas/v0_9_1/catalog.json"))?,
            custom_cuttable_keys: None,
        })
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

    /// A bundle holding only a catalog document.
    pub fn from_catalog(catalog: Value) -> Self {
        Self {
            catalog,
            ..Self::default()
        }
    }

    /// The `catalogId` this bundle's catalog declares.
    pub fn catalog_id(&self) -> Option<&str> {
        self.catalog.get("catalogId").and_then(Value::as_str)
    }

    /// The keys whose string values may be closed early while streaming.
    pub fn cuttable_keys(&self) -> Vec<String> {
        match &self.custom_cuttable_keys {
            Some(keys) => keys.clone(),
            None => DEFAULT_CUTTABLE_KEYS
                .iter()
                .map(|key| (*key).to_string())
                .collect(),
        }
    }

    /// Renders the bundle as the schema block of a prompt.
    ///
    /// The common-types section is dropped when it has nothing to say — no
    /// `$defs`, or an empty one — so the model is not handed an empty document
    /// to reason about.
    pub fn render_llm_instructions(&self) -> String {
        let mut sections = vec![SCHEMA_BLOCK_START.to_string()];
        sections.push(format!(
            "### Server To Client Schema:\n{}",
            compact(&self.s2c)
        ));
        if has_defs(&self.common_types) {
            sections.push(format!(
                "### Common Types Schema:\n{}",
                compact(&self.common_types)
            ));
        }
        sections.push(format!("### Catalog Schema:\n{}", compact(&self.catalog)));
        sections.push(SCHEMA_BLOCK_END.to_string());
        sections.join("\n\n")
    }

    /// Restricts the bundle to a subset of components and message types.
    ///
    /// Either list may be empty, meaning "keep everything of that kind". After
    /// the restriction, unreferenced type definitions are dropped from both the
    /// envelope schema and the common types.
    #[must_use]
    pub fn prune(mut self, allowed_components: &[String], allowed_messages: &[String]) -> Self {
        if !allowed_components.is_empty() {
            prune_components(&mut self.catalog, allowed_components);
        }
        if !allowed_messages.is_empty() {
            prune_messages(&mut self.s2c, allowed_messages);
        }
        // Common types are shared, so what survives depends on what the other
        // two documents still reference.
        prune_common_types(&mut self.common_types, &[&self.catalog, &self.s2c]);
        self
    }

    /// Drops `additionalProperties: false` everywhere in the catalog.
    ///
    /// Structured-output APIs reject schemas that forbid extra properties in
    /// places they need them; explicit `true` is left alone because that is a
    /// deliberate statement rather than a default.
    #[must_use]
    pub fn relaxed(mut self) -> Self {
        remove_strict_validation(&mut self.s2c);
        remove_strict_validation(&mut self.common_types);
        remove_strict_validation(&mut self.catalog);
        self
    }
}

fn compact(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string())
}

fn has_defs(schema: &Value) -> bool {
    schema
        .get("$defs")
        .and_then(Value::as_object)
        .is_some_and(|defs| !defs.is_empty())
}

/// Keeps only the named components, and the `anyComponent` branches that reach
/// them.
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

/// Keeps only the named messages, then drops definitions nothing reaches.
///
/// Handles both envelope shapes: a `oneOf` of `$ref`s, and a flat `properties`
/// map keyed by message name.
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
fn collect_refs(value: &Value) -> Vec<String> {
    let mut out = Vec::new();
    walk_refs(value, &mut out);
    out
}

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

/// Removes every `additionalProperties: false`, recursively.
pub fn remove_strict_validation(schema: &mut Value) {
    match schema {
        Value::Object(map) => {
            if map.get("additionalProperties") == Some(&Value::Bool(false)) {
                map.remove("additionalProperties");
            }
            for child in map.values_mut() {
                remove_strict_validation(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                remove_strict_validation(item);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_schema_block_carries_all_three_documents() {
        let bundle = SchemaBundle {
            s2c: json!({"s2c": "schema"}),
            common_types: json!({"$defs": {"common": "types"}}),
            catalog: json!({"catalogId": "id_basic"}),
            custom_cuttable_keys: None,
        };
        let rendered = bundle.render_llm_instructions();
        assert_eq!(
            rendered,
            "---BEGIN A2UI JSON SCHEMA---\n\n\
             ### Server To Client Schema:\n{\"s2c\":\"schema\"}\n\n\
             ### Common Types Schema:\n{\"$defs\":{\"common\":\"types\"}}\n\n\
             ### Catalog Schema:\n{\"catalogId\":\"id_basic\"}\n\n\
             ---END A2UI JSON SCHEMA---"
        );
    }

    #[test]
    fn empty_common_types_are_left_out_of_the_block() {
        for common in [
            json!({}),
            json!({"something": "else"}),
            json!({"$defs": {}}),
        ] {
            let bundle = SchemaBundle {
                s2c: json!({"s2c": "schema"}),
                common_types: common,
                catalog: json!({"catalogId": "id"}),
                custom_cuttable_keys: None,
            };
            let rendered = bundle.render_llm_instructions();
            assert!(!rendered.contains("Common Types Schema"), "{rendered}");
            assert!(rendered.contains("Server To Client Schema"));
        }
    }

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
        let bundle = SchemaBundle {
            s2c: Value::Null,
            common_types: json!({"$defs": {
                "TypeForA": {"type": "string", "$ref": "#/$defs/SubtypeForA"},
                "TypeForB": {"type": "number"},
                "SubtypeForA": {"type": "boolean"}
            }}),
            catalog: json!({"catalogId": "basic", "components": {
                "CompA": {"$ref": "common_types.json#/$defs/TypeForA"},
                "CompB": {"$ref": "common_types.json#/$defs/TypeForB"}
            }}),
            custom_cuttable_keys: None,
        };
        let pruned = bundle.prune(&["CompA".to_string()], &[]);
        assert_eq!(
            pruned.common_types,
            json!({"$defs": {
                "TypeForA": {"type": "string", "$ref": "#/$defs/SubtypeForA"},
                "SubtypeForA": {"type": "boolean"}
            }})
        );
    }

    #[test]
    fn strict_validation_is_removed_but_explicit_true_is_kept() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "a": {"type": "string", "additionalProperties": false},
                "b": {"type": "array", "items": {"type": "object", "additionalProperties": false}}
            },
            "additionalProperties": false
        });
        remove_strict_validation(&mut schema);
        assert_eq!(
            schema,
            json!({
                "type": "object",
                "properties": {
                    "a": {"type": "string"},
                    "b": {"type": "array", "items": {"type": "object"}}
                }
            })
        );

        let mut kept = json!({"type": "object", "additionalProperties": true});
        remove_strict_validation(&mut kept);
        assert_eq!(
            kept,
            json!({"type": "object", "additionalProperties": true})
        );
    }

    #[test]
    fn cuttable_keys_default_until_overridden() {
        let bundle = SchemaBundle::default();
        assert!(bundle.cuttable_keys().contains(&"text".to_string()));

        let custom = SchemaBundle {
            custom_cuttable_keys: Some(vec!["customKey1".to_string()]),
            ..SchemaBundle::default()
        };
        assert_eq!(custom.cuttable_keys(), vec!["customKey1".to_string()]);
    }
}
