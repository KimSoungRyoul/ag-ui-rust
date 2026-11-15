//! What a renderer says it can draw, and which of the agent's catalogs that
//! makes active.

use serde_json::{Map, Value};

use super::catalog::{CatalogConfig, CatalogDocument};
use crate::error::{Error, Result};
use crate::protocol::validate::{ErrorCode, ValidationError};
use crate::protocol::version::A2uiVersion;

/// The metadata key a v0.9-family renderer sends its capabilities under.
/// From v1.0 the key is `a2uiRendererCapabilities`:
/// [`A2uiVersion::capabilities_key`] gives the key of any version.
pub const CLIENT_CAPABILITIES_KEY: &str = "a2uiClientCapabilities";

const SUPPORTED_CATALOG_IDS: &str = "supportedCatalogIds";
const INLINE_CATALOGS: &str = "inlineCatalogs";
const ACCEPTS_INLINE_CATALOGS: &str = "acceptsInlineCatalogs";

/// The key of a version's entry in a capabilities object: `v0.9` for the
/// whole v0.9 family, `1.0` for v1.0.
fn capabilities_key(version: A2uiVersion) -> &'static str {
    version.capabilities_entry()
}

/// A renderer's capabilities, keyed by protocol version, as they travel.
///
/// The object is kept as it was sent. An entry is read, strictly, when a
/// version asks for it: by [`RendererCapabilities::from_metadata`], which
/// reads one version's entry out of transport metadata, and by
/// [`resolve_catalogs`], which reads the entry of the registered catalogs'
/// version.
///
/// A renderer sends its capabilities in the metadata of a transport message,
/// keyed by protocol version. The metadata key is the version's own
/// ([`A2uiVersion::capabilities_key`]), and so is the key of its entry
/// ([`A2uiVersion::capabilities_entry`]):
///
/// ```json
/// {"a2uiClientCapabilities": {"v0.9": {"supportedCatalogIds": ["…"], "inlineCatalogs": [ … ]}}}
/// {"a2uiRendererCapabilities": {"1.0": {"supportedCatalogIds": ["…"], "inlineCatalogs": [ … ]}}}
/// ```
///
/// [`RendererCapabilities::from_metadata`] reads that, and [`resolve_catalogs`]
/// is the negotiation: every catalog the renderer names and the agent holds
/// becomes active, in the renderer's order of preference.
///
/// ```
/// use ag_ui_a2ui::A2uiVersion;
/// use ag_ui_a2ui::agent::{CatalogConfig, RendererCapabilities, resolve_catalogs};
/// use serde_json::json;
///
/// let registered = [CatalogConfig::basic(A2uiVersion::V0_9_1)?];
/// let metadata = json!({"a2uiClientCapabilities": {"v0.9": {"supportedCatalogIds": [
///     "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json",
///     "a/catalog/this/agent/does/not/hold",
/// ]}}});
///
/// let capabilities = RendererCapabilities::from_metadata(Some(&metadata), A2uiVersion::V0_9_1)?;
/// let active = resolve_catalogs(&registered, capabilities.as_ref(), false)?;
/// assert_eq!(active.len(), 1);
/// # Ok::<(), ag_ui_a2ui::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct RendererCapabilities {
    entries: Map<String, Value>,
}

impl RendererCapabilities {
    /// Capabilities naming the catalogs a renderer of `version` supports, most
    /// preferred first.
    pub fn supporting<I, S>(version: A2uiVersion, catalog_ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let ids: Vec<Value> = catalog_ids
            .into_iter()
            .map(|id| Value::String(id.into()))
            .collect();
        let mut entry = Map::new();
        entry.insert(SUPPORTED_CATALOG_IDS.into(), Value::Array(ids));
        let mut entries = Map::new();
        entries.insert(capabilities_key(version).into(), Value::Object(entry));
        Self { entries }
    }

    /// Adds catalog documents the renderer supplies itself to the entry of
    /// `version`.
    #[must_use]
    pub fn with_inline_catalogs(mut self, version: A2uiVersion, catalogs: Vec<Value>) -> Self {
        let entry = self
            .entries
            .entry(capabilities_key(version))
            .or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(entry) = entry {
            entry
                .entry(SUPPORTED_CATALOG_IDS)
                .or_insert_with(|| Value::Array(Vec::new()));
            entry.insert(INLINE_CATALOGS.into(), Value::Array(catalogs));
        }
        self
    }

    /// Takes a capabilities object as it was sent: entries keyed by protocol
    /// version. The entries are not read yet.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Validation`] when `capabilities` is not an object.
    pub fn from_json(capabilities: &Value) -> Result<Self> {
        match capabilities {
            Value::Object(entries) => Ok(Self {
                entries: entries.clone(),
            }),
            other => Err(invalid(
                "capabilities",
                format!(
                    "Renderer capabilities must be an object keyed by protocol version, not {}.",
                    kind(other)
                ),
            )),
        }
    }

    /// Reads the capabilities a renderer of `version` sent in the metadata of
    /// a transport message, under the key that version sends them under:
    /// [`CLIENT_CAPABILITIES_KEY`] for the v0.9 family,
    /// `a2uiRendererCapabilities` from v1.0.
    ///
    /// Returns `None` when there is no metadata, when the key is absent or
    /// null, and when it has no entry for `version`: a request that carries no
    /// capabilities has stated no preference. What is returned holds that one
    /// entry, and [`resolve_catalogs`] accepts it unchanged. A renderer that
    /// speaks both families sends both keys, and each is read by the version
    /// it belongs to.
    ///
    /// ```
    /// use ag_ui_a2ui::A2uiVersion;
    /// use ag_ui_a2ui::agent::RendererCapabilities;
    /// use serde_json::json;
    ///
    /// let metadata = json!({
    ///     "a2uiClientCapabilities": {"v0.9": {"supportedCatalogIds": ["old"]}},
    ///     "a2uiRendererCapabilities": {"1.0": {"supportedCatalogIds": ["new"]}},
    /// });
    /// let read = |version| RendererCapabilities::from_metadata(Some(&metadata), version);
    /// assert_eq!(
    ///     read(A2uiVersion::V1_0)?.unwrap().to_json(),
    ///     json!({"1.0": {"supportedCatalogIds": ["new"]}})
    /// );
    /// assert_eq!(
    ///     read(A2uiVersion::V0_9_1)?.unwrap().to_json(),
    ///     json!({"v0.9": {"supportedCatalogIds": ["old"]}})
    /// );
    /// # Ok::<(), ag_ui_a2ui::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::Catalog`] when the metadata, the value under the key
    /// or the version's entry is malformed: an entry without
    /// `supportedCatalogIds`, with a property the protocol does not define, or
    /// with an inline catalog that cannot be read or targets another protocol
    /// version.
    pub fn from_metadata(metadata: Option<&Value>, version: A2uiVersion) -> Result<Option<Self>> {
        let Some(metadata) = metadata.filter(|metadata| !metadata.is_null()) else {
            return Ok(None);
        };
        let Some(metadata) = metadata.as_object() else {
            return Err(Error::catalog("transport metadata must be a JSON object"));
        };
        let metadata_key = version.capabilities_key();
        let raw = match metadata.get(metadata_key) {
            None | Some(Value::Null) => return Ok(None),
            Some(Value::Object(raw)) => raw,
            Some(_) => {
                return Err(Error::catalog(format!(
                    "the '{metadata_key}' metadata value must be a JSON object"
                )));
            }
        };
        let key = capabilities_key(version);
        let entry = match raw.get(key) {
            None | Some(Value::Null) => return Ok(None),
            Some(entry) => entry,
        };
        let read = Entry::read(entry).map_err(|fault| {
            Error::catalog(format!(
                "invalid '{key}' renderer capabilities under '{metadata_key}': {}",
                fault.reason
            ))
        })?;
        for document in &read.inline_catalogs {
            inline_catalog(document, version)?;
        }
        let mut entries = Map::new();
        entries.insert(key.into(), entry.clone());
        Ok(Some(Self { entries }))
    }

    /// The capabilities object, keyed by protocol version, as it travels.
    pub fn to_json(&self) -> Value {
        Value::Object(self.entries.clone())
    }
}

/// What an agent advertises to renderers: the catalogs it can write UI for,
/// and whether it takes the catalogs a renderer declares inline.
///
/// It is the counterpart of [`RendererCapabilities`], in the other direction.
/// As an object keyed by protocol version it is what the protocol's
/// `agent_capabilities.json` describes for v1.0 and `server_capabilities.json`
/// for the v0.9 family, and [`AgentCapabilities::to_json`] writes it:
///
/// ```json
/// {"1.0": {"supportedCatalogIds": ["…"], "acceptsInlineCatalogs": false}}
/// {"v0.9": {"supportedCatalogIds": ["…"], "acceptsInlineCatalogs": false}}
/// ```
///
/// Where it travels is each transport's business. On an A2A agent card the
/// same two properties are the `params` of the A2UI extension, without a
/// version key, since the extension's URI names the version
/// (`adapters::a2a::agent_extension`).
///
/// [`A2uiGenerator::capabilities`](super::A2uiGenerator::capabilities) gives
/// the capabilities of a generator: the catalogs it registered.
///
/// ```
/// use ag_ui_a2ui::A2uiVersion;
/// use ag_ui_a2ui::agent::{A2uiGenerator, AgentCapabilities, CatalogConfig};
/// use serde_json::json;
///
/// let generator = A2uiGenerator::new(vec![CatalogConfig::basic(A2uiVersion::V1_0)?]);
/// let advertised = generator.capabilities().to_json(A2uiVersion::V1_0);
/// assert_eq!(
///     advertised,
///     json!({"1.0": {
///         "supportedCatalogIds": ["https://a2ui.org/specification/v1_0/catalogs/basic/catalog.json"],
///         "acceptsInlineCatalogs": false,
///     }})
/// );
///
/// // And read back, by a renderer or a test.
/// let read = AgentCapabilities::from_json(&advertised, A2uiVersion::V1_0)?.unwrap();
/// assert_eq!(read, generator.capabilities());
/// # Ok::<(), ag_ui_a2ui::Error>(())
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct AgentCapabilities {
    /// The ids of the catalogs the agent can write UI for. From v1.0 one
    /// surface may draw from several of them.
    pub supported_catalog_ids: Vec<String>,
    /// Whether the agent takes the catalogs a renderer declares inline in its
    /// own capabilities.
    pub accepts_inline_catalogs: bool,
}

impl AgentCapabilities {
    /// Capabilities naming the catalogs an agent can write UI for. Inline
    /// catalogs are not accepted until
    /// [`AgentCapabilities::accepting_inline_catalogs`] says so.
    pub fn supporting<I, S>(catalog_ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            supported_catalog_ids: catalog_ids.into_iter().map(Into::into).collect(),
            accepts_inline_catalogs: false,
        }
    }

    /// Whether the agent takes a renderer's inline catalogs.
    #[must_use]
    pub fn accepting_inline_catalogs(mut self, accepts: bool) -> Self {
        self.accepts_inline_catalogs = accepts;
        self
    }

    /// The capabilities object an agent of `version` advertises: one entry
    /// under the version's key ([`A2uiVersion::capabilities_entry`]), with
    /// both properties stated.
    pub fn to_json(&self, version: A2uiVersion) -> Value {
        let mut entry = Map::new();
        entry.insert(
            SUPPORTED_CATALOG_IDS.into(),
            Value::Array(
                self.supported_catalog_ids
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        );
        entry.insert(
            ACCEPTS_INLINE_CATALOGS.into(),
            Value::Bool(self.accepts_inline_catalogs),
        );
        let mut entries = Map::new();
        entries.insert(capabilities_key(version).into(), Value::Object(entry));
        Value::Object(entries)
    }

    /// Reads the entry of `version` out of a capabilities object an agent
    /// advertised.
    ///
    /// Returns `None` when the object has no entry for `version`: the agent
    /// does not speak it. Both properties are optional in the protocol's
    /// schemas: an entry that names no catalogs supports none it has said,
    /// and one that does not say whether it takes inline catalogs does not.
    /// A property the protocol does not define is passed over, as its schemas
    /// allow.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Catalog`] when `capabilities` or the entry is not an
    /// object, when `supportedCatalogIds` is not an array of strings, and
    /// when `acceptsInlineCatalogs` is not a boolean.
    pub fn from_json(capabilities: &Value, version: A2uiVersion) -> Result<Option<Self>> {
        let key = capabilities_key(version);
        let Some(entries) = capabilities.as_object() else {
            return Err(Error::catalog(format!(
                "agent capabilities must be an object keyed by protocol version, not {}",
                kind(capabilities)
            )));
        };
        let entry = match entries.get(key) {
            None | Some(Value::Null) => return Ok(None),
            Some(Value::Object(entry)) => entry,
            Some(other) => {
                return Err(Error::catalog(format!(
                    "invalid '{key}' agent capabilities: the entry must be a JSON object, not {}",
                    kind(other)
                )));
            }
        };
        let supported_catalog_ids = match entry.get(SUPPORTED_CATALOG_IDS) {
            None => Vec::new(),
            Some(Value::Array(ids)) => ids
                .iter()
                .map(|id| id.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| {
                    Error::catalog(format!(
                        "invalid '{key}' agent capabilities: '{SUPPORTED_CATALOG_IDS}' must be \
                         an array of strings"
                    ))
                })?,
            Some(other) => {
                return Err(Error::catalog(format!(
                    "invalid '{key}' agent capabilities: '{SUPPORTED_CATALOG_IDS}' must be an \
                     array of strings, not {}",
                    kind(other)
                )));
            }
        };
        let accepts_inline_catalogs = match entry.get(ACCEPTS_INLINE_CATALOGS) {
            None => false,
            Some(Value::Bool(accepts)) => *accepts,
            Some(other) => {
                return Err(Error::catalog(format!(
                    "invalid '{key}' agent capabilities: '{ACCEPTS_INLINE_CATALOGS}' must be a \
                     boolean, not {}",
                    kind(other)
                )));
            }
        };
        Ok(Some(Self {
            supported_catalog_ids,
            accepts_inline_catalogs,
        }))
    }
}

/// One version's entry of a capabilities object, read strictly.
struct Entry {
    supported_catalog_ids: Vec<String>,
    inline_catalogs: Vec<Value>,
}

/// Why an entry could not be read, and whether its inline catalogs were what
/// was wrong: that is a catalog failure, anything else is a malformed object.
struct Fault {
    inline: bool,
    reason: String,
}

impl Entry {
    fn read(entry: &Value) -> std::result::Result<Self, Fault> {
        let fault = |inline: bool, reason: String| Fault { inline, reason };
        let Some(entry) = entry.as_object() else {
            return Err(fault(
                false,
                format!("the entry must be a JSON object, not {}", kind(entry)),
            ));
        };
        if let Some(unknown) = entry
            .keys()
            .find(|key| *key != SUPPORTED_CATALOG_IDS && *key != INLINE_CATALOGS)
        {
            return Err(fault(
                false,
                format!("'{unknown}' is not a property of renderer capabilities"),
            ));
        }

        let inline_catalogs = match entry.get(INLINE_CATALOGS) {
            None => Vec::new(),
            Some(Value::Array(documents)) => {
                if let Some(other) = documents.iter().find(|document| !document.is_object()) {
                    return Err(fault(
                        true,
                        format!(
                            "every entry of '{INLINE_CATALOGS}' must be a catalog object, not {}",
                            kind(other)
                        ),
                    ));
                }
                documents.clone()
            }
            Some(other) => {
                return Err(fault(
                    true,
                    format!(
                        "'{INLINE_CATALOGS}' must be an array of catalog objects, not {}",
                        kind(other)
                    ),
                ));
            }
        };

        let supported_catalog_ids = match entry.get(SUPPORTED_CATALOG_IDS) {
            Some(Value::Array(ids)) => ids
                .iter()
                .map(|id| id.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| {
                    fault(
                        false,
                        format!("'{SUPPORTED_CATALOG_IDS}' must be an array of strings"),
                    )
                })?,
            Some(other) => {
                return Err(fault(
                    false,
                    format!(
                        "'{SUPPORTED_CATALOG_IDS}' must be an array of strings, not {}",
                        kind(other)
                    ),
                ));
            }
            None => {
                return Err(fault(
                    false,
                    format!("'{SUPPORTED_CATALOG_IDS}' is required"),
                ));
            }
        };

        Ok(Self {
            supported_catalog_ids,
            inline_catalogs,
        })
    }
}

/// Reads a catalog a renderer declared inline, for a session of `version`.
///
/// A document that states its own version keeps it, as long as that version
/// reads the same capabilities entry; one that states none is read as
/// `version`.
fn inline_catalog(document: &Value, version: A2uiVersion) -> Result<CatalogDocument> {
    let stated = document
        .get("protocolVersion")
        .is_some_and(|stated| !stated.is_null());
    let catalog = CatalogDocument::load(document.clone(), None, (!stated).then_some(version))?;
    if capabilities_key(catalog.version()) != capabilities_key(version) {
        return Err(Error::catalog(format!(
            "inline catalog '{}' targets {}, which '{}' renderer capabilities cannot carry",
            catalog.catalog_id(),
            catalog.version(),
            capabilities_key(version)
        )));
    }
    Ok(catalog)
}

/// The catalogs that are active for a renderer's capabilities.
///
/// - Every registered catalog the renderer names becomes active, in the
///   renderer's order of preference, since the renderer's first choice should
///   be the first catalog a model reads about. An id the agent does not hold is
///   ignored, and an id named twice activates its catalog once.
/// - With `accepts_inline_catalogs`, each inline catalog becomes an active
///   catalog of its own after the registered ones, unless an active catalog
///   already has its id. Otherwise inline catalogs are dropped. They are read
///   either way: a malformed one makes the capabilities malformed whether or
///   not the agent would have used it.
/// - `None`, for a request that carried no capabilities, activates every
///   registered catalog in registration order: the renderer stated no
///   preference. With nothing registered that is an empty result.
///
/// The catalogs returned are the transformed ones
/// ([`CatalogConfig::transformed`]), so a pruning transformer shows both in the
/// prompt and in what validation accepts. Inline catalogs are not transformed:
/// they belong to the renderer.
///
/// # Errors
///
/// Returns [`Error::Catalog`] when capabilities are given and no catalog ends
/// up active (the renderer names nothing the agent holds, or sends an empty
/// `supportedCatalogIds` and no usable inline catalog), when an inline catalog
/// cannot be read, and when the registered catalogs read different
/// capabilities entries. Returns [`Error::Validation`] when the capabilities
/// have no well-formed entry for the registered catalogs' protocol version:
/// that is a malformed object, not a statement of no preference.
pub fn resolve_catalogs(
    catalogs: &[CatalogConfig],
    capabilities: Option<&RendererCapabilities>,
    accepts_inline_catalogs: bool,
) -> Result<Vec<CatalogDocument>> {
    let registered: Vec<CatalogDocument> = catalogs
        .iter()
        .map(|config| config.transformed().clone())
        .collect();
    let Some(capabilities) = capabilities else {
        return Ok(registered);
    };
    let Some(first) = registered.first() else {
        return Err(Error::catalog(
            "no catalogs are registered, so none of the renderer's catalogs can be resolved",
        ));
    };
    let version = first.version();
    let key = capabilities_key(version);
    if let Some(other) = registered
        .iter()
        .find(|catalog| capabilities_key(catalog.version()) != key)
    {
        return Err(Error::catalog(format!(
            "the registered catalogs read different capabilities entries: '{}' reads '{key}' \
             and '{}' reads '{}'",
            first.catalog_id(),
            other.catalog_id(),
            capabilities_key(other.version())
        )));
    }

    let Some(entry) = capabilities.entries.get(key) else {
        return Err(invalid(
            "capabilities",
            format!(
                "The renderer capabilities have no '{key}' entry, which the registered \
                 catalogs read."
            ),
        ));
    };
    let entry = Entry::read(entry).map_err(|fault| {
        if fault.inline {
            Error::catalog(format!(
                "invalid inline catalog in '{key}' renderer capabilities: {}",
                fault.reason
            ))
        } else {
            invalid(
                format!("capabilities[\"{key}\"]"),
                format!("Invalid '{key}' renderer capabilities: {}.", fault.reason),
            )
        }
    })?;
    let inline = entry
        .inline_catalogs
        .iter()
        .map(|document| inline_catalog(document, version))
        .collect::<Result<Vec<_>>>()?;

    let mut active: Vec<CatalogDocument> = Vec::new();
    let holds = |active: &[CatalogDocument], id: &str| {
        active.iter().any(|catalog| catalog.catalog_id() == id)
    };
    for id in &entry.supported_catalog_ids {
        if holds(&active, id) {
            continue;
        }
        if let Some(catalog) = registered.iter().find(|catalog| catalog.catalog_id() == id) {
            active.push(catalog.clone());
        }
    }
    if accepts_inline_catalogs {
        for catalog in inline {
            if !holds(&active, catalog.catalog_id()) {
                active.push(catalog);
            }
        }
    }
    if active.is_empty() {
        let held: Vec<&str> = registered.iter().map(CatalogDocument::catalog_id).collect();
        return Err(Error::catalog(format!(
            "the renderer supports none of the agent's catalogs; the agent holds {held:?}"
        )));
    }
    Ok(active)
}

/// A capabilities object that is malformed, as opposed to one that leaves the
/// session without a catalog.
fn invalid(path: impl Into<String>, message: impl Into<String>) -> Error {
    Error::Validation {
        errors: vec![ValidationError::new(ErrorCode::InvalidValue, path, message)].into(),
    }
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn catalog(id: &str, component: &str) -> CatalogConfig {
        CatalogConfig::new(
            CatalogDocument::new(json!({
                "catalogId": id,
                "protocolVersion": "0.9",
                "components": {component: {"properties": {}}},
            }))
            .unwrap(),
        )
    }

    fn ids(active: &[CatalogDocument]) -> Vec<&str> {
        active.iter().map(CatalogDocument::catalog_id).collect()
    }

    #[test]
    fn an_agent_advertises_under_the_key_of_its_version() {
        let advertised = AgentCapabilities::supporting(["a", "b"]).accepting_inline_catalogs(true);
        for (version, key) in [
            (A2uiVersion::V0_9, "v0.9"),
            (A2uiVersion::V0_9_1, "v0.9"),
            (A2uiVersion::V1_0, "1.0"),
        ] {
            let written = advertised.to_json(version);
            assert_eq!(
                written,
                json!({key: {"supportedCatalogIds": ["a", "b"], "acceptsInlineCatalogs": true}})
            );
            assert_eq!(
                AgentCapabilities::from_json(&written, version).unwrap(),
                Some(advertised.clone())
            );
        }
        // An agent that advertises one family says nothing of the other.
        let v0_9 = advertised.to_json(A2uiVersion::V0_9_1);
        assert_eq!(
            AgentCapabilities::from_json(&v0_9, A2uiVersion::V1_0).unwrap(),
            None
        );
    }

    #[test]
    fn what_an_agent_leaves_out_of_its_capabilities_is_the_default() {
        let read = |value: Value| AgentCapabilities::from_json(&value, A2uiVersion::V1_0);
        assert_eq!(
            read(json!({"1.0": {}})).unwrap(),
            Some(AgentCapabilities::default())
        );
        assert_eq!(
            read(json!({"1.0": {"supportedCatalogIds": ["a"], "somethingLater": 1}})).unwrap(),
            Some(AgentCapabilities::supporting(["a"]))
        );
        for malformed in [
            json!([]),
            json!({"1.0": "everything"}),
            json!({"1.0": {"supportedCatalogIds": "a"}}),
            json!({"1.0": {"supportedCatalogIds": ["a", 2]}}),
            json!({"1.0": {"acceptsInlineCatalogs": "yes"}}),
        ] {
            let error = read(malformed.clone()).unwrap_err();
            assert!(matches!(error, Error::Catalog(_)), "{malformed}: {error}");
        }
    }

    /// What is written is what the protocol's own schema of each version
    /// accepts, and the schema requires the version's entry.
    #[cfg(feature = "schema-validation")]
    #[test]
    fn what_an_agent_advertises_is_held_by_the_pinned_schemas() {
        use crate::protocol::schema_validation::compile_with;

        let advertised = AgentCapabilities::supporting(["a"]).accepting_inline_catalogs(true);
        for (version, schema) in [
            (
                A2uiVersion::V0_9,
                include_str!("../../schemas/v0_9/server_capabilities.json"),
            ),
            (
                A2uiVersion::V0_9_1,
                include_str!("../../schemas/v0_9_1/server_capabilities.json"),
            ),
            (
                A2uiVersion::V1_0,
                include_str!("../../schemas/v1_0/agent_capabilities.json"),
            ),
        ] {
            let schema: Value = serde_json::from_str(schema).unwrap();
            let validator = compile_with(version, &schema, &[]).unwrap();
            let written = advertised.to_json(version);
            assert!(validator.is_valid(&written), "{version}: {written}");
            let other = if version == A2uiVersion::V1_0 {
                A2uiVersion::V0_9_1
            } else {
                A2uiVersion::V1_0
            };
            assert!(
                !validator.is_valid(&advertised.to_json(other)),
                "{version} takes the entry of {other}"
            );
        }
    }

    #[test]
    fn both_versions_of_the_family_read_the_same_entry() {
        assert_eq!(capabilities_key(A2uiVersion::V0_9), "v0.9");
        assert_eq!(capabilities_key(A2uiVersion::V0_9_1), "v0.9");
        assert_eq!(capabilities_key(A2uiVersion::V1_0), "1.0");
    }

    fn v1_0(id: &str, component: &str) -> CatalogConfig {
        CatalogConfig::new(
            CatalogDocument::new(json!({
                "catalogId": id,
                "protocolVersion": "1.0",
                "components": {component: {"type": "object"}},
            }))
            .unwrap(),
        )
    }

    #[test]
    fn a_v1_0_renderer_is_read_under_its_own_keys() {
        let registered = [v1_0("a", "A"), v1_0("b", "B")];
        let metadata = json!({
            "a2uiClientCapabilities": {"v0.9": {"supportedCatalogIds": ["a"]}},
            "a2uiRendererCapabilities": {"1.0": {
                "supportedCatalogIds": ["b"],
                "inlineCatalogs": [{"catalogId": "renderer/inline", "components": {"Marquee": {}}}],
            }},
        });
        let capabilities = RendererCapabilities::from_metadata(Some(&metadata), A2uiVersion::V1_0)
            .unwrap()
            .unwrap();
        assert_eq!(
            capabilities.to_json()["1.0"]["supportedCatalogIds"],
            json!(["b"])
        );
        let active = resolve_catalogs(&registered, Some(&capabilities), true).unwrap();
        assert_eq!(ids(&active), ["b", "renderer/inline"]);
        // An inline catalog that states no version is one of the entry's.
        assert_eq!(active[1].version(), A2uiVersion::V1_0);
        assert_eq!(active[1].catalog().version(), A2uiVersion::V1_0);

        assert_eq!(
            RendererCapabilities::supporting(A2uiVersion::V1_0, ["b"]).to_json(),
            json!({"1.0": {"supportedCatalogIds": ["b"]}})
        );

        // The entry of one family says nothing to catalogs of the other.
        let old = RendererCapabilities::supporting(A2uiVersion::V0_9, ["a"]);
        assert!(matches!(
            resolve_catalogs(&registered, Some(&old), false),
            Err(Error::Validation { .. })
        ));
        // Nor is a v0.9 catalog one a v1.0 entry can carry inline.
        let mixed = json!({"a2uiRendererCapabilities": {"1.0": {
            "supportedCatalogIds": [],
            "inlineCatalogs": [{"catalogId": "x", "protocolVersion": "0.9"}],
        }}});
        assert!(matches!(
            RendererCapabilities::from_metadata(Some(&mixed), A2uiVersion::V1_0),
            Err(Error::Catalog(_))
        ));
    }

    #[test]
    fn the_renderer_preference_order_decides_the_order_of_the_active_catalogs() {
        let registered = [catalog("a", "A"), catalog("b", "B"), catalog("c", "C")];
        let capabilities =
            RendererCapabilities::supporting(A2uiVersion::V0_9, ["c", "nope", "a", "c"]);
        let active = resolve_catalogs(&registered, Some(&capabilities), false).unwrap();
        assert_eq!(ids(&active), ["c", "a"]);
    }

    #[test]
    fn no_capabilities_is_no_preference() {
        let registered = [catalog("a", "A"), catalog("b", "B")];
        assert_eq!(
            ids(&resolve_catalogs(&registered, None, false).unwrap()),
            ["a", "b"]
        );
        assert!(resolve_catalogs(&[], None, false).unwrap().is_empty());
    }

    #[test]
    fn nothing_in_common_is_a_catalog_error() {
        let registered = [catalog("a", "A")];
        for capabilities in [
            RendererCapabilities::supporting(A2uiVersion::V0_9, ["z"]),
            RendererCapabilities::supporting(A2uiVersion::V0_9, Vec::<String>::new()),
        ] {
            assert!(matches!(
                resolve_catalogs(&registered, Some(&capabilities), false),
                Err(Error::Catalog(_))
            ));
        }
        let named = RendererCapabilities::supporting(A2uiVersion::V0_9, ["a"]);
        assert!(matches!(
            resolve_catalogs(&[], Some(&named), false),
            Err(Error::Catalog(_))
        ));
    }

    #[test]
    fn an_inline_catalog_is_active_only_when_the_agent_accepts_it() {
        let registered = [catalog("a", "A").keeping_components(Vec::<String>::new())];
        let inline = json!({"catalogId": "renderer/inline", "components": {"Marquee": {}}});
        let capabilities = RendererCapabilities::supporting(A2uiVersion::V0_9, ["a"])
            .with_inline_catalogs(A2uiVersion::V0_9, vec![inline]);

        let accepted = resolve_catalogs(&registered, Some(&capabilities), true).unwrap();
        assert_eq!(ids(&accepted), ["a", "renderer/inline"]);
        assert!(
            accepted[0].catalog().components.is_empty(),
            "a registered catalog is the transformed one"
        );
        assert_eq!(
            accepted[1].catalog().components.len(),
            1,
            "an inline catalog is the renderer's, and is not transformed"
        );
        assert_eq!(accepted[1].version(), A2uiVersion::V0_9);

        let dropped = resolve_catalogs(&registered, Some(&capabilities), false).unwrap();
        assert_eq!(ids(&dropped), ["a"]);
    }

    #[test]
    fn a_malformed_inline_catalog_fails_whether_or_not_it_would_be_used() {
        let registered = [catalog("a", "A")];
        for inline in [
            json!("a"),
            json!(["a"]),
            json!([{"components": {}}]),
            json!([{"catalogId": "", "components": {}}]),
            json!([{"catalogId": "x", "protocolVersion": "1.0"}]),
            json!([{"catalogId": "x", "protocolVersion": "2.0"}]),
        ] {
            let capabilities = RendererCapabilities::from_json(&json!({
                "v0.9": {"supportedCatalogIds": ["a"], "inlineCatalogs": inline},
            }))
            .unwrap();
            for accepts in [true, false] {
                assert!(
                    matches!(
                        resolve_catalogs(&registered, Some(&capabilities), accepts),
                        Err(Error::Catalog(_))
                    ),
                    "{inline}"
                );
            }
        }
    }

    #[test]
    fn capabilities_without_the_catalogs_version_are_malformed() {
        let registered = [catalog("a", "A")];
        for capabilities in [
            json!({"v0.8": {"supportedCatalogIds": ["a"]}}),
            json!({"v0.9": {}}),
            json!({"v0.9": {"supportedCatalogIds": "a"}}),
            json!({"v0.9": {"supportedCatalogIds": ["a"], "extra": true}}),
            json!({"v0.9": ["a"]}),
        ] {
            let capabilities = RendererCapabilities::from_json(&capabilities).unwrap();
            assert!(
                matches!(
                    resolve_catalogs(&registered, Some(&capabilities), false),
                    Err(Error::Validation { .. })
                ),
                "{capabilities:?}"
            );
        }
        assert!(matches!(
            RendererCapabilities::from_json(&json!(["v0.9"])),
            Err(Error::Validation { .. })
        ));
    }

    #[test]
    fn metadata_without_capabilities_reads_as_none() {
        for metadata in [
            None,
            Some(json!(null)),
            Some(json!({})),
            Some(json!({"a2uiClientCapabilities": null})),
            Some(json!({"a2uiClientCapabilities": {}})),
            Some(json!({"a2uiClientCapabilities": {"v0.8": {"supportedCatalogIds": []}}})),
            Some(json!({"a2uiRendererCapabilities": {"1.0": {"supportedCatalogIds": []}}})),
        ] {
            assert_eq!(
                RendererCapabilities::from_metadata(metadata.as_ref(), A2uiVersion::V0_9).unwrap(),
                None,
                "{metadata:?}"
            );
        }
    }

    #[test]
    fn metadata_yields_the_one_entry_its_version_reads() {
        let metadata = json!({
            "a2uiClientCapabilities": {
                "v0.9": {"supportedCatalogIds": ["a"]},
                "v0.8": {"supportedCatalogIds": ["old"]},
            },
            "a2uiRendererCapabilities": {"1.0": {"supportedCatalogIds": ["new"]}},
        });
        for version in [A2uiVersion::V0_9, A2uiVersion::V0_9_1] {
            let capabilities = RendererCapabilities::from_metadata(Some(&metadata), version)
                .unwrap()
                .unwrap();
            assert_eq!(
                capabilities.to_json(),
                json!({"v0.9": {"supportedCatalogIds": ["a"]}})
            );
            assert_eq!(
                capabilities,
                RendererCapabilities::supporting(version, ["a"])
            );
        }
    }

    #[test]
    fn malformed_metadata_is_a_catalog_error() {
        for metadata in [
            json!("metadata"),
            json!({"a2uiClientCapabilities": "v0.9"}),
            json!({"a2uiClientCapabilities": {"v0.9": "a"}}),
            json!({"a2uiClientCapabilities": {"v0.9": {}}}),
            json!({"a2uiClientCapabilities": {"v0.9": {"inlineCatalogs": []}}}),
            json!({"a2uiClientCapabilities": {"v0.9": {"supportedCatalogIds": "a"}}}),
            json!({"a2uiClientCapabilities": {"v0.9": {"supportedCatalogIds": [], "extra": 1}}}),
            json!({"a2uiClientCapabilities": {"v0.9": {
                "supportedCatalogIds": [], "inlineCatalogs": null,
            }}}),
            json!({"a2uiClientCapabilities": {"v0.9": {
                "supportedCatalogIds": [], "inlineCatalogs": "x",
            }}}),
            json!({"a2uiClientCapabilities": {"v0.9": {
                "supportedCatalogIds": [], "inlineCatalogs": [{"components": {}}],
            }}}),
            json!({"a2uiClientCapabilities": {"v0.9": {
                "supportedCatalogIds": [],
                "inlineCatalogs": [{"catalogId": "x", "protocolVersion": "1.0"}],
            }}}),
            json!({"a2uiClientCapabilities": {"v0.9": {
                "supportedCatalogIds": [],
                "inlineCatalogs": [{"catalogId": "x", "protocolVersion": 9}],
            }}}),
        ] {
            assert!(
                matches!(
                    RendererCapabilities::from_metadata(Some(&metadata), A2uiVersion::V0_9),
                    Err(Error::Catalog(_))
                ),
                "{metadata}"
            );
        }
    }
}
