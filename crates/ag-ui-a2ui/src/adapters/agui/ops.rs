//! Building the operation stream that renders a surface.
//!
//! Three builders plus [`assemble_ops`], which puts them in the order a
//! renderer expects: create the surface, define its components, then supply the
//! data.
//!
//! # `intent = "update"` must not re-create the surface
//!
//! `createSurface` allocates a `surfaceId` and fixes its catalog. Sending it
//! again for a surface that already exists is an error per spec — the renderer
//! rejects it, and the frontend surfaces that failure to the user. Editing an
//! existing surface therefore means sending only `updateComponents` and
//! `updateDataModel`. [`Intent::Update`] encodes exactly that.

use serde_json::Value;

use crate::constants::{BASIC_CATALOG_ID, DEFAULT_SURFACE_ID};
use crate::protocol::catalog::Catalog;
use crate::protocol::message::{AgentMessage, Component};
use crate::protocol::version::A2uiVersion;

/// The id an AG-UI frontend registers the basic catalog of `version` under,
/// which is the id a surface is created with when the application names no
/// catalog.
///
/// For v0.9 and v0.9.1 that is [`BASIC_CATALOG_ID`], which is not the id the
/// published document declares. For v1.0 the two are the same. Read at
/// `ag-ui-protocol/ag-ui` commit `2330368a`:
///
/// - the toolkit's default, `BASIC_CATALOG_ID`
///   (`sdks/typescript/packages/a2ui-toolkit/src/index.ts:17`, used at lines
///   714 and 833);
/// - the middleware's fallback, `basicCatalogId(protocol)`
///   (`middlewares/a2ui-middleware/src/protocol.ts:19-23`, used at
///   `index.ts:964-968`), which gives the same id for v0.9 and the published
///   one for 1.0;
/// - the renderer behind them, `@copilotkit/a2ui-renderer` 1.87.0 (the
///   version `apps/dojo/package.json` pins), which registers its v0.9 basic
///   catalog under that id and its v1 one under the published id
///   (`dist/react-renderer/a2ui-react/catalog/basic/index.mjs:59-61`).
///
/// The renderer of the A2UI project itself, `@a2ui/web_core`, registers both
/// under the published ids ([`A2uiVersion::basic_catalog_id`]). An
/// application that draws with it names that id
/// ([`SurfaceSpec::with_catalog_id`]).
///
/// ```
/// use ag_ui_a2ui::A2uiVersion;
/// use ag_ui_a2ui::adapters::agui::basic_catalog_id;
///
/// assert_eq!(
///     basic_catalog_id(A2uiVersion::V0_9),
///     "https://a2ui.org/specification/v0_9/basic_catalog.json"
/// );
/// assert_eq!(basic_catalog_id(A2uiVersion::V1_0), A2uiVersion::V1_0.basic_catalog_id());
/// ```
pub fn basic_catalog_id(version: A2uiVersion) -> &'static str {
    if A2uiVersion::V0_9_1.accepts(version) {
        BASIC_CATALOG_ID
    } else {
        version.basic_catalog_id()
    }
}

/// The basic catalog of `version`, under the id an AG-UI frontend registers
/// it with ([`basic_catalog_id`]): the catalog a [`SurfaceSpec`] that names
/// none is drawn from.
///
/// It is what to check an adapter's operations against. [`Catalog::basic`]
/// is the same catalog under the id its document declares, so a
/// [`MessageProcessor`](crate::processor::MessageProcessor) that holds only
/// that one refuses a surface created under this id.
///
/// ```
/// use ag_ui_a2ui::adapters::agui::{Intent, SurfaceSpec, assemble_ops, basic_catalog};
/// use ag_ui_a2ui::{A2uiVersion, Component, MessageProcessor};
/// use serde_json::json;
///
/// let spec = SurfaceSpec::new("cart")
///     .with_components(vec![Component::new("root", "Text").with("text", json!("Cart"))]);
/// let operations: Vec<_> = assemble_ops(Intent::Create, &spec)
///     .iter()
///     .map(serde_json::to_value)
///     .collect::<Result<_, _>>()?;
///
/// let mut processor = MessageProcessor::new();
/// processor.register(basic_catalog(A2uiVersion::V0_9))?;
/// processor.process_all(&operations).map_err(|rejection| rejection.error)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn basic_catalog(version: A2uiVersion) -> Catalog {
    Catalog::basic_for(version).with_catalog_id(basic_catalog_id(version))
}

/// Whether the caller is building a new surface or editing one that exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Intent {
    /// Build a surface that does not exist yet.
    #[default]
    Create,
    /// Edit a surface the renderer already holds.
    Update,
}

impl Intent {
    /// Parses the wire spelling, case-insensitively.
    ///
    /// Anything unrecognized is `None` rather than a silent default: guessing
    /// wrong in the "update" direction re-creates a live surface.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "create" | "new" => Some(Intent::Create),
            "update" | "edit" => Some(Intent::Update),
            _ => None,
        }
    }

    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Intent::Create => "create",
            Intent::Update => "update",
        }
    }

    /// Whether a `createSurface` operation belongs in this stream.
    pub fn needs_create_surface(self) -> bool {
        matches!(self, Intent::Create)
    }
}

impl std::fmt::Display for Intent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a surface should look like, as the authoring layer sees it.
///
/// Non-exhaustive: a later protocol version may give a surface more. Build
/// one with [`SurfaceSpec::new`] and the `with_` methods; the fields stay
/// public to read and to set.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SurfaceSpec {
    /// Target surface id. Defaults to [`DEFAULT_SURFACE_ID`].
    pub surface_id: String,
    /// The protocol version the operations are written in. Defaults to
    /// [`A2uiVersion::DEFAULT`], which is `v0.9`.
    pub version: A2uiVersion,
    /// Catalog the components are drawn from. Defaults to
    /// [`basic_catalog_id`] of the version.
    pub catalog_id: String,
    /// The flat component adjacency list.
    pub components: Vec<Component>,
    /// Initial or updated data model. `None` sends no data operation.
    pub data_model: Option<Value>,
    /// Where the data model is written.
    ///
    /// Defaults to `/`, which replaces the whole model. When updating a live
    /// surface, prefer a narrower pointer: the renderer's two-way bindings write
    /// user input straight into this model, and replacing the root discards it.
    pub data_path: String,
    /// Optional catalog-defined theme for `createSurface`. The v0.9 family
    /// only: a v1.0 creation has none, and one that is given a theme cannot
    /// be written.
    pub theme: Option<Value>,
    /// Ask the renderer to echo the data model back with every message.
    pub send_data_model: Option<bool>,
}

impl Default for SurfaceSpec {
    fn default() -> Self {
        Self {
            surface_id: DEFAULT_SURFACE_ID.to_string(),
            version: A2uiVersion::DEFAULT,
            catalog_id: basic_catalog_id(A2uiVersion::DEFAULT).to_string(),
            components: Vec::new(),
            data_model: None,
            data_path: "/".to_string(),
            theme: None,
            send_data_model: None,
        }
    }
}

impl SurfaceSpec {
    /// A spec for the given surface with default catalog and no content.
    pub fn new(surface_id: impl Into<String>) -> Self {
        Self {
            surface_id: surface_id.into(),
            ..Self::default()
        }
    }

    /// Sets the component list.
    #[must_use]
    pub fn with_components(mut self, components: Vec<Component>) -> Self {
        self.components = components;
        self
    }

    /// Sets the data model, written at [`SurfaceSpec::data_path`].
    #[must_use]
    pub fn with_data_model(mut self, data_model: Value) -> Self {
        self.data_model = Some(data_model);
        self
    }

    /// Sets the protocol version the operations are written in.
    ///
    /// A spec that still names the basic catalog of its version moves to the
    /// basic catalog of the new one ([`basic_catalog_id`]); a catalog the
    /// application named stays. Bindings in the components are the caller's
    /// to write as the version does: [`A2uiVersion::path`].
    ///
    /// ```
    /// use ag_ui_a2ui::adapters::agui::{Intent, SurfaceSpec, assemble_ops};
    /// use ag_ui_a2ui::{A2uiVersion, Component};
    ///
    /// let version = A2uiVersion::V1_0;
    /// let spec = SurfaceSpec::new("cart")
    ///     .with_version(version)
    ///     .with_components(vec![Component::new("root", "Text").with("text", version.path("/total"))]);
    /// let operations = serde_json::to_value(assemble_ops(Intent::Create, &spec))?;
    /// assert_eq!(operations[0]["version"], "1.0");
    /// assert_eq!(
    ///     operations[0]["createSurface"]["catalogId"],
    ///     "https://a2ui.org/specification/v1_0/catalogs/basic/catalog.json"
    /// );
    /// assert_eq!(operations[1]["updateComponents"]["components"][0]["text"]["@path"], "/total");
    /// # Ok::<(), serde_json::Error>(())
    /// ```
    #[must_use]
    pub fn with_version(mut self, version: A2uiVersion) -> Self {
        if self.catalog_id == basic_catalog_id(self.version) {
            self.catalog_id = basic_catalog_id(version).to_string();
        }
        self.version = version;
        self
    }

    /// Sets the catalog the surface is created under.
    ///
    /// A spec starts on the id an AG-UI frontend registers the basic catalog
    /// under ([`basic_catalog_id`]), and [`basic_catalog`] is that catalog.
    /// For v0.9 it is not the id the published document declares
    /// ([`OFFICIAL_BASIC_CATALOG_ID`](crate::constants::OFFICIAL_BASIC_CATALOG_ID)),
    /// which is the one [`Catalog::basic`] carries and the A2UI project's
    /// own renderer registers. A surface for that renderer, or for a catalog
    /// of the application's own, says so here, since a renderer draws a
    /// surface with the catalog its `createSurface` names.
    #[must_use]
    pub fn with_catalog_id(mut self, catalog_id: impl Into<String>) -> Self {
        self.catalog_id = catalog_id.into();
        self
    }

    /// Sets the theme `createSurface` carries. The v0.9 family only.
    #[must_use]
    pub fn with_theme(mut self, theme: Value) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Asks the renderer to send the data model back with every message.
    #[must_use]
    pub fn with_send_data_model(mut self, send: bool) -> Self {
        self.send_data_model = Some(send);
        self
    }

    /// Sets the pointer the data model is written at.
    #[must_use]
    pub fn with_data_path(mut self, data_path: impl Into<String>) -> Self {
        self.data_path = data_path.into();
        self
    }
}

/// Builds a `createSurface` operation.
pub fn create_surface(
    surface_id: impl Into<String>,
    catalog_id: impl Into<String>,
    theme: Option<Value>,
    send_data_model: Option<bool>,
) -> AgentMessage {
    let mut message = AgentMessage::create_surface(surface_id, catalog_id);
    if let crate::protocol::message::AgentPayload::CreateSurface(payload) = &mut message.payload {
        payload.theme = theme;
        payload.send_data_model = send_data_model;
    }
    message
}

/// Builds an `updateComponents` operation.
pub fn update_components(
    surface_id: impl Into<String>,
    components: Vec<Component>,
) -> AgentMessage {
    AgentMessage::update_components(surface_id, components)
}

/// Builds an `updateDataModel` operation.
///
/// A `null` value is stored explicitly; a `path` of `/` replaces the whole
/// data model.
pub fn update_data_model(
    surface_id: impl Into<String>,
    path: impl Into<String>,
    value: Value,
) -> AgentMessage {
    AgentMessage::update_data_model(surface_id, path, value)
}

/// Assembles the full operation stream for a surface.
///
/// Order is `createSurface` (create intent only) → `updateComponents` →
/// `updateDataModel`, so the renderer has a surface to attach to, then a tree to
/// draw, then data to fill it with. Empty components or an absent data model
/// simply omit that operation.
///
/// Every operation states the spec's version. The three stay three in v1.0,
/// whose `createSurface` could carry the components and the data: AG-UI's
/// middleware writes them apart for either protocol
/// (`a2ui-middleware/src/index.ts:711-730`, `streamedOps`), and a frontend
/// that folds a later update onto a surface does so operation by operation.
///
/// ```
/// use ag_ui_a2ui::adapters::agui::{assemble_ops, Intent, SurfaceSpec};
/// use ag_ui_a2ui::message::Component;
/// use serde_json::json;
///
/// let spec = SurfaceSpec::new("cart")
///     .with_components(vec![Component::new("root", "Text").with("text", json!("hi"))]);
///
/// assert_eq!(assemble_ops(Intent::Create, &spec).len(), 2);
/// // Updating an existing surface must not re-create it.
/// assert_eq!(assemble_ops(Intent::Update, &spec).len(), 1);
/// ```
pub fn assemble_ops(intent: Intent, spec: &SurfaceSpec) -> Vec<AgentMessage> {
    let mut ops = Vec::with_capacity(3);

    if intent.needs_create_surface() {
        ops.push(create_surface(
            &spec.surface_id,
            &spec.catalog_id,
            spec.theme.clone(),
            spec.send_data_model,
        ));
    }
    if !spec.components.is_empty() {
        ops.push(update_components(&spec.surface_id, spec.components.clone()));
    }
    if let Some(data_model) = &spec.data_model {
        ops.push(update_data_model(
            &spec.surface_id,
            &spec.data_path,
            data_model.clone(),
        ));
    }
    ops.into_iter()
        .map(|operation| operation.with_version(spec.version))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::message::AgentPayload;
    use serde_json::json;

    #[test]
    fn a_spec_creates_its_surface_under_the_catalog_it_names() {
        let created = |spec: &SurfaceSpec| match &assemble_ops(Intent::Create, spec)[0].payload {
            AgentPayload::CreateSurface(create) => create.catalog_id.clone().unwrap_or_default(),
            other => panic!("expected createSurface first, got {other:?}"),
        };
        assert_eq!(created(&SurfaceSpec::new("s")), BASIC_CATALOG_ID);
        let official = crate::constants::OFFICIAL_BASIC_CATALOG_ID;
        assert_eq!(
            created(&SurfaceSpec::new("s").with_catalog_id(official)),
            official
        );
    }

    fn spec() -> SurfaceSpec {
        SurfaceSpec::new("s1")
            .with_components(vec![
                Component::new("root", "Text").with("text", json!("hello")),
            ])
            .with_data_model(json!({"a": 1}))
    }

    #[test]
    fn create_emits_all_three_operations_in_order() {
        let ops = assemble_ops(Intent::Create, &spec());
        let kinds: Vec<&str> = ops
            .iter()
            .map(|op| match op.payload {
                AgentPayload::CreateSurface(_) => "create",
                AgentPayload::UpdateComponents(_) => "components",
                AgentPayload::UpdateDataModel(_) => "data",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, vec!["create", "components", "data"]);
    }

    #[test]
    fn update_never_emits_create_surface() {
        let ops = assemble_ops(Intent::Update, &spec());
        assert!(
            !ops.iter()
                .any(|op| matches!(op.payload, AgentPayload::CreateSurface(_))),
            "updating an existing surface must not re-create it"
        );
        assert_eq!(ops.len(), 2);
    }

    #[test]
    fn empty_content_omits_its_operation() {
        let bare = SurfaceSpec::new("s1");
        assert_eq!(assemble_ops(Intent::Update, &bare).len(), 0);
        assert_eq!(assemble_ops(Intent::Create, &bare).len(), 1);
    }

    #[test]
    fn defaults_come_from_the_wire_constants() {
        let spec = SurfaceSpec::default();
        assert_eq!(spec.surface_id, DEFAULT_SURFACE_ID);
        assert_eq!(spec.catalog_id, BASIC_CATALOG_ID);
        assert_eq!(spec.version, A2uiVersion::DEFAULT);
        assert_eq!(spec.data_path, "/");
    }

    /// The default catalog of a spec and the catalog the adapter offers for
    /// it are one: a processor that holds `basic_catalog` takes what
    /// `assemble_ops` writes, in every version.
    #[test]
    fn the_default_catalog_of_a_spec_is_the_one_the_adapter_offers() {
        use crate::protocol::processor::{MessageProcessor, RejectionKind};

        for &version in A2uiVersion::ALL {
            let spec = SurfaceSpec::new("s")
                .with_version(version)
                .with_components(vec![
                    Component::new("root", "Text").with("text", version.path("/title")),
                ])
                .with_data_model(json!({"title": "Hello"}));
            assert_eq!(spec.catalog_id, basic_catalog_id(version), "{version}");
            let operations: Vec<Value> = assemble_ops(Intent::Create, &spec)
                .iter()
                .map(|operation| serde_json::to_value(operation).unwrap())
                .collect();
            assert_eq!(operations.len(), 3, "{version}");
            for operation in &operations {
                assert_eq!(operation["version"], version.as_str());
            }

            let catalog = basic_catalog(version);
            assert_eq!(catalog.catalog_id, spec.catalog_id);
            assert_eq!(catalog.version(), version);
            let mut processor = MessageProcessor::new();
            processor.register(catalog).unwrap();
            processor
                .process_all(&operations)
                .unwrap_or_else(|rejection| panic!("{version}: {rejection}"));

            // The catalog under the id its document declares is another name.
            let mut published = MessageProcessor::new();
            published.register(Catalog::basic_for(version)).unwrap();
            let refused = published.process_all(&operations);
            if basic_catalog_id(version) == version.basic_catalog_id() {
                refused.unwrap();
            } else {
                assert_eq!(refused.unwrap_err().kind, RejectionKind::Catalog);
            }
        }
        assert_eq!(basic_catalog_id(A2uiVersion::V0_9_1), BASIC_CATALOG_ID);
        assert_eq!(
            basic_catalog_id(A2uiVersion::V1_0),
            "https://a2ui.org/specification/v1_0/catalogs/basic/catalog.json"
        );
    }

    #[test]
    fn a_catalog_the_application_named_stays_when_the_version_changes() {
        let named = SurfaceSpec::new("s")
            .with_catalog_id("example.com:cards")
            .with_version(A2uiVersion::V1_0);
        assert_eq!(named.catalog_id, "example.com:cards");
        // And back: the default follows the version both ways.
        let back = SurfaceSpec::new("s")
            .with_version(A2uiVersion::V1_0)
            .with_version(A2uiVersion::V0_9_1);
        assert_eq!(back.catalog_id, BASIC_CATALOG_ID);
    }

    #[test]
    fn theme_and_send_data_model_ride_on_create_surface() {
        let spec = spec()
            .with_theme(json!({"primaryColor": "#00BFFF"}))
            .with_send_data_model(true);
        let ops = assemble_ops(Intent::Create, &spec);
        let AgentPayload::CreateSurface(payload) = &ops[0].payload else {
            panic!("expected createSurface");
        };
        assert_eq!(payload.theme, Some(json!({"primaryColor": "#00BFFF"})));
        assert_eq!(payload.send_data_model, Some(true));
    }

    #[test]
    fn a_narrower_data_path_is_preserved() {
        let spec = SurfaceSpec::new("s1")
            .with_data_model(json!("Ada"))
            .with_data_path("/user/name");
        let ops = assemble_ops(Intent::Update, &spec);
        let AgentPayload::UpdateDataModel(payload) = &ops[0].payload else {
            panic!("expected updateDataModel");
        };
        assert_eq!(payload.path, "/user/name");
    }

    #[test]
    fn intent_parses_the_wire_spellings_and_rejects_the_rest() {
        assert_eq!(Intent::from_wire("create"), Some(Intent::Create));
        assert_eq!(Intent::from_wire("  UPDATE "), Some(Intent::Update));
        assert_eq!(Intent::from_wire("edit"), Some(Intent::Update));
        assert_eq!(Intent::from_wire("replace"), None);
        assert_eq!(Intent::default(), Intent::Create);
    }
}
