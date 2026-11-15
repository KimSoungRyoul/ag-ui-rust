//! Full Draft 2020-12 validation with an explicitly local resource registry.

use super::operation::Operation;
use crate::toolkit::schema::SchemaBundle;
use crate::{Error, ErrorCode, Result, ValidationError};
use jsonschema::Evaluation;
use jsonschema::error::ValidationErrorKind;
use jsonschema::output::ErrorDescription;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug)]
struct NoExternalResources;
impl jsonschema::Retrieve for NoExternalResources {
    fn retrieve(
        &self,
        uri: &jsonschema::Uri<String>,
    ) -> std::result::Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Err(format!(
            "unregistered schema resource {uri}; network and filesystem retrieval are disabled"
        )
        .into())
    }
}

/// A compiled envelope and catalog schema. Construct once, reuse across requests.
/// Unknown `$ref`s fail construction; URLs never cause network requests.
#[derive(Debug)]
pub struct SchemaValidator {
    compiled: jsonschema::Validator,
    /// Every registered document by URI. A diagnostic carries only its
    /// keyword's absolute location, so a failed `enum` reads its allowed values
    /// back from here.
    documents: BTreeMap<String, Value>,
}
impl SchemaValidator {
    /// Compiles the official envelope against the supplied catalog and common types.
    /// Additional resources must be supplied locally, keyed by their absolute URI.
    pub fn new(bundle: &SchemaBundle, resources: &BTreeMap<String, Value>) -> Result<Self> {
        let mut documents = resources.clone();
        let mut register = |id: &str, value: &Value| -> Result<()> {
            if let Some(prior) = documents.get(id) {
                if prior != value {
                    return Err(Error::catalog(format!("conflicting schema resource {id}")));
                }
            }
            documents.insert(id.into(), value.clone());
            Ok(())
        };
        for document in [&bundle.s2c, &bundle.common_types, &bundle.catalog] {
            let id = document
                .get("$id")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::catalog("full schema documents require an absolute $id"))?;
            register(id, document)?;
        }
        // Official s2c references this relative URI while the catalog declares
        // its own, longer canonical URI. This is a resolver alias, not a wire ID alias.
        register(
            "https://a2ui.org/specification/v0_9/catalog.json",
            &bundle.catalog,
        )?;
        // Each document keeps the draft its own `$schema` declares. An unknown
        // `$schema` is a meta-schema the retriever would have to fetch, so it
        // fails here rather than reaching the network.
        let registry =
            jsonschema::Registry::new()
                .retriever(NoExternalResources)
                .extend(documents.iter().map(|(id, document)| {
                    (id, jsonschema::Resource::from_contents(document.clone()))
                }))
                .and_then(jsonschema::RegistryBuilder::prepare)
                .map_err(Error::catalog)?;
        let compiled = jsonschema::options()
            .with_draft(jsonschema::Draft::Draft202012)
            .with_retriever(NoExternalResources)
            .with_registry(&registry)
            .build(&bundle.s2c)
            .map_err(Error::catalog)?;
        Ok(Self {
            compiled,
            documents,
        })
    }
    /// Revalidates a complete observed surface, including prior component definitions
    /// and its creation theme. Useful when restoring state or changing validators.
    pub fn validate_surface(
        &self,
        surface: &crate::protocol::surface::SurfaceState,
        catalog: &crate::Catalog,
    ) -> Result<()> {
        self.validate_surface_in(None, surface, catalog)
    }

    /// [`SchemaValidator::validate_surface`], for a surface `store` holds.
    ///
    /// The store knows which of its messages wrote each component, so a
    /// semantic or composition failure names that message and the position in
    /// it — `messages[1].updateComponents.components[0]` — instead of the
    /// component's position in the folded surface. Schema failures are located
    /// in the surface as a whole, since that is the document the schema is
    /// given here; each message was already checked against it on its own.
    pub(crate) fn validate_surface_in(
        &self,
        store: Option<&crate::protocol::surface::SurfaceStore>,
        surface: &crate::protocol::surface::SurfaceState,
        catalog: &crate::Catalog,
    ) -> Result<()> {
        let create = crate::AgentMessage {
            version: surface.version.clone(),
            payload: crate::AgentPayload::CreateSurface(crate::protocol::message::CreateSurface {
                surface_id: surface.surface_id.clone(),
                catalog_id: surface.catalog_id.clone(),
                theme: surface.theme.clone(),
                send_data_model: surface.send_data_model,
            }),
        };
        self.validate_message(&serde_json::to_value(create)?)?;
        let components = crate::AgentMessage {
            version: surface.version.clone(),
            payload: crate::AgentPayload::UpdateComponents(
                crate::protocol::message::UpdateComponents {
                    surface_id: surface.surface_id.clone(),
                    components: surface.components.clone(),
                },
            ),
        };
        self.validate_message(&serde_json::to_value(components)?)?;
        let validator = crate::Validator::new(catalog);
        match store {
            Some(store) => validator.validate_stored(store, surface),
            None => validator.validate_model(&surface.components, &surface.data_model),
        }
        .into_result()?;
        let mut composition = catalog.composition_violations(&surface.components);
        if let Some(origin) = store.and_then(|store| store.origin(&surface.surface_id)) {
            for violation in &mut composition {
                // `components[3].children[0]`: the position in the surface,
                // then where in that component.
                let located = violation
                    .path
                    .strip_prefix("components[")
                    .and_then(|rest| rest.split_once(']'))
                    .and_then(|(position, rest)| {
                        let component = surface.components.get(position.parse::<usize>().ok()?)?;
                        Some(format!("{}{rest}", origin.component(&component.id)?))
                    });
                if let Some(located) = located {
                    violation.path = located;
                }
            }
        }
        if !composition.is_empty() {
            return Err(Error::catalog(format!(
                "component composition constraints failed: {composition:?}"
            )));
        }
        Ok(())
    }

    /// Checks raw wire JSON before serde defaults or field filtering can change it.
    pub fn validate_message(&self, message: &Value) -> Result<()> {
        let errors: Vec<_> = self.compiled.iter_errors(message).collect();
        if errors.is_empty() {
            return Ok(());
        }
        let details =
            schema_diagnostics(message, &self.compiled.evaluate(message), &self.documents);
        Err(Error::Validation {
            errors: if details.is_empty() {
                errors
                    .into_iter()
                    .take(16)
                    .map(|error| {
                        ValidationError::new(
                            ErrorCode::InvalidValue,
                            error.instance_path().to_string(),
                            bounded_diagnostic(&masked_message(&error)),
                        )
                    })
                    .collect::<Vec<_>>()
            } else {
                details
            }
            .into(),
        })
    }
}

// jsonschema 0.61 words a failed `enum` as `"x" is not one of "a", "b" or 5
// other candidates`. A model correcting its own output in the retry loop needs
// every allowed value, which 0.29 gave it as `"x" is not one of ["a","b",…]`, so
// these keep 0.29's wording. Every other message is the engine's own.

/// `error`'s message, naming every allowed value of a failed `enum`.
pub(crate) fn message(error: &jsonschema::ValidationError<'_>) -> String {
    match error.kind() {
        ValidationErrorKind::Enum { options } => enum_message(error.instance(), options),
        _ => error.to_string(),
    }
}

/// [`message`] with the instance replaced by `value`, so no input is echoed.
fn masked_message(error: &jsonschema::ValidationError<'_>) -> String {
    match error.kind() {
        ValidationErrorKind::Enum { options } => enum_message("value", options),
        _ => error.masked().to_string(),
    }
}

fn enum_message(value: impl std::fmt::Display, options: &Value) -> String {
    format!("{value} is not one of {options}")
}

/// The `enum` array a diagnostic's keyword URI points at, such as
/// `https://…/catalog.json#/components/Text/allOf/2/properties/variant/enum`.
/// `None` when the document is not one of ours or the pointer misses.
fn enum_options<'a>(
    documents: &'a BTreeMap<String, Value>,
    schema_location: &str,
) -> Option<&'a Value> {
    let (document, _) = schema_location.split_once('#')?;
    let uri = jsonschema::uri::from_str(schema_location).ok()?;
    let pointer = uri.fragment()?.decode().to_string().ok()?;
    documents
        .get(document)?
        .pointer(&pointer)
        .filter(|options| options.is_array())
}

fn bounded_diagnostic(message: &str) -> String {
    let mut chars = message.chars();
    let mut result: String = chars.by_ref().take(384).collect();
    if chars.next().is_some() {
        result.push('…');
    }
    result
}

/// For `…/oneOf/1/type` (or `anyOf`, through any `$ref`s), the combinator
/// `…/oneOf` and the branch `1` whose own `type` the value failed.
fn type_only_branch(keyword: &str) -> Option<(&str, usize)> {
    let mut branch = keyword.strip_suffix("/type")?;
    while let Some(rest) = branch.strip_suffix("/$ref") {
        branch = rest;
    }
    let at = branch.rfind("/oneOf/").max(branch.rfind("/anyOf/"))?;
    let index = branch[at + "/oneOf/".len()..].parse().ok()?;
    Some((&branch[..at + "/oneOf".len()], index))
}

fn at_or_below(path: &str, ancestor: &str) -> bool {
    path.strip_prefix(ancestor)
        .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with('/'))
}

/// Every error in the evaluation tree, in pre-order, keyed by its evaluation
/// path (the keywords followed from the root, `$ref`s included). The branch
/// matching below reads `oneOf`/`anyOf` indices off that path, so the absolute
/// schema location `Evaluation::iter_errors` reports would not do; that
/// location rides along for [`enum_options`].
fn evaluation_errors(evaluation: &Evaluation) -> Vec<(&str, &str, (&str, &ErrorDescription))> {
    let mut errors = Vec::new();
    let mut stack = vec![evaluation.root()];
    while let Some(unit) = stack.pop() {
        let path = unit.evaluation_path().as_str();
        for error in unit.errors() {
            errors.push((
                path,
                unit.instance_location().as_str(),
                (unit.schema_location(), error),
            ));
        }
        // `unevaluatedProperties` already names every offending property in
        // its own error. Its children are the per-property subschema results
        // (`False schema does not allow …` for each one), which would only
        // repeat that error one property at a time.
        if path.ends_with("/unevaluatedProperties") {
            continue;
        }
        let start = stack.len();
        stack.extend(unit.children());
        stack[start..].reverse();
    }
    errors
}

fn schema_diagnostics(
    instance: &Value,
    evaluation: &Evaluation,
    documents: &BTreeMap<String, Value>,
) -> Vec<ValidationError> {
    if evaluation.flag().valid {
        return Vec::new();
    }
    let details = evaluation_errors(evaluation);
    // Discriminator failures belong to other catalog branches. Scope exclusions
    // by instance because another component may match that same schema branch.
    let unrelated: Vec<_> = details
        .iter()
        .filter_map(|(keyword, location, _)| {
            let component = location.strip_suffix("/component")?;
            if !keyword.ends_with("/properties/component/const") {
                return None;
            }
            let (_, branch) = keyword
                .rsplit_once("/oneOf/")
                .or_else(|| keyword.rsplit_once("/anyOf/"))?;
            let end = keyword.len() - branch.len() + branch.split('/').next()?.len();
            Some((component, &keyword[..end]))
        })
        .collect();
    let selected: Vec<_> = details
        .into_iter()
        .filter(|(keyword, location, _)| {
            !location.is_empty()
                && !unrelated.iter().any(|(component, branch)| {
                    at_or_below(location, component) && at_or_below(keyword, branch)
                })
        })
        .collect();
    // A oneOf/anyOf branch that fails on the value's JSON type alone is an
    // alternative the value never tried to be: the template form of a child
    // list, when the value is an array. Report the branch it did try.
    let untried: Vec<_> = selected
        .iter()
        .map(|(keyword, _, _)| type_only_branch(keyword))
        .collect();
    let tried_another = |combinator: &str, branch: usize| {
        selected
            .iter()
            .zip(&untried)
            .any(|((keyword, _, _), other)| {
                let Some(index) = keyword
                    .strip_prefix(combinator)
                    .and_then(|rest| rest.strip_prefix('/'))
                    .and_then(|rest| rest.split('/').next())
                    .and_then(|index| index.parse::<usize>().ok())
                else {
                    return false;
                };
                index != branch && *other != Some((combinator, index))
            })
    };
    let mut results = Vec::new();
    for ((keyword, location, (schema_location, error)), untried) in selected.iter().zip(&untried) {
        if untried.is_some_and(|(combinator, branch)| tried_another(combinator, branch)) {
            continue;
        }
        // Failed allOf branches also emit secondary unevaluated-property errors.
        if keyword.ends_with("/unevaluatedProperties")
            && selected.iter().any(|(other, path, _)| {
                !other.ends_with("/unevaluatedProperties") && at_or_below(path, location)
            })
        {
            continue;
        }
        let failed = instance.pointer(location);
        // The evaluation keeps only the rendered message, so a failed `enum`
        // reads its options back from the schema it came from.
        let mut description = match (error.keyword(), failed) {
            ("enum", Some(value)) => enum_options(documents, schema_location)
                .map_or_else(|| error.to_string(), |options| enum_message(value, options)),
            _ => error.to_string(),
        };
        if let Some(value) = failed {
            if let Some(suffix) = description.strip_prefix(&value.to_string()) {
                description = format!("value{suffix}");
            }
        }
        let diagnostic = ValidationError::new(
            ErrorCode::InvalidValue,
            *location,
            bounded_diagnostic(&description),
        );
        if !results.contains(&diagnostic) {
            results.push(diagnostic);
        }
        if results.len() == 16 {
            break;
        }
    }
    results
}

/// A stateful validator for one renderer context containing multiple catalogs
/// and surfaces. `push` accepts forward references; `finish` requires complete trees.
#[derive(Debug, Default)]
pub struct SurfaceValidator {
    catalogs: BTreeMap<String, (SchemaValidator, crate::Catalog)>,
    state: crate::protocol::surface::SurfaceStore,
}
impl SurfaceValidator {
    /// Empty renderer state and catalog registry.
    pub fn new() -> Self {
        Self::default()
    }
    /// Begins a partial update stream from explicit, previously observed state.
    ///
    /// The messages pushed from here on are numbered from zero; a component
    /// the supplied state already held is located by its position in its
    /// surface.
    pub fn from_state(mut state: crate::protocol::surface::SurfaceStore) -> Self {
        state.begin_batch();
        Self {
            state,
            ..Self::default()
        }
    }
    /// Registers a complete local catalog contract. Conflicting duplicate IDs
    /// are rejected; create a new validator to change a registered contract.
    pub fn register(
        &mut self,
        bundle: &SchemaBundle,
        resources: &BTreeMap<String, Value>,
    ) -> Result<()> {
        let catalog = crate::Catalog::from_schema(&bundle.catalog)?;
        if self.catalogs.contains_key(&catalog.catalog_id) {
            return Err(Error::catalog("catalog ID is already registered"));
        }
        let validator = SchemaValidator::new(bundle, resources)?;
        self.catalogs
            .insert(catalog.catalog_id.clone(), (validator, catalog));
        Ok(())
    }
    /// Validates raw JSON before decoding, then applies one message atomically.
    /// Unknown catalogs never cause network requests. Invalid messages leave
    /// all previously accepted state unchanged.
    ///
    /// A semantic failure names the message that wrote the component, counting
    /// the messages this validator has accepted: the third one accepted is
    /// `messages[2]`, and a rejected message takes no number.
    pub fn push(&mut self, raw: &Value) -> Result<()> {
        let catalog_id = if let Some(create) = raw.get(Operation::CreateSurface.as_str()) {
            create
                .get("catalogId")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::catalog("createSurface.catalogId is required"))?
        } else {
            let surface_id = Operation::ALL
                .iter()
                .find_map(|operation| {
                    raw.get(operation.as_str())
                        .and_then(|p| p.get("surfaceId"))
                        .and_then(Value::as_str)
                })
                .ok_or_else(|| Error::parse("expected a v0.9-family server message"))?;
            &self
                .state
                .get(surface_id)
                .ok_or_else(|| Error::parse("surface must be created before updating it"))?
                .catalog_id
        };
        let (schema, catalog) = self
            .catalogs
            .get(catalog_id)
            .ok_or_else(|| Error::catalog(format!("unregistered catalog {catalog_id}")))?;
        schema.validate_message(raw)?;
        let op: crate::AgentMessage = serde_json::from_value(raw.clone())?;
        let mut next = self.state.clone();
        next.apply(&op)?;
        if let Some(surface) = op
            .surface_id()
            .and_then(|id| next.get(id))
            .filter(|s| !s.deleted && !s.components.is_empty())
        {
            let options = crate::ValidateOptions {
                check_bindings: false,
                ..crate::ValidateOptions::incremental_update()
            };
            crate::Validator::with_options(catalog, options)
                .validate_stored(&next, surface)
                .into_result()?;
        }
        self.state = next;
        Ok(())
    }
    /// Checks pending roots, references, and bindings at the end of a batch.
    ///
    /// Failures are located the way [`SurfaceValidator::push`] locates them.
    pub fn finish(&self) -> Result<()> {
        for surface in self.state.surfaces().filter(|s| !s.deleted) {
            let (schema, catalog) = self.catalogs.get(&surface.catalog_id).ok_or_else(|| {
                Error::catalog(format!("unregistered catalog {}", surface.catalog_id))
            })?;
            schema.validate_surface_in(Some(&self.state), surface, catalog)?;
        }
        Ok(())
    }
    /// Observed accepted state, for incremental streams and lossless replay.
    pub fn state(&self) -> &crate::protocol::surface::SurfaceStore {
        &self.state
    }
}

/// Validates a value against one definition of the published common types,
/// `common_types.json`: `DynamicString`, `DynamicValue`, `ChildList`,
/// `Action`, `FunctionCall` and the rest of its `$defs`.
///
/// The common types are what a catalog builds its properties from, so this is
/// the check for one value on its own: a literal a catalog author wants to
/// try, or a value an agent is about to put into a component. A whole message
/// is [`SchemaValidator::validate_message`]'s to check.
///
/// The document is the one this crate ships, which v0.9 and v0.9.1 share.
///
/// # Function calls
///
/// In the published document a `FunctionCall` is whatever the surface's
/// catalog says one is, through `catalog.json#/$defs/anyFunction`. A value on
/// its own has no catalog. So a call is held to the type's own shape, with any
/// function name: `call`, optional `args` and `returnType`, and no other key.
/// Whether a catalog defines the function, and takes those arguments, is the
/// message validator's to say.
///
/// ```
/// use ag_ui_a2ui::schema_validation::validate_common_type;
/// use serde_json::json;
///
/// validate_common_type("ChildList", &json!(["header", "body"]))?;
/// validate_common_type("ChildList", &json!({"componentId": "row", "path": "/items"}))?;
/// assert!(validate_common_type("ChildList", &json!("header")).is_err());
///
/// // A binding and a call are different values: an object that is both is neither.
/// validate_common_type("DynamicValue", &json!({"path": "/user/name"}))?;
/// validate_common_type("DynamicValue", &json!({"call": "now"}))?;
/// assert!(validate_common_type("DynamicValue", &json!({"path": "/a", "call": "now"})).is_err());
/// # Ok::<(), ag_ui_a2ui::Error>(())
/// ```
///
/// # Errors
///
/// Returns [`Error::Validation`] with one `invalid_value` error for each
/// failure, located by JSON Pointer within `value`, and [`Error::Catalog`]
/// when the common types define no `definition`.
pub fn validate_common_type(definition: &str, value: &Value) -> Result<()> {
    const CATALOG: &str = "https://a2ui.org/specification/v0_9/catalog.json";
    let common: Value =
        serde_json::from_str(include_str!("../../schemas/v0_9_1/common_types.json"))?;
    let defined = common.get("$defs").and_then(Value::as_object);
    if !defined.is_some_and(|defined| defined.contains_key(definition)) {
        let names: Vec<&str> = defined
            .into_iter()
            .flat_map(|defined| defined.keys().map(String::as_str))
            .collect();
        return Err(Error::catalog(format!(
            "the common types define no {definition:?}; they define {}",
            names.join(", ")
        )));
    }
    let id = common
        .get("$id")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::catalog("the common types document has no $id"))?
        .to_string();
    // The catalog the common types refer to, standing in for all of them: any
    // function, called with the keys a call has.
    let any_catalog = serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": CATALOG,
        "$defs": {"anyFunction": {
            "type": "object",
            "properties": {"call": true, "args": true, "returnType": true},
            "additionalProperties": false
        }}
    });
    let pointer = definition.replace('~', "~0").replace('/', "~1");
    let schema = serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$ref": format!("{id}#/$defs/{pointer}"),
    });
    let registry = jsonschema::Registry::new()
        .retriever(NoExternalResources)
        .extend([
            (id, jsonschema::Resource::from_contents(common)),
            (
                CATALOG.to_string(),
                jsonschema::Resource::from_contents(any_catalog),
            ),
        ])
        .and_then(jsonschema::RegistryBuilder::prepare)
        .map_err(Error::catalog)?;
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .with_retriever(NoExternalResources)
        .with_registry(&registry)
        .build(&schema)
        .map_err(Error::catalog)?;
    let errors: Vec<ValidationError> = validator
        .iter_errors(value)
        .take(16)
        .map(|error| {
            ValidationError::new(
                ErrorCode::InvalidValue,
                error.instance_path().to_string(),
                bounded_diagnostic(&masked_message(&error)),
            )
        })
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(Error::Validation {
            errors: errors.into(),
        })
    }
}

/// Validates raw `a2uiClientCapabilities` metadata using the pinned official schema.
pub fn validate_capabilities(value: &Value) -> Result<()> {
    let schema: Value = serde_json::from_str(include_str!(
        "../../schemas/v0_9_1/client_capabilities.json"
    ))?;
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .with_retriever(NoExternalResources)
        .build(&schema)
        .map_err(Error::catalog)?;
    validator
        .validate(value)
        .map_err(|e| Error::catalog(message(&e)))
}
