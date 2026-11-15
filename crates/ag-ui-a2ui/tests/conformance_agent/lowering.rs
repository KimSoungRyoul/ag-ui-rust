//! Every rule by which a case written against A2UI v1.0 is run by a crate
//! that speaks v0.9.
//!
//! Upstream writes its agent-tier suites once, against v1.0, and asks an SDK
//! that speaks an older version to lower what a case hands it and lift what it
//! hands back, instead of keeping a second copy of the suites
//! (`conformance_harness_implementation_guide.md`, "Patterns and pitfalls in
//! Inference harnesses"). This module is that translation, and the only place
//! it happens. `tests/conformance/README.md` lists the same rules.
//!
//! A rule here rewrites how a case is *spelled*. A case whose *meaning* v0.9
//! cannot state is not lowered at all: [`v1_only`] names it, and it is
//! skipped for #42 to pick up.
//!
//! | # | v1.0, as the suites write it | v0.9, as this crate reads it |
//! |---|---|---|
//! | 1 | `"version": "1.0"` | `"version": "v0.9"`. A case that writes `v0.9` means "a version other than the catalog's", and becomes `1.0`. |
//! | 2 | a catalog document's `"protocolVersion": "1.0"` | `"0.9"` |
//! | 3 | `common_types.json#/$defs/Child` in a catalog | `…#/$defs/ComponentId`, which v1.0's `Child` is a reference to |
//! | 4 | capabilities keyed `"1.0"` | keyed `"v0.9"`, inline catalogs lowered by rules 2 and 3 |
//! | 5 | a binding `{"@path": …}`, a call `{"@call": …}` in a component | `{"path": …}`, `{"call": …}`. A `ChildList` template (`componentId` beside `path`) keeps its keys in both versions. |
//! | 6 | `"@checks"`, `"@accessibility"` on a component | `"checks"`, `"accessibility"` |
//! | 7 | a `createSurface` that names no catalog, in a case that never names one | the first catalog of the case is named, and removed again from what comes back |
//! | 8 | a `createSurface` carrying `components` and `dataModel` (decoded messages only) | `createSurface`, then the `updateComponents` and the `updateDataModel` at `/` that fill it |
//!
//! Lifting applies rules 1, 5, 6 and 7 backwards to the messages the crate
//! emits, and drops `sendDataModel: false`, which v1.0 leaves out.

use serde_json::{Map, Value, json};

/// The version the agent-tier suites are written against.
const SUITE: &str = "1.0";
/// The version a lowered case runs under.
const LOWERED: &str = "v0.9";

/// Rule 1. Any other value is returned as it is.
pub(crate) fn swap_version(version: &str) -> &str {
    match version {
        "1.0" | "v1.0" => LOWERED,
        "v0.9" => SUITE,
        other => other,
    }
}

/// What a catalog document says about the version it is written against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Declared {
    /// v1.0, or nothing: a fixture that states no version is a v1.0 one.
    Suite,
    /// A version this crate speaks: the case runs as written.
    Spoken,
    /// v0.8.
    V0_8,
}

pub(crate) fn declared(document: &Value) -> Declared {
    match document.get("protocolVersion").and_then(Value::as_str) {
        None | Some("1.0" | "v1.0") => Declared::Suite,
        Some(version) if version.trim_start_matches('v').starts_with("0.8") => Declared::V0_8,
        Some(_) => Declared::Spoken,
    }
}

/// Rules 2 and 3, for a document that declares v1.0. A document that states
/// no version is left without one: its reader supplies it.
pub(crate) fn lower_catalog(document: &Value) -> Value {
    fn child_to_component_id(node: &mut Value) {
        match node {
            Value::Object(map) => {
                if let Some(Value::String(reference)) = map.get_mut("$ref") {
                    if let Some(prefix) = reference.strip_suffix("#/$defs/Child") {
                        *reference = format!("{prefix}#/$defs/ComponentId");
                    }
                }
                map.values_mut().for_each(child_to_component_id);
            }
            Value::Array(items) => items.iter_mut().for_each(child_to_component_id),
            _ => {}
        }
    }
    let mut lowered = document.clone();
    if declared(document) != Declared::Suite {
        return lowered;
    }
    if let Some(version) = lowered.get_mut("protocolVersion") {
        *version = json!("0.9");
    }
    child_to_component_id(&mut lowered);
    lowered
}

/// Rule 4.
pub(crate) fn lower_capabilities(capabilities: &Value) -> Value {
    let Some(entries) = capabilities.as_object() else {
        return capabilities.clone();
    };
    let mut lowered = Map::new();
    for (key, entry) in entries {
        let mut entry = entry.clone();
        if let Some(Value::Array(inline)) = entry.get_mut("inlineCatalogs") {
            for document in inline.iter_mut().filter(|document| document.is_object()) {
                *document = lower_catalog(document);
            }
        }
        lowered.insert(swap_version(key).to_string(), entry);
    }
    Value::Object(lowered)
}

/// Rules 5 and 6 on one node of a component, in either direction.
fn rename_keys(node: &Value, lower: bool) -> Value {
    const RESERVED: [(&str, &str); 2] = [("@path", "path"), ("@call", "call")];
    const MIXINS: [(&str, &str); 2] = [("@checks", "checks"), ("@accessibility", "accessibility")];
    let rename = |key: &str, pairs: &[(&str, &str)]| -> Option<String> {
        pairs.iter().find_map(|(suite, lowered)| {
            let (from, to) = if lower {
                (suite, lowered)
            } else {
                (lowered, suite)
            };
            (key == *from).then(|| (*to).to_string())
        })
    };
    match node {
        Value::Array(items) => {
            Value::Array(items.iter().map(|item| rename_keys(item, lower)).collect())
        }
        Value::Object(map) => {
            // A `ChildList` template: its `path` is unprefixed in both versions.
            let template = map.contains_key("componentId");
            let component = map.contains_key("id") && map.contains_key("component");
            Value::Object(
                map.iter()
                    .map(|(key, value)| {
                        let renamed = if template {
                            None
                        } else {
                            component
                                .then(|| rename(key, &MIXINS))
                                .flatten()
                                .or_else(|| rename(key, &RESERVED))
                        };
                        (
                            renamed.unwrap_or_else(|| key.clone()),
                            rename_keys(value, lower),
                        )
                    })
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

/// Rules 5 and 6 on the components of one message.
fn rename_in_message(message: &Value, lower: bool) -> Value {
    let Some(map) = message.as_object() else {
        return message.clone();
    };
    Value::Object(
        map.iter()
            .map(|(key, body)| {
                let body = match body {
                    Value::Object(fields) => Value::Object(
                        fields
                            .iter()
                            .map(|(field, value)| {
                                let value = if field == "components" {
                                    rename_keys(value, lower)
                                } else {
                                    value.clone()
                                };
                                (field.clone(), value)
                            })
                            .collect(),
                    ),
                    other => other.clone(),
                };
                (key.clone(), body)
            })
            .collect(),
    )
}

fn swap_message_version(message: &mut Value) {
    if let Some(Value::String(version)) = message.get_mut("version") {
        *version = swap_version(version).to_string();
    }
}

/// Rules 1, 5, 6, 7 and 8 on decoded messages.
pub(crate) fn lower_messages(messages: &[Value], inject_catalog: Option<&str>) -> Vec<Value> {
    let mut lowered = Vec::new();
    for message in messages {
        let mut message = rename_in_message(message, true);
        swap_message_version(&mut message);
        let version = message.get("version").cloned().unwrap_or(Value::Null);
        let Some(Value::Object(create)) = message.get_mut("createSurface") else {
            lowered.push(message);
            continue;
        };
        let components = create.remove("components");
        let data_model = create.remove("dataModel");
        if let Some(catalog_id) = inject_catalog {
            create
                .entry("catalogId")
                .or_insert_with(|| json!(catalog_id));
        }
        let surface_id = create.get("surfaceId").cloned().unwrap_or(Value::Null);
        lowered.push(message);
        if let Some(components) = components {
            lowered.push(json!({
                "version": version,
                "updateComponents": {"surfaceId": surface_id, "components": components},
            }));
        }
        if let Some(data_model) = data_model {
            lowered.push(json!({
                "version": version,
                "updateDataModel": {"surfaceId": surface_id, "path": "/", "value": data_model},
            }));
        }
    }
    lowered
}

/// Rules 1, 5, 6 and 7 backwards, on the messages the crate emitted.
pub(crate) fn lift_messages(messages: &[Value], injected_catalog: Option<&str>) -> Vec<Value> {
    messages
        .iter()
        .map(|message| {
            let mut message = rename_in_message(message, false);
            swap_message_version(&mut message);
            unstate_defaults(&mut message, injected_catalog);
            message
        })
        .collect()
}

/// What a typed message states that the suites leave out: `sendDataModel:
/// false`, and a catalog the harness injected (rule 7). Applied to messages
/// that were not lowered too, with no injected catalog.
pub(crate) fn unstate_defaults(message: &mut Value, injected_catalog: Option<&str>) {
    if let Some(Value::Object(create)) = message.get_mut("createSurface") {
        if create.get("sendDataModel") == Some(&Value::Bool(false)) {
            create.remove("sendDataModel");
        }
        if injected_catalog.is_some()
            && create.get("catalogId").and_then(Value::as_str) == injected_catalog
        {
            create.remove("catalogId");
        }
    }
}

/// Rules 5 and 6 backwards on a component's own properties.
pub(crate) fn lift_component(component: &Value) -> Value {
    rename_keys(component, false)
}

/// One rewrite of a model's text: the bytes `start..end` become
/// `replacement`.
struct Edit {
    start: usize,
    end: usize,
    replacement: String,
}

/// The text after `at`, if `at` starts with one of `options`.
fn after<'a>(text: &'a str, options: &[&str]) -> Option<&'a str> {
    options.iter().find_map(|option| text.strip_prefix(option))
}

/// Rules 1, 5 and 7 on text a model wrote. The quotes around a `version` may
/// be curly: a case may write them so, for the reader to repair.
fn edits(text: &str, inject_catalog: Option<&str>) -> Vec<Edit> {
    const OPEN: [&str; 2] = ["\"", "\u{201c}"];
    const CLOSE: [&str; 2] = ["\"", "\u{201d}"];
    let offset = |rest: &str| text.len() - rest.len();
    let mut found = Vec::new();
    for (at, _) in text.char_indices() {
        let here = &text[at..];
        let Some(inside) = after(here, &OPEN) else {
            continue;
        };
        // `"version": "1.0"`
        if let Some(value) = inside
            .strip_prefix("version")
            .and_then(|rest| after(rest, &CLOSE))
            .and_then(|rest| rest.trim_start().strip_prefix(':'))
            .and_then(|rest| after(rest.trim_start(), &OPEN))
        {
            for version in ["v1.0", "1.0", "v0.9"] {
                let closes = value
                    .strip_prefix(version)
                    .is_some_and(|rest| after(rest, &CLOSE).is_some());
                if closes {
                    found.push(Edit {
                        start: offset(value),
                        end: offset(value) + version.len(),
                        replacement: swap_version(version).to_string(),
                    });
                    break;
                }
            }
        }
        // `"@path":` and `"@call":`. A doubled `@` escapes a literal key and
        // does not match.
        for key in ["path", "call"] {
            let reserved = inside
                .strip_prefix('@')
                .and_then(|rest| rest.strip_prefix(key))
                .and_then(|rest| rest.strip_prefix('"'))
                .is_some_and(|rest| rest.trim_start().starts_with(':'));
            if reserved {
                found.push(Edit {
                    start: offset(inside),
                    end: offset(inside) + 1,
                    replacement: String::new(),
                });
            }
        }
        // `"createSurface": {`
        if let (Some(catalog_id), Some(body)) = (
            inject_catalog,
            inside
                .strip_prefix("createSurface\"")
                .and_then(|rest| rest.trim_start().strip_prefix(':'))
                .and_then(|rest| rest.trim_start().strip_prefix('{')),
        ) {
            found.push(Edit {
                start: offset(body),
                end: offset(body),
                replacement: format!("\"catalogId\": {}, ", json!(catalog_id)),
            });
        }
    }
    found.sort_by_key(|edit| edit.start);
    found
}

/// Rules 1, 5 and 7 on one text.
pub(crate) fn lower_text(text: &str, inject_catalog: Option<&str>) -> String {
    lower_chunks(&[text], inject_catalog).concat()
}

/// Rules 1, 5 and 7 on a text that arrives in chunks.
///
/// The rules are applied to the chunks joined, since a boundary may fall
/// inside what a rule rewrites, and the result is cut where the boundaries
/// fell: a boundary inside a rewritten span stays inside it.
pub(crate) fn lower_chunks(chunks: &[&str], inject_catalog: Option<&str>) -> Vec<String> {
    let joined = chunks.concat();
    let edits = edits(&joined, inject_catalog);
    let mut boundaries = Vec::with_capacity(chunks.len());
    let mut end = 0;
    for chunk in chunks {
        end += chunk.len();
        boundaries.push(end);
    }

    let mut lowered = String::with_capacity(joined.len());
    let mut cuts = Vec::with_capacity(chunks.len());
    let mut boundaries = boundaries.into_iter().peekable();
    let mut at = 0;
    for edit in &edits {
        // Boundaries in the text before this edit.
        while let Some(boundary) = boundaries.next_if(|boundary| *boundary <= edit.start) {
            cuts.push(lowered.len() + (boundary - at));
        }
        lowered.push_str(&joined[at..edit.start]);
        // Boundaries inside what the edit replaces.
        while let Some(boundary) = boundaries.next_if(|boundary| *boundary < edit.end) {
            let inside = (boundary - edit.start).min(edit.replacement.len());
            cuts.push(lowered.len() + inside);
        }
        lowered.push_str(&edit.replacement);
        at = edit.end;
    }
    for boundary in boundaries {
        cuts.push(lowered.len() + (boundary - at));
    }
    lowered.push_str(&joined[at..]);

    let mut pieces = Vec::with_capacity(chunks.len());
    let mut start = 0;
    for cut in cuts {
        // A cut never lands inside a character: replacements are ASCII, and
        // the boundaries outside them are the originals.
        pieces.push(lowered[start..cut].to_string());
        start = cut;
    }
    pieces
}

/// Why v0.9 cannot state what a case carries, when it cannot.
///
/// `text` is everything the case hands the crate, joined. The names are the
/// cases whose meaning is in the shape of a v1.0 message rather than in a
/// word that can be searched for.
pub(crate) fn v1_only(name: &str, text: &str) -> Option<&'static str> {
    const BY_NAME: [(&str, &str); 4] = [
        (
            "test_decompile_json_catalog_qualified_components",
            "v0.9 components cannot name a catalog of their own",
        ),
        (
            "test_compile_json_newline_in_create_surface_data_model_preserved",
            "createSurface carries its data model, written as text",
        ),
        (
            "test_stream_create_surface_inline_components_v10",
            "createSurface carries its components, written as text",
        ),
        (
            "test_stream_create_surface_inline_components_in_small_chunks_v10",
            "createSurface carries its components, written as text",
        ),
    ];
    if let Some((_, why)) = BY_NAME.iter().find(|(case, _)| *case == name) {
        return Some(why);
    }
    [
        (
            "callRendererFunction",
            "v0.9 has no callRendererFunction message",
        ),
        (
            "agentFunctionResponse",
            "v0.9 has no agentFunctionResponse message",
        ),
        (
            "@catalogId",
            "v0.9 components cannot name a catalog of their own",
        ),
    ]
    .into_iter()
    .find_map(|(marker, why)| text.contains(marker).then_some(why))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_is_swapped_between_the_suite_and_the_crate() {
        assert_eq!(swap_version("1.0"), "v0.9");
        assert_eq!(swap_version("v0.9"), "1.0");
        assert_eq!(swap_version("v0.8"), "v0.8");
    }

    #[test]
    fn a_catalog_is_lowered_only_when_it_declares_the_suite_version() {
        let suite = json!({
            "catalogId": "c",
            "protocolVersion": "1.0",
            "components": {"Card": {"properties": {
                "child": {"$ref": "common_types.json#/$defs/Child"},
                "children": {"$ref": "common_types.json#/$defs/ChildList"},
            }}},
        });
        let lowered = lower_catalog(&suite);
        assert_eq!(lowered["protocolVersion"], "0.9");
        assert_eq!(
            lowered["components"]["Card"]["properties"]["child"]["$ref"],
            "common_types.json#/$defs/ComponentId"
        );
        assert_eq!(
            lowered["components"]["Card"]["properties"]["children"]["$ref"],
            "common_types.json#/$defs/ChildList"
        );

        let spoken = json!({"catalogId": "c", "protocolVersion": "0.9"});
        assert_eq!(lower_catalog(&spoken), spoken);
        assert_eq!(declared(&json!({"protocolVersion": "0.8"})), Declared::V0_8);
        assert_eq!(declared(&json!({})), Declared::Suite);
    }

    #[test]
    fn messages_are_lowered_and_lifted_back() {
        let suite = json!({"version": "1.0", "updateComponents": {"surfaceId": "s", "components": [{
            "id": "root",
            "component": "Column",
            "children": {"componentId": "row", "path": "/rows"},
            "text": {"@path": "/title"},
            "@checks": [{"condition": {"@call": "required", "args": {"value": {"@path": "/x"}}}}],
        }]}});
        let lowered = lower_messages(std::slice::from_ref(&suite), None);
        assert_eq!(
            lowered,
            [
                json!({"version": "v0.9", "updateComponents": {"surfaceId": "s", "components": [{
                    "id": "root",
                    "component": "Column",
                    "children": {"componentId": "row", "path": "/rows"},
                    "text": {"path": "/title"},
                    "checks": [{"condition": {"call": "required", "args": {"value": {"path": "/x"}}}}],
                }]}})
            ]
        );
        assert_eq!(lift_messages(&lowered, None), [suite]);
    }

    #[test]
    fn an_inline_create_surface_is_split_and_a_catalog_injected() {
        let suite = json!({"version": "1.0", "createSurface": {
            "surfaceId": "s",
            "components": [{"id": "root", "component": "Text", "text": "Hi"}],
            "dataModel": {"title": "T"},
        }});
        let lowered = lower_messages(&[suite], Some("the/catalog"));
        assert_eq!(
            lowered,
            [
                json!({"version": "v0.9", "createSurface": {"surfaceId": "s", "catalogId": "the/catalog"}}),
                json!({"version": "v0.9", "updateComponents": {"surfaceId": "s", "components": [
                    {"id": "root", "component": "Text", "text": "Hi"},
                ]}}),
                json!({"version": "v0.9", "updateDataModel": {"surfaceId": "s", "path": "/", "value": {"title": "T"}}}),
            ]
        );
        let mut create = lowered[0].clone();
        create["createSurface"]["sendDataModel"] = json!(false);
        assert_eq!(
            lift_messages(&[create], Some("the/catalog")),
            [json!({"version": "1.0", "createSurface": {"surfaceId": "s"}})]
        );
    }

    #[test]
    fn text_is_lowered_where_the_rules_apply_and_nowhere_else() {
        assert_eq!(
            lower_text(
                r#"[{"version": "1.0", "createSurface": {"surfaceId": "s"}}, {"version":"v0.9", "x": {"@path": "/a", "@@path": 1, "path": "@path"}}]"#,
                Some("c"),
            ),
            r#"[{"version": "v0.9", "createSurface": {"catalogId": "c", "surfaceId": "s"}}, {"version":"1.0", "x": {"path": "/a", "@@path": 1, "path": "@path"}}]"#
        );
        assert_eq!(
            lower_text("{\u{201c}version\u{201d}: \u{201c}1.0\u{201d}}", None),
            "{\u{201c}version\u{201d}: \u{201c}v0.9\u{201d}}"
        );
        assert_eq!(
            lower_text("version 1.0 of the text", None),
            "version 1.0 of the text"
        );
    }

    #[test]
    fn chunk_boundaries_stay_where_they_fell() {
        let chunks = [
            "{\"version\": \"1.",
            "0\", \"createSurface\": {",
            "\"surfaceId\": \"s\"}}",
        ];
        let lowered = lower_chunks(&chunks, Some("c"));
        assert_eq!(
            lowered,
            [
                "{\"version\": \"v0",
                ".9\", \"createSurface\": {",
                "\"catalogId\": \"c\", \"surfaceId\": \"s\"}}",
            ]
        );
        assert_eq!(lowered.concat(), lower_text(&chunks.concat(), Some("c")));
        assert_eq!(lower_chunks(&["a", "", "b"], None), ["a", "", "b"]);
    }

    #[test]
    fn a_case_v0_9_cannot_state_is_named() {
        assert!(v1_only("any", r#"{"callRendererFunction": {}}"#).is_some());
        assert!(v1_only("any", r#"{"@catalogId": "c"}"#).is_some());
        assert!(v1_only("test_stream_create_surface_inline_components_v10", "").is_some());
        assert!(v1_only("any", r#"{"createSurface": {"catalogId": "c"}}"#).is_none());
    }
}
