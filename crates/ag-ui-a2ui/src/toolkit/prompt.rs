//! Assembling the prompt for the surface-generating model.
//!
//! A2UI v0.9 is designed to be prompted rather than schema-constrained: the
//! catalog goes into the prompt, the model writes JSON against it, and the
//! result is validated afterwards. That makes prompt assembly load-bearing, so
//! it lives here rather than being open-coded per agent.
//!
//! A prompt has five parts, in this order:
//!
//! 1. the role,
//! 2. the generation guidelines (the rules that make output parseable),
//! 3. the catalog,
//! 4. the conversation context,
//! 5. the current surface, when updating one.
//!
//! Section 5 is what makes `intent = "update"` work: without the components
//! already on screen, the model cannot edit them, only replace them.

use serde_json::Value;

use crate::catalog::Catalog;
use crate::constants::{A2UI_CLOSE_TAG, A2UI_OPEN_TAG, ROOT_ID};
use crate::toolkit::history::PriorSurface;
use crate::toolkit::ops::Intent;
use crate::validate::ValidationError;

/// The rules that keep generated A2UI parseable and renderable.
///
/// Ordering is not cosmetic: a streaming renderer paints as components arrive,
/// so `root` first and parents before children is what makes progressive
/// rendering work.
pub const GENERATION_GUIDELINES: &str = "\
- Reply with a flat list of A2UI messages as raw JSON. No prose inside the JSON.
- Every component object is `{\"id\": <unique id>, \"component\": <type>, ...props}`.
- Components are NEVER nested inline. A parent names its children by id.
- Exactly one component must have `\"id\": \"root\"`; it is the top of the tree.
- Order components top-down: `root` first, every parent before its children.
- Reference only ids that exist in the same payload.
- Never create a reference loop: a component must not be its own ancestor.
- Static text goes in the component; only bind to the data model when the value \
is dynamic.
- Data bindings are `{\"path\": \"/pointer\"}` (JSON Pointer, absolute) and must \
point at data you also send.
- Relative paths (no leading `/`) are only valid inside a list template.
- Use only component types and properties from the catalog below.";

/// Everything the prompt builder needs.
#[derive(Debug, Clone)]
pub struct PromptSpec<'a> {
    /// What the model is, e.g. "You generate UI surfaces for a travel agent."
    pub role: &'a str,
    /// What the user asked for on this turn.
    pub request: &'a str,
    /// Whether a new surface is being built or an existing one edited.
    pub intent: Intent,
    /// The surface id the model should target.
    pub surface_id: &'a str,
    /// The catalog the model must stay inside.
    pub catalog: &'a Catalog,
    /// Prior conversation turns, oldest first, as `(role, text)`.
    pub conversation: &'a [(String, String)],
    /// The surface currently on screen, for [`Intent::Update`].
    pub prior_surface: Option<&'a PriorSurface>,
    /// Extra design guidance from the application.
    pub ui_description: Option<&'a str>,
    /// Whether to spell out the response format with the `<a2ui-json>` tags.
    ///
    /// Turn this off when the model answers through a structured-output tool,
    /// where the tags would end up inside the JSON.
    pub include_response_format: bool,
}

impl<'a> PromptSpec<'a> {
    /// A spec with sensible defaults for a create-intent turn.
    pub fn new(role: &'a str, request: &'a str, catalog: &'a Catalog) -> Self {
        Self {
            role,
            request,
            intent: Intent::Create,
            surface_id: crate::constants::DEFAULT_SURFACE_ID,
            catalog,
            conversation: &[],
            prior_surface: None,
            ui_description: None,
            include_response_format: true,
        }
    }

    /// Points the spec at an existing surface, switching to update intent.
    #[must_use]
    pub fn updating(mut self, prior: &'a PriorSurface) -> Self {
        self.intent = Intent::Update;
        self.surface_id = &prior.surface_id;
        self.prior_surface = Some(prior);
        self
    }

    /// Supplies prior conversation turns.
    #[must_use]
    pub fn with_conversation(mut self, conversation: &'a [(String, String)]) -> Self {
        self.conversation = conversation;
        self
    }
}

/// Builds the full system prompt for the generating model.
pub fn build_subagent_prompt(spec: &PromptSpec<'_>) -> String {
    let mut sections: Vec<String> = Vec::new();
    sections.push(spec.role.trim().to_string());

    sections.push(format!(
        "## Task\n{}\n\nTarget surfaceId: {}\nIntent: {}",
        spec.request.trim(),
        spec.surface_id,
        spec.intent
    ));

    let mut rules = GENERATION_GUIDELINES.to_string();
    if spec.intent == Intent::Update {
        rules.push_str(&format!(
            "\n- This surface already exists. Do NOT emit `createSurface` for '{}'; send only \
             the components and data that change.",
            spec.surface_id
        ));
    }
    sections.push(format!("## Generation rules\n{rules}"));

    if let Some(description) = spec.ui_description {
        sections.push(format!("## Design guidance\n{}", description.trim()));
    }

    sections.push(
        spec.catalog
            .render_llm_instructions()
            .trim_end()
            .to_string(),
    );

    if !spec.conversation.is_empty() {
        let mut block = String::from("### Conversation so far\n");
        for (role, text) in spec.conversation {
            block.push_str(&format!("{role}: {}\n", text.trim()));
        }
        sections.push(block.trim_end().to_string());
    }

    if let Some(prior) = spec.prior_surface {
        sections.push(render_prior_surface(prior));
    }

    if spec.include_response_format {
        sections.push(format!(
            "## Response format\nWrap each A2UI JSON block in {A2UI_OPEN_TAG} and \
             {A2UI_CLOSE_TAG}. Conversational text may go before or after a block, never inside \
             one."
        ));
    }
    sections.join("\n\n")
}

/// Renders the surface currently on screen, so the model can edit it.
fn render_prior_surface(prior: &PriorSurface) -> String {
    let components =
        serde_json::to_string_pretty(&prior.components).unwrap_or_else(|_| "[]".to_string());
    let data =
        serde_json::to_string_pretty(&prior.data_model).unwrap_or_else(|_| "null".to_string());
    let catalog = prior
        .catalog_id
        .as_deref()
        .map(|id| format!("\ncatalogId: {id} (fixed; it cannot change)"))
        .unwrap_or_default();

    format!(
        "## Surface currently on screen\nsurfaceId: {}{catalog}\n\nComponents:\n```json\n{components}\n```\n\n\
         Data model:\n```json\n{data}\n```\n\nRe-send only the components you change, keeping \
         their ids. Keep '{ROOT_ID}' as the root.",
        prior.surface_id
    )
}

/// Formats validation errors for a retry prompt.
///
/// One line per error, code first so the model can see the kind at a glance,
/// then the locator, then the sentence explaining the fix.
pub fn format_validation_errors(errors: &[ValidationError]) -> String {
    if errors.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "The previous attempt was rejected. Fix every problem below and return the corrected \
         A2UI, not a diff:\n",
    );
    for error in errors {
        out.push_str(&format!(
            "- [{}] at {}: {}\n",
            error.code, error.path, error.message
        ));
    }
    out.trim_end().to_string()
}

/// Appends the errors from a failed attempt to the prompt for the next one.
pub fn augment_prompt_with_errors(prompt: &str, errors: &[ValidationError]) -> String {
    if errors.is_empty() {
        return prompt.to_string();
    }
    format!(
        "{prompt}\n\n## Correction required\n{}",
        format_validation_errors(errors)
    )
}

/// A short human-readable summary of a surface, for logs and activity reports.
pub fn describe_surface(prior: &PriorSurface) -> String {
    let mut kinds: Vec<&str> = prior
        .components
        .iter()
        .map(|c| c.component.as_str())
        .collect();
    kinds.sort_unstable();
    kinds.dedup();
    format!(
        "surface '{}' with {} component(s) [{}]",
        prior.surface_id,
        prior.components.len(),
        kinds.join(", ")
    )
}

/// Renders a data model compactly for inclusion in a prompt.
pub fn render_data_model(data_model: &Value) -> String {
    serde_json::to_string_pretty(data_model).unwrap_or_else(|_| "null".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Component;
    use serde_json::json;

    fn prior() -> PriorSurface {
        PriorSurface {
            surface_id: "cart".to_string(),
            catalog_id: Some("cat".to_string()),
            components: vec![
                Component::new("root", "Column").with("children", json!(["title"])),
                Component::new("title", "Text").with("text", json!("Cart")),
            ],
            data_model: json!({"total": 12}),
            deleted: false,
        }
    }

    #[test]
    fn a_create_prompt_has_every_section() {
        let catalog = Catalog::basic();
        let conversation = vec![("user".to_string(), "show my cart".to_string())];
        let spec = PromptSpec::new("You build UI.", "Render the cart.", &catalog)
            .with_conversation(&conversation);
        let prompt = build_subagent_prompt(&spec);

        assert!(prompt.starts_with("You build UI."));
        assert!(prompt.contains("## Task"));
        assert!(prompt.contains("Intent: create"));
        assert!(prompt.contains("## Generation rules"));
        assert!(prompt.contains("### Component catalog"));
        assert!(prompt.contains("- Text required: text (value)"));
        assert!(prompt.contains("### Conversation so far"));
        assert!(prompt.contains("user: show my cart"));
        assert!(prompt.contains(A2UI_OPEN_TAG));
        assert!(!prompt.contains("## Surface currently on screen"));
    }

    #[test]
    fn an_update_prompt_forbids_create_surface_and_shows_the_current_tree() {
        let catalog = Catalog::basic();
        let prior = prior();
        let spec =
            PromptSpec::new("You build UI.", "Add a checkout button.", &catalog).updating(&prior);
        let prompt = build_subagent_prompt(&spec);

        assert!(prompt.contains("Intent: update"));
        assert!(prompt.contains("Do NOT emit `createSurface` for 'cart'"));
        assert!(prompt.contains("## Surface currently on screen"));
        assert!(prompt.contains("catalogId: cat (fixed; it cannot change)"));
        assert!(prompt.contains("\"id\": \"title\""));
        assert!(prompt.contains("\"total\": 12"));
        assert!(prompt.contains("Target surfaceId: cart"));
    }

    #[test]
    fn the_response_format_section_can_be_suppressed() {
        let catalog = Catalog::basic();
        let mut spec = PromptSpec::new("role", "request", &catalog);
        spec.include_response_format = false;
        let prompt = build_subagent_prompt(&spec);
        assert!(!prompt.contains(A2UI_OPEN_TAG));
    }

    #[test]
    fn design_guidance_is_included_when_supplied() {
        let catalog = Catalog::basic();
        let mut spec = PromptSpec::new("role", "request", &catalog);
        spec.ui_description = Some("Use the brand blue for primary buttons.");
        assert!(build_subagent_prompt(&spec).contains("Use the brand blue"));
    }

    #[test]
    fn validation_errors_render_one_actionable_line_each() {
        let errors = vec![
            ValidationError::new(
                crate::validate::ErrorCode::NoRoot,
                "components",
                "No component has id 'root'.",
            ),
            ValidationError::new(
                crate::validate::ErrorCode::ChildCycle,
                "components[1].child",
                "Child references form a loop: a -> b -> a.",
            ),
        ];
        let rendered = format_validation_errors(&errors);
        assert!(rendered.contains("- [no_root] at components: No component has id 'root'."));
        assert!(rendered.contains("- [child_cycle] at components[1].child:"));
        assert_eq!(format_validation_errors(&[]), "");

        let augmented = augment_prompt_with_errors("base prompt", &errors);
        assert!(augmented.starts_with("base prompt"));
        assert!(augmented.contains("## Correction required"));
        assert_eq!(
            augment_prompt_with_errors("base prompt", &[]),
            "base prompt"
        );
    }

    #[test]
    fn surfaces_summarize_for_logs() {
        assert_eq!(
            describe_surface(&prior()),
            "surface 'cart' with 2 component(s) [Column, Text]"
        );
    }

    #[test]
    fn guidelines_state_the_rules_the_validator_enforces() {
        for rule in [
            "\"id\": \"root\"",
            "NEVER nested inline",
            "reference loop",
            "top-down",
        ] {
            assert!(
                GENERATION_GUIDELINES
                    .to_lowercase()
                    .contains(&rule.to_lowercase()),
                "guidelines omit {rule}"
            );
        }
    }
}
