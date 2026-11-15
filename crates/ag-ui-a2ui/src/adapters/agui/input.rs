//! What an AG-UI request says about A2UI: the action a user took on a
//! surface, and the catalog the frontend registered.
//!
//! Neither is an AG-UI field. Both ride where AG-UI's frontends put them: the
//! action in `forwardedProps.a2uiAction.userAction`, and the catalog in a
//! `context` entry recognised by its description.

use ag_ui::RunAgentInput;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::agent::RendererCapabilities;
use crate::protocol::version::A2uiVersion;

/// The description of the `context` entry that carries the frontend's
/// catalog. The entry is found by this text, so it is the frontends' text,
/// character for character.
pub const SCHEMA_CONTEXT_DESCRIPTION: &str = "A2UI Component Schema — available components for generating UI surfaces. Use these component names and properties when creating A2UI operations.";

/// An action a user took on a surface: a button pressed, a form sent.
///
/// Every field is optional on the wire, and a frontend sends what it has.
///
/// A later version may add a field, so one is not written out field by
/// field. [`user_action`] reads one from a request; to build one, start from
/// the default and set what there is:
///
/// ```
/// use ag_ui_a2ui::adapters::agui::UserAction;
///
/// let mut action = UserAction::default();
/// action.name = Some("checkout".into());
/// action.surface_id = Some("cart".into());
/// assert_eq!(
///     action.as_user_text(),
///     "A2UI user action:\n{\"name\":\"checkout\",\"surfaceId\":\"cart\"}"
/// );
/// ```
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct UserAction {
    /// The name the component gave its action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The surface the action happened on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_id: Option<String>,
    /// The component that raised it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_component_id: Option<String>,
    /// What the action carries, with its bindings resolved by the renderer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Map<String, Value>>,
    /// When it happened, as the frontend wrote it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

impl UserAction {
    /// The action as the text of a user message, the way AG-UI's middleware
    /// hands it to a model: `A2UI user action:`, then the action as JSON.
    pub fn as_user_text(&self) -> String {
        let action = serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string());
        format!("A2UI user action:\n{action}")
    }
}

/// The action a request reports, from `forwardedProps.a2uiAction.userAction`.
///
/// `None` when the request reports none, or when what is there is not an
/// object a [`UserAction`] reads.
///
/// ```
/// use ag_ui::RunAgentInput;
/// use ag_ui_a2ui::adapters::agui::user_action;
/// use serde_json::json;
///
/// let mut input = RunAgentInput::new("thread", "run");
/// assert!(user_action(&input).is_none());
///
/// input.forwarded_props = json!({"a2uiAction": {"userAction": {
///     "name": "checkout", "surfaceId": "cart", "sourceComponentId": "pay",
///     "context": {"total": "12.00"}
/// }}});
/// let action = user_action(&input).unwrap();
/// assert_eq!(action.name.as_deref(), Some("checkout"));
/// assert_eq!(action.surface_id.as_deref(), Some("cart"));
/// assert!(action.as_user_text().starts_with("A2UI user action:\n{"));
/// ```
pub fn user_action(input: &RunAgentInput) -> Option<UserAction> {
    let action = input.forwarded_props.get("a2uiAction")?.get("userAction")?;
    if !action.is_object() {
        return None;
    }
    serde_json::from_value(action.clone()).ok()
}

/// The catalog a frontend registered, as it sends it on every run.
///
/// A later version may add a field, so one is built with
/// [`CatalogContext::new`]. [`catalog_context`] reads one from a request.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct CatalogContext {
    /// The id of the catalog the renderer registered: a `createSurface` that
    /// names it resolves on that frontend.
    pub catalog_id: Option<String>,
    /// The entry's value: an inline catalog (`catalogId`, `components`), or
    /// whatever else the frontend sent.
    pub schema: Value,
}

impl CatalogContext {
    /// The entry whose value is `schema`. The catalog id is the `catalogId`
    /// the value states, when it states one that is not empty.
    ///
    /// ```
    /// use ag_ui_a2ui::adapters::agui::CatalogContext;
    /// use serde_json::json;
    ///
    /// let catalog = CatalogContext::new(json!({"catalogId": "example.com:cards", "components": {}}));
    /// assert_eq!(catalog.catalog_id.as_deref(), Some("example.com:cards"));
    /// assert_eq!(CatalogContext::new(json!([{"name": "Card"}])).catalog_id, None);
    /// ```
    pub fn new(schema: Value) -> Self {
        let catalog_id = schema
            .get("catalogId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_string);
        Self { catalog_id, schema }
    }

    /// The entry as the capabilities of a renderer that speaks `version`: it
    /// draws the catalog it named, and sends that catalog inline when the
    /// entry defines components.
    ///
    /// This is what
    /// [`A2uiGenerator::create_processor`](crate::agent::A2uiGenerator::create_processor)
    /// negotiates with. `None` when the entry names no catalog.
    pub fn capabilities(&self, version: A2uiVersion) -> Option<RendererCapabilities> {
        let catalog_id = self.catalog_id.as_deref()?;
        let capabilities = RendererCapabilities::supporting(version, [catalog_id]);
        let inline = self
            .schema
            .get("components")
            .and_then(Value::as_object)
            .is_some_and(|components| !components.is_empty());
        Some(if inline {
            capabilities.with_inline_catalogs(version, vec![self.schema.clone()])
        } else {
            capabilities
        })
    }
}

/// The catalog entry of a request's `context`: the one whose description is
/// [`SCHEMA_CONTEXT_DESCRIPTION`], with its value read as JSON.
///
/// `None` when there is no such entry or its value is not JSON.
///
/// ```
/// use ag_ui::{Context, RunAgentInput};
/// use ag_ui_a2ui::A2uiVersion;
/// use ag_ui_a2ui::adapters::agui::{SCHEMA_CONTEXT_DESCRIPTION, catalog_context};
///
/// let mut input = RunAgentInput::new("thread", "run");
/// input.context.push(Context::new(
///     SCHEMA_CONTEXT_DESCRIPTION,
///     r#"{"catalogId": "https://example.com/catalog.json", "components": {"Card": {}}}"#,
/// ));
///
/// let catalog = catalog_context(&input).unwrap();
/// assert_eq!(catalog.catalog_id.as_deref(), Some("https://example.com/catalog.json"));
/// assert!(catalog.capabilities(A2uiVersion::V0_9).is_some());
/// ```
pub fn catalog_context(input: &RunAgentInput) -> Option<CatalogContext> {
    let entry = input
        .context
        .iter()
        .find(|entry| entry.description == SCHEMA_CONTEXT_DESCRIPTION)?;
    let schema: Value = serde_json::from_str(&entry.value).ok()?;
    Some(CatalogContext::new(schema))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{CatalogConfig, CatalogDocument, resolve_catalogs};
    use ag_ui::Context;
    use serde_json::json;

    #[test]
    fn an_action_is_read_where_the_frontends_put_it() {
        let mut input = RunAgentInput::new("t", "r");
        for not_an_action in [
            json!(null),
            json!({"a2uiAction": {}}),
            json!({"a2uiAction": {"userAction": "checkout"}}),
            json!({"userAction": {"name": "checkout"}}),
        ] {
            input.forwarded_props = not_an_action;
            assert!(user_action(&input).is_none());
        }

        input.forwarded_props = json!({"a2uiAction": {"userAction": {
            "name": "checkout", "surfaceId": "cart", "sourceComponentId": "pay",
            "context": {"total": 12}, "timestamp": "2026-01-01T00:00:00Z", "extra": true
        }}});
        let action = user_action(&input).unwrap();
        assert_eq!(action.source_component_id.as_deref(), Some("pay"));
        assert_eq!(action.context.unwrap()["total"], 12);
        // Sent back the way it came, without what was not there.
        assert_eq!(
            UserAction {
                name: Some("checkout".into()),
                ..UserAction::default()
            }
            .as_user_text(),
            "A2UI user action:\n{\"name\":\"checkout\"}"
        );
    }

    #[test]
    fn the_catalog_entry_is_found_by_its_description() {
        let mut input = RunAgentInput::new("t", "r");
        input
            .context
            .push(Context::new("the user's locale", "en-GB"));
        assert!(catalog_context(&input).is_none());

        input
            .context
            .push(Context::new(SCHEMA_CONTEXT_DESCRIPTION, "not JSON"));
        assert!(catalog_context(&input).is_none());

        input.context.pop();
        input.context.push(Context::new(
            SCHEMA_CONTEXT_DESCRIPTION,
            json!({"catalogId": "example.com:cards", "components": {
                "Card": {"type": "object", "properties": {"title": {"type": "string"}}}
            }})
            .to_string(),
        ));
        let catalog = catalog_context(&input).unwrap();
        assert_eq!(catalog.catalog_id.as_deref(), Some("example.com:cards"));

        // An agent that takes a frontend's own catalog gets it active.
        let capabilities = catalog.capabilities(A2uiVersion::V0_9).unwrap();
        let registered = [CatalogConfig::basic(A2uiVersion::V0_9).unwrap()];
        let active = resolve_catalogs(&registered, Some(&capabilities), true).unwrap();
        assert_eq!(
            active
                .iter()
                .map(CatalogDocument::catalog_id)
                .collect::<Vec<_>>(),
            ["example.com:cards"]
        );
        // One that does not is told the renderer draws nothing it has.
        assert!(resolve_catalogs(&registered, Some(&capabilities), false).is_err());
    }

    #[test]
    fn an_entry_that_names_no_catalog_negotiates_nothing() {
        let mut input = RunAgentInput::new("t", "r");
        input.context.push(Context::new(
            SCHEMA_CONTEXT_DESCRIPTION,
            r#"[{"name": "Card"}]"#,
        ));
        let catalog = catalog_context(&input).unwrap();
        assert_eq!(catalog.catalog_id, None);
        assert!(catalog.capabilities(A2uiVersion::V0_9).is_none());
    }
}
