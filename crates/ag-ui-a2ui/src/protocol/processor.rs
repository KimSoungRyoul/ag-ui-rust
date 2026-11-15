//! A message processor: the surfaces of one renderer context, the catalogs
//! they may speak, and the one way a raw message gets into them.
//!
//! [`SurfaceStore`] is the state machine, and it takes typed messages as they
//! are. A [`MessageProcessor`] is what stands between the wire and a store:
//!
//! - **A catalog registry.** A surface names its catalog by id when it is
//!   created. The processor holds the catalogs this context speaks, refuses a
//!   surface whose catalog it does not hold, and validates every surface
//!   against its own.
//! - **Raw messages, checked in one order.** The envelope and the nesting of
//!   the JSON as it arrived; then the surface's place in its life; then the
//!   components, against the catalog and as a graph.
//! - **Transactions.** A message changes the state or is refused and changes
//!   nothing, and so does a batch ([`MessageProcessor::process_all`]).
//! - **Why.** A refusal is a [`Rejection`]: its kind, its surface, the
//!   position of the message in its batch, and the [`Error`] itself.
//!
//! Nothing here is specific to an agent or to a renderer. An agent holds the
//! state it has sent in one; a renderer written in Rust, or a test standing in
//! for one, takes messages in through one.
//!
//! # The kinds of refusal
//!
//! The A2UI reference SDKs and the conformance suites name the ways a message
//! is refused. [`RejectionKind`] is those names, and each is carried by one
//! variant of [`Error`]:
//!
//! | Suite category | [`RejectionKind`] | [`Error`] | What it is |
//! |---|---|---|---|
//! | `IntegrityError` | [`Integrity`](RejectionKind::Integrity) | [`Validation`](Error::Validation) | The surface is not in the state the message needs: created while it is live, or updated while it is not. One error, `invalid_value` at `messages[i]`. Also a child reference to a component that does not exist (`unresolved_child`). |
//! | `RecursionError` | [`Recursion`](RejectionKind::Recursion) | [`Validation`](Error::Validation) | The component graph loops (`child_cycle`) or nests too deep (`max_depth_exceeded`). |
//! | `ValidationError` | [`Validation`](RejectionKind::Validation) | [`Validation`](Error::Validation) | Anything else a message gets wrong: its envelope, a component, a property, a path. |
//! | `CatalogError` | [`Catalog`](RejectionKind::Catalog) | [`Catalog`](Error::Catalog) | The surface names a catalog that is not registered. |
//!
//! A message that is not JSON of the expected shape is a `Validation` refusal
//! too, located at `messages[i]`: no [`Error::Json`] or [`Error::Parse`] leaves
//! a processor.
//!
//! In the reference SDKs `IntegrityError` and `RecursionError` are kinds of
//! `ValidationError`, so a suite that expects a `ValidationError` accepts
//! either. [`RejectionKind::is`] answers a category that way.
//!
//! # Deleting a surface that is not there
//!
//! `deleteSurface` for a surface that is not live does nothing and is not an
//! error: that is the reference SDKs' rule (`core/message_processor_v1_0.yaml`
//! states it; the v0.9 suite has no case either way), and deleting twice is
//! harmless. The rule lives here. [`SurfaceStore::apply`] stays strict, and
//! refuses such a delete.

use std::collections::BTreeMap;
use std::fmt;

use serde_json::{Map, Value};

use super::catalog::Catalog;
use super::message::{AgentMessage, RendererError, RendererMessage, RendererPayload};
use super::operation::Operation;
use super::surface::{Lifecycle, SurfaceStore};
use super::validate::{
    ErrorCode, ValidateOptions, ValidationError, ValidationReport, Validator, check_envelope,
    check_value_depth, foldable,
};
use super::version::A2uiVersion;
use crate::error::{Error, Result, ValidationErrors};

/// The surfaces of one renderer context and the catalogs they may speak.
///
/// ```
/// use ag_ui_a2ui::processor::{MessageProcessor, RejectionKind};
/// use ag_ui_a2ui::{Catalog, Lifecycle};
/// use serde_json::json;
///
/// let catalog = Catalog::basic();
/// let catalog_id = catalog.catalog_id.clone();
/// let mut processor = MessageProcessor::new();
/// processor.register(catalog)?;
///
/// processor.process_all(&[
///     json!({"version": "v0.9", "createSurface": {"surfaceId": "main", "catalogId": catalog_id}}),
///     json!({"version": "v0.9", "updateComponents": {"surfaceId": "main", "components": [
///         {"id": "root", "component": "Text", "text": "Hello"}
///     ]}}),
/// ])?;
/// assert_eq!(processor.surfaces().lifecycle("main"), Lifecycle::Live);
///
/// // A refused message changes nothing, and says what kind of refusal it is.
/// let again = json!({"version": "v0.9", "createSurface": {"surfaceId": "main", "catalogId": catalog_id}});
/// let refused = processor.process(&again).unwrap_err();
/// assert_eq!(refused.kind, RejectionKind::Integrity);
/// assert_eq!(refused.surface_id.as_deref(), Some("main"));
///
/// let unknown = json!({"version": "v0.9", "createSurface": {"surfaceId": "side", "catalogId": "nope"}});
/// assert_eq!(processor.process(&unknown).unwrap_err().kind, RejectionKind::Catalog);
/// assert_eq!(processor.surfaces().lifecycle("side"), Lifecycle::Absent);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct MessageProcessor {
    catalogs: BTreeMap<String, Catalog>,
    options: ValidateOptions,
    store: SurfaceStore,
}

impl Default for MessageProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageProcessor {
    /// A processor for a stream in flight: after each message, the surface it
    /// changed must be valid as far as it goes.
    ///
    /// That is [`ValidateOptions::incremental_update`] with bindings left
    /// unresolved: component types, required properties, property types,
    /// duplicate ids and cycles are checked, while the root and the children a
    /// component names may still be on their way, and so may the data.
    pub fn new() -> Self {
        Self::with_options(ValidateOptions {
            check_bindings: false,
            ..ValidateOptions::incremental_update()
        })
    }

    /// A processor that holds each surface to `options` after every message
    /// that changes its components, and after every `updateDataModel` when
    /// `options.check_bindings` is set.
    ///
    /// With [`ValidateOptions::full_surface`] a surface has to be complete
    /// after each message: a root, every child present.
    pub fn with_options(options: ValidateOptions) -> Self {
        Self {
            catalogs: BTreeMap::new(),
            options,
            store: SurfaceStore::new(),
        }
    }

    /// Starts from surfaces observed earlier. Their catalogs still have to be
    /// registered before a message can change their components.
    #[must_use]
    pub fn with_state(mut self, mut store: SurfaceStore) -> Self {
        store.begin_batch();
        self.store = store;
        self
    }

    /// Registers a catalog under its `catalog_id`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Catalog`] when another catalog is registered under the
    /// same id. Registering the same catalog again is not an error.
    pub fn register(&mut self, catalog: Catalog) -> Result<()> {
        match self.catalogs.get(&catalog.catalog_id) {
            Some(held) if *held == catalog => Ok(()),
            Some(_) => Err(Error::catalog(format!(
                "another catalog is already registered as {:?}",
                catalog.catalog_id
            ))),
            None => {
                self.catalogs.insert(catalog.catalog_id.clone(), catalog);
                Ok(())
            }
        }
    }

    /// A registered catalog.
    pub fn catalog(&self, catalog_id: &str) -> Option<&Catalog> {
        self.catalogs.get(catalog_id)
    }

    /// The ids of the registered catalogs, in order.
    pub fn catalog_ids(&self) -> impl Iterator<Item = &str> {
        self.catalogs.keys().map(String::as_str)
    }

    /// The state the accepted messages have left.
    pub fn surfaces(&self) -> &SurfaceStore {
        &self.store
    }

    /// Takes one raw message in, or refuses it and changes nothing.
    ///
    /// # Errors
    ///
    /// Returns a [`Rejection`] whose [`Rejection::message_index`] is `0`.
    pub fn process(&mut self, message: &Value) -> std::result::Result<(), Rejection> {
        self.process_all(std::slice::from_ref(message))
    }

    /// Takes a batch of raw messages in, in order, as one transaction: every
    /// message is accepted, or the first one that is not is returned and the
    /// state is what it was before the batch.
    ///
    /// Each message is checked as [`MessageProcessor`] describes, against the
    /// state the messages before it in the batch left. Locators name the
    /// messages of this batch from zero, `messages[1].updateComponents…`; a
    /// component an earlier batch wrote is located by its position in its
    /// surface, `components[3]`.
    ///
    /// # Errors
    ///
    /// Returns the [`Rejection`] of the first message that is refused.
    pub fn process_all(&mut self, messages: &[Value]) -> std::result::Result<(), Rejection> {
        let mut next = self.store.clone();
        next.begin_batch();
        for (index, message) in messages.iter().enumerate() {
            self.step(&mut next, index, message)?;
        }
        self.store = next;
        Ok(())
    }

    /// Validates one live surface as it stands, by this processor's options
    /// and against the surface's own catalog.
    ///
    /// The whole report, with what an accepted message does not return: the
    /// components the root cannot reach ([`ValidationReport::unreachable`]).
    /// `None` for a surface that is not live, or whose catalog is not
    /// registered.
    pub fn report(&self, surface_id: &str) -> Option<ValidationReport> {
        let surface = self.store.get(surface_id).filter(|s| !s.deleted)?;
        let catalog = self.catalogs.get(&surface.catalog_id)?;
        Some(
            Validator::with_options(catalog, self.options.clone())
                .validate_stored(&self.store, surface),
        )
    }

    /// One message, against `store`. An error leaves `store` in an unknown
    /// state: the caller owns a copy and drops it.
    fn step(
        &self,
        store: &mut SurfaceStore,
        index: usize,
        message: &Value,
    ) -> std::result::Result<(), Rejection> {
        let locator = format!("messages[{index}]");
        let invalid = |errors: Vec<ValidationError>| Rejection::invalid(index, message, errors);
        let at_message = |code: ErrorCode, reason: String| {
            invalid(vec![ValidationError::new(code, locator.clone(), reason)])
        };

        // The message as a document: its envelope, and how deep it nests.
        let mut report = ValidationReport::default();
        if self.options.check_envelope {
            check_envelope(message, &locator, &mut report);
        }
        check_value_depth(
            message,
            &locator,
            1,
            self.options.max_depth,
            self.options.max_function_call_depth,
            &mut report,
        );
        if !report.errors.is_empty() {
            return Err(invalid(report.errors));
        }

        // The surface's place in its life, and its catalog.
        let surface_id = surface_id_of(message).unwrap_or_default();
        let lifecycle = store.lifecycle(surface_id);
        // A fault of the surface's life is one error, at the message.
        let out_of_place = |reason: String| {
            let error = ValidationError::new(ErrorCode::InvalidValue, locator.clone(), reason);
            Rejection::new(
                RejectionKind::Integrity,
                index,
                message,
                Error::Validation {
                    errors: ValidationErrors(vec![error]),
                },
            )
        };
        let operation = Operation::of(message);
        match operation {
            Some(Operation::CreateSurface) => {
                let catalog_id = message
                    .pointer("/createSurface/catalogId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !self.catalogs.contains_key(catalog_id) {
                    return Err(unregistered(index, message, catalog_id));
                }
                if lifecycle == Lifecycle::Live {
                    return Err(out_of_place(format!(
                        "surface {surface_id:?} already exists"
                    )));
                }
            }
            // Nothing to delete is nothing to do.
            Some(Operation::DeleteSurface) if lifecycle != Lifecycle::Live => return Ok(()),
            Some(Operation::UpdateComponents | Operation::UpdateDataModel)
                if lifecycle != Lifecycle::Live =>
            {
                return Err(out_of_place(match lifecycle {
                    Lifecycle::Deleted => format!(
                        "surface {surface_id:?} was deleted; create it again before updating it"
                    ),
                    _ => format!(
                        "surface {surface_id:?} does not exist; create it before updating it"
                    ),
                }));
            }
            _ => {}
        }

        // Components the store cannot key by id are located one by one; serde
        // would only say that the message did not decode.
        let components = message
            .pointer("/updateComponents/components")
            .and_then(Value::as_array);
        if let Some(components) = components.filter(|components| !foldable(components)) {
            let catalog = self.catalog_of(store, index, message, surface_id)?;
            let report = Validator::with_options(catalog, self.options.clone())
                .validate_unfolded(index, components);
            return Err(if report.errors.is_empty() {
                at_message(
                    ErrorCode::InvalidValue,
                    "the components of this message cannot be stored".to_string(),
                )
            } else {
                invalid(report.errors)
            });
        }

        let typed: AgentMessage = serde_json::from_value(message.clone())
            .map_err(|error| at_message(ErrorCode::InvalidValue, error.to_string()))?;
        store
            .apply_at(index, &typed)
            .map_err(|error| at_message(ErrorCode::InvalidValue, error.to_string()))?;

        // The surface the message changed, as a whole.
        let changed = match operation {
            Some(Operation::UpdateComponents) => true,
            Some(Operation::UpdateDataModel) => self.options.check_bindings,
            _ => false,
        };
        if changed {
            let surface = store
                .get(surface_id)
                .filter(|surface| !surface.deleted && !surface.components.is_empty());
            if let Some(surface) = surface {
                let catalog = self.catalog_of(store, index, message, surface_id)?;
                let report = Validator::with_options(catalog, self.options.clone())
                    .validate_stored(store, surface);
                if !report.errors.is_empty() {
                    return Err(invalid(report.errors));
                }
            }
        }
        Ok(())
    }

    /// The catalog of a surface `store` holds.
    fn catalog_of(
        &self,
        store: &SurfaceStore,
        index: usize,
        message: &Value,
        surface_id: &str,
    ) -> std::result::Result<&Catalog, Rejection> {
        let catalog_id = store
            .get(surface_id)
            .map(|surface| surface.catalog_id.as_str())
            .unwrap_or_default();
        self.catalogs
            .get(catalog_id)
            .ok_or_else(|| unregistered(index, message, catalog_id))
    }
}

fn unregistered(index: usize, message: &Value, catalog_id: &str) -> Rejection {
    Rejection::new(
        RejectionKind::Catalog,
        index,
        message,
        Error::catalog(format!("catalog {catalog_id:?} is not registered")),
    )
}

/// The `surfaceId` a raw message names, when it names one as a string.
fn surface_id_of(message: &Value) -> Option<&str> {
    let operation = Operation::of(message)?;
    message.get(operation.as_str())?.get("surfaceId")?.as_str()
}

/// What kind of refusal a [`Rejection`] is.
///
/// The names are the error categories of the A2UI reference SDKs and
/// conformance suites; see the [module documentation](self) for what each
/// covers and which [`Error`] carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RejectionKind {
    /// The surface is not in the state the message needs, or a component
    /// names a child that does not exist.
    Integrity,
    /// The component graph loops, or nests too deep.
    Recursion,
    /// The surface names a catalog that is not registered.
    Catalog,
    /// Anything else the message gets wrong.
    Validation,
}

impl RejectionKind {
    /// The category the conformance suites call this kind: `IntegrityError`,
    /// `RecursionError`, `CatalogError` or `ValidationError`.
    pub const fn category(self) -> &'static str {
        match self {
            Self::Integrity => "IntegrityError",
            Self::Recursion => "RecursionError",
            Self::Catalog => "CatalogError",
            Self::Validation => "ValidationError",
        }
    }

    /// Whether this kind is one a conformance `category` names: its own, or
    /// `ValidationError` for the two kinds the reference SDKs make kinds of
    /// it, [`Integrity`](Self::Integrity) and [`Recursion`](Self::Recursion).
    ///
    /// ```
    /// use ag_ui_a2ui::processor::RejectionKind;
    ///
    /// assert!(RejectionKind::Integrity.is("IntegrityError"));
    /// assert!(RejectionKind::Integrity.is("ValidationError"));
    /// assert!(!RejectionKind::Validation.is("IntegrityError"));
    /// assert!(!RejectionKind::Catalog.is("ValidationError"));
    /// ```
    pub fn is(self, category: &str) -> bool {
        category == self.category()
            || (category == Self::Validation.category()
                && matches!(self, Self::Integrity | Self::Recursion))
    }

    /// The `code` a reference renderer reports a refusal of this kind under.
    ///
    /// The v0.9 specification defines `VALIDATION_FAILED` and leaves every
    /// other code open; the rest are the reference renderer's.
    pub const fn renderer_code(self) -> &'static str {
        match self {
            Self::Integrity => "INTEGRITY_ERROR",
            Self::Recursion => "RECURSION_LIMIT_EXCEEDED",
            Self::Catalog => "CATALOG_NOT_FOUND",
            Self::Validation => "VALIDATION_FAILED",
        }
    }

    /// The kind of a refusal that is a list of validation errors: the kind of
    /// the first, which is the one a renderer would report.
    fn of(errors: &[ValidationError]) -> Self {
        match errors.first().map(|error| error.code) {
            Some(ErrorCode::ChildCycle | ErrorCode::MaxDepthExceeded) => Self::Recursion,
            Some(ErrorCode::UnresolvedChild) => Self::Integrity,
            _ => Self::Validation,
        }
    }
}

impl fmt::Display for RejectionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.category())
    }
}

/// A message a [`MessageProcessor`] refused. The state is unchanged.
#[derive(Debug)]
pub struct Rejection {
    /// What kind of refusal this is.
    pub kind: RejectionKind,
    /// Position of the refused message in the batch it came in; `0` for
    /// [`MessageProcessor::process`].
    pub message_index: usize,
    /// The surface the message names, when it names one.
    pub surface_id: Option<String>,
    /// The failure: [`Error::Catalog`] for [`RejectionKind::Catalog`], and
    /// [`Error::Validation`] for every other kind.
    pub error: Error,
    /// The version the refused message declares, for the reply.
    version: A2uiVersion,
}

impl Rejection {
    fn new(kind: RejectionKind, index: usize, message: &Value, error: Error) -> Self {
        Self {
            kind,
            message_index: index,
            surface_id: surface_id_of(message).map(str::to_string),
            error,
            version: message
                .get("version")
                .and_then(Value::as_str)
                .and_then(A2uiVersion::parse)
                .unwrap_or(A2uiVersion::DEFAULT),
        }
    }

    fn invalid(index: usize, message: &Value, errors: Vec<ValidationError>) -> Self {
        Self::new(
            RejectionKind::of(&errors),
            index,
            message,
            Error::Validation {
                errors: ValidationErrors(errors),
            },
        )
    }

    /// The validation errors of the refusal; none for a
    /// [`RejectionKind::Catalog`] one.
    pub fn errors(&self) -> &[ValidationError] {
        match &self.error {
            Error::Validation { errors } => errors.as_slice(),
            _ => &[],
        }
    }

    /// The `error` message a renderer that refused this message would send
    /// back to the agent.
    ///
    /// Its `code` is [`RejectionKind::renderer_code`], its `message` the first
    /// error's sentence, and its `surfaceId` the surface the refused message
    /// names, or `""`. A `VALIDATION_FAILED` error also carries `path`, the
    /// JSON Pointer to the failing field within the message's payload, such
    /// as `/components/0/text`.
    ///
    /// ```
    /// use ag_ui_a2ui::processor::MessageProcessor;
    /// use serde_json::json;
    ///
    /// let mut processor = MessageProcessor::new();
    /// let update = json!({"version": "v0.9", "updateDataModel": {
    ///     "surfaceId": "main", "path": "/count", "value": 1
    /// }});
    /// let refused = processor.process(&update).unwrap_err();
    /// assert_eq!(
    ///     serde_json::to_value(refused.renderer_error())?,
    ///     json!({"version": "v0.9", "error": {
    ///         "code": "INTEGRITY_ERROR",
    ///         "surfaceId": "main",
    ///         "message": "surface \"main\" does not exist; create it before updating it"
    ///     }})
    /// );
    /// # Ok::<(), serde_json::Error>(())
    /// ```
    pub fn renderer_error(&self) -> RendererMessage {
        let first = self.errors().first();
        let mut extensions = Map::new();
        if self.kind == RejectionKind::Validation {
            let path = first.map_or_else(|| "/".to_string(), |error| payload_pointer(&error.path));
            extensions.insert("path".to_string(), Value::String(path));
        }
        RendererMessage {
            version: self.version.as_str().to_string(),
            payload: RendererPayload::Error(RendererError {
                code: self.kind.renderer_code().to_string(),
                message: first
                    .map_or_else(|| self.error.to_string(), |error| error.message.clone()),
                surface_id: Some(self.surface_id.clone().unwrap_or_default()),
                extensions,
            }),
        }
    }
}

impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.error)
    }
}

impl std::error::Error for Rejection {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

impl From<Rejection> for Error {
    fn from(rejection: Rejection) -> Self {
        rejection.error
    }
}

/// A locator such as `messages[1].updateComponents.components[0].text` as the
/// JSON Pointer a renderer reports: into the message's payload,
/// `/components/0/text`.
fn payload_pointer(locator: &str) -> String {
    let mut rest = locator;
    if let Some(after) = rest.strip_prefix("messages[") {
        rest = after.split_once(']').map_or("", |(_, rest)| rest);
        rest = rest.strip_prefix('.').unwrap_or(rest);
        for operation in Operation::ALL {
            let inside = rest
                .strip_prefix(operation.as_str())
                .filter(|inside| inside.is_empty() || inside.starts_with(['.', '[']));
            if let Some(inside) = inside {
                rest = inside;
                break;
            }
        }
    }
    let mut pointer = String::new();
    for segment in rest.split(['.', '[']) {
        let segment = segment.trim_end_matches(']');
        if !segment.is_empty() {
            pointer.push('/');
            pointer.push_str(&segment.replace('~', "~0").replace('/', "~1"));
        }
    }
    if pointer.is_empty() {
        pointer.push('/');
    }
    pointer
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn create(surface: &str, catalog: &str) -> Value {
        json!({"version": "v0.9", "createSurface": {"surfaceId": surface, "catalogId": catalog}})
    }

    fn update(surface: &str, components: Value) -> Value {
        json!({"version": "v0.9", "updateComponents": {"surfaceId": surface, "components": components}})
    }

    fn data(surface: &str, path: &str, value: Value) -> Value {
        json!({"version": "v0.9", "updateDataModel": {"surfaceId": surface, "path": path, "value": value}})
    }

    fn delete(surface: &str) -> Value {
        json!({"version": "v0.9", "deleteSurface": {"surfaceId": surface}})
    }

    fn basic() -> (MessageProcessor, String) {
        let catalog = Catalog::basic();
        let id = catalog.catalog_id.clone();
        let mut processor = MessageProcessor::new();
        processor.register(catalog).unwrap();
        (processor, id)
    }

    #[test]
    fn a_surface_is_refused_when_it_is_not_where_the_message_needs_it() {
        let (mut processor, catalog) = basic();
        for early in [
            update(
                "s",
                json!([{"id": "root", "component": "Text", "text": "hi"}]),
            ),
            data("s", "/x", json!(1)),
        ] {
            let refused = processor.process(&early).unwrap_err();
            assert_eq!(refused.kind, RejectionKind::Integrity, "{refused}");
            assert_eq!(refused.surface_id.as_deref(), Some("s"));
            let [error] = refused.errors() else {
                panic!("one error: {:?}", refused.errors());
            };
            assert_eq!(
                (error.code, error.path.as_str()),
                (ErrorCode::InvalidValue, "messages[0]")
            );
            assert!(
                error.message.contains("does not exist"),
                "{}",
                error.message
            );
            assert!(matches!(refused.error, Error::Validation { .. }));
        }

        processor.process(&create("s", &catalog)).unwrap();
        let refused = processor.process(&create("s", &catalog)).unwrap_err();
        assert_eq!(refused.kind, RejectionKind::Integrity);
        assert!(refused.to_string().contains("already exists"), "{refused}");

        processor.process(&delete("s")).unwrap();
        let refused = processor.process(&data("s", "/x", json!(1))).unwrap_err();
        assert_eq!(refused.kind, RejectionKind::Integrity);
        assert!(refused.to_string().contains("was deleted"), "{refused}");
        // A deleted surface may be created again.
        processor.process(&create("s", &catalog)).unwrap();
        assert_eq!(processor.surfaces().lifecycle("s"), Lifecycle::Live);
    }

    #[test]
    fn deleting_a_surface_that_is_not_live_does_nothing() {
        let (mut processor, catalog) = basic();
        processor.process(&delete("never")).unwrap();
        assert_eq!(processor.surfaces().lifecycle("never"), Lifecycle::Absent);

        processor
            .process_all(&[create("s", &catalog), delete("s"), delete("s")])
            .unwrap();
        assert_eq!(processor.surfaces().lifecycle("s"), Lifecycle::Deleted);

        // The store itself still refuses it; the rule is the processor's.
        let mut store = SurfaceStore::new();
        assert!(store.apply(&AgentMessage::delete_surface("never")).is_err());

        // The envelope is still held to its contract.
        let refused = processor
            .process(&json!({"version": "v0.9", "deleteSurface": {}}))
            .unwrap_err();
        assert_eq!(refused.kind, RejectionKind::Validation);
    }

    #[test]
    fn a_surface_whose_catalog_is_not_registered_is_refused() {
        let (mut processor, _) = basic();
        let refused = processor.process(&create("s", "nope")).unwrap_err();
        assert_eq!(refused.kind, RejectionKind::Catalog);
        assert!(matches!(&refused.error, Error::Catalog(reason) if reason.contains("\"nope\"")));
        assert!(refused.errors().is_empty());
        assert_eq!(processor.surfaces().lifecycle("s"), Lifecycle::Absent);

        // State handed in may name a catalog that was never registered.
        let mut store = SurfaceStore::new();
        store
            .apply(&AgentMessage::create_surface("old", "gone"))
            .unwrap();
        let mut processor = MessageProcessor::new().with_state(store);
        let refused = processor
            .process(&update("old", json!([{"id": "root", "component": "Text"}])))
            .unwrap_err();
        assert_eq!(refused.kind, RejectionKind::Catalog);
        // Data needs no catalog.
        processor.process(&data("old", "/x", json!(1))).unwrap();
    }

    #[test]
    fn each_surface_is_validated_against_its_own_catalog() {
        let (mut processor, basic_id) = basic();
        let charts = Catalog::from_schema(&json!({
            "catalogId": "charts",
            "components": {"Chart": {"type": "object", "properties": {"title": {"type": "string"}},
                "required": ["title"]}}
        }))
        .unwrap();
        processor.register(charts.clone()).unwrap();
        assert_eq!(
            processor.catalog_ids().collect::<Vec<_>>(),
            ["charts", basic_id.as_str()]
        );
        // The same catalog again is fine; another one under its id is not.
        processor.register(charts).unwrap();
        assert!(matches!(
            processor.register(Catalog::empty("charts")),
            Err(Error::Catalog(_))
        ));

        processor
            .process_all(&[create("a", &basic_id), create("b", "charts")])
            .unwrap();
        let chart = json!([{"id": "root", "component": "Chart", "title": "Sales"}]);
        processor.process(&update("b", chart.clone())).unwrap();
        let refused = processor.process(&update("a", chart)).unwrap_err();
        assert_eq!(refused.kind, RejectionKind::Validation);
        assert_eq!(refused.errors()[0].code, ErrorCode::UnknownComponent);
        assert_eq!(
            refused.errors()[0].path,
            "messages[0].updateComponents.components[0].component"
        );
    }

    #[test]
    fn a_batch_is_all_or_nothing_and_locates_the_message_that_failed() {
        let (mut processor, catalog) = basic();
        processor.process(&create("kept", &catalog)).unwrap();
        let before = processor.surfaces().clone();

        let refused = processor
            .process_all(&[
                create("s", &catalog),
                data("kept", "/x", json!(1)),
                update("s", json!([{"id": "root", "component": "Text", "text": 5}])),
                data("kept", "/y", json!(2)),
            ])
            .unwrap_err();
        assert_eq!(refused.message_index, 2);
        assert_eq!(refused.surface_id.as_deref(), Some("s"));
        assert_eq!(refused.errors()[0].code, ErrorCode::TypeMismatch);
        assert_eq!(
            refused.errors()[0].path,
            "messages[2].updateComponents.components[0].text"
        );
        assert_eq!(
            processor.surfaces(),
            &before,
            "nothing of the batch was kept"
        );

        // `?` takes a refusal to the crate's error.
        fn run(processor: &mut MessageProcessor, message: &Value) -> Result<()> {
            Ok(processor.process(message)?)
        }
        assert!(matches!(
            run(&mut processor, &json!("not a message")),
            Err(Error::Validation { .. })
        ));
    }

    #[test]
    fn components_that_cannot_be_stored_are_located_one_by_one() {
        let (mut processor, catalog) = basic();
        processor.process(&create("s", &catalog)).unwrap();
        for (components, code, path) in [
            (
                json!([{"component": "Text", "text": "no id"}]),
                ErrorCode::MissingId,
                "messages[0].updateComponents.components[0].id",
            ),
            (
                json!([{"id": "a", "text": "no type"}]),
                ErrorCode::MissingComponentType,
                "messages[0].updateComponents.components[0].component",
            ),
            (
                json!([{"id": "a", "component": "Text", "text": "x"},
                       {"id": "a", "component": "Text", "text": "y"}]),
                ErrorCode::DuplicateId,
                "messages[0].updateComponents.components[1].id",
            ),
            (
                json!([]),
                ErrorCode::EmptyComponents,
                "messages[0].updateComponents.components",
            ),
        ] {
            let refused = processor.process(&update("s", components)).unwrap_err();
            assert_eq!(refused.kind, RejectionKind::Validation);
            assert_eq!(
                (refused.errors()[0].code, refused.errors()[0].path.as_str()),
                (code, path)
            );
        }
        assert!(processor.surfaces().get("s").unwrap().components.is_empty());
    }

    #[test]
    fn the_first_error_decides_the_kind_of_a_refusal() {
        let catalog = Catalog::basic();
        let id = catalog.catalog_id.clone();
        // Held to the whole-surface contract after every message.
        let mut strict = MessageProcessor::with_options(ValidateOptions {
            check_bindings: false,
            ..ValidateOptions::full_surface()
        });
        strict.register(catalog).unwrap();
        strict.process(&create("s", &id)).unwrap();

        let looped = update(
            "s",
            json!([{"id": "root", "component": "Card", "child": "a"},
                   {"id": "a", "component": "Card", "child": "root"}]),
        );
        let refused = strict.process(&looped).unwrap_err();
        assert_eq!(refused.kind, RejectionKind::Recursion);
        assert!(refused.kind.is("ValidationError") && refused.kind.is("RecursionError"));
        assert!(!refused.kind.is("IntegrityError"));

        let dangling = update(
            "s",
            json!([{"id": "root", "component": "Card", "child": "gone"}]),
        );
        let refused = strict.process(&dangling).unwrap_err();
        assert_eq!(refused.kind, RejectionKind::Integrity);
        assert_eq!(refused.errors()[0].code, ErrorCode::UnresolvedChild);

        let rootless = update("s", json!([{"id": "a", "component": "Text", "text": "x"}]));
        let refused = strict.process(&rootless).unwrap_err();
        assert_eq!(refused.kind, RejectionKind::Validation);
        assert!(!refused.kind.is("IntegrityError"));

        // The stream-in-flight default takes the same two as they come.
        let (mut streaming, id) = basic();
        streaming
            .process_all(&[create("s", &id), dangling, rootless])
            .unwrap();
        // A loop is refused however the processor is set up.
        assert_eq!(
            streaming.process(&looped).unwrap_err().kind,
            RejectionKind::Recursion
        );
    }

    #[test]
    fn a_report_carries_what_an_accepted_message_does_not_return() {
        let (mut processor, catalog) = basic();
        processor
            .process_all(&[
                create("s", &catalog),
                update(
                    "s",
                    json!([{"id": "root", "component": "Text", "text": "x"},
                           {"id": "stray", "component": "Text", "text": "y"}]),
                ),
            ])
            .unwrap();
        let report = processor.report("s").unwrap();
        assert!(report.is_valid());
        assert_eq!(report.unreachable, ["stray"]);
        assert!(processor.report("absent").is_none());
    }

    #[test]
    fn a_refusal_is_the_error_message_a_renderer_would_send() {
        let (mut processor, catalog) = basic();
        let reply =
            |rejection: &Rejection| serde_json::to_value(rejection.renderer_error()).unwrap();

        let refused = processor.process(&data("s", "/x", json!(1))).unwrap_err();
        assert_eq!(
            reply(&refused),
            json!({"version": "v0.9", "error": {
                "code": "INTEGRITY_ERROR", "surfaceId": "s",
                "message": "surface \"s\" does not exist; create it before updating it"
            }})
        );

        let v091 =
            json!({"version": "v0.9.1", "createSurface": {"surfaceId": "s", "catalogId": "nope"}});
        let refused = processor.process(&v091).unwrap_err();
        assert_eq!(
            reply(&refused),
            json!({"version": "v0.9.1", "error": {
                "code": "CATALOG_NOT_FOUND", "surfaceId": "s",
                "message": "invalid catalog: catalog \"nope\" is not registered"
            }})
        );

        processor.process(&create("s", &catalog)).unwrap();
        let refused = processor
            .process(&update(
                "s",
                json!([{"id": "root", "component": "Text", "text": 5}]),
            ))
            .unwrap_err();
        let sent = reply(&refused);
        assert_eq!(sent["error"]["code"], "VALIDATION_FAILED");
        assert_eq!(sent["error"]["path"], "/components/0/text");

        // No surface to name: the schema still wants the field.
        let refused = processor.process(&json!({"version": "v0.9"})).unwrap_err();
        let sent = reply(&refused);
        assert_eq!(sent["error"]["surfaceId"], "");
        assert_eq!(sent["error"]["path"], "/");

        #[cfg(feature = "schema-validation")]
        for message in [
            data("absent", "/x", json!(1)),
            create("t", "nope"),
            update("s", json!([{"id": "root", "component": "Text", "text": 5}])),
            json!({"version": "v0.9"}),
        ] {
            let refused = processor.process(&message).unwrap_err();
            let sent = serde_json::to_value(refused.renderer_error()).unwrap();
            RendererMessage::from_json(&sent)
                .unwrap_or_else(|error| panic!("{sent} is not a client message: {error}"));
        }
    }

    #[test]
    fn a_locator_becomes_a_pointer_into_the_payload() {
        for (locator, pointer) in [
            (
                "messages[1].updateComponents.components[0].text",
                "/components/0/text",
            ),
            ("messages[0].createSurface.catalogId", "/catalogId"),
            ("messages[3]", "/"),
            ("messages[0].updateComponents.components", "/components"),
            // A component carried over from before the batch.
            ("components[3].children[1]", "/components/3/children/1"),
            ("messages[0].version", "/version"),
        ] {
            assert_eq!(payload_pointer(locator), pointer, "{locator}");
        }
    }
}
