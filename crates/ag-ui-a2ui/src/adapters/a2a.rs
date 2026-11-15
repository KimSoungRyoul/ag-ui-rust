//! A2UI over A2A (feature `a2a`): data parts, the agent-card extension, and
//! a renderer's capabilities.
//!
//! An A2A agent sends each A2UI message as a `DataPart` labelled with a MIME
//! type, says on its agent card which A2UI versions it speaks, and reads what
//! the renderer can draw from the metadata of the message it was sent. These
//! helpers build and read those JSON shapes. They depend on no A2A library:
//! a part, a message and an extension are [`serde_json::Value`]s, to be put
//! into whatever A2A types the application uses.
//!
//! The shapes and rules are those of the A2UI project's own A2A integration
//! (`a2ui.a2a`, read at upstream commit
//! `3ed9601862ecc78027e20ac80ad8b1b98a5d9079`), and its conformance suite
//! `extensions/a2a/a2a_integration.yaml` is run against this module.
//!
//! ```
//! use ag_ui_a2ui::adapters::a2a;
//! use ag_ui_a2ui::{A2uiVersion, AgentMessage};
//!
//! // On the agent card.
//! let extension = a2a::agent_extension("0.9.1", false, &[]);
//! assert_eq!(extension["uri"], "https://a2ui.org/a2a-extension/a2ui/v0.9.1");
//!
//! // On a request: the newest version both sides speak.
//! let requested = [a2a::extension_uri("0.9.1")];
//! let (uri, version) = a2a::activate(&requested, &[extension["uri"].as_str().unwrap()]).unwrap();
//! assert_eq!((uri, version), (requested[0].as_str(), A2uiVersion::V0_9_1));
//!
//! // In the answer: one data part for each A2UI message.
//! let message = AgentMessage::delete_surface("cart").with_version(version);
//! let part = a2a::data_part(&serde_json::to_value(&message)?, None);
//! assert_eq!(part["metadata"]["mimeType"], "application/a2ui+json");
//! assert!(a2a::is_a2ui_part(&part));
//! # Ok::<(), serde_json::Error>(())
//! ```

use serde_json::{Map, Value, json};

use crate::agent::RendererCapabilities;
use crate::error::Result;
use crate::protocol::version::A2uiVersion;

/// The MIME type of a data part that carries an A2UI message.
pub const MIME_TYPE: &str = "application/a2ui+json";

/// The MIME type A2UI v0.8 and v0.9 parts were labelled with, and that a part
/// which states no version still is. It is read as A2UI too.
pub const DEPRECATED_MIME_TYPE: &str = "application/json+a2ui";

/// The metadata key of a part that holds its MIME type.
pub const MIME_TYPE_KEY: &str = "mimeType";

/// What every A2UI extension URI starts with. The version follows it:
/// `…/a2ui/v0.9.1`.
pub const EXTENSION_BASE_URI: &str = "https://a2ui.org/a2a-extension/a2ui";

/// A version as an extension URI and a MIME type spell it: without the
/// leading `v`.
fn version_number(version: &str) -> &str {
    let version = version.trim();
    version.strip_prefix(['v', 'V']).unwrap_or(version)
}

/// The MIME type for a part of `version`: the deprecated one for v0.8, v0.9
/// and no version at all, [`MIME_TYPE`] from v0.9.1 on.
pub fn mime_type(version: Option<&str>) -> &'static str {
    match version.map(version_number) {
        None | Some("0.8" | "0.9") => DEPRECATED_MIME_TYPE,
        Some(_) => MIME_TYPE,
    }
}

/// An A2A data part that carries one A2UI message.
///
/// `version` is the A2UI version the part is labelled for, with or without a
/// leading `v`. Without it the message's own `version` field is used.
pub fn data_part(message: &Value, version: Option<&str>) -> Value {
    let version = version.or_else(|| message.get("version").and_then(Value::as_str));
    json!({
        "kind": "data",
        "data": message,
        "metadata": {MIME_TYPE_KEY: mime_type(version)},
    })
}

/// Whether an A2A part is a data part: its `kind` says so, or, written
/// without a `kind` as some serializers leave it, it has `data`.
fn is_data_part(part: &Value) -> bool {
    match part.get("kind").and_then(Value::as_str) {
        Some(kind) => kind == "data",
        None => part.get("data").is_some(),
    }
}

/// Whether an A2A part is a data part that carries A2UI: its metadata names
/// [`MIME_TYPE`] or [`DEPRECATED_MIME_TYPE`].
pub fn is_a2ui_part(part: &Value) -> bool {
    is_data_part(part)
        && part
            .pointer("/metadata/mimeType")
            .and_then(Value::as_str)
            .is_some_and(|mime| mime == MIME_TYPE || mime == DEPRECATED_MIME_TYPE)
}

/// The A2UI messages the parts of an A2A message carry, in the order of the
/// parts. Parts that are not A2UI are passed over.
pub fn a2ui_messages<'a>(parts: impl IntoIterator<Item = &'a Value>) -> Vec<&'a Value> {
    parts
        .into_iter()
        .filter(|part| is_a2ui_part(part))
        .filter_map(|part| part.get("data"))
        .collect()
}

/// The extension URI of an A2UI version, given with or without a leading `v`.
pub fn extension_uri(version: &str) -> String {
    format!("{EXTENSION_BASE_URI}/v{}", version_number(version))
}

/// The version an A2UI extension URI names, without the `v`; `None` for a
/// URI of another extension.
pub fn extension_version(uri: &str) -> Option<&str> {
    uri.strip_prefix(EXTENSION_BASE_URI)?.strip_prefix("/v")
}

/// The A2UI entry of an agent card's `capabilities.extensions`.
///
/// `params` is there only when it says something: that the agent takes a
/// renderer's inline catalogs, or which catalogs it supports.
pub fn agent_extension(
    version: &str,
    accepts_inline_catalogs: bool,
    supported_catalog_ids: &[&str],
) -> Value {
    let mut params = Map::new();
    if accepts_inline_catalogs {
        params.insert("acceptsInlineCatalogs".into(), json!(true));
    }
    if !supported_catalog_ids.is_empty() {
        params.insert("supportedCatalogIds".into(), json!(supported_catalog_ids));
    }
    let mut extension = Map::new();
    extension.insert("uri".into(), json!(extension_uri(version)));
    extension.insert(
        "description".into(),
        json!("Provides agent driven UI using the A2UI JSON format."),
    );
    if !params.is_empty() {
        extension.insert("params".into(), Value::Object(params));
    }
    Value::Object(extension)
}

/// A version as numbers, for ordering: `1.10.0` is newer than `1.2.0`.
fn ordered(uri: &str) -> Vec<u64> {
    extension_version(uri)
        .unwrap_or_default()
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

/// The newest A2UI extension among those a client requested that the agent
/// also advertises.
pub fn select_newest<'a, R, A>(requested: &'a [R], advertised: &[A]) -> Option<&'a str>
where
    R: AsRef<str>,
    A: AsRef<str>,
{
    let mut newest: Option<&'a str> = None;
    for uri in requested.iter().map(AsRef::as_ref) {
        if !advertised.iter().any(|offered| offered.as_ref() == uri) {
            continue;
        }
        if newest.is_none_or(|best| ordered(uri) > ordered(best)) {
            newest = Some(uri);
        }
    }
    newest
}

/// Activates A2UI for a request: the newest extension the client requested
/// and the agent advertises, among those that name a version this crate
/// speaks. Gives the URI to report as activated, and the version to answer
/// in.
///
/// `requested` is what the request asked for (the A2A extensions header and
/// the message's `extensions`); `advertised` is what the agent card lists.
/// URIs of other extensions are passed over.
pub fn activate<'a, R, A>(requested: &'a [R], advertised: &[A]) -> Option<(&'a str, A2uiVersion)>
where
    R: AsRef<str>,
    A: AsRef<str>,
{
    let spoken = |uri: &str| {
        extension_version(uri).and_then(|number| A2uiVersion::parse(&format!("v{number}")))
    };
    let advertised: Vec<&str> = advertised
        .iter()
        .map(AsRef::as_ref)
        .filter(|uri| spoken(uri).is_some())
        .collect();
    let uri = select_newest(requested, &advertised)?;
    Some((uri, spoken(uri)?))
}

/// What the renderer behind an A2A message can draw, for `version`.
///
/// The capabilities are looked for where renderers put them, in this order:
/// the message's `metadata`, the `metadata` of each data part, and the
/// `metadata` inside the data of each data part, which is where older clients
/// wrote them. The first that has them wins. `None` when none does.
///
/// Only data parts are read: a part whose `kind` is `data`, or one written
/// without a `kind` that has `data`. A text or file part is passed over
/// whatever else it carries.
///
/// What is inside a part's data is the application's, and `metadata` there
/// may be a key of its own. So in that third place a `metadata` that is not
/// an object is not capabilities and is passed over, with the part, as the
/// reference does (`_capabilities_from_legacy_data` in `a2ui.a2a`); the
/// parts after it are still read.
///
/// # Errors
///
/// Returns [`Error::Catalog`](crate::Error::Catalog) when the capabilities
/// are there and malformed, as
/// [`RendererCapabilities::from_metadata`] does, and when the `metadata` of
/// the message or of a data part is not an object.
pub fn renderer_capabilities(
    message: &Value,
    version: A2uiVersion,
) -> Result<Option<RendererCapabilities>> {
    if let Some(found) = RendererCapabilities::from_metadata(message.get("metadata"), version)? {
        return Ok(Some(found));
    }
    let data_parts: Vec<&Value> = message
        .get("parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|part| is_data_part(part))
        .collect();
    for part in &data_parts {
        if let Some(found) = RendererCapabilities::from_metadata(part.get("metadata"), version)? {
            return Ok(Some(found));
        }
    }
    for part in &data_parts {
        let legacy = part
            .pointer("/data/metadata")
            .filter(|metadata| metadata.is_object());
        if let Some(found) = RendererCapabilities::from_metadata(legacy, version)? {
            return Ok(Some(found));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::CLIENT_CAPABILITIES_KEY;

    #[test]
    fn a_part_is_labelled_for_its_version() {
        let message = json!({"version": "v0.9.1", "deleteSurface": {"surfaceId": "s"}});
        assert_eq!(data_part(&message, None)["metadata"]["mimeType"], MIME_TYPE);
        assert_eq!(
            data_part(&message, Some("0.9"))["metadata"]["mimeType"],
            DEPRECATED_MIME_TYPE
        );
        // A message that states no version, labelled for none.
        let unversioned = json!({"foo": "bar"});
        assert_eq!(
            data_part(&unversioned, None)["metadata"]["mimeType"],
            DEPRECATED_MIME_TYPE
        );
        for (version, mime) in [
            ("v0.8", DEPRECATED_MIME_TYPE),
            ("0.9", DEPRECATED_MIME_TYPE),
            ("v0.9.1", MIME_TYPE),
            ("1.0", MIME_TYPE),
        ] {
            assert_eq!(mime_type(Some(version)), mime, "{version}");
        }
    }

    #[test]
    fn only_a_data_part_with_an_a2ui_mime_type_is_a2ui() {
        let message = json!({"version": "v0.9.1", "deleteSurface": {"surfaceId": "s"}});
        let part = data_part(&message, None);
        let text = json!({"kind": "text", "text": "hello", "metadata": {"mimeType": MIME_TYPE}});
        let other =
            json!({"kind": "data", "data": {}, "metadata": {"mimeType": "application/json"}});
        let bare = json!({"kind": "data", "data": {}});
        // A part written without `kind`, as some serializers leave it.
        let kindless = json!({"data": {"x": 1}, "metadata": {"mimeType": DEPRECATED_MIME_TYPE}});

        assert!(is_a2ui_part(&part) && is_a2ui_part(&kindless));
        assert!(!is_a2ui_part(&text) && !is_a2ui_part(&other) && !is_a2ui_part(&bare));
        assert_eq!(
            a2ui_messages([&text, &part, &other, &kindless]),
            [&message, &json!({"x": 1})]
        );
    }

    #[test]
    fn the_extension_says_only_what_differs_from_the_default() {
        let minimal = agent_extension("v0.9", false, &[]);
        assert_eq!(minimal["uri"], "https://a2ui.org/a2a-extension/a2ui/v0.9");
        assert!(minimal.get("params").is_none());
        assert_eq!(
            agent_extension("0.9.1", true, &["a", "b"])["params"],
            json!({"acceptsInlineCatalogs": true, "supportedCatalogIds": ["a", "b"]})
        );
        assert_eq!(extension_version(&extension_uri("v0.9.1")), Some("0.9.1"));
        assert_eq!(extension_version("https://example.com/other/v1"), None);
    }

    #[test]
    fn the_newest_version_both_sides_name_is_activated() {
        let v09 = extension_uri("0.9");
        let v091 = extension_uri("0.9.1");
        let other = "https://example.com/extension/v2".to_string();

        // Numbers, not text: 1.10 is newer than 1.2.
        let requested = [extension_uri("1.2.0"), extension_uri("1.10.0")];
        assert_eq!(
            select_newest(&requested, &requested),
            Some(requested[1].as_str())
        );

        let requested = [other.clone(), v09.clone(), v091.clone()];
        assert_eq!(
            activate(&requested, &[v09.clone(), v091.clone()]),
            Some((v091.as_str(), A2uiVersion::V0_9_1))
        );
        assert_eq!(
            activate(&requested, &[v09.as_str()]),
            Some((v09.as_str(), A2uiVersion::V0_9))
        );
        // Nothing requested, nothing in common, or a version this crate does
        // not speak: A2UI is not activated.
        let none: [&str; 0] = [];
        assert_eq!(activate(&none, &[v09.as_str()]), None);
        assert_eq!(activate(&[other.as_str()], &[other.as_str()]), None);
        let v08 = extension_uri("0.8");
        assert_eq!(
            activate(&[v08.as_str()], &[v08.as_str(), v09.as_str()]),
            None
        );
    }

    #[test]
    fn capabilities_are_read_from_the_message_then_its_parts() {
        let capabilities =
            |id: &str| json!({CLIENT_CAPABILITIES_KEY: {"v0.9": {"supportedCatalogIds": [id]}}});
        let supports = |message: &Value| {
            renderer_capabilities(message, A2uiVersion::V0_9)
                .unwrap()
                .map(|found| found.to_json()["v0.9"]["supportedCatalogIds"][0].clone())
        };

        let on_message = json!({"metadata": capabilities("m"), "parts": [
            {"kind": "data", "data": {}, "metadata": capabilities("p")}
        ]});
        assert_eq!(supports(&on_message), Some(json!("m")));

        let on_part = json!({"parts": [
            {"kind": "text", "text": "hi", "metadata": capabilities("text")},
            {"kind": "data", "data": {"metadata": capabilities("legacy")}, "metadata": capabilities("p")}
        ]});
        assert_eq!(supports(&on_part), Some(json!("p")));

        let legacy =
            json!({"parts": [{"kind": "data", "data": {"metadata": capabilities("legacy")}}]});
        assert_eq!(supports(&legacy), Some(json!("legacy")));

        assert_eq!(
            supports(&json!({"parts": [{"kind": "text", "text": "hi"}]})),
            None
        );
        // Malformed where it is found is an error, not an absence.
        let malformed =
            json!({"metadata": {CLIENT_CAPABILITIES_KEY: {"v0.9": {"supportedCatalogIds": "x"}}}});
        assert!(renderer_capabilities(&malformed, A2uiVersion::V0_9).is_err());
    }

    /// What a data part holds is the application's, and it may have a
    /// `metadata` of its own that has nothing to do with capabilities.
    #[test]
    fn a_data_part_whose_own_metadata_is_not_an_object_is_passed_over() {
        let capabilities =
            json!({CLIENT_CAPABILITIES_KEY: {"v0.9": {"supportedCatalogIds": ["c"]}}});
        let supports = |message: &Value| {
            renderer_capabilities(message, A2uiVersion::V0_9)
                .unwrap_or_else(|error| panic!("{message}: {error}"))
                .map(|found| found.to_json()["v0.9"]["supportedCatalogIds"][0].clone())
        };

        // The part after it is still read.
        let behind = json!({"parts": [
            {"kind": "data", "data": {"metadata": "build 42", "rows": [1, 2]}},
            {"kind": "data", "data": {"metadata": capabilities}},
        ]});
        assert_eq!(supports(&behind), Some(json!("c")));
        for own in [
            json!("build 42"),
            json!(7),
            json!([1]),
            json!(true),
            json!(null),
        ] {
            let alone = json!({"parts": [{"kind": "data", "data": {"metadata": own}}]});
            assert_eq!(supports(&alone), None, "{alone}");
        }
        // Data that is not an object has no metadata at all.
        let listed = json!({"parts": [{"kind": "data", "data": [1, 2]}]});
        assert_eq!(supports(&listed), None);

        // An object there is read, and one that is malformed is still an error.
        let malformed = json!({"parts": [{"kind": "data", "data": {"metadata": {
            CLIENT_CAPABILITIES_KEY: {"v0.9": {"supportedCatalogIds": "x"}}
        }}}]});
        assert!(renderer_capabilities(&malformed, A2uiVersion::V0_9).is_err());
    }

    /// A part that says what it is is taken at its word.
    #[test]
    fn only_data_parts_are_read_for_capabilities() {
        let capabilities =
            json!({CLIENT_CAPABILITIES_KEY: {"v0.9": {"supportedCatalogIds": ["c"]}}});
        let read = |message: &Value| {
            renderer_capabilities(message, A2uiVersion::V0_9)
                .unwrap_or_else(|error| panic!("{message}: {error}"))
        };

        // A text part with a `data` key is a text part.
        for part in [
            json!({"kind": "text", "text": "hi", "data": {"metadata": 7}}),
            json!({"kind": "text", "text": "hi", "data": {"metadata": capabilities}}),
            json!({"kind": "text", "text": "hi", "data": {}, "metadata": capabilities}),
            json!({"kind": "file", "file": {}, "data": {}, "metadata": "not an object"}),
        ] {
            assert!(read(&json!({"parts": [part]})).is_none(), "{part}");
        }
        // One that states no kind is a data part when it has data.
        let kindless = json!({"parts": [{"data": {}, "metadata": capabilities}]});
        assert!(read(&kindless).is_some());
    }
}
