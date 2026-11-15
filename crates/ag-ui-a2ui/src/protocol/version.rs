//! The protocol version axis.
//!
//! [`A2uiVersion`] is the one place that knows which A2UI versions exist and
//! how each is spelled on the wire. Nothing else in the crate compares a
//! version string: a `version` field is read with [`A2uiVersion::parse`] and
//! written with [`A2uiVersion::as_str`].
//!
//! The versions come in two wire families. `v0.9` and `v0.9.1` share one; `1.0`
//! is the other. A message says which it is written in, and one set of types
//! carries both: nothing in this crate rewrites a message of one family into
//! the other. What differs from one family to the next is a method here, so the
//! rest of the crate asks the version a question instead of matching on it:
//!
//! | Question | Method |
//! |---|---|
//! | Which operations may a message carry? | [`A2uiVersion::operations`] |
//! | Which fields does an operation's payload have, and which are required? | `A2uiVersion::envelope` (crate-internal) |
//! | May a message of that version act on a surface created under this one? | [`A2uiVersion::accepts`] |
//! | How is a data binding written, and a function call? | [`A2uiVersion::path_key`], [`A2uiVersion::call_key`], [`A2uiVersion::path`], [`A2uiVersion::call`] |
//! | Which keys of a component are its envelope and not its properties? | [`A2uiVersion::component_keys`] |
//! | Is a leading `@` reserved in a dynamic value? | [`A2uiVersion::reserves_at_keys`] |
//! | Must a component's id and type be Unicode identifiers, and is a catalog held to the rules v1.0 gave catalogs? | [`A2uiVersion::names_are_identifiers`] |
//! | May a catalog keep a function from the renderer? | [`A2uiVersion::restricts_function_callers`] |
//! | Is a function call resolved when a message is read, and `@index` held to a template? | [`A2uiVersion::resolves_function_calls`] |
//! | Does a `null` value in `updateDataModel` remove, or is it stored? | [`A2uiVersion::null_removes`] |
//! | Does removing a path that holds nothing still create what leads to it? | [`A2uiVersion::removal_pads`] |
//! | May a surface mix catalogs, each component naming its own? | [`A2uiVersion::mixes_catalogs`] |
//! | Which basic catalog is published for it? | [`A2uiVersion::basic_catalog_id`] |
//! | Under which metadata keys do capabilities and the data model travel? | [`A2uiVersion::capabilities_key`], [`A2uiVersion::capabilities_entry`], [`A2uiVersion::data_model_key`] |
//!
//! Adding a version is therefore a change to this file: a variant, its
//! discriminator, and an arm in each method.
//!
//! ```
//! use ag_ui_a2ui::{A2uiVersion, Operation};
//! use serde_json::json;
//!
//! // Reading and naming.
//! let version = A2uiVersion::parse("v0.9.1").expect("a version this crate speaks");
//! assert_eq!(version.as_str(), "v0.9.1");
//! assert_eq!(A2uiVersion::parse("1.0"), Some(A2uiVersion::V1_0));
//! assert_eq!(A2uiVersion::parse("v0.8"), None);
//!
//! // The versions and the operations are slices, so either list can grow.
//! let versions: &'static [A2uiVersion] = A2uiVersion::ALL;
//! let operations: &'static [Operation] = Operation::ALL;
//! assert!(versions.contains(&A2uiVersion::DEFAULT));
//! assert_eq!(version.operations(), Operation::SURFACE);
//! assert_eq!(A2uiVersion::V1_0.operations(), operations);
//!
//! // A version-dependent question is a method, not a string comparison.
//! assert!(A2uiVersion::V0_9.accepts(version), "v0.9.1 may update a v0.9 surface");
//! assert!(!A2uiVersion::V0_9.accepts(A2uiVersion::V1_0), "1.0 is another wire format");
//! assert_eq!(version.path("/user/name"), json!({"path": "/user/name"}));
//! assert_eq!(A2uiVersion::V1_0.path("/user/name"), json!({"@path": "/user/name"}));
//! ```

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::catalog::PropType;
use super::operation::{EnvelopeField, Operation};
use crate::error::Error;

/// The A2UI versions this crate speaks.
///
/// Non-exhaustive, so that a version can be added without breaking a `match`
/// written outside this crate: such a `match` needs a wildcard arm. Ask the
/// version a question ([`A2uiVersion::operations`], [`A2uiVersion::accepts`],
/// [`A2uiVersion::path_key`]) rather than matching on it where one fits.
///
/// ```compile_fail,E0004
/// use ag_ui_a2ui::A2uiVersion;
///
/// // No wildcard arm. This would stop compiling the day a version is added,
/// // so it does not compile today either.
/// fn discriminator(version: A2uiVersion) -> &'static str {
///     match version {
///         A2uiVersion::V0_9 => "v0.9",
///         A2uiVersion::V0_9_1 => "v0.9.1",
///         A2uiVersion::V1_0 => "1.0",
///     }
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum A2uiVersion {
    /// v0.9 wire discriminator, also accepted by the v0.9.1 schema.
    V0_9,
    /// v0.9.1 discriminator.
    V0_9_1,
    /// v1.0, whose discriminator is `"1.0"`, with no `v`.
    V1_0,
}

/// Versions that share one wire format and may meet on one surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    /// `v0.9` and `v0.9.1`: the v0.9.1 schema accepts either discriminator.
    V0_9,
    /// `1.0`.
    V1_0,
}

/// The keys of a component object that belong to its envelope, as one version
/// spells them. See [`A2uiVersion::component_keys`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ComponentKeys {
    /// The catalog the component is drawn from, where a surface may mix
    /// catalogs: `@catalogId` in v1.0. `None` before that, when a surface has
    /// one catalog.
    pub catalog_id: Option<&'static str>,
    /// Accessibility attributes: `accessibility`, or `@accessibility` in v1.0.
    pub accessibility: &'static str,
    /// The check rules of an input component: `checks`, or `@checks` in v1.0.
    pub checks: &'static str,
    /// Extension metadata: `@metadata` in v1.0. `None` before that.
    pub metadata: Option<&'static str>,
}

impl ComponentKeys {
    /// The keys this version has, in the order above.
    pub fn all(&self) -> impl Iterator<Item = &'static str> {
        [
            self.catalog_id,
            Some(self.accessibility),
            Some(self.checks),
            self.metadata,
        ]
        .into_iter()
        .flatten()
    }

    /// Whether `key` is one of the envelope keys.
    pub fn contains(&self, key: &str) -> bool {
        self.all().any(|own| own == key)
    }
}

impl A2uiVersion {
    /// Every version this crate speaks, oldest first.
    ///
    /// A slice rather than an array: an array's length is part of its type,
    /// so adding a version would change the type of this constant.
    pub const ALL: &'static [Self] = &[Self::V0_9, Self::V0_9_1, Self::V1_0];

    /// The version a constructor stamps when the caller names none: `v0.9`.
    ///
    /// It is not the newest ([`A2uiVersion::LATEST`]), on purpose. Nothing a
    /// caller wrote against v0.9 changes on the wire unless it names
    /// [`A2uiVersion::V1_0`]. At the commits this crate is written against, a
    /// v0.9 message is also what the rest of the stack expects by default:
    ///
    /// - The official renderer core (`@a2ui/web_core` 0.15.0, upstream commit
    ///   `3ed9601`) routes each message by its own `version`, and the
    ///   `version` option of its `MessageProcessor` defaults to `v0.9`.
    /// - AG-UI's A2UI middleware (`ag-ui-protocol/ag-ui` commit `2330368a`)
    ///   takes the protocol from the catalog's `protocolVersion` and falls
    ///   back to v0.9 when the catalog states none; its toolkit writes
    ///   `"version": "v0.9"`.
    ///
    /// A 1.0 message is painted where the application registers a 1.0 catalog,
    /// so 1.0 is a choice a caller makes together with its catalog.
    pub const DEFAULT: Self = Self::V0_9;

    /// The newest version this crate speaks.
    ///
    /// Named so that code which means "the newest" does not read
    /// [`A2uiVersion::DEFAULT`], which is something else.
    pub const LATEST: Self = Self::V1_0;

    /// The exact wire discriminator.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V0_9 => "v0.9",
            Self::V0_9_1 => "v0.9.1",
            Self::V1_0 => "1.0",
        }
    }

    /// The version as a number, without the `v` some discriminators carry:
    /// `0.9`, `0.9.1`, `1.0`.
    ///
    /// This is how a catalog document states its `protocolVersion`, how an
    /// A2A extension URI ends, and the key a capabilities object is filed
    /// under from v1.0 on.
    pub const fn number(self) -> &'static str {
        match self {
            Self::V0_9 => "0.9",
            Self::V0_9_1 => "0.9.1",
            Self::V1_0 => "1.0",
        }
    }

    /// Reads a wire discriminator.
    ///
    /// `None` for anything this crate does not speak, including spellings of a
    /// supported version the wire does not use (`0.9`, `V0.9`, `v1.0`).
    pub fn parse(discriminator: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|version| version.as_str() == discriminator)
    }

    /// Reads a version written as a number, with or without a leading `v`:
    /// `0.9.1`, `v0.9.1`, `1.0`, `v1.0`.
    ///
    /// For the places that are not a message's `version` field and spell a
    /// version their own way: a catalog's `protocolVersion`, an extension URI.
    pub fn from_number(text: &str) -> Option<Self> {
        let bare = text.strip_prefix('v').unwrap_or(text);
        Self::ALL
            .iter()
            .copied()
            .find(|version| version.number() == bare)
    }

    /// The operations a message of this version may carry, in envelope order:
    /// the four that change a surface, and from v1.0 the two that carry a
    /// function call.
    pub fn operations(self) -> &'static [Operation] {
        match self.family() {
            Family::V0_9 => Operation::SURFACE,
            Family::V1_0 => Operation::ALL,
        }
    }

    /// Whether an agent and a renderer call each other's functions with
    /// messages: `callRendererFunction` and `callAgentFunction`, and the two
    /// responses. From v1.0.
    pub const fn carries_function_calls(self) -> bool {
        match self.family() {
            Family::V0_9 => false,
            Family::V1_0 => true,
        }
    }

    /// Whether a message of version `message` may act on a surface created
    /// under this one.
    ///
    /// The v0.9.1 schema accepts either v0.9-family discriminator, so the two
    /// may be mixed on one surface. v1.0 is another wire format: a v1.0
    /// message cannot update a v0.9 surface, nor the reverse.
    pub fn accepts(self, message: Self) -> bool {
        self.family() == message.family()
    }

    /// The key that makes an object a data binding: `path`, or `@path` in
    /// v1.0.
    ///
    /// In v1.0 a plain `{"path": …}` is a literal map. A list template,
    /// `{"componentId": …, "path": …}`, keeps the bare `path` in every
    /// version: it is not a binding.
    pub const fn path_key(self) -> &'static str {
        match self.family() {
            Family::V0_9 => "path",
            Family::V1_0 => "@path",
        }
    }

    /// The key that makes an object a function call: `call`, or `@call` in
    /// v1.0.
    pub const fn call_key(self) -> &'static str {
        match self.family() {
            Family::V0_9 => "call",
            Family::V1_0 => "@call",
        }
    }

    /// A data binding to `pointer`, as this version writes one.
    ///
    /// ```
    /// use ag_ui_a2ui::{A2uiVersion, Component};
    ///
    /// let version = A2uiVersion::V1_0;
    /// let title = Component::new("title", "Text").with("text", version.path("/title"));
    /// assert_eq!(title.prop("text").unwrap()["@path"], "/title");
    /// ```
    pub fn path(self, pointer: impl Into<String>) -> Value {
        let mut binding = Map::new();
        binding.insert(self.path_key().to_string(), Value::String(pointer.into()));
        Value::Object(binding)
    }

    /// A call of the function `name`, as this version writes one. `args` is
    /// the object of named arguments; `Value::Null` writes a call with none.
    ///
    /// ```
    /// use ag_ui_a2ui::A2uiVersion;
    /// use serde_json::json;
    ///
    /// let call = A2uiVersion::V0_9.call("formatString", json!({"value": "Hello, ${/name}"}));
    /// assert_eq!(call, json!({"call": "formatString", "args": {"value": "Hello, ${/name}"}}));
    /// assert_eq!(A2uiVersion::V1_0.call("@index", json!(null)), json!({"@call": "@index"}));
    /// ```
    pub fn call(self, name: impl Into<String>, args: Value) -> Value {
        let mut call = Map::new();
        call.insert(self.call_key().to_string(), Value::String(name.into()));
        if !args.is_null() {
            call.insert("args".to_string(), args);
        }
        Value::Object(call)
    }

    /// Whether a key that starts with one `@` is reserved inside a dynamic
    /// value, so that a literal one is written doubled (`@@type`).
    ///
    /// True from v1.0. Before it `@` means nothing, and a component key that
    /// starts with one is an ordinary property a catalog may or may not
    /// declare.
    pub const fn reserves_at_keys(self) -> bool {
        match self.family() {
            Family::V0_9 => false,
            Family::V1_0 => true,
        }
    }

    /// Whether a component's `id` and `component` have to be identifiers as
    /// Unicode Standard Annex #31 defines one: `user_card`, not `user-card` or
    /// `1st_item`.
    ///
    /// From v1.0. Before it an id is any non-empty string.
    ///
    /// The same goes for the names a catalog gives its components and
    /// functions, and with it the other rules v1.0 holds a catalog document
    /// to when it is read; see
    /// [`Catalog::from_schema`](crate::Catalog::from_schema).
    pub const fn names_are_identifiers(self) -> bool {
        match self.family() {
            Family::V0_9 => false,
            Family::V1_0 => true,
        }
    }

    /// Whether a catalog's `allowedCallers` binds: a function it gives to the
    /// agent alone may then not stand in a component property or a local
    /// action, both of which a renderer evaluates.
    ///
    /// From v1.0. A v0.9 catalog may state `allowedCallers`, and it restricts
    /// nothing there.
    pub const fn restricts_function_callers(self) -> bool {
        match self.family() {
            Family::V0_9 => false,
            Family::V1_0 => true,
        }
    }

    /// Whether a function call is resolved when a message is read: its
    /// catalog found, its name looked up there, a `formatString` template
    /// parsed, and `@index` allowed only where a list template gives it an
    /// item to count.
    ///
    /// From v1.0, which says how a call finds its catalog and where `@index`
    /// may stand. Before it a call is the catalog schema's to describe and
    /// the renderer's to run.
    pub const fn resolves_function_calls(self) -> bool {
        match self.family() {
            Family::V0_9 => false,
            Family::V1_0 => true,
        }
    }

    /// The keys that are a component's envelope in another version and not in
    /// this one: `@catalogId`, `@accessibility`, `@checks` and `@metadata` on
    /// a surface older than v1.0, the bare `accessibility` and `checks` on a
    /// v1.0 one.
    ///
    /// On a component they are ordinary properties, which its catalog declares
    /// or does not.
    pub(crate) fn foreign_component_keys(self) -> Vec<&'static str> {
        let own = self.component_keys();
        let mut foreign = Vec::new();
        for other in Self::ALL {
            for key in other.component_keys().all() {
                if !own.contains(key) && !foreign.contains(&key) {
                    foreign.push(key);
                }
            }
        }
        foreign
    }

    /// The keys of a component object that are its envelope rather than
    /// properties its catalog declares.
    ///
    /// `id` and `component` are the same in every version and are not listed.
    pub const fn component_keys(self) -> ComponentKeys {
        match self.family() {
            Family::V0_9 => ComponentKeys {
                catalog_id: None,
                accessibility: "accessibility",
                checks: "checks",
                metadata: None,
            },
            Family::V1_0 => ComponentKeys {
                catalog_id: Some("@catalogId"),
                accessibility: "@accessibility",
                checks: "@checks",
                metadata: Some("@metadata"),
            },
        }
    }

    /// Whether `"value": null` in `updateDataModel` removes what is at the
    /// path.
    ///
    /// From v1.0 it does, and `value` is required. Before it, `null` is a
    /// value that is stored, and a message removes by leaving `value` out.
    pub const fn null_removes(self) -> bool {
        match self.family() {
            Family::V0_9 => false,
            Family::V1_0 => true,
        }
    }

    /// Whether removing a path that holds nothing still creates what leads to
    /// it: the containers on the way, and the slots of a list up to the index.
    ///
    /// Before v1.0 it does, which is what the renderer this workspace is
    /// tested against does with a v0.9 surface (`@a2ui/web_core` 0.11.0).
    /// From v1.0 such a removal changes nothing, and removing the root leaves
    /// an empty object.
    pub const fn removal_pads(self) -> bool {
        match self.family() {
            Family::V0_9 => true,
            Family::V1_0 => false,
        }
    }

    /// Whether a catalog's `allowedParents` and `allowedChildren` are part of
    /// the protocol, so that a [`Validator`](crate::validate::Validator) and a
    /// [`MessageProcessor`](crate::processor::MessageProcessor) refuse a
    /// surface that breaks them. From v1.0, whose catalog definition has both.
    ///
    /// In the v0.9 family they are this crate's own reading of a catalog:
    /// [`Catalog::composition_violations`](crate::Catalog::composition_violations)
    /// reports them to a caller that asks.
    pub const fn constrains_composition(self) -> bool {
        matches!(self, Self::V1_0)
    }

    /// Whether a surface may mix catalogs: `createSurface` need not name one,
    /// and a component or a function call may name its own.
    ///
    /// From v1.0. Before it a surface has exactly the catalog it was created
    /// with.
    pub const fn mixes_catalogs(self) -> bool {
        match self.family() {
            Family::V0_9 => false,
            Family::V1_0 => true,
        }
    }

    /// Whether `createSurface` may carry the surface's first `components` and
    /// `dataModel` itself. From v1.0.
    pub(crate) const fn creates_inline(self) -> bool {
        match self.family() {
            Family::V0_9 => false,
            Family::V1_0 => true,
        }
    }

    /// Whether `createSurface` carries a `theme`. The v0.9 family only: v1.0
    /// took the theme out of the protocol.
    pub(crate) const fn has_theme(self) -> bool {
        match self.family() {
            Family::V0_9 => true,
            Family::V1_0 => false,
        }
    }

    /// The `catalogId` of the basic catalog published for this version.
    ///
    /// v0.9 and v0.9.1 publish two documents under one id.
    pub const fn basic_catalog_id(self) -> &'static str {
        match self.family() {
            Family::V0_9 => "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json",
            Family::V1_0 => "https://a2ui.org/specification/v1_0/catalogs/basic/catalog.json",
        }
    }

    /// The transport metadata key a renderer's capabilities travel under:
    /// `a2uiClientCapabilities`, or `a2uiRendererCapabilities` from v1.0,
    /// which calls the two sides agent and renderer throughout.
    pub const fn capabilities_key(self) -> &'static str {
        match self.family() {
            Family::V0_9 => "a2uiClientCapabilities",
            Family::V1_0 => "a2uiRendererCapabilities",
        }
    }

    /// The key inside a capabilities object that holds this version's entry:
    /// `v0.9` for both v0.9 and v0.9.1, `1.0` for v1.0.
    pub const fn capabilities_entry(self) -> &'static str {
        match self.family() {
            Family::V0_9 => "v0.9",
            Family::V1_0 => "1.0",
        }
    }

    /// The transport metadata key a renderer's data model travels under, for
    /// a surface created with `sendDataModel`: `a2uiClientDataModel`, or
    /// `a2uiRendererDataModel` from v1.0.
    pub const fn data_model_key(self) -> &'static str {
        match self.family() {
            Family::V0_9 => "a2uiClientDataModel",
            Family::V1_0 => "a2uiRendererDataModel",
        }
    }

    /// The fields of an operation's payload in this version: name, JSON type,
    /// and whether the protocol requires it.
    ///
    /// Transcribed from the payload structs in [`crate::message`], which
    /// are the port of the wire format. Fields the specification leaves untyped
    /// (an `updateDataModel` value, a function's arguments) are simply absent:
    /// this is the envelope contract, not a schema for everything inside it.
    pub(crate) fn envelope(self, operation: Operation) -> &'static [EnvelopeField] {
        match self.family() {
            Family::V0_9 => match operation {
                Operation::CreateSurface => &[
                    ("surfaceId", PropType::String, true),
                    ("catalogId", PropType::String, true),
                    ("theme", PropType::Object, false),
                    ("sendDataModel", PropType::Boolean, false),
                ],
                Operation::UpdateComponents => &[
                    ("surfaceId", PropType::String, true),
                    ("components", PropType::Array, true),
                ],
                Operation::UpdateDataModel => &[
                    ("surfaceId", PropType::String, true),
                    ("path", PropType::String, false),
                ],
                Operation::DeleteSurface => &[("surfaceId", PropType::String, true)],
                // Not operations of this family.
                Operation::CallRendererFunction | Operation::AgentFunctionResponse => &[],
            },
            Family::V1_0 => match operation {
                Operation::CreateSurface => &[
                    ("surfaceId", PropType::String, true),
                    ("catalogId", PropType::String, false),
                    ("sendDataModel", PropType::Boolean, false),
                    ("components", PropType::Array, false),
                    ("dataModel", PropType::Object, false),
                    ("metadata", PropType::Object, false),
                ],
                Operation::UpdateComponents => &[
                    ("surfaceId", PropType::String, true),
                    ("components", PropType::Array, true),
                ],
                Operation::UpdateDataModel => &[
                    ("surfaceId", PropType::String, true),
                    ("path", PropType::String, false),
                ],
                Operation::DeleteSurface => &[("surfaceId", PropType::String, true)],
                Operation::CallRendererFunction => &[
                    ("functionCallId", PropType::String, true),
                    ("callFunction", PropType::Object, true),
                    ("surfaceId", PropType::String, false),
                ],
                Operation::AgentFunctionResponse => &[
                    ("functionCallId", PropType::String, true),
                    ("error", PropType::Object, false),
                ],
            },
        }
    }

    const fn family(self) -> Family {
        match self {
            Self::V0_9 | Self::V0_9_1 => Family::V0_9,
            Self::V1_0 => Family::V1_0,
        }
    }
}

impl fmt::Display for A2uiVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for A2uiVersion {
    type Err = Error;

    /// # Errors
    ///
    /// Returns [`Error::Parse`] for a discriminator [`A2uiVersion::parse`]
    /// does not recognize.
    fn from_str(discriminator: &str) -> Result<Self, Self::Err> {
        Self::parse(discriminator).ok_or_else(|| Error::parse("unsupported A2UI version"))
    }
}

impl Serialize for A2uiVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for A2uiVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let discriminator = String::deserialize(deserializer)?;
        Self::parse(&discriminator)
            .ok_or_else(|| serde::de::Error::custom("unsupported A2UI version"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_version_round_trips_through_its_discriminator() {
        for &version in A2uiVersion::ALL {
            assert_eq!(A2uiVersion::parse(version.as_str()), Some(version));
            assert_eq!(version.as_str().parse::<A2uiVersion>().unwrap(), version);
            assert_eq!(version.to_string(), version.as_str());
            // serde spells a version exactly as `as_str` does: one list of names.
            assert_eq!(serde_json::to_value(version).unwrap(), version.as_str());
            assert_eq!(
                serde_json::from_value::<A2uiVersion>(json!(version.as_str())).unwrap(),
                version
            );
            // A number is read with or without the `v`, whatever the wire uses.
            assert_eq!(A2uiVersion::from_number(version.number()), Some(version));
            assert_eq!(
                A2uiVersion::from_number(&format!("v{}", version.number())),
                Some(version)
            );
        }
    }

    #[test]
    fn a_discriminator_this_crate_does_not_speak_is_not_a_version() {
        for other in [
            "", "0.9", "V0.9", "v0.8", "v0.9.2", "v1.0", "1", "1.0.0", "1.1", " v0.9",
        ] {
            assert_eq!(A2uiVersion::parse(other), None, "{other:?}");
            assert!(matches!(
                other.parse::<A2uiVersion>(),
                Err(Error::Parse(reason)) if reason == "unsupported A2UI version"
            ));
            assert!(serde_json::from_value::<A2uiVersion>(json!(other)).is_err());
        }
        assert!(serde_json::from_value::<A2uiVersion>(json!(0.9)).is_err());
        assert!(serde_json::from_value::<A2uiVersion>(json!(1.0)).is_err());
        for other in ["", "0.8", "v0.8", "1", "1.0.0", "V1.0", "vv1.0"] {
            assert_eq!(A2uiVersion::from_number(other), None, "{other:?}");
        }
    }

    #[test]
    fn the_default_is_v0_9_and_the_latest_is_not_the_default() {
        assert!(A2uiVersion::ALL.contains(&A2uiVersion::DEFAULT));
        assert_eq!(A2uiVersion::DEFAULT, A2uiVersion::V0_9);
        assert_eq!(crate::constants::PROTOCOL_VERSION, "v0.9");
        assert_eq!(
            crate::constants::PROTOCOL_VERSION,
            A2uiVersion::DEFAULT.as_str()
        );
        assert_eq!(A2uiVersion::ALL.last(), Some(&A2uiVersion::LATEST));
        assert_eq!(A2uiVersion::LATEST.as_str(), "1.0");
    }

    #[test]
    fn a_family_shares_surfaces_and_no_two_families_do() {
        let v0_9 = [A2uiVersion::V0_9, A2uiVersion::V0_9_1];
        for &created in A2uiVersion::ALL {
            for &message in A2uiVersion::ALL {
                let same_family = v0_9.contains(&created) == v0_9.contains(&message);
                assert_eq!(
                    created.accepts(message),
                    same_family,
                    "{created} surface, {message}"
                );
            }
            assert_eq!(
                created.operations(),
                if created.carries_function_calls() {
                    Operation::ALL
                } else {
                    Operation::SURFACE
                }
            );
            for &operation in Operation::SURFACE {
                let fields = created.envelope(operation);
                assert_eq!(
                    fields[0],
                    ("surfaceId", PropType::String, true),
                    "every {created} operation addresses a surface"
                );
            }
        }
    }

    #[test]
    fn each_family_spells_bindings_and_the_component_envelope_its_own_way() {
        for version in [A2uiVersion::V0_9, A2uiVersion::V0_9_1] {
            assert_eq!(version.path("/a"), json!({"path": "/a"}));
            assert_eq!(
                version.call("f", json!({"x": 1})),
                json!({"call": "f", "args": {"x": 1}})
            );
            assert!(!version.reserves_at_keys());
            assert!(!version.names_are_identifiers());
            assert!(!version.restricts_function_callers());
            assert!(!version.resolves_function_calls());
            assert_eq!(
                version.foreign_component_keys(),
                ["@catalogId", "@accessibility", "@checks", "@metadata"]
            );
            assert!(!version.null_removes());
            assert!(version.removal_pads());
            assert!(!version.mixes_catalogs());
            let keys = version.component_keys();
            assert_eq!(keys.all().collect::<Vec<_>>(), ["accessibility", "checks"]);
            assert!(!keys.contains("@checks"));
        }
        let version = A2uiVersion::V1_0;
        assert_eq!(version.path("/a"), json!({"@path": "/a"}));
        assert_eq!(version.call("f", json!(null)), json!({"@call": "f"}));
        assert!(version.reserves_at_keys());
        assert!(version.names_are_identifiers());
        assert!(version.restricts_function_callers());
        assert!(version.resolves_function_calls());
        assert_eq!(
            version.foreign_component_keys(),
            ["accessibility", "checks"]
        );
        assert!(version.null_removes());
        assert!(!version.removal_pads());
        assert!(version.mixes_catalogs());
        let keys = version.component_keys();
        assert_eq!(
            keys.all().collect::<Vec<_>>(),
            ["@catalogId", "@accessibility", "@checks", "@metadata"]
        );
        assert!(!keys.contains("checks"));
    }

    #[test]
    fn creating_a_surface_asks_for_a_catalog_only_before_v1_0() {
        let required = |version: A2uiVersion, field: &str| {
            version
                .envelope(Operation::CreateSurface)
                .iter()
                .find(|(name, _, _)| *name == field)
                .map(|(_, _, required)| *required)
        };
        assert_eq!(required(A2uiVersion::V0_9, "catalogId"), Some(true));
        assert_eq!(required(A2uiVersion::V0_9, "theme"), Some(false));
        assert_eq!(required(A2uiVersion::V0_9, "components"), None);
        assert_eq!(required(A2uiVersion::V1_0, "catalogId"), Some(false));
        assert_eq!(required(A2uiVersion::V1_0, "theme"), None);
        assert_eq!(required(A2uiVersion::V1_0, "components"), Some(false));
        assert_ne!(
            A2uiVersion::V0_9.basic_catalog_id(),
            A2uiVersion::V1_0.basic_catalog_id()
        );
        // v1.0 says renderer where the v0.9 family says client.
        for version in [A2uiVersion::V0_9, A2uiVersion::V0_9_1] {
            assert_eq!(version.capabilities_key(), "a2uiClientCapabilities");
            assert_eq!(version.capabilities_entry(), "v0.9");
            assert_eq!(version.data_model_key(), "a2uiClientDataModel");
        }
        assert_eq!(
            A2uiVersion::V1_0.capabilities_key(),
            "a2uiRendererCapabilities"
        );
        assert_eq!(A2uiVersion::V1_0.capabilities_entry(), "1.0");
        assert_eq!(A2uiVersion::V1_0.data_model_key(), "a2uiRendererDataModel");
    }
}
