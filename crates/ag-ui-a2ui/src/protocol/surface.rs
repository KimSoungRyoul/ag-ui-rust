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
//! prior state, the message processor, the stateful schema validator, history
//! replay and the agent layer, reading an answer whole or as it streams, all
//! move a surface through [`SurfaceStore::apply`], so they cannot disagree
//! about what a stream of messages means.
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
use serde_json::Value;

use super::binding::{MAX_VALUE_DEPTH, Scope};
use super::catalog::{Catalog, ComponentRef};
use super::message::{
    AgentMessage, AgentPayload, ClientDataModel, Component, CreateSurface, UpdateComponents,
};
use super::model::{DataModel, MAX_LIST_INDEX, ModelValue};
use super::validate::MAX_DEPTH;
use super::version::A2uiVersion;
use crate::constants::ROOT_ID;
use crate::error::{Error, Result};

/// The observed state of one live or deleted surface.
///
/// Non-exhaustive: a later protocol version may give a surface more. A state
/// comes out of a [`SurfaceStore`], or is read back from one it was
/// serialized from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SurfaceState {
    /// Wire version fixed by creation.
    pub version: String,
    /// Renderer-scoped surface ID.
    pub surface_id: String,
    /// Catalog fixed by creation. Before v1.0 a surface always has one. From
    /// v1.0 it is the surface's default and may be absent, each component then
    /// naming its own ([`SurfaceState::catalog_of`]).
    pub catalog_id: Option<String>,
    /// Catalog-defined theme observed at creation. The v0.9 family only.
    pub theme: Option<serde_json::Value>,
    /// Surface-level metadata observed at creation. From v1.0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    /// Whether this surface asks the renderer to echo its data model.
    pub send_data_model: Option<bool>,
    /// Last component definition by ID, in first-definition order.
    pub components: Vec<Component>,
    /// Exact data-model state, including undefined slots.
    pub data_model: DataModel,
    /// A delete was observed; only a new create can make it live.
    pub deleted: bool,
}

/// One component, where a renderer would draw it.
///
/// What [`SurfaceState::outline`] yields. Nothing here draws: it says which
/// components the root reaches, in what order and how deep, and which scope
/// each one's bindings resolve in.
#[derive(Debug, Clone)]
pub struct Placed<'a> {
    /// How far below the root the component sits. The root is at `0`.
    pub depth: usize,
    /// The component.
    pub component: &'a Component,
    /// The scope its bindings resolve in: the root of the data model, or the
    /// item a list template is drawing it for.
    pub scope: Scope<'a>,
}

impl SurfaceState {
    /// The protocol version the surface was created under, which is the
    /// spelling of everything it holds: its bindings, its components'
    /// envelope keys, how a removal is written.
    ///
    /// [`A2uiVersion::DEFAULT`] for a state whose `version` is not one this
    /// crate speaks, which a [`SurfaceStore`] never holds.
    pub fn spoken_version(&self) -> A2uiVersion {
        A2uiVersion::parse(&self.version).unwrap_or(A2uiVersion::DEFAULT)
    }

    /// The id of the catalog `component` is drawn from: the one the component
    /// names, where the surface's version lets a component name one
    /// (`@catalogId` in v1.0), and otherwise the surface's own.
    ///
    /// `None` when neither names one, which a renderer refuses to draw.
    ///
    /// ```
    /// use ag_ui_a2ui::message::{AgentMessage, AgentPayload, Component, CreateSurface};
    /// use ag_ui_a2ui::{A2uiVersion, SurfaceStore};
    /// use serde_json::json;
    ///
    /// let create = CreateSurface::new("dashboard")
    ///     .with_catalog_id("basic")
    ///     .with_components(vec![
    ///         Component::new("root", "Column").with("children", json!(["chart"])),
    ///         Component::new("chart", "LineChart").with("@catalogId", json!("charts")),
    ///     ]);
    /// let mut store = SurfaceStore::new();
    /// store.apply(&AgentMessage::new(AgentPayload::CreateSurface(create)).with_version(A2uiVersion::V1_0))?;
    ///
    /// let surface = store.get("dashboard").unwrap();
    /// assert_eq!(surface.catalog_of(&surface.components[0]), Some("basic"));
    /// assert_eq!(surface.catalog_of(&surface.components[1]), Some("charts"));
    /// # Ok::<(), ag_ui_a2ui::Error>(())
    /// ```
    pub fn catalog_of<'a>(&'a self, component: &'a Component) -> Option<&'a str> {
        component
            .catalog_id(self.spoken_version())
            .or(self.catalog_id.as_deref())
    }

    /// The messages that rebuild this surface on a renderer that holds
    /// nothing: its creation, every component, and its data.
    ///
    /// That is what a transport sends when one message has to stand on its
    /// own, such as a snapshot. A surface that was deleted ends in its
    /// `deleteSurface`.
    ///
    /// # How the data model is written
    ///
    /// At the root, in one message, as far as JSON can say it and a
    /// [`MessageProcessor`](crate::processor::MessageProcessor) takes it; the
    /// rest in a message of its own for each part, after it. What the replay
    /// writes follows the values the model holds, not the slots no message
    /// wrote:
    ///
    /// - A list with undefined slots (a JSON array cannot leave one out) is
    ///   written by whichever of two forms takes fewer messages. Either the
    ///   elements up to its first undefined slot, then each later element at
    ///   its index, which leaves the slots between undefined as a renderer
    ///   leaves them, and for an undefined last slot a placeholder written
    ///   there and removed. Or the whole list with `null` in each undefined
    ///   slot, then a removal of each. Only the second form can write a list
    ///   longer than [`MAX_LIST_INDEX`](crate::model::MAX_LIST_INDEX) allows
    ///   an index, and only it can write a `null` element after a gap on a
    ///   v1.0 surface, where a written `null` removes.
    /// - A part of the model too deep for one message, which a processor
    ///   refuses past [`MAX_DEPTH`], is written at its own path. Parts start
    ///   at fixed depths, so that a model built by any path of up to
    ///   [`MAX_VALUE_DEPTH`](crate::binding::MAX_VALUE_DEPTH) segments is
    ///   written by paths no longer than that.
    /// - A removed root, which a v0.9 model holds as undefined, is removed
    ///   again.
    ///
    /// ```
    /// use ag_ui_a2ui::{AgentMessage, Component, SurfaceStore};
    /// use serde_json::json;
    ///
    /// let mut store = SurfaceStore::new();
    /// for message in [
    ///     AgentMessage::create_surface("cart", "basic"),
    ///     AgentMessage::update_components("cart", vec![Component::new("root", "Text")]),
    ///     AgentMessage::update_data_model("cart", "/total", json!(12)),
    ///     AgentMessage::update_data_model("cart", "/total", json!(15)),
    /// ] {
    ///     store.apply(&message)?;
    /// }
    ///
    /// let replay = store.get("cart").unwrap().replay();
    /// assert_eq!(replay.len(), 3, "one creation, one component list, one data model");
    /// let mut again = SurfaceStore::new();
    /// for message in &replay {
    ///     again.apply(message)?;
    /// }
    /// assert_eq!(again.get("cart"), store.get("cart"));
    /// # Ok::<(), ag_ui_a2ui::Error>(())
    /// ```
    pub fn replay(&self) -> Vec<AgentMessage> {
        let versioned = |mut message: AgentMessage| {
            message.version.clone_from(&self.version);
            message
        };
        let mut payload = CreateSurface::new(&self.surface_id);
        payload.catalog_id.clone_from(&self.catalog_id);
        payload.theme.clone_from(&self.theme);
        payload.metadata.clone_from(&self.metadata);
        payload.send_data_model = self.send_data_model;
        let create = AgentMessage::new(AgentPayload::CreateSurface(payload));
        let mut messages = vec![versioned(create)];
        if !self.components.is_empty() {
            messages.push(versioned(AgentMessage::update_components(
                &self.surface_id,
                self.components.clone(),
            )));
        }
        let null_removes = self.spoken_version().null_removes();
        messages.extend(
            data_messages(&self.surface_id, self.data_model.root(), null_removes)
                .into_iter()
                .map(versioned),
        );
        if self.deleted {
            messages.push(versioned(AgentMessage::delete_surface(&self.surface_id)));
        }
        messages
    }

    /// The components the root reaches, in the order a renderer draws them:
    /// a parent, then each of its children in turn.
    ///
    /// `catalog` says which properties of a component name children. A list
    /// template is followed once for each item of the list it names in
    /// `data`, and the component it draws is placed in that item's scope, so
    /// its relative paths resolve. `data` is the data model as JSON
    /// ([`DataModel::to_json`]).
    ///
    /// Each scope reads a binding as the surface's version writes one
    /// ([`SurfaceState::spoken_version`]). The walk follows the references
    /// `catalog` knows: on a v1.0 surface that draws from several catalogs,
    /// a component of another catalog is placed and its children are not.
    ///
    /// This is the walk a client makes before it can draw anything, and it
    /// draws nothing itself. A reference to a component that does not exist
    /// is skipped, and so is one that would loop or nest deeper than
    /// [`MAX_DEPTH`]: an outline is of what is there. Whether a surface is
    /// whole is the validator's to say.
    ///
    /// # An outline can be far larger than its surface
    ///
    /// A component is placed once for every route the root has to it, and a
    /// surface the validator accepts may have many when its components share
    /// children, so the placements can grow far faster than the components. A
    /// list template inside another multiplies the items the same way. So the
    /// outline is lazy: [`Outline`] places one component each time it is
    /// advanced, holds no more than the path from the root, and stops when
    /// its caller does. A client drawing a surface it did not write takes
    /// what it can draw, with [`Iterator::take`], rather than all of it.
    ///
    /// ```
    /// use ag_ui_a2ui::{AgentMessage, Catalog, Component, SurfaceStore};
    /// use serde_json::json;
    ///
    /// let mut store = SurfaceStore::new();
    /// for message in [
    ///     AgentMessage::create_surface("tasks", "basic"),
    ///     AgentMessage::update_components("tasks", vec![
    ///         Component::new("root", "Column").with("children", json!(["title", "list"])),
    ///         Component::new("title", "Text").with("text", json!("Today")),
    ///         Component::new("list", "List")
    ///             .with("children", json!({"componentId": "row", "path": "/tasks"})),
    ///         Component::new("row", "Text").with("text", json!({"path": "name"})),
    ///     ]),
    ///     AgentMessage::update_data_model("tasks", "/", json!({
    ///         "tasks": [{"name": "Write"}, {"name": "Review"}]
    ///     })),
    /// ] {
    ///     store.apply(&message)?;
    /// }
    /// let surface = store.get("tasks").unwrap();
    /// let data = surface.data_model.to_json()?;
    ///
    /// let lines: Vec<String> = surface
    ///     .outline(&Catalog::basic(), &data)
    ///     .take(100)
    ///     .filter(|placed| placed.component.component == "Text")
    ///     .map(|placed| {
    ///         let text = placed.component.prop("text").unwrap();
    ///         format!("{}{}", "  ".repeat(placed.depth), placed.scope.resolve_dynamic(text).unwrap())
    ///     })
    ///     .collect();
    /// assert_eq!(lines, ["  \"Today\"", "    \"Write\"", "    \"Review\""]);
    /// # Ok::<(), ag_ui_a2ui::Error>(())
    /// ```
    pub fn outline<'a>(&'a self, catalog: &'a Catalog, data: &'a Value) -> Outline<'a> {
        let by_id: BTreeMap<&str, &Component> = self
            .components
            .iter()
            .map(|component| (component.id.as_str(), component))
            .collect();
        // Bindings are read as the surface's version writes them.
        let next = by_id
            .get(ROOT_ID)
            .map(|root| (*root, Scope::root(data).speaking(self.spoken_version())));
        Outline {
            catalog,
            by_id,
            next,
            path: Vec::new(),
        }
    }
}

/// The components a surface's root reaches, placed one at a time.
///
/// What [`SurfaceState::outline`] returns: an iterator of [`Placed`], in the
/// order a renderer draws them. Each step places one component and walks only
/// as far as that takes. The walk holds the path from the root to the last
/// component placed and nothing else, so a caller that stops early pays for
/// what it took.
#[derive(Debug, Clone)]
pub struct Outline<'a> {
    catalog: &'a Catalog,
    by_id: BTreeMap<&'a str, &'a Component>,
    /// The component to place next, in its scope, below the end of `path`.
    next: Option<(&'a Component, Scope<'a>)>,
    /// The components from the root down, each with the children it has
    /// still to place.
    path: Vec<Step<'a>>,
}

/// One component on an outline's path, and how far its children have been
/// placed.
#[derive(Debug, Clone)]
struct Step<'a> {
    component: &'a Component,
    scope: Scope<'a>,
    /// The references the component makes, in the order the catalog reports.
    references: Vec<ComponentRef>,
    /// The reference whose child is placed next.
    at: usize,
    /// Of a template reference: how many items its list has, and the next.
    items: Option<(usize, usize)>,
}

impl<'a> Step<'a> {
    /// The next child to place, in its scope, or `None` once every reference
    /// has been followed. `on_path` says whether a component is on the path
    /// from the root down to this one: placing it again would loop.
    fn next_child(
        &mut self,
        by_id: &BTreeMap<&'a str, &'a Component>,
        on_path: impl Fn(&str) -> bool,
    ) -> Option<(&'a Component, Scope<'a>)> {
        let component = self.component;
        while let Some(reference) = self.references.get(self.at) {
            let child = by_id
                .get(reference.id.as_str())
                .copied()
                .filter(|child| !on_path(&child.id));
            let Some(child) = child else {
                self.at += 1;
                continue;
            };
            // A template names its child once and draws it for every item.
            let template = reference
                .location
                .strip_suffix(".componentId")
                .and_then(|property| component.props.get(property)?.get("path")?.as_str());
            let Some(path) = template else {
                self.at += 1;
                return Some((child, self.scope.clone()));
            };
            let scope = &self.scope;
            let (items, item) = *self.items.get_or_insert_with(|| {
                let items = scope
                    .resolve(path)
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                (items, 0)
            });
            if item < items {
                self.items = Some((items, item + 1));
                return Some((child, self.scope.item(path, item)));
            }
            self.items = None;
            self.at += 1;
        }
        None
    }
}

impl<'a> Iterator for Outline<'a> {
    type Item = Placed<'a>;

    fn next(&mut self) -> Option<Placed<'a>> {
        loop {
            if let Some((component, scope)) = self.next.take() {
                let depth = self.path.len();
                let placed = Placed {
                    depth,
                    component,
                    scope: scope.clone(),
                };
                // A component at the deepest a renderer draws is placed, and
                // its children are not.
                if depth < MAX_DEPTH {
                    self.path.push(Step {
                        component,
                        scope,
                        references: self.catalog.references(component),
                        at: 0,
                        items: None,
                    });
                }
                return Some(placed);
            }
            let (last, above) = self.path.split_last_mut()?;
            let on_path = |id: &str| {
                last.component.id == id || above.iter().any(|step| step.component.id == id)
            };
            match last.next_child(&self.by_id, on_path) {
                Some(child) => self.next = Some(child),
                None => {
                    self.path.pop();
                }
            }
        }
    }
}

/// How many levels a value may nest below an `updateDataModel`'s `value` in a
/// message a processor takes: the message is at depth 1, the operation at 2,
/// the value at 3, and nothing may be deeper than [`MAX_DEPTH`].
const VALUE_LEVELS: usize = MAX_DEPTH - 3;

/// Whether a replay writes a container that is `depth` segments below the
/// root in a message of its own, as the start of a part.
///
/// Parts start every [`VALUE_LEVELS`] counting up from [`MAX_VALUE_DEPTH`],
/// the longest path a write may name, so that the deepest part of a model
/// any write built starts at a path no longer than that, and no message
/// nests deeper than a processor takes. A write that starts elsewhere, at the
/// root or at an element of a list, reaches the next start within
/// [`VALUE_LEVELS`].
fn starts_a_part(depth: usize) -> bool {
    (1..=MAX_VALUE_DEPTH).contains(&depth) && (MAX_VALUE_DEPTH - depth) % VALUE_LEVELS == 0
}

/// One step of a data model's replay.
enum Write<'a> {
    /// Write `value`, which is `depth` segments below the root, at `path`.
    Value {
        path: String,
        depth: usize,
        value: &'a ModelValue,
    },
    /// Make the list at `path` `len` long, its last slot undefined: write a
    /// placeholder in that slot and remove it.
    Length { path: String, len: usize },
}

/// The messages that write `root` on a surface whose data model holds
/// nothing. `null_removes` is the surface version's
/// [`A2uiVersion::null_removes`]. See [`SurfaceState::replay`].
fn data_messages(surface_id: &str, root: &ModelValue, null_removes: bool) -> Vec<AgentMessage> {
    // As a v0.9 model holds a removed root.
    if matches!(root, ModelValue::Undefined) {
        return vec![AgentMessage::remove_data_model_value(surface_id, "/")];
    }
    let pointer = |path: &str| {
        if path.is_empty() {
            "/".to_string()
        } else {
            path.to_string()
        }
    };
    let mut messages = Vec::new();
    let mut writes = std::collections::VecDeque::from([Write::Value {
        path: String::new(),
        depth: 0,
        value: root,
    }]);
    while let Some(write) = writes.pop_front() {
        match write {
            Write::Value { path, depth, value } => {
                let mut removals = Vec::new();
                let mut part = Part {
                    null_removes,
                    writes: &mut writes,
                    removals: &mut removals,
                };
                let json = part.json(value, &path, depth);
                messages.push(AgentMessage::update_data_model(
                    surface_id,
                    pointer(&path),
                    json,
                ));
                messages.extend(
                    removals
                        .into_iter()
                        .map(|path| AgentMessage::remove_data_model_value(surface_id, path)),
                );
            }
            Write::Length { path, len } => {
                let last = format!("{path}/{}", len - 1);
                messages.push(AgentMessage::update_data_model(
                    surface_id,
                    &last,
                    Value::Bool(false),
                ));
                messages.push(AgentMessage::remove_data_model_value(surface_id, last));
            }
        }
    }
    messages
}

/// What writing one part of a model as JSON leaves to do: later writes, and
/// the removals that follow this one.
struct Part<'a, 'w> {
    null_removes: bool,
    writes: &'w mut std::collections::VecDeque<Write<'a>>,
    removals: &'w mut Vec<String>,
}

impl<'a> Part<'a, '_> {
    /// `value`, `depth` segments below the root at `path`, as JSON. What JSON
    /// cannot say there, or a message cannot nest, is left to later writes.
    fn json(&mut self, value: &'a ModelValue, path: &str, depth: usize) -> Value {
        match value {
            // Never written itself: a slot is left as one, a field left out.
            ModelValue::Undefined | ModelValue::Null => Value::Null,
            ModelValue::Bool(flag) => Value::Bool(*flag),
            ModelValue::Number(number) => Value::Number(number.clone()),
            ModelValue::String(text) => Value::String(text.clone()),
            ModelValue::Object(fields) => Value::Object(
                fields
                    .iter()
                    .filter(|(_, field)| !matches!(field, ModelValue::Undefined))
                    .filter_map(|(key, field)| {
                        let at = format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"));
                        let json = self.child(field, at, depth + 1)?;
                        Some((key.clone(), json))
                    })
                    .collect(),
            ),
            ModelValue::Array(slots) => self.list(slots, path, depth),
        }
    }

    /// A child at `path` as JSON, or `None` when it starts a part of its own,
    /// which is then written after this one.
    fn child(&mut self, value: &'a ModelValue, path: String, depth: usize) -> Option<Value> {
        let holds_something = match value {
            ModelValue::Array(slots) => !slots.is_empty(),
            ModelValue::Object(fields) => !fields.is_empty(),
            _ => false,
        };
        if holds_something && starts_a_part(depth) {
            self.writes.push_back(Write::Value { path, depth, value });
            return None;
        }
        Some(self.json(value, &path, depth))
    }

    /// A list, in the form of [`SurfaceState::replay`] that takes fewer
    /// messages when it has undefined slots.
    fn list(&mut self, slots: &'a [ModelValue], path: &str, depth: usize) -> Value {
        let undefined = |slot: &ModelValue| matches!(slot, ModelValue::Undefined);
        let Some(first_gap) = slots.iter().position(undefined) else {
            return self.elements(slots, path, depth);
        };
        let gaps = slots.iter().filter(|slot| undefined(slot)).count();
        let later = &slots[first_gap + 1..];
        let ends_in_a_gap = slots.last().is_some_and(undefined);
        let by_index = later.iter().filter(|slot| !undefined(slot)).count()
            + if ends_in_a_gap { 2 } else { 0 };
        let indexable = slots.len() <= MAX_LIST_INDEX + 1
            && !(self.null_removes && later.iter().any(|slot| matches!(slot, ModelValue::Null)));
        if !indexable || gaps <= by_index {
            // The whole list, `null` in each gap, and each gap removed.
            for (index, slot) in slots.iter().enumerate() {
                if undefined(slot) {
                    self.removals.push(format!("{path}/{index}"));
                }
            }
            return self.elements(slots, path, depth);
        }
        // The elements before the first gap, then each later one at its index.
        for (index, slot) in later.iter().enumerate() {
            if !undefined(slot) {
                self.writes.push_back(Write::Value {
                    path: format!("{path}/{}", first_gap + 1 + index),
                    depth: depth + 1,
                    value: slot,
                });
            }
        }
        if ends_in_a_gap {
            self.writes.push_back(Write::Length {
                path: path.to_string(),
                len: slots.len(),
            });
        }
        self.elements(&slots[..first_gap], path, depth)
    }

    /// The elements of a list as a JSON array, an undefined one as `null`.
    fn elements(&mut self, slots: &'a [ModelValue], path: &str, depth: usize) -> Value {
        Value::Array(
            slots
                .iter()
                .enumerate()
                .map(|(index, slot)| {
                    self.child(slot, format!("{path}/{index}"), depth + 1)
                        .unwrap_or(Value::Null)
                })
                .collect(),
        )
    }
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
    /// Whether the creation carried components itself, as a v1.0 one may.
    inline: bool,
}

impl Origin {
    /// Locates the component list as a whole: where a missing root or an empty
    /// surface would be fixed. `None` when no located message touched the
    /// surface.
    pub(crate) fn list(&self) -> Option<String> {
        match (self.updated, self.created) {
            (Some(message), _) => Some(format!("messages[{message}].updateComponents.components")),
            (None, Some(message)) if self.inline => {
                Some(format!("messages[{message}].createSurface.components"))
            }
            (None, Some(message)) => Some(format!("messages[{message}].createSurface")),
            (None, None) => None,
        }
    }

    /// Locates one component in the message that last wrote it. `None` for a
    /// component no located message wrote: it was carried over from prior
    /// state.
    pub(crate) fn component(&self, id: &str) -> Option<String> {
        self.components.get(id).map(|(message, position)| {
            let operation = if self.created == Some(*message) && self.inline {
                "createSurface"
            } else {
                "updateComponents"
            };
            format!("messages[{message}].{operation}.components[{position}]")
        })
    }
}

/// Folds one message's components into a surface's: a component whose id is
/// already there takes its place, a new one goes to the end.
fn fold_components(held: &mut Vec<Component>, incoming: &[Component]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for component in incoming {
        if !seen.insert(&component.id) {
            return Err(Error::parse(format!(
                "duplicate component ID {:?} within one message",
                component.id
            )));
        }
        if let Some(existing) = held.iter_mut().find(|c| c.id == component.id) {
            *existing = component.clone()
        } else {
            held.push(component.clone())
        }
    }
    Ok(())
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
    /// Drops everything known of a surface, tombstone included, so that a
    /// snapshot of it can take its place.
    #[cfg(feature = "ag-ui")]
    pub(crate) fn forget(&mut self, surface_id: &str) {
        self.surfaces.remove(surface_id);
        self.origins.remove(surface_id);
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
            if create.catalog_id.is_none() && !version.mixes_catalogs() {
                return Err(Error::parse(format!(
                    "a {version} surface names its catalog; createSurface for {id:?} names none"
                )));
            }
            let mut created = SurfaceState {
                version: message.version.clone(),
                surface_id: id.into(),
                catalog_id: create.catalog_id.clone(),
                theme: create.theme.clone().filter(|_| version.has_theme()),
                metadata: None,
                send_data_model: create.send_data_model,
                components: vec![],
                data_model: DataModel::default(),
                deleted: false,
            };
            // What a v1.0 creation carries itself. The data comes first, so
            // the components bind against it at once.
            if version.creates_inline() {
                created.metadata.clone_from(&create.metadata);
                if let Some(data_model) = &create.data_model {
                    created.data_model = DataModel::from(Value::Object(data_model.clone()));
                }
                if !create.components.is_empty() {
                    fold_components(&mut created.components, &create.components)?;
                }
            }
            self.surfaces.insert(id.into(), created);
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
            AgentPayload::UpdateComponents(UpdateComponents { components, .. }) => {
                if components.is_empty() {
                    return Err(Error::parse("updateComponents must not be empty"));
                }
                fold_components(&mut next.components, components)?;
            }
            // A removal is applied as the surface's own version applies one.
            AgentPayload::UpdateDataModel(update) => {
                let created = next.spoken_version();
                next.data_model
                    .apply_as(created, &update.path, &update.value)?;
            }
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
            AgentPayload::CreateSurface(create) => {
                let mut origin = Origin {
                    created: Some(index),
                    ..Origin::default()
                };
                // The components a v1.0 creation carried, when it was read as
                // one: they are located in the creation.
                if self
                    .surfaces
                    .get(surface_id)
                    .is_some_and(|surface| !surface.components.is_empty())
                {
                    origin.inline = true;
                    for (position, component) in create.components.iter().enumerate() {
                        origin
                            .components
                            .insert(component.id.clone(), (index, position));
                    }
                }
                self.origins.insert(surface_id.to_string(), origin);
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
        message.version = "v0.8".into();
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
        prior.version = "v0.8".into();
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
