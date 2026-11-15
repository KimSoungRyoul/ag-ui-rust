//! The catalogs an agent registers, and what it does to them before a model
//! or a validator sees them.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use serde_json::Value;

use super::ordered::Ordered;
use crate::error::{Error, Result};
use crate::protocol::catalog::Catalog;
use crate::protocol::schema::{SchemaBundle, prune_components, prune_functions};
use crate::protocol::version::A2uiVersion;

/// One catalog, read under one protocol version.
///
/// Holds the catalog as a JSON document and as a [`Catalog`]. The document is
/// what a prompt shows the model and what a JSON Schema validator compiles;
/// the summary is what the semantic validator reads. The summary is read from
/// the document, at the load and again after every transformer, so the two
/// always agree, and [`Catalog::document`] of the summary is this document.
///
/// The document is held once: the summary keeps the document it was read
/// from, and this is that one. Cloning is cheap, since both are shared, and a
/// renderer state that checks against the catalog shares the summary too.
///
/// # The order a prompt shows
///
/// A prompt writes the catalog the way its author did: `title` and
/// `description` before `components`, each component's properties in the order
/// the author gave them. A JSON value does not keep that order, so a document
/// read from text ([`CatalogDocument::parse`], [`CatalogDocument::read`],
/// [`CatalogDocument::basic`]) keeps it beside the value. One made from a
/// value ([`CatalogDocument::load`]) is shown in the order the value has.
#[derive(Debug, Clone)]
pub struct CatalogDocument {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    /// The allocation `catalog` holds as the document it was read from.
    document: Arc<Value>,
    /// The same document, in the order it is to be written in.
    written: Arc<Ordered>,
    catalog: Arc<Catalog>,
    version: A2uiVersion,
}

/// Two documents are equal when they are the same JSON under the same version.
impl PartialEq for CatalogDocument {
    fn eq(&self, other: &Self) -> bool {
        self.inner.version == other.inner.version && self.inner.document == other.inner.document
    }
}

impl CatalogDocument {
    /// Reads a catalog document that states its own `catalogId` and
    /// `protocolVersion`.
    ///
    /// # Errors
    ///
    /// As [`CatalogDocument::load`], with nothing to fall back on.
    pub fn new(document: Value) -> Result<Self> {
        Self::load(document, None, None)
    }

    /// Reads a catalog document, with what its reader knows about it.
    ///
    /// `catalog_id` and `version` are each both a default and an assertion.
    /// Where the document states the same field the two must agree; where it
    /// states nothing, the argument is used. Neither has a fallback: a catalog
    /// nothing names cannot be addressed in `supportedCatalogIds` or
    /// `createSurface`, and a document read under the wrong version
    /// mis-validates every message written against it.
    ///
    /// A document spells its version as the bare number (`"0.9.1"`); the wire
    /// spelling (`"v0.9.1"`) is read too.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Catalog`] when the document is not an object, when its
    /// id or version is missing, malformed or contradicts the argument, when
    /// it targets a protocol version this crate does not speak, or when its
    /// `components` or `functions` cannot be read.
    pub fn load(
        document: Value,
        catalog_id: Option<&str>,
        version: Option<A2uiVersion>,
    ) -> Result<Self> {
        let written = Ordered::from_value(&document);
        Self::of_written(written, document, catalog_id, version)
    }

    /// Reads a catalog document from JSON text. As [`CatalogDocument::load`],
    /// and a prompt shows the catalog in the order `text` is written in.
    ///
    /// ```
    /// use ag_ui_a2ui::agent::CatalogDocument;
    ///
    /// let catalog = CatalogDocument::parse(
    ///     r#"{"title": "Cards", "catalogId": "example.com:cards", "protocolVersion": "1.0",
    ///         "components": {"Card": {"type": "object"}}}"#,
    ///     None,
    ///     None,
    /// )?;
    /// assert_eq!(catalog.catalog_id(), "example.com:cards");
    /// assert!(catalog.written().starts_with(r#"{"title":"Cards","catalogId":"#));
    /// # Ok::<(), ag_ui_a2ui::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::Catalog`] when `text` is not JSON, and what
    /// [`CatalogDocument::load`] returns for its contents.
    pub fn parse(
        text: &str,
        catalog_id: Option<&str>,
        version: Option<A2uiVersion>,
    ) -> Result<Self> {
        let written = Ordered::parse(text).map_err(|error| {
            Error::catalog(format!("the catalog document is not valid JSON: {error}"))
        })?;
        let document = written.to_value();
        Self::of_written(written, document, catalog_id, version)
    }

    /// A document, as a value and in the order it is to be written in.
    fn of_written(
        mut written: Ordered,
        document: Value,
        catalog_id: Option<&str>,
        version: Option<A2uiVersion>,
    ) -> Result<Self> {
        let Value::Object(mut document) = document else {
            return Err(Error::catalog("a catalog document must be a JSON object"));
        };

        let id = match (document.get("catalogId"), catalog_id) {
            (Some(Value::Null) | None, Some(given)) if !given.is_empty() => given.to_string(),
            (Some(Value::Null) | None, Some(_)) => {
                return Err(Error::catalog("the catalog id given must not be empty"));
            }
            (Some(Value::Null) | None, None) => {
                return Err(Error::catalog(
                    "the catalog document declares no 'catalogId' and none was given, so \
                     nothing names the catalog",
                ));
            }
            (Some(Value::String(declared)), given) if !declared.is_empty() => {
                if given.is_some_and(|given| given != declared) {
                    return Err(Error::catalog(format!(
                        "the catalog document declares catalog id '{declared}', but it was \
                         loaded as '{}'",
                        given.unwrap_or_default()
                    )));
                }
                declared.clone()
            }
            (Some(_), _) => {
                return Err(Error::catalog(
                    "the catalog document declares a 'catalogId' that is not a non-empty string",
                ));
            }
        };

        let version = match (document.get("protocolVersion"), version) {
            (Some(Value::Null) | None, Some(given)) => given,
            (Some(Value::Null) | None, None) => {
                return Err(Error::catalog(format!(
                    "catalog '{id}' declares no 'protocolVersion' and none was given, so \
                     nothing states which protocol it is written against"
                )));
            }
            (Some(Value::String(declared)), given) => {
                let Some(stated) = document_version(declared) else {
                    return Err(Error::catalog(format!(
                        "catalog '{id}' targets protocol version '{declared}', which this \
                         crate does not speak"
                    )));
                };
                if given.is_some_and(|given| given != stated) {
                    return Err(Error::catalog(format!(
                        "catalog '{id}' declares protocol version '{declared}', but it was \
                         loaded as {}",
                        given.map(A2uiVersion::as_str).unwrap_or_default()
                    )));
                }
                stated
            }
            (Some(_), _) => {
                return Err(Error::catalog(format!(
                    "catalog '{id}' declares a 'protocolVersion' that is not a string"
                )));
            }
        };

        if document
            .get("components")
            .is_some_and(|components| !components.is_object())
        {
            return Err(Error::catalog(format!(
                "catalog '{id}': 'components' must be an object keyed by component name"
            )));
        }
        if document
            .get("functions")
            .is_some_and(|functions| !functions.is_object() && !functions.is_array())
        {
            return Err(Error::catalog(format!(
                "catalog '{id}': 'functions' must be an object keyed by function name, or an \
                 array of function definitions"
            )));
        }

        // A catalog named by its reader is no different from one that named
        // itself: the document a model is shown carries the id either way.
        if document.get("catalogId").and_then(Value::as_str) != Some(id.as_str()) {
            written.set_first("catalogId", Ordered::String(id.clone()));
        }
        document.insert("catalogId".into(), Value::String(id));
        let document = Value::Object(document);
        // A document that states no version is one of the version it was
        // given, and is held to that version's rules.
        let catalog = Catalog::from_schema_as(&document, version)?;
        Ok(Self::of(catalog, document, written, version))
    }

    /// `catalog` as read from `document`, which it keeps; that copy is the
    /// one held.
    fn of(catalog: Catalog, document: Value, written: Ordered, version: A2uiVersion) -> Self {
        let document = catalog
            .shared_document()
            .cloned()
            .unwrap_or_else(|| Arc::new(document));
        Self {
            inner: Arc::new(Inner {
                document,
                written: Arc::new(written),
                catalog: Arc::new(catalog),
                version,
            }),
        }
    }

    /// Reads a catalog document from a file. See [`CatalogDocument::load`]
    /// for `catalog_id` and `version`.
    ///
    /// This is the only place the agent layer touches the filesystem.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Catalog`] when the file cannot be read or is not JSON,
    /// and whatever [`CatalogDocument::load`] returns for its contents.
    pub fn read(
        path: impl AsRef<Path>,
        catalog_id: Option<&str>,
        version: Option<A2uiVersion>,
    ) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|error| {
            Error::catalog(format!(
                "cannot read the catalog document '{}': {error}",
                path.display()
            ))
        })?;
        let written = Ordered::parse(&text).map_err(|error| {
            Error::catalog(format!(
                "the catalog document '{}' is not valid JSON: {error}",
                path.display()
            ))
        })?;
        let document = written.to_value();
        Self::of_written(written, document, catalog_id, version)
    }

    /// The official basic catalog of `version`, as pinned in this crate. No
    /// network or filesystem access is performed.
    ///
    /// Each version has a document of its own: the v0.9 one declares
    /// `List.listStyle`, which the v0.9.1 one does not.
    ///
    /// # Errors
    ///
    /// Returns an error only if the embedded document does not parse.
    pub fn basic(version: A2uiVersion) -> Result<Self> {
        let [_, _, catalog] = SchemaBundle::texts(version);
        Self::parse(catalog, None, Some(version))
    }

    /// The id agent and renderer negotiate on.
    pub fn catalog_id(&self) -> &str {
        &self.inner.catalog.catalog_id
    }

    /// The protocol version the catalog is written against.
    pub fn version(&self) -> A2uiVersion {
        self.inner.version
    }

    /// The catalog as the semantic validator reads it.
    pub fn catalog(&self) -> &Catalog {
        &self.inner.catalog
    }

    /// The catalog as JSON: what a validator compiles, and what
    /// [`CatalogDocument::written`] writes.
    pub fn document(&self) -> &Value {
        &self.inner.document
    }

    /// The catalog as the compact JSON text a prompt shows the model: the
    /// document, in the order it was read in.
    pub fn written(&self) -> String {
        self.inner.written.compact()
    }

    /// The summary, shared: what a message processor registers.
    pub(crate) fn shared_catalog(&self) -> Arc<Catalog> {
        self.inner.catalog.clone()
    }

    /// A copy whose document `change` has rewritten, by taking entries out of
    /// `section` (`components` or `functions`).
    ///
    /// The summary is read again from the rewritten document, as it was at
    /// the load, so the two cannot disagree and [`Catalog::document`] is the
    /// document this value shows.
    ///
    /// A catalog lists its entries a second time, in the union the envelope
    /// reaches them through: `$defs/anyComponent`, `$defs/anyFunction`. Once
    /// an entry is gone the union an author wrote no longer describes the
    /// catalog, so it is written again from the entries that are left, in
    /// their order, the way the reference SDK writes one (`_component_union`
    /// and `_function_union` in `a2ui_core`'s `catalog.py`): the references,
    /// with the `component` discriminator for components and nothing else
    /// for functions, or a schema nothing matches when no entry is left. A
    /// rule that removes nothing leaves the union as it was written.
    fn changed(&self, section: &str, change: impl FnOnce(&mut Value)) -> Self {
        let entries = |document: &Value| -> Option<Vec<String>> {
            Some(
                document
                    .get(section)?
                    .as_object()?
                    .keys()
                    .cloned()
                    .collect(),
            )
        };
        let mut document = Value::clone(&self.inner.document);
        let before = entries(&document);
        change(&mut document);
        // What is left, in the order it was written in.
        let mut written = self.inner.written.keeping(&document);
        let union = match section {
            "components" => "anyComponent",
            _ => "anyFunction",
        };
        let declared = document
            .get("$defs")
            .and_then(|defs| defs.get(union))
            .is_some();
        if declared && before.is_some() && entries(&document) != before {
            let left: Vec<Ordered> = written
                .field(section)
                .map(Ordered::keys)
                .unwrap_or_default()
                .into_iter()
                .map(|name| {
                    let name = name.replace('~', "~0").replace('/', "~1");
                    Ordered::Object(vec![(
                        "$ref".to_string(),
                        Ordered::String(format!("#/{section}/{name}")),
                    )])
                })
                .collect();
            let derived = if left.is_empty() {
                Ordered::Object(vec![("not".to_string(), Ordered::Object(Vec::new()))])
            } else if section == "components" {
                Ordered::Object(vec![
                    ("oneOf".to_string(), Ordered::Array(left)),
                    (
                        "discriminator".to_string(),
                        Ordered::Object(vec![(
                            "propertyName".to_string(),
                            Ordered::String("component".to_string()),
                        )]),
                    ),
                ])
            } else {
                Ordered::Object(vec![("oneOf".to_string(), Ordered::Array(left))])
            };
            document["$defs"][union] = derived.to_value();
            if let Some(held) = written
                .field_mut("$defs")
                .and_then(|defs| defs.field_mut(union))
            {
                *held = derived;
            }
        }
        // A rule removes entries from an object that carries its id, which is
        // all a catalog needs to be read. Were a rule ever to leave less, the
        // catalog would allow nothing rather than everything.
        let catalog = Catalog::from_schema_as(&document, self.inner.version)
            .unwrap_or_else(|_| Catalog::empty(self.catalog_id()).with_version(self.inner.version));
        Self::of(catalog, document, written, self.inner.version)
    }
}

/// Reads the version a catalog document states: a version this crate speaks,
/// with or without the wire's leading `v`.
fn document_version(declared: &str) -> Option<A2uiVersion> {
    A2uiVersion::from_number(declared)
}

/// A rule an agent applies to a catalog before the catalog reaches a prompt
/// or a validator.
///
/// An allowlist is read literally: an empty one keeps nothing, and a name the
/// catalog never declared is ignored, because an agent that allowlists a
/// component a catalog does not have has asked for nothing, not for an error.
/// Keeping everything is said by applying no transformer.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CatalogTransformer {
    /// Keeps only the named components. Functions are left alone.
    KeepComponents(BTreeSet<String>),
    /// Keeps only the named functions. Components are left alone.
    KeepFunctions(BTreeSet<String>),
}

impl CatalogTransformer {
    /// The catalog this rule leaves. Its id and version are unchanged.
    pub fn apply(&self, catalog: &CatalogDocument) -> CatalogDocument {
        match self {
            Self::KeepComponents(names) => catalog.changed("components", |document| {
                let allowed: Vec<String> = names.iter().cloned().collect();
                prune_components(document, &allowed);
            }),
            Self::KeepFunctions(names) => catalog.changed("functions", |document| {
                let allowed: Vec<String> = names.iter().cloned().collect();
                prune_functions(document, &allowed);
            }),
        }
    }
}

/// A catalog an agent registered: the document as published, and the
/// transformers the agent applies to it.
///
/// The published document is kept untouched, so one renderer's session cannot
/// narrow the next one's. What a session works with is
/// [`CatalogConfig::transformed`].
///
/// ```
/// use ag_ui_a2ui::A2uiVersion;
/// use ag_ui_a2ui::agent::CatalogConfig;
///
/// // The official basic catalog, cut down to what this agent wants drawn.
/// let config = CatalogConfig::basic(A2uiVersion::V0_9_1)?
///     .keeping_components(["Column", "Text", "Button"])
///     .keeping_functions(["formatString"]);
///
/// assert_eq!(config.catalog().catalog().components.len(), 18);
/// assert_eq!(config.transformed().catalog().components.len(), 3);
/// # Ok::<(), ag_ui_a2ui::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogConfig {
    catalog: CatalogDocument,
    transformers: Vec<CatalogTransformer>,
    transformed: CatalogDocument,
}

impl CatalogConfig {
    /// Registers a catalog as it is.
    pub fn new(catalog: CatalogDocument) -> Self {
        Self {
            transformed: catalog.clone(),
            catalog,
            transformers: Vec::new(),
        }
    }

    /// Registers the official basic catalog. See [`CatalogDocument::basic`].
    ///
    /// # Errors
    ///
    /// As [`CatalogDocument::basic`].
    pub fn basic(version: A2uiVersion) -> Result<Self> {
        CatalogDocument::basic(version).map(Self::new)
    }

    /// Adds a transformer. Transformers apply in the order they were added.
    #[must_use]
    pub fn with_transformer(mut self, transformer: CatalogTransformer) -> Self {
        self.transformed = transformer.apply(&self.transformed);
        self.transformers.push(transformer);
        self
    }

    /// Keeps only the named components. See [`CatalogTransformer`].
    #[must_use]
    pub fn keeping_components<I, S>(self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.with_transformer(CatalogTransformer::KeepComponents(
            names.into_iter().map(Into::into).collect(),
        ))
    }

    /// Keeps only the named functions. See [`CatalogTransformer`].
    #[must_use]
    pub fn keeping_functions<I, S>(self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.with_transformer(CatalogTransformer::KeepFunctions(
            names.into_iter().map(Into::into).collect(),
        ))
    }

    /// The catalog as it was registered, before any transformer.
    pub fn catalog(&self) -> &CatalogDocument {
        &self.catalog
    }

    /// The transformers, in the order they apply.
    pub fn transformers(&self) -> &[CatalogTransformer] {
        &self.transformers
    }

    /// The catalog after every transformer: what a session works with.
    pub fn transformed(&self) -> &CatalogDocument {
        &self.transformed
    }
}

impl From<CatalogDocument> for CatalogConfig {
    fn from(catalog: CatalogDocument) -> Self {
        Self::new(catalog)
    }
}

/// The catalogs a format or a state is built for: at least one, each under an
/// id of its own, and all of one protocol family, since their messages state
/// one `version`.
///
/// # Errors
///
/// Returns [`Error::Catalog`] when `catalogs` is empty, holds two catalogs
/// under one id, or mixes versions that cannot meet on one surface.
pub(crate) fn check_catalogs(catalogs: &[CatalogDocument]) -> Result<A2uiVersion> {
    let Some(first) = catalogs.first() else {
        return Err(Error::catalog("at least one active catalog is required"));
    };
    let mut ids = BTreeSet::new();
    if let Some(twice) = catalogs
        .iter()
        .map(CatalogDocument::catalog_id)
        .find(|id| !ids.insert(*id))
    {
        // A surface, a component and a call name a catalog by its id.
        return Err(Error::catalog(format!(
            "two active catalogs share the id '{twice}': an id names one catalog"
        )));
    }
    for catalog in &catalogs[1..] {
        if !first.version().accepts(catalog.version()) {
            return Err(Error::catalog(format!(
                "the catalogs target incompatible protocol versions: '{}' targets {} and '{}' \
                 targets {}",
                first.catalog_id(),
                first.version(),
                catalog.catalog_id(),
                catalog.version()
            )));
        }
    }
    Ok(first.version())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn document() -> Value {
        json!({
            "catalogId": "test/shapes",
            "protocolVersion": "0.9",
            "components": {
                "Text": {"properties": {"text": {"type": "string"}}, "required": ["text"]},
                "Card": {"properties": {"child": {"$ref": "common_types.json#/$defs/ComponentId"}}},
            },
            "functions": {
                "required": {"returnType": "boolean"},
                "formatString": {"returnType": "string"},
            },
            "$defs": {
                "anyComponent": {"oneOf": [
                    {"$ref": "#/components/Text"},
                    {"$ref": "#/components/Card"},
                ]},
                "anyFunction": {"oneOf": [
                    {"$ref": "#/functions/required"},
                    {"$ref": "#/functions/formatString"},
                ]},
            },
        })
    }

    #[test]
    fn a_document_states_its_id_and_version_or_its_reader_does() {
        let stated = CatalogDocument::new(document()).unwrap();
        assert_eq!(stated.catalog_id(), "test/shapes");
        assert_eq!(stated.version(), A2uiVersion::V0_9);

        let mut bare = document();
        bare.as_object_mut().unwrap().remove("catalogId");
        bare.as_object_mut().unwrap().remove("protocolVersion");
        let given =
            CatalogDocument::load(bare.clone(), Some("mine"), Some(A2uiVersion::V0_9_1)).unwrap();
        assert_eq!(given.catalog_id(), "mine");
        assert_eq!(given.version(), A2uiVersion::V0_9_1);
        assert_eq!(
            given.document()["catalogId"],
            "mine",
            "the document a model is shown names the catalog"
        );

        for (id, version) in [(None, Some(A2uiVersion::V0_9)), (Some("mine"), None)] {
            assert!(matches!(
                CatalogDocument::load(bare.clone(), id, version),
                Err(Error::Catalog(_))
            ));
        }
    }

    #[test]
    fn what_the_reader_says_must_agree_with_the_document() {
        for (id, version) in [
            (Some("test/other"), None),
            (None, Some(A2uiVersion::V0_9_1)),
        ] {
            assert!(matches!(
                CatalogDocument::load(document(), id, version),
                Err(Error::Catalog(_))
            ));
        }
        CatalogDocument::load(document(), Some("test/shapes"), Some(A2uiVersion::V0_9)).unwrap();
    }

    #[test]
    fn a_version_this_crate_does_not_speak_is_refused() {
        for version in [json!("2.0"), json!("0.8"), json!(0.9), json!("")] {
            let mut other = document();
            other["protocolVersion"] = version;
            assert!(matches!(
                CatalogDocument::new(other),
                Err(Error::Catalog(_))
            ));
        }
        let mut wire = document();
        wire["protocolVersion"] = json!("v0.9.1");
        assert_eq!(
            CatalogDocument::new(wire).unwrap().version(),
            A2uiVersion::V0_9_1
        );
    }

    #[test]
    fn a_malformed_document_is_a_catalog_error() {
        for broken in [
            json!("test/shapes"),
            json!({"catalogId": "", "protocolVersion": "0.9"}),
            json!({"catalogId": 7, "protocolVersion": "0.9"}),
            json!({"catalogId": "x", "protocolVersion": "0.9", "components": []}),
            json!({"catalogId": "x", "protocolVersion": "0.9", "functions": "none"}),
        ] {
            assert!(matches!(
                CatalogDocument::new(broken),
                Err(Error::Catalog(_))
            ));
        }
    }

    #[test]
    fn pruning_changes_the_document_and_the_summary_together() {
        let config = CatalogConfig::new(CatalogDocument::new(document()).unwrap())
            .keeping_components(["Text", "Sparkline"])
            .keeping_functions(["required"]);

        let pruned = config.transformed();
        assert_eq!(
            pruned.catalog().components.keys().collect::<Vec<_>>(),
            ["Text"]
        );
        assert_eq!(
            pruned.catalog().functions.keys().collect::<Vec<_>>(),
            ["required"]
        );
        assert_eq!(
            pruned.document()["components"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            ["Text"]
        );
        assert_eq!(
            pruned.document()["$defs"]["anyComponent"]["oneOf"],
            json!([{"$ref": "#/components/Text"}])
        );
        assert_eq!(
            pruned.document()["$defs"]["anyFunction"]["oneOf"],
            json!([{"$ref": "#/functions/required"}])
        );
        assert_eq!(pruned.catalog_id(), "test/shapes");
        assert_eq!(pruned.version(), A2uiVersion::V0_9);

        // The summary is the pruned document's, and says so. It is the one
        // copy of the document that is held.
        assert_eq!(pruned.catalog().document(), Some(pruned.document()));
        assert!(std::ptr::eq(
            pruned.catalog().document().unwrap(),
            pruned.document()
        ));
        assert_eq!(
            &Catalog::from_schema(pruned.document()).unwrap(),
            pruned.catalog()
        );

        // The registration keeps what was published.
        assert_eq!(config.catalog().catalog().components.len(), 2);
        assert_eq!(config.transformers().len(), 2);
    }

    #[test]
    fn an_empty_allowlist_keeps_nothing_and_no_transformer_keeps_everything() {
        let catalog = CatalogDocument::new(document()).unwrap();
        let none = CatalogConfig::new(catalog.clone()).keeping_components(Vec::<String>::new());
        assert!(none.transformed().catalog().components.is_empty());
        assert_eq!(none.transformed().catalog().functions.len(), 2);

        let all = CatalogConfig::new(catalog.clone());
        assert_eq!(all.transformed(), &catalog);
    }

    #[test]
    fn the_basic_catalog_is_the_pinned_official_document() {
        let basic = CatalogDocument::basic(A2uiVersion::V0_9_1).unwrap();
        assert_eq!(
            basic.catalog_id(),
            crate::constants::OFFICIAL_BASIC_CATALOG_ID
        );
        assert_eq!(basic.catalog().components.len(), 18);
    }

    #[test]
    fn catalogs_of_one_family_may_share_a_session() {
        let old = CatalogDocument::new(document()).unwrap();
        let new = CatalogDocument::basic(A2uiVersion::V0_9_1).unwrap();
        assert_eq!(
            check_catalogs(&[old.clone(), new]).unwrap(),
            A2uiVersion::V0_9
        );
        assert!(matches!(check_catalogs(&[]), Err(Error::Catalog(_))));
        // An id names one catalog, and the two families do not mix.
        assert!(matches!(
            check_catalogs(&[old.clone(), old.clone()]),
            Err(Error::Catalog(_))
        ));
        let v1_0 = CatalogDocument::basic(A2uiVersion::V1_0).unwrap();
        assert_eq!(
            check_catalogs(std::slice::from_ref(&v1_0)).unwrap(),
            A2uiVersion::V1_0
        );
        assert!(matches!(
            check_catalogs(&[old, v1_0]),
            Err(Error::Catalog(_))
        ));
    }
}
