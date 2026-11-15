//! The surface state machine: lifecycle and transactional operation replay.
//!
//! A surface id is in one of three places at any time ([`Lifecycle`]):
//!
//! ```text
//!   Absent ──createSurface──▶ Live ──deleteSurface──▶ Deleted
//!                              ▲                         │
//!                              └──────createSurface──────┘
//! ```
//!
//! `updateComponents` and `updateDataModel` act on a live surface and leave it
//! live; every other combination is an error. [`SurfaceStore`] is the one
//! implementation of that machine. Whole-stream validation, validation against
//! prior state, the stateful schema validator, history replay, the author and
//! the streaming parser all move a surface through [`SurfaceStore::apply`], so
//! they cannot disagree about what a stream of messages means.
//!
//! # Where a component came from
//!
//! A store also remembers which applied message last wrote each component.
//! That is not part of a surface's state — two stores holding the same surfaces
//! are equal however they got there — but it is what lets a failure found on a
//! folded surface be located in the message a model would have to change,
//! `messages[2].updateComponents.components[0]`, rather than by the component's
//! position in the surface.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::message::{AgentMessage, AgentPayload, ClientDataModel, Component};
use super::model::DataModel;
use super::version::A2uiVersion;
use crate::error::{Error, Result};

/// The observed state of one live or deleted surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SurfaceState {
    /// Wire version fixed by creation.
    pub version: String,
    /// Renderer-scoped surface ID.
    pub surface_id: String,
    /// Catalog fixed by creation.
    pub catalog_id: String,
    /// Catalog-defined theme observed at creation.
    pub theme: Option<serde_json::Value>,
    /// Whether this surface asks the renderer to echo its data model.
    pub send_data_model: Option<bool>,
    /// Last component definition by ID, in first-definition order.
    pub components: Vec<Component>,
    /// Exact data-model state, including undefined slots.
    pub data_model: DataModel,
    /// A delete was observed; only a new create can make it live.
    pub deleted: bool,
}

/// Where a surface id is in its life, as the messages applied so far leave it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle {
    /// Never created. Only `createSurface` may name it.
    Absent,
    /// Created, and not deleted since. Updates and a delete may name it.
    Live,
    /// Deleted. Only a new `createSurface` may name it.
    Deleted,
}

/// Which applied message last wrote what on one surface.
#[derive(Debug, Clone, Default)]
pub(crate) struct Origin {
    /// The `createSurface` that began the surface's current life, when one of
    /// the located messages was it.
    created: Option<usize>,
    /// The last `updateComponents` applied to it.
    updated: Option<usize>,
    /// Each component's latest definition, as (message, position).
    components: BTreeMap<String, (usize, usize)>,
}

impl Origin {
    /// Locates the component list as a whole: where a missing root or an empty
    /// surface would be fixed. `None` when no located message touched the
    /// surface.
    pub(crate) fn list(&self) -> Option<String> {
        match (self.updated, self.created) {
            (Some(message), _) => Some(format!("messages[{message}].updateComponents.components")),
            (None, Some(message)) => Some(format!("messages[{message}].createSurface")),
            (None, None) => None,
        }
    }

    /// Locates one component in the message that last wrote it. `None` for a
    /// component no located message wrote: it was carried over from prior
    /// state.
    pub(crate) fn component(&self, id: &str) -> Option<String> {
        self.components.get(id).map(|(message, position)| {
            format!("messages[{message}].updateComponents.components[{position}]")
        })
    }
}

/// A single renderer/conversation context. Never share this across users.
#[derive(Debug, Clone, Default)]
pub struct SurfaceStore {
    surfaces: BTreeMap<String, SurfaceState>,
    /// Bookkeeping for locators, keyed by surface id. Not state.
    origins: BTreeMap<String, Origin>,
    /// How many messages have been applied since the store, or the batch being
    /// located, began: the index the next one takes.
    applied: usize,
}

/// Two stores are equal when they hold the same surfaces, however the
/// messages that built them were numbered.
impl PartialEq for SurfaceStore {
    fn eq(&self, other: &Self) -> bool {
        self.surfaces == other.surfaces
    }
}

impl SurfaceStore {
    /// Empty state: updates require a previous create or explicitly supplied prior state.
    pub fn new() -> Self {
        Self::default()
    }
    /// Supplies observed prior state for incremental validation or replay.
    pub fn from_surface(surface: SurfaceState) -> Result<Self> {
        let mut ids = BTreeSet::new();
        if surface.components.iter().any(|c| !ids.insert(c.id.clone())) {
            return Err(Error::parse("duplicate component ID in prior surface"));
        }
        if A2uiVersion::parse(&surface.version).is_none() {
            return Err(Error::parse("unsupported prior surface version"));
        }
        Ok(Self {
            surfaces: BTreeMap::from([(surface.surface_id.clone(), surface)]),
            ..Self::default()
        })
    }
    /// Reads the latest state, including deleted tombstones.
    pub fn get(&self, id: &str) -> Option<&SurfaceState> {
        self.surfaces.get(id)
    }
    /// Every tracked surface in this renderer context.
    pub fn surfaces(&self) -> impl Iterator<Item = &SurfaceState> {
        self.surfaces.values()
    }
    /// Where a surface id is in its life.
    pub fn lifecycle(&self, id: &str) -> Lifecycle {
        match self.get(id) {
            None => Lifecycle::Absent,
            Some(surface) if surface.deleted => Lifecycle::Deleted,
            Some(_) => Lifecycle::Live,
        }
    }
    /// The `a2uiClientDataModel` metadata a renderer holding these surfaces
    /// attaches to what it sends: the data model of every live surface that
    /// was created with `sendDataModel: true`, stamped `version`.
    ///
    /// `None` when no surface asks for its data model back. A surface created
    /// under a version that shares no wire format with `version` is left out.
    ///
    /// ```
    /// use ag_ui_a2ui::{A2uiVersion, AgentMessage, AgentPayload, SurfaceStore};
    /// use serde_json::json;
    ///
    /// let mut echoed = AgentMessage::create_surface("form", "catalog");
    /// if let AgentPayload::CreateSurface(create) = &mut echoed.payload {
    ///     create.send_data_model = Some(true);
    /// }
    /// let mut store = SurfaceStore::new();
    /// store.apply(&echoed)?;
    /// store.apply(&AgentMessage::create_surface("banner", "catalog"))?;
    /// store.apply(&AgentMessage::update_data_model("form", "/email", json!("a@b.c")))?;
    /// store.apply(&AgentMessage::update_data_model("banner", "/text", json!("hi")))?;
    ///
    /// let metadata = store.client_data_model(A2uiVersion::V0_9)?.expect("one surface asks");
    /// assert_eq!(
    ///     serde_json::to_value(metadata)?,
    ///     json!({"version": "v0.9", "surfaces": {"form": {"email": "a@b.c"}}})
    /// );
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// The metadata carries JSON objects. Returns [`Error::Undefined`] when
    /// the model of a surface that asks holds an undefined slot, and
    /// [`Error::Parse`] when its root is not an object.
    pub fn client_data_model(&self, version: A2uiVersion) -> Result<Option<ClientDataModel>> {
        let mut metadata = ClientDataModel::new(version);
        for surface in self.surfaces.values() {
            let shares_format = A2uiVersion::parse(&surface.version)
                .is_some_and(|created| created.accepts(version));
            if surface.deleted || surface.send_data_model != Some(true) || !shares_format {
                continue;
            }
            let serde_json::Value::Object(model) = surface.data_model.to_json()? else {
                return Err(Error::parse(format!(
                    "the data model of surface {:?} is not an object, and a2uiClientDataModel \
                     carries objects",
                    surface.surface_id
                )));
            };
            metadata.surfaces.insert(surface.surface_id.clone(), model);
        }
        Ok((!metadata.surfaces.is_empty()).then_some(metadata))
    }
    /// The surface an update or delete may target: created, and not deleted since.
    pub(crate) fn live(&self, id: &str) -> Result<&SurfaceState> {
        self.get(id).filter(|s| !s.deleted).ok_or_else(|| {
            Error::parse(format!(
                "surface {id:?} must be created before updating or deleting it"
            ))
        })
    }
    /// Atomically applies one operation. Prior messages are never rolled back.
    pub fn apply(&mut self, message: &AgentMessage) -> Result<()> {
        self.apply_at(self.applied, message)
    }
    /// [`SurfaceStore::apply`], for a caller that numbers the messages itself
    /// because some of them never reach the store. `index` is the message's
    /// position in the caller's batch, and is what a locator will name.
    pub(crate) fn apply_at(&mut self, index: usize, message: &AgentMessage) -> Result<()> {
        self.transition(message)?;
        self.record(index, message);
        self.applied = index + 1;
        Ok(())
    }
    /// Starts a new batch of located messages: forgets where components came
    /// from and numbers the next message applied `0`. The surfaces stay.
    ///
    /// Components already held then count as prior state, and are located by
    /// their position in the surface.
    pub(crate) fn begin_batch(&mut self) {
        self.origins.clear();
        self.applied = 0;
    }
    /// Where the messages applied since the batch began wrote this surface.
    pub(crate) fn origin(&self, surface_id: &str) -> Option<&Origin> {
        self.origins.get(surface_id)
    }

    /// The state machine itself: one message, all or nothing.
    fn transition(&mut self, message: &AgentMessage) -> Result<()> {
        let Some(version) = A2uiVersion::parse(&message.version) else {
            return Err(Error::parse("unsupported A2UI version"));
        };
        let id = message
            .surface_id()
            .ok_or_else(|| Error::parse("RPC messages are not part of A2UI v0.9 or v0.9.1"))?;
        if let AgentPayload::CreateSurface(create) = &message.payload {
            if self.lifecycle(id) == Lifecycle::Live {
                return Err(Error::parse(format!("surface {id:?} already exists")));
            }
            self.surfaces.insert(
                id.into(),
                SurfaceState {
                    version: message.version.clone(),
                    surface_id: id.into(),
                    catalog_id: create.catalog_id.clone(),
                    theme: create.theme.clone(),
                    send_data_model: create.send_data_model,
                    components: vec![],
                    data_model: DataModel::default(),
                    deleted: false,
                },
            );
            return Ok(());
        }
        let mut next = self.live(id)?.clone();
        // Which discriminators may meet on one surface is the version's rule:
        // v0.9.1 explicitly accepts either one of its family.
        if let Some(created) = A2uiVersion::parse(&next.version) {
            if !created.accepts(version) {
                return Err(Error::parse(format!(
                    "surface {id:?} was created as {created}; a {version} message cannot update it"
                )));
            }
        }
        match &message.payload {
            AgentPayload::UpdateComponents(update) => {
                if update.components.is_empty() {
                    return Err(Error::parse("updateComponents must not be empty"));
                }
                let mut seen = BTreeSet::new();
                for component in &update.components {
                    if !seen.insert(&component.id) {
                        return Err(Error::parse(format!(
                            "duplicate component ID {:?} within one message",
                            component.id
                        )));
                    }
                    if let Some(existing) =
                        next.components.iter_mut().find(|c| c.id == component.id)
                    {
                        *existing = component.clone()
                    } else {
                        next.components.push(component.clone())
                    }
                }
            }
            AgentPayload::UpdateDataModel(update) => update.apply_model(&mut next.data_model)?,
            AgentPayload::DeleteSurface(_) => {
                next.deleted = true;
                next.components.clear();
                next.data_model = DataModel::default();
            }
            _ => return Err(Error::parse("unsupported operation")),
        }
        self.surfaces.insert(id.into(), next);
        Ok(())
    }

    /// Notes what a message that was just applied wrote.
    fn record(&mut self, index: usize, message: &AgentMessage) {
        let Some(surface_id) = message.surface_id() else {
            return;
        };
        match &message.payload {
            AgentPayload::CreateSurface(_) => {
                self.origins.insert(
                    surface_id.to_string(),
                    Origin {
                        created: Some(index),
                        ..Origin::default()
                    },
                );
            }
            AgentPayload::UpdateComponents(update) => {
                let origin = self.origins.entry(surface_id.to_string()).or_default();
                origin.updated = Some(index);
                for (position, component) in update.components.iter().enumerate() {
                    origin
                        .components
                        .insert(component.id.clone(), (index, position));
                }
            }
            // A deleted surface holds nothing left to locate.
            AgentPayload::DeleteSurface(_) => {
                self.origins.remove(surface_id);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(id: &str) -> Component {
        Component::new(id, "Text").with("text", json!(id))
    }

    #[test]
    fn a_surface_moves_through_absent_live_deleted_and_live_again() {
        let mut store = SurfaceStore::new();
        assert_eq!(store.lifecycle("s"), Lifecycle::Absent);

        // Nothing but a create may name a surface that is not live.
        for early in [
            AgentMessage::update_components("s", vec![text("root")]),
            AgentMessage::update_data_model("s", "/x", json!(1)),
            AgentMessage::delete_surface("s"),
        ] {
            assert!(matches!(store.apply(&early), Err(Error::Parse(_))));
            assert_eq!(store.lifecycle("s"), Lifecycle::Absent);
        }

        store
            .apply(&AgentMessage::create_surface("s", "c"))
            .unwrap();
        assert_eq!(store.lifecycle("s"), Lifecycle::Live);
        assert!(matches!(
            store.apply(&AgentMessage::create_surface("s", "c")),
            Err(Error::Parse(reason)) if reason.contains("already exists")
        ));

        store
            .apply(&AgentMessage::update_components("s", vec![text("root")]))
            .unwrap();
        store.apply(&AgentMessage::delete_surface("s")).unwrap();
        assert_eq!(store.lifecycle("s"), Lifecycle::Deleted);
        assert!(store.get("s").unwrap().components.is_empty());
        assert!(store.apply(&AgentMessage::delete_surface("s")).is_err());

        store
            .apply(&AgentMessage::create_surface("s", "c"))
            .unwrap();
        assert_eq!(store.lifecycle("s"), Lifecycle::Live);
    }

    #[test]
    fn either_discriminator_of_the_family_may_update_a_surface() {
        for (created, updated) in [
            (A2uiVersion::V0_9, A2uiVersion::V0_9_1),
            (A2uiVersion::V0_9_1, A2uiVersion::V0_9),
        ] {
            let mut store = SurfaceStore::new();
            store
                .apply(&AgentMessage::create_surface("s", "c").with_version(created))
                .unwrap();
            store
                .apply(
                    &AgentMessage::update_components("s", vec![text("root")]).with_version(updated),
                )
                .unwrap();
            // The surface keeps the version it was created under.
            assert_eq!(store.get("s").unwrap().version, created.as_str());
        }
    }

    #[test]
    fn a_version_this_crate_does_not_speak_is_refused_everywhere() {
        let mut message = AgentMessage::create_surface("s", "c");
        message.version = "1.0".into();
        let mut store = SurfaceStore::new();
        assert!(matches!(
            store.apply(&message),
            Err(Error::Parse(reason)) if reason == "unsupported A2UI version"
        ));
        assert_eq!(store.lifecycle("s"), Lifecycle::Absent);

        store
            .apply(&AgentMessage::create_surface("s", "c"))
            .unwrap();
        let mut prior = store.get("s").unwrap().clone();
        prior.version = "1.0".into();
        assert!(SurfaceStore::from_surface(prior).is_err());
    }

    #[test]
    fn a_failed_message_changes_nothing_and_takes_no_number() {
        let mut store = SurfaceStore::new();
        store
            .apply(&AgentMessage::create_surface("s", "c"))
            .unwrap();
        let before = store.clone();

        let duplicate = AgentMessage::update_components("s", vec![text("a"), text("a")]);
        assert!(store.apply(&duplicate).is_err());
        assert_eq!(store, before);

        store
            .apply(&AgentMessage::update_components("s", vec![text("a")]))
            .unwrap();
        assert_eq!(
            store.origin("s").unwrap().component("a").as_deref(),
            Some("messages[1].updateComponents.components[0]"),
            "the rejected message was never part of the stream"
        );
    }

    #[test]
    fn origins_name_the_message_and_position_that_last_wrote_a_component() {
        let mut store = SurfaceStore::new();
        store
            .apply(&AgentMessage::create_surface("s", "c"))
            .unwrap();
        assert_eq!(
            store.origin("s").unwrap().list().as_deref(),
            Some("messages[0].createSurface")
        );

        store
            .apply(&AgentMessage::update_components(
                "s",
                vec![text("root"), text("a")],
            ))
            .unwrap();
        store
            .apply(&AgentMessage::update_components(
                "s",
                vec![text("b"), text("a")],
            ))
            .unwrap();
        let origin = store.origin("s").unwrap();
        assert_eq!(
            origin.list().as_deref(),
            Some("messages[2].updateComponents.components")
        );
        assert_eq!(
            origin.component("root").as_deref(),
            Some("messages[1].updateComponents.components[0]")
        );
        assert_eq!(
            origin.component("a").as_deref(),
            Some("messages[2].updateComponents.components[1]"),
            "a redefinition moves the locator to the message that redefined it"
        );
        assert_eq!(origin.component("nope"), None);
    }

    #[test]
    fn a_new_batch_forgets_origins_and_keeps_the_surfaces() {
        let mut store = SurfaceStore::new();
        store
            .apply(&AgentMessage::create_surface("s", "c"))
            .unwrap();
        store
            .apply(&AgentMessage::update_components("s", vec![text("root")]))
            .unwrap();
        let surfaces = store.clone();

        store.begin_batch();
        assert_eq!(store, surfaces, "origins are not state");
        assert!(store.origin("s").is_none());

        store
            .apply(&AgentMessage::update_components("s", vec![text("a")]))
            .unwrap();
        let origin = store.origin("s").unwrap();
        assert_eq!(
            origin.component("a").as_deref(),
            Some("messages[0].updateComponents.components[0]")
        );
        assert_eq!(
            origin.component("root"),
            None,
            "a component from before the batch has no message to name"
        );
    }

    #[test]
    fn deleting_a_surface_drops_its_origins() {
        let mut store = SurfaceStore::new();
        store
            .apply(&AgentMessage::create_surface("s", "c"))
            .unwrap();
        store
            .apply(&AgentMessage::update_components("s", vec![text("root")]))
            .unwrap();
        store.apply(&AgentMessage::delete_surface("s")).unwrap();
        assert!(store.origin("s").is_none());

        store
            .apply(&AgentMessage::create_surface("s", "c"))
            .unwrap();
        let origin = store.origin("s").unwrap();
        assert_eq!(origin.list().as_deref(), Some("messages[3].createSurface"));
        assert_eq!(origin.component("root"), None);
    }
}
