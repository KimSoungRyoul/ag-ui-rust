//! The instructions that tell a model how to write direct JSON.
//!
//! The snippet is, in order: the rules of the format, the protocol's own
//! schemas (the message envelope, and the common types the catalogs and the
//! envelope refer to), one schema block per catalog, and the examples, each
//! written the way the model is to write.
//!
//! The schema blocks sit between fixed marker lines, which evaluation
//! harnesses key on. The protocol's schemas appear once however many catalogs
//! are active. They are the ones of the catalogs' protocol version, and each
//! document is written in the order its author wrote it.

use serde_json::Value;

use super::blocks::{CLOSE_TAG, OPEN_TAG};
use crate::agent::catalog::{CatalogDocument, check_catalogs};
use crate::agent::ordered::Ordered;
use crate::error::Result;
use crate::protocol::message::AgentMessage;
use crate::protocol::operation::Operation;
use crate::protocol::schema::{
    SCHEMA_BLOCK_END, SCHEMA_BLOCK_START, SchemaBundle, prune_common_types, prune_messages,
};
use crate::protocol::version::A2uiVersion;

/// The heading the examples sit under.
const EXAMPLES_HEADING: &str = "### Examples:";

/// How a direct JSON answer is to be written.
///
/// The text is the reference SDK's, from its first character, which is a line
/// break (`DEFAULT_WORKFLOW_RULES` in `a2ui_agent`'s `constants.py`): the
/// conformance suite compares a snippet with the reference's byte for byte.
fn rules() -> String {
    format!(
        "\nThe generated response MUST follow these rules:\n\
         - The response can contain one or more A2UI JSON blocks.\n\
         - Each A2UI JSON block MUST be wrapped in literal `{OPEN_TAG}` and `{CLOSE_TAG}` tags \
         (always include both the opening and closing tags; do NOT wrap the tags in backticks or \
         replace the opening tag with a markdown code fence such as ```json).\n\
         - Between or around these blocks, you can provide conversational text.\n\
         - The JSON part MUST be a JSON array of A2UI messages, each following the provided A2UI \
         JSON SCHEMA.\n\
         - Top-Down Component Ordering: Within the `components` list of a message:\n    \
         - The 'root' component MUST be the FIRST element.\n    \
         - Parent components MUST appear before their child components.\n    \
         This specific ordering allows the streaming parser to yield and render the UI \
         incrementally as it arrives.\n"
    )
}

fn schema_block(sections: &[String]) -> String {
    let mut lines = vec![SCHEMA_BLOCK_START.to_string()];
    lines.extend(sections.iter().cloned());
    lines.push(SCHEMA_BLOCK_END.to_string());
    lines.join("\n\n")
}

/// The name the envelope schema gives the definition of an operation's
/// message: `createSurface` is `CreateSurfaceMessage`.
fn message_definition(operation: Operation) -> String {
    let name = operation.as_str();
    let mut definition: String = name.chars().take(1).flat_map(char::to_uppercase).collect();
    definition.push_str(&name[name.chars().next().map_or(0, char::len_utf8)..]);
    definition.push_str("Message");
    definition
}

/// One example turn, written as the block a model would write.
pub(super) fn example_block(turn: &[AgentMessage]) -> Result<String> {
    Ok(format!(
        "{OPEN_TAG}\n{}\n{CLOSE_TAG}",
        serde_json::to_string_pretty(turn)?
    ))
}

/// The direct JSON prompt snippet for `catalogs`.
///
/// `allowed_messages` restricts the envelope schema to those operations, and
/// with it the common types that are shown; `None` shows every operation.
pub(super) fn prompt_snippet(
    catalogs: &[CatalogDocument],
    examples: &[Vec<AgentMessage>],
    allowed_messages: Option<&[Operation]>,
) -> Result<String> {
    let version = check_catalogs(catalogs)?;
    // The v0.9 family is shown the documents of v0.9.1, whose envelope takes
    // either of the family's discriminators. Any other version is shown its
    // own.
    let shown = if A2uiVersion::V0_9_1.accepts(version) {
        A2uiVersion::V0_9_1
    } else {
        version
    };
    let [envelope, common_types, _] = SchemaBundle::texts(shown);
    let (envelope, common_types) = (Ordered::parse(envelope)?, Ordered::parse(common_types)?);

    // What is shown is decided on values, and written in the documents' order.
    let mut s2c = envelope.to_value();
    if let Some(allowed) = allowed_messages {
        let definitions: Vec<String> = allowed.iter().copied().map(message_definition).collect();
        prune_messages(&mut s2c, &definitions);
    }
    let mut referenced_by: Vec<&Value> = catalogs.iter().map(CatalogDocument::document).collect();
    referenced_by.push(&s2c);
    let mut referenced = common_types.to_value();
    prune_common_types(&mut referenced, &referenced_by);

    let mut protocol = vec![format!(
        "### Agent to Renderer Schema:\n{}",
        envelope.keeping(&s2c).compact()
    )];
    if referenced
        .get("$defs")
        .and_then(Value::as_object)
        .is_some_and(|definitions| !definitions.is_empty())
    {
        protocol.push(format!(
            "### Common Types Schema:\n{}",
            common_types.keeping(&referenced).compact()
        ));
    }

    let mut parts = vec![format!("{}\n\n{}", rules(), schema_block(&protocol))];
    parts.push(
        catalogs
            .iter()
            .map(|catalog| schema_block(&[format!("### Catalog Schema:\n{}", catalog.written())]))
            .collect::<Vec<_>>()
            .join("\n\n"),
    );
    if !examples.is_empty() {
        let blocks = examples
            .iter()
            .map(|turn| example_block(turn))
            .collect::<Result<Vec<_>>>()?;
        parts.push(format!("{EXAMPLES_HEADING}\n\n{}", blocks.join("\n\n")));
    }
    Ok(parts.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::A2uiVersion;
    use crate::Component;
    use serde_json::json;

    fn basic() -> CatalogDocument {
        CatalogDocument::basic(A2uiVersion::V0_9_1).unwrap()
    }

    fn other() -> CatalogDocument {
        CatalogDocument::new(json!({
            "catalogId": "test/charts",
            "protocolVersion": "0.9.1",
            "components": {"Chart": {"properties": {
                "caption": {"$ref": "common_types.json#/$defs/DynamicString"},
            }}},
        }))
        .unwrap()
    }

    #[test]
    fn the_snippet_states_the_rules_the_protocol_and_each_catalog() {
        let snippet = prompt_snippet(&[basic(), other()], &[], None).unwrap();
        assert!(snippet.starts_with("\nThe generated response MUST follow these rules:\n"));
        assert!(snippet.contains("`<a2ui-json>` and `</a2ui-json>` tags"));

        // The protocol's schemas once, then one block per catalog.
        assert_eq!(
            snippet.matches("### Agent to Renderer Schema:\n{").count(),
            1
        );
        assert_eq!(snippet.matches("### Common Types Schema:\n{").count(), 1);
        assert_eq!(snippet.matches("### Catalog Schema:\n{").count(), 2);
        assert_eq!(snippet.matches(SCHEMA_BLOCK_START).count(), 3);
        assert_eq!(snippet.matches(SCHEMA_BLOCK_END).count(), 3);
        let basic_at = snippet
            .find(crate::constants::OFFICIAL_BASIC_CATALOG_ID)
            .unwrap();
        assert!(basic_at < snippet.find("test/charts").unwrap());
        assert!(!snippet.contains(EXAMPLES_HEADING));
    }

    #[test]
    fn the_same_inputs_give_the_same_snippet() {
        let catalogs = [basic(), other()];
        assert_eq!(
            prompt_snippet(&catalogs, &[], None).unwrap(),
            prompt_snippet(&catalogs, &[], None).unwrap()
        );
    }

    #[test]
    fn only_the_common_types_something_refers_to_are_shown() {
        let only_charts = prompt_snippet(&[other()], &[], None).unwrap();
        let common = only_charts
            .split("### Common Types Schema:\n")
            .nth(1)
            .and_then(|rest| rest.split("\n\n").next())
            .unwrap();
        let common: Value = serde_json::from_str(common).unwrap();
        let shown: Vec<&String> = common["$defs"].as_object().unwrap().keys().collect();
        assert!(shown.iter().any(|name| *name == "DynamicString"));
        assert!(
            !shown.iter().any(|name| *name == "Checkable"),
            "nothing in this catalog or in the envelope refers to it: {shown:?}"
        );
    }

    #[test]
    fn an_allowlist_prunes_the_envelope_schema() {
        let allowed = [Operation::CreateSurface, Operation::UpdateComponents];
        let snippet = prompt_snippet(&[basic()], &[], Some(&allowed)).unwrap();
        assert!(snippet.contains("CreateSurfaceMessage"));
        assert!(snippet.contains("UpdateComponentsMessage"));
        assert!(!snippet.contains("DeleteSurfaceMessage"));
        assert!(!snippet.contains("UpdateDataModelMessage"));

        let everything = prompt_snippet(&[basic()], &[], None).unwrap();
        assert!(everything.contains("DeleteSurfaceMessage"));
    }

    #[test]
    fn examples_are_written_as_the_blocks_a_model_is_to_write() {
        let first = vec![
            AgentMessage::create_surface("s1", "test/charts"),
            AgentMessage::update_components("s1", vec![Component::new("root", "Chart")]),
        ];
        let second = vec![AgentMessage::delete_surface("s1")];
        let snippet = prompt_snippet(&[other()], &[first, second], None).unwrap();
        let examples = snippet.split("### Examples:\n\n").nth(1).unwrap();
        assert!(examples.starts_with("<a2ui-json>\n[\n  {\n"));
        assert_eq!(examples.matches("<a2ui-json>\n").count(), 2);
        assert!(
            examples.find("createSurface").unwrap() < examples.find("deleteSurface").unwrap(),
            "examples keep the order they were given in"
        );
        assert!(examples.ends_with("\n</a2ui-json>"));
    }

    #[test]
    fn a_snippet_needs_a_catalog() {
        assert!(matches!(
            prompt_snippet(&[], &[], None),
            Err(crate::Error::Catalog(_))
        ));
    }
}
