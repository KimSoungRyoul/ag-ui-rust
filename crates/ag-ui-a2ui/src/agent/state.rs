//! The agent's mirror of the surfaces a renderer holds.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::Value;

use super::catalog::{CatalogDocument, check_catalogs};
use crate::error::{Error, Result};
use crate::protocol::message::AgentMessage;
use crate::protocol::operation::Operation;
use crate::protocol::processor::{MessageProcessor, Rejection, RejectionKind};
use crate::protocol::surface::{SurfaceState, SurfaceStore};
use crate::protocol::validate::{
    ErrorCode, ValidateOptions, ValidationError, ValidationReport, Validator,
};
use crate::protocol::version::A2uiVersion;

/// The message processor of a session that holds `catalogs` and speaks
/// `version`, with no surfaces yet.
///
/// A surface is validated while its tree may still be arriving, which is the
/// processor's default; the whole tree is [`RendererState::check_trees`]'s.
pub(crate) fn session_processor(
    catalogs: &[CatalogDocument],
    version: A2uiVersion,
) -> Result<MessageProcessor> {
    let mut processor = MessageProcessor::new().accepting_version(version);
    for catalog in catalogs {
        processor.register(catalog.shared_catalog())?;
    }
    Ok(processor)
}

/// The surfaces a renderer holds, as the agent's messages leave them.
///
/// A state is the protocol's own [`MessageProcessor`] with the session's
/// catalogs registered, so an agent's outbound message is taken in the way a
/// renderer takes it, and is checked against everything the renderer has
/// already received: an update must name a surface that exists, a component is
/// held to the catalog its surface was created with, or from v1.0 to the one
/// it names itself, and a cycle is found across the whole surface rather than
/// within one message. The state adds
/// what only an agent needs: which catalogs are active and in what order, the
/// check of a finished tree, and a transaction around both.
///
/// How long a state lives is the one difference between the two ways an agent
/// shows UI:
///
/// - In a **chat**, each turn shows new UI, so each turn uses a new state.
/// - In a **canvas**, turns update the same surfaces, so one state lives for
///   the whole conversation and every turn is read against it.
///
/// Cloning copies the surfaces; the catalogs are shared. Two states are equal
/// when they hold the same catalogs and the same surfaces.
///
/// # Two checks
///
/// A message is checked when it is applied ([`RendererState::apply`]): its
/// envelope, its surface's life, and each component against the catalog. What
/// can only be judged once every message of an answer has arrived is left to
/// [`RendererState::check_trees`]: that each surface has its root, and that
/// every child reference and binding resolves. A component may name a child
/// that a later message adds, so asking earlier would reject a tree for being
/// unfinished.
///
/// # Every rejection is the payload's
///
/// A state is given catalogs once, and messages ever after, so whatever it
/// rejects, a message got wrong. Both checks return
/// [`Error::Validation`](crate::Error::Validation), each failure with a locator
/// into the messages, and nothing else:
///
/// - A fault of a surface's life (created while it is live, updated while it
///   is not) is one `invalid_value` at `messages[i]`. So is a message that does
///   not decode.
/// - A component or a function call of a v1.0 surface that names a catalog
///   the state does not hold, or names none on a surface that has none of its
///   own, is an `unknown_catalog` where the message wrote it, such as
///   `messages[i].updateComponents.components[0].@catalogId`.
/// - A `createSurface` that names a catalog the state does not hold is one
///   `invalid_value` at `messages[i].createSurface.catalogId`. A renderer
///   calls that a catalog error, since it was handed a catalog it does not
///   have, and so does the processor underneath
///   ([`RejectionKind::Catalog`](crate::processor::RejectionKind::Catalog)).
///   Here the name came from the payload, and the state turns the refusal
///   into the payload's. The reference SDK's agent
///   layer makes the same turn above its core processor (`RendererState` wraps
///   it in `_as_validation_error`: "errors of other kinds, such as a catalog
///   that a message names but the processor doesn't hold, come from the
///   payload too"), and `agent/request_processor.yaml` of the conformance
///   suite expects it.
///
/// [`Error::Catalog`](crate::Error::Catalog) is kept for what no payload can
/// fix: [`RendererState::new`] with no catalogs or catalogs of two protocol
/// families.
///
/// ```
/// use ag_ui_a2ui::A2uiVersion;
/// use ag_ui_a2ui::agent::{CatalogDocument, RendererState};
/// use serde_json::json;
///
/// let catalog = CatalogDocument::basic(A2uiVersion::V0_9)?;
/// let id = catalog.catalog_id().to_string();
/// let mut state = RendererState::new(vec![catalog])?;
///
/// let touched = state.apply(&[
///     json!({"version": "v0.9", "createSurface": {"surfaceId": "main", "catalogId": id}}),
///     json!({"version": "v0.9", "updateComponents": {"surfaceId": "main", "components": [
///         {"id": "root", "component": "Text", "text": "Hello"},
///     ]}}),
/// ])?;
/// state.check_trees(&touched)?;
/// assert_eq!(state.surface("main").unwrap().components.len(), 1);
///
/// // A rejected call changes nothing.
/// let before = state.clone();
/// let unknown = json!({"version": "v0.9", "updateComponents": {"surfaceId": "main", "components": [
///     {"id": "extra", "component": "Sparkline"},
/// ]}});
/// assert!(state.apply(&[unknown]).is_err());
/// assert_eq!(state, before);
/// # Ok::<(), ag_ui_a2ui::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct RendererState {
    /// The active catalogs, in priority order.
    catalogs: Arc<[CatalogDocument]>,
    /// The version the catalogs are written against. Every message has to be
    /// of a version that may meet it on one surface.
    version: A2uiVersion,
    /// The surfaces, and the one way a message gets into them. The catalogs
    /// are registered in it, with their schemas when the session checks them.
    processor: MessageProcessor,
}

impl PartialEq for RendererState {
    fn eq(&self, other: &Self) -> bool {
        self.processor.surfaces() == other.processor.surfaces() && self.catalogs == other.catalogs
    }
}

/// What applying one message did.
#[derive(Debug, Clone)]
pub(crate) struct Applied {
    /// The message, decoded.
    pub(crate) message: AgentMessage,
    /// The surface it creates, updates or deletes.
    pub(crate) surface_id: String,
}

impl RendererState {
    /// The empty state of a renderer that holds `catalogs`.
    ///
    /// Each surface is checked against the catalog its `createSurface` names.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Catalog`] when no catalog is given, or when the
    /// catalogs target versions whose messages cannot meet on one surface.
    pub fn new(catalogs: Vec<CatalogDocument>) -> Result<Self> {
        let version = check_catalogs(&catalogs)?;
        let processor = session_processor(&catalogs, version)?;
        Ok(Self::from_parts(catalogs.into(), version, &processor))
    }

    /// A state for catalogs that were already checked to share `version`,
    /// taking messages in the way `processor` does. `processor` holds no
    /// surfaces.
    pub(crate) fn from_parts(
        catalogs: Arc<[CatalogDocument]>,
        version: A2uiVersion,
        processor: &MessageProcessor,
    ) -> Self {
        Self {
            catalogs,
            version,
            processor: processor.clone(),
        }
    }

    /// Makes the state take messages in the way `processor` does: a request
    /// processor's own, which registers the catalogs the state holds and may
    /// check schemas the state did not. The surfaces stay.
    pub(crate) fn bind(&mut self, processor: &MessageProcessor) {
        self.processor.adopt_registry(processor);
    }

    /// The active catalogs, in priority order.
    pub fn catalogs(&self) -> impl Iterator<Item = &CatalogDocument> {
        self.catalogs.iter()
    }

    /// A surface the renderer holds: created, and not deleted since.
    pub fn surface(&self, surface_id: &str) -> Option<&SurfaceState> {
        self.store()
            .get(surface_id)
            .filter(|surface| !surface.deleted)
    }

    /// Every surface the renderer holds.
    pub fn surfaces(&self) -> impl Iterator<Item = &SurfaceState> {
        self.store().surfaces().filter(|surface| !surface.deleted)
    }

    /// The surface state machine underneath, including the tombstones of
    /// deleted surfaces.
    pub fn store(&self) -> &SurfaceStore {
        self.processor.surfaces()
    }

    /// The message processor underneath: the catalogs it registers, and
    /// [`MessageProcessor::report`] for the standing of one surface.
    pub fn processor(&self) -> &MessageProcessor {
        &self.processor
    }

    /// Checks the messages against the state and applies them.
    ///
    /// Each message must be a well-formed envelope of a version the catalogs
    /// accept. A `createSurface` must name an active catalog, and an update
    /// must name a surface the renderer holds. Each component is checked
    /// against its surface's catalog, and component ids and cycles against the
    /// surface's components. Roots, child references and bindings are left to
    /// [`RendererState::check_trees`].
    ///
    /// A `deleteSurface` that names a surface the renderer does not hold is
    /// accepted and changes nothing. That is the processor's rule, and the
    /// reference renderers' (the suite states it in
    /// `core/message_processor_v1_0.yaml::test_delete_non_existent_surface_is_safe_noop`):
    /// it is what a renderer that holds nothing by that name can only do. The
    /// surface state machine itself, [`SurfaceStore`], rejects such a delete.
    ///
    /// The call is atomic: if a message is rejected, none of them is applied.
    /// A failure is located in the message that has to change,
    /// `messages[1].updateComponents.components[0]`, counting from the first
    /// message of this call.
    ///
    /// Returns the ids of the surfaces the messages create, update or delete,
    /// in the order the messages first name them.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Validation`] with every failure found in the first
    /// message a renderer in this state would reject. A fault of a surface's
    /// life (created while it is live, updated while it is not) is one
    /// `invalid_value` at `messages[i]`, and so is a message that does not
    /// decode. A `createSurface` that names a catalog that is not active is
    /// an `invalid_value` at its `catalogId`: the name came from the payload,
    /// so it is the payload's fault, not a fault of the catalogs. No other
    /// variant leaves this call.
    pub fn apply(&mut self, messages: &[Value]) -> Result<Vec<String>> {
        self.transaction(|state| {
            state.begin_answer();
            let mut touched: Vec<String> = Vec::new();
            for (index, message) in messages.iter().enumerate() {
                let applied = state.apply_at(index, message)?;
                if !touched.contains(&applied.surface_id) {
                    touched.push(applied.surface_id);
                }
            }
            Ok(touched)
        })
    }

    /// Checks the whole tree of each surface named.
    ///
    /// The surface must have its root, and every child reference and binding
    /// must resolve. A surface with no components yet passes, since they may
    /// arrive later, and so does a surface the renderer no longer holds.
    ///
    /// Each component is read from the catalog it names, or from its
    /// surface's own, among the active catalogs: a v1.0 surface may draw from
    /// several.
    ///
    /// Composition constraints (`allowedParents`, `allowedChildren`) are
    /// checked here too, since they are about a component's place in the tree,
    /// and for a surface of any version: a catalog that states them means
    /// them. A failure is an `unallowed_parent` or an `unallowed_child`, in
    /// the words a [`Validator`] has for a v1.0 surface.
    ///
    /// A component the root cannot reach is not an error here, as it is not in
    /// [`Validator`]: the protocol has no message that removes one component,
    /// so replacing part of a canvas always leaves such components behind.
    /// Its bindings are not resolved either, which is the validator's rule
    /// too ([`ValidationReport::unreachable`]). A row of a list that was
    /// replaced still has its relative paths, and no list above it any more to
    /// give them a scope; nothing draws it, so nothing resolves them.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Validation`] with every failure, across all the
    /// surfaces named. A component is located in the message that wrote it
    /// when that message was applied since the answer began, and by its
    /// position in the surface otherwise.
    pub fn check_trees<I, S>(&self, surface_ids: I) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut checked = BTreeSet::new();
        let mut report = ValidationReport::default();
        // One validator over the active catalogs, for every surface checked.
        let validator = Validator::over(
            self.catalogs.iter().map(CatalogDocument::catalog),
            ValidateOptions::full_surface(),
        );
        for surface_id in surface_ids {
            let surface_id = surface_id.as_ref();
            if !checked.insert(surface_id.to_string()) {
                continue;
            }
            let Some(surface) = self.surface(surface_id) else {
                continue;
            };
            if surface.components.is_empty() {
                continue;
            }
            // The surface's own catalog has to be one of the session's. A
            // surface that names none draws each component from the catalog
            // the component names.
            if let Some(catalog_id) = surface.catalog_id.as_deref() {
                if self.processor.catalog(catalog_id).is_none() {
                    return Err(inactive_catalog(
                        format!("surfaces[{surface_id:?}]"),
                        catalog_id,
                        &self.catalogs,
                    ));
                }
            }
            let found = validator.validate_stored(self.store(), surface);
            // A version that constrains composition has it in the report
            // already, once nothing else is wrong; so is it added here.
            if found.errors.is_empty() && !surface.spoken_version().constrains_composition() {
                report
                    .errors
                    .extend(validator.composition_stored(self.store(), surface));
            }
            report.errors.extend(found.errors);
        }
        report.into_result()
    }

    /// Runs `turn`, and puts the state back as it was if `turn` fails.
    ///
    /// This is how an answer read whole is applied all or nothing: apply its
    /// messages and check its trees inside one transaction, and a rejected
    /// answer leaves no trace.
    ///
    /// ```
    /// # use ag_ui_a2ui::A2uiVersion;
    /// # use ag_ui_a2ui::agent::{CatalogDocument, RendererState};
    /// # use serde_json::json;
    /// # let catalog = CatalogDocument::basic(A2uiVersion::V0_9)?;
    /// # let id = catalog.catalog_id().to_string();
    /// let mut state = RendererState::new(vec![catalog])?;
    /// let before = state.clone();
    ///
    /// // The messages apply one by one, but the tree they leave has no root.
    /// let rejected = state.transaction(|state| {
    ///     let touched = state.apply(&[
    ///         json!({"version": "v0.9", "createSurface": {"surfaceId": "s", "catalogId": id}}),
    ///         json!({"version": "v0.9", "updateComponents": {"surfaceId": "s", "components": [
    ///             {"id": "title", "component": "Text", "text": "No root"},
    ///         ]}}),
    ///     ])?;
    ///     state.check_trees(&touched)
    /// });
    /// assert!(rejected.is_err());
    /// assert_eq!(state, before);
    /// # Ok::<(), ag_ui_a2ui::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns what `turn` returns.
    pub fn transaction<T, E>(
        &mut self,
        turn: impl FnOnce(&mut Self) -> std::result::Result<T, E>,
    ) -> std::result::Result<T, E> {
        let before = self.store().clone();
        let outcome = turn(self);
        if outcome.is_err() {
            *self.processor.surfaces_mut() = before;
        }
        outcome
    }

    /// Starts numbering messages from zero: the next one applied is
    /// `messages[0]` in a locator, and components already held count as prior
    /// state.
    pub(crate) fn begin_answer(&mut self) {
        self.processor.begin_batch();
    }

    /// Applies one message under the active catalogs, as `messages[index]`.
    ///
    /// This is the one place a message meets the state, and it is the
    /// processor's: the checks, their order and their wording are the ones a
    /// renderer written on this crate applies. All or nothing: a rejected
    /// message changes nothing.
    pub(crate) fn apply_at(&mut self, index: usize, message: &Value) -> Result<Applied> {
        match self.processor.process_typed_at(index, message) {
            Ok(message) => Ok(Applied {
                surface_id: message.surface_id().unwrap_or_default().to_string(),
                message,
            }),
            Err(rejection) => Err(self.payloads_fault(index, message, rejection)),
        }
    }

    /// A refusal, as the agent reads it: whatever a message was refused for,
    /// the payload got it wrong.
    ///
    /// The one refusal a renderer does not blame on the message is a catalog
    /// it does not hold. Here the catalog's name came from the payload; see
    /// [`inactive_catalog`]. From v1.0 a component and a function call name
    /// catalogs too, and such a refusal is the errors the processor found,
    /// each where the payload wrote the name.
    fn payloads_fault(&self, index: usize, message: &Value, rejection: Rejection) -> Error {
        match rejection.kind {
            RejectionKind::Catalog if !rejection.named_catalogs().is_empty() => Error::Validation {
                errors: rejection.named_catalogs().to_vec().into(),
            },
            RejectionKind::Catalog => {
                let named = message
                    .pointer("/createSurface/catalogId")
                    .and_then(Value::as_str);
                let (place, catalog_id) = match named {
                    Some(catalog_id) => (
                        format!("messages[{index}].createSurface.catalogId"),
                        catalog_id.to_string(),
                    ),
                    // A surface this state was handed with a catalog that is
                    // not the session's.
                    None => (
                        format!("messages[{index}]"),
                        rejection
                            .surface_id
                            .as_deref()
                            .and_then(|surface_id| self.store().get(surface_id))
                            .and_then(|surface| surface.catalog_id.clone())
                            .unwrap_or_default(),
                    ),
                };
                inactive_catalog(place, &catalog_id, &self.catalogs)
            }
            _ => rejection.into(),
        }
    }

    /// Creates a surface the caller assumes the renderer already holds,
    /// without checking anything and without numbering a message.
    pub(crate) fn assume_surface(
        &mut self,
        surface_id: &str,
        catalog_id: &str,
        version: A2uiVersion,
    ) -> Result<()> {
        self.processor
            .surfaces_mut()
            .apply(&AgentMessage::create_surface(surface_id, catalog_id).with_version(version))
    }
}

/// A surface names a catalog the session does not hold.
///
/// For a renderer that is a catalog error: it was handed a catalog it does
/// not have. Here the name came from the payload, so it is one more thing the
/// payload got wrong: a validation error, located where the name is written.
/// The reference SDK's agent layer makes the same turn (`_as_validation_error`
/// in its `RendererState`: "errors of other kinds, such as a catalog that a
/// message names but the processor doesn't hold, come from the payload too"),
/// and `agent/request_processor.yaml` expects it.
fn inactive_catalog(place: String, catalog_id: &str, active: &[CatalogDocument]) -> Error {
    let ids: Vec<&str> = active.iter().map(CatalogDocument::catalog_id).collect();
    Error::Validation {
        errors: vec![ValidationError::new(
            ErrorCode::InvalidValue,
            place,
            format!("Catalog {catalog_id:?} is not one of the active catalogs: {ids:?}."),
        )]
        .into(),
    }
}

/// Checks one payload on its own, the way a renderer holding `catalogs` would,
/// with no record of what earlier payloads sent.
///
/// A surface the payload creates is checked in full: its components must be
/// valid, and their references must resolve once every message of the payload
/// is applied. A surface the payload only updates is assumed to exist already,
/// holding components this check never saw, so references to components
/// outside the payload and a missing root are accepted there. Each of its
/// components is still checked against a catalog: the first active one that
/// defines every component type the payload adds to that surface.
///
/// A surface an earlier message of the payload deleted is not assumed again,
/// and neither is one a later message creates: a renderer rejects an update to
/// either, since the surface does not exist when the update arrives.
///
/// To check a payload against what earlier payloads sent, keep a
/// [`RendererState`] and call [`RendererState::apply`] and
/// [`RendererState::check_trees`] inside [`RendererState::transaction`].
///
/// # Errors
///
/// Returns [`Error::Catalog`] when `catalogs` is empty or mixes protocol
/// families, and [`Error::Validation`] when a renderer holding them would
/// reject the payload.
pub fn validate_payload(catalogs: &[CatalogDocument], messages: &[Value]) -> Result<()> {
    let mut state = RendererState::new(catalogs.to_vec())?;
    // The surface a message names, and whether the message creates it.
    let target = |message: &Value| -> Option<(String, Operation)> {
        let operation = Operation::of(message)?;
        let surface_id = message[operation.as_str()]["surfaceId"].as_str()?;
        Some((surface_id.to_string(), operation))
    };

    let mut named: BTreeSet<String> = BTreeSet::new();
    let mut assumed: BTreeSet<String> = BTreeSet::new();
    let mut touched: Vec<String> = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        if let Some((surface_id, operation)) = target(message) {
            let first_mention = named.insert(surface_id.clone());
            // Updates to this surface from here, up to a message that creates
            // or deletes it.
            let mut updates: Vec<&Value> = Vec::new();
            let mut created_later = false;
            for later in &messages[index..] {
                match target(later) {
                    Some((id, Operation::CreateSurface)) if id == surface_id => {
                        created_later = true;
                        break;
                    }
                    Some((id, Operation::DeleteSurface)) if id == surface_id => break,
                    Some((id, _)) if id == surface_id => updates.push(later),
                    _ => {}
                }
            }
            if first_mention && operation != Operation::CreateSurface && !created_later {
                let version = message["version"]
                    .as_str()
                    .and_then(A2uiVersion::parse)
                    .unwrap_or(state.version);
                let types: BTreeSet<&str> = updates
                    .iter()
                    .filter_map(|update| update["updateComponents"]["components"].as_array())
                    .flatten()
                    .filter_map(|component| component["component"].as_str())
                    .collect();
                let catalog_id = state
                    .catalogs()
                    .find(|catalog| {
                        types
                            .iter()
                            .all(|name| catalog.catalog().has_component(name))
                    })
                    .or_else(|| state.catalogs().next())
                    .map(|catalog| catalog.catalog_id().to_string())
                    .unwrap_or_default();
                state.assume_surface(&surface_id, &catalog_id, version)?;
                assumed.insert(surface_id);
            }
        }
        let applied = state.apply_at(index, message)?;
        if !touched.contains(&applied.surface_id) {
            touched.push(applied.surface_id);
        }
    }
    state.check_trees(touched.iter().filter(|id| !assumed.contains(*id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn catalog(id: &str, component: &str) -> CatalogDocument {
        CatalogDocument::new(json!({
            "catalogId": id,
            "protocolVersion": "0.9",
            "components": {
                component: {
                    "properties": {"text": {"type": "string"}},
                    "required": ["text"],
                },
                "Box": {"properties": {
                    "children": {"$ref": "common_types.json#/$defs/ChildList"},
                }},
            },
        }))
        .unwrap()
    }

    fn state() -> RendererState {
        RendererState::new(vec![catalog("one", "Text"), catalog("two", "Label")]).unwrap()
    }

    fn create(surface: &str, catalog: &str) -> Value {
        json!({"version": "v0.9", "createSurface": {"surfaceId": surface, "catalogId": catalog}})
    }

    fn update(surface: &str, components: Value) -> Value {
        json!({"version": "v0.9", "updateComponents": {"surfaceId": surface, "components": components}})
    }

    fn delete(surface: &str) -> Value {
        json!({"version": "v0.9", "deleteSurface": {"surfaceId": surface}})
    }

    fn located(error: Error) -> Vec<(ErrorCode, String)> {
        match error {
            Error::Validation { errors } => errors
                .0
                .into_iter()
                .map(|error| (error.code, error.path))
                .collect(),
            other => panic!("expected a validation error, got {other:?}"),
        }
    }

    #[test]
    fn each_surface_is_checked_against_the_catalog_it_was_created_with() {
        let mut state = state();
        state
            .apply(&[
                create("a", "one"),
                create("b", "two"),
                update(
                    "a",
                    json!([{"id": "root", "component": "Text", "text": "x"}]),
                ),
                update(
                    "b",
                    json!([{"id": "root", "component": "Label", "text": "x"}]),
                ),
            ])
            .unwrap();

        let wrong = update("b", json!([{"id": "t", "component": "Text", "text": "x"}]));
        assert_eq!(
            located(state.apply(&[wrong]).unwrap_err()),
            [(
                ErrorCode::UnknownComponent,
                "messages[0].updateComponents.components[0].component".to_string()
            )]
        );
    }

    #[test]
    fn a_surface_must_name_an_active_catalog() {
        let mut state = state();
        let before = state.clone();
        // The payload's fault, so a validation error, where the name is
        // written. It is looked at before the surface's place in its life.
        for (messages, path) in [
            (
                vec![create("a", "three")],
                "messages[0].createSurface.catalogId",
            ),
            (
                vec![create("a", "one"), create("a", "three")],
                "messages[1].createSurface.catalogId",
            ),
        ] {
            assert_eq!(
                located(state.apply(&messages).unwrap_err()),
                [(ErrorCode::InvalidValue, path.to_string())]
            );
            assert_eq!(state, before);
        }
    }

    #[test]
    fn a_message_whose_components_cannot_be_keyed_is_located_component_by_component() {
        let mut state = state();
        state.apply(&[create("a", "one")]).unwrap();
        let twice = update(
            "a",
            json!([
                {"id": "root", "component": "Text", "text": "x"},
                {"id": "root", "component": "Text", "text": "y"},
            ]),
        );
        assert_eq!(
            located(state.apply(&[twice]).unwrap_err()),
            [(
                ErrorCode::DuplicateId,
                "messages[0].updateComponents.components[1].id".to_string()
            )]
        );
        assert_eq!(
            located(state.apply(&[update("a", json!([]))]).unwrap_err()),
            [(
                ErrorCode::EmptyComponents,
                "messages[0].updateComponents.components".to_string()
            )]
        );
    }

    #[test]
    fn a_lifecycle_fault_is_a_validation_error_at_its_message() {
        let mut state = state();
        let orphan = update(
            "nope",
            json!([{"id": "root", "component": "Text", "text": "x"}]),
        );
        assert_eq!(
            located(state.apply(&[create("a", "one"), orphan]).unwrap_err()),
            [(ErrorCode::InvalidValue, "messages[1]".to_string())]
        );
        assert!(state.surface("a").is_none(), "the call is all or nothing");

        state.apply(&[create("a", "one")]).unwrap();
        assert_eq!(
            located(state.apply(&[create("a", "one")]).unwrap_err()),
            [(ErrorCode::InvalidValue, "messages[0]".to_string())]
        );
    }

    #[test]
    fn deleting_a_surface_the_renderer_does_not_hold_changes_nothing() {
        let mut state = state();
        let before = state.clone();
        assert_eq!(state.apply(&[delete("never")]).unwrap(), ["never"]);
        assert_eq!(state, before);

        // The store underneath stays strict about it.
        let typed: AgentMessage = serde_json::from_value(delete("never")).unwrap();
        assert!(SurfaceStore::new().apply(&typed).is_err());

        state
            .apply(&[create("a", "one"), delete("a"), delete("a")])
            .unwrap();
        assert!(state.surface("a").is_none());
        assert_eq!(
            state.store().lifecycle("a"),
            crate::protocol::surface::Lifecycle::Deleted
        );
    }

    #[test]
    fn a_component_that_cannot_be_decoded_is_located() {
        let mut state = state();
        let nameless = update(
            "a",
            json!([{"id": "root", "component": "Text", "text": "x"}, {"component": "Text", "text": "y"}]),
        );
        assert_eq!(
            located(state.apply(&[create("a", "one"), nameless]).unwrap_err()),
            [(
                ErrorCode::MissingId,
                "messages[1].updateComponents.components[1].id".to_string()
            )]
        );
    }

    #[test]
    fn an_envelope_fault_is_reported_before_anything_is_applied() {
        let mut state = state();
        let versionless = json!({"createSurface": {"surfaceId": "a", "catalogId": "one"}});
        assert_eq!(
            located(state.apply(&[versionless]).unwrap_err()),
            [(ErrorCode::MissingField, "messages[0].version".to_string())]
        );
        assert_eq!(
            located(state.apply(&[json!("create a surface")]).unwrap_err()),
            [(ErrorCode::TypeMismatch, "messages[0]".to_string())]
        );
    }

    #[test]
    fn a_tree_may_be_unfinished_until_it_is_checked() {
        let mut state = state();
        let touched = state
            .apply(&[
                create("a", "one"),
                update(
                    "a",
                    json!([{"id": "root", "component": "Box", "children": ["late"]}]),
                ),
            ])
            .unwrap();
        assert_eq!(
            located(state.check_trees(&touched).unwrap_err()),
            [(
                ErrorCode::UnresolvedChild,
                "messages[1].updateComponents.components[0].children[0]".to_string()
            )]
        );

        let more = state
            .apply(&[update(
                "a",
                json!([{"id": "late", "component": "Text", "text": "x"}]),
            )])
            .unwrap();
        state.check_trees(&more).unwrap();
    }

    #[test]
    fn a_cycle_is_found_when_the_message_that_closes_it_is_applied() {
        let mut state = state();
        state
            .apply(&[
                create("a", "one"),
                update(
                    "a",
                    json!([{"id": "root", "component": "Box", "children": ["inner"]}]),
                ),
            ])
            .unwrap();
        let closing = update(
            "a",
            json!([{"id": "inner", "component": "Box", "children": ["root"]}]),
        );
        let before = state.clone();
        assert_eq!(
            located(state.apply(&[closing]).unwrap_err())[0].0,
            ErrorCode::ChildCycle
        );
        assert_eq!(state, before);
    }

    #[test]
    fn a_created_surface_with_nothing_in_it_yet_passes_the_tree_check() {
        let mut state = state();
        let touched = state.apply(&[create("a", "one")]).unwrap();
        state.check_trees(&touched).unwrap();
        state.check_trees(["a", "a", "never-created"]).unwrap();
    }

    #[test]
    fn a_failed_transaction_restores_the_surfaces() {
        let mut state = state();
        state.apply(&[create("a", "one")]).unwrap();
        let before = state.clone();
        let outcome: Result<()> = state.transaction(|state| {
            state.apply(&[create("b", "two"), delete("a")])?;
            Err(Error::parse("the caller changed its mind"))
        });
        assert!(outcome.is_err());
        assert_eq!(state, before);
    }

    #[test]
    fn a_state_needs_catalogs_of_one_family() {
        assert!(matches!(
            RendererState::new(Vec::new()),
            Err(Error::Catalog(_))
        ));
    }

    #[test]
    fn a_payload_is_checked_alone_and_an_updated_surface_is_assumed() {
        let catalogs = [catalog("one", "Text"), catalog("two", "Label")];
        // Updated, never created: assumed to exist, on the catalog that
        // defines what the payload adds to it.
        validate_payload(
            &catalogs,
            &[
                update(
                    "held",
                    json!([{"id": "t", "component": "Label", "text": "x"}]),
                ),
                delete("gone"),
            ],
        )
        .unwrap();
        // Its components are still checked.
        assert_eq!(
            located(
                validate_payload(
                    &catalogs,
                    &[update("held", json!([{"id": "t", "component": "Label"}]))],
                )
                .unwrap_err()
            )[0]
            .0,
            ErrorCode::MissingRequiredProp
        );
        // A surface the payload creates is held to the whole contract.
        assert_eq!(
            located(
                validate_payload(
                    &catalogs,
                    &[
                        create("new", "one"),
                        update(
                            "new",
                            json!([{"id": "t", "component": "Text", "text": "x"}])
                        ),
                    ],
                )
                .unwrap_err()
            )[0]
            .0,
            ErrorCode::NoRoot
        );
        // An update that arrives before its surface is created is rejected,
        // and so is one after the payload deleted the surface.
        for payload in [
            vec![
                update(
                    "s",
                    json!([{"id": "root", "component": "Text", "text": "x"}]),
                ),
                create("s", "one"),
            ],
            vec![
                delete("s"),
                update(
                    "s",
                    json!([{"id": "root", "component": "Text", "text": "x"}]),
                ),
            ],
        ] {
            assert!(validate_payload(&catalogs, &payload).is_err());
        }
    }
}
