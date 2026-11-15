//! Semantic validation of a component tree.
//!
//! JSON Schema can say that `children` is an array of strings. It cannot say
//! that every one of those strings names a component that exists, that the tree
//! has a root, or that `a → b → a` is a loop the renderer will never finish
//! drawing. That is what this module checks.
//!
//! Every failure is a [`ValidationError`] carrying a machine-readable
//! [`ErrorCode`], a `path` locator into the components list, and a sentence
//! written to be fed straight back to a model on retry. The validator collects
//! *all* errors rather than stopping at the first, so one retry can fix
//! everything at once.
//!
//! # Depth
//!
//! The component graph is walked iteratively, with an explicit worklist, in
//! every case — cycle detection, reachability, and scope assignment. That is not
//! a style preference: the graph is model-generated and its depth is bounded by
//! nothing, so a recursive walk would abort the process rather than fail a
//! request. [`MAX_DEPTH`] is therefore a *policy* about what a renderer will
//! draw, not what keeps this crate standing, and it can be raised safely.
//!
//! # Full surfaces and incremental updates
//!
//! A payload that creates a surface is held to the full contract: a `root` must
//! exist and every child reference must resolve within the payload. An
//! incremental `updateComponents` is not — its components may legitimately
//! reference ids the renderer already holds, and it need not include the root.
//! [`ValidateOptions::incremental_update`] relaxes exactly those two rules;
//! duplicate ids and cycles still fail, because those are broken either way.
//!
//! ```
//! use ag_ui_a2ui::{catalog::Catalog, message::Component, validate::{ErrorCode, Validator}};
//! use serde_json::json;
//!
//! let catalog = Catalog::basic();
//! let report = Validator::new(&catalog).validate(&[
//!     Component::new("root", "Card").with("child", json!("nope")),
//! ]);
//! assert_eq!(report.errors[0].code, ErrorCode::UnresolvedChild);
//! assert_eq!(report.errors[0].path, "components[0].child");
//! ```

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::binding::{
    ModelScope, TemplatePart, collect_bindings_in, literal_key, names_the_scope, parse_template,
};
use super::catalog::{AllowedCallers, Catalog, ComponentDef, PropKind};
use super::graph::{Cycle, Finding, Graph};
use super::message::{AgentMessage, Component};
use super::model::{DataModel, ModelValue};
use super::operation::Operation;
use super::surface::{Origin, SurfaceState, SurfaceStore};
use super::version::A2uiVersion;
use crate::constants::ROOT_ID;
use crate::error::{Error, Result, ShownPath, ValidationErrors};

#[cfg(test)]
thread_local! {
    /// How many (component, scope) entries the binding walk took off its
    /// queue, on this thread: what a test reads to hold the walk linear.
    static BINDING_SCOPES_QUEUED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Deepest nesting accepted by default, for both the component graph and the
/// raw JSON of a message.
///
/// Matches the limit every other A2UI toolkit enforces, so a payload one of them
/// accepts is accepted here and vice versa. Nothing in this crate needs the cap
/// to stay safe — every walk is iterative — but a renderer that recurses does,
/// and the input is model-generated.
pub const MAX_DEPTH: usize = 50;

/// Deepest chain of nested function calls accepted by default.
pub const MAX_FUNCTION_CALL_DEPTH: usize = 5;

/// The complete set of semantic failures this validator reports.
///
/// These codes are the contract with callers that route on them (a recovery
/// loop, a renderer's error channel). The enum is non-exhaustive, so that a
/// protocol version can bring a failure of its own: v1.0 brought the last
/// four. A caller that matches on a code keeps a wildcard arm.
///
/// [`crate::catalog::CompositionCode`] has the two composition failures as
/// [`Catalog::composition_violations`](crate::Catalog::composition_violations)
/// reports them, to a caller that asks the catalog directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorCode {
    /// The payload declares a surface but carries no components.
    EmptyComponents,
    /// A component has no usable `id`.
    MissingId,
    /// A component has no usable `component` type name.
    MissingComponentType,
    /// Two components share an `id`.
    DuplicateId,
    /// No component has the root id, so the renderer has nothing to draw from.
    NoRoot,
    /// A component's type is not defined by the surface's catalog.
    UnknownComponent,
    /// A property the catalog marks required is missing.
    MissingRequiredProp,
    /// A field the protocol requires on a message envelope is missing.
    ///
    /// Distinct from [`ErrorCode::MissingRequiredProp`], which is about a
    /// component property a *catalog* declares: this one is fixed by the wire
    /// format and holds whatever catalog is in play.
    MissingField,
    /// A value has the right shape but is not one the protocol permits, such as
    /// a `version` naming a protocol revision this crate does not speak.
    InvalidValue,
    /// A value is of the wrong JSON type — `"3"` where a number is required, or
    /// a number where the catalog declares a string.
    TypeMismatch,
    /// A child reference names a component id that does not exist.
    UnresolvedChild,
    /// Following child references leads back to where it started.
    ChildCycle,
    /// A data binding cannot resolve against the surface's data model.
    UnresolvedBinding,
    /// Nesting runs deeper than the configured maximum.
    ///
    /// Distinct from [`ErrorCode::ChildCycle`]: a deep tree is finite and
    /// acyclic, it is just deeper than anything a renderer will draw, and deep
    /// enough to threaten a recursive consumer. Covers three kinds of nesting —
    /// the component graph, the raw JSON, and chained function calls — because
    /// all three are model-generated and all three are unbounded without a cap.
    MaxDepthExceeded,
    /// A component or a function call names a catalog its surface cannot draw
    /// from, or names none on a surface that has no default. From v1.0, where
    /// a surface may mix catalogs.
    UnknownCatalog,
    /// A key starts with one `@` and is not a directive of the protocol. From
    /// v1.0, which reserves the prefix: a literal key is written `@@…`.
    ReservedKey,
    /// A component sits under a parent its catalog's `allowedParents`
    /// excludes: a renderer's `UNALLOWED_PARENT`. Reported in a version that
    /// [constrains composition](A2uiVersion::constrains_composition), once
    /// nothing else is wrong with the tree.
    UnallowedParent,
    /// A container holds a child its catalog's `allowedChildren` excludes.
    /// A renderer's `UNALLOWED_CHILD`.
    UnallowedChild,
}

impl ErrorCode {
    /// The wire string for this code.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::EmptyComponents => "empty_components",
            ErrorCode::MissingId => "missing_id",
            ErrorCode::MissingComponentType => "missing_component_type",
            ErrorCode::DuplicateId => "duplicate_id",
            ErrorCode::NoRoot => "no_root",
            ErrorCode::UnknownComponent => "unknown_component",
            ErrorCode::MissingRequiredProp => "missing_required_prop",
            // These three are spelled the way the conformance suite spells
            // them, so a caller routing on codes sees the same strings from
            // every A2UI toolkit.
            ErrorCode::MissingField => "missing_field",
            ErrorCode::InvalidValue => "invalid_value",
            ErrorCode::TypeMismatch => "type_mismatch",
            ErrorCode::UnresolvedChild => "unresolved_child",
            ErrorCode::ChildCycle => "child_cycle",
            ErrorCode::UnresolvedBinding => "unresolved_binding",
            ErrorCode::MaxDepthExceeded => "max_depth_exceeded",
            ErrorCode::UnknownCatalog => "unknown_catalog",
            ErrorCode::ReservedKey => "reserved_key",
            ErrorCode::UnallowedParent => "unallowed_parent",
            ErrorCode::UnallowedChild => "unallowed_child",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One semantic failure, located and explained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationError {
    /// What kind of failure this is.
    pub code: ErrorCode,
    /// Where it is, e.g. `components[2].component` or `components[0].children[1]`,
    /// or `messages[1].updateComponents.components[0].child` from the entry
    /// points that take whole messages.
    pub path: String,
    /// A sentence a human or a model can act on.
    pub message: String,
}

impl ValidationError {
    /// Builds an error.
    pub fn new(code: ErrorCode, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code,
            path: path.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}: {}", self.code, self.path, self.message)
    }
}

/// What the validator should demand of a payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidateOptions {
    /// Id the tree root must have. Defaults to [`ROOT_ID`].
    pub root_id: String,
    /// Whether a component with the root id must be present.
    pub require_root: bool,
    /// Whether child references may point outside this payload.
    pub allow_dangling_children: bool,
    /// Whether component types must exist in the catalog.
    ///
    /// Turned off automatically when the catalog defines no components at all,
    /// since that means the caller has not supplied one.
    pub check_component_types: bool,
    /// Whether required properties are enforced.
    pub check_required_props: bool,
    /// Whether property values must match the JSON type the catalog declares,
    /// and a literal string be one of the values its `enum` lists.
    ///
    /// Only properties the catalog pins to one type are type-checked, and a
    /// value the renderer resolves — a `{"path": …}` binding or a function call
    /// — is never checked, because its type on the wire says nothing about the
    /// type it will have. See [`PropType`](super::catalog::PropType).
    ///
    /// Structural properties are outside this switch: a child reference must be
    /// a component id however this is set, because a child written any other
    /// way is not in the tree the other checks walk. See [`PropKind`].
    pub check_prop_types: bool,
    /// Whether message envelopes must satisfy the wire contract of the
    /// version they state.
    ///
    /// Applies only to the raw-message entry points
    /// ([`Validator::validate_json_messages`] and
    /// [`Validator::validate_messages`]); the component entry points are handed
    /// components directly and have no envelope to check.
    pub check_envelope: bool,
    /// Whether bindings use valid collection scopes and existing collections are arrays.
    pub check_bindings: bool,
    /// Whether missing or undefined bound values are reported as errors.
    ///
    /// Disabled by default: fields may be initialized by the renderer or a later
    /// update. Enable this lint to require a fully populated data model when
    /// [`Self::check_bindings`] is enabled.
    pub require_bound_values: bool,
    /// Whether absolute and relative binding paths use valid JSON Pointer escapes.
    ///
    /// Separate from [`ValidateOptions::check_bindings`] because it needs no
    /// data model and cannot produce a false positive: a malformed escape can
    /// never resolve, whatever the data turns out to be.
    pub check_binding_syntax: bool,
    /// Deepest nesting accepted, for both the component graph and the raw JSON.
    ///
    /// Defaults to [`MAX_DEPTH`]. Raising it is safe here — every walk in this
    /// crate is iterative — but a renderer on the other end may not be, and a
    /// tree this deep is a generation failure rather than a design.
    pub max_depth: usize,
    /// Deepest chain of function calls accepted, counting nesting through `args`.
    ///
    /// Defaults to [`MAX_FUNCTION_CALL_DEPTH`].
    pub max_function_call_depth: usize,
}

impl Default for ValidateOptions {
    fn default() -> Self {
        Self::full_surface()
    }
}

impl ValidateOptions {
    /// The full contract, for a payload that creates a surface.
    pub fn full_surface() -> Self {
        Self {
            root_id: ROOT_ID.to_string(),
            require_root: true,
            allow_dangling_children: false,
            check_component_types: true,
            check_required_props: true,
            check_prop_types: true,
            check_envelope: true,
            check_bindings: true,
            require_bound_values: false,
            check_binding_syntax: true,
            max_depth: MAX_DEPTH,
            max_function_call_depth: MAX_FUNCTION_CALL_DEPTH,
        }
    }

    /// The relaxed contract, for a payload that updates an existing surface.
    ///
    /// The root and the referenced components may already live on the renderer,
    /// so their absence from this payload is not an error.
    pub fn incremental_update() -> Self {
        Self {
            require_root: false,
            allow_dangling_children: true,
            ..Self::full_surface()
        }
    }

    /// Overrides the maximum nesting depth.
    #[must_use]
    pub fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.max_depth = max_depth;
        self
    }
}

/// What a validation run found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidationReport {
    /// Failures, in discovery order.
    pub errors: Vec<ValidationError>,
    /// Ids that exist but cannot be reached from the root.
    ///
    /// Not an error: the specification tells renderers to buffer components
    /// until their parent shows up, so an unreachable component is usually a
    /// half-streamed tree rather than a broken one. It is still worth telling a
    /// generating model about, so it is reported separately.
    ///
    /// The component itself is checked like any other: its type, its
    /// properties, its ids. Its bindings are not resolved. What a relative
    /// path means depends on where a component sits, and this one sits
    /// nowhere; and nothing draws it, so nothing reads the data it names. A
    /// path that is not a JSON Pointer is still an error, wherever it is.
    pub unreachable: Vec<String>,
}

impl ValidationReport {
    /// Whether the payload is free of errors.
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    /// Turns the report into a `Result`, discarding warnings.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Validation`] when any error was reported.
    pub fn into_result(self) -> Result<()> {
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(Error::Validation {
                errors: ValidationErrors(self.errors),
            })
        }
    }

    /// The errors as a [`ValidationErrors`] list, for prompting or reporting.
    pub fn errors(&self) -> ValidationErrors {
        ValidationErrors(self.errors.clone())
    }
}

/// Validates component trees against a catalog, or against several.
///
/// A validator made with one catalog ([`Validator::new`]) holds every surface
/// to it, whatever id the surface names. One made over several
/// ([`Validator::over`]) reads them by id: a surface's own, and from v1.0 the
/// one a component names with `@catalogId`.
///
/// The components are read in the spelling of a protocol version: the
/// surface's, where there is one, and otherwise [`Validator::speaking`], which
/// defaults to the version the catalog is written against.
#[derive(Clone, Debug)]
pub struct Validator<'a> {
    /// The one catalog every surface is held to.
    catalog: Option<&'a Catalog>,
    /// The catalogs a surface or a component names, by id.
    registry: BTreeMap<&'a str, &'a Catalog>,
    /// The version components are read in when no surface says.
    version: Option<A2uiVersion>,
    options: ValidateOptions,
}

/// What the components of one run are read against.
#[derive(Clone, Copy)]
struct Drawing<'a, 's> {
    /// The spelling of the components: bindings, envelope keys, identifiers.
    version: A2uiVersion,
    /// The catalog of a component that names none.
    default: Option<&'a Catalog>,
    /// The surface the components are on, when they are on one.
    surface: Option<&'s SurfaceState>,
}

/// Where one component's definition is to be found.
enum Source<'a> {
    /// The catalog the component is drawn from.
    Catalog(&'a Catalog),
    /// The component names no catalog, and its surface has no default.
    Unnamed,
    /// It names a catalog the validator does not hold: the id, and the key
    /// that names it (empty when the surface does).
    Unknown(String, &'static str),
    /// Its catalog is written against another wire format than the surface.
    OtherFormat(&'a Catalog, &'static str),
}

impl<'a> Source<'a> {
    fn catalog(&self) -> Option<&'a Catalog> {
        match self {
            Self::Catalog(catalog) => Some(catalog),
            _ => None,
        }
    }
}

/// The components of a stored surface, each located in the message that last
/// wrote it, or by its position in the surface when none of the store's
/// messages did.
fn stored_nodes<'s>(store: &SurfaceStore, surface: &'s SurfaceState) -> Vec<Node<'s>> {
    let origin = store.origin(&surface.surface_id);
    surface
        .components
        .iter()
        .enumerate()
        .map(|(i, component)| {
            let node = Node::from_component(i, component);
            match origin.and_then(|origin| origin.component(&component.id)) {
                Some(locator) => node.at(locator),
                None => node,
            }
        })
        .collect()
}

/// A component normalized for validation, from either typed or raw JSON input.
struct Node<'a> {
    index: usize,
    /// Where the component is in the caller's input, e.g. `components[2]`.
    path: String,
    id: Option<&'a str>,
    kind: Option<&'a str>,
    props: Option<&'a Map<String, Value>>,
    /// Set when the caller handed us typed components.
    borrowed: Option<&'a Component>,
    /// Set when we rebuilt a typed component from raw JSON.
    owned: Option<Component>,
}

impl<'a> Node<'a> {
    fn from_component(index: usize, component: &'a Component) -> Self {
        Self {
            index,
            path: format!("components[{index}]"),
            id: (!component.id.is_empty()).then_some(component.id.as_str()),
            kind: (!component.component.is_empty()).then_some(component.component.as_str()),
            props: Some(&component.props),
            borrowed: Some(component),
            owned: None,
        }
    }

    fn from_json(index: usize, value: &'a Value) -> Self {
        let object = value.as_object();
        let id = object
            .and_then(|o| o.get("id"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        let kind = object
            .and_then(|o| o.get("component"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        // Rebuild a typed component so catalog reference extraction works on
        // raw LLM output too.
        let owned = match (id, kind) {
            (Some(id), Some(kind)) => {
                let mut props = object.cloned().unwrap_or_default();
                props.remove("id");
                props.remove("component");
                Some(Component {
                    id: id.to_string(),
                    component: kind.to_string(),
                    props,
                })
            }
            _ => None,
        };
        Self {
            index,
            path: format!("components[{index}]"),
            id,
            kind,
            props: object,
            borrowed: None,
            owned,
        }
    }

    /// Locates the component somewhere other than `components[index]`.
    fn at(mut self, path: String) -> Self {
        self.path = path;
        self
    }

    fn component(&self) -> Option<&Component> {
        self.borrowed.or(self.owned.as_ref())
    }

    fn locator(&self, suffix: &str) -> String {
        if suffix.is_empty() {
            self.path.clone()
        } else {
            format!("{}.{suffix}", self.path)
        }
    }
}

impl<'a> Validator<'a> {
    /// A validator holding a payload to the full-surface contract.
    pub fn new(catalog: &'a Catalog) -> Self {
        Self::with_options(catalog, ValidateOptions::full_surface())
    }

    /// A validator for an incremental `updateComponents` payload.
    pub fn incremental(catalog: &'a Catalog) -> Self {
        Self::with_options(catalog, ValidateOptions::incremental_update())
    }

    /// A validator with explicit options.
    pub fn with_options(catalog: &'a Catalog, options: ValidateOptions) -> Self {
        Self {
            catalog: Some(catalog),
            registry: BTreeMap::new(),
            version: None,
            options,
        }
    }

    /// A validator over several catalogs, read by id.
    ///
    /// A surface is held to the catalog its `createSurface` named. From v1.0 a
    /// component may name another with `@catalogId`, and a surface may name
    /// none, each of its components then naming its own. A catalog that is
    /// not among `catalogs`, and a component with none to fall back on, is an
    /// [`ErrorCode::UnknownCatalog`]; so is a catalog written against another
    /// wire format than the surface.
    ///
    /// ```
    /// use ag_ui_a2ui::{Catalog, ValidateOptions, Validator};
    /// use ag_ui_a2ui::validate::ErrorCode;
    /// use serde_json::json;
    ///
    /// let basic = Catalog::basic_for(ag_ui_a2ui::A2uiVersion::V1_0);
    /// let charts = Catalog::from_schema(&json!({
    ///     "catalogId": "charts",
    ///     "protocolVersion": "1.0",
    ///     "components": {"LineChart": {"type": "object", "properties": {"title": {"type": "string"}}}}
    /// }))?;
    /// let validator = Validator::over([&basic, &charts], ValidateOptions::full_surface());
    ///
    /// let stream = |chart_catalog: &str| vec![
    ///     json!({"version": "1.0", "createSurface": {"surfaceId": "s", "catalogId": basic.catalog_id}}),
    ///     json!({"version": "1.0", "updateComponents": {"surfaceId": "s", "components": [
    ///         {"id": "root", "component": "Column", "children": ["chart"]},
    ///         {"id": "chart", "component": "LineChart", "@catalogId": chart_catalog, "title": "Sales"}
    ///     ]}}),
    /// ];
    /// assert!(validator.validate_json_messages(&stream("charts")).is_valid());
    /// let report = validator.validate_json_messages(&stream("maps"));
    /// assert_eq!(report.errors[0].code, ErrorCode::UnknownCatalog);
    /// # Ok::<(), ag_ui_a2ui::Error>(())
    /// ```
    pub fn over(catalogs: impl IntoIterator<Item = &'a Catalog>, options: ValidateOptions) -> Self {
        Self {
            catalog: None,
            registry: catalogs
                .into_iter()
                .map(|catalog| (catalog.catalog_id.as_str(), catalog))
                .collect(),
            version: None,
            options,
        }
    }

    /// Reads components that are on no surface in the spelling of `version`.
    ///
    /// That is what [`Validator::validate`], [`Validator::validate_surface`],
    /// [`Validator::validate_model`] and [`Validator::validate_json`] are
    /// given: a list of components and no message to say what they are written
    /// in. Without this they are read in the version the catalog is written
    /// against ([`Catalog::version`]). A stream of messages is always read as
    /// each surface was created.
    #[must_use]
    pub fn speaking(mut self, version: A2uiVersion) -> Self {
        self.version = Some(version);
        self
    }

    /// What components that are on no surface are read against.
    fn bare(&self) -> Drawing<'a, 'static> {
        Drawing {
            version: self
                .version
                .or(self.catalog.map(Catalog::version))
                .unwrap_or(A2uiVersion::DEFAULT),
            default: self.catalog,
            surface: None,
        }
    }

    /// What the components of `surface` are read against.
    fn drawing<'s>(&self, surface: &'s SurfaceState) -> Drawing<'a, 's> {
        Drawing {
            version: surface.spoken_version(),
            default: self.catalog.or_else(|| {
                let named = surface.catalog_id.as_deref()?;
                self.registry.get(named).copied()
            }),
            surface: Some(surface),
        }
    }

    /// A catalog by id. With one catalog, that catalog under its own id or
    /// under the one the surface gave it.
    fn named(&self, drawing: &Drawing<'a, '_>, catalog_id: &str) -> Option<&'a Catalog> {
        match self.catalog {
            Some(only) => {
                let surface_names_it = drawing
                    .surface
                    .is_some_and(|surface| surface.catalog_id.as_deref() == Some(catalog_id));
                (only.catalog_id == catalog_id || surface_names_it).then_some(only)
            }
            None => self.registry.get(catalog_id).copied(),
        }
    }

    /// The bare `catalogId` of a component, where it is the key the drafts
    /// before v1.0 was final named a catalog with, and not a property: the
    /// component's type declares no property of that name in the surface's
    /// own catalog, which is where a component that names none is read from.
    ///
    /// The bare name and the value under it.
    fn bare_catalog_key<'n>(
        &self,
        drawing: &Drawing<'a, '_>,
        node: &Node<'n>,
    ) -> Option<(&'static str, &'n Value)> {
        let bare = drawing
            .version
            .component_keys()
            .catalog_id?
            .trim_start_matches('@');
        let value = node.props?.get(bare)?;
        let declared = drawing
            .default
            .and_then(|catalog| catalog.component(node.kind?))
            .is_some_and(|def| def.declares(bare));
        (!declared).then_some((bare, value))
    }

    /// Which catalog `node` is drawn from.
    fn source(&self, drawing: &Drawing<'a, '_>, node: &Node<'_>) -> Source<'a> {
        let named = drawing.version.component_keys().catalog_id.and_then(|key| {
            if let Some(Value::String(id)) = node.props?.get(key) {
                return Some((id.as_str(), key));
            }
            match self.bare_catalog_key(drawing, node) {
                Some((_, Value::String(id))) => Some((id.as_str(), key)),
                _ => None,
            }
        });
        let found = |catalog: &'a Catalog, key: &'static str| {
            // One catalog stands for whatever a surface names. Several are
            // told apart, and each has to be written for the surface's format.
            if self.catalog.is_none() && !drawing.version.accepts(catalog.version()) {
                Source::OtherFormat(catalog, key)
            } else {
                Source::Catalog(catalog)
            }
        };
        match named {
            Some((id, key)) => match self.named(drawing, id) {
                Some(catalog) => found(catalog, key),
                None => Source::Unknown(id.to_string(), key),
            },
            None => match (
                drawing.default,
                drawing.surface.and_then(|s| s.catalog_id.as_deref()),
            ) {
                (Some(catalog), _) => found(catalog, ""),
                (None, Some(id)) => Source::Unknown(id.to_string(), ""),
                (None, None) if self.registry.is_empty() => Source::Unnamed,
                (None, None) => Source::Unnamed,
            },
        }
    }

    /// Validates typed components, with no data model to bind against.
    pub fn validate(&self, components: &[Component]) -> ValidationReport {
        self.validate_surface(components, None)
    }

    /// Validates typed components against a surface data model.
    pub fn validate_surface(
        &self,
        components: &[Component],
        data_model: Option<&Value>,
    ) -> ValidationReport {
        let nodes: Vec<Node<'_>> = components
            .iter()
            .enumerate()
            .map(|(i, c)| Node::from_component(i, c))
            .collect();
        let model = data_model.cloned().map(DataModel::from);
        self.run(&self.bare(), &nodes, model.as_ref(), "components")
    }

    /// Validates graph and bindings against a lossless model, including every
    /// defined item of a template collection and undefined array slots.
    pub fn validate_model(&self, components: &[Component], model: &DataModel) -> ValidationReport {
        let nodes: Vec<_> = components
            .iter()
            .enumerate()
            .map(|(i, component)| Node::from_component(i, component))
            .collect();
        self.run(&self.bare(), &nodes, Some(model), "components")
    }

    /// Validates raw JSON components, as they arrive from a model.
    ///
    /// Unlike the typed entry points this can report [`ErrorCode::MissingId`]
    /// and [`ErrorCode::MissingComponentType`], because raw objects are free to
    /// omit them.
    pub fn validate_json(
        &self,
        components: &[Value],
        data_model: Option<&Value>,
    ) -> ValidationReport {
        let nodes: Vec<Node<'_>> = components
            .iter()
            .enumerate()
            .map(|(i, v)| Node::from_json(i, v))
            .collect();
        let model = data_model.cloned().map(DataModel::from);
        self.run(&self.bare(), &nodes, model.as_ref(), "components")
    }

    /// Validates a whole operation stream.
    ///
    /// Replays messages separately per surface, then validates complete trees.
    /// Updates require creation within this stream. Use `validate_updates`
    /// with explicit prior state for a partial stream.
    pub fn validate_messages(&self, messages: &[AgentMessage]) -> ValidationReport {
        match messages
            .iter()
            .map(serde_json::to_value)
            .collect::<std::result::Result<Vec<_>, _>>()
        {
            Ok(raw) => self.validate_json_messages(&raw),
            Err(error) => ValidationReport {
                errors: vec![ValidationError::new(
                    ErrorCode::InvalidValue,
                    "messages",
                    error.to_string(),
                )],
                ..Default::default()
            },
        }
    }

    /// Validates raw protocol messages, as they arrive on the wire.
    ///
    /// The same folding as [`Validator::validate_messages`], plus the checks
    /// that only make sense on the raw JSON: the message envelope, how deeply
    /// the message nests, and how long a chain of function calls it carries.
    /// None of the three survives deserialization into typed messages, because
    /// all three are properties of the document rather than of any one
    /// component.
    ///
    /// Each defect is reported once. Components are checked on the folded
    /// surface that holds them; a message the store cannot fold — its surface
    /// was never created, or a component in it has no usable id or type — is
    /// checked on its own instead. Either way the locator names the message
    /// and position that wrote the component, e.g.
    /// `messages[2].updateComponents.components[0].child`, so it is unambiguous
    /// across messages and surfaces.
    pub fn validate_json_messages(&self, messages: &[Value]) -> ValidationReport {
        self.validate_json_updates(&SurfaceStore::new(), messages).0
    }

    /// Validates raw protocol messages against previously observed renderer
    /// state: [`Validator::validate_json_messages`] for a batch that continues
    /// a stream rather than beginning one.
    ///
    /// An update may target a surface `prior` already holds, and every live
    /// surface is held to the whole-surface contract once the batch is folded,
    /// whether or not the batch touched it. Locators name the messages of this
    /// batch, numbered from zero; a component `prior` already held is located
    /// by its position in its surface, `components[3]`.
    ///
    /// Returns the report together with the state the batch leaves: `prior`
    /// with every message that could be folded applied. That state is a
    /// candidate. Adopt it when the report is clean; a caller that wants a
    /// rejected batch to change nothing keeps `prior`.
    pub fn validate_json_updates(
        &self,
        prior: &SurfaceStore,
        messages: &[Value],
    ) -> (ValidationReport, SurfaceStore) {
        let mut store = prior.clone();
        // Whatever `prior` was built from, its messages are not these ones.
        store.begin_batch();
        let mut message_report = ValidationReport::default();
        for (index, message) in messages.iter().enumerate() {
            let locator = format!("messages[{index}]");
            if self.options.check_envelope {
                check_envelope(message, &locator, &mut message_report);
            }
            check_value_depth(
                message,
                &locator,
                // The enclosing array is depth 0, so a message sits at 1.
                1,
                &self.options,
                stated_version(message),
                &mut message_report,
            );
        }

        if !message_report.errors.is_empty() {
            return (message_report, store);
        }
        // Surfaces an update could not be folded into, so its components are
        // missing from the store.
        let mut incomplete: BTreeSet<String> = BTreeSet::new();
        for (index, message) in messages.iter().enumerate() {
            let carried = carried_components(message);
            let components = carried.map(|(_, components)| components);
            let folded = match carried {
                // The store keys components by id, so it cannot hold these, and
                // serde would only say the message failed to decode. The checks
                // below locate the component instead; the one thing left to
                // report here is a surface the message could not have updated.
                Some((Operation::UpdateComponents, components)) if !foldable(components) => {
                    let surface_id = message
                        .pointer("/updateComponents/surfaceId")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    match store.live(surface_id) {
                        Ok(_) => {
                            incomplete.insert(surface_id.to_string());
                        }
                        Err(error) => message_report.errors.push(ValidationError::new(
                            ErrorCode::InvalidValue,
                            format!("messages[{index}]"),
                            error.to_string(),
                        )),
                    }
                    false
                }
                // A creation that carries components the store cannot hold
                // creates nothing: what is wrong with them is reported below.
                Some((_, components)) if !foldable(components) => false,
                // The store numbers what it is given, and this loop skips
                // messages, so it says which one this is.
                _ => match serde_json::from_value::<AgentMessage>(message.clone())
                    .map_err(|error| error.to_string())
                    .and_then(|op| {
                        store
                            .apply_at(index, &op)
                            .map_err(|error| error.to_string())
                    }) {
                    Ok(()) => true,
                    Err(detail) => {
                        message_report.errors.push(ValidationError::new(
                            ErrorCode::InvalidValue,
                            format!("messages[{index}]"),
                            detail,
                        ));
                        false
                    }
                },
            };
            if let (false, Some(components)) = (folded, components) {
                let report = self.validate_unfolded(&store, index, message, components);
                message_report.errors.extend(report.errors);
            }
        }
        for surface in store.surfaces().filter(|surface| !surface.deleted) {
            let incomplete = incomplete.contains(&surface.surface_id);
            // A surface that lost a message is missing what it carried, and
            // that is already reported: held to the whole-surface contract it
            // would be reported again, as a missing root or children.
            if incomplete && surface.components.is_empty() {
                continue;
            }
            let options = if incomplete {
                ValidateOptions {
                    require_root: false,
                    allow_dangling_children: true,
                    ..self.options.clone()
                }
            } else {
                self.options.clone()
            };
            let report = self.with(options).validate_stored(&store, surface);
            message_report.errors.extend(report.errors);
            message_report.unreachable.extend(report.unreachable);
        }
        (message_report, store)
    }

    /// The same validator, with other options.
    fn with(&self, options: ValidateOptions) -> Self {
        Self {
            options,
            ..self.clone()
        }
    }

    /// Validates the components of a message that no surface will hold,
    /// because the message could not be folded: one of them has no usable id
    /// or type, or its surface is not live. They are checked alone, since
    /// their root and their children may be elsewhere, and are located in
    /// message `index`.
    ///
    /// `message` is an `updateComponents`, or a v1.0 `createSurface` that
    /// carries components. They are read as the surface `store` holds was
    /// created, or as the message is written when there is no such surface.
    pub(crate) fn validate_unfolded(
        &self,
        store: &SurfaceStore,
        index: usize,
        message: &Value,
        components: &[Value],
    ) -> ValidationReport {
        let options = ValidateOptions {
            require_root: false,
            allow_dangling_children: true,
            check_bindings: false,
            ..self.options.clone()
        };
        let operation = Operation::of(message).unwrap_or(Operation::UpdateComponents);
        let payload = message.get(operation.as_str());
        let surface = payload
            .and_then(|payload| payload.get("surfaceId"))
            .and_then(Value::as_str)
            .and_then(|surface_id| store.live(surface_id).ok());
        let drawing = match surface {
            Some(surface) => self.drawing(surface),
            None => {
                let named = payload
                    .and_then(|payload| payload.get("catalogId"))
                    .and_then(Value::as_str);
                Drawing {
                    version: stated_version(message),
                    default: self.catalog.or_else(|| self.registry.get(named?).copied()),
                    surface: None,
                }
            }
        };
        let list = format!("messages[{index}].{operation}.components");
        let nodes: Vec<Node<'_>> = components
            .iter()
            .enumerate()
            .map(|(i, v)| Node::from_json(i, v).at(format!("{list}[{i}]")))
            .collect();
        self.with(options).run(&drawing, &nodes, None, &list)
    }

    /// Validates updates against explicit previously observed renderer state.
    /// The caller receives a candidate store only when every message succeeds.
    ///
    /// A failure names the message that has to change: a component one of
    /// `messages` wrote is located as
    /// `messages[1].updateComponents.components[0]`, the same way
    /// [`Validator::validate_json_messages`] locates it. A component carried
    /// over from `prior` has no message here, and is located by its position
    /// in the surface, `components[3]`.
    pub fn validate_updates(
        &self,
        prior: &SurfaceStore,
        messages: &[AgentMessage],
    ) -> Result<SurfaceStore> {
        let mut next = prior.clone();
        // Whatever `prior` was built from, its messages are not these ones.
        next.begin_batch();
        for (index, message) in messages.iter().enumerate() {
            let raw = serde_json::to_value(message)?;
            let mut report = ValidationReport::default();
            check_envelope(&raw, &format!("messages[{index}]"), &mut report);
            report.into_result()?;
            next.apply_at(index, message)?;
        }
        for surface in next.surfaces().filter(|s| !s.deleted) {
            self.validate_stored(&next, surface).into_result()?;
        }
        Ok(next)
    }

    /// Validates one surface a store holds, graph and bindings both, locating
    /// each component in the message that last wrote it.
    ///
    /// The store knows which of the messages applied to it wrote what
    /// ([`SurfaceStore`]); a component none of them wrote was already there, and
    /// keeps the locator its position in the surface gives it.
    pub(crate) fn validate_stored(
        &self,
        store: &SurfaceStore,
        surface: &SurfaceState,
    ) -> ValidationReport {
        let list = store
            .origin(&surface.surface_id)
            .and_then(Origin::list)
            .unwrap_or_else(|| "components".to_string());
        self.run(
            &self.drawing(surface),
            &stored_nodes(store, surface),
            Some(&surface.data_model),
            &list,
        )
    }

    /// Where the components of a stored surface sit against their catalogs'
    /// `allowedParents` and `allowedChildren`, whatever version the surface
    /// speaks.
    ///
    /// [`Validator::validate_stored`] reports this itself for a version that
    /// [constrains composition](A2uiVersion::constrains_composition). This is
    /// for a caller that holds a surface of an earlier version to the
    /// constraints its catalog states all the same, as the agent layer does,
    /// and wants them reported the same way.
    #[cfg(feature = "agent")]
    pub(crate) fn composition_stored(
        &self,
        store: &SurfaceStore,
        surface: &SurfaceState,
    ) -> Vec<ValidationError> {
        let nodes = stored_nodes(store, surface);
        let drawing = self.drawing(surface);
        let sources: Vec<Source<'a>> = nodes
            .iter()
            .map(|node| self.source(&drawing, node))
            .collect();
        let mut ids: BTreeMap<&str, usize> = BTreeMap::new();
        for node in &nodes {
            if let Some(id) = node.id {
                ids.entry(id).or_insert(node.index);
            }
        }
        let mut report = ValidationReport::default();
        self.check_composition(&nodes, &sources, &ids, &mut report);
        report.errors
    }

    /// Every check, over `nodes`. `list` locates the component list itself, for
    /// the failures that belong to no one component.
    fn run(
        &self,
        drawing: &Drawing<'a, '_>,
        nodes: &[Node<'_>],
        data_model: Option<&DataModel>,
        list: &str,
    ) -> ValidationReport {
        let mut report = ValidationReport::default();

        if nodes.is_empty() {
            report.errors.push(ValidationError::new(
                ErrorCode::EmptyComponents,
                list,
                "The components list is empty. A surface needs at least a component with \
                 id 'root'.",
            ));
            return report;
        }

        let sources: Vec<Source<'a>> = nodes
            .iter()
            .map(|node| self.source(drawing, node))
            .collect();
        let ids = self.check_identity(drawing, nodes, &mut report);
        self.check_types_and_props(drawing, nodes, &sources, &mut report);
        self.check_component_keys(drawing, nodes, &sources, &mut report);
        self.check_function_calls(drawing, nodes, &mut report);
        self.check_component_depth(drawing, nodes, &mut report);

        if self.options.require_root && !ids.contains_key(self.options.root_id.as_str()) {
            report.errors.push(ValidationError::new(
                ErrorCode::NoRoot,
                list,
                format!(
                    "No component has id '{}'. Exactly one component must use that id; it is \
                     the root the renderer draws from.",
                    self.options.root_id
                ),
            ));
        }

        let graph = self.build_graph(nodes, &sources, &ids, &mut report);
        let root = ids.get(self.options.root_id.as_str()).copied();
        self.check_cycles(nodes, &graph, root, &mut report);
        self.check_index_scope(drawing, nodes, &graph, &mut report);
        // Where a component may sit is asked of a tree that is otherwise
        // sound, and of a caller that holds components to their catalogs.
        if drawing.version.constrains_composition()
            && self.options.check_component_types
            && report.errors.is_empty()
        {
            self.check_composition(nodes, &sources, &ids, &mut report);
        }

        // A second pass of the same walk, from the root alone and with no
        // depth limit: what the root can reach, not what a renderer will draw.
        if let Some(root) = root {
            let reachable = graph.walk([root], None).reached;
            for node in nodes {
                if let Some(id) = node.id {
                    if !reachable.contains(&node.index) {
                        report.unreachable.push(id.to_string());
                    }
                }
            }
        }

        if self.options.check_bindings || self.options.check_binding_syntax {
            self.check_bindings(drawing, nodes, &graph, data_model, &mut report);
        }
        report
    }

    /// The composition constraints: what each component's catalog says of the
    /// parents it may sit under (`allowedParents`) and the children it may
    /// hold (`allowedChildren`).
    ///
    /// The root's parent is the surface itself, `Surface`, and only the
    /// root's: a component nothing refers to has no parent to be checked
    /// against. A component with several parents is checked against each. A
    /// component that may not sit where it is is located as that component; a
    /// child that may not be held is located as the reference to it.
    fn check_composition(
        &self,
        nodes: &[Node<'_>],
        sources: &[Source<'a>],
        ids: &BTreeMap<&str, usize>,
        report: &mut ValidationReport,
    ) {
        let definition = |index: usize| {
            sources[index]
                .catalog()
                .and_then(|catalog| catalog.component(nodes[index].kind?))
        };
        let listed = |names: &[String]| {
            let quoted: Vec<String> = names.iter().map(|name| format!("'{name}'")).collect();
            format!("[{}]", quoted.join(", "))
        };
        let named = |index: usize| {
            (
                nodes[index].id.unwrap_or_default(),
                nodes[index].kind.unwrap_or_default(),
            )
        };

        if let Some(&root) = ids.get(self.options.root_id.as_str()) {
            let allowed = definition(root).and_then(|def| def.allowed_parents.as_ref());
            if let Some(allowed) = allowed.filter(|allowed| {
                !allowed
                    .iter()
                    .any(|parent| parent == crate::constants::SURFACE_COMPONENT)
            }) {
                let (id, kind) = named(root);
                report.errors.push(ValidationError::new(
                    ErrorCode::UnallowedParent,
                    nodes[root].locator(""),
                    format!(
                        "Component '{id}' ({kind}) cannot be placed under parent 'Surface' \
                         (Surface). Allowed parents: {}.",
                        listed(allowed)
                    ),
                ));
            }
        }
        for node in nodes {
            let (Some(parent_def), Some(parent)) = (definition(node.index), node.component())
            else {
                continue;
            };
            for reference in parent_def.references(parent) {
                let Some(&child_index) = ids.get(reference.id.as_str()) else {
                    continue;
                };
                let (child_id, child_kind) = named(child_index);
                if let Some(allowed) = parent_def
                    .allowed_children
                    .as_ref()
                    .filter(|allowed| !allowed.iter().any(|kind| kind == child_kind))
                {
                    report.errors.push(ValidationError::new(
                        ErrorCode::UnallowedChild,
                        node.locator(&reference.location),
                        format!(
                            "Container '{}' ({}) cannot contain child '{child_id}' \
                             ({child_kind}). Allowed children: {}.",
                            parent.id,
                            parent.component,
                            listed(allowed)
                        ),
                    ));
                }
                let allowed = definition(child_index).and_then(|def| def.allowed_parents.as_ref());
                if let Some(allowed) =
                    allowed.filter(|allowed| !allowed.contains(&parent.component))
                {
                    report.errors.push(ValidationError::new(
                        ErrorCode::UnallowedParent,
                        nodes[child_index].locator(""),
                        format!(
                            "Component '{child_id}' ({child_kind}) cannot be placed under \
                             parent '{}' ({}). Allowed parents: {}.",
                            parent.id,
                            parent.component,
                            listed(allowed)
                        ),
                    ));
                }
            }
        }
    }

    /// Ids, types and duplicates. Returns id → node index for resolved ids.
    fn check_identity<'n>(
        &self,
        drawing: &Drawing<'a, '_>,
        nodes: &'n [Node<'n>],
        report: &mut ValidationReport,
    ) -> BTreeMap<&'n str, usize> {
        let mut ids: BTreeMap<&str, usize> = BTreeMap::new();
        for node in nodes {
            let Some(id) = node.id else {
                report.errors.push(ValidationError::new(
                    ErrorCode::MissingId,
                    node.locator("id"),
                    "Every component needs a non-empty string 'id'; other components reference \
                     it by that id.",
                ));
                continue;
            };
            if drawing.version.names_are_identifiers() && !is_identifier(id) {
                report.errors.push(ValidationError::new(
                    ErrorCode::InvalidValue,
                    node.locator("id"),
                    format!(
                        "Component id '{id}' must be a valid identifier in A2UI {}: a letter or \
                         '_' first, then letters, digits and '_', as in 'user_card'. No '-', no \
                         spaces, no leading digit.",
                        drawing.version
                    ),
                ));
            }
            if let Some(&first) = ids.get(id) {
                report.errors.push(ValidationError::new(
                    ErrorCode::DuplicateId,
                    node.locator("id"),
                    format!(
                        "Component id '{id}' is already used by {}. Ids must be unique within a \
                         surface; rename this one.",
                        nodes[first].path
                    ),
                ));
                continue;
            }
            ids.insert(id, node.index);
        }
        ids
    }

    fn check_types_and_props(
        &self,
        drawing: &Drawing<'a, '_>,
        nodes: &[Node<'_>],
        sources: &[Source<'a>],
        report: &mut ValidationReport,
    ) {
        for (node, source) in nodes.iter().zip(sources) {
            let Some(kind) = node.kind else {
                report.errors.push(ValidationError::new(
                    ErrorCode::MissingComponentType,
                    node.locator("component"),
                    "Every component needs a 'component' field naming its type, e.g. \
                     \"component\": \"Text\".",
                ));
                continue;
            };
            if drawing.version.names_are_identifiers() && !is_identifier(kind) {
                report.errors.push(ValidationError::new(
                    ErrorCode::InvalidValue,
                    node.locator("component"),
                    format!(
                        "Component type '{kind}' must be a valid identifier in A2UI {}: a \
                         letter or '_' first, then letters, digits and '_'.",
                        drawing.version
                    ),
                ));
                continue;
            }
            let catalog = match source {
                Source::Catalog(catalog) => *catalog,
                Source::Unnamed => {
                    report.errors.push(ValidationError::new(
                        ErrorCode::UnknownCatalog,
                        node.locator("component"),
                        format!(
                            "Component '{}' names no catalog, and its surface has no default \
                             catalogId. Give the component '@catalogId', or create the surface \
                             with a catalogId.",
                            node.id.unwrap_or(kind)
                        ),
                    ));
                    continue;
                }
                Source::Unknown(catalog_id, key) => {
                    let place = if key.is_empty() { "component" } else { key };
                    report.errors.push(ValidationError::new(
                        ErrorCode::UnknownCatalog,
                        node.locator(place),
                        match drawing.surface {
                            Some(surface) => format!(
                                "Catalog '{catalog_id}' is not supported by surface '{}'.",
                                surface.surface_id
                            ),
                            None => format!(
                                "Catalog '{catalog_id}' is not one this surface can draw from."
                            ),
                        },
                    ));
                    continue;
                }
                Source::OtherFormat(catalog, key) => {
                    let place = if key.is_empty() { "component" } else { key };
                    report.errors.push(ValidationError::new(
                        ErrorCode::UnknownCatalog,
                        node.locator(place),
                        format!(
                            "Protocol version mismatch: cannot mix catalog '{}' ({}) with \
                             surface version {}.",
                            catalog.catalog_id,
                            catalog.version(),
                            drawing.version
                        ),
                    ));
                    continue;
                }
            };
            if catalog.components.is_empty() {
                continue;
            }
            let Some(def) = catalog.component(kind) else {
                // Whether an unfamiliar type is itself an error is the caller's
                // choice; either way there is no definition to check against, so
                // this component is done.
                if self.options.check_component_types {
                    report.errors.push(ValidationError::new(
                        ErrorCode::UnknownComponent,
                        node.locator("component"),
                        format!(
                            "Component type '{kind}' is not in catalog '{}'. Use one of: {}.",
                            catalog.catalog_id,
                            catalog
                                .components_in_order()
                                .map(|d| d.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    ));
                }
                continue;
            };
            if self.options.check_required_props {
                for required in &def.required {
                    let present = node.props.is_some_and(|props| props.contains_key(required));
                    if !present {
                        report.errors.push(ValidationError::new(
                            ErrorCode::MissingRequiredProp,
                            node.locator(required),
                            format!("'{kind}' requires the property '{required}'."),
                        ));
                    }
                }
            }
            check_structure(node, kind, def, report);
            if self.options.check_prop_types {
                // The key that sent the component to this catalog is not one
                // of its properties, whatever this catalog declares by that
                // name.
                let routed_by = self.bare_catalog_key(drawing, node).map(|(bare, _)| bare);
                self.check_prop_types(drawing.version, node, kind, def, routed_by, report);
            }
        }
    }

    /// The keys of a component that are its envelope and not its properties,
    /// as the surface's version has them.
    ///
    /// Before v1.0 the envelope keys of v1.0 mean nothing, and the reference
    /// renderers refuse a component that carries one unless its catalog
    /// declares a property by that name: a model that has seen v1.0 writes
    /// them. From v1.0 `@` is reserved, here and inside every value; the
    /// bare names of the drafts are still read where the component declares
    /// no property of that name, and may not stand beside the `@` key.
    ///
    /// Always checked: what these keys are decides what the rest of the
    /// component is.
    fn check_component_keys(
        &self,
        drawing: &Drawing<'a, '_>,
        nodes: &[Node<'_>],
        sources: &[Source<'a>],
        report: &mut ValidationReport,
    ) {
        let version = drawing.version;
        let keys = version.component_keys();
        for (node, source) in nodes.iter().zip(sources) {
            let Some(props) = node.props else { continue };
            let def = node.kind.and_then(|kind| source.catalog()?.component(kind));
            let declares = |name: &str| def.is_some_and(|def| def.declares(name));

            if !version.reserves_at_keys() {
                for key in version.foreign_component_keys() {
                    if props.contains_key(key) && !declares(key) {
                        report.errors.push(ValidationError::new(
                            ErrorCode::InvalidValue,
                            node.locator(key),
                            format!(
                                "'{key}' is a component key of a later A2UI version than this \
                                 surface's {version}; the component declares no property by that \
                                 name."
                            ),
                        ));
                    }
                }
                continue;
            }

            for key in props.keys() {
                if literal_key(version, key).is_none() && !keys.contains(key) {
                    report.errors.push(reserved_key(node.locator(key), key));
                }
            }
            // A catalog id that is not a string names nothing, and must not
            // pass for no id at all: the component would be read against the
            // surface's catalog instead of the one it meant.
            if let Some(key) = keys.catalog_id {
                let named = props
                    .get(key)
                    .map(|value| (key, value))
                    .or_else(|| self.bare_catalog_key(drawing, node));
                if let Some((written, _)) = named.filter(|(_, value)| !value.is_string()) {
                    report.errors.push(ValidationError::new(
                        ErrorCode::InvalidValue,
                        node.locator(written),
                        format!("'{written}' names a catalog by its catalogId, which is a string."),
                    ));
                }
            }
            // The envelope under its `@` key, or under the bare one the
            // drafts used when the component does not declare that name.
            let envelope = |key: &'static str| {
                let bare = key.trim_start_matches('@');
                // The catalog's key is read before the catalog is known, so
                // what declares it is the surface's own catalog.
                let legacy = if Some(key) == keys.catalog_id {
                    self.bare_catalog_key(drawing, node).map(|(_, value)| value)
                } else {
                    props.get(bare).filter(|_| !declares(bare))
                };
                (props.get(key), legacy, bare)
            };
            for key in keys.all() {
                let (current, legacy, bare) = envelope(key);
                if current.is_some() && legacy.is_some() {
                    report.errors.push(ValidationError::new(
                        ErrorCode::InvalidValue,
                        node.locator(bare),
                        format!(
                            "'{bare}' and '{key}' are the same component key, and this \
                             component has both. Keep '{key}'."
                        ),
                    ));
                }
            }
            let (checks, legacy_checks, _) = envelope(keys.checks);
            if (checks.is_some() || legacy_checks.is_some())
                && def.is_some_and(|def| !def.is_checkable())
            {
                report.errors.push(ValidationError::new(
                    ErrorCode::InvalidValue,
                    node.locator(keys.checks),
                    format!(
                        "'{}' takes no check rules: '{}' belongs on an input component.",
                        node.kind.unwrap_or_default(),
                        keys.checks
                    ),
                ));
            }
            if let Some(metadata_key) = keys.metadata {
                let (current, legacy, bare) = envelope(metadata_key);
                for (key, metadata) in [(metadata_key, current), (bare, legacy)] {
                    let Some(Value::Object(extensions)) =
                        metadata.and_then(|metadata| metadata.get("extensions"))
                    else {
                        continue;
                    };
                    for name in extensions.keys().filter(|name| !is_identifier(name)) {
                        report.errors.push(ValidationError::new(
                            ErrorCode::InvalidValue,
                            node.locator(&format!("{key}.extensions.{name}")),
                            format!(
                                "Extension name '{name}' must be a valid identifier: a letter \
                                 or '_' first, then letters, digits and '_'."
                            ),
                        ));
                    }
                }
            }
            for (key, value) in props {
                check_reserved_keys(version, value, &node.locator(key), report);
            }
        }
    }

    /// The function calls a component carries, from v1.0, which says how a
    /// call is resolved ([`A2uiVersion::resolves_function_calls`]).
    ///
    /// A call finds its catalog as a component does: the one it names with
    /// `catalogId`, or the surface's own. A call written inside a
    /// `formatString` template has nowhere to name one and is looked up where
    /// that `formatString` was. A catalog that is not held, one written for
    /// another wire format, and a call with no catalog at all are refused
    /// whatever the options say, since a renderer cannot run them.
    ///
    /// With [`ValidateOptions::check_component_types`] the name has to be a
    /// function of that catalog, or `@index`, the one system function there
    /// is. A function its catalog gives to the agent alone
    /// (`allowedCallers: agentOnly`) cannot stand in a property or an action,
    /// both of which the renderer evaluates. A `formatString` template has to
    /// parse.
    fn check_function_calls(
        &self,
        drawing: &Drawing<'a, '_>,
        nodes: &[Node<'_>],
        report: &mut ValidationReport,
    ) {
        let version = drawing.version;
        if !version.resolves_function_calls() {
            return;
        }
        // The surface's own catalog is named and not held: every component
        // says so already, and a call would only say it again.
        let default_is_missing = drawing.default.is_none()
            && drawing
                .surface
                .is_some_and(|surface| surface.catalog_id.is_some());
        for node in nodes {
            let Some(props) = node.props else { continue };
            let mut calls = Vec::new();
            let mut malformed = Vec::new();
            for (key, value) in props {
                function_calls(
                    version,
                    value,
                    &node.locator(key),
                    &mut calls,
                    &mut malformed,
                );
            }
            for (location, reason) in malformed {
                report.errors.push(ValidationError::new(
                    ErrorCode::InvalidValue,
                    location,
                    format!("The formatString template does not parse: {reason}."),
                ));
            }
            // The catalog each call resolved to, for the calls its template
            // wrote. `None` for a call that resolved to none.
            let mut resolved: Vec<Option<&'a Catalog>> = Vec::with_capacity(calls.len());
            for call in &calls {
                let name = call.name.as_ref();
                // A system function belongs to no catalog.
                if name.starts_with('@') {
                    resolved.push(None);
                    if name != "@index" && self.options.check_component_types {
                        report.errors.push(ValidationError::new(
                            ErrorCode::InvalidValue,
                            call.location.clone(),
                            format!(
                                "'{name}' is not a system function. A2UI {version} defines one, \
                                 '@index'; a function of a catalog has no '@'."
                            ),
                        ));
                    }
                    continue;
                }
                if !is_identifier(name) {
                    resolved.push(None);
                    report.errors.push(ValidationError::new(
                        ErrorCode::InvalidValue,
                        call.location.clone(),
                        format!(
                            "Function name '{name}' must be a valid UAX #31 identifier: a letter \
                             or '_' first, then letters, digits and '_'."
                        ),
                    ));
                    continue;
                }
                let catalog = match (call.catalog_id, call.written_in) {
                    (_, Some(outer)) => resolved[outer],
                    (Some(Value::String(named)), None) => match self.named(drawing, named) {
                        Some(catalog)
                            if self.catalog.is_some() || version.accepts(catalog.version()) =>
                        {
                            Some(catalog)
                        }
                        Some(catalog) => {
                            report.errors.push(ValidationError::new(
                                ErrorCode::UnknownCatalog,
                                format!("{}.catalogId", call.location),
                                format!(
                                    "Protocol version mismatch: cannot mix catalog '{named}' \
                                     ({}) with surface version {version}.",
                                    catalog.version()
                                ),
                            ));
                            None
                        }
                        None => {
                            report.errors.push(ValidationError::new(
                                ErrorCode::UnknownCatalog,
                                format!("{}.catalogId", call.location),
                                format!("Catalog not found: {named}"),
                            ));
                            None
                        }
                    },
                    (Some(_), None) => {
                        report.errors.push(ValidationError::new(
                            ErrorCode::InvalidValue,
                            format!("{}.catalogId", call.location),
                            "'catalogId' of a function call names a catalog by its id, which is \
                             a string."
                                .to_string(),
                        ));
                        None
                    }
                    (None, None) => {
                        if drawing.default.is_none() && !default_is_missing {
                            report.errors.push(ValidationError::new(
                                ErrorCode::UnknownCatalog,
                                call.location.clone(),
                                match drawing.surface {
                                    Some(surface) => format!(
                                        "Function call '{name}' names no catalog, and surface \
                                         '{}' has no default catalogId. Give the call a \
                                         'catalogId'.",
                                        surface.surface_id
                                    ),
                                    None => format!(
                                        "Function call '{name}' names no catalog, and there is \
                                         no default catalogId to fall back on."
                                    ),
                                },
                            ));
                        }
                        drawing.default
                    }
                };
                resolved.push(catalog);
                let Some(catalog) = catalog else { continue };
                match catalog.functions.get(name) {
                    Some(function) => {
                        if version.restricts_function_callers()
                            && !function.allowed_callers.includes_renderer()
                        {
                            report.errors.push(ValidationError::new(
                                ErrorCode::InvalidValue,
                                call.location.clone(),
                                format!(
                                    "Function '{name}' is agentOnly: only the agent runs it, and \
                                     a component property or an action is evaluated on the \
                                     renderer. Compute the value on the agent and send it as \
                                     data."
                                ),
                            ));
                        }
                        // What the declaration says of the arguments' names.
                        let none = Map::new();
                        let written = match (call.args, call.written_in) {
                            (Some(args), _) => args.as_object(),
                            (None, None) => Some(&none),
                            (None, Some(_)) => None,
                        };
                        let fault = written
                            .filter(|_| self.options.check_component_types)
                            .and_then(|args| function.argument_fault(args));
                        if let Some(fault) = fault {
                            report.errors.push(ValidationError::new(
                                ErrorCode::InvalidValue,
                                format!("{}.args", call.location),
                                fault,
                            ));
                        }
                    }
                    // A catalog with nothing in it is one the caller has not
                    // supplied; it defines no function to compare with.
                    None if self.options.check_component_types
                        && !(catalog.functions.is_empty() && catalog.components.is_empty()) =>
                    {
                        report.errors.push(ValidationError::new(
                            ErrorCode::InvalidValue,
                            call.location.clone(),
                            format!(
                                "Unrecognized function '{name}': catalog '{}' defines no such \
                                 function.",
                                catalog.catalog_id
                            ),
                        ));
                    }
                    None => {}
                }
            }
        }
    }

    /// A `callRendererFunction`, as a renderer checks one before it runs
    /// anything: the shape of the call, the surface it names, the catalog and
    /// the function, who may call it, and what its arguments hold.
    ///
    /// The call is the agent's, so the function has to be one an agent may
    /// call (`allowedCallers` `agentOnly` or `rendererOrAgent`) and one that
    /// needs no user gesture, which a call from the agent never has. The
    /// arguments are resolved by the renderer, at the root of the surface's
    /// data model and outside any list template: a binding is `{"@path": …}`
    /// and nothing else, a nested call is one the renderer may make, in the
    /// catalog it names, or the surface's own, or the catalog of the call
    /// when no surface is named, and `@index` has nothing to count. A call
    /// that leaves `args` out passes none, and is held to what its function
    /// requires as one that passes `{}` is.
    ///
    /// Gives how the call stands ([`CallCheck`]), which is what decides how a
    /// renderer answers it. The errors are in the order a renderer finds
    /// them: what is wrong with the call itself, then what it cannot resolve
    /// in the arguments, then the arguments against what the function takes.
    pub(crate) fn check_renderer_call(
        &self,
        store: &SurfaceStore,
        locator: &str,
        message: &Value,
        report: &mut ValidationReport,
    ) -> CallCheck {
        let key = Operation::CallRendererFunction.as_str();
        let at = format!("{locator}.{key}");
        let version = stated_version(message);
        let Some(payload) = message.get(key).and_then(Value::as_object) else {
            return CallCheck::Accepted;
        };
        let invalid = |path: String, reason: String| {
            ValidationError::new(ErrorCode::InvalidValue, path, reason)
        };
        // What makes the message no call at all: there is none to answer.
        let mut malformed: Vec<ValidationError> = Vec::new();
        for field in payload.keys() {
            if !["surfaceId", "functionCallId", "callFunction"].contains(&field.as_str()) {
                malformed.push(invalid(
                    format!("{at}.{field}"),
                    format!("'{field}' is not a field of '{key}'."),
                ));
            }
        }
        if payload.get("functionCallId").and_then(Value::as_str) == Some("") {
            malformed.push(invalid(
                format!("{at}.functionCallId"),
                "'functionCallId' must not be empty: the response copies it.".to_string(),
            ));
        }
        let call = payload.get("callFunction").and_then(Value::as_object);
        let call_at = format!("{at}.callFunction");
        if let Some(call) = call {
            call_fields(version, call, &call_at, &mut malformed);
        }
        let name = call
            .and_then(|call| call.get(version.call_key()))
            .and_then(Value::as_str);
        let catalog_id = call
            .and_then(|call| call.get("catalogId"))
            .and_then(Value::as_str);
        if malformed.is_empty() {
            if let Some(name) = name.filter(|name| name.starts_with('@') || !is_identifier(name)) {
                malformed.push(invalid(
                    format!("{call_at}.{}", version.call_key()),
                    format!(
                        "'{name}' cannot be called with '{key}': the function is one a catalog \
                         declares, named by an identifier with no '@'."
                    ),
                ));
            } else if let (Some(name), None) = (name, catalog_id) {
                malformed.push(invalid(
                    format!("{call_at}.catalogId"),
                    format!(
                        "'{key}' requires 'catalogId' on its 'callFunction': the call names the \
                         catalog that declares '{name}'."
                    ),
                ));
            }
        }
        if !malformed.is_empty() {
            report.errors.extend(malformed);
            return CallCheck::Malformed;
        }
        let (Some(call), Some(name), Some(catalog_id)) = (call, name, catalog_id) else {
            // The envelope check has said what is missing.
            return CallCheck::Accepted;
        };

        // The call itself: a renderer does not run one it refuses here.
        let surface = match payload.get("surfaceId").and_then(Value::as_str) {
            Some(surface_id) => match store.get(surface_id).filter(|surface| !surface.deleted) {
                Some(surface) => Some(surface),
                None => {
                    report.errors.push(invalid(
                        format!("{at}.surfaceId"),
                        format!(
                            "surface {surface_id:?} does not exist; a call names a live surface, \
                             or none."
                        ),
                    ));
                    return CallCheck::Refused;
                }
            },
            None => None,
        };
        let drawing = match surface {
            Some(surface) => self.drawing(surface),
            None => Drawing {
                version,
                default: None,
                surface: None,
            },
        };
        let Some(catalog) = self
            .named(&drawing, catalog_id)
            .filter(|catalog| version.accepts(catalog.version()))
        else {
            let reason = match self.named(&drawing, catalog_id) {
                Some(other) => format!(
                    "Protocol version mismatch: the protocol version of catalog '{catalog_id}' \
                     ({}) does not match message protocol version {version}.",
                    other.version()
                ),
                None => format!("Catalog not found: {catalog_id}"),
            };
            report
                .errors
                .push(invalid(format!("{call_at}.catalogId"), reason));
            return CallCheck::Refused;
        };
        let Some(function) = catalog.functions.get(name) else {
            report.errors.push(invalid(
                call_at.clone(),
                format!(
                    "Function not found: {name}. Catalog '{catalog_id}' defines no such function."
                ),
            ));
            return CallCheck::Refused;
        };
        let mut refused = false;
        if function.allowed_callers == AllowedCallers::RendererOnly {
            refused = true;
            report.errors.push(invalid(
                call_at.clone(),
                format!(
                    "Function '{name}' cannot be called by agent (allowedCallers is rendererOnly)."
                ),
            ));
        }
        if function.requires_user_activation {
            refused = true;
            report.errors.push(invalid(
                call_at.clone(),
                format!(
                    "Function '{name}' requires user activation context to execute, and a call \
                     from the agent has no user gesture."
                ),
            ));
        }

        // The arguments, as the renderer resolves them. A call that leaves
        // `args` out passes none.
        let none = Map::new();
        let args = call.get("args").and_then(Value::as_object).unwrap_or(&none);
        // A nested call that names no catalog runs in the surface's own, and
        // in the catalog of the call when no surface is named.
        let fallback = match surface {
            Some(_) => drawing.default,
            None => Some(catalog),
        };
        let mut unresolvable: Vec<ValidationError> = Vec::new();
        // Keys that are the protocol's and no directive, found on the way.
        let mut reserved: Vec<ValidationError> = Vec::new();
        let mut stack: Vec<(&Value, String, usize)> = args
            .iter()
            .rev()
            .map(|(name, value)| (value, format!("{call_at}.args.{name}"), 1))
            .collect();
        while let Some((value, location, depth)) = stack.pop() {
            if depth > super::binding::MAX_VALUE_DEPTH {
                continue;
            }
            let map = match value {
                Value::Array(items) => {
                    for (index, item) in items.iter().enumerate().rev() {
                        stack.push((item, format!("{location}[{index}]"), depth + 1));
                    }
                    continue;
                }
                Value::Object(map) => map,
                _ => continue,
            };
            // A binding is its path and nothing else.
            if let Some(path) = map.get(version.path_key()) {
                if !path.is_string() {
                    unresolvable.push(invalid(
                        format!("{location}.{}", version.path_key()),
                        format!(
                            "'{}' of a data binding must be a string, not {}.",
                            version.path_key(),
                            type_name(path)
                        ),
                    ));
                }
                for field in map.keys().filter(|field| *field != version.path_key()) {
                    unresolvable.push(invalid(
                        format!("{location}.{field}"),
                        format!(
                            "'{field}' is not a field of a data binding: a binding holds '{}' \
                             and nothing else.",
                            version.path_key()
                        ),
                    ));
                }
                continue;
            }
            let nested = map.get(version.call_key()).and_then(Value::as_str);
            let Some(nested) = nested else {
                // A plain object, whose keys are data unless they are the
                // protocol's.
                for (field, child) in map.iter().rev() {
                    let directive = field == version.call_key();
                    if !directive && literal_key(version, field).is_none() {
                        reserved.push(reserved_key(format!("{location}.{field}"), field));
                    }
                    stack.push((child, format!("{location}.{field}"), depth + 1));
                }
                continue;
            };
            if !call_fields(version, map, &location, &mut unresolvable) {
                continue;
            }
            // A nested call that leaves `args` out passes none, too.
            let inner = map.get("args").and_then(Value::as_object).unwrap_or(&none);
            for (field, child) in inner.iter().rev() {
                stack.push((child, format!("{location}.args.{field}"), depth + 1));
            }
            if nested.starts_with('@') {
                unresolvable.push(invalid(
                    location.clone(),
                    if nested == "@index" {
                        "The @index function can only be evaluated inside a collection template \
                         iteration scope, and the arguments of a function call are resolved \
                         outside any."
                            .to_string()
                    } else {
                        format!(
                            "'{nested}' is not a system function. A2UI {version} defines one, \
                             '@index'; a function of a catalog has no '@'."
                        )
                    },
                ));
                continue;
            }
            let resolved = match map.get("catalogId").and_then(Value::as_str) {
                Some(named) => match self
                    .named(&drawing, named)
                    .filter(|catalog| version.accepts(catalog.version()))
                {
                    Some(catalog) => Some(catalog),
                    None => {
                        unresolvable.push(invalid(
                            format!("{location}.catalogId"),
                            format!("Catalog not found: {named}"),
                        ));
                        None
                    }
                },
                None => {
                    if fallback.is_none() {
                        unresolvable.push(invalid(
                            location.clone(),
                            format!(
                                "Function call '{nested}' names no catalog, and the surface has \
                                 no default catalog. Give the call a 'catalogId'."
                            ),
                        ));
                    }
                    fallback
                }
            };
            let Some(resolved) = resolved else { continue };
            match resolved.functions.get(nested) {
                Some(function) => {
                    if !function.allowed_callers.includes_renderer() {
                        unresolvable.push(invalid(
                            location.clone(),
                            format!(
                                "Function '{nested}' is agentOnly: the renderer resolves the \
                                 arguments of a call, and may not call it."
                            ),
                        ));
                    }
                    if function.requires_user_activation {
                        unresolvable.push(invalid(
                            location.clone(),
                            format!(
                                "Function '{nested}' requires user activation, and a call from \
                                 the agent has no user gesture."
                            ),
                        ));
                    }
                    if let Some(fault) = function.argument_fault(inner) {
                        unresolvable.push(invalid(format!("{location}.args"), fault));
                    }
                }
                None => unresolvable.push(invalid(
                    location.clone(),
                    format!(
                        "Unrecognized function '{nested}': catalog '{}' defines no such \
                         function.",
                        resolved.catalog_id
                    ),
                )),
            }
        }
        unresolvable.extend(reserved);
        let unresolved = !unresolvable.is_empty();
        report.errors.extend(unresolvable);
        // Last, the arguments against what the function takes, which a
        // renderer checks once it has resolved them.
        let fault = function.argument_fault(args);
        let faulty = fault.is_some();
        if let Some(fault) = fault {
            report
                .errors
                .push(invalid(format!("{call_at}.args"), fault));
        }
        match (refused, unresolved, faulty) {
            (true, _, _) => CallCheck::Refused,
            (false, true, _) => CallCheck::Unresolvable,
            (false, false, true) => CallCheck::Refused,
            (false, false, false) => CallCheck::Accepted,
        }
    }

    /// `@index` where no list template gives it an item to count, from v1.0
    /// ([`A2uiVersion::resolves_function_calls`]).
    ///
    /// The walk is the one bindings are resolved on: from the root, or from
    /// every component nothing points at when the list is a fragment, noting
    /// which components a template draws. A component both inside and outside
    /// a template is outside one somewhere, and is reported.
    fn check_index_scope(
        &self,
        drawing: &Drawing<'a, '_>,
        nodes: &[Node<'_>],
        graph: &Graph,
        report: &mut ValidationReport,
    ) {
        let version = drawing.version;
        if !version.resolves_function_calls() || !self.options.check_binding_syntax {
            return;
        }
        // Where each component calls `@index`, if it does.
        let indexed: Vec<Vec<String>> = nodes
            .iter()
            .map(|node| {
                let mut calls = Vec::new();
                for (key, value) in node.props.into_iter().flatten() {
                    function_calls(
                        version,
                        value,
                        &node.locator(key),
                        &mut calls,
                        &mut Vec::new(),
                    );
                }
                calls
                    .into_iter()
                    .filter(|call| call.name == "@index")
                    .map(|call| call.location)
                    .collect()
            })
            .collect();
        if indexed.iter().all(Vec::is_empty)
            || report
                .errors
                .iter()
                .any(|error| error.code == ErrorCode::ChildCycle)
        {
            return;
        }
        let mut has_parent = vec![false; nodes.len()];
        for edge in graph.all_edges() {
            has_parent[edge.target] = true;
        }
        let is_root = |node: &Node<'_>| node.id == Some(self.options.root_id.as_str());
        let rooted = nodes.iter().any(is_root);
        let mut queue: Vec<(usize, bool)> = nodes
            .iter()
            .filter(|node| {
                if rooted {
                    is_root(node)
                } else {
                    !has_parent[node.index]
                }
            })
            .map(|node| (node.index, false))
            .collect();
        let mut visited = BTreeSet::new();
        while let Some((index, in_template)) = queue.pop() {
            if !visited.insert((index, in_template)) {
                continue;
            }
            if !in_template {
                for location in &indexed[index] {
                    report.errors.push(ValidationError::new(
                        ErrorCode::InvalidValue,
                        location.clone(),
                        "The @index function can only be evaluated inside a collection \
                         template iteration scope. Use it in a component a list template draws."
                            .to_string(),
                    ));
                }
            }
            let Some(component) = nodes[index].component() else {
                continue;
            };
            for edge in graph.edges(index) {
                let templated = template_path(component, &edge.location).is_some();
                queue.push((edge.target, in_template || templated));
            }
        }
    }

    /// Property values against the JSON types the catalog declares, and literal
    /// strings against the values an `enum` lists.
    ///
    /// Walks the *definition* rather than the value, so a property the catalog
    /// says nothing about costs nothing and is never rejected: an unknown
    /// property is the catalog's business (or the schema's), not this check's.
    /// Structural properties are left to [`check_structure`], which knows what
    /// shape a reference takes.
    fn check_prop_types(
        &self,
        version: A2uiVersion,
        node: &Node<'_>,
        kind: &str,
        def: &ComponentDef,
        routed_by: Option<&str>,
        report: &mut ValidationReport,
    ) {
        let Some(props) = node.props else { return };
        for prop in def.props.values() {
            if prop.kind.is_structural() || Some(prop.name.as_str()) == routed_by {
                continue;
            }
            let Some(value) = props.get(&prop.name) else {
                continue;
            };
            if resolves_at_render_time(version, value) {
                continue;
            }
            if !prop.value_type.accepts(value) {
                report.errors.push(ValidationError::new(
                    ErrorCode::TypeMismatch,
                    node.locator(&prop.name),
                    format!(
                        "'{kind}' expects '{}' to be {}, not {}. Write a literal of that type, \
                         or bind it with {{\"{}\": \"/...\"}}.",
                        prop.name,
                        prop.value_type.describe(),
                        type_name(value),
                        version.path_key()
                    ),
                ));
                continue;
            }
            // Only a literal string can be one of an enum's values; anything
            // else is a type question, settled above.
            if let Value::String(literal) = value {
                if !prop.enum_values.is_empty() && !prop.enum_values.contains(literal) {
                    report.errors.push(ValidationError::new(
                        ErrorCode::InvalidValue,
                        node.locator(&prop.name),
                        format!(
                            "'{kind}' does not accept '{literal}' for '{}'. Use one of: {}.",
                            prop.name,
                            prop.enum_values.join(", ")
                        ),
                    ));
                }
            }
        }
    }

    /// How deeply each component nests inside itself, and how long a chain of
    /// function calls it carries.
    ///
    /// Separate from the component *graph* depth checked in
    /// [`Validator::check_cycles`]: a component can be shallow in the tree and
    /// still carry a pathologically nested `action` or data binding.
    fn check_component_depth(
        &self,
        drawing: &Drawing<'a, '_>,
        nodes: &[Node<'_>],
        report: &mut ValidationReport,
    ) {
        for node in nodes {
            let Some(props) = node.props else { continue };
            for (key, value) in props {
                check_value_depth(
                    value,
                    &node.locator(key),
                    1,
                    &self.options,
                    drawing.version,
                    report,
                );
            }
        }
    }

    /// The child-reference graph, reporting references that do not resolve.
    /// Each component's references are read from the catalog it is drawn from.
    fn build_graph(
        &self,
        nodes: &[Node<'_>],
        sources: &[Source<'a>],
        ids: &BTreeMap<&str, usize>,
        report: &mut ValidationReport,
    ) -> Graph {
        Graph::build(
            |node, component| {
                sources[node]
                    .catalog()
                    .map(|catalog| catalog.references(component))
                    .unwrap_or_default()
            },
            nodes.iter().map(Node::component),
            ids,
            |node, component, reference| {
                if self.options.allow_dangling_children {
                    return;
                }
                report.errors.push(ValidationError::new(
                    ErrorCode::UnresolvedChild,
                    nodes[node].locator(&reference.location),
                    format!(
                        "Component '{}' references '{}', which is not defined in this \
                         payload. Add a component with that id, or point at one that exists.",
                        component.id, reference.id
                    ),
                ));
            },
        )
    }

    /// Reports each distinct cycle once, and a component graph nested past
    /// [`ValidateOptions::max_depth`].
    ///
    /// Depth is how far below the root a renderer draws a component, so the
    /// walk with the limit starts at the root, wherever the list puts it, as
    /// upstream's does (`validation_helpers.py`, `analyze_topology`). A list
    /// with no root is a fragment of an update: the limit is measured from each
    /// component nothing in it refers to. The walk then goes on from every
    /// other component, with no limit, so that a loop the root cannot reach is
    /// found too. It is one [`Graph::walk_from`], which enters each component
    /// once.
    fn check_cycles(
        &self,
        nodes: &[Node<'_>],
        graph: &Graph,
        root: Option<usize>,
        report: &mut ValidationReport,
    ) {
        let limit = Some(self.options.max_depth);
        let tops: Vec<usize> = match root {
            Some(root) => vec![root],
            None => {
                let mut has_parent = vec![false; graph.len()];
                for edge in graph.all_edges() {
                    has_parent[edge.target] = true;
                }
                (0..graph.len()).filter(|node| !has_parent[*node]).collect()
            }
        };
        let walk = graph.walk_from(
            tops.into_iter()
                .map(|top| (top, limit))
                .chain((0..graph.len()).map(|node| (node, None))),
        );
        for finding in walk.findings {
            report.errors.push(match finding {
                Finding::Cycle(cycle) => cycle_error(
                    &cycle,
                    |node| nodes[node].id.unwrap_or("<missing id>"),
                    nodes[cycle.from].locator(&cycle.edge.location),
                ),
                Finding::TooDeep { from, edge } => ValidationError::new(
                    ErrorCode::MaxDepthExceeded,
                    nodes[from].locator(&edge.location),
                    format!(
                        "Global recursion limit exceeded: logical depth > {}. The \
                         component tree nests deeper than a renderer will draw; \
                         flatten it.",
                        self.options.max_depth
                    ),
                ),
            });
        }
    }

    fn check_bindings(
        &self,
        drawing: &Drawing<'a, '_>,
        nodes: &[Node<'_>],
        graph: &Graph,
        model: Option<&DataModel>,
        report: &mut ValidationReport,
    ) {
        let bindings: Vec<_> = nodes
            .iter()
            .map(|node| {
                let Some(component) = node.component() else {
                    return Vec::new();
                };
                let raw = serde_json::to_value(component).expect("component JSON is infallible");
                collect_bindings_in(drawing.version, &raw)
                    .into_iter()
                    .filter(|binding| {
                        let absolute = if binding.path.starts_with('/') {
                            binding.path.clone()
                        } else {
                            format!("/{}", binding.path)
                        };
                        let valid = is_valid_pointer(&absolute);
                        if !valid && self.options.check_binding_syntax {
                            report.errors.push(ValidationError::new(
                                ErrorCode::UnresolvedBinding,
                                node.locator(&binding.location),
                                format!(
                                    "Invalid path syntax: '{}' is not a valid JSON Pointer. \
                                     Escape '~' as '~0' and '/' within a key as '~1'.",
                                    ShownPath(&binding.path)
                                ),
                            ));
                        }
                        valid
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        if !self.options.check_bindings
            || report
                .errors
                .iter()
                .any(|error| error.code == ErrorCode::ChildCycle)
        {
            return;
        }

        let mut has_parent = vec![false; nodes.len()];
        for edge in graph.all_edges() {
            has_parent[edge.target] = true;
        }
        let root_scope = model.map(ModelScope::root);
        // With a root in hand, the tree is what the root reaches, and that is
        // where bindings are resolved. Without one this is a fragment of an
        // update, and every component nothing in it points at is a way in.
        let is_root = |node: &Node<'_>| node.id == Some(self.options.root_id.as_str());
        let rooted = nodes.iter().any(is_root);
        let mut queue: Vec<_> = nodes
            .iter()
            .filter(|node| {
                if rooted {
                    is_root(node)
                } else {
                    !has_parent[node.index]
                }
            })
            .map(|node| (node.index, root_scope.clone(), false))
            .collect();
        // A component is checked once in each scope it is drawn in: the key
        // is the component, the item its scope is at, and whether it is
        // inside a template.
        let key = |index: usize, scope: &Option<ModelScope<'_>>, in_template: bool| {
            (
                index,
                scope.as_ref().map(|s| s.base().to_owned()),
                in_template,
            )
        };
        let mut visited = BTreeSet::new();
        // The collections a template has been expanded over, by the component
        // it draws. A template over an absolute path draws the same items
        // whichever scope it is reached from, so one inside another over the
        // same list is expanded once and not once per outer item: the walk is
        // linear in components times items.
        let mut expanded: BTreeSet<(usize, String)> = BTreeSet::new();
        while let Some((index, scope, in_template)) = queue.pop() {
            #[cfg(test)]
            BINDING_SCOPES_QUEUED.with(|queued| queued.set(queued.get() + 1));
            if !visited.insert(key(index, &scope, in_template)) {
                continue;
            }
            let node = &nodes[index];
            for binding in &bindings[index] {
                let absolute = binding.path.starts_with('/');
                // The empty path and `.` name the scope the binding is in,
                // which outside a template is the whole model: nothing about
                // them needs a template to mean something.
                if !absolute && !in_template && !names_the_scope(&binding.path) {
                    report.errors.push(ValidationError::new(
                        ErrorCode::UnresolvedBinding,
                        node.locator(&binding.location),
                        format!(
                            "Relative path '{}' is not inside a list template. \
                             Use an absolute path starting with '/'.",
                            ShownPath(&binding.path)
                        ),
                    ));
                    continue;
                }
                let resolver = if absolute {
                    root_scope.as_ref()
                } else {
                    scope.as_ref()
                };
                let Some(resolver) = resolver else { continue };
                let detail = match resolver.resolve(&binding.path) {
                    Ok(Some(ModelValue::Array(_))) => None,
                    Ok(Some(_)) if binding.is_collection => Some(format!(
                        "Template path '{}' must point at an array to iterate.",
                        ShownPath(&resolver.absolute(&binding.path))
                    )),
                    Ok(None) if self.options.require_bound_values => Some(format!(
                        "Path '{}' is missing or undefined in the data model.",
                        ShownPath(&resolver.absolute(&binding.path))
                    )),
                    Err(error) => Some(error.to_string()),
                    _ => None,
                };
                if let Some(detail) = detail {
                    report.errors.push(ValidationError::new(
                        ErrorCode::UnresolvedBinding,
                        node.locator(&binding.location),
                        detail,
                    ));
                }
            }
            let Some(component) = node.component() else {
                continue;
            };
            for edge in graph.edges(index) {
                if let Some(path) = template_path(component, &edge.location) {
                    let resolver = if path.starts_with('/') {
                        root_scope.as_ref()
                    } else {
                        scope.as_ref()
                    };
                    if let Some(resolver) = resolver {
                        let collection = resolver.absolute(&path);
                        if expanded.insert((edge.target, collection)) {
                            if let Ok(Some(ModelValue::Array(items))) = resolver.resolve(&path) {
                                for (i, item) in items.iter().enumerate() {
                                    if matches!(item, ModelValue::Undefined) {
                                        continue;
                                    }
                                    let item = Some(resolver.item(&path, i));
                                    if !visited.contains(&key(edge.target, &item, true)) {
                                        queue.push((edge.target, item, true));
                                    }
                                }
                            }
                        }
                    }
                    // Also check syntax and absolute bindings in empty or pending templates.
                    if !visited.contains(&key(edge.target, &None, true)) {
                        queue.push((edge.target, None, true));
                    }
                } else if !visited.contains(&key(edge.target, &scope, in_template)) {
                    queue.push((edge.target, scope.clone(), in_template));
                }
            }
        }
    }
}

/// Renders a loop the graph walk closed.
///
/// `name` gives a node's component id, and `locator` is the path to the
/// reference that closes the loop.
pub(crate) fn cycle_error<'a>(
    cycle: &Cycle,
    name: impl Fn(usize) -> &'a str,
    locator: String,
) -> ValidationError {
    let mut chain: Vec<&str> = cycle.nodes.iter().map(|node| name(*node)).collect();
    chain.push(name(cycle.edge.target));
    // The two lead-ins are the phrasing every A2UI SDK uses for these
    // conditions; keeping them identical means a renderer or an operator
    // reading logs from a mixed-language system sees one vocabulary.
    let detail = if cycle.nodes.len() == 1 {
        format!(
            "Self-reference detected: component '{}' references itself in '{}'.",
            name(cycle.from),
            cycle.edge.location
        )
    } else {
        format!(
            "Circular reference detected: child references form a loop: {}.",
            chain.join(" -> ")
        )
    };
    ValidationError::new(
        ErrorCode::ChildCycle,
        locator,
        format!(
            "{detail} A component tree must be acyclic; break the loop by pointing at a \
             different component."
        ),
    )
}

/// Whether a string is a syntactically valid RFC 6901 JSON Pointer.
fn is_valid_pointer(path: &str) -> bool {
    super::binding::pointer_is_valid(path)
}

/// Whether the renderer computes this value rather than reading it literally.
///
/// A binding and a function call, in the spelling of `version`, and a
/// `{"functionCall": …}` invocation, all carry something other than the
/// property's own value, so the type they have on the wire says nothing about
/// the type the renderer will see. `{componentId, path}` is excluded: that is
/// a child template, and its `path` names a collection rather than a value.
fn resolves_at_render_time(version: A2uiVersion, value: &Value) -> bool {
    let Some(map) = value.as_object() else {
        return false;
    };
    (map.contains_key(version.path_key()) && !map.contains_key("componentId"))
        || map.contains_key(version.call_key())
        || map.contains_key("functionCall")
}

/// Whether a name is an identifier as Unicode Standard Annex #31 defines one,
/// with `_` allowed first: `^[\p{XID_Start}_][\p{XID_Continue}]*$`.
pub(crate) fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || unicode_ident::is_xid_start(first))
        && chars.all(unicode_ident::is_xid_continue)
}

fn reserved_key(location: String, key: &str) -> ValidationError {
    ValidationError::new(
        ErrorCode::ReservedKey,
        location,
        format!(
            "'{key}' starts with '@', which is reserved for the protocol's own keys. Write a \
             literal key doubled, as '@{key}', or drop the '@'."
        ),
    )
}

/// Reports a key that starts with one `@` and is no directive, anywhere inside
/// a property's value. `@path` and `@call` are the directives a value may
/// hold.
fn check_reserved_keys(
    version: A2uiVersion,
    value: &Value,
    location: &str,
    report: &mut ValidationReport,
) {
    let mut stack: Vec<(&Value, String, usize)> = vec![(value, location.to_string(), 0)];
    while let Some((current, location, depth)) = stack.pop() {
        if depth > super::binding::MAX_VALUE_DEPTH {
            continue;
        }
        match current {
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    stack.push((item, format!("{location}[{index}]"), depth + 1));
                }
            }
            Value::Object(map) => {
                for (key, child) in map {
                    let directive = key == version.path_key() || key == version.call_key();
                    if !directive && literal_key(version, key).is_none() {
                        report
                            .errors
                            .push(reserved_key(format!("{location}.{key}"), key));
                    }
                    stack.push((child, format!("{location}.{key}"), depth + 1));
                }
            }
            _ => {}
        }
    }
}

/// How a `callRendererFunction` stands with a renderer that has checked it,
/// which decides how the renderer answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CallCheck {
    /// Nothing is wrong with it: a renderer goes on to run the function.
    Accepted,
    /// The message is not a call at all, and there is none to answer: it is
    /// reported, by its id when it has one.
    Malformed,
    /// A call the renderer does not run: the surface, the catalog, the
    /// function, who may call it, or the arguments it takes. Answered with
    /// `INVALID_FUNCTION_CALL`.
    Refused,
    /// A call the renderer may run, with an argument it cannot resolve: a
    /// binding that is no binding, a nested call it may not make, `@index`
    /// with nothing to count. Answered with `EXECUTION_ERROR`, which is what a
    /// renderer answers for any argument it fails to resolve.
    Unresolvable,
}

/// The fields of a function call as a message writes one: the name, a
/// `catalogId` that is a string, `args` that are an object, and nothing else.
/// Adds what is wrong to `errors`, and says whether the call can be read on.
fn call_fields(
    version: A2uiVersion,
    call: &serde_json::Map<String, Value>,
    location: &str,
    errors: &mut Vec<ValidationError>,
) -> bool {
    let name_key = version.call_key();
    let before = errors.len();
    let mut invalid = |path: String, reason: String| {
        errors.push(ValidationError::new(ErrorCode::InvalidValue, path, reason));
    };
    match call.get(name_key) {
        Some(Value::String(_)) => {}
        Some(other) => invalid(
            format!("{location}.{name_key}"),
            format!(
                "'{name_key}' names the function to call, which is a string, not {}.",
                type_name(other)
            ),
        ),
        None => invalid(
            format!("{location}.{name_key}"),
            format!("A function call requires '{name_key}', the name of the function."),
        ),
    }
    if let Some(named) = call.get("catalogId").filter(|named| !named.is_string()) {
        invalid(
            format!("{location}.catalogId"),
            format!(
                "'catalogId' of a function call names a catalog by its id, which is a string, \
                 not {}.",
                type_name(named)
            ),
        );
    }
    if let Some(args) = call.get("args").filter(|args| !args.is_object()) {
        invalid(
            format!("{location}.args"),
            format!(
                "'args' of a function call must be an object, not {}. Leave it out for a call \
                 without arguments.",
                type_name(args)
            ),
        );
    }
    for field in call.keys() {
        if field != name_key && field != "catalogId" && field != "args" {
            invalid(
                format!("{location}.{field}"),
                format!(
                    "'{field}' is not a field of a function call: a call holds '{name_key}', \
                     'catalogId' and 'args'."
                ),
            );
        }
    }
    errors.len() == before
}

/// One function call found in a component's property.
struct Call<'v> {
    /// Where it is, as a locator.
    location: String,
    /// The function it names.
    name: Cow<'v, str>,
    /// The `catalogId` written beside it.
    catalog_id: Option<&'v Value>,
    /// The `args` written beside it. None for a call a template wrote, whose
    /// arguments are not JSON.
    args: Option<&'v Value>,
    /// The `formatString` call whose template wrote this one, by its place in
    /// the list. Such a call has nowhere to name a catalog.
    written_in: Option<usize>,
}

/// Every function call inside `value`, a call before the ones in its
/// arguments, and each `formatString` template that does not parse with why.
fn function_calls<'v>(
    version: A2uiVersion,
    value: &'v Value,
    location: &str,
    calls: &mut Vec<Call<'v>>,
    malformed: &mut Vec<(String, String)>,
) {
    let mut stack: Vec<(&'v Value, String, usize)> = vec![(value, location.to_string(), 0)];
    while let Some((current, location, depth)) = stack.pop() {
        if depth > super::binding::MAX_VALUE_DEPTH {
            continue;
        }
        match current {
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate().rev() {
                    stack.push((item, format!("{location}[{index}]"), depth + 1));
                }
            }
            Value::Object(map) => {
                if let Some(Value::String(name)) = map.get(version.call_key()) {
                    let this = calls.len();
                    calls.push(Call {
                        location: location.clone(),
                        name: Cow::Borrowed(name),
                        catalog_id: map.get("catalogId"),
                        args: map.get("args"),
                        written_in: None,
                    });
                    let template = map
                        .get("args")
                        .and_then(|args| args.get("value"))
                        .and_then(Value::as_str)
                        .filter(|_| name == "formatString");
                    if let Some(template) = template {
                        let written = format!("{location}.args.value");
                        match parse_template(template) {
                            Ok(parts) => {
                                let mut inner: Vec<&TemplatePart> = parts.iter().rev().collect();
                                while let Some(part) = inner.pop() {
                                    if let TemplatePart::Call { name, args } = part {
                                        calls.push(Call {
                                            location: written.clone(),
                                            name: Cow::Owned(name.clone()),
                                            catalog_id: None,
                                            args: None,
                                            written_in: Some(this),
                                        });
                                        inner.extend(args.iter().rev().map(|(_, value)| value));
                                    }
                                }
                            }
                            Err(error) => {
                                let reason = match error {
                                    Error::Binding { reason, .. } => reason,
                                    other => other.to_string(),
                                };
                                malformed.push((written, reason));
                            }
                        }
                    }
                }
                for (key, child) in map.iter().rev() {
                    stack.push((child, format!("{location}.{key}"), depth + 1));
                }
            }
            _ => {}
        }
    }
}

/// Whether a raw message names a catalog other than `own`, the one of the
/// surface it is for: on a component, where `version` lets a component name
/// one, or on a function call anywhere in it.
#[cfg(feature = "schema-validation")]
pub(crate) fn names_another_catalog(
    version: A2uiVersion,
    message: &Value,
    own: Option<&str>,
) -> bool {
    let Some(component_key) = version.component_keys().catalog_id else {
        return false;
    };
    let other = |value: &Value| value.as_str().is_none_or(|named| Some(named) != own);
    let mut stack: Vec<(&Value, bool)> = Vec::new();
    for component in carried_components(message)
        .into_iter()
        .flat_map(|(_, components)| components)
    {
        stack.push((component, true));
    }
    while let Some((current, is_component)) = stack.pop() {
        match current {
            Value::Array(items) => stack.extend(items.iter().map(|item| (item, false))),
            Value::Object(map) => {
                // The bare key of the v1.0 drafts names a catalog too, unless
                // it is a property; a schema of another catalog cannot tell.
                let named = map
                    .get(component_key)
                    .or_else(|| map.get(component_key.trim_start_matches('@')));
                if is_component && named.is_some_and(other) {
                    return true;
                }
                if map.contains_key(version.call_key()) && map.get("catalogId").is_some_and(other) {
                    return true;
                }
                stack.extend(map.values().map(|value| (value, false)));
            }
            _ => {}
        }
    }
    false
}

/// The version a raw message states, or the default when it states none this
/// crate speaks: the message is still held to a contract.
pub(crate) fn stated_version(message: &Value) -> A2uiVersion {
    message
        .get("version")
        .and_then(Value::as_str)
        .and_then(A2uiVersion::parse)
        .unwrap_or(A2uiVersion::DEFAULT)
}

/// The components a raw message carries, and the operation that carries them:
/// an `updateComponents`, or a `createSurface` of a version that lets a
/// creation carry them.
pub(crate) fn carried_components(message: &Value) -> Option<(Operation, &Vec<Value>)> {
    let operation = Operation::of(message)?;
    let carries = match operation {
        Operation::UpdateComponents => true,
        Operation::CreateSurface => stated_version(message).creates_inline(),
        _ => false,
    };
    let components = message.get(operation.as_str())?.get("components")?;
    carries
        .then(|| components.as_array())
        .flatten()
        .map(|components| (operation, components))
}

/// Structural properties against the shape their [`PropKind`] gives them.
///
/// [`ComponentDef::references`] reads a component id wherever one can be and
/// skips everything else, so a child written inline would drop out of the graph
/// every later check walks: the payload would pass and the renderer would draw
/// nothing there. A2UI components are a flat list — the specification's catalog
/// says children "cannot be defined inline, they must be referred to by ID" — so
/// this reports each value that is not a reference instead.
///
/// Not governed by [`ValidateOptions::check_prop_types`]: whether a value is a
/// reference decides which children the tree has, and no check downstream is
/// meaningful without that.
fn check_structure(node: &Node<'_>, kind: &str, def: &ComponentDef, report: &mut ValidationReport) {
    let Some(props) = node.props else { return };
    for prop in def.props.values() {
        let Some(value) = props.get(&prop.name) else {
            continue;
        };
        let name = prop.name.as_str();
        match &prop.kind {
            PropKind::Value => {}
            PropKind::ComponentRef => {
                if !value.is_string() {
                    report.errors.push(not_a_reference(node, kind, name, value));
                }
            }
            PropKind::ChildList => match value {
                Value::Array(items) => {
                    for (index, item) in items.iter().enumerate() {
                        if !item.is_string() {
                            report.errors.push(not_a_reference(
                                node,
                                kind,
                                &format!("{name}[{index}]"),
                                item,
                            ));
                        }
                    }
                }
                Value::Object(template) if !is_inline_component(value) => {
                    if let Some(id) = template.get("componentId").filter(|id| !id.is_string()) {
                        report.errors.push(not_a_reference(
                            node,
                            kind,
                            &format!("{name}.componentId"),
                            id,
                        ));
                    } else if !template.contains_key("componentId")
                        || !template.get("path").is_some_and(Value::is_string)
                    {
                        report.errors.push(ValidationError::new(
                            ErrorCode::InvalidValue,
                            node.locator(name),
                            format!(
                                "A template in '{name}' of '{kind}' needs both \"componentId\", \
                                 the id of the component to repeat, and \"path\", the data-model \
                                 list to repeat it over."
                            ),
                        ));
                    }
                }
                _ => report.errors.push(ValidationError::new(
                    ErrorCode::TypeMismatch,
                    node.locator(name),
                    if is_inline_component(value) {
                        inline_component(kind, name)
                    } else {
                        format!(
                            "'{kind}' expects '{name}' to be an array of component ids or a \
                             template {{\"componentId\": ..., \"path\": ...}}, not {}.",
                            type_name(value)
                        )
                    },
                )),
            },
            PropKind::ObjectListRefs { ref_keys } => {
                let Value::Array(items) = value else {
                    report.errors.push(ValidationError::new(
                        ErrorCode::TypeMismatch,
                        node.locator(name),
                        format!(
                            "'{kind}' expects '{name}' to be an array of objects, not {}.",
                            type_name(value)
                        ),
                    ));
                    continue;
                };
                for (index, item) in items.iter().enumerate() {
                    let Value::Object(entry) = item else {
                        report.errors.push(ValidationError::new(
                            ErrorCode::TypeMismatch,
                            node.locator(&format!("{name}[{index}]")),
                            format!(
                                "'{kind}' expects each entry of '{name}' to be an object, not {}.",
                                type_name(item)
                            ),
                        ));
                        continue;
                    };
                    for key in ref_keys {
                        if let Some(id) = entry.get(key).filter(|id| !id.is_string()) {
                            report.errors.push(not_a_reference(
                                node,
                                kind,
                                &format!("{name}[{index}].{key}"),
                                id,
                            ));
                        }
                    }
                }
            }
        }
    }
}

/// A value where a component id belongs, located at `location`.
fn not_a_reference(node: &Node<'_>, kind: &str, location: &str, value: &Value) -> ValidationError {
    let message = if is_inline_component(value) {
        inline_component(kind, location)
    } else {
        format!(
            "'{kind}' expects '{location}' to be a component id (a string), not {}.",
            type_name(value)
        )
    };
    ValidationError::new(ErrorCode::TypeMismatch, node.locator(location), message)
}

fn inline_component(kind: &str, location: &str) -> String {
    format!(
        "'{kind}' names other components by id, but '{location}' holds a component written \
         inline. Components are a flat list: move this object to the top-level components list \
         and reference it here by its id."
    )
}

/// Whether a value is a component definition, which never nests.
fn is_inline_component(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|map| map.contains_key("component"))
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// Whether the store can fold these components: a non-empty list of objects,
/// each with a non-empty string id and type, and no id twice.
///
/// Exactly the cases the checks report as
/// [`ErrorCode::EmptyComponents`], [`ErrorCode::MissingId`],
/// [`ErrorCode::MissingComponentType`] and [`ErrorCode::DuplicateId`], so a
/// message this rejects always carries an error that says why.
pub(crate) fn foldable(components: &[Value]) -> bool {
    let named = |value: &Value, key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|name| !name.is_empty())
    };
    let mut ids = BTreeSet::new();
    !components.is_empty()
        && components.iter().all(|component| {
            named(component, "id")
                && named(component, "component")
                && ids.insert(component.get("id").and_then(Value::as_str))
        })
}

/// The collection path of a template edge, if this reference is one.
fn template_path(component: &Component, location: &str) -> Option<String> {
    let prop = location.strip_suffix(".componentId")?;
    component
        .props
        .get(prop)?
        .as_object()?
        .get("path")?
        .as_str()
        .map(str::to_string)
}

/// Checks one message against the envelope contract of the version it names.
///
/// Every other toolkit gets this from `server_to_client.json` through a JSON
/// Schema engine. Here the contract is a table the version carries
/// (`A2uiVersion::envelope`) rather than a document — and a table is what lets
/// a failure carry a locator into the message the caller sent, rather than a
/// path into a schema the caller never saw. The codes match the ones the
/// schema-driven toolkits report, because callers route on them.
///
/// This is the one envelope check: the validator and the
/// [`MessageProcessor`](crate::processor::MessageProcessor), and through it
/// the agent layer's reading of an answer, whole or streamed, all report what
/// it finds.
///
/// Only agent → renderer messages belong here: a renderer's reply is not agent
/// output and is not part of a payload this validator is asked about.
pub(crate) fn check_envelope(message: &Value, locator: &str, report: &mut ValidationReport) {
    let Some(map) = message.as_object() else {
        report.errors.push(ValidationError::new(
            ErrorCode::TypeMismatch,
            locator,
            format!("A message must be an object, not {}.", type_name(message)),
        ));
        return;
    };

    let declared = map.get("version");
    let version = declared
        .and_then(Value::as_str)
        .and_then(A2uiVersion::parse);
    match (declared, version) {
        (_, Some(_)) => {}
        (Some(declared), None) => report.errors.push(ValidationError::new(
            ErrorCode::InvalidValue,
            format!("{locator}.version"),
            format!(
                "This crate speaks A2UI {}, but the message declares {declared}. \
                 Every message in a stream carries the same version.",
                A2uiVersion::DEFAULT
            ),
        )),
        (None, None) => report.errors.push(ValidationError::new(
            ErrorCode::MissingField,
            format!("{locator}.version"),
            format!(
                "Every message needs \"version\": \"{}\".",
                A2uiVersion::DEFAULT
            ),
        )),
    }
    // A message with no usable version is still held to a contract, so one
    // retry can fix the version and the payload together.
    let version = version.unwrap_or(A2uiVersion::DEFAULT);

    if map.keys().filter(|key| key.as_str() != "version").count() != 1 {
        report.errors.push(ValidationError::new(
            ErrorCode::InvalidValue,
            locator,
            "Exactly one A2UI payload key is required.",
        ));
    }

    let operations = version.operations();
    let Some(operation) = operations
        .iter()
        .copied()
        .find(|operation| map.contains_key(operation.as_str()))
    else {
        let names: Vec<&str> = operations.iter().map(|op| op.as_str()).collect();
        report.errors.push(ValidationError::new(
            ErrorCode::MissingField,
            locator.to_string(),
            format!(
                "A message must carry one of {}. This one carries {:?}.",
                names.join(", "),
                map.keys().collect::<Vec<_>>()
            ),
        ));
        return;
    };
    let key = operation.as_str();
    let fields = version.envelope(operation);

    let Some(payload) = map[key].as_object() else {
        report.errors.push(ValidationError::new(
            ErrorCode::TypeMismatch,
            format!("{locator}.{key}"),
            format!("'{key}' must be an object, not {}.", type_name(&map[key])),
        ));
        return;
    };
    for (field, value_type, required) in fields {
        match payload.get(*field) {
            Some(value) if !value.is_null() => {
                if !value_type.accepts(value) {
                    report.errors.push(ValidationError::new(
                        ErrorCode::TypeMismatch,
                        format!("{locator}.{key}.{field}"),
                        format!(
                            "'{field}' of '{key}' must be {}, not {}.",
                            value_type.describe(),
                            type_name(value)
                        ),
                    ));
                }
            }
            Some(Value::Null) if !required => report.errors.push(ValidationError::new(
                ErrorCode::TypeMismatch,
                format!("{locator}.{key}.{field}"),
                format!(
                    "'{field}' of '{key}' must be {}, not null.",
                    value_type.describe()
                ),
            )),
            _ if *required => report.errors.push(ValidationError::new(
                ErrorCode::MissingField,
                format!("{locator}.{key}.{field}"),
                format!("'{key}' requires the field '{field}'."),
            )),
            _ => {}
        }
    }
    // A field of this operation in another version, which this one does not
    // have: a `theme` in v1.0, inline `components` before it. A reader drops
    // such a field as it drops any it does not know, and what the agent meant
    // by it would be lost without a word.
    for other in A2uiVersion::ALL {
        for (field, _, _) in other.envelope(operation) {
            let own = fields.iter().any(|(name, _, _)| name == field);
            if !own && payload.contains_key(*field) {
                let already = report
                    .errors
                    .iter()
                    .any(|error| error.path == format!("{locator}.{key}.{field}"));
                if !already {
                    report.errors.push(ValidationError::new(
                        ErrorCode::InvalidValue,
                        format!("{locator}.{key}.{field}"),
                        format!(
                            "'{field}' is not a field of '{key}' in A2UI {version}; it is one in \
                             {other}. Leave it out, or send the message as {other}."
                        ),
                    ));
                }
            }
        }
    }
    // Surface-level metadata, where a creation has it: an `extensions` map
    // and nothing else, each extension named by an identifier.
    if operation == Operation::CreateSurface {
        if let Some(Value::Object(metadata)) = payload.get("metadata") {
            for name in metadata.keys().filter(|name| *name != "extensions") {
                report.errors.push(ValidationError::new(
                    ErrorCode::InvalidValue,
                    format!("{locator}.{key}.metadata.{name}"),
                    format!("'metadata' of '{key}' holds 'extensions' and nothing else."),
                ));
            }
            match metadata.get("extensions") {
                None => {}
                Some(Value::Object(extensions)) => {
                    for name in extensions.keys().filter(|name| !is_identifier(name)) {
                        report.errors.push(ValidationError::new(
                            ErrorCode::InvalidValue,
                            format!("{locator}.{key}.metadata.extensions.{name}"),
                            format!(
                                "Extension name '{name}' must be a valid identifier: a letter \
                                 or '_' first, then letters, digits and '_'."
                            ),
                        ));
                    }
                }
                Some(other) => report.errors.push(ValidationError::new(
                    ErrorCode::TypeMismatch,
                    format!("{locator}.{key}.metadata.extensions"),
                    format!("'extensions' must be an object, not {}.", type_name(other)),
                )),
            }
        }
    }
    if operation == Operation::AgentFunctionResponse {
        let (value, error) = (payload.contains_key("value"), payload.get("error"));
        match (value, error) {
            (true, None) => {}
            (false, Some(Value::Object(error))) => {
                for field in ["code", "message"] {
                    if !error.get(field).is_some_and(Value::is_string) {
                        report.errors.push(ValidationError::new(
                            ErrorCode::MissingField,
                            format!("{locator}.{key}.error.{field}"),
                            format!("The 'error' of '{key}' requires '{field}', a string."),
                        ));
                    }
                }
            }
            (false, Some(_)) => {}
            (true, Some(_)) => report.errors.push(ValidationError::new(
                ErrorCode::InvalidValue,
                format!("{locator}.{key}"),
                format!("'{key}' carries 'value' or 'error', not both."),
            )),
            (false, None) => report.errors.push(ValidationError::new(
                ErrorCode::MissingField,
                format!("{locator}.{key}.value"),
                format!(
                    "'{key}' carries the 'value' the function returned, which may be null, or \
                     an 'error'."
                ),
            )),
        }
    }
    // The value of an update has no one type, so the table does not list it.
    // Where `null` is how a message removes, leaving it out says nothing.
    if operation == Operation::UpdateDataModel
        && version.null_removes()
        && !payload.contains_key("value")
    {
        report.errors.push(ValidationError::new(
            ErrorCode::MissingField,
            format!("{locator}.{key}.value"),
            format!(
                "'{key}' requires the field 'value' in A2UI {version}. To remove what is at \
                 the path, send \"value\": null."
            ),
        ));
    }
}

/// Reports JSON nesting and function-call chains that run past their limits.
///
/// Iterative with an explicit stack: the input is model-generated, and a
/// recursive walk over it is exactly the stack overflow this check exists to
/// prevent. `base_depth` is the depth `value` already sits at within its
/// enclosing document.
///
/// A `components` array is skipped, because components are checked one at a
/// time with locators that name the offending one; walking them here as well
/// would report the same nesting twice under a vaguer path.
///
/// At most one error of each kind is reported — a payload that is too deep is
/// too deep once, and listing every node past the limit would bury the point.
pub(crate) fn check_value_depth(
    value: &Value,
    path: &str,
    base_depth: usize,
    options: &ValidateOptions,
    version: A2uiVersion,
    report: &mut ValidationReport,
) {
    let (max_depth, max_function_call_depth) = (options.max_depth, options.max_function_call_depth);
    let mut reported_depth = false;
    let mut reported_calls = false;
    let mut stack: Vec<(&Value, usize, usize)> = vec![(value, base_depth, 0)];

    while let Some((current, depth, call_depth)) = stack.pop() {
        if depth > max_depth {
            if !reported_depth {
                reported_depth = true;
                report.errors.push(ValidationError::new(
                    ErrorCode::MaxDepthExceeded,
                    path,
                    format!(
                        "Global recursion limit exceeded: depth > {max_depth}. Flatten the \
                         structure; a renderer will not draw nesting this deep."
                    ),
                ));
            }
            // Stop descending: the message is already condemned, and walking
            // the rest costs time on input that is probably adversarial.
            continue;
        }

        match current {
            Value::Array(items) => {
                for item in items {
                    stack.push((item, depth + 1, call_depth));
                }
            }
            Value::Object(map) => {
                // Two spellings of a function call, and both nest: a
                // `{"functionCall": ...}` wrapper, and the `{call, args}` object
                // it wraps. Each costs one level, so a chain written with both
                // spends the budget twice as fast as the number suggests. That
                // is what every other toolkit counts, and a payload one of them
                // rejects must be rejected here too.
                let wrapper = map.get("functionCall").filter(|value| value.is_object());
                let is_call = map.contains_key(version.call_key()) && map.contains_key("args");

                if (wrapper.is_some() || is_call) && call_depth >= max_function_call_depth {
                    if !reported_calls {
                        reported_calls = true;
                        report.errors.push(ValidationError::new(
                            ErrorCode::MaxDepthExceeded,
                            path,
                            format!(
                                "Recursion limit exceeded: functionCall depth > \
                                 {max_function_call_depth}. Compute the value before sending it \
                                 rather than chaining more calls."
                            ),
                        ));
                    }
                    continue;
                }

                if let Some(wrapper) = wrapper {
                    stack.push((wrapper, depth + 1, call_depth + 1));
                    continue;
                }
                for (key, child) in map {
                    if key == "components" {
                        continue;
                    }
                    let next_call_depth = if is_call && key == "args" {
                        call_depth + 1
                    } else {
                        call_depth
                    };
                    stack.push((child, depth + 1, next_call_depth));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn basic() -> Catalog {
        Catalog::basic()
    }

    fn codes(report: &ValidationReport) -> Vec<ErrorCode> {
        report.errors.iter().map(|e| e.code).collect()
    }

    fn located(report: &ValidationReport) -> Vec<(ErrorCode, &str)> {
        report
            .errors
            .iter()
            .map(|e| (e.code, e.path.as_str()))
            .collect()
    }

    fn create(surface: &str) -> Value {
        json!({"version": "v0.9", "createSurface": {"surfaceId": surface, "catalogId": "c"}})
    }

    /// A list template inside another, both over the same absolute path: each
    /// item of the outer list draws the inner list over every item again. The
    /// inner list is the same list whichever item it is drawn for, so its
    /// items are walked once, and the walk is linear in the items.
    #[test]
    fn nested_templates_over_one_absolute_path_are_walked_once_per_item() {
        let catalog = basic();
        let validator = Validator::with_options(
            &catalog,
            ValidateOptions {
                require_bound_values: true,
                ..ValidateOptions::default()
            },
        );
        let components = vec![
            Component::new("root", "List").with(
                "children",
                json!({"componentId": "inner", "path": "/items"}),
            ),
            Component::new("inner", "List")
                .with("children", json!({"componentId": "leaf", "path": "/items"})),
            Component::new("leaf", "Text").with("text", json!({"path": "/title"})),
        ];
        for items in [100, 400] {
            let data = json!({"title": "t", "items": vec![0; items]});
            BINDING_SCOPES_QUEUED.with(|queued| queued.set(0));
            let report = validator.validate_surface(&components, Some(&data));
            assert!(report.is_valid(), "{report:?}");
            let queued = BINDING_SCOPES_QUEUED.with(std::cell::Cell::get);
            // `inner` and `leaf` once in each item's scope and once outside
            // any, and the root.
            assert!(queued <= 2 * items + 8, "{items} items: {queued} scopes");
        }
    }

    fn update(surface: &str, components: Value) -> Value {
        json!({
            "version": "v0.9",
            "updateComponents": {"surfaceId": surface, "components": components}
        })
    }

    #[test]
    fn a_well_formed_surface_validates_clean() {
        let catalog = basic();
        let components = vec![
            Component::new("root", "Column").with("children", json!(["title", "cta"])),
            Component::new("title", "Text").with("text", json!("Hello")),
            Component::new("cta", "Button")
                .with("child", json!("title"))
                .with("action", json!({"event": {"name": "go"}})),
        ];
        let report = Validator::new(&catalog).validate(&components);
        assert!(report.is_valid(), "{:?}", report.errors);
        assert!(report.unreachable.is_empty());
    }

    #[test]
    fn empty_components_is_reported_once() {
        let report = Validator::new(&basic()).validate(&[]);
        assert_eq!(codes(&report), vec![ErrorCode::EmptyComponents]);
        assert_eq!(report.errors[0].path, "components");
    }

    #[test]
    fn missing_id_and_type_come_from_raw_json() {
        let catalog = basic();
        let report = Validator::new(&catalog).validate_json(
            &[
                json!({"component": "Text", "text": "x"}),
                json!({"id": "b"}),
            ],
            None,
        );
        assert!(codes(&report).contains(&ErrorCode::MissingId));
        assert!(codes(&report).contains(&ErrorCode::MissingComponentType));
        assert_eq!(
            report
                .errors
                .iter()
                .find(|e| e.code == ErrorCode::MissingId)
                .unwrap()
                .path,
            "components[0].id"
        );
        assert_eq!(
            report
                .errors
                .iter()
                .find(|e| e.code == ErrorCode::MissingComponentType)
                .unwrap()
                .path,
            "components[1].component"
        );
    }

    #[test]
    fn duplicate_ids_point_at_the_later_component() {
        let catalog = basic();
        let components = vec![
            Component::new("root", "Text").with("text", json!("a")),
            Component::new("dup", "Text").with("text", json!("b")),
            Component::new("dup", "Text").with("text", json!("c")),
        ];
        let report = Validator::new(&catalog).validate(&components);
        let error = report
            .errors
            .iter()
            .find(|e| e.code == ErrorCode::DuplicateId)
            .unwrap();
        assert_eq!(error.path, "components[2].id");
        assert!(error.message.contains("components[1]"));
    }

    #[test]
    fn a_missing_root_is_reported_for_full_surfaces_only() {
        let catalog = basic();
        let components = vec![Component::new("c1", "Text").with("text", json!("hi"))];
        assert!(
            codes(&Validator::new(&catalog).validate(&components)).contains(&ErrorCode::NoRoot)
        );
        assert!(
            Validator::incremental(&catalog)
                .validate(&components)
                .is_valid()
        );
    }

    #[test]
    fn unknown_component_types_are_rejected_against_the_catalog() {
        let catalog = basic();
        let report = Validator::new(&catalog)
            .validate(&[Component::new("root", "Sparkline").with("data", json!([1, 2]))]);
        let error = report
            .errors
            .iter()
            .find(|e| e.code == ErrorCode::UnknownComponent)
            .unwrap();
        assert_eq!(error.path, "components[0].component");
        assert!(error.message.contains("Text"));
    }

    #[test]
    fn required_props_are_enforced_per_component_type() {
        let catalog = basic();
        let report = Validator::new(&catalog).validate(&[
            Component::new("root", "Column").with("children", json!(["t"])),
            Component::new("t", "Text"),
        ]);
        let error = report
            .errors
            .iter()
            .find(|e| e.code == ErrorCode::MissingRequiredProp)
            .unwrap();
        assert_eq!(error.path, "components[1].text");
    }

    #[test]
    fn unresolved_children_are_located_precisely() {
        let catalog = basic();
        let report = Validator::new(&catalog).validate(&[
            Component::new("root", "Row").with("children", json!(["there", "gone"])),
            Component::new("there", "Text").with("text", json!("x")),
        ]);
        let error = report
            .errors
            .iter()
            .find(|e| e.code == ErrorCode::UnresolvedChild)
            .unwrap();
        assert_eq!(error.path, "components[0].children[1]");
        assert!(error.message.contains("'gone'"));
    }

    #[test]
    fn template_references_are_resolved_like_any_other_child() {
        let catalog = basic();
        let ok = Validator::new(&catalog).validate(&[
            Component::new("root", "List")
                .with("children", json!({"componentId": "tpl", "path": "/items"})),
            Component::new("tpl", "Text").with("text", json!({"path": "label"})),
        ]);
        assert!(ok.is_valid(), "{:?}", ok.errors);

        let broken = Validator::new(&catalog).validate(&[Component::new("root", "List").with(
            "children",
            json!({"componentId": "missing", "path": "/items"}),
        )]);
        let error = broken
            .errors
            .iter()
            .find(|e| e.code == ErrorCode::UnresolvedChild)
            .unwrap();
        assert_eq!(error.path, "components[0].children.componentId");
    }

    #[test]
    fn dangling_children_are_allowed_for_incremental_updates() {
        let catalog = basic();
        let components = vec![Component::new("card", "Card").with("child", json!("elsewhere"))];
        assert!(!Validator::new(&catalog).validate(&components).is_valid());
        assert!(
            Validator::incremental(&catalog)
                .validate(&components)
                .is_valid()
        );
    }

    #[test]
    fn self_reference_is_a_cycle_even_in_incremental_updates() {
        let catalog = basic();
        let components = vec![Component::new("card", "Card").with("child", json!("card"))];
        let report = Validator::incremental(&catalog).validate(&components);
        let error = report
            .errors
            .iter()
            .find(|e| e.code == ErrorCode::ChildCycle)
            .unwrap();
        assert_eq!(error.path, "components[0].child");
        assert!(error.message.contains("Self-reference detected"));
    }

    #[test]
    fn two_node_cycles_are_reported_once_with_the_chain() {
        let catalog = basic();
        let components = vec![
            Component::new("root", "Card").with("child", json!("c1")),
            Component::new("c1", "Card").with("child", json!("root")),
        ];
        let report = Validator::new(&catalog).validate(&components);
        let cycles: Vec<_> = report
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::ChildCycle)
            .collect();
        assert_eq!(cycles.len(), 1, "{:?}", report.errors);
        assert!(cycles[0].message.contains("Circular reference detected"));
        assert!(cycles[0].message.contains("root -> c1 -> root"));
    }

    #[test]
    fn cycles_are_found_when_they_are_unreachable_from_the_root() {
        let catalog = basic();
        let components = vec![
            Component::new("root", "Text").with("text", json!("hi")),
            Component::new("a", "Card").with("child", json!("b")),
            Component::new("b", "Card").with("child", json!("a")),
        ];
        let report = Validator::new(&catalog).validate(&components);
        assert_eq!(
            report
                .errors
                .iter()
                .filter(|e| e.code == ErrorCode::ChildCycle)
                .count(),
            1
        );
        assert_eq!(report.unreachable, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn distinct_cycles_are_each_reported() {
        let catalog = basic();
        let components = vec![
            Component::new("root", "Row").with("children", json!(["a", "c"])),
            Component::new("a", "Card").with("child", json!("b")),
            Component::new("b", "Card").with("child", json!("a")),
            Component::new("c", "Card").with("child", json!("c")),
        ];
        let report = Validator::new(&catalog).validate(&components);
        assert_eq!(
            report
                .errors
                .iter()
                .filter(|e| e.code == ErrorCode::ChildCycle)
                .count(),
            2
        );
    }

    /// A chain of `Card`s `depth` links long, rooted at `root`.
    fn deep_chain(depth: usize) -> Vec<Component> {
        let mut components = Vec::with_capacity(depth + 2);
        components.push(Component::new("root", "Card").with("child", json!("n0")));
        for i in 0..depth {
            let next = if i + 1 == depth {
                json!("leaf")
            } else {
                json!(format!("n{}", i + 1))
            };
            components.push(Component::new(format!("n{i}"), "Card").with("child", next));
        }
        components.push(Component::new("leaf", "Text").with("text", json!("end")));
        components
    }

    /// A value nested `depth` objects deep.
    fn deep_value(depth: usize) -> Value {
        let mut value = json!({"level": depth});
        for level in (0..depth).rev() {
            value = json!({"level": level, "next": value});
        }
        value
    }

    #[test]
    fn a_tree_deeper_than_the_limit_is_reported_not_crashed() {
        let catalog = basic();
        // Far past the limit, and far past what any stack would survive if the
        // walk recursed.
        let report = Validator::new(&catalog).validate(&deep_chain(50_000));
        let depth_errors: Vec<_> = report
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::MaxDepthExceeded)
            .collect();
        assert_eq!(depth_errors.len(), 1, "reported once, not once per node");
        assert!(depth_errors[0].message.contains("logical depth > 50"));
        assert_eq!(depth_errors[0].path, "components[50].child");
    }

    #[test]
    fn the_depth_limit_is_policy_not_what_keeps_the_walk_safe() {
        // With the limit lifted, the same 50k-deep tree still validates: every
        // walk is iterative, so nothing here depends on the cap to survive.
        // This is the test that would blow the stack if a walk recursed.
        let catalog = basic();
        let options = ValidateOptions::full_surface().with_max_depth(usize::MAX);
        let report = Validator::with_options(&catalog, options).validate(&deep_chain(50_000));
        assert!(
            report.is_valid(),
            "{:?}",
            &report.errors[..report.errors.len().min(3)]
        );
    }

    #[test]
    fn json_from_the_wire_is_depth_bounded_before_this_crate_sees_it() {
        // The value walks in this crate recurse, and this is why that is safe:
        // anything arriving as text has already been through serde_json, which
        // refuses to build a `Value` nested deeper than 128. If that ever
        // changes, those walks need the same treatment as the graph walks.
        let ok = format!("{}{}", "[".repeat(127), "]".repeat(127));
        assert!(serde_json::from_str::<Value>(&ok).is_ok());

        let too_deep = format!("{}{}", "[".repeat(200), "]".repeat(200));
        let error = serde_json::from_str::<Value>(&too_deep).unwrap_err();
        assert!(
            error.to_string().contains("recursion limit exceeded"),
            "{error}"
        );
    }

    #[test]
    fn a_chain_within_the_limit_is_accepted() {
        let catalog = basic();
        // root -> n0..n47 -> leaf is 49 links, one inside the limit.
        let report = Validator::new(&catalog).validate(&deep_chain(48));
        assert!(report.is_valid(), "{:?}", report.errors);
    }

    #[test]
    fn deeply_nested_json_inside_a_component_is_reported() {
        let catalog = basic();
        let component = Component::new("root", "Text")
            .with("text", json!("hi"))
            .with("accessibility", deep_value(400));
        let report = Validator::new(&catalog).validate(&[component]);
        let error = report
            .errors
            .iter()
            .find(|e| e.code == ErrorCode::MaxDepthExceeded)
            .expect("a depth error");
        assert!(error.message.contains("depth > 50"));
        assert_eq!(error.path, "components[0].accessibility");
    }

    #[test]
    fn a_deeply_nested_data_model_is_reported_on_the_message() {
        let catalog = basic();
        let messages = vec![json!({
            "version": "v0.9",
            "updateDataModel": {"surfaceId": "s", "value": deep_value(400)}
        })];
        let report = Validator::new(&catalog).validate_json_messages(&messages);
        let error = report
            .errors
            .iter()
            .find(|e| e.code == ErrorCode::MaxDepthExceeded)
            .expect("a depth error");
        assert!(error.message.contains("Global recursion limit exceeded"));
        assert_eq!(error.path, "messages[0]");
    }

    #[test]
    fn a_chain_of_function_calls_past_the_limit_is_reported() {
        let catalog = basic();
        // Six nested calls, one past the budget of five.
        let mut call = json!({"call": "f5", "args": {}});
        for level in (0..5).rev() {
            call = json!({"call": format!("f{level}"), "args": {"functionCall": call}});
        }
        let component = Component::new("root", "Button")
            .with("child", json!("root"))
            .with("action", json!({"functionCall": call}));

        let report = Validator::with_options(
            &catalog,
            ValidateOptions {
                // Isolate the call-depth check from the cycle this component
                // has for brevity.
                ..ValidateOptions::incremental_update()
            },
        )
        .validate(&[component]);
        let error = report
            .errors
            .iter()
            .find(|e| e.code == ErrorCode::MaxDepthExceeded)
            .expect("a depth error");
        assert!(error.message.contains("functionCall depth > 5"), "{error}");
        assert_eq!(error.path, "components[0].action");
    }

    #[test]
    fn a_short_chain_of_function_calls_is_accepted() {
        let catalog = basic();
        // Two calls. The budget of five is spent two levels per call, because
        // the wrapper and the call object each count, so this is close to the
        // practical ceiling.
        let mut call = json!({"call": "f1", "args": {}});
        for level in (0..1).rev() {
            call = json!({"call": format!("f{level}"), "args": {"functionCall": call}});
        }
        let components = vec![
            Component::new("root", "Button")
                .with("child", json!("label"))
                .with("action", json!({"functionCall": call})),
            Component::new("label", "Text").with("text", json!("go")),
        ];
        let report = Validator::new(&catalog).validate(&components);
        assert!(report.is_valid(), "{:?}", report.errors);
    }

    #[test]
    fn unreachable_components_are_warnings_not_errors() {
        let catalog = basic();
        let report = Validator::new(&catalog).validate(&[
            Component::new("root", "Text").with("text", json!("root")),
            Component::new("orphan", "Text").with("text", json!("nobody points here")),
        ]);
        assert!(report.is_valid(), "{:?}", report.errors);
        assert_eq!(report.unreachable, vec!["orphan".to_string()]);
    }

    /// A component the root cannot reach has no place in the tree, so its
    /// bindings have no scope to be held to and no data anything reads.
    #[test]
    fn the_bindings_of_an_unreachable_component_are_not_resolved() {
        let catalog = basic();
        let components = [
            Component::new("root", "Text").with("text", json!("root")),
            // A row whose list was replaced: its relative path had a scope.
            Component::new("row", "Text").with("text", json!({"path": "name"})),
            // A list left behind, over data that is a map by now.
            Component::new("old", "List")
                .with("children", json!({"componentId": "row", "path": "/items"})),
        ];
        let data = json!({"items": {"not": "a list"}});
        let report = Validator::new(&catalog).validate_surface(&components, Some(&data));
        assert!(report.is_valid(), "{:?}", report.errors);
        assert_eq!(report.unreachable, ["row", "old"]);

        // Reached, the same two are held to the tree and to the data.
        let mut reached = components.to_vec();
        reached[0] = Component::new("root", "Column").with("children", json!(["old"]));
        let report = Validator::new(&catalog).validate_surface(&reached, Some(&data));
        assert_eq!(codes(&report), [ErrorCode::UnresolvedBinding]);
        assert!(report.errors[0].message.contains("must point at an array"));

        // A path that is no JSON Pointer is wrong wherever it is written.
        let mut broken = components.to_vec();
        broken[1] = Component::new("row", "Text").with("text", json!({"path": "/a~2b"}));
        let report = Validator::new(&catalog).validate_surface(&broken, Some(&data));
        assert_eq!(codes(&report), [ErrorCode::UnresolvedBinding]);

        // Without a root this is a fragment, and what nothing in it points at
        // is a way in, as before.
        let fragment = [Component::new("row", "Text").with("text", json!({"path": "name"}))];
        let report = Validator::with_options(&catalog, ValidateOptions::incremental_update())
            .validate_surface(&fragment, Some(&data));
        assert_eq!(codes(&report), [ErrorCode::UnresolvedBinding]);
    }

    #[test]
    fn relative_paths_outside_a_template_are_unresolved_bindings() {
        let catalog = basic();
        let report = Validator::new(&catalog)
            .validate(&[Component::new("root", "Text").with("text", json!({"path": "name"}))]);
        let error = report
            .errors
            .iter()
            .find(|e| e.code == ErrorCode::UnresolvedBinding)
            .unwrap();
        assert_eq!(error.path, "components[0].text");
        assert!(error.message.contains("not inside a list template"));
    }

    #[test]
    fn relative_paths_inside_a_template_are_accepted() {
        let catalog = basic();
        let report = Validator::new(&catalog).validate(&[
            Component::new("root", "List")
                .with("children", json!({"componentId": "row", "path": "/people"})),
            Component::new("row", "Text").with("text", json!({"path": "name"})),
        ]);
        assert!(report.is_valid(), "{:?}", report.errors);
    }

    #[test]
    fn bindings_are_resolved_against_a_supplied_data_model() {
        let catalog = basic();
        let components = vec![
            Component::new("root", "Column").with("children", json!(["a", "b"])),
            Component::new("a", "Text").with("text", json!({"path": "/user/name"})),
            Component::new("b", "Text").with("text", json!({"path": "/user/nope"})),
        ];
        let data = json!({"user": {"name": "Ada"}});
        let report = Validator::with_options(
            &catalog,
            ValidateOptions {
                require_bound_values: true,
                ..Default::default()
            },
        )
        .validate_surface(&components, Some(&data));
        let errors: Vec<_> = report
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::UnresolvedBinding)
            .collect();
        assert_eq!(errors.len(), 1, "{:?}", report.errors);
        assert_eq!(errors[0].path, "components[2].text");
    }

    #[test]
    fn template_paths_must_point_at_an_array() {
        let catalog = basic();
        let components = vec![
            Component::new("root", "List")
                .with("children", json!({"componentId": "row", "path": "/people"})),
            Component::new("row", "Text").with("text", json!({"path": "name"})),
        ];
        let data = json!({"people": {"not": "an array"}});
        let report = Validator::new(&catalog).validate_surface(&components, Some(&data));
        let error = report
            .errors
            .iter()
            .find(|e| e.code == ErrorCode::UnresolvedBinding)
            .unwrap();
        assert_eq!(error.path, "components[0].children");
        assert!(error.message.contains("must point at an array"));
    }

    #[test]
    fn relative_paths_resolve_against_collection_items() {
        let catalog = basic();
        let components = vec![
            Component::new("root", "List")
                .with("children", json!({"componentId": "row", "path": "/people"})),
            Component::new("row", "Column").with("children", json!(["name", "typo"])),
            Component::new("name", "Text").with("text", json!({"path": "name"})),
            Component::new("typo", "Text").with("text", json!({"path": "nmae"})),
        ];
        let data = json!({"people": [{"name": "Ada"}]});
        let report = Validator::with_options(
            &catalog,
            ValidateOptions {
                require_bound_values: true,
                ..Default::default()
            },
        )
        .validate_surface(&components, Some(&data));
        let errors: Vec<_> = report
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::UnresolvedBinding)
            .collect();
        assert_eq!(errors.len(), 1, "{:?}", report.errors);
        assert_eq!(errors[0].path, "components[3].text");
    }

    #[test]
    fn every_error_code_has_a_stable_wire_string() {
        let all = [
            (ErrorCode::EmptyComponents, "empty_components"),
            (ErrorCode::MissingId, "missing_id"),
            (ErrorCode::MissingComponentType, "missing_component_type"),
            (ErrorCode::DuplicateId, "duplicate_id"),
            (ErrorCode::NoRoot, "no_root"),
            (ErrorCode::UnknownComponent, "unknown_component"),
            (ErrorCode::MissingRequiredProp, "missing_required_prop"),
            (ErrorCode::MissingField, "missing_field"),
            (ErrorCode::InvalidValue, "invalid_value"),
            (ErrorCode::TypeMismatch, "type_mismatch"),
            (ErrorCode::UnresolvedChild, "unresolved_child"),
            (ErrorCode::ChildCycle, "child_cycle"),
            (ErrorCode::UnresolvedBinding, "unresolved_binding"),
            (ErrorCode::MaxDepthExceeded, "max_depth_exceeded"),
        ];
        for (code, wire) in all {
            assert_eq!(code.as_str(), wire);
            assert_eq!(serde_json::to_value(code).unwrap(), json!(wire));
        }
    }

    #[test]
    fn a_property_of_the_wrong_json_type_is_reported_against_the_catalog() {
        let catalog = basic();
        let report = Validator::new(&catalog).validate(&[
            Component::new("root", "Column").with("children", json!(["count"])),
            // The catalog says Slider.value is a number and Slider.label a
            // string; a model that swaps them produces a surface no renderer can
            // draw, and nothing before this caught it.
            Component::new("count", "Slider")
                .with("value", json!("seven"))
                .with("max", json!(10))
                .with("label", json!(3)),
        ]);
        let mismatches: Vec<&str> = report
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::TypeMismatch)
            .map(|e| e.path.as_str())
            .collect();
        assert_eq!(
            mismatches,
            vec!["components[1].label", "components[1].value"]
        );
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.message.contains("not a string")),
            "{:?}",
            report.errors
        );
    }

    #[test]
    fn a_value_the_renderer_computes_is_never_type_checked() {
        let catalog = basic();
        // Every one of these is legal in a string property: a binding, a
        // function call, and the wrapper spelling of a call. Their type on the
        // wire is an object, and rejecting them would break every real surface.
        let report = Validator::new(&catalog).validate(&[
            Component::new("root", "Column").with("children", json!(["a", "b", "c"])),
            Component::new("a", "Text").with("text", json!({"path": "/user/name"})),
            Component::new("b", "Text").with(
                "text",
                json!({"call": "formatString", "args": {"value": "hi"}}),
            ),
            Component::new("c", "Text").with("text", json!({"functionCall": {"call": "now"}})),
        ]);
        assert!(report.is_valid(), "{:?}", report.errors);
    }

    #[test]
    fn a_property_the_catalog_leaves_untyped_accepts_anything() {
        let catalog = basic();
        // `Icon.name` is a string, an `{svgPath}` object, or a binding, so the
        // catalog pins no type and this crate demands none.
        let report = Validator::new(&catalog)
            .validate(&[Component::new("root", "Icon").with("name", json!({"svgPath": "M0 0"}))]);
        assert!(report.is_valid(), "{:?}", report.errors);
    }

    #[test]
    fn type_checking_can_be_switched_off() {
        let catalog = basic();
        let options = ValidateOptions {
            check_prop_types: false,
            ..ValidateOptions::full_surface()
        };
        let report = Validator::with_options(&catalog, options)
            .validate(&[Component::new("root", "Text").with("text", json!(123))]);
        assert!(report.is_valid(), "{:?}", report.errors);
    }

    #[test]
    fn a_child_nested_inline_is_rejected_with_the_way_to_fix_it() {
        // Components are a flat list and a parent names its children by id, so
        // this Column has no child a renderer can find.
        let catalog = basic();
        let messages = vec![
            create("s"),
            update(
                "s",
                json!([{"id": "root", "component": "Column", "children": [
                    {"id": "t", "component": "Text", "text": "hi"}
                ]}]),
            ),
        ];
        let report = Validator::new(&catalog).validate_json_messages(&messages);
        assert_eq!(
            located(&report),
            vec![(
                ErrorCode::TypeMismatch,
                "messages[1].updateComponents.components[0].children[0]"
            )]
        );
        assert!(
            report.errors[0]
                .message
                .contains("move this object to the top-level components list"),
            "{}",
            report.errors[0]
        );
    }

    #[test]
    fn every_structural_property_holds_component_ids_and_nothing_else() {
        let catalog = basic();
        let check = |component: Component| {
            let report = Validator::incremental(&catalog).validate(&[component]);
            report
                .errors
                .into_iter()
                .map(|e| (e.code, e.path))
                .collect::<Vec<_>>()
        };
        let mismatch = |path: &str| vec![(ErrorCode::TypeMismatch, path.to_string())];
        let inline = json!({"id": "t", "component": "Text", "text": "hi"});

        // A single reference, a child list written as one component, and a
        // reference inside a list of objects.
        assert_eq!(
            check(Component::new("c", "Card").with("child", inline.clone())),
            mismatch("components[0].child")
        );
        assert_eq!(
            check(Component::new("l", "List").with("children", inline.clone())),
            mismatch("components[0].children")
        );
        assert_eq!(
            check(
                Component::new("t", "Tabs")
                    .with("tabs", json!([{"title": "One", "child": inline}]))
            ),
            mismatch("components[0].tabs[0].child")
        );
        // A component id is never data-bound, and a child list is never a
        // bare id.
        assert_eq!(
            check(Component::new("c", "Card").with("child", json!({"path": "/id"}))),
            mismatch("components[0].child")
        );
        assert_eq!(
            check(Component::new("r", "Row").with("children", json!("a"))),
            mismatch("components[0].children")
        );
        // A template needs the list it repeats over.
        assert_eq!(
            check(Component::new("r", "Row").with("children", json!({"componentId": "a"}))),
            vec![(
                ErrorCode::InvalidValue,
                "components[0].children".to_string()
            )]
        );
    }

    #[test]
    fn the_shape_of_a_reference_is_checked_even_with_type_checks_off() {
        // Type checking is the caller's choice; whether a value is a reference
        // decides which children the graph has, so it is not.
        let catalog = basic();
        let options = ValidateOptions {
            check_prop_types: false,
            ..ValidateOptions::incremental_update()
        };
        let report = Validator::with_options(&catalog, options).validate(&[
            Component::new("c", "Column")
                .with("children", json!([{"id": "t", "component": "Text"}])),
            Component::new("t", "Text")
                .with("text", json!("hi"))
                .with("variant", json!("huge")),
        ]);
        assert_eq!(
            located(&report),
            vec![(ErrorCode::TypeMismatch, "components[0].children[0]")]
        );
    }

    #[test]
    fn a_literal_outside_the_catalog_enum_is_rejected() {
        let catalog = basic();
        let report = Validator::new(&catalog).validate(&[
            Component::new("root", "Column")
                .with("children", json!(["title", "icon", "bound", "svg"]))
                .with("justify", json!("spaceBetween")),
            Component::new("title", "Text")
                .with("text", json!("Hi"))
                .with("variant", json!("huge")),
            Component::new("icon", "Icon").with("name", json!("sparkles")),
            // A binding and an `{svgPath}` are not literals, so no enum applies.
            Component::new("bound", "Text")
                .with("text", json!("x"))
                .with("variant", json!({"path": "/style"})),
            Component::new("svg", "Icon").with("name", json!({"svgPath": "M0 0"})),
        ]);
        assert_eq!(
            located(&report),
            vec![
                (ErrorCode::InvalidValue, "components[1].variant"),
                (ErrorCode::InvalidValue, "components[2].name"),
            ]
        );
        assert!(
            report.errors[0].message.contains("h1, h2"),
            "{}",
            report.errors[0]
        );
    }

    #[test]
    fn an_unknown_component_type_is_reported_once() {
        let catalog = basic();
        let messages = vec![
            create("s"),
            update("s", json!([{"id": "root", "component": "Sparkline"}])),
        ];
        let report = Validator::new(&catalog).validate_json_messages(&messages);
        assert_eq!(
            located(&report),
            vec![(
                ErrorCode::UnknownComponent,
                "messages[1].updateComponents.components[0].component"
            )]
        );
    }

    #[test]
    fn a_folded_defect_is_reported_once_where_it_was_last_written() {
        let catalog = basic();
        let messages = vec![
            create("a"),
            update(
                "a",
                json!([
                    {"id": "root", "component": "Column", "children": ["x", "y"]},
                    {"id": "x", "component": "Text", "text": "one"},
                    {"id": "y", "component": "Text", "text": "two"}
                ]),
            ),
            // Replaces `y`, which the folded surface keeps at position 2.
            update("a", json!([{"id": "y", "component": "Sparkline"}])),
            create("b"),
            update(
                "b",
                json!([{"id": "root", "component": "Text", "text": "hi", "variant": "huge"}]),
            ),
        ];
        let report = Validator::new(&catalog).validate_json_messages(&messages);
        assert_eq!(
            located(&report),
            vec![
                (
                    ErrorCode::UnknownComponent,
                    "messages[2].updateComponents.components[0].component"
                ),
                (
                    ErrorCode::InvalidValue,
                    "messages[4].updateComponents.components[0].variant"
                ),
            ]
        );
    }

    #[test]
    fn a_message_the_store_cannot_fold_reports_each_defect_once() {
        let catalog = basic();
        let validator = Validator::new(&catalog);

        // The surface was never created. That is one defect; the components
        // the message carried are checked on their own, since no folded
        // surface will hold them.
        let never_created = vec![update(
            "s",
            json!([
                {"id": "root", "component": "Card", "child": "root"},
                {"id": "x", "component": "Sparkline"}
            ]),
        )];
        let report = validator.validate_json_messages(&never_created);
        assert_eq!(
            located(&report),
            vec![
                (ErrorCode::InvalidValue, "messages[0]"),
                (
                    ErrorCode::UnknownComponent,
                    "messages[0].updateComponents.components[1].component"
                ),
                (
                    ErrorCode::ChildCycle,
                    "messages[0].updateComponents.components[0].child"
                ),
            ]
        );
        assert!(report.errors[0].message.contains("must be created"));

        // Components the store cannot key are reported from the raw JSON, not
        // again by serde or the store, nor as a surface left without them.
        let duplicated = vec![
            create("s"),
            update(
                "s",
                json!([
                    {"id": "root", "component": "Text", "text": "a"},
                    {"id": "root", "component": "Text", "text": "b"}
                ]),
            ),
        ];
        assert_eq!(
            located(&validator.validate_json_messages(&duplicated)),
            vec![(
                ErrorCode::DuplicateId,
                "messages[1].updateComponents.components[1].id"
            )]
        );

        // Both at once: the missing creation is still its own defect.
        let both = vec![update("s", json!([{"component": "Text", "text": "a"}]))];
        assert_eq!(
            located(&validator.validate_json_messages(&both)),
            vec![
                (ErrorCode::InvalidValue, "messages[0]"),
                (
                    ErrorCode::MissingId,
                    "messages[0].updateComponents.components[0].id"
                ),
            ]
        );
    }

    #[test]
    fn a_message_without_a_version_is_reported_as_a_missing_field() {
        let catalog = basic();
        let messages = vec![json!({"createSurface": {"surfaceId": "s", "catalogId": "c"}})];
        let report = Validator::new(&catalog).validate_json_messages(&messages);
        assert_eq!(codes(&report), vec![ErrorCode::MissingField]);
        assert_eq!(report.errors[0].path, "messages[0].version");
    }

    #[test]
    fn a_message_from_another_protocol_version_is_reported_as_an_invalid_value() {
        let catalog = basic();
        let messages = vec![json!({
            "version": "v0.8",
            "createSurface": {"surfaceId": "s", "catalogId": "c"}
        })];
        let report = Validator::new(&catalog).validate_json_messages(&messages);
        assert_eq!(codes(&report), vec![ErrorCode::InvalidValue]);
        assert_eq!(report.errors[0].path, "messages[0].version");
    }

    #[test]
    fn an_operation_is_held_to_its_own_required_fields_and_types() {
        let catalog = basic();
        let messages = vec![
            json!({"version": "v0.9", "createSurface": {"surfaceId": "s"}}),
            json!({"version": "v0.9", "deleteSurface": {"surfaceId": 123}}),
        ];
        let report = Validator::new(&catalog).validate_json_messages(&messages);
        let located: Vec<(ErrorCode, &str)> = report
            .errors
            .iter()
            .map(|e| (e.code, e.path.as_str()))
            .collect();
        assert_eq!(
            located,
            vec![
                (
                    ErrorCode::MissingField,
                    "messages[0].createSurface.catalogId"
                ),
                (
                    ErrorCode::TypeMismatch,
                    "messages[1].deleteSurface.surfaceId"
                ),
            ]
        );
    }

    #[test]
    fn a_message_carrying_no_operation_at_all_is_reported() {
        let catalog = basic();
        let messages = vec![json!({"version": "v0.9", "action": {"name": "go"}})];
        let report = Validator::new(&catalog).validate_json_messages(&messages);
        assert_eq!(codes(&report), vec![ErrorCode::MissingField]);
        assert_eq!(report.errors[0].path, "messages[0]");
    }

    #[test]
    fn disabling_extra_envelope_checks_does_not_bypass_typed_wire_contract() {
        let catalog = basic();
        let options = ValidateOptions {
            check_envelope: false,
            ..ValidateOptions::incremental_update()
        };
        let messages = vec![
            json!({"updateComponents": {"surfaceId": "s", "components": [
                {"id": "root", "component": "Text", "text": "hi"}
            ]}}),
        ];
        let report = Validator::with_options(&catalog, options).validate_json_messages(&messages);
        assert!(!report.is_valid());
    }

    #[test]
    fn every_message_this_crate_emits_satisfies_its_own_envelope_check() {
        let catalog = basic();
        let messages = vec![
            AgentMessage::create_surface("s", "cat"),
            AgentMessage::update_components(
                "s",
                vec![Component::new(ROOT_ID, "Text").with("text", json!("hi"))],
            ),
            AgentMessage::update_data_model("s", "/user", json!({"name": "Ada"})),
            AgentMessage::delete_surface("s"),
        ];
        let report = Validator::new(&catalog).validate_messages(&messages);
        assert!(report.is_valid(), "{:?}", report.errors);
    }

    #[test]
    fn incremental_streams_require_observed_prior_state() {
        let catalog = basic();
        let validator = Validator::new(&catalog);

        let incremental = vec![AgentMessage::update_components(
            "s",
            vec![Component::new("c", "Card").with("child", json!("already-there"))],
        )];
        assert!(!validator.validate_messages(&incremental).is_valid());

        let full = vec![
            AgentMessage::create_surface("s", "cat"),
            AgentMessage::update_components(
                "s",
                vec![Component::new("c", "Card").with("child", json!("gone"))],
            ),
        ];
        let report = validator.validate_messages(&full);
        assert!(codes(&report).contains(&ErrorCode::NoRoot));
        assert!(codes(&report).contains(&ErrorCode::UnresolvedChild));
    }

    #[test]
    fn validate_messages_replays_the_data_model() {
        let catalog = basic();
        let validator = Validator::with_options(
            &catalog,
            ValidateOptions {
                require_bound_values: true,
                ..Default::default()
            },
        );
        let messages = vec![
            AgentMessage::create_surface("s", "cat"),
            AgentMessage::update_components(
                "s",
                vec![Component::new("root", "Text").with("text", json!({"path": "/user/name"}))],
            ),
            AgentMessage::update_data_model("s", "/user/name", json!("Ada")),
        ];
        assert!(validator.validate_messages(&messages).is_valid());

        let messages = vec![
            AgentMessage::create_surface("s", "cat"),
            AgentMessage::update_components(
                "s",
                vec![Component::new("root", "Text").with("text", json!({"path": "/user/name"}))],
            ),
            AgentMessage::update_data_model("s", "/user/other", json!("Ada")),
        ];
        let report = validator.validate_messages(&messages);
        assert!(codes(&report).contains(&ErrorCode::UnresolvedBinding));
    }

    #[test]
    fn an_empty_catalog_skips_type_checks() {
        let catalog = Catalog::empty("none");
        let report = Validator::new(&catalog)
            .validate(&[Component::new("root", "Whatever").with("x", json!(1))]);
        assert!(report.is_valid(), "{:?}", report.errors);
    }

    #[test]
    fn report_converts_into_a_result_carrying_every_error() {
        let catalog = basic();
        let report = Validator::new(&catalog).validate(&[]);
        let Err(Error::Validation { errors }) = report.into_result() else {
            panic!("expected a validation error");
        };
        assert_eq!(errors.len(), 1);
        assert!(errors.to_string().contains("empty_components"));
    }
}
