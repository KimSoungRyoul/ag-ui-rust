//! Reads the frozen AG-UI 1.0 JSON Schema as the normative event surface, and
//! every other `$defs` shape beside it.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::baseline;
use super::shapes::{Member, Shape, Union};
use super::upstream::{Field, Upstream, UpstreamEvent};

/// `$id` of the frozen AG-UI 1.0 schema.
pub const ID_1_0: &str = "https://ag-ui.com/spec/1.0/schema.json";
/// `$id` of the unreleased draft that follows it. Same layout, so the same
/// reader serves both.
pub const ID_DRAFT: &str = "https://ag-ui.com/spec/draft/schema.json";

/// Definitions that make up the event union itself. The event comparison
/// owns them; every other `$defs` entry is a shape.
const EVENT_MACHINERY: &[&str] = &["Event", "EventType", "BaseEvent"];

pub fn extract(text: &str) -> Result<Upstream, String> {
    extract_as(text, ID_1_0)
}

/// Reads a schema that must declare `id`.
pub fn extract_as(text: &str, id: &str) -> Result<Upstream, String> {
    let document: Value = serde_json::from_str(text)
        .map_err(|error| format!("protocol schema is invalid JSON: {error}"))?;
    if document.get("$id").and_then(Value::as_str) != Some(id) {
        return Err(format!("expected the schema $id {id}"));
    }
    if document.get("$ref").and_then(Value::as_str) != Some("#/$defs/Event") {
        return Err("the schema root must reference #/$defs/Event".to_owned());
    }
    let defs = document
        .get("$defs")
        .and_then(Value::as_object)
        .ok_or("protocol schema has no $defs object")?;
    let event_types = defs
        .get("EventType")
        .and_then(|v| v.get("enum"))
        .and_then(Value::as_array)
        .ok_or("protocol schema has no EventType enum")?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "EventType enum contains a non-string member".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let base_fields = fields(defs.get("BaseEvent").ok_or("missing BaseEvent")?, defs, &[])?
        .into_iter()
        .filter(|field| field.name != "type")
        .collect::<Vec<_>>();
    let inherited = base_fields
        .iter()
        .map(|field| field.name.as_str())
        .chain(std::iter::once("type"))
        .collect::<Vec<_>>();
    let union = defs
        .get("Event")
        .and_then(|value| value.get("oneOf"))
        .and_then(Value::as_array)
        .ok_or("protocol schema has no Event.oneOf")?;
    let mut events = Vec::with_capacity(union.len());
    let mut seen = BTreeSet::new();
    for reference in union {
        let name = reference
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|reference| reference.strip_prefix("#/$defs/"))
            .ok_or("Event.oneOf contains an unsupported reference")?;
        let shape = defs
            .get(name)
            .ok_or_else(|| format!("Event.oneOf points at missing {name}"))?;
        let event_type = shape
            .pointer("/properties/type/const")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{name} has no type.const"))?;
        if !event_types.iter().any(|known| known == event_type) {
            return Err(format!(
                "{name} declares {event_type}, absent from EventType"
            ));
        }
        if !seen.insert(event_type.to_owned()) {
            return Err(format!("Event.oneOf declares {event_type} more than once"));
        }
        let event_fields = fields(shape, defs, &inherited)?;
        events.push(UpstreamEvent {
            event_type: event_type.to_owned(),
            schema: Some(name.to_owned()),
            fields: event_fields,
            unparsed: None,
        });
    }
    let expected = event_types.iter().cloned().collect::<BTreeSet<_>>();
    if expected.len() != event_types.len() {
        return Err("EventType enum contains duplicate values".to_owned());
    }
    if seen != expected {
        return Err(format!(
            "Event.oneOf does not match EventType: missing {:?}",
            expected.difference(&seen).collect::<Vec<_>>()
        ));
    }
    let mut root = document.clone();
    let root_object = root
        .as_object_mut()
        .expect("a schema with $defs must be an object");
    root_object.remove("$defs");
    // The identity is checked above, so leaving it out of the fingerprint
    // loses nothing — and lets the draft's root compare equal to 1.0's when
    // only its name differs.
    root_object.remove("$id");
    let mut machinery = EVENT_MACHINERY
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    machinery.extend(events.iter().filter_map(|event| event.schema.clone()));
    let shapes = defs
        .iter()
        .filter(|(name, _)| !machinery.contains(*name))
        .map(|(name, definition)| Ok((name.clone(), shape(definition, defs)?)))
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    Ok(Upstream {
        event_types,
        base_fields,
        events,
        root_signature: signature(&root),
        schema_signatures: defs
            .iter()
            .map(|(name, shape)| (name.clone(), signature(shape)))
            .collect(),
        shapes,
        notes: Vec::new(),
    })
}

/// One non-event definition, read as far as a Rust type can be compared
/// with it: its own kind, its properties, its union members, its values.
fn shape(definition: &Value, defs: &serde_json::Map<String, Value>) -> Result<Shape, String> {
    let has_fields = definition.get("properties").is_some() || definition.get("allOf").is_some();
    Ok(Shape {
        kind: wire_kind(definition, defs),
        fields: if has_fields {
            fields(definition, defs, &[])?
                .into_iter()
                .map(|field| baseline::Field {
                    name: field.name,
                    required: field.required,
                    kind: Some(field.kind),
                })
                .collect()
        } else {
            Vec::new()
        },
        union: union(definition, defs),
        values: definition
            .get("enum")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// A `oneOf` whose branches all name definitions, with the property that
/// tells them apart: the one every member pins to a string constant.
fn union(definition: &Value, defs: &serde_json::Map<String, Value>) -> Option<Union> {
    let names = definition
        .get("oneOf")?
        .as_array()?
        .iter()
        .map(|branch| {
            branch
                .get("$ref")?
                .as_str()?
                .strip_prefix("#/$defs/")
                .map(str::to_owned)
        })
        .collect::<Option<Vec<_>>>()?;
    let first = defs.get(names.first()?)?;
    let candidates = first
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| properties.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let discriminator = candidates
        .into_iter()
        .find(|property| {
            names.iter().all(|name| {
                defs.get(name)
                    .and_then(|member| own_const(member, property, defs))
                    .is_some()
            })
        })
        .unwrap_or_default();
    Some(Union {
        members: names
            .iter()
            .map(|name| Member {
                schema: name.clone(),
                tag: defs
                    .get(name)
                    .and_then(|member| own_const(member, &discriminator, defs))
                    .unwrap_or_default(),
            })
            .collect(),
        discriminator,
    })
}

/// The string constant a definition pins `property` to, looking through
/// `allOf` only when the definition does not say so itself.
fn own_const(
    definition: &Value,
    property: &str,
    defs: &serde_json::Map<String, Value>,
) -> Option<String> {
    if let Some(constant) = definition
        .pointer(&format!("/properties/{property}/const"))
        .and_then(Value::as_str)
    {
        return Some(constant.to_owned());
    }
    definition
        .get("allOf")?
        .as_array()?
        .iter()
        .find_map(|part| {
            let part = match part.get("$ref").and_then(Value::as_str) {
                Some(reference) => defs.get(reference.strip_prefix("#/$defs/")?)?,
                None => part,
            };
            own_const(part, property, defs)
        })
}

/// FNV-1a over a canonical JSON representation. This is a drift fingerprint,
/// not a security hash: changed field types and union variants must demand
/// review even when their names and requiredness did not change.
fn signature(shape: &Value) -> String {
    let mut shape = shape.clone();
    remove_annotations(&mut shape);
    shape.sort_all_objects();
    let canonical = serde_json::to_string(&shape).expect("a schema is JSON");
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in canonical.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{hash:016x}")
}

/// Only schema nodes carry annotations. Keys in a property map and values
/// inside literals such as `const` or `enum` are part of the wire contract.
fn remove_annotations(schema: &mut Value) {
    let Some(fields) = schema.as_object_mut() else {
        return;
    };
    for annotation in ["description", "$anchor", "title", "examples"] {
        fields.remove(annotation);
    }
    for (key, value) in fields {
        match key.as_str() {
            "properties" | "patternProperties" | "$defs" | "definitions" | "dependentSchemas" => {
                if let Some(schemas) = value.as_object_mut() {
                    schemas.values_mut().for_each(remove_annotations);
                }
            }
            "allOf" | "anyOf" | "oneOf" | "prefixItems" => {
                if let Some(schemas) = value.as_array_mut() {
                    schemas.iter_mut().for_each(remove_annotations);
                }
            }
            "items"
            | "additionalProperties"
            | "unevaluatedProperties"
            | "unevaluatedItems"
            | "contains"
            | "propertyNames"
            | "not"
            | "if"
            | "then"
            | "else"
            | "contentSchema" => remove_annotations(value),
            _ => {}
        }
    }
}

fn fields(
    shape: &Value,
    defs: &serde_json::Map<String, Value>,
    inherited: &[&str],
) -> Result<Vec<Field>, String> {
    let mut fields = BTreeMap::new();
    let mut required = Vec::new();
    collect(shape, defs, &mut fields, &mut required)?;
    Ok(fields
        .into_iter()
        .filter(|(name, _)| !inherited.contains(&name.as_str()))
        .map(|(name, shape)| Field {
            required: required.contains(&name),
            name,
            kind: wire_kind(&shape, defs),
        })
        .collect())
}

fn wire_kind(shape: &Value, defs: &serde_json::Map<String, Value>) -> String {
    if let Some(reference) = shape.get("$ref").and_then(Value::as_str) {
        if let Some(name) = reference.strip_prefix("#/$defs/") {
            if let Some(definition) = defs.get(name) {
                return wire_kind(definition, defs);
            }
        }
        return format!("unsupported-ref:{reference}");
    }
    if let Some(ty) = shape.get("type").and_then(Value::as_str) {
        // A count with `minimum: 0` is a different contract from a signed
        // integer: a Rust `i64` there accepts what the schema rejects.
        let unsigned = shape
            .get("minimum")
            .and_then(Value::as_f64)
            .is_some_and(|minimum| minimum >= 0.0);
        if ty == "integer" && unsigned {
            return "integer>=0".to_owned();
        }
        return ty.to_owned();
    }
    if let Some(constant) = shape.get("const") {
        return match constant {
            Value::String(_) => "string",
            Value::Bool(_) => "boolean",
            Value::Number(number) if number.is_i64() || number.is_u64() => "integer",
            Value::Number(_) => "number",
            Value::Null => "null",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        }
        .to_owned();
    }
    if let Some(branches) = shape
        .get("oneOf")
        .or_else(|| shape.get("anyOf"))
        .and_then(Value::as_array)
    {
        let mut kinds = branches
            .iter()
            .map(|branch| wire_kind(branch, defs))
            .collect::<Vec<_>>();
        kinds.sort();
        kinds.dedup();
        return kinds.join("|");
    }
    if shape.get("properties").is_some() {
        return "object".to_owned();
    }
    "any".to_owned()
}

fn collect(
    shape: &Value,
    defs: &serde_json::Map<String, Value>,
    fields: &mut BTreeMap<String, Value>,
    required: &mut Vec<String>,
) -> Result<(), String> {
    if let Some(reference) = shape.get("$ref").and_then(Value::as_str) {
        let name = reference
            .strip_prefix("#/$defs/")
            .ok_or_else(|| format!("unsupported schema reference {reference}"))?;
        let definition = defs
            .get(name)
            .ok_or_else(|| format!("missing schema definition {name}"))?;
        return collect(definition, defs, fields, required);
    }
    if let Some(properties) = shape.get("properties").and_then(Value::as_object) {
        for (name, property) in properties {
            fields.insert(name.clone(), property.clone());
        }
    }
    if let Some(names) = shape.get("required").and_then(Value::as_array) {
        for name in names {
            let name = name
                .as_str()
                .ok_or("required list contains a non-string field")?;
            if !required.iter().any(|value| value == name) {
                required.push(name.to_owned());
            }
        }
    }
    if let Some(parts) = shape.get("allOf").and_then(Value::as_array) {
        for part in parts {
            collect(part, defs, fields, required)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_schema_exposes_version_and_inherited_fields() {
        let schema = include_str!("../../../crates/ag-ui/src/protocol/schema-1.0.json");
        let surface = extract(schema).unwrap();
        assert_eq!(surface.event_types.len(), 31);
        let started = surface
            .events
            .iter()
            .find(|event| event.event_type == "RUN_STARTED")
            .unwrap();
        assert!(
            started
                .fields
                .iter()
                .any(|field| field.name == "protocolVersion" && !field.required)
        );
        assert!(!started.fields.iter().any(|field| field.name == "timestamp"));
    }

    #[test]
    fn a_property_named_description_is_part_of_the_wire_contract() {
        let original =
            serde_json::json!({"properties":{"description":{"type":"string","description":"old"}}});
        let mut changed = original.clone();
        changed["properties"]["description"]["description"] = Value::from("new annotation");
        assert_eq!(signature(&original), signature(&changed));
        changed["properties"]["description"]["type"] = Value::from("integer");
        assert_ne!(signature(&original), signature(&changed));
    }

    #[test]
    fn signature_detects_a_known_field_type_change() {
        let schema = include_str!("../../../crates/ag-ui/src/protocol/schema-1.0.json");
        let original = extract(schema).unwrap();
        let mut changed: Value = serde_json::from_str(schema).unwrap();
        changed["$defs"]["TextMessageContentEvent"]["properties"]["delta"]["type"] =
            Value::String("integer".into());
        let changed = extract(&changed.to_string()).unwrap();
        assert_ne!(
            original.schema_signatures["TextMessageContentEvent"],
            changed.schema_signatures["TextMessageContentEvent"]
        );
    }

    #[test]
    fn duplicated_event_union_member_cannot_hide_a_missing_type() {
        let schema = include_str!("../../../crates/ag-ui/src/protocol/schema-1.0.json");
        let mut changed: Value = serde_json::from_str(schema).unwrap();
        let repeated = changed["$defs"]["Event"]["oneOf"][0].clone();
        changed["$defs"]["Event"]["oneOf"][30] = repeated;
        let error = extract(&changed.to_string()).unwrap_err();
        assert!(error.contains("more than once"), "{error}");
    }

    #[test]
    fn a_changed_schema_root_cannot_pass_as_event_conformance() {
        let schema = include_str!("../../../crates/ag-ui/src/protocol/schema-1.0.json");
        let mut changed: Value = serde_json::from_str(schema).unwrap();
        changed["$ref"] = Value::String("#/$defs/RunAgentInput".into());
        let error = extract(&changed.to_string()).unwrap_err();
        assert!(error.contains("schema root"), "{error}");
    }

    #[test]
    fn every_non_event_definition_is_read_as_a_shape() {
        let schema = include_str!("../../../crates/ag-ui/src/protocol/schema-1.0.json");
        let surface = extract(schema).unwrap();
        // The event union owns its own definitions.
        for owned in ["Event", "EventType", "BaseEvent", "RunStartedEvent"] {
            assert!(!surface.shapes.contains_key(owned), "{owned}");
        }
        let multi_agent = &surface.shapes["MultiAgentCapabilities"];
        assert_eq!(multi_agent.kind, "object");
        let subagents = multi_agent
            .fields
            .iter()
            .find(|field| field.name == "subagents")
            .unwrap();
        assert!(!subagents.required);
        assert_eq!(subagents.kind.as_deref(), Some("array"));

        // Fields arrive through `allOf`, mixins included.
        let developer = &surface.shapes["DeveloperMessage"];
        for name in ["id", "role", "content", "name", "metadata", "subagentRunId"] {
            assert!(
                developer.fields.iter().any(|field| field.name == name),
                "{name}"
            );
        }

        let message = surface.shapes["Message"].union.as_ref().unwrap();
        assert_eq!(message.discriminator, "role");
        assert_eq!(message.members[0].schema, "DeveloperMessage");
        assert_eq!(message.members[0].tag, "developer");
        let patch = surface.shapes["JsonPatchOperation"].union.as_ref().unwrap();
        assert_eq!(patch.discriminator, "op");

        assert_eq!(surface.shapes["Role"].values.len(), 7);
        let execution = &surface.shapes["ExecutionCapabilities"];
        assert!(
            execution
                .fields
                .iter()
                .any(|field| field.name == "maxIterations"
                    && field.kind.as_deref() == Some("integer>=0"))
        );
    }

    #[test]
    fn the_draft_is_read_under_its_own_identity_only() {
        let schema = include_str!("../../../crates/ag-ui/src/protocol/schema-1.0.json");
        let mut draft: Value = serde_json::from_str(schema).unwrap();
        draft["$id"] = Value::String(ID_DRAFT.into());
        let draft = draft.to_string();
        assert!(extract(&draft).unwrap_err().contains(ID_1_0));
        let read = extract_as(&draft, ID_DRAFT).unwrap();
        // Nothing but the name differs, so nothing else may.
        let one = extract(schema).unwrap();
        assert_eq!(read.root_signature, one.root_signature);
        assert_eq!(read.schema_signatures, one.schema_signatures);
    }

    #[test]
    fn root_signature_detects_adjacent_constraints() {
        let schema = include_str!("../../../crates/ag-ui/src/protocol/schema-1.0.json");
        let original = extract(schema).unwrap();
        let mut changed: Value = serde_json::from_str(schema).unwrap();
        changed["allOf"] = serde_json::json!([{
            "properties": {"type": {"const": "RUN_STARTED"}}
        }]);
        let changed = extract(&changed.to_string()).unwrap();
        assert_ne!(original.root_signature, changed.root_signature);
        assert_eq!(original.schema_signatures, changed.schema_signatures);
    }
}
