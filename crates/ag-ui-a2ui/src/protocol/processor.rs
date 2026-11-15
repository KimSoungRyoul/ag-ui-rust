//! A message processor: the surfaces of one renderer context, the catalogs
//! they may speak, and the one way a raw message gets into them.
//!
//! [`SurfaceStore`] is the state machine, and it takes typed messages as they
//! are. A [`MessageProcessor`] is what stands between the wire and a store:
//!
//! - **A catalog registry.** A surface names its catalog by id when it is
//!   created. The processor holds the catalogs this context speaks, refuses a
//!   surface whose catalog it does not hold, and validates every surface
//!   against its own. From v1.0 a surface may mix catalogs: a component names
//!   its own with `@catalogId`, the surface's is the default, and a surface
//!   may be created with none. Every catalog a surface draws from is written
//!   against the surface's wire format ([`Catalog::version`]).
//! - **Raw messages, checked in one order.** The envelope and the nesting of
//!   the JSON as it arrived, and its version when the context speaks one
//!   ([`MessageProcessor::accepting_version`]); then the surface's place in
//!   its life; then the JSON Schemas, when the catalog has them registered;
//!   then the components, against the catalog and as a graph. From v1.0 the
//!   components a message carries are checked against their catalogs before
//!   the schemas are: a catalog or a function that cannot be found is then
//!   said in those words, and not as a value that matches no schema.
//! - **Transactions.** A message changes the state or is refused and changes
//!   nothing, and so does a batch ([`MessageProcessor::process_all`]). A batch
//!   that arrives a message at a time is [`MessageProcessor::begin_batch`] and
//!   then [`MessageProcessor::process_at`] for each.
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
//! | `IntegrityError` | [`Integrity`](RejectionKind::Integrity) | [`Validation`](Error::Validation) | The surface is not in the state the message needs: created while it is live, or updated while it is not. One error, `invalid_value` at `messages[i]`. Also a child reference to a component that does not exist (`unresolved_child`), and a component id that is used twice (`duplicate_id`). |
//! | `RecursionError` | [`Recursion`](RejectionKind::Recursion) | [`Validation`](Error::Validation) | The component graph loops (`child_cycle`) or nests too deep (`max_depth_exceeded`). |
//! | `ValidationError` | [`Validation`](RejectionKind::Validation) | [`Validation`](Error::Validation) | Anything else a message gets wrong: its envelope, a component, a property, a path. Also a component where its catalog's `allowedParents` or `allowedChildren` does not let it be (`unallowed_parent`, `unallowed_child`), which a renderer reports as `UNALLOWED_PARENT` or `UNALLOWED_CHILD`. |
//! | `CatalogError` | [`Catalog`](RejectionKind::Catalog) | [`Catalog`](Error::Catalog) | The surface names a catalog that is not registered. From v1.0 also a component or a function call that does, and one with no catalog to fall back on. |
//!
//! A message that is not JSON of the expected shape is a `Validation` refusal
//! too, located at `messages[i]`: no [`Error::Json`] or [`Error::Parse`] leaves
//! a processor.
//!
//! In the reference SDKs `IntegrityError` and `RecursionError` are kinds of
//! `ValidationError`, so a suite that expects a `ValidationError` accepts
//! either. [`RejectionKind::is`] answers a category that way.
//!
//! # JSON Schema
//!
//! The checks above are the semantic validator's: what a catalog summary can
//! say about a component. With the `schema-validation` feature a catalog can
//! also be given its compiled schemas
#![cfg_attr(
    feature = "schema-validation",
    doc = "([`MessageProcessor::register_schema`]),"
)]
#![cfg_attr(
    not(feature = "schema-validation"),
    doc = "(`MessageProcessor::register_schema`),"
)]
//! and every message of a surface of that catalog is then held to them as
//! well: a property the component does not declare, a theme that breaks the
//! catalog's pattern, a function argument of the wrong type. A catalog with no
//! schema registered is checked as before.
//!
//! A schema is one catalog's. A v1.0 message in which a component or a
//! function call names another catalog than its surface's own is therefore
//! not held to the surface's schema: its catalogs, components and functions
//! are checked by name, and the properties and arguments drawn from the other
//! catalog are not checked against that catalog's schema.
//!
//! # The versions of a context
//!
//! A message is read as its own `version` spells it, and a surface is held to
//! the version it was created under: its bindings, the keys of a component
//! that are its envelope, how a removal is written. The validator asks the
//! version ([`A2uiVersion`]), so one processor takes a v0.9 surface and a v1.0
//! surface side by side. [`MessageProcessor::accepting_version`] narrows a
//! context to one wire format.
//!
//! A v1.0 `createSurface` may carry the surface's first components and data
//! itself. They are checked as an `updateComponents` right after the creation
//! would be, and located in the creation:
//! `messages[0].createSurface.components[1].text`.
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
use std::sync::Arc;

use serde_json::{Map, Value};

use super::catalog::Catalog;
use super::message::{
    AgentMessage, FunctionResponse, RendererError, RendererMessage, RendererPayload,
};
use super::operation::Operation;
use super::rpc;
#[cfg(feature = "schema-validation")]
use super::schema_validation::SchemaValidator;
use super::surface::{Lifecycle, SurfaceStore};
#[cfg(feature = "schema-validation")]
use super::validate::names_another_catalog;
use super::validate::{
    ErrorCode, ValidateOptions, ValidationError, ValidationReport, Validator, carried_components,
    check_envelope, check_value_depth, foldable, stated_version,
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
    /// Shared, so that a copy of a processor (a transaction, a state per
    /// request) does not copy its catalogs.
    catalogs: BTreeMap<String, Arc<Catalog>>,
    #[cfg(feature = "schema-validation")]
    schemas: BTreeMap<String, Arc<SchemaValidator>>,
    version: Option<A2uiVersion>,
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
            #[cfg(feature = "schema-validation")]
            schemas: BTreeMap::new(),
            version: None,
            options,
            store: SurfaceStore::new(),
        }
    }

    /// Makes this the processor of a context that speaks `version`: a message
    /// of a version that cannot share a surface with it is refused, at
    /// `messages[i].version`.
    ///
    /// Without this a message may state any version this crate speaks, and
    /// only the surface it meets holds it to one.
    #[must_use]
    pub fn accepting_version(mut self, version: A2uiVersion) -> Self {
        self.version = Some(version);
        self
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
    ///
    /// A catalog is taken as it is or already shared (`Arc<Catalog>`); a
    /// processor never copies one.
    pub fn register(&mut self, catalog: impl Into<Arc<Catalog>>) -> Result<()> {
        let catalog: Arc<Catalog> = catalog.into();
        match self.catalogs.get(&catalog.catalog_id) {
            Some(held) if **held == *catalog => Ok(()),
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

    /// Also holds every message of a surface of catalog `catalog_id` to
    /// `schema`: the protocol's JSON Schemas and the catalog's own, compiled.
    ///
    /// The check runs on the message as it arrived, after the surface's place
    /// in its life and before the components are looked at. What it refuses is
    /// a [`RejectionKind::Validation`] refusal, each failure located like any
    /// other, `messages[1].updateComponents.components[0].text`.
    ///
    /// A schema is one catalog's, so it holds a message that draws from one
    /// catalog. From v1.0 a component or a function call may name another
    /// catalog than its surface's own, and a message in which one does is not
    /// put to a schema at all: what it names is resolved against the
    /// registered catalogs (the catalog, the component type, the function),
    /// and the properties and arguments are not checked against any JSON
    /// Schema, the surface's own included.
    ///
    /// ```
    /// use std::collections::BTreeMap;
    ///
    /// use ag_ui_a2ui::Catalog;
    /// use ag_ui_a2ui::processor::MessageProcessor;
    /// use ag_ui_a2ui::schema::SchemaBundle;
    /// use ag_ui_a2ui::schema_validation::SchemaValidator;
    /// use serde_json::json;
    ///
    /// let bundle = SchemaBundle::basic()?;
    /// let catalog = Catalog::from_schema(&bundle.catalog)?;
    /// let id = catalog.catalog_id.clone();
    ///
    /// let mut processor = MessageProcessor::new();
    /// processor.register(catalog)?;
    /// processor.register_schema(&id, SchemaValidator::new(&bundle, &BTreeMap::new())?)?;
    ///
    /// processor.process(&json!({"version": "v0.9.1", "createSurface": {"surfaceId": "s", "catalogId": id}}))?;
    /// // `Text` declares no `secret`. Only a schema says so.
    /// let refused = processor
    ///     .process(&json!({"version": "v0.9.1", "updateComponents": {"surfaceId": "s", "components": [
    ///         {"id": "root", "component": "Text", "text": "Hello", "secret": true}
    ///     ]}}))
    ///     .unwrap_err();
    /// assert_eq!(refused.errors()[0].path, "messages[0].updateComponents.components[0]");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::Catalog`] when no catalog is registered as
    /// `catalog_id`: a schema checks the surfaces of a catalog the processor
    /// holds.
    #[cfg(feature = "schema-validation")]
    pub fn register_schema(
        &mut self,
        catalog_id: &str,
        schema: impl Into<Arc<SchemaValidator>>,
    ) -> Result<()> {
        if !self.catalogs.contains_key(catalog_id) {
            return Err(Error::catalog(format!(
                "catalog {catalog_id:?} is not registered; register it before its schema"
            )));
        }
        self.schemas.insert(catalog_id.to_string(), schema.into());
        Ok(())
    }

    /// A registered catalog.
    pub fn catalog(&self, catalog_id: &str) -> Option<&Catalog> {
        self.catalogs.get(catalog_id).map(Arc::as_ref)
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

    /// Starts a batch whose messages arrive one at a time, each through
    /// [`MessageProcessor::process_at`].
    ///
    /// From here on a locator names the messages of the new batch, and a
    /// component an earlier batch wrote is located by its position in its
    /// surface.
    pub fn begin_batch(&mut self) {
        self.store.begin_batch();
    }

    /// Takes message `index` of the batch [`MessageProcessor::begin_batch`]
    /// opened, or refuses it and changes nothing.
    ///
    /// This is [`MessageProcessor::process_all`] for a batch that is not all
    /// there yet, such as an answer read while it is written: the message is
    /// checked against the state the messages before it left, and a failure is
    /// located as `messages[index]`. Each message is a transaction of its own;
    /// a caller that wants the batch to be one keeps a copy of the processor.
    ///
    /// ```
    /// use ag_ui_a2ui::Catalog;
    /// use ag_ui_a2ui::processor::MessageProcessor;
    /// use serde_json::json;
    ///
    /// let catalog = Catalog::basic();
    /// let id = catalog.catalog_id.clone();
    /// let mut processor = MessageProcessor::new();
    /// processor.register(catalog)?;
    ///
    /// processor.begin_batch();
    /// processor.process_at(0, &json!({"version": "v0.9", "createSurface": {"surfaceId": "s", "catalogId": id}}))?;
    /// let refused = processor
    ///     .process_at(1, &json!({"version": "v0.9", "updateComponents": {"surfaceId": "s", "components": [
    ///         {"id": "root", "component": "Sparkline"}
    ///     ]}}))
    ///     .unwrap_err();
    /// assert_eq!(refused.message_index, 1);
    /// assert_eq!(refused.errors()[0].path, "messages[1].updateComponents.components[0].component");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns a [`Rejection`] whose [`Rejection::message_index`] is `index`.
    pub fn process_at(
        &mut self,
        index: usize,
        message: &Value,
    ) -> std::result::Result<(), Rejection> {
        self.process_typed_at(index, message).map(drop)
    }

    /// [`MessageProcessor::process_at`], handing back the message as it was
    /// decoded on the way in.
    pub(crate) fn process_typed_at(
        &mut self,
        index: usize,
        message: &Value,
    ) -> std::result::Result<AgentMessage, Rejection> {
        let mut next = self.store.clone();
        let typed = self.step(&mut next, index, message)?;
        self.store = next;
        Ok(typed)
    }

    /// The surfaces, to put aside and to put back: how a caller makes several
    /// calls one transaction.
    #[cfg(feature = "agent")]
    pub(crate) fn surfaces_mut(&mut self) -> &mut SurfaceStore {
        &mut self.store
    }

    /// Takes the catalogs, schemas, version and options of `other`, and keeps
    /// its own surfaces.
    #[cfg(feature = "agent")]
    pub(crate) fn adopt_registry(&mut self, other: &Self) {
        self.catalogs.clone_from(&other.catalogs);
        #[cfg(feature = "schema-validation")]
        self.schemas.clone_from(&other.schemas);
        self.version = other.version;
        self.options.clone_from(&other.options);
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
        if let Some(named) = surface.catalog_id.as_deref() {
            self.catalogs.get(named)?;
        }
        Some(self.validator().validate_stored(&self.store, surface))
    }

    /// A validator over the registered catalogs, with this processor's
    /// options.
    fn validator(&self) -> Validator<'_> {
        Validator::over(
            self.catalogs.values().map(Arc::as_ref),
            self.options.clone(),
        )
    }

    /// One message, against `store`, decoded. An error leaves `store` in an
    /// unknown state: the caller owns a copy and drops it.
    fn step(
        &self,
        store: &mut SurfaceStore,
        index: usize,
        message: &Value,
    ) -> std::result::Result<AgentMessage, Rejection> {
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
        let stated = stated_version(message);
        check_value_depth(message, &locator, 1, &self.options, stated, &mut report);
        if !report.errors.is_empty() {
            let mut rejection = invalid(report.errors);
            // A call that cannot be read is reported by its id, when it has
            // one: there is no call to answer.
            rejection.call = FunctionCallId::of(message).map(|id| (id, Refused::Malformed));
            return Err(rejection);
        }
        if let Some(spoken) = self.version {
            if !spoken.accepts(stated) {
                return Err(invalid(vec![ValidationError::new(
                    ErrorCode::InvalidValue,
                    format!("{locator}.version"),
                    format!(
                        "The message states version {stated}, but this context speaks {spoken}."
                    ),
                )]));
            }
        }
        let decode = |message: &Value| {
            serde_json::from_value::<AgentMessage>(message.clone())
                .map_err(|error| at_message(ErrorCode::InvalidValue, error.to_string()))
        };

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
        // A function call changes no surface: it is checked as a renderer
        // checks one before running anything, and that is all.
        match operation {
            Some(Operation::CallRendererFunction) => {
                let mut report = ValidationReport::default();
                self.validator()
                    .check_renderer_call(store, &locator, message, &mut report);
                if !report.errors.is_empty() {
                    return Err(Rejection::refused_call(index, message, report.errors));
                }
                return decode(message);
            }
            Some(Operation::AgentFunctionResponse) => return decode(message),
            _ => {}
        }
        match operation {
            Some(Operation::CreateSurface) => {
                let named = message
                    .pointer("/createSurface/catalogId")
                    .and_then(Value::as_str);
                match named {
                    Some(catalog_id) => {
                        let Some(catalog) = self.catalogs.get(catalog_id) else {
                            return Err(unregistered(index, message, catalog_id));
                        };
                        // A surface and its catalog are of one wire format.
                        if !stated.accepts(catalog.version()) {
                            return Err(Rejection::new(
                                RejectionKind::Catalog,
                                index,
                                message,
                                Error::catalog(format!(
                                    "Protocol version mismatch: cannot mix catalog \
                                     '{catalog_id}' ({}) with surface version {stated}.",
                                    catalog.version()
                                )),
                            ));
                        }
                    }
                    // Before v1.0 a surface names its one catalog.
                    None if !stated.mixes_catalogs() => {
                        return Err(unregistered(index, message, ""));
                    }
                    // From v1.0 it may name none, and its components theirs.
                    None => {}
                }
                if lifecycle == Lifecycle::Live {
                    return Err(out_of_place(format!(
                        "surface {surface_id:?} already exists"
                    )));
                }
            }
            // Nothing to delete is nothing to do.
            Some(Operation::DeleteSurface) if lifecycle != Lifecycle::Live => {
                return decode(message);
            }
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

        // The components the message carries: an update's, or the ones a v1.0
        // creation brings with it.
        let components = carried_components(message).map(|(_, components)| components);
        let validator = self.validator();
        // Where a version resolves what a component names, that comes first:
        // a catalog or a function that is not there is a clearer thing to be
        // told than a value no schema matches.
        let read_as = store
            .get(surface_id)
            .filter(|surface| !surface.deleted)
            .map_or(stated, |surface| surface.spoken_version());
        if let Some(components) = components.filter(|_| read_as.resolves_function_calls()) {
            self.default_is_held(store, index, message, surface_id)?;
            let report = validator.validate_unfolded(store, index, message, components);
            if !report.errors.is_empty() {
                return Err(invalid(report.errors));
            }
        }

        // The message as its schemas see it, when its catalog has them and
        // everything in the message is drawn from that catalog.
        #[cfg(feature = "schema-validation")]
        {
            // The surface's own catalog, as the message or the store names it.
            let own_catalog = match operation {
                Some(Operation::CreateSurface) => message
                    .pointer("/createSurface/catalogId")
                    .and_then(Value::as_str),
                _ => store
                    .get(surface_id)
                    .and_then(|surface| surface.catalog_id.as_deref()),
            };
            let schema = own_catalog
                .filter(|_| !names_another_catalog(read_as, message, own_catalog))
                .and_then(|catalog_id| self.schemas.get(catalog_id));
            if let Some(Err(error)) = schema.map(|schema| schema.validate_message(message)) {
                return Err(match error {
                    Error::Validation { errors } => invalid(
                        errors
                            .0
                            .into_iter()
                            .map(|mut error| {
                                error.path = located(&locator, &error.path);
                                error
                            })
                            .collect(),
                    ),
                    other => at_message(ErrorCode::InvalidValue, other.to_string()),
                });
            }
        }

        // Components the store cannot key by id are located one by one; serde
        // would only say that the message did not decode.
        if let Some(components) = components.filter(|components| !foldable(components)) {
            self.default_is_held(store, index, message, surface_id)?;
            let report = validator.validate_unfolded(store, index, message, components);
            return Err(if report.errors.is_empty() {
                at_message(
                    ErrorCode::InvalidValue,
                    "the components of this message cannot be stored".to_string(),
                )
            } else {
                invalid(report.errors)
            });
        }

        let typed = decode(message)?;
        store
            .apply_at(index, &typed)
            .map_err(|error| at_message(ErrorCode::InvalidValue, error.to_string()))?;

        // The surface the message changed, as a whole.
        let changed = match operation {
            Some(Operation::UpdateComponents | Operation::CreateSurface) => components.is_some(),
            Some(Operation::UpdateDataModel) => self.options.check_bindings,
            _ => false,
        };
        if changed {
            let surface = store
                .get(surface_id)
                .filter(|surface| !surface.deleted && !surface.components.is_empty());
            if let Some(surface) = surface {
                self.default_is_held(store, index, message, surface_id)?;
                let report = validator.validate_stored(store, surface);
                if !report.errors.is_empty() {
                    return Err(invalid(report.errors));
                }
            }
        }
        Ok(typed)
    }

    /// Refuses a message for a surface whose own catalog is not registered.
    /// A surface that names none has none to hold.
    fn default_is_held(
        &self,
        store: &SurfaceStore,
        index: usize,
        message: &Value,
        surface_id: &str,
    ) -> std::result::Result<(), Rejection> {
        match store
            .get(surface_id)
            .and_then(|surface| surface.catalog_id.as_deref())
        {
            Some(catalog_id) if !self.catalogs.contains_key(catalog_id) => {
                Err(unregistered(index, message, catalog_id))
            }
            _ => Ok(()),
        }
    }
}

/// A JSON Pointer into a message, as the locator the other checks write:
/// `/updateComponents/components/0/text` under `messages[1]` is
/// `messages[1].updateComponents.components[0].text`.
#[cfg(feature = "schema-validation")]
fn located(message: &str, pointer: &str) -> String {
    let mut locator = message.to_string();
    for segment in pointer.split('/').skip(1) {
        if !segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_digit()) {
            locator.push('[');
            locator.push_str(segment);
            locator.push(']');
        } else {
            locator.push('.');
            locator.push_str(&segment.replace("~1", "/").replace("~0", "~"));
        }
    }
    locator
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
    /// The surface, or one of its components, names a catalog that is not
    /// registered, or one written against another wire format than the
    /// surface.
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
            Some(ErrorCode::UnresolvedChild | ErrorCode::DuplicateId) => Self::Integrity,
            Some(ErrorCode::UnknownCatalog) => Self::Catalog,
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
    /// What a [`RejectionKind::Catalog`] refusal was found as, when a
    /// component or a function call named the catalog: the errors, each where
    /// the message wrote the name. Empty for any other refusal.
    named: Vec<ValidationError>,
    /// The `functionCallId` of a refused `callRendererFunction`, and whether
    /// the call could be read.
    call: Option<(String, Refused)>,
}

/// How far a refused `callRendererFunction` got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refused {
    /// The message is not a call a renderer can read.
    Malformed,
    /// The call reads, and is one the renderer does not run.
    NotRun,
}

/// The `functionCallId` of a raw `callRendererFunction`.
struct FunctionCallId;

impl FunctionCallId {
    fn of(message: &Value) -> Option<String> {
        message
            .get(Operation::CallRendererFunction.as_str())?
            .get("functionCallId")?
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_string)
    }
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
            named: Vec::new(),
            call: None,
        }
    }

    /// A `callRendererFunction` that reads and that a renderer does not run.
    fn refused_call(index: usize, message: &Value, errors: Vec<ValidationError>) -> Self {
        let mut rejection = Self::new(
            RejectionKind::Validation,
            index,
            message,
            Error::Validation {
                errors: ValidationErrors(errors),
            },
        );
        rejection.call = FunctionCallId::of(message).map(|id| (id, Refused::NotRun));
        rejection
    }

    fn invalid(index: usize, message: &Value, errors: Vec<ValidationError>) -> Self {
        let kind = RejectionKind::of(&errors);
        match (kind, errors.first()) {
            // A catalog a component names is refused as one the surface names.
            (RejectionKind::Catalog, Some(first)) => {
                let error = Error::catalog(first.message.clone());
                let mut rejection = Self::new(kind, index, message, error);
                rejection.named = errors;
                rejection
            }
            _ => Self::new(
                kind,
                index,
                message,
                Error::Validation {
                    errors: ValidationErrors(errors),
                },
            ),
        }
    }

    /// The errors behind a refusal for a catalog that a component or a
    /// function call named, each located where the message names it. Empty
    /// when the surface named the catalog, and for every other kind.
    ///
    /// A renderer reports such a refusal as a catalog it does not have, and
    /// [`Rejection::errors`] is empty for it. A caller that wrote the message
    /// reads here which name to change: each error is an
    /// [`ErrorCode::UnknownCatalog`](crate::validate::ErrorCode::UnknownCatalog)
    /// located where the message wrote the catalog, such as
    /// `messages[0].updateComponents.components[2].@catalogId`.
    ///
    /// ```
    /// use ag_ui_a2ui::{A2uiVersion, Catalog, MessageProcessor};
    /// use ag_ui_a2ui::processor::RejectionKind;
    /// use serde_json::json;
    ///
    /// let mut processor = MessageProcessor::new();
    /// processor.register(Catalog::basic_for(A2uiVersion::V1_0))?;
    /// let refused = processor
    ///     .process(&json!({"version": "1.0", "createSurface": {
    ///         "surfaceId": "s",
    ///         "catalogId": A2uiVersion::V1_0.basic_catalog_id(),
    ///         "components": [
    ///             {"id": "root", "component": "Chart", "@catalogId": "example.com:charts"}
    ///         ]
    ///     }}))
    ///     .unwrap_err();
    /// assert_eq!(refused.kind, RejectionKind::Catalog);
    /// assert!(refused.errors().is_empty());
    /// let [named] = refused.named_catalogs() else { panic!("one catalog was named") };
    /// assert!(named.path.ends_with("components[0].@catalogId"), "{}", named.path);
    /// # Ok::<(), ag_ui_a2ui::Error>(())
    /// ```
    pub fn named_catalogs(&self) -> &[ValidationError] {
        &self.named
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
        // A refused function call is reported by the id of the call, which
        // takes the place of a surface.
        if let Some((function_call_id, _)) = &self.call {
            extensions.insert(
                "functionCallId".to_string(),
                Value::String(function_call_id.clone()),
            );
            return RendererMessage {
                version: self.version.as_str().to_string(),
                payload: RendererPayload::Error(RendererError {
                    code: rpc::code::INVALID_FUNCTION_CALL.to_string(),
                    message: first
                        .map_or_else(|| self.error.to_string(), |error| error.message.clone()),
                    surface_id: None,
                    extensions,
                }),
            };
        }
        if self.kind == RejectionKind::Validation {
            let path = first.map_or_else(|| "/".to_string(), |error| payload_pointer(&error.path));
            extensions.insert("path".to_string(), Value::String(path));
        }
        // A composition failure has a code of its own.
        let code = match first.map(|error| error.code) {
            Some(ErrorCode::UnallowedParent) => "UNALLOWED_PARENT",
            Some(ErrorCode::UnallowedChild) => "UNALLOWED_CHILD",
            _ => self.kind.renderer_code(),
        };
        RendererMessage {
            version: self.version.as_str().to_string(),
            payload: RendererPayload::Error(RendererError {
                code: code.to_string(),
                message: first
                    .map_or_else(|| self.error.to_string(), |error| error.message.clone()),
                surface_id: Some(self.surface_id.clone().unwrap_or_default()),
                extensions,
            }),
        }
    }
}

impl Rejection {
    /// The `rendererFunctionResponse` a renderer sends for a
    /// `callRendererFunction` it reads and does not run: the call's
    /// `functionCallId`, and an `INVALID_FUNCTION_CALL` error with the first
    /// failure's sentence. A renderer answers every call it can read, so this
    /// is what it sends in place of [`Rejection::renderer_error`].
    ///
    /// `None` for any other refusal, and for a call that could not be read,
    /// which has no call to answer: [`Rejection::renderer_error`] reports it
    /// by its `functionCallId`.
    ///
    /// ```
    /// use ag_ui_a2ui::{A2uiVersion, Catalog, MessageProcessor};
    /// use serde_json::json;
    ///
    /// let mut processor = MessageProcessor::new();
    /// processor.register(Catalog::basic_for(A2uiVersion::V1_0))?;
    /// let refused = processor
    ///     .process(&json!({"version": "1.0", "callRendererFunction": {
    ///         "functionCallId": "call-1",
    ///         "callFunction": {
    ///             "@call": "formatString",
    ///             "catalogId": A2uiVersion::V1_0.basic_catalog_id(),
    ///             "args": {"value": "Saved"}
    ///         }
    ///     }}))
    ///     .unwrap_err();
    /// assert_eq!(
    ///     serde_json::to_value(refused.renderer_response().unwrap())?,
    ///     json!({"version": "1.0", "rendererFunctionResponse": {
    ///         "functionCallId": "call-1",
    ///         "error": {
    ///             "code": "INVALID_FUNCTION_CALL",
    ///             "message": "Function 'formatString' cannot be called by agent \
    ///                         (allowedCallers is rendererOnly)."
    ///         }
    ///     }})
    /// );
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn renderer_response(&self) -> Option<RendererMessage> {
        let (function_call_id, Refused::NotRun) = self.call.as_ref()? else {
            return None;
        };
        let message = self
            .errors()
            .first()
            .map_or_else(|| self.error.to_string(), |error| error.message.clone());
        Some(RendererMessage {
            version: self.version.as_str().to_string(),
            payload: RendererPayload::RendererFunctionResponse(FunctionResponse::error(
                function_call_id.clone(),
                rpc::RpcError::invalid_call(message),
            )),
        })
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
            // An id used twice is a fault of the surface's integrity, as the
            // reference processors file it; the rest are the message's own.
            let kind = match code {
                ErrorCode::DuplicateId => RejectionKind::Integrity,
                _ => RejectionKind::Validation,
            };
            assert_eq!(refused.kind, kind, "{code}");
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
