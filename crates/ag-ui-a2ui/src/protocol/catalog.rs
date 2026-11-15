//! Component catalogs: what a surface is allowed to contain.
//!
//! A catalog names the component types and functions a surface may use, the
//! properties each component takes, which of those properties are structural
//! (they hold component ids rather than data), and any composition constraints.
//! Agent and renderer must agree on one, identified by an opaque `catalogId`.
//!
//! Two ways in:
//!
//! - [`Catalog::basic`] and [`Catalog::basic_for`] — the published basic
//!   catalog of a protocol version, 18 components, read from the document
//!   this crate ships.
//! - [`Catalog::from_schema`] — parse any A2UI catalog JSON Schema document,
//!   which is how custom design systems are described.
//!
//! # One surface, several catalogs
//!
//! Before v1.0 a surface has the one catalog it was created with. From v1.0
//! it may mix them: a component names its own with `@catalogId`, and falls
//! back to the surface's default. A
//! [`Validator`](crate::validate::Validator) made
//! [`over`](crate::validate::Validator::over) several catalogs, and a
//! [`MessageProcessor`](crate::processor::MessageProcessor), read each
//! component from the catalog it names. All the catalogs of one surface are
//! written against the surface's protocol version ([`Catalog::version`]).
//!
//! # Structural properties are what make validation possible
//!
//! The specification is explicit that a catalog must type child references as
//! `ComponentId` or `ChildList` rather than as bare strings — a validator
//! decides which fields are structural links by looking for exactly those
//! references. A raw `"type": "string"` is treated as static text (a URL, a
//! label) and its target is never checked. [`PropKind`] preserves that
//! distinction.
//!
//! # Property types are carried, not interpreted
//!
//! Each property also keeps the JSON type its schema pins it to ([`PropType`]),
//! which is what lets [`crate::validate`] reject `{"columns": "three"}` where the
//! catalog says integer. An `enum` is kept as well ([`PropDef::enum_values`]),
//! whether it is the property's own or one alternative of a `oneOf` beside a
//! data binding, so a literal it does not list is rejected too. Those are two
//! constraints out
//! of JSON Schema, not an engine: `pattern`, `minimum`, `additionalProperties`
//! and the rest are left to whatever validates the document itself. A property
//! whose schema states no type, or states several, is
//! [`PropType::Unconstrained`] and is never rejected — a catalog this crate reads
//! loosely must not turn into false failures.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

use serde_json::Value;

use crate::constants::{ROOT_ID, SURFACE_COMPONENT};
use crate::error::{Error, Result};
use crate::protocol::message::Component;
use crate::protocol::validate::{ErrorCode, ValidationError, is_identifier};
use crate::protocol::version::A2uiVersion;

/// Component types of the basic catalog, in the order the published
/// documents of v0.9, v0.9.1 and v1.0 all declare them.
pub const BASIC_COMPONENTS: [&str; 18] = [
    "Text",
    "Image",
    "Icon",
    "Video",
    "AudioPlayer",
    "Row",
    "Column",
    "List",
    "Card",
    "Tabs",
    "Modal",
    "Divider",
    "Button",
    "TextField",
    "CheckBox",
    "ChoicePicker",
    "Slider",
    "DateTimeInput",
];

/// Icon names the basic catalog's `Icon` component accepts.
pub const BASIC_ICON_NAMES: [&str; 59] = [
    "accountCircle",
    "add",
    "arrowBack",
    "arrowForward",
    "attachFile",
    "calendarToday",
    "call",
    "camera",
    "check",
    "close",
    "delete",
    "download",
    "edit",
    "event",
    "error",
    "fastForward",
    "favorite",
    "favoriteOff",
    "folder",
    "help",
    "home",
    "info",
    "locationOn",
    "lock",
    "lockOpen",
    "mail",
    "menu",
    "moreVert",
    "moreHoriz",
    "notificationsOff",
    "notifications",
    "pause",
    "payment",
    "person",
    "phone",
    "photo",
    "play",
    "print",
    "refresh",
    "rewind",
    "search",
    "send",
    "settings",
    "share",
    "shoppingCart",
    "skipNext",
    "skipPrevious",
    "star",
    "starHalf",
    "starOff",
    "stop",
    "upload",
    "visibility",
    "visibilityOff",
    "volumeDown",
    "volumeMute",
    "volumeOff",
    "volumeUp",
    "warning",
];

/// How a component property participates in the component graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropKind {
    /// A literal or data-bound value. Never a structural link.
    Value,
    /// A single component id (`common_types.json#/$defs/ComponentId`).
    ComponentRef,
    /// A `ChildList`: either an array of component ids or a template object.
    ChildList,
    /// An array of objects, where the named keys each hold a component id.
    ///
    /// `Tabs.tabs` is the canonical case: `[{title, child}, ...]`.
    ObjectListRefs {
        /// Keys within each array element that hold a component id.
        ref_keys: Vec<String>,
    },
}

impl PropKind {
    /// Whether this property can point at other components.
    pub fn is_structural(&self) -> bool {
        !matches!(self, PropKind::Value)
    }
}

/// The JSON type a property's schema pins its value to.
///
/// Populated from a bare `"type"` in the property schema, or from a `$ref` to
/// one of the common types, which name a JSON type and a data binding together:
/// `DynamicString` is "a string *or* a `{"path": …}`". The binding half needs no
/// representation here because a binding carries the renderer's value, not the
/// property's, so [`crate::validate`] skips bound values entirely.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PropType {
    /// The schema pins no single type, so no value is ever rejected.
    #[default]
    Unconstrained,
    /// `"type": "string"`, `ComponentId`, `DynamicString`.
    String,
    /// `"type": "number"`, `DynamicNumber`.
    Number,
    /// `"type": "integer"`, which also accepts a whole number written `2.0`.
    Integer,
    /// `"type": "boolean"`, `DynamicBoolean`.
    Boolean,
    /// `"type": "object"`, `Action`.
    Object,
    /// `"type": "array"`, `DynamicStringList`.
    Array,
}

impl PropType {
    /// Whether `value` satisfies this type.
    ///
    /// An [`PropType::Unconstrained`] property accepts anything. Nullable unions
    /// are left unconstrained by this summary and checked by the full schema.
    pub fn accepts(self, value: &Value) -> bool {
        match self {
            PropType::Unconstrained => true,
            PropType::String => value.is_string(),
            PropType::Number => value.is_number(),
            // Matches JSON Schema, where 2.0 is an integer and 2.5 is not.
            PropType::Integer => value.as_f64().is_some_and(|n| n.fract() == 0.0),
            PropType::Boolean => value.is_boolean(),
            PropType::Object => value.is_object(),
            PropType::Array => value.is_array(),
        }
    }

    /// The type as a noun phrase, for error messages.
    pub fn describe(self) -> &'static str {
        match self {
            PropType::Unconstrained => "any value",
            PropType::String => "a string",
            PropType::Number => "a number",
            PropType::Integer => "an integer",
            PropType::Boolean => "a boolean",
            PropType::Object => "an object",
            PropType::Array => "an array",
        }
    }

    /// Reads the JSON Schema spelling of a type.
    fn from_schema_name(name: &str) -> Self {
        match name {
            "string" => PropType::String,
            "number" => PropType::Number,
            "integer" => PropType::Integer,
            "boolean" => PropType::Boolean,
            "object" => PropType::Object,
            "array" => PropType::Array,
            // Types outside this summary require full schema validation.
            _ => PropType::Unconstrained,
        }
    }
}

/// One property of a component type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropDef {
    /// Property name as it appears on the wire.
    pub name: String,
    /// Whether the property holds data or component references.
    pub kind: PropKind,
    /// The JSON type the schema pins the value to, if it pins one.
    pub value_type: PropType,
    /// Human-readable description, carried into generated prompts.
    pub description: Option<String>,
    /// Permitted literal strings, when the schema constrains them to an enum.
    ///
    /// A literal string outside them fails [`crate::validate`]; a binding or a
    /// function call is not a literal and is left to the renderer. Read from
    /// the property's own `enum`, or from the `enum`s its `oneOf` / `anyOf`
    /// alternatives list when every alternative that can be a string has one.
    /// Empty when any string is accepted.
    pub enum_values: Vec<String>,
    /// Whether the component is invalid without this property.
    pub required: bool,
}

impl PropDef {
    fn new(name: &str, kind: PropKind, required: bool) -> Self {
        Self {
            name: name.to_string(),
            kind,
            value_type: PropType::Unconstrained,
            description: None,
            enum_values: Vec::new(),
            required,
        }
    }
}

/// One component type in a catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentDef {
    /// Type name, e.g. `Text`.
    pub name: String,
    /// Human-readable description, carried into generated prompts.
    pub description: Option<String>,
    /// Properties, keyed by name.
    pub props: BTreeMap<String, PropDef>,
    /// Property names that must be present, in declaration order.
    pub required: Vec<String>,
    /// Parent component types this may sit under. `None` means unrestricted.
    ///
    /// [`SURFACE_COMPONENT`] stands for the
    /// implicit surface container, so `["Surface"]` restricts a component to
    /// being the tree root.
    pub allowed_parents: Option<Vec<String>>,
    /// Child component types this may contain. `None` means unrestricted.
    pub allowed_children: Option<Vec<String>>,
    /// Declaration order of properties, for stable prompt rendering.
    prop_order: Vec<String>,
    /// Whether the component takes check rules: its schema composes the
    /// common `Checkable` type, or declares `checks` itself.
    checkable: bool,
    /// The definition's own `metadata`, where a v1.0 catalog gives it one.
    metadata: Option<Value>,
    /// Properties declared only inside a `oneOf` or `anyOf` branch. They are
    /// not in `props`, since no one branch is the component's shape, and
    /// they are still names the component has.
    branch_props: BTreeSet<String>,
}

impl ComponentDef {
    #[cfg(test)]
    fn new(name: &str, description: &str, props: Vec<PropDef>) -> Self {
        let required = props
            .iter()
            .filter(|p| p.required)
            .map(|p| p.name.clone())
            .collect();
        let prop_order = props.iter().map(|p| p.name.clone()).collect();
        let props = props.into_iter().map(|p| (p.name.clone(), p)).collect();
        Self {
            name: name.to_string(),
            description: Some(description.to_string()),
            props,
            required,
            allowed_parents: None,
            allowed_children: None,
            prop_order,
            checkable: false,
            metadata: None,
            branch_props: BTreeSet::new(),
        }
    }

    /// Whether the definition declares a property of this name anywhere: in
    /// itself, in what it is composed of, or in one of its alternatives.
    ///
    /// Wider than [`ComponentDef::props`], which leaves out a property that
    /// only a `oneOf` or `anyOf` branch declares. It is what decides whether
    /// a bare `metadata` or `catalogId` on a v1.0 component is the
    /// component's own property or the envelope key of the v1.0 drafts.
    pub fn declares(&self, name: &str) -> bool {
        self.props.contains_key(name) || self.branch_props.contains(name)
    }

    /// Whether a component of this type may carry check rules (`checks`, or
    /// `@checks` from v1.0): its schema composes the common `Checkable` type,
    /// or declares a `checks` property itself.
    pub fn is_checkable(&self) -> bool {
        self.checkable
    }

    /// The metadata the catalog gives this component type: an object with an
    /// `extensions` map. From v1.0; `None` for a definition that carries none.
    ///
    /// It says something about the type, in the catalog. The `@metadata` of a
    /// component on a surface says something about that one component.
    pub fn metadata(&self) -> Option<&Value> {
        self.metadata.as_ref()
    }

    /// Properties in declaration order.
    pub fn props_in_order(&self) -> impl Iterator<Item = &PropDef> {
        self.prop_order
            .iter()
            .filter_map(|name| self.props.get(name))
    }

    /// Every component id a concrete component of this type references.
    ///
    /// Locators are relative to the component object, e.g. `child`,
    /// `children[2]`, `children.componentId`, `tabs[0].child`.
    ///
    /// Only strings are ids. Anything else in a structural property — most
    /// often a child written inline as an object — references nothing, and
    /// [`crate::validate`] reports it: A2UI components are a flat list, and a
    /// parent names its children by id.
    pub fn references(&self, component: &Component) -> Vec<ComponentRef> {
        let mut refs = Vec::new();
        for (name, def) in &self.props {
            let Some(value) = component.props.get(name) else {
                continue;
            };
            match &def.kind {
                PropKind::Value => {}
                PropKind::ComponentRef => {
                    if let Value::String(id) = value {
                        refs.push(ComponentRef {
                            location: name.clone(),
                            id: id.clone(),
                        });
                    }
                }
                PropKind::ChildList => match value {
                    Value::Array(items) => {
                        for (index, item) in items.iter().enumerate() {
                            if let Value::String(id) = item {
                                refs.push(ComponentRef {
                                    location: format!("{name}[{index}]"),
                                    id: id.clone(),
                                });
                            }
                        }
                    }
                    Value::Object(map) => {
                        if let Some(Value::String(id)) = map.get("componentId") {
                            refs.push(ComponentRef {
                                location: format!("{name}.componentId"),
                                id: id.clone(),
                            });
                        }
                    }
                    _ => {}
                },
                PropKind::ObjectListRefs { ref_keys } => {
                    if let Value::Array(items) = value {
                        for (index, item) in items.iter().enumerate() {
                            let Value::Object(map) = item else { continue };
                            for key in ref_keys {
                                if let Some(Value::String(id)) = map.get(key) {
                                    refs.push(ComponentRef {
                                        location: format!("{name}[{index}].{key}"),
                                        id: id.clone(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
        refs.sort_by(|a, b| a.location.cmp(&b.location));
        refs
    }
}

/// One component id reference found inside a component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentRef {
    /// Where the reference sits within the component, e.g. `children[1]`.
    pub location: String,
    /// The referenced component id.
    pub id: String,
}

/// Which side may call a catalog function. A v1.0 catalog states it per
/// function, as `allowedCallers`.
///
/// Non-exhaustive, so that a caller can be added without breaking a `match`
/// written outside this crate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum AllowedCallers {
    /// The renderer alone, in a binding or an action. The default, and what
    /// every function is before v1.0.
    #[default]
    RendererOnly,
    /// The agent alone: a renderer asks the agent to run it. It cannot be
    /// bound to a component property or run by a local action.
    AgentOnly,
    /// Either side.
    RendererOrAgent,
}

impl AllowedCallers {
    /// The catalog's spelling: `rendererOnly`, `agentOnly`, `rendererOrAgent`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RendererOnly => "rendererOnly",
            Self::AgentOnly => "agentOnly",
            Self::RendererOrAgent => "rendererOrAgent",
        }
    }

    /// Reads the catalog's spelling. `None` for any other string.
    pub fn parse(text: &str) -> Option<Self> {
        [Self::RendererOnly, Self::AgentOnly, Self::RendererOrAgent]
            .into_iter()
            .find(|callers| callers.as_str() == text)
    }

    /// Whether a renderer may run the function itself, which is what a
    /// binding and a local action ask of it.
    pub const fn includes_renderer(self) -> bool {
        !matches!(self, Self::AgentOnly)
    }
}

/// One function in a catalog.
///
/// Non-exhaustive: a later protocol version may say more about a function.
/// Build one with [`FunctionDef::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FunctionDef {
    /// Function name, e.g. `formatString`.
    pub name: String,
    /// Human-readable description, carried into generated prompts.
    pub description: Option<String>,
    /// Declared return type, when the catalog states one.
    pub return_type: Option<String>,
    /// Which side may call it. [`AllowedCallers::RendererOnly`] unless a v1.0
    /// catalog says otherwise.
    pub allowed_callers: AllowedCallers,
    /// Whether the function runs only in answer to a user's gesture, such as
    /// `openUrl`. From v1.0.
    pub requires_user_activation: bool,
    /// The JSON Schema of the function's `args`, when its declaration states
    /// one: `properties.args` of a function written as the schema of a call
    /// to it, or its `args`.
    pub args: Option<Value>,
}

impl FunctionDef {
    /// A function a renderer runs, with nothing else said about it.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            return_type: None,
            allowed_callers: AllowedCallers::default(),
            requires_user_activation: false,
            args: None,
        }
    }

    /// What is wrong with the names of `args`, by what the declaration says
    /// of them without a schema engine: an argument a closed `args` schema
    /// does not declare, and a required one that is missing. `None` when the
    /// declaration states no schema, or one whose arguments cannot be read
    /// off its `properties`.
    ///
    /// The types of the arguments are a schema engine's to check.
    pub(crate) fn argument_fault(&self, args: &serde_json::Map<String, Value>) -> Option<String> {
        let schema = self.args.as_ref()?.as_object()?;
        // Anything that composes the arguments from elsewhere is not read.
        const COMPOSED: [&str; 6] = [
            "$ref",
            "allOf",
            "anyOf",
            "oneOf",
            "patternProperties",
            "propertyNames",
        ];
        if COMPOSED.iter().any(|keyword| schema.contains_key(*keyword)) {
            return None;
        }
        let declared = schema.get("properties").and_then(Value::as_object);
        let closed = schema.get("unevaluatedProperties") == Some(&Value::Bool(false))
            || schema.get("additionalProperties") == Some(&Value::Bool(false));
        if closed {
            let undeclared = args
                .keys()
                .find(|name| !declared.is_some_and(|declared| declared.contains_key(*name)));
            if let Some(name) = undeclared {
                return Some(format!(
                    "Function '{}' takes no argument '{name}'.",
                    self.name
                ));
            }
        }
        let missing = schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .find(|name| !args.contains_key(*name));
        missing.map(|name| format!("Function '{}' requires the argument '{name}'.", self.name))
    }
}

/// A set of component and function definitions, identified by `catalogId`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    /// Opaque identifier agent and renderer negotiate on.
    pub catalog_id: String,
    /// Human-readable title.
    pub title: Option<String>,
    /// Human-readable description.
    pub description: Option<String>,
    /// Markdown design guidance specific to this catalog, for prompts.
    pub instructions: Option<String>,
    /// Component types, keyed by name.
    pub components: BTreeMap<String, ComponentDef>,
    /// Functions, keyed by name.
    pub functions: BTreeMap<String, FunctionDef>,
    component_order: Vec<String>,
    /// The document the catalog was read from. Shared, so that cloning a
    /// catalog does not copy it.
    document: Option<Arc<Value>>,
    /// The protocol version the catalog is written against.
    version: A2uiVersion,
}

impl Catalog {
    /// An empty catalog with the given id, written against
    /// [`A2uiVersion::DEFAULT`].
    pub fn empty(catalog_id: impl Into<String>) -> Self {
        Self {
            catalog_id: catalog_id.into(),
            title: None,
            description: None,
            instructions: None,
            components: BTreeMap::new(),
            functions: BTreeMap::new(),
            component_order: Vec::new(),
            document: None,
            version: A2uiVersion::DEFAULT,
        }
    }

    /// The protocol version the catalog is written against: how its
    /// components spell a binding, and which surfaces may draw from it.
    ///
    /// A document states it as `protocolVersion`. One that states none is a
    /// v0.9 catalog, which is how the specification reads it: `protocolVersion`
    /// arrived with v1.0, and a catalog for v1.0 has to state it.
    pub fn version(&self) -> A2uiVersion {
        self.version
    }

    /// The same catalog, said to be written against `version`.
    ///
    /// For a catalog built in code, and for a v0.9.1 document, which states no
    /// version and would be read as v0.9. The document is not rewritten.
    #[must_use]
    pub fn with_version(mut self, version: A2uiVersion) -> Self {
        self.version = version;
        self
    }

    /// The same catalog under another id.
    ///
    /// For a renderer that registers a published catalog under a name of its
    /// own, as AG-UI's toolkit does with the v0.9 basic catalog
    /// ([`BASIC_CATALOG_ID`](crate::constants::BASIC_CATALOG_ID)). The catalog
    /// is then no longer the one its document names, so [`Catalog::document`]
    /// is `None` afterwards.
    #[must_use]
    pub fn with_catalog_id(mut self, catalog_id: impl Into<String>) -> Self {
        self.catalog_id = catalog_id.into();
        self.document = None;
        self
    }

    /// The document this catalog was read from, exactly as
    /// [`Catalog::from_schema`] was given it.
    ///
    /// A `Catalog` is a summary: it keeps what the validator reads. The
    /// document keeps everything else, such as `pattern` and `minimum`, the
    /// theme, `$defs`, and keywords this crate has never heard of. Reading a
    /// catalog and asking for its document is a round trip, and the document
    /// is what a schema validator, a prompt or a renderer is handed.
    ///
    /// `None` for a catalog that was not read from a document:
    /// [`Catalog::empty`], and any catalog after
    /// [`Catalog::insert_component`] or [`Catalog::with_catalog_id`], which
    /// make it another catalog than the one its document describes. Changing
    /// the public fields does not rewrite the document.
    ///
    /// ```
    /// use ag_ui_a2ui::Catalog;
    /// use serde_json::json;
    ///
    /// let document = json!({
    ///     "catalogId": "https://example.com/catalogs/charts",
    ///     "components": {
    ///         "Chart": {"type": "object", "properties": {"title": {"type": "string", "maxLength": 80}}}
    ///     },
    ///     "$defs": {"theme": {"type": "object"}},
    ///     "x-vendor": true
    /// });
    /// let catalog = Catalog::from_schema(&document)?;
    /// assert_eq!(catalog.document(), Some(&document));
    /// // The document reads back into the same catalog.
    /// assert_eq!(Catalog::from_schema(catalog.document().unwrap())?, catalog);
    ///
    /// // The basic catalog is read from the published document, and gives it back.
    /// assert_eq!(Catalog::basic().document().unwrap()["catalogId"], Catalog::basic().catalog_id);
    /// assert!(Catalog::empty("x").document().is_none());
    /// # Ok::<(), ag_ui_a2ui::Error>(())
    /// ```
    pub fn document(&self) -> Option<&Value> {
        self.document.as_deref()
    }

    /// [`Catalog::document`], as the catalog holds it: for a caller in this
    /// crate that keeps the document too and should not keep a second copy.
    #[cfg(feature = "agent")]
    pub(crate) fn shared_document(&self) -> Option<&Arc<Value>> {
        self.document.as_ref()
    }

    /// Looks up a component type.
    pub fn component(&self, name: &str) -> Option<&ComponentDef> {
        self.components.get(name)
    }

    /// Whether this catalog defines the given component type.
    pub fn has_component(&self, name: &str) -> bool {
        self.components.contains_key(name)
    }

    /// Component types in declaration order.
    pub fn components_in_order(&self) -> impl Iterator<Item = &ComponentDef> {
        self.component_order
            .iter()
            .filter_map(|name| self.components.get(name))
    }

    /// Adds or replaces a component type.
    ///
    /// The catalog is then no longer the one a document described, so
    /// [`Catalog::document`] is `None` afterwards.
    pub fn insert_component(&mut self, def: ComponentDef) {
        if !self.components.contains_key(&def.name) {
            self.component_order.push(def.name.clone());
        }
        self.components.insert(def.name.clone(), def);
        self.document = None;
    }

    /// Every component id referenced by `component`, using this catalog's
    /// structural property definitions.
    ///
    /// Returns an empty list for an unknown component type: without a
    /// definition there is no way to tell a child reference from a label.
    pub fn references(&self, component: &Component) -> Vec<ComponentRef> {
        self.component(&component.component)
            .map(|def| def.references(component))
            .unwrap_or_default()
    }

    /// The basic catalog published for v0.9: [`Catalog::basic_for`] with
    /// [`A2uiVersion::V0_9`].
    ///
    /// Its `catalog_id` is the published one,
    /// [`OFFICIAL_BASIC_CATALOG_ID`](crate::constants::OFFICIAL_BASIC_CATALOG_ID).
    pub fn basic() -> Self {
        Self::basic_for(A2uiVersion::V0_9)
    }

    /// The basic catalog published for `version`: 18 components and 14
    /// functions, read from the document this crate ships, which
    /// [`Catalog::document`] gives back. No file or network is touched.
    ///
    /// The three documents differ. v0.9 has `List.listStyle` and v0.9.1 does
    /// not, though the two carry one id. v1.0 is another catalog under another
    /// id, with `Video.posterUrl`, `TextField.placeholder` and `Slider.steps`,
    /// and its components spell bindings the v1.0 way.
    ///
    /// ```
    /// use ag_ui_a2ui::{A2uiVersion, Catalog};
    ///
    /// let v0_9 = Catalog::basic_for(A2uiVersion::V0_9);
    /// let v0_9_1 = Catalog::basic_for(A2uiVersion::V0_9_1);
    /// let v1_0 = Catalog::basic_for(A2uiVersion::V1_0);
    /// assert!(v0_9.component("List").unwrap().props.contains_key("listStyle"));
    /// assert!(!v0_9_1.component("List").unwrap().props.contains_key("listStyle"));
    /// assert_eq!(v0_9.catalog_id, v0_9_1.catalog_id);
    /// assert_eq!(v1_0.catalog_id, A2uiVersion::V1_0.basic_catalog_id());
    /// assert_eq!(v1_0.version(), A2uiVersion::V1_0);
    /// assert!(v1_0.component("TextField").unwrap().props.contains_key("placeholder"));
    /// ```
    pub fn basic_for(version: A2uiVersion) -> Self {
        static V0_9: OnceLock<Catalog> = OnceLock::new();
        static V0_9_1: OnceLock<Catalog> = OnceLock::new();
        static V1_0: OnceLock<Catalog> = OnceLock::new();
        let (cell, document) = match version {
            A2uiVersion::V0_9 => (&V0_9, include_str!("../../schemas/v0_9/catalog.json")),
            A2uiVersion::V0_9_1 => (&V0_9_1, include_str!("../../schemas/v0_9_1/catalog.json")),
            A2uiVersion::V1_0 => (&V1_0, include_str!("../../schemas/v1_0/catalog.json")),
        };
        cell.get_or_init(|| {
            // The documents are part of this crate, and a test reads each one.
            let parsed: Value =
                serde_json::from_str(document).expect("the shipped basic catalog is JSON");
            let mut catalog = Self::from_schema(&parsed)
                .expect("the shipped basic catalog is a catalog")
                .with_version(version);
            // A `Value` does not keep the order a document declares its
            // components in, and that order is the specification's.
            if let Ok(declared) = serde_json::from_str::<DeclaredOrder>(document) {
                catalog.component_order = declared
                    .components
                    .0
                    .into_iter()
                    .filter(|name| catalog.components.contains_key(name))
                    .collect();
            }
            catalog
        })
        .clone()
    }

    /// Parses an A2UI catalog JSON Schema document, and keeps it:
    /// [`Catalog::document`] gives it back.
    ///
    /// Understands the shape the specification uses: a top-level `components`
    /// map whose values are JSON Schemas, each typically an `allOf` of shared
    /// fragments plus a final object holding `properties` and `required`. The
    /// `allOf` members are merged; `$ref`s to `ComponentId` and `ChildList` are
    /// what mark a property structural.
    ///
    /// The catalog's version is the document's `protocolVersion`, written as
    /// a number (`"1.0"`, `"0.9.1"`), and v0.9 when the document states none
    /// ([`Catalog::version`]).
    ///
    /// # What v1.0 asks of a catalog
    ///
    /// A document that states `"protocolVersion": "1.0"` is held to the rules
    /// v1.0 gave catalogs, and is refused when it breaks one:
    ///
    /// - A component name, a function name, and the names of a component's
    ///   properties and of a function's arguments are identifiers
    ///   (`^[\p{XID_Start}_][\p{XID_Continue}]*$`), so no `-`, and no `@`,
    ///   which belongs to the protocol. `Surface` is the protocol's own
    ///   component and cannot be defined.
    /// - A `$ref` stays inside the catalog or points at `common_types.json`.
    ///   Nothing is ever fetched, so any other target could not be read.
    /// - `requiresUserActivation` is for a function the renderer alone may
    ///   call: a user's gesture happens there.
    /// - The names in a component's `metadata.extensions` are identifiers.
    ///
    /// Where the document lists its components in `$defs/anyComponent`, or its
    /// functions in `$defs/anyFunction`, the catalog defines the listed ones:
    /// an entry the union leaves out is one no message can use.
    ///
    /// A v0.9 or v0.9.1 document is read as before, with none of this.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Catalog`] if the document is not an object, has no
    /// `catalogId`, states a `protocolVersion` this crate does not speak, or is
    /// a v1.0 catalog that breaks one of the rules above;
    /// [`Error::Validation`] for an extension name that is not an identifier.
    pub fn from_schema(schema: &Value) -> Result<Self> {
        Self::from_schema_as(schema, A2uiVersion::V0_9)
    }

    /// [`Catalog::from_schema`], for a reader that knows which version a
    /// document that states none is written for: the agent layer's, which is
    /// handed a version with a catalog.
    pub(crate) fn from_schema_as(schema: &Value, unstated: A2uiVersion) -> Result<Self> {
        let root = schema
            .as_object()
            .ok_or_else(|| Error::catalog("catalog document must be a JSON object"))?;

        let catalog_id = root
            .get("catalogId")
            .or_else(|| root.get("$id"))
            .and_then(Value::as_str)
            .ok_or_else(|| Error::catalog("catalog document is missing 'catalogId'"))?;

        let mut catalog = Self::empty(catalog_id);
        catalog.version = match root.get("protocolVersion") {
            None | Some(Value::Null) => unstated,
            Some(Value::String(stated)) => A2uiVersion::from_number(stated).ok_or_else(|| {
                Error::catalog(format!(
                    "catalog '{catalog_id}' targets protocol version '{stated}', which this \
                     crate does not speak"
                ))
            })?,
            Some(_) => {
                return Err(Error::catalog(format!(
                    "catalog '{catalog_id}' declares a 'protocolVersion' that is not a string"
                )));
            }
        };
        catalog.title = string_field(root.get("title"));
        catalog.description = string_field(root.get("description"));
        catalog.instructions = string_field(root.get("instructions"));

        let held_to_rules = catalog.version.names_are_identifiers();
        if held_to_rules {
            check_catalog_rules(catalog.version, catalog_id, root)?;
        }
        // The entries a union lists, where the document has one.
        let listed = |union: &str, section: &str| -> Option<BTreeSet<String>> {
            let branches = root.get("$defs")?.get(union)?.get("oneOf")?.as_array()?;
            let prefix = format!("#/{section}/");
            held_to_rules.then(|| {
                branches
                    .iter()
                    .filter_map(|branch| branch.get("$ref")?.as_str()?.strip_prefix(&prefix))
                    .map(|name| name.replace("~1", "/").replace("~0", "~"))
                    .collect()
            })
        };
        let listed_components = listed("anyComponent", "components");
        let listed_functions = listed("anyFunction", "functions");
        let is_listed = |listed: &Option<BTreeSet<String>>, name: &str| {
            listed.as_ref().is_none_or(|listed| listed.contains(name))
        };

        if let Some(Value::Object(components)) = root.get("components") {
            for (name, component_schema) in components {
                if is_listed(&listed_components, name) {
                    catalog.insert_component(component_def_from_schema(name, component_schema));
                }
            }
        }
        if let Some(Value::Object(functions)) = root.get("functions") {
            for (name, function_schema) in functions {
                if !is_listed(&listed_functions, name) {
                    continue;
                }
                // A function is a schema for its call, so the specification
                // states the return type as the constant the call's
                // `returnType` has to be. The bare field is the shorthand.
                let return_type = function_schema
                    .get("returnType")
                    .or_else(|| function_schema.pointer("/properties/returnType/const"));
                let mut function = FunctionDef::new(name);
                function.description = string_field(function_schema.get("description"));
                function.return_type = string_field(return_type);
                // An unknown caller is the schema's to refuse; read as the
                // default, it allows no more than a renderer.
                function.allowed_callers = function_schema
                    .get("allowedCallers")
                    .and_then(Value::as_str)
                    .and_then(AllowedCallers::parse)
                    .unwrap_or_default();
                function.requires_user_activation =
                    function_schema.get("requiresUserActivation") == Some(&Value::Bool(true));
                function.args = function_schema
                    .pointer("/properties/args")
                    .or_else(|| function_schema.get("args"))
                    .filter(|args| args.is_object())
                    .cloned();
                catalog.functions.insert(name.to_string(), function);
            }
        }
        if let Some(Value::Array(functions)) = root.get("functions") {
            for definition in functions {
                let name = definition
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::catalog("inline function needs a name"))?;
                if catalog.functions.contains_key(name) {
                    return Err(Error::catalog(format!("duplicate function {name}")));
                }
                let mut function = FunctionDef::new(name);
                function.description = string_field(definition.get("description"));
                function.return_type = string_field(definition.get("returnType"));
                catalog.functions.insert(name.into(), function);
            }
        }
        catalog.document = Some(Arc::new(schema.clone()));
        Ok(catalog)
    }

    /// Checks `allowedParents` / `allowedChildren` across a component tree.
    ///
    /// Kept separate from [`crate::validate`] on purpose: the specification
    /// assigns composition failures their own renderer-side error codes
    /// (`UNALLOWED_PARENT`, `UNALLOWED_CHILD`), distinct from the structural
    /// validation codes. Components with no declared constraints never produce
    /// violations, which is every component of the basic catalog.
    ///
    /// The root component's parent is the implicit
    /// [`Surface`](crate::constants::SURFACE_COMPONENT) container.
    pub fn composition_violations(&self, components: &[Component]) -> Vec<CompositionViolation> {
        let types: BTreeMap<&str, &str> = components
            .iter()
            .map(|c| (c.id.as_str(), c.component.as_str()))
            .collect();
        let index: BTreeMap<&str, usize> = components
            .iter()
            .enumerate()
            .map(|(i, c)| (c.id.as_str(), i))
            .collect();
        let mut out = Vec::new();

        // The implicit Surface container is the root component's parent.
        if let Some(root) = components.iter().find(|c| c.id == ROOT_ID) {
            if let Some(def) = self.component(&root.component) {
                if let Some(allowed) = &def.allowed_parents {
                    if !allowed.iter().any(|p| p == SURFACE_COMPONENT) {
                        let position = index.get(ROOT_ID).copied().unwrap_or(0);
                        out.push(CompositionViolation {
                            code: CompositionCode::UnallowedParent,
                            path: format!("components[{position}].component"),
                            message: format!(
                                "'{}' cannot be the root of a surface: allowedParents is [{}], \
                                 which does not include '{SURFACE_COMPONENT}'.",
                                root.component,
                                allowed.join(", ")
                            ),
                        });
                    }
                }
            }
        }

        for (position, parent) in components.iter().enumerate() {
            let Some(parent_def) = self.component(&parent.component) else {
                continue;
            };
            for reference in parent_def.references(parent) {
                let Some(child_type) = types.get(reference.id.as_str()) else {
                    continue;
                };
                if let Some(allowed) = &parent_def.allowed_children {
                    if !allowed.iter().any(|c| c == child_type) {
                        out.push(CompositionViolation {
                            code: CompositionCode::UnallowedChild,
                            path: format!("components[{position}].{}", reference.location),
                            message: format!(
                                "'{}' cannot contain '{child_type}': allowedChildren is [{}].",
                                parent.component,
                                allowed.join(", ")
                            ),
                        });
                    }
                }
                if let Some(child_def) = self.component(child_type) {
                    if let Some(allowed) = &child_def.allowed_parents {
                        if !allowed.iter().any(|p| p == &parent.component) {
                            out.push(CompositionViolation {
                                code: CompositionCode::UnallowedParent,
                                path: format!("components[{position}].{}", reference.location),
                                message: format!(
                                    "'{child_type}' cannot sit under '{}': allowedParents is [{}].",
                                    parent.component,
                                    allowed.join(", ")
                                ),
                            });
                        }
                    }
                }
            }
        }
        out
    }
}

/// A composition-constraint failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionViolation {
    /// Which constraint was broken.
    pub code: CompositionCode,
    /// Locator into the components list, e.g. `components[3].children[0]`.
    pub path: String,
    /// Human- and LLM-readable explanation.
    pub message: String,
}

/// Error codes the specification assigns to composition failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompositionCode {
    /// A component sits under a parent its `allowedParents` excludes.
    UnallowedParent,
    /// A container holds a child its `allowedChildren` excludes.
    UnallowedChild,
}

impl CompositionCode {
    /// The wire string for this code.
    pub fn as_str(self) -> &'static str {
        match self {
            CompositionCode::UnallowedParent => "UNALLOWED_PARENT",
            CompositionCode::UnallowedChild => "UNALLOWED_CHILD",
        }
    }
}

impl std::fmt::Display for CompositionCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

fn string_field(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_string)
}

/// The order a catalog document declares its components in.
#[derive(serde::Deserialize)]
struct DeclaredOrder {
    components: KeysInOrder,
}

/// The keys of a JSON object in the order its text writes them.
struct KeysInOrder(Vec<String>);

impl<'de> serde::Deserialize<'de> for KeysInOrder {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct Keys;
        impl<'de> serde::de::Visitor<'de> for Keys {
            type Value = KeysInOrder;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an object")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut keys = Vec::new();
                while let Some(key) = map.next_key::<String>()? {
                    map.next_value::<serde::de::IgnoredAny>()?;
                    keys.push(key);
                }
                Ok(KeysInOrder(keys))
            }
        }
        deserializer.deserialize_map(Keys)
    }
}

/// Merges an `allOf` chain into a flat view of properties and requirements.
fn component_def_from_schema(name: &str, schema: &Value) -> ComponentDef {
    let mut props: BTreeMap<String, PropDef> = BTreeMap::new();
    let mut prop_order: Vec<String> = Vec::new();
    let mut required: BTreeSet<String> = BTreeSet::new();
    let mut required_order: Vec<String> = Vec::new();
    let mut description = None;
    let mut allowed_parents = None;
    let mut allowed_children = None;
    let mut checkable = false;
    let mut branch_props: BTreeSet<String> = BTreeSet::new();

    let mut stack = vec![schema];
    while let Some(current) = stack.pop() {
        let Some(object) = current.as_object() else {
            continue;
        };
        // What the alternatives declare, by name only: a branch is not the
        // component's shape, so nothing in it is required or typed here.
        let mut branches: Vec<&Value> = ["oneOf", "anyOf"]
            .iter()
            .filter_map(|keyword| object.get(*keyword)?.as_array())
            .flatten()
            .collect();
        while let Some(branch) = branches.pop() {
            let Some(branch) = branch.as_object() else {
                continue;
            };
            if let Some(Value::Object(properties)) = branch.get("properties") {
                branch_props.extend(properties.keys().cloned());
            }
            branches.extend(
                ["allOf", "oneOf", "anyOf"]
                    .iter()
                    .filter_map(|keyword| branch.get(*keyword)?.as_array())
                    .flatten(),
            );
        }
        // The check rules come from the common `Checkable` type, which a
        // component composes by reference.
        if object
            .get("$ref")
            .and_then(Value::as_str)
            .is_some_and(|reference| reference.rsplit('/').next() == Some("Checkable"))
        {
            checkable = true;
        }
        if description.is_none() {
            description = string_field(object.get("description"));
        }
        if allowed_parents.is_none() {
            allowed_parents = string_list(object.get("allowedParents"));
        }
        if allowed_children.is_none() {
            allowed_children = string_list(object.get("allowedChildren"));
        }
        if let Some(Value::Array(members)) = object.get("allOf") {
            // Reversed so the declared order survives the stack.
            for member in members.iter().rev() {
                stack.push(member);
            }
        }
        if let Some(Value::Array(names)) = object.get("required") {
            for entry in names {
                if let Some(field) = entry.as_str() {
                    if field != "component" && field != "id" && required.insert(field.to_string()) {
                        required_order.push(field.to_string());
                    }
                }
            }
        }
        if let Some(Value::Object(properties)) = object.get("properties") {
            for (prop_name, prop_schema) in properties {
                if prop_name == "component" || prop_name == "id" {
                    continue;
                }
                if !props.contains_key(prop_name) {
                    prop_order.push(prop_name.clone());
                }
                props.insert(
                    prop_name.clone(),
                    PropDef {
                        name: prop_name.clone(),
                        kind: prop_kind_from_schema(prop_schema),
                        value_type: prop_type_from_schema(prop_schema),
                        description: string_field(prop_schema.get("description")),
                        enum_values: permitted_strings(prop_schema).unwrap_or_default(),
                        required: false,
                    },
                );
            }
        }
    }

    for field in &required_order {
        if let Some(prop) = props.get_mut(field) {
            prop.required = true;
        } else {
            // Required but undescribed: keep it so the validator can still
            // demand it.
            prop_order.push(field.clone());
            props.insert(field.clone(), PropDef::new(field, PropKind::Value, true));
        }
    }

    checkable |= props.contains_key("checks");
    ComponentDef {
        name: name.to_string(),
        description,
        props,
        required: required_order,
        allowed_parents,
        allowed_children,
        prop_order,
        checkable,
        metadata: schema
            .get("metadata")
            .filter(|value| value.is_object())
            .cloned(),
        branch_props,
    }
}

/// The rules v1.0 holds a catalog document to when it is read. See
/// [`Catalog::from_schema`].
fn check_catalog_rules(
    version: A2uiVersion,
    catalog_id: &str,
    root: &serde_json::Map<String, Value>,
) -> Result<()> {
    let entries = |section: &str| {
        root.get(section)
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
    };
    let envelope = version.component_keys();
    for (name, definition) in entries("components") {
        if !is_identifier(name) {
            return Err(Error::catalog(format!(
                "Invalid UAX #31 component identifier '{name}' in catalog '{catalog_id}': a \
                 letter or '_' first, then letters, digits and '_'"
            )));
        }
        if name == SURFACE_COMPONENT {
            return Err(Error::catalog(format!(
                "catalog '{catalog_id}' defines '{SURFACE_COMPONENT}', which is the protocol's \
                 own component for a surface"
            )));
        }
        // The properties the definition declares, in itself and in what it
        // is composed of. The envelope's keys are the protocol's.
        let own = definition.get("properties");
        let composed = definition
            .get("allOf")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|branch| branch.get("properties"));
        let declared = std::iter::once(own)
            .chain(composed)
            .filter_map(|properties| properties?.as_object())
            .flat_map(serde_json::Map::keys);
        for property in declared {
            if !envelope.contains(property) && !is_identifier(property) {
                return Err(Error::catalog(format!(
                    "Invalid UAX #31 property identifier '{property}' of component '{name}' in \
                     catalog '{catalog_id}': a letter or '_' first, then letters, digits and '_'"
                )));
            }
        }
        let extensions = definition
            .get("metadata")
            .and_then(|metadata| metadata.get("extensions"))
            .and_then(Value::as_object);
        for key in extensions.into_iter().flatten().map(|(key, _)| key) {
            if !is_identifier(key) {
                return Err(Error::Validation {
                    errors: vec![ValidationError::new(
                        ErrorCode::InvalidValue,
                        format!("components.{name}.metadata.extensions.{key}"),
                        format!(
                            "Extension name '{key}' of component '{name}' in catalog \
                             '{catalog_id}' must be a valid identifier: a letter or '_' first, \
                             then letters, digits and '_'."
                        ),
                    )]
                    .into(),
                });
            }
        }
    }
    for (name, definition) in entries("functions") {
        if !is_identifier(name) {
            return Err(Error::catalog(format!(
                "Invalid UAX #31 function identifier '{name}' in catalog '{catalog_id}': a \
                 letter or '_' first, then letters, digits and '_'; '@' is the protocol's"
            )));
        }
        if let Some(argument) = argument_names(definition, root)
            .into_iter()
            .find(|argument| !is_identifier(argument))
        {
            return Err(Error::catalog(format!(
                "Invalid UAX #31 argument identifier '{argument}' of function '{name}' in \
                 catalog '{catalog_id}': a letter or '_' first, then letters, digits and '_'"
            )));
        }
        let needs_gesture = definition.get("requiresUserActivation") == Some(&Value::Bool(true));
        let callers = definition.get("allowedCallers").and_then(Value::as_str);
        if needs_gesture && callers.is_some_and(|callers| callers != "rendererOnly") {
            return Err(Error::catalog(format!(
                "function '{name}' of catalog '{catalog_id}' sets requiresUserActivation with \
                 allowedCallers '{}': a user's gesture happens on the renderer, so only a \
                 rendererOnly function may require one",
                callers.unwrap_or_default()
            )));
        }
    }
    // Every `$ref` in the document, wherever it sits.
    let mut stack: Vec<&Value> = root.values().collect();
    while let Some(value) = stack.pop() {
        match value {
            Value::Array(items) => stack.extend(items),
            Value::Object(map) => {
                if let Some(Value::String(reference)) = map.get("$ref") {
                    let document = reference.split('#').next().unwrap_or_default();
                    let own = document.is_empty();
                    let common_types =
                        document == "common_types.json" || document.ends_with("/common_types.json");
                    if !own && !common_types {
                        return Err(Error::catalog(format!(
                            "Unsupported external $ref '{reference}' in catalog \
                             '{catalog_id}': a catalog refers to its own definitions and to \
                             common_types.json, and no other document is ever fetched"
                        )));
                    }
                }
                stack.extend(map.values());
            }
            _ => {}
        }
    }
    Ok(())
}

/// The argument names a function's definition declares.
///
/// A published document writes a function as the schema of a call to it, so
/// the arguments are the properties of its `args`. A shorter definition gives
/// them under `args` or `parameters`, as a schema or as a plain map of names.
/// A reference into the catalog's own `$defs` is followed.
fn argument_names<'a>(
    definition: &'a Value,
    root: &'a serde_json::Map<String, Value>,
) -> Vec<&'a str> {
    let object = |value: Option<&'a Value>| value.filter(|value| value.is_object());
    let Some(arguments) = object(definition.pointer("/properties/args"))
        .or_else(|| object(definition.get("args")))
        .or_else(|| object(definition.get("parameters")))
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    const COMBINERS: [&str; 3] = ["allOf", "anyOf", "oneOf"];
    let is_schema = arguments.get("properties").is_some_and(Value::is_object)
        || arguments.get("$ref").is_some_and(Value::is_string)
        || COMBINERS
            .iter()
            .any(|keyword| arguments.get(*keyword).is_some_and(Value::is_array));
    if !is_schema {
        // A plain map of names: nothing in it is a keyword of a schema.
        let is_map = !arguments.get("type").is_some_and(Value::is_string)
            && !arguments.contains_key("not")
            && arguments
                .values()
                .all(|value| value.is_object() || value.is_boolean());
        return match is_map {
            true => arguments.keys().map(String::as_str).collect(),
            false => Vec::new(),
        };
    }
    let mut names = Vec::new();
    let mut visited = BTreeSet::new();
    let mut stack = vec![arguments];
    while let Some(node) = stack.pop() {
        if let Some(properties) = node.get("properties").and_then(Value::as_object) {
            names.extend(properties.keys().map(String::as_str));
        }
        let referred = node
            .get("$ref")
            .and_then(Value::as_str)
            .filter(|reference| reference.starts_with("#/$defs/") && visited.insert(*reference))
            .and_then(|reference| {
                reference[1..]
                    .split('/')
                    .skip(1)
                    .try_fold(None::<&Value>, |at, segment| {
                        let segment = segment.replace("~1", "/").replace("~0", "~");
                        match at {
                            None => root.get(&segment).map(Some),
                            Some(at) => at.get(&segment).map(Some),
                        }
                    })
                    .flatten()
            });
        let branches = COMBINERS
            .iter()
            .filter_map(|keyword| node.get(*keyword)?.as_array())
            .flatten();
        stack.extend(
            referred
                .into_iter()
                .chain(branches)
                .filter_map(Value::as_object),
        );
    }
    names
}

fn string_list(value: Option<&Value>) -> Option<Vec<String>> {
    let Some(Value::Array(items)) = value else {
        return None;
    };
    Some(
        items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
    )
}

/// The literal strings a schema permits, when it permits only some of them.
///
/// `None` means any string is accepted, and so does a schema this summary
/// cannot read: a constraint that is not understood must never reject a value.
/// `Some` lists the strings that are; it is empty for a schema no string
/// satisfies at all, such as `"type": "object"`.
///
/// A bare `enum` lists them directly. More often the `enum` is one alternative
/// of several — the specification's `Icon.name` is
/// `oneOf: [{"enum": […]}, {svgPath}, DataBinding]` — and then what the
/// property permits is what its arms permit between them: the union of the
/// strings each arm lists, unless some arm takes strings freely (a plain
/// `"type": "string"`, a `DynamicString`), in which case nothing is ruled out.
/// An arm that cannot be a string says nothing about which strings are
/// allowed, so it adds none and rules none out.
///
/// The keywords of one schema all have to hold, so each that narrows the
/// strings narrows them further.
fn permitted_strings(schema: &Value) -> Option<Vec<String>> {
    let object = schema.as_object()?;
    let mut narrowed: Option<Vec<String>> = None;
    let mut narrow = |values: Vec<String>| {
        narrowed = Some(match narrowed.take() {
            Some(current) => current
                .into_iter()
                .filter(|value| values.contains(value))
                .collect(),
            None => values,
        });
    };

    if let Some(values) = string_list(object.get("enum")) {
        narrow(values);
    }
    if let Some(constant) = object.get("const") {
        narrow(constant.as_str().map(str::to_string).into_iter().collect());
    }
    let admits_strings = match object.get("type") {
        Some(Value::String(name)) => name == "string",
        Some(Value::Array(names)) => names.iter().any(|name| name == "string"),
        _ => true,
    };
    let refers_to_strings = object
        .get("$ref")
        .and_then(Value::as_str)
        .is_none_or(|reference| {
            matches!(
                ref_type(reference),
                PropType::String | PropType::Unconstrained
            )
        });
    if !admits_strings || !refers_to_strings {
        narrow(Vec::new());
    }
    if let Some(Value::Array(members)) = object.get("allOf") {
        for values in members.iter().filter_map(permitted_strings) {
            narrow(values);
        }
    }
    for key in ["oneOf", "anyOf"] {
        let Some(Value::Array(arms)) = object.get(key) else {
            continue;
        };
        // `None` from any arm makes the whole `collect` `None`: that arm takes
        // every string, so the alternatives together rule none out.
        if let Some(arms) = arms
            .iter()
            .map(permitted_strings)
            .collect::<Option<Vec<_>>>()
        {
            let mut union: Vec<String> = Vec::new();
            for value in arms.into_iter().flatten() {
                if !union.contains(&value) {
                    union.push(value);
                }
            }
            narrow(union);
        }
    }
    narrowed
}

/// Decides whether a property schema describes a structural link.
fn prop_kind_from_schema(schema: &Value) -> PropKind {
    if let Some(kind) = ref_kind(schema.get("$ref")) {
        return kind;
    }
    // `allOf` / `oneOf` / `anyOf` wrappers around a structural $ref.
    for key in ["allOf", "oneOf", "anyOf"] {
        if let Some(Value::Array(members)) = schema.get(key) {
            for member in members {
                let kind = prop_kind_from_schema(member);
                if kind.is_structural() {
                    return kind;
                }
            }
        }
    }
    if schema.get("type").and_then(Value::as_str) == Some("array") {
        if let Some(items) = schema.get("items") {
            if let Some(kind) = ref_kind(items.get("$ref")) {
                // An array of ComponentId behaves exactly like a static child list.
                return match kind {
                    PropKind::ComponentRef => PropKind::ChildList,
                    other => other,
                };
            }
            if let Some(Value::Object(item_props)) = items.get("properties") {
                let ref_keys: Vec<String> = item_props
                    .iter()
                    .filter(|(_, value)| prop_kind_from_schema(value).is_structural())
                    .map(|(key, _)| key.clone())
                    .collect();
                if !ref_keys.is_empty() {
                    return PropKind::ObjectListRefs { ref_keys };
                }
            }
        }
    }
    PropKind::Value
}

fn ref_kind(reference: Option<&Value>) -> Option<PropKind> {
    let reference = reference?.as_str()?;
    let target = reference.rsplit('/').next()?;
    match target {
        // `Child` is v1.0's name for a reference to a single child.
        "ComponentId" | "Child" => Some(PropKind::ComponentRef),
        "ChildList" => Some(PropKind::ChildList),
        _ => None,
    }
}

/// Reads the JSON type a property schema pins its value to.
///
/// Deliberately narrow. A `$ref` is resolved by the *name* of its target rather
/// than by following it, because the common types are a fixed, published set and
/// a catalog is free to reference them across documents this crate never loads.
/// An unrecognized target constrains nothing.
fn prop_type_from_schema(schema: &Value) -> PropType {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        return ref_type(reference);
    }
    if let Some(name) = schema.get("type").and_then(Value::as_str) {
        return PropType::from_schema_name(name);
    }
    // `allOf` members all have to hold, so any one of them that states a type
    // states the type. `oneOf` / `anyOf` members are alternatives, so they only
    // pin a type when every arm agrees — `DynamicString` spelled out inline is
    // `anyOf: [string, DataBinding]`, which pins nothing.
    if let Some(Value::Array(members)) = schema.get("allOf") {
        if let Some(found) = members
            .iter()
            .map(prop_type_from_schema)
            .find(|found| *found != PropType::Unconstrained)
        {
            return found;
        }
    }
    for key in ["oneOf", "anyOf"] {
        let Some(Value::Array(members)) = schema.get(key) else {
            continue;
        };
        let mut arms = members.iter().map(prop_type_from_schema);
        if let Some(first) = arms.next() {
            if first != PropType::Unconstrained && arms.all(|arm| arm == first) {
                return first;
            }
        }
    }
    PropType::Unconstrained
}

/// The JSON type behind a `$ref` to one of the published common types.
fn ref_type(reference: &str) -> PropType {
    match reference.rsplit('/').next().unwrap_or_default() {
        "ComponentId" | "Child" | "DynamicString" => PropType::String,
        "DynamicNumber" => PropType::Number,
        "DynamicBoolean" => PropType::Boolean,
        "DynamicStringList" => PropType::Array,
        "Action" | "DataBinding" | "AccessibilityAttributes" => PropType::Object,
        // `ChildList` is an array or a template object; `DynamicValue` is
        // anything at all. Neither narrows to one type.
        _ => PropType::Unconstrained,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_catalog_keeps_the_document_it_was_read_from() {
        let document = json!({
            "$id": "https://example.com/catalogs/forms",
            "catalogId": "https://example.com/catalogs/forms",
            "components": {"Field": {"type": "object", "unevaluatedProperties": false,
                "properties": {"label": {"type": "string", "pattern": "^[A-Z]"}}}},
            "functions": {"upper": {"returnType": "string"}},
            "$defs": {"theme": {"type": "object"}, "unused": {"const": 1}},
            "x-anything": [1, 2, 3]
        });
        let catalog = Catalog::from_schema(&document).unwrap();
        assert_eq!(catalog.document(), Some(&document));
        // Read again from what it gave back, it gives the same back.
        let again = Catalog::from_schema(catalog.document().unwrap()).unwrap();
        assert_eq!(again.document(), Some(&document));
        assert_eq!(again, catalog);

        // A catalog that was not read from a document has none, and one that
        // has been added to or renamed no longer has the one it was read from.
        assert!(Catalog::basic().document().is_some());
        assert!(
            Catalog::basic()
                .with_catalog_id(crate::constants::BASIC_CATALOG_ID)
                .document()
                .is_none()
        );
        assert!(Catalog::empty("x").document().is_none());
        let mut extended = catalog.clone();
        assert_eq!(extended.document(), Some(&document));
        extended.insert_component(ComponentDef::new("Extra", "An extra.", vec![]));
        assert!(extended.document().is_none());
        assert_ne!(extended, catalog);

        // Two documents that summarize alike are still two catalogs.
        let mut stricter = document.clone();
        stricter["components"]["Field"]["properties"]["label"]["pattern"] = json!("^[A-Z][a-z]");
        let stricter = Catalog::from_schema(&stricter).unwrap();
        assert_eq!(stricter.components, catalog.components);
        assert_ne!(stricter, catalog);
    }

    #[test]
    fn a_function_return_type_is_read_from_the_schema_of_its_call() {
        // The form the specification's catalogs use, and the shorthand.
        let catalog = Catalog::from_schema(&json!({
            "catalogId": "c",
            "functions": {
                "capitalize": {"type": "object", "properties": {
                    "call": {"const": "capitalize"},
                    "returnType": {"const": "string"}
                }},
                "count": {"returnType": "number"},
                "untyped": {"type": "object"}
            }
        }))
        .unwrap();
        let returns = |name: &str| catalog.functions[name].return_type.as_deref();
        assert_eq!(returns("capitalize"), Some("string"));
        assert_eq!(returns("count"), Some("number"));
        assert_eq!(returns("untyped"), None);

        // Every function of a published basic catalog states one.
        for &version in A2uiVersion::ALL {
            for (name, function) in &Catalog::basic_for(version).functions {
                assert!(function.return_type.is_some(), "{version} {name}");
            }
        }
    }

    #[test]
    fn basic_catalog_has_the_eighteen_components_and_fourteen_functions() {
        assert_eq!(Catalog::basic(), Catalog::basic_for(A2uiVersion::V0_9));
        for &version in A2uiVersion::ALL {
            let catalog = Catalog::basic_for(version);
            assert_eq!(catalog.version(), version);
            assert_eq!(catalog.catalog_id, version.basic_catalog_id());
            assert_eq!(catalog.components.len(), 18, "{version}");
            assert_eq!(catalog.functions.len(), 14, "{version}");
            // The order is the document's, which a parsed value does not keep.
            let order: Vec<&str> = catalog
                .components_in_order()
                .map(|d| d.name.as_str())
                .collect();
            assert_eq!(order, BASIC_COMPONENTS.to_vec(), "{version}");
            // The catalog is the document, and gives it back.
            let document = catalog.document().expect("read from a document");
            assert_eq!(
                Catalog::from_schema(document).unwrap().components,
                catalog.components
            );
        }
    }

    #[test]
    fn the_three_basic_catalogs_are_three_documents() {
        let [v0_9, v0_9_1, v1_0] =
            [A2uiVersion::V0_9, A2uiVersion::V0_9_1, A2uiVersion::V1_0].map(Catalog::basic_for);
        let has = |catalog: &Catalog, component: &str, prop: &str| {
            catalog
                .component(component)
                .unwrap()
                .props
                .contains_key(prop)
        };
        // v0.9 got `listStyle` back; v0.9.1 never had it.
        assert!(has(&v0_9, "List", "listStyle"));
        assert!(!has(&v0_9_1, "List", "listStyle"));
        assert!(!has(&v1_0, "List", "listStyle"));
        // What v1.0 added and took away.
        assert!(has(&v1_0, "Video", "posterUrl"));
        assert!(has(&v1_0, "TextField", "placeholder"));
        assert!(has(&v1_0, "Slider", "steps"));
        assert!(!has(&v1_0, "TextField", "validationRegexp"));
        assert!(has(&v0_9, "TextField", "validationRegexp"));
        assert!(
            v1_0.component("TextField")
                .unwrap()
                .required
                .contains(&"value".to_string())
        );

        // The input components take checks in every version; the others never.
        // The documents give them the rules through the common `Checkable`
        // type, so `checks` is not one of a component's own properties.
        assert!(!has(&v0_9, "TextField", "checks"));
        for catalog in [&v0_9, &v0_9_1, &v1_0] {
            for (component, checkable) in [("Button", true), ("TextField", true), ("Text", false)] {
                assert_eq!(
                    catalog.component(component).unwrap().is_checkable(),
                    checkable,
                    "{component} of {}",
                    catalog.version()
                );
            }
        }

        // A reference to one child is `ComponentId` before v1.0 and `Child` in it.
        for catalog in [&v0_9, &v1_0] {
            assert_eq!(
                catalog.component("Card").unwrap().props["child"].kind,
                PropKind::ComponentRef
            );
        }

        // v1.0 says who may call a function, and which need a user's gesture.
        let open_url = &v1_0.functions["openUrl"];
        assert!(open_url.requires_user_activation);
        assert_eq!(open_url.allowed_callers, AllowedCallers::RendererOnly);
        assert_eq!(
            v1_0.functions["required"].return_type.as_deref(),
            Some("validationResult")
        );
        assert_eq!(
            v0_9.functions["required"].return_type.as_deref(),
            Some("boolean")
        );
        assert!(!v0_9.functions["openUrl"].requires_user_activation);
    }

    #[test]
    fn a_catalog_states_its_version_or_is_a_v0_9_one() {
        let read = |version: Value| {
            let mut document = json!({"catalogId": "c"});
            if !version.is_null() {
                document["protocolVersion"] = version;
            }
            Catalog::from_schema(&document)
        };
        assert_eq!(read(json!(null)).unwrap().version(), A2uiVersion::V0_9);
        assert_eq!(read(json!("0.9")).unwrap().version(), A2uiVersion::V0_9);
        assert_eq!(read(json!("0.9.1")).unwrap().version(), A2uiVersion::V0_9_1);
        assert_eq!(
            read(json!("v0.9.1")).unwrap().version(),
            A2uiVersion::V0_9_1
        );
        assert_eq!(read(json!("1.0")).unwrap().version(), A2uiVersion::V1_0);
        for other in [json!("0.8"), json!("2.0"), json!(1.0), json!(true)] {
            assert!(
                matches!(read(other.clone()), Err(Error::Catalog(_))),
                "{other}"
            );
        }
        assert_eq!(
            Catalog::empty("c")
                .with_version(A2uiVersion::V1_0)
                .version(),
            A2uiVersion::V1_0
        );

        // What a v1.0 catalog says of a function, and what it says nothing of.
        let catalog = Catalog::from_schema(&json!({
            "catalogId": "c",
            "protocolVersion": "1.0",
            "functions": {
                "fetchData": {"returnType": "object", "allowedCallers": "agentOnly"},
                "both": {"returnType": "string", "allowedCallers": "rendererOrAgent"},
                "plain": {"returnType": "string"},
                "odd": {"returnType": "string", "allowedCallers": "everyone"}
            }
        }))
        .unwrap();
        let callers = |name: &str| catalog.functions[name].allowed_callers;
        assert_eq!(callers("fetchData"), AllowedCallers::AgentOnly);
        assert!(!callers("fetchData").includes_renderer());
        assert_eq!(callers("both"), AllowedCallers::RendererOrAgent);
        assert_eq!(callers("plain"), AllowedCallers::RendererOnly);
        assert_eq!(callers("odd"), AllowedCallers::RendererOnly);
    }

    #[test]
    fn a_v1_0_catalog_is_held_to_the_rules_of_v1_0_when_it_is_read() {
        // Each document breaks one rule, and says which in its refusal.
        let broken = [
            (
                json!({"components": {"My-Card": {"type": "object"}}}),
                "Invalid UAX #31 component identifier 'My-Card'",
            ),
            (
                json!({"components": {"Surface": {"type": "object"}}}),
                "defines 'Surface'",
            ),
            (
                json!({"components": {"Card": {"type": "object", "properties": {"sub-title": {}}}}}),
                "Invalid UAX #31 property identifier 'sub-title' of component 'Card'",
            ),
            (
                json!({"components": {"Card": {"allOf": [{"properties": {"1st": {}}}]}}}),
                "Invalid UAX #31 property identifier '1st' of component 'Card'",
            ),
            (
                json!({"functions": {"@now": {"returnType": "string"}}}),
                "Invalid UAX #31 function identifier '@now'",
            ),
            (
                json!({"functions": {"join": {"parameters": {"with-sep": {"type": "string"}}}}}),
                "Invalid UAX #31 argument identifier 'with-sep' of function 'join'",
            ),
            (
                json!({"functions": {"join": {"properties": {
                    "args": {"type": "object", "properties": {"a b": {}}}
                }}}}),
                "Invalid UAX #31 argument identifier 'a b' of function 'join'",
            ),
            (
                json!({"functions": {"join": {"args": {"$ref": "#/$defs/JoinArgs"}}},
                       "$defs": {"JoinArgs": {"properties": {"no.dots": {}}}}}),
                "Invalid UAX #31 argument identifier 'no.dots' of function 'join'",
            ),
            (
                json!({"functions": {"openUrl": {
                    "requiresUserActivation": true, "allowedCallers": "rendererOrAgent"
                }}}),
                "sets requiresUserActivation with allowedCallers 'rendererOrAgent'",
            ),
            (
                json!({"components": {"Card": {"properties": {
                    "title": {"$ref": "other_catalog.json#/$defs/Title"}
                }}}}),
                "Unsupported external $ref 'other_catalog.json#/$defs/Title'",
            ),
        ];
        for (mut document, says) in broken {
            document["catalogId"] = json!("c");
            // The same document is a v0.9 catalog, which has none of the rules.
            assert!(Catalog::from_schema(&document).is_ok(), "{document}");
            document["protocolVersion"] = json!("1.0");
            match Catalog::from_schema(&document) {
                Err(Error::Catalog(reason)) => assert!(reason.contains(says), "{reason}"),
                other => panic!("{document}: {other:?}"),
            }
        }
        // An extension that is not named by an identifier is a value the
        // protocol does not allow, in a document that is a catalog.
        let extension = json!({
            "catalogId": "c",
            "protocolVersion": "1.0",
            "components": {"Card": {"metadata": {"extensions": {"com.example": 1}}}}
        });
        assert!(matches!(
            Catalog::from_schema(&extension),
            Err(Error::Validation { .. })
        ));

        // What the rules let through: the envelope's own keys beside the
        // properties, names outside ASCII, a reference to the common types
        // by a relative or an absolute name, and a gesture a renderer asks for.
        let sound = Catalog::from_schema(&json!({
            "catalogId": "c",
            "protocolVersion": "1.0",
            "components": {
                "Carte_ℓ": {
                    "metadata": {"extensions": {"com_example": 1}},
                    "properties": {
                        "@catalogId": {"type": "string"},
                        "titre_ℓ": {"$ref": "common_types.json#/$defs/DynamicString"},
                        "child": {"$ref": "https://a2ui.org/specification/v1_0/common_types.json#/$defs/Child"}
                    }
                }
            },
            "functions": {
                "openUrl": {
                    "requiresUserActivation": true,
                    "allowedCallers": "rendererOnly",
                    "parameters": {"url": {"type": "string"}}
                }
            }
        }))
        .unwrap();
        let card = sound.component("Carte_ℓ").unwrap();
        assert_eq!(
            card.metadata(),
            Some(&json!({"extensions": {"com_example": 1}}))
        );
        // v1.0's `Child` is a reference to a component, like `ComponentId`.
        assert_eq!(card.props["child"].kind, PropKind::ComponentRef);
        assert!(sound.functions["openUrl"].requires_user_activation);
    }

    #[test]
    fn a_v1_0_catalog_defines_the_entries_its_unions_list() {
        let document = json!({
            "catalogId": "c",
            "protocolVersion": "1.0",
            "components": {"Text": {"type": "object"}, "Draft": {"type": "object"}},
            "functions": {"upper": {"returnType": "string"}, "draft": {"returnType": "string"}},
            "$defs": {
                "anyComponent": {"oneOf": [{"$ref": "#/components/Text"}]},
                "anyFunction": {"oneOf": [{"$ref": "#/functions/upper"}]}
            }
        });
        let catalog = Catalog::from_schema(&document).unwrap();
        assert_eq!(catalog.components.keys().collect::<Vec<_>>(), ["Text"]);
        assert_eq!(catalog.functions.keys().collect::<Vec<_>>(), ["upper"]);
        // The document is kept whole all the same.
        assert_eq!(catalog.document(), Some(&document));

        // A v0.9 document has no such rule: every entry is defined.
        let mut older = document;
        older["protocolVersion"] = json!("v0.9");
        let catalog = Catalog::from_schema(&older).unwrap();
        assert_eq!(catalog.components.len(), 2);
        assert_eq!(catalog.functions.len(), 2);
    }

    #[test]
    fn a_definition_declares_what_its_alternatives_declare_too() {
        let catalog = Catalog::from_schema(&json!({
            "catalogId": "c",
            "components": {
                "Pager": {
                    "type": "object",
                    "properties": {"text": {"type": "string"}},
                    "allOf": [{
                        "properties": {"page": {"type": "integer"}},
                        "oneOf": [
                            {"properties": {"metadata": {"type": "string"}}},
                            {"properties": {"mode": {"const": "none"}}, "anyOf": [
                                {"properties": {"catalogId": {"type": "string"}}}
                            ]}
                        ]
                    }]
                }
            }
        }))
        .unwrap();
        let pager = catalog.component("Pager").unwrap();
        // A branch is not the component's shape, so it adds no property.
        assert_eq!(pager.props.keys().collect::<Vec<_>>(), ["page", "text"]);
        for name in ["text", "page", "metadata", "mode", "catalogId"] {
            assert!(pager.declares(name), "{name}");
        }
        assert!(!pager.declares("accessibility"));
    }

    #[test]
    fn structural_props_are_marked_and_value_props_are_not() {
        let catalog = Catalog::basic();
        assert_eq!(
            catalog.component("Card").unwrap().props["child"].kind,
            PropKind::ComponentRef
        );
        assert_eq!(
            catalog.component("Row").unwrap().props["children"].kind,
            PropKind::ChildList
        );
        assert_eq!(
            catalog.component("Text").unwrap().props["text"].kind,
            PropKind::Value
        );
        // An Image url is a string but never a component reference.
        assert!(
            !catalog.component("Image").unwrap().props["url"]
                .kind
                .is_structural()
        );
    }

    #[test]
    fn references_cover_ids_lists_templates_and_nested_objects() {
        let catalog = Catalog::basic();

        let card = Component::new("c", "Card").with("child", json!("inner"));
        assert_eq!(
            catalog.references(&card),
            vec![ComponentRef {
                location: "child".into(),
                id: "inner".into()
            }]
        );

        let row = Component::new("r", "Row").with("children", json!(["a", "b"]));
        let ids: Vec<String> = catalog.references(&row).into_iter().map(|r| r.id).collect();
        assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);

        let list = Component::new("l", "List")
            .with("children", json!({"componentId": "tpl", "path": "/items"}));
        assert_eq!(
            catalog.references(&list),
            vec![ComponentRef {
                location: "children.componentId".into(),
                id: "tpl".into()
            }]
        );

        let tabs = Component::new("t", "Tabs").with(
            "tabs",
            json!([{"title": "One", "child": "p1"}, {"title": "Two", "child": "p2"}]),
        );
        let locations: Vec<String> = catalog
            .references(&tabs)
            .into_iter()
            .map(|r| r.location)
            .collect();
        assert_eq!(
            locations,
            vec!["tabs[0].child".to_string(), "tabs[1].child".to_string()]
        );
    }

    #[test]
    fn unknown_component_types_yield_no_references() {
        let catalog = Catalog::basic();
        let mystery = Component::new("m", "Sparkline").with("child", json!("x"));
        assert!(catalog.references(&mystery).is_empty());
    }

    #[test]
    fn from_schema_merges_all_of_and_detects_ref_kinds() {
        let schema = json!({
            "catalogId": "test",
            "components": {
                "Panel": {
                    "type": "object",
                    "allOf": [
                        {"$ref": "common_types.json#/$defs/ComponentCommon"},
                        {
                            "type": "object",
                            "properties": {
                                "component": {"const": "Panel"},
                                "children": {"$ref": "common_types.json#/$defs/ChildList"},
                                "header": {"$ref": "common_types.json#/$defs/ComponentId"},
                                "title": {"type": "string"}
                            },
                            "required": ["component", "children"]
                        }
                    ]
                }
            },
            "functions": {"now": {"returnType": "string"}}
        });
        let catalog = Catalog::from_schema(&schema).unwrap();
        let panel = catalog.component("Panel").unwrap();
        assert_eq!(panel.props["children"].kind, PropKind::ChildList);
        assert_eq!(panel.props["header"].kind, PropKind::ComponentRef);
        assert_eq!(panel.props["title"].kind, PropKind::Value);
        assert_eq!(panel.required, vec!["children"]);
        assert_eq!(
            catalog.functions["now"].return_type.as_deref(),
            Some("string")
        );
    }

    #[test]
    fn from_schema_reads_the_type_each_property_declares() {
        let schema = json!({
            "catalogId": "test",
            "components": {
                "Chart": {
                    "type": "object",
                    "allOf": [
                        {
                            "type": "object",
                            "properties": {
                                "columns": {"type": "integer"},
                                "ratio": {"type": "number"},
                                "stacked": {"type": "boolean"},
                                "series": {"type": "array"},
                                "legend": {"type": "object"}
                            }
                        },
                        {
                            "type": "object",
                            "properties": {
                                "title": {"$ref": "common_types.json#/$defs/DynamicString"},
                                "caption": {"anyOf": [{"type": "string"}, {"type": "number"}]},
                                "anything": {"description": "no type at all"}
                            }
                        }
                    ]
                }
            }
        });
        let catalog = Catalog::from_schema(&schema).unwrap();
        let chart = catalog.component("Chart").unwrap();
        let type_of = |name: &str| chart.props[name].value_type;
        assert_eq!(type_of("columns"), PropType::Integer);
        assert_eq!(type_of("ratio"), PropType::Number);
        assert_eq!(type_of("stacked"), PropType::Boolean);
        assert_eq!(type_of("series"), PropType::Array);
        assert_eq!(type_of("legend"), PropType::Object);
        // A `Dynamic*` reference is its underlying type; a binding in its place
        // is skipped by the validator rather than described here.
        assert_eq!(type_of("title"), PropType::String);
        // Alternatives that disagree, and a schema with no type at all, pin
        // nothing rather than guessing.
        assert_eq!(type_of("caption"), PropType::Unconstrained);
        assert_eq!(type_of("anything"), PropType::Unconstrained);
    }

    #[test]
    fn from_schema_reads_an_enum_wherever_the_schema_puts_it() {
        let binding = "common_types.json#/$defs/DataBinding";
        let schema = json!({
            "catalogId": "test",
            "components": {
                "Badge": {
                    "type": "object",
                    "properties": {
                        // The property's own.
                        "tone": {"type": "string", "enum": ["info", "warn"]},
                        // One alternative of several, beside an object and a
                        // binding: the shape the specification gives `Icon.name`.
                        "icon": {"oneOf": [
                            {"type": "string", "enum": ["check", "close"]},
                            {"type": "object", "properties": {"svgPath": {"type": "string"}}},
                            {"$ref": binding}
                        ]},
                        // Spread over two arms, with a `const` for a third.
                        "size": {"anyOf": [
                            {"enum": ["small", "medium"]},
                            {"enum": ["medium", "large"]},
                            {"const": "auto"},
                            {"type": "number"}
                        ]},
                        // Inside an `allOf` wrapper, as a described property is.
                        "shape": {"allOf": [
                            {"description": "Outline."},
                            {"oneOf": [{"enum": ["round", "square"]}, {"$ref": binding}]}
                        ]},
                        // An arm that takes any string rules nothing out.
                        "label": {"oneOf": [{"enum": ["ok"]}, {"type": "string"}]},
                        "caption": {"oneOf": [
                            {"enum": ["ok"]},
                            {"$ref": "common_types.json#/$defs/DynamicString"}
                        ]},
                        // Nor does one this summary cannot read.
                        "kind": {"oneOf": [{"enum": ["ok"]}, {"$ref": "#/$defs/Custom"}]},
                        "note": {"oneOf": [{"enum": ["ok"]}, {"description": "anything"}]},
                        // Two keywords of one schema both have to hold.
                        "both": {"enum": ["a", "b", "c"], "oneOf": [{"enum": ["b", "c", "d"]}]},
                        // No string satisfies these, so there is no list.
                        "count": {"type": "integer"},
                        "flags": {"oneOf": [{"type": "array"}, {"$ref": binding}]}
                    }
                }
            }
        });
        let catalog = Catalog::from_schema(&schema).unwrap();
        let badge = catalog.component("Badge").unwrap();
        let values = |name: &str| -> Vec<&str> {
            badge.props[name]
                .enum_values
                .iter()
                .map(String::as_str)
                .collect()
        };
        assert_eq!(values("tone"), ["info", "warn"]);
        assert_eq!(values("icon"), ["check", "close"]);
        assert_eq!(values("size"), ["small", "medium", "large", "auto"]);
        assert_eq!(values("shape"), ["round", "square"]);
        assert_eq!(values("both"), ["b", "c"]);
        for open in ["label", "caption", "kind", "note", "count", "flags"] {
            assert!(values(open).is_empty(), "{open} lists {:?}", values(open));
        }
    }

    #[test]
    fn the_basic_catalogs_list_the_icon_names_their_documents_do() {
        // `Icon.name` is the one enum that is not the property's own: it sits
        // in the string arm of a `oneOf` beside `{svgPath}` and a binding. The
        // parser has to lift it from there like any other. The v0.9 family
        // composes `Icon` with `allOf`; v1.0 declares it flat.
        for &version in A2uiVersion::ALL {
            let catalog = Catalog::basic_for(version);
            let document = catalog.document().expect("a basic catalog is a document");
            let listed = [
                "/components/Icon/allOf/2/properties/name/oneOf/0/enum",
                "/components/Icon/properties/name/oneOf/0/enum",
            ]
            .iter()
            .find_map(|pointer| string_list(document.pointer(pointer)))
            .unwrap_or_else(|| panic!("the {version} document lists the icon names"));
            assert_eq!(
                catalog.component("Icon").unwrap().props["name"].enum_values,
                listed,
                "{version}"
            );
            assert_eq!(listed, BASIC_ICON_NAMES, "{version}");
        }
    }

    #[test]
    fn from_schema_treats_bare_strings_as_data_not_links() {
        // Per spec: a raw string type is static text, so its target is never checked.
        let schema = json!({
            "catalogId": "loose",
            "components": {
                "Card": {"type": "object", "properties": {"child": {"type": "string"}}}
            }
        });
        let catalog = Catalog::from_schema(&schema).unwrap();
        assert_eq!(
            catalog.component("Card").unwrap().props["child"].kind,
            PropKind::Value
        );
        let card = Component::new("root", "Card").with("child", json!("missing"));
        assert!(catalog.references(&card).is_empty());
    }

    #[test]
    fn from_schema_requires_a_catalog_id() {
        assert!(Catalog::from_schema(&json!({"components": {}})).is_err());
        assert!(Catalog::from_schema(&json!("nope")).is_err());
    }

    #[test]
    fn composition_constraints_flag_bad_parents_and_children() {
        let schema = json!({
            "catalogId": "menu",
            "components": {
                "AppLayout": {
                    "type": "object",
                    "allowedParents": ["Surface"],
                    "properties": {"child": {"$ref": "#/$defs/ComponentId"}}
                },
                "Menu": {
                    "type": "object",
                    "allowedChildren": ["MenuItem"],
                    "properties": {"children": {"$ref": "#/$defs/ChildList"}}
                },
                "MenuItem": {"type": "object", "allowedParents": ["Menu"]},
                "Text": {"type": "object"}
            }
        });
        let catalog = Catalog::from_schema(&schema).unwrap();

        let good = vec![
            Component::new(ROOT_ID, "AppLayout").with("child", json!("m")),
            Component::new("m", "Menu").with("children", json!(["i"])),
            Component::new("i", "MenuItem"),
        ];
        assert!(catalog.composition_violations(&good).is_empty());

        let bad = vec![
            Component::new(ROOT_ID, "Menu").with("children", json!(["t"])),
            Component::new("t", "Text"),
        ];
        let violations = catalog.composition_violations(&bad);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].code, CompositionCode::UnallowedChild);
        assert_eq!(violations[0].path, "components[0].children[0]");

        let misplaced = vec![
            Component::new(ROOT_ID, "Menu").with("children", json!(["a"])),
            Component::new("a", "AppLayout"),
        ];
        let violations = catalog.composition_violations(&misplaced);
        assert!(
            violations
                .iter()
                .any(|v| v.code == CompositionCode::UnallowedParent)
        );
    }

    #[test]
    fn root_must_satisfy_its_own_allowed_parents() {
        let schema = json!({
            "catalogId": "menu",
            "components": {"MenuItem": {"type": "object", "allowedParents": ["Menu"]}}
        });
        let catalog = Catalog::from_schema(&schema).unwrap();
        let violations = catalog.composition_violations(&[Component::new(ROOT_ID, "MenuItem")]);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].code, CompositionCode::UnallowedParent);
        assert_eq!(violations[0].path, "components[0].component");
    }

    #[test]
    fn basic_catalog_declares_no_composition_constraints() {
        let catalog = Catalog::basic();
        let components = vec![
            Component::new(ROOT_ID, "Card").with("child", json!("t")),
            Component::new("t", "Text").with("text", json!("hi")),
        ];
        assert!(catalog.composition_violations(&components).is_empty());
    }
}
