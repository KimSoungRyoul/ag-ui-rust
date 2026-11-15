//! The two tools an A2UI agent on AG-UI works with, and their arguments.
//!
//! They sit at different levels and are easy to confuse:
//!
//! - [`generate_a2ui_tool`] is **planner-facing**. The orchestrating model
//!   calls it to say "render something, as a new surface or as a change to
//!   that one". Its arguments are an intent, not components.
//! - [`render_a2ui_tool`] is the **structured-output** tool. The generating
//!   model calls it to emit the surface itself: a surface id, a flat component
//!   list and a data model.
//!
//! Names and arguments are the ones AG-UI's A2UI toolkit and middleware use,
//! so a frontend that recognises a call by its name and reads its arguments
//! sees the same call from a Rust agent. The catalog is not an argument of
//! either: which catalog a surface speaks is the host's to say, since a model
//! must not be able to name one the renderer never registered.

use ag_ui::Tool;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::ops::{Intent, SurfaceSpec};
use crate::constants::{GENERATE_A2UI_TOOL_NAME, RENDER_A2UI_TOOL_NAME, ROOT_ID};
use crate::protocol::message::Component;

/// The structured-output tool: emit a surface as
/// `{surfaceId, components, data}`.
///
/// ```
/// use ag_ui_a2ui::adapters::agui::render_a2ui_tool;
///
/// let tool = render_a2ui_tool();
/// assert_eq!(tool.name, "render_a2ui");
/// assert_eq!(tool.parameters["required"], serde_json::json!(["surfaceId", "components"]));
/// ```
pub fn render_a2ui_tool() -> Tool {
    Tool::new(
        RENDER_A2UI_TOOL_NAME,
        format!(
            "Render a dynamic A2UI v0.9 surface. The root component must have id '{ROOT_ID}'. \
             Use components from the available catalog only."
        ),
        json!({
            "type": "object",
            "properties": {
                "surfaceId": {
                    "type": "string",
                    "description": "Unique surface identifier."
                },
                "components": {
                    "type": "array",
                    "description": format!(
                        "A2UI v0.9 component array (flat format). The root component must have \
                         id '{ROOT_ID}'."
                    ),
                    "items": {"type": "object"}
                },
                "data": {
                    "type": "object",
                    "description": "Optional initial data model for the surface (form values, \
                                    list items, etc.)."
                }
            },
            "required": ["surfaceId", "components"]
        }),
    )
}

/// The arguments of a [`render_a2ui_tool`] call.
///
/// ```
/// use ag_ui_a2ui::adapters::agui::{Intent, RenderArgs, assemble_ops};
/// use serde_json::json;
///
/// let args: RenderArgs = serde_json::from_value(json!({
///     "surfaceId": "cart",
///     "components": [{"id": "root", "component": "Text", "text": {"path": "/total"}}],
///     "data": {"total": "12.00"}
/// }))?;
/// // The host names the catalog; the model does not.
/// let spec = args.into_spec("https://a2ui.org/specification/v0_9/basic_catalog.json");
/// assert_eq!(assemble_ops(Intent::Create, &spec).len(), 3);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderArgs {
    /// The surface to draw.
    pub surface_id: String,
    /// The flat component list.
    pub components: Vec<Component>,
    /// The data model, written at the root. Absent sends no data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RenderArgs {
    /// The surface these arguments describe, under the catalog the host
    /// chose.
    pub fn into_spec(self, catalog_id: impl Into<String>) -> SurfaceSpec {
        let mut spec = SurfaceSpec::new(self.surface_id)
            .with_components(self.components)
            .with_catalog_id(catalog_id);
        spec.data_model = self.data;
        spec
    }
}

/// The planner-facing tool: ask for a surface, new or changed, as
/// `{intent?, target_surface_id?, changes?}`.
///
/// ```
/// use ag_ui_a2ui::adapters::agui::generate_a2ui_tool;
///
/// let tool = generate_a2ui_tool();
/// assert_eq!(tool.name, "generate_a2ui");
/// assert!(tool.parameters["properties"]["target_surface_id"].is_object());
/// ```
pub fn generate_a2ui_tool() -> Tool {
    Tool::new(
        GENERATE_A2UI_TOOL_NAME,
        "Generate or update a dynamic A2UI surface based on the conversation. A secondary LLM \
         designs the UI components and data. Use intent='create' (default) when the user \
         requests new visual content (cards, forms, lists, dashboards, comparisons, etc.). Use \
         intent='update' with target_surface_id to modify a surface you previously rendered \
         (e.g. 'change the second card's price', 'add a Buy button', 'use red instead of blue').",
        json!({
            "type": "object",
            "properties": {
                "intent": {
                    "type": "string",
                    "enum": ["create", "update"],
                    "description": "'create' to render a new surface; 'update' to modify a \
                                    surface previously rendered in this conversation. Defaults \
                                    to 'create'."
                },
                "target_surface_id": {
                    "type": "string",
                    "description": "Required when intent='update'. The surface id of the prior \
                                    render to modify."
                },
                "changes": {
                    "type": "string",
                    "description": "Optional natural-language description of the changes to \
                                    apply when intent='update'."
                }
            }
        }),
    )
}

/// The arguments of a [`generate_a2ui_tool`] call.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerateArgs {
    /// `"create"` or `"update"`; absent means create.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    /// The surface an update changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_surface_id: Option<String>,
    /// What an update should change, in words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changes: Option<String>,
}

impl GenerateArgs {
    /// What the planner asked for. An update needs a target: `"update"` with
    /// no `target_surface_id` is a create, as it is to AG-UI's toolkit.
    ///
    /// ```
    /// use ag_ui_a2ui::adapters::agui::{GenerateArgs, Intent};
    /// use serde_json::json;
    ///
    /// let update: GenerateArgs =
    ///     serde_json::from_value(json!({"intent": "update", "target_surface_id": "cart"}))?;
    /// assert_eq!(update.intent(), Intent::Update);
    /// let untargeted: GenerateArgs = serde_json::from_value(json!({"intent": "update"}))?;
    /// assert_eq!(untargeted.intent(), Intent::Create);
    /// assert_eq!(GenerateArgs::default().intent(), Intent::Create);
    /// # Ok::<(), serde_json::Error>(())
    /// ```
    pub fn intent(&self) -> Intent {
        let targeted = self
            .target_surface_id
            .as_deref()
            .is_some_and(|id| !id.is_empty());
        match self.intent.as_deref().and_then(Intent::from_wire) {
            Some(Intent::Update) if targeted => Intent::Update,
            _ => Intent::Create,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_render_tool_takes_what_upstream_takes() {
        let tool = render_a2ui_tool();
        let properties = tool.parameters["properties"].as_object().unwrap();
        assert_eq!(
            properties.keys().collect::<Vec<_>>(),
            ["components", "data", "surfaceId"]
        );
        assert_eq!(
            tool.parameters["required"],
            json!(["surfaceId", "components"])
        );
    }

    #[test]
    fn the_generate_tool_requires_nothing() {
        let tool = generate_a2ui_tool();
        let properties = tool.parameters["properties"].as_object().unwrap();
        assert_eq!(
            properties.keys().collect::<Vec<_>>(),
            ["changes", "intent", "target_surface_id"]
        );
        assert!(tool.parameters.get("required").is_none());
    }

    #[test]
    fn render_arguments_read_and_write_the_wire_names() {
        let wire = json!({
            "surfaceId": "s",
            "components": [{"id": "root", "component": "Text", "text": "hi"}]
        });
        let args: RenderArgs = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(args.surface_id, "s");
        assert_eq!(args.data, None);
        assert_eq!(serde_json::to_value(&args).unwrap(), wire);
        // The model cannot choose a catalog: an argument by that name is not read.
        let spec = args.into_spec("the/host/catalog");
        assert_eq!(spec.catalog_id, "the/host/catalog");
    }
}
