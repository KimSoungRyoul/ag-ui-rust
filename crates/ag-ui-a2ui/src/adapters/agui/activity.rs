//! The `a2ui-surface` activity: how a surface reaches an AG-UI frontend.
//!
//! A frontend paints a surface from an `ACTIVITY_SNAPSHOT` whose
//! `activityType` is [`SURFACE_ACTIVITY_TYPE`] and whose content is an
//! operations envelope. The same activity, under the same message id, carries
//! the surface's life before it is painted: `building`, `retrying`, `failed`.
//! Every state replaces the one before it, so a frontend shows one thing per
//! surface: a skeleton, then the surface in its place.
//!
//! [`SurfacePainter`] decides which message id a paint goes under and what the
//! activity holds. It needs no server: it turns a batch of operations into the
//! tool result and the events, and whatever sends events sends them.
//!
//! The rules for which message id a paint goes under, and the lines of AG-UI's
//! middleware they follow, are in the documentation of [`SurfacePainter`].

use std::collections::{BTreeMap, BTreeSet};

use ag_ui::{Event, JsonObject, Message, MessageId};
use serde_json::{Value, json};

use super::envelope::{Attempt, operations_envelope, wrap_as_operations_envelope};
use super::history::{calls_showing_a_status, deletes_nothing, surfaces_in};
use crate::agent::Feedback;
use crate::error::Result;
use crate::protocol::message::AgentMessage;
use crate::protocol::surface::SurfaceStore;
use crate::protocol::validate::ValidationError;

/// The `activityType` of the activity a surface is painted from.
pub const SURFACE_ACTIVITY_TYPE: &str = "a2ui-surface";

/// The message id of the surface activity a tool call owns:
/// `a2ui-surface-<tool call id>`.
///
/// The call's lifecycle and its paint share it, which is what makes the
/// painted surface take the skeleton's place.
pub fn surface_activity_id(tool_call_id: &str) -> MessageId {
    MessageId::from(format!("{SURFACE_ACTIVITY_TYPE}-{tool_call_id}"))
}

/// Where a surface is before it is painted.
///
/// Shown with [`SurfacePainter::status`]. A later version may add a state, so
/// a `match` on one needs a wildcard arm.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SurfaceStatus {
    /// A model is writing the surface.
    Building,
    /// An attempt was rejected and another is being made.
    Retrying {
        /// The attempt now being made, counting from one.
        attempt: u32,
        /// How many attempts there are in all.
        max_attempts: u32,
        /// What was wrong with the attempt before it.
        errors: Vec<ValidationError>,
    },
    /// Every attempt was rejected, or the surface could not be made at all.
    Failed {
        /// What to tell the user.
        error: String,
        /// The attempts that were made.
        attempts: Vec<Attempt>,
        /// How many attempts there were in all.
        max_attempts: u32,
    },
}

impl SurfaceStatus {
    /// The status after a [`RetryDriver`](crate::agent::RetryDriver) said
    /// [`Verdict::Retry`](crate::agent::Verdict::Retry): the attempt after the
    /// rejected one is under way.
    pub fn retrying(feedback: &Feedback, max_attempts: u32) -> Self {
        Self::Retrying {
            attempt: (feedback.attempt() + 1).min(max_attempts.max(1)),
            max_attempts,
            errors: feedback.errors().to_vec(),
        }
    }

    /// The content of the activity in this state.
    fn content(&self) -> JsonObject {
        let content = match self {
            Self::Building => json!({"status": "building"}),
            Self::Retrying {
                attempt,
                max_attempts,
                errors,
            } => json!({
                "status": "retrying",
                "attempt": attempt,
                "maxAttempts": max_attempts,
                "errors": errors,
            }),
            Self::Failed {
                error,
                attempts,
                max_attempts,
            } => json!({
                "status": "failed",
                "error": error,
                "attempts": attempts,
                "maxAttempts": max_attempts,
            }),
        };
        match content {
            Value::Object(content) => content,
            _ => JsonObject::new(),
        }
    }
}

/// What painting a batch produces: the tool result, and the events after it.
///
/// A later version may add a field, so one is built with [`Painted::new`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Painted {
    /// The content of the tool result: the batch as it was given, as an
    /// operations envelope.
    pub envelope: String,
    /// One `ACTIVITY_SNAPSHOT` for each surface the batch changed, to be sent
    /// after the tool result. Empty when the batch changed none.
    pub activities: Vec<Event>,
}

impl Painted {
    /// A tool result and the activities to send after it.
    pub fn new(envelope: impl Into<String>, activities: Vec<Event>) -> Self {
        Self {
            envelope: envelope.into(),
            activities,
        }
    }
}

/// Paints surfaces on an AG-UI frontend, and keeps what the frontend holds.
///
/// A paint is two things on the wire, in this order:
///
/// 1. the **tool result**: the batch, as an operations envelope. This is what
///    the model sees and what the next request brings back.
/// 2. one **`a2ui-surface` activity** for each surface the batch touched,
///    carrying that surface whole: its creation, every component, its data.
///    A frontend replays exactly what an activity carries, so an update that
///    sent only what changed is painted as the surface it leaves.
///
/// Within one painter, which is one run, a surface keeps the activity it was
/// first painted under, and a later batch replaces it in place. A painter for
/// a later run starts from the conversation
/// ([`SurfacePainter::from_history`]) and paints under that run's tool call,
/// so to a client an update in a later run is a second activity carrying the
/// whole surface. That is what AG-UI's middleware does, and a renderer that
/// mounts one surface per activity is written against it.
///
/// # Which activity a paint goes under
///
/// A painter keeps what its frontend shows: the surfaces, the activity each
/// was painted under, and the tool calls whose activity shows a state in
/// place of a surface ([`SurfacePainter::status`]). A batch that is the
/// result of tool call `C` is painted by three rules, the first that applies
/// to a surface:
///
/// 1. **A call whose activity shows a state paints in it.** The first surface
///    of the batch that this painter has not painted takes `a2ui-surface-C`.
///    When it has painted them all, the first one moves there, and the
///    activity it was under keeps what it showed. Nothing but that call's
///    paint ever replaces its state, so a batch that went wholly under other
///    activities would leave the skeleton on screen for good.
/// 2. **A surface this painter painted before is painted where it is**, in
///    place, whichever call the batch is the result of.
/// 3. **A surface it has not painted** goes under `a2ui-surface-C` when it is
///    the only surface of the batch and no other surface is painted there,
///    and under `a2ui-surface-<surface id>-C` otherwise.
///
/// Rule 1 is the middleware's, in `middlewares/a2ui-middleware/src/index.ts`
/// at the commit the [adapter](super) names. Lines 676-678: a call whose slot
/// shows a state "resolves that slot instead of folding into another call's
/// activity and leaving the skeleton behind". Lines 1288-1302: of a result
/// that spans several surfaces, "while that slot still shows a lifecycle
/// state, the first fallback surface replaces it; a painted slot is never
/// taken over". The middleware needs the first only for `retrying`, since it
/// paints a `building` call while the call streams. A painter paints a
/// finished batch, so it holds every state to both.
///
/// A `deleteSurface` for a surface the frontend does not hold live is passed
/// over: it is not applied, its surface is not one the batch touched, and no
/// activity is sent for it. The tool result still carries it, as sent.
///
/// ```
/// use ag_ui::Event;
/// use ag_ui_a2ui::adapters::agui::{Intent, SurfacePainter, SurfaceSpec, assemble_ops};
/// use ag_ui_a2ui::{AgentMessage, Component};
/// use serde_json::json;
///
/// let spec = SurfaceSpec::new("cart")
///     .with_components(vec![Component::new("root", "Text").with("text", json!({"path": "/total"}))])
///     .with_data_model(json!({"total": "12.00"}));
/// let mut painter = SurfacePainter::new();
///
/// let created = painter.settle("call-1", &assemble_ops(Intent::Create, &spec))?;
/// assert!(created.envelope.starts_with(r#"{"a2ui_operations":[{"createSurface""#));
/// let Event::ActivitySnapshot(activity) = &created.activities[0] else { unreachable!() };
/// assert_eq!(activity.message_id.as_str(), "a2ui-surface-call-1");
///
/// // An update is sent as the change, and painted as the whole surface, in place.
/// let change = [AgentMessage::update_data_model("cart", "/total", json!("15.00"))];
/// let updated = painter.settle("call-2", &change)?;
/// let Event::ActivitySnapshot(activity) = &updated.activities[0] else { unreachable!() };
/// assert_eq!(activity.message_id.as_str(), "a2ui-surface-call-1");
/// let replayed = activity.content["a2ui_operations"].as_array().unwrap();
/// assert!(replayed[0].get("createSurface").is_some());
/// assert_eq!(replayed[2]["updateDataModel"]["value"], json!({"total": "15.00"}));
/// # Ok::<(), ag_ui_a2ui::Error>(())
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SurfacePainter {
    surfaces: SurfaceStore,
    /// The activity each surface was painted under, by this painter.
    painted: BTreeMap<String, MessageId>,
    /// The tool calls whose own activity shows a state, with no surface
    /// painted in its place since.
    showing_status: BTreeSet<String>,
}

/// A batch applied to a copy of what a painter's frontend holds.
struct Applied<'a> {
    /// What the frontend holds once the batch is painted.
    surfaces: SurfaceStore,
    /// The operations that changed it: the batch, less what was passed over.
    changes: Vec<&'a AgentMessage>,
}

impl SurfacePainter {
    /// A painter for a frontend that holds no surface.
    pub fn new() -> Self {
        Self::default()
    }

    /// A painter for a frontend that holds what `messages` drew: the messages
    /// of the request, on a server.
    ///
    /// A surface activity in `messages` that shows a state and not a surface
    /// is noted, as [`SurfacePainter::status`] notes one. Only a list that
    /// holds activity messages has any, and the request a server is sent does
    /// not, since a client leaves them out. So a painter made on a server
    /// knows nothing of a state an earlier run left on screen, and a later
    /// run's call has another id in any case: that activity stays as it is.
    ///
    /// # Errors
    ///
    /// As [`surfaces_in`].
    pub fn from_history(messages: &[Message]) -> Result<Self> {
        let mut painter = Self::with_surfaces(surfaces_in(messages)?);
        painter.showing_status = calls_showing_a_status(messages);
        Ok(painter)
    }

    /// A painter for a frontend that holds `surfaces`.
    pub fn with_surfaces(surfaces: SurfaceStore) -> Self {
        Self {
            surfaces,
            painted: BTreeMap::new(),
            showing_status: BTreeSet::new(),
        }
    }

    /// What the frontend holds.
    pub fn surfaces(&self) -> &SurfaceStore {
        &self.surfaces
    }

    /// The event that puts the surface activity of the tool call
    /// `tool_call_id` in `status`, and a note that the activity now shows
    /// that state.
    ///
    /// The note is why the paint that follows takes the state's place:
    /// [`SurfacePainter::settle`] for the same call paints in the call's own
    /// activity, wherever else its surfaces were painted before. So show a
    /// state with the painter that will paint for the call, and send the
    /// event this returns.
    ///
    /// A state stays on screen until something is painted in its place. A
    /// call that ends without a surface (every attempt was rejected, or the
    /// answer that stood drew nothing) says so with [`SurfaceStatus::Failed`].
    /// One that has no painter at hand, because it failed before it would
    /// paint, gives the event with `SurfacePainter::new().status(id, &failed)`.
    ///
    /// ```
    /// use ag_ui::Event;
    /// use ag_ui_a2ui::adapters::agui::{Intent, SurfacePainter, SurfaceSpec, SurfaceStatus, assemble_ops};
    /// use ag_ui_a2ui::Component;
    /// use serde_json::json;
    ///
    /// let mut painter = SurfacePainter::new();
    /// let Event::ActivitySnapshot(building) = painter.status("call-1", &SurfaceStatus::Building)
    /// else {
    ///     unreachable!()
    /// };
    /// assert_eq!(building.message_id.as_str(), "a2ui-surface-call-1");
    /// assert_eq!(building.activity_type, "a2ui-surface");
    /// assert_eq!(building.content["status"], "building");
    /// assert!(building.replace);
    ///
    /// // The paint that follows goes under the same id, so it takes the skeleton's place.
    /// let spec = SurfaceSpec::new("cart")
    ///     .with_components(vec![Component::new("root", "Text").with("text", json!("Cart"))]);
    /// let painted = painter.settle("call-1", &assemble_ops(Intent::Create, &spec))?;
    /// let Event::ActivitySnapshot(surface) = &painted.activities[0] else { unreachable!() };
    /// assert_eq!(surface.message_id, building.message_id);
    /// # Ok::<(), ag_ui_a2ui::Error>(())
    /// ```
    pub fn status(&mut self, tool_call_id: &str, status: &SurfaceStatus) -> Event {
        let own = surface_activity_id(tool_call_id);
        // The state takes the place of a surface that was painted there.
        self.painted.retain(|_, activity| *activity != own);
        self.showing_status.insert(tool_call_id.to_string());
        Event::activity_snapshot(own, SURFACE_ACTIVITY_TYPE, status.content())
    }

    /// Applies `operations` and gives what to send for them, as the result of
    /// the tool call `tool_call_id`.
    ///
    /// Which activity each surface goes under is the three rules of
    /// [`SurfacePainter`]: in the call's own activity when that shows a
    /// state, in place when this painter painted the surface before, and
    /// otherwise under `a2ui-surface-<tool call id>`, or
    /// `a2ui-surface-<surface id>-<tool call id>` when the batch touches
    /// several surfaces or that id is another surface's.
    ///
    /// A `deleteSurface` for a surface the frontend does not hold live is
    /// passed over, as a renderer passes it over. A batch of nothing else
    /// gives the tool result and no activity.
    ///
    /// # Errors
    ///
    /// Returns what [`SurfaceStore::apply`] returns when an operation does not
    /// apply to what the frontend holds. Nothing is changed then, and nothing
    /// is to be sent.
    pub fn settle(&mut self, tool_call_id: &str, operations: &[AgentMessage]) -> Result<Painted> {
        let applied = self.apply(operations)?;
        self.commit(tool_call_id, operations, applied)
    }

    /// Applies `operations` to a copy of what the frontend holds.
    fn apply<'a>(&self, operations: &'a [AgentMessage]) -> Result<Applied<'a>> {
        let mut surfaces = self.surfaces.clone();
        let mut changes = Vec::with_capacity(operations.len());
        for operation in operations {
            if deletes_nothing(&surfaces, operation) {
                continue;
            }
            surfaces.apply(operation)?;
            changes.push(operation);
        }
        Ok(Applied { surfaces, changes })
    }

    /// Takes an applied batch as what the frontend holds, and gives what to
    /// send for it. Nothing is changed when this fails.
    fn commit(
        &mut self,
        tool_call_id: &str,
        operations: &[AgentMessage],
        applied: Applied<'_>,
    ) -> Result<Painted> {
        let mut touched: Vec<&str> = Vec::new();
        for surface_id in applied
            .changes
            .iter()
            .filter_map(|change| change.surface_id())
        {
            if !touched.contains(&surface_id) {
                touched.push(surface_id);
            }
        }
        let envelope = wrap_as_operations_envelope(operations)?;

        let own = surface_activity_id(tool_call_id);
        let mut painted = self.painted.clone();
        let unpainted = |surface_id: &&&str| !self.painted.contains_key(**surface_id);
        let free = !painted.values().any(|activity| *activity == own);
        let showing_status = self.showing_status.contains(tool_call_id);
        // The surface that goes in the call's own activity, if one does.
        let takes_own = match touched.as_slice() {
            _ if !free => None,
            // Rule 1: the state gives way to a surface of the batch.
            all if showing_status => all.iter().find(unpainted).or(all.first()),
            // Rule 3: the only surface of a batch, when it is new.
            [only] => Some(only).filter(unpainted),
            _ => None,
        }
        .copied();

        let mut activities = Vec::with_capacity(touched.len());
        for surface_id in touched {
            let Some(surface) = applied.surfaces.get(surface_id) else {
                continue;
            };
            let message_id = match painted.get(surface_id) {
                _ if takes_own == Some(surface_id) => own.clone(),
                Some(owner) => owner.clone(),
                None => surface_activity_id(&format!("{surface_id}-{tool_call_id}")),
            };
            let Value::Object(content) = operations_envelope(&surface.replay())? else {
                continue;
            };
            activities.push(Event::activity_snapshot(
                message_id.clone(),
                SURFACE_ACTIVITY_TYPE,
                content,
            ));
            painted.insert(surface_id.to_string(), message_id);
        }
        if takes_own.is_some() {
            self.showing_status.remove(tool_call_id);
        }
        self.surfaces = applied.surfaces;
        self.painted = painted;
        Ok(Painted {
            envelope,
            activities,
        })
    }
}

#[cfg(feature = "ag-ui-server")]
impl SurfacePainter {
    /// Paints `operations` in a run: a `render_a2ui` tool call whose result is
    /// the batch, then the surface activities.
    ///
    /// ```
    /// use ag_ui::server::RunContext;
    /// use ag_ui::{Event, RunAgentInput};
    /// use ag_ui_a2ui::adapters::agui::{Intent, SurfacePainter, SurfaceSpec, assemble_ops};
    /// use ag_ui_a2ui::Component;
    /// use serde_json::json;
    ///
    /// let (mut ctx, mut events) = RunContext::<()>::new(RunAgentInput::new("thread", "run"))?;
    /// let spec = SurfaceSpec::new("cart")
    ///     .with_components(vec![Component::new("root", "Text").with("text", json!("Cart"))]);
    ///
    /// let mut painter = SurfacePainter::from_history(ctx.messages())?;
    /// painter.paint(&mut ctx, &assemble_ops(Intent::Create, &spec))?;
    ///
    /// let kinds: Vec<&str> = events.drain().iter().map(Event::event_type).map(|t| t.as_str()).collect();
    /// assert_eq!(
    ///     kinds,
    ///     ["TOOL_CALL_START", "TOOL_CALL_ARGS", "TOOL_CALL_END", "TOOL_CALL_RESULT", "ACTIVITY_SNAPSHOT"]
    /// );
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// A `deleteSurface` for a surface the frontend does not hold live is
    /// passed over, as [`SurfacePainter::settle`] passes it over: the call's
    /// result carries it, and its arguments name the first surface the batch
    /// changed.
    ///
    /// # What the call carries
    ///
    /// The call's arguments are the surface, not the batch: `surfaceId`, every
    /// component the surface holds once the batch is applied, and `data` when
    /// the batch writes the root of the data model. A batch that changes one
    /// component, or only data, is still sent as that batch in the result.
    ///
    /// `render_a2ui` describes a surface whole. Its `components` are required
    /// (AG-UI's `a2ui-middleware/src/tools.ts:54`, read at
    /// `ag-ui-protocol/ag-ui` commit `2330368a`), and AG-UI's handler holds
    /// every closed list to "a non-empty array" with a root
    /// (`a2ui-toolkit/src/validate.ts:112-117`, called at
    /// `a2ui-middleware/src/index.ts:985-997`), so neither a list left out nor
    /// an empty one is a call that tool accepts. AG-UI's middleware never
    /// makes this call for an update itself: there an update is the result of
    /// whatever tool the agent ran (`index.ts:1176-1309` reads the operations
    /// out of any tool result). An agent that has such a call sends the batch
    /// with [`SurfacePainter::settle`] under it. `paint` is for an agent that
    /// has none, and the call it makes is one the tool's own schema accepts.
    ///
    /// A surface that holds no component, because the batch deleted it or has
    /// not drawn it yet, has none to send, and the call then carries
    /// `surfaceId` alone.
    ///
    /// ```
    /// use ag_ui::server::RunContext;
    /// use ag_ui::{Event, RunAgentInput};
    /// use ag_ui_a2ui::adapters::agui::{Intent, SurfacePainter, SurfaceSpec, assemble_ops};
    /// use ag_ui_a2ui::{AgentMessage, Component};
    /// use serde_json::{Value, json};
    ///
    /// let (mut ctx, mut events) = RunContext::<()>::new(RunAgentInput::new("thread", "run"))?;
    /// let spec = SurfaceSpec::new("cart")
    ///     .with_components(vec![Component::new("root", "Text").with("text", json!({"path": "/total"}))])
    ///     .with_data_model(json!({"total": "12.00"}));
    /// let mut painter = SurfacePainter::new();
    /// painter.paint(&mut ctx, &assemble_ops(Intent::Create, &spec))?;
    /// events.drain();
    ///
    /// // A change to the data alone.
    /// painter.paint(&mut ctx, &[AgentMessage::update_data_model("cart", "/total", json!("15.00"))])?;
    /// let sent = events.drain();
    /// let Event::ToolCallArgs(arguments) = &sent[1] else { unreachable!() };
    /// let arguments: Value = serde_json::from_str(&arguments.delta)?;
    /// assert_eq!(arguments["surfaceId"], "cart");
    /// assert_eq!(arguments["components"][0]["id"], "root");
    /// let Event::ToolCallResult(result) = &sent[3] else { unreachable!() };
    /// assert!(result.content.to_text().contains(r#""path":"/total""#));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an agent error when the operations do not apply to what the
    /// frontend holds, before anything is sent, and what the run returns when
    /// an event cannot be emitted.
    pub fn paint<S: ag_ui::server::AgentState>(
        &mut self,
        ctx: &mut ag_ui::server::RunContext<S>,
        operations: &[AgentMessage],
    ) -> ag_ui::server::Result<ag_ui::ToolCallId> {
        let id = ctx.new_tool_call_id();
        let applied = self
            .apply(operations)
            .map_err(ag_ui::server::Error::agent)?;
        let arguments = render_arguments(operations, &applied);
        let painted = self
            .commit(id.as_str(), operations, applied)
            .map_err(ag_ui::server::Error::agent)?;
        let mut call =
            ctx.tool_call_with_id(id.clone(), crate::constants::RENDER_A2UI_TOOL_NAME)?;
        call.args_json(&arguments)?;
        call.result(painted.envelope)?;
        for activity in painted.activities {
            ctx.emit(activity)?;
        }
        Ok(id)
    }
}

/// The arguments of the `render_a2ui` call a batch is sent as: the first
/// surface its changes name, with every component that surface holds once
/// the batch is applied, and its root data when the batch writes it. A batch
/// that changed nothing names the first surface of its `operations`.
///
/// See [`SurfacePainter::paint`] for why the components are the surface's and
/// not the batch's.
#[cfg(feature = "ag-ui-server")]
fn render_arguments(operations: &[AgentMessage], applied: &Applied<'_>) -> Value {
    use crate::protocol::message::{AgentPayload, DataModelUpdate};
    use crate::protocol::version::A2uiVersion;

    let surface_id = applied
        .changes
        .iter()
        .find_map(|change| change.surface_id())
        .or_else(|| operations.iter().find_map(AgentMessage::surface_id));
    let mut arguments = JsonObject::new();
    arguments.insert("surfaceId".into(), json!(surface_id.unwrap_or_default()));
    let held = surface_id
        .and_then(|surface_id| applied.surfaces.get(surface_id))
        .filter(|surface| !surface.deleted && !surface.components.is_empty());
    if let Some(surface) = held {
        arguments.insert("components".into(), json!(surface.components));
    }
    for change in &applied.changes {
        if change.surface_id() != surface_id {
            continue;
        }
        // What a change writes is read as its version writes it.
        let version = A2uiVersion::parse(&change.version);
        match &change.payload {
            AgentPayload::UpdateDataModel(update) if matches!(update.path.as_str(), "" | "/") => {
                let null_removes = version.is_some_and(A2uiVersion::null_removes);
                match &update.value {
                    DataModelUpdate::Set(value) if !(null_removes && value.is_null()) => {
                        arguments.insert("data".into(), value.clone())
                    }
                    // A later removal of the root takes the data back.
                    _ => arguments.remove("data"),
                };
            }
            // A surface made again starts with the data its creation carries,
            // which a creation may from v1.0, and with none otherwise.
            AgentPayload::CreateSurface(create) => {
                arguments.remove("data");
                if let Some(data) = create
                    .data_model
                    .as_ref()
                    .filter(|_| version.is_some_and(A2uiVersion::creates_inline))
                {
                    arguments.insert("data".into(), Value::Object(data.clone()));
                }
            }
            AgentPayload::DeleteSurface(_) => {
                arguments.remove("data");
            }
            _ => {}
        }
    }
    Value::Object(arguments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::agui::{Intent, SurfaceSpec, assemble_ops};
    use crate::protocol::message::Component;
    use crate::protocol::validate::ErrorCode;

    fn spec(surface: &str) -> SurfaceSpec {
        SurfaceSpec::new(surface)
            .with_components(vec![
                Component::new("root", "Text").with("text", json!({"path": "/title"})),
            ])
            .with_data_model(json!({"title": surface}))
    }

    fn snapshot(event: &Event) -> (&str, &JsonObject) {
        let Event::ActivitySnapshot(snapshot) = event else {
            panic!("expected an activity snapshot, got {event:?}");
        };
        assert_eq!(snapshot.activity_type, SURFACE_ACTIVITY_TYPE);
        assert!(
            snapshot.replace,
            "a surface activity replaces what was there"
        );
        (snapshot.message_id.as_str(), &snapshot.content)
    }

    #[test]
    fn a_paint_is_the_batch_as_a_result_and_the_whole_surface_as_an_activity() {
        let mut painter = SurfacePainter::new();
        let operations = assemble_ops(Intent::Create, &spec("cart"));
        let painted = painter.settle("call-1", &operations).unwrap();

        assert_eq!(
            painted.envelope,
            wrap_as_operations_envelope(&operations).unwrap()
        );
        assert_eq!(painted.activities.len(), 1);
        let (id, content) = snapshot(&painted.activities[0]);
        assert_eq!(id, "a2ui-surface-call-1");
        assert_eq!(
            content.keys().collect::<Vec<_>>(),
            ["a2ui_operations"],
            "nothing rides in the envelope but the operations"
        );
        assert_eq!(content["a2ui_operations"], json!(operations));
    }

    #[test]
    fn an_update_is_painted_whole_and_in_place_within_a_run() {
        let mut painter = SurfacePainter::new();
        painter
            .settle("call-1", &assemble_ops(Intent::Create, &spec("cart")))
            .unwrap();
        let change = [AgentMessage::update_data_model(
            "cart",
            "/title",
            json!("Your cart"),
        )];
        let painted = painter.settle("call-2", &change).unwrap();

        // The model's record is the change.
        assert_eq!(
            painted.envelope,
            wrap_as_operations_envelope(&change).unwrap()
        );
        // The frontend's is the surface, under the activity that painted it.
        let (id, content) = snapshot(&painted.activities[0]);
        assert_eq!(id, "a2ui-surface-call-1");
        let replayed: Vec<AgentMessage> =
            serde_json::from_value(content["a2ui_operations"].clone()).unwrap();
        let mut fresh = SurfaceStore::new();
        for operation in &replayed {
            fresh.apply(operation).unwrap();
        }
        assert_eq!(&fresh, painter.surfaces());
    }

    #[test]
    fn a_later_run_paints_an_update_as_a_second_activity_with_the_whole_surface() {
        let mut first = SurfacePainter::new();
        first
            .settle("call-1", &assemble_ops(Intent::Create, &spec("cart")))
            .unwrap();

        // The next run knows the surfaces, not the activities.
        let mut second = SurfacePainter::with_surfaces(first.surfaces().clone());
        let painted = second
            .settle(
                "call-2",
                &[AgentMessage::update_data_model(
                    "cart",
                    "/title",
                    json!("x"),
                )],
            )
            .unwrap();
        let (id, content) = snapshot(&painted.activities[0]);
        assert_eq!(id, "a2ui-surface-call-2");
        let replayed = content["a2ui_operations"].as_array().unwrap();
        assert!(replayed[0].get("createSurface").is_some());
        assert_eq!(replayed.len(), 3);
    }

    #[test]
    fn several_surfaces_in_one_batch_get_an_activity_each() {
        let mut painter = SurfacePainter::new();
        let mut operations = assemble_ops(Intent::Create, &spec("a"));
        operations.extend(assemble_ops(Intent::Create, &spec("b")));
        let painted = painter.settle("call-1", &operations).unwrap();
        let ids: Vec<&str> = painted
            .activities
            .iter()
            .map(|event| snapshot(event).0)
            .collect();
        assert_eq!(ids, ["a2ui-surface-a-call-1", "a2ui-surface-b-call-1"]);

        // A new surface under a call whose own id another surface took.
        let mut painter = SurfacePainter::new();
        painter
            .settle("call-1", &assemble_ops(Intent::Create, &spec("a")))
            .unwrap();
        let painted = painter
            .settle("call-1", &assemble_ops(Intent::Create, &spec("b")))
            .unwrap();
        assert_eq!(snapshot(&painted.activities[0]).0, "a2ui-surface-b-call-1");
    }

    #[test]
    fn a_batch_that_does_not_apply_paints_nothing_and_changes_nothing() {
        let mut painter = SurfacePainter::new();
        let before = painter.clone();
        let orphan = [AgentMessage::update_data_model(
            "cart",
            "/title",
            json!("x"),
        )];
        assert!(painter.settle("call-1", &orphan).is_err());
        assert_eq!(painter, before);
    }

    #[test]
    fn a_deleted_surface_is_painted_as_created_and_then_deleted() {
        let mut painter = SurfacePainter::new();
        painter
            .settle("call-1", &assemble_ops(Intent::Create, &spec("cart")))
            .unwrap();
        let painted = painter
            .settle("call-2", &[AgentMessage::delete_surface("cart")])
            .unwrap();
        let (id, content) = snapshot(&painted.activities[0]);
        assert_eq!(id, "a2ui-surface-call-1");
        let replayed = content["a2ui_operations"].as_array().unwrap();
        assert!(replayed.first().unwrap().get("createSurface").is_some());
        assert!(replayed.last().unwrap().get("deleteSurface").is_some());
    }

    #[test]
    fn the_lifecycle_rides_the_activity_the_paint_replaces() {
        let errors = vec![ValidationError::new(
            ErrorCode::NoRoot,
            "messages[1].updateComponents.components",
            "No component has id 'root'.",
        )];
        let mut painter = SurfacePainter::new();
        let (id, building) = {
            let event = painter.status("call-1", &SurfaceStatus::Building);
            let (id, content) = snapshot(&event);
            (id.to_string(), content.clone())
        };
        assert_eq!(id, "a2ui-surface-call-1");
        assert_eq!(Value::Object(building), json!({"status": "building"}));

        let retrying = painter.status(
            "call-1",
            &SurfaceStatus::Retrying {
                attempt: 2,
                max_attempts: 3,
                errors: errors.clone(),
            },
        );
        assert_eq!(
            Value::Object(snapshot(&retrying).1.clone()),
            json!({"status": "retrying", "attempt": 2, "maxAttempts": 3, "errors": [{
                "code": "no_root",
                "path": "messages[1].updateComponents.components",
                "message": "No component has id 'root'.",
            }]})
        );

        let failed = painter.status(
            "call-1",
            &SurfaceStatus::Failed {
                error: "Failed to generate valid A2UI after 1 attempt(s)".into(),
                attempts: vec![Attempt::failed(1, errors)],
                max_attempts: 1,
            },
        );
        let content = snapshot(&failed).1;
        assert_eq!(content["status"], "failed");
        assert_eq!(content["attempts"][0]["errors"][0]["code"], "no_root");
        assert_eq!(content["maxAttempts"], 1);

        // The paint that follows takes the same slot.
        let painted = painter
            .settle("call-1", &assemble_ops(Intent::Create, &spec("cart")))
            .unwrap();
        assert_eq!(snapshot(&painted.activities[0]).0, id);
    }

    /// What a frontend shows: its surface activities by message id, each
    /// replaced in place by the next event under that id.
    #[derive(Default)]
    struct Screen(BTreeMap<String, JsonObject>);

    impl Screen {
        fn show(&mut self, event: &Event) {
            let (id, content) = snapshot(event);
            self.0.insert(id.to_string(), content.clone());
        }

        fn paint(&mut self, painted: &Painted) {
            for activity in &painted.activities {
                self.show(activity);
            }
        }

        /// The activities that show a state and not a surface.
        fn skeletons(&self) -> Vec<&str> {
            self.0
                .iter()
                .filter(|(_, content)| content.contains_key("status"))
                .map(|(id, _)| id.as_str())
                .collect()
        }

        /// The activities that show `surface`, each with its title.
        fn showing(&self, surface: &str) -> Vec<(&str, Value)> {
            self.0
                .iter()
                .filter_map(|(id, content)| {
                    let operations = content.get("a2ui_operations")?.as_array()?;
                    let named = |operation: &Value| {
                        operation
                            .as_object()
                            .into_iter()
                            .flat_map(|fields| fields.values())
                            .any(|body| body["surfaceId"] == surface)
                    };
                    let data = operations
                        .iter()
                        .filter(|operation| named(operation))
                        .find_map(|operation| operation.pointer("/updateDataModel/value/title"))?;
                    Some((id.as_str(), data.clone()))
                })
                .collect()
        }
    }

    /// An update of a surface another call painted would go in that call's
    /// activity. When the updating call showed a skeleton first, nothing would
    /// ever replace it.
    #[test]
    fn a_call_that_showed_a_skeleton_paints_an_update_in_its_own_activity() {
        let mut painter = SurfacePainter::new();
        let mut screen = Screen::default();

        screen.show(&painter.status("call-1", &SurfaceStatus::Building));
        screen.paint(
            &painter
                .settle("call-1", &assemble_ops(Intent::Create, &spec("cart")))
                .unwrap(),
        );
        assert!(screen.skeletons().is_empty());

        screen.show(&painter.status("call-2", &SurfaceStatus::Building));
        assert_eq!(screen.skeletons(), ["a2ui-surface-call-2"]);
        let change = [AgentMessage::update_data_model(
            "cart",
            "/title",
            json!("Your cart"),
        )];
        let painted = painter.settle("call-2", &change).unwrap();
        assert_eq!(snapshot(&painted.activities[0]).0, "a2ui-surface-call-2");
        screen.paint(&painted);
        assert!(screen.skeletons().is_empty(), "{:?}", screen.skeletons());
        // The surface is whole in its new place; the first activity keeps
        // what it showed, as it does when a later run updates a surface.
        assert_eq!(
            screen.showing("cart"),
            [
                ("a2ui-surface-call-1", json!("cart")),
                ("a2ui-surface-call-2", json!("Your cart")),
            ]
        );

        // The surface has moved: a call that shows no skeleton updates it
        // where it now is.
        let painted = painter
            .settle(
                "call-3",
                &[AgentMessage::update_data_model(
                    "cart",
                    "/title",
                    json!("Paid"),
                )],
            )
            .unwrap();
        assert_eq!(snapshot(&painted.activities[0]).0, "a2ui-surface-call-2");
    }

    /// A batch of several surfaces is painted under ids that name the surface,
    /// which is none of them the id the call's skeleton is under.
    #[test]
    fn the_first_surface_of_a_batch_takes_the_place_of_the_calls_skeleton() {
        let mut both = assemble_ops(Intent::Create, &spec("cart"));
        both.extend(assemble_ops(Intent::Create, &spec("receipt")));

        let mut painter = SurfacePainter::new();
        let mut screen = Screen::default();
        screen.show(&painter.status("call-1", &SurfaceStatus::Building));
        let painted = painter.settle("call-1", &both).unwrap();
        let ids: Vec<&str> = painted
            .activities
            .iter()
            .map(|event| snapshot(event).0)
            .collect();
        assert_eq!(ids, ["a2ui-surface-call-1", "a2ui-surface-receipt-call-1"]);
        screen.paint(&painted);
        assert!(screen.skeletons().is_empty());

        // Of a surface painted before and a new one, the new one takes the
        // skeleton's place and the other stays where it is.
        let mut painter = SurfacePainter::new();
        let mut screen = Screen::default();
        screen.paint(
            &painter
                .settle("call-1", &assemble_ops(Intent::Create, &spec("cart")))
                .unwrap(),
        );
        screen.show(&painter.status(
            "call-2",
            &SurfaceStatus::Retrying {
                attempt: 2,
                max_attempts: 3,
                errors: Vec::new(),
            },
        ));
        let mut batch = vec![AgentMessage::update_data_model(
            "cart",
            "/title",
            json!("Your cart"),
        )];
        batch.extend(assemble_ops(Intent::Create, &spec("receipt")));
        let painted = painter.settle("call-2", &batch).unwrap();
        let ids: Vec<&str> = painted
            .activities
            .iter()
            .map(|event| snapshot(event).0)
            .collect();
        assert_eq!(ids, ["a2ui-surface-call-1", "a2ui-surface-call-2"]);
        screen.paint(&painted);
        assert!(screen.skeletons().is_empty());
        assert_eq!(
            screen.showing("cart"),
            [("a2ui-surface-call-1", json!("Your cart"))]
        );
    }

    /// A state shown where a surface was painted takes the surface's place, so
    /// the painter no longer counts the surface as painted there.
    #[test]
    fn a_state_shown_over_a_painted_surface_is_replaced_by_the_next_paint_of_the_call() {
        let mut painter = SurfacePainter::new();
        let mut screen = Screen::default();
        screen.paint(
            &painter
                .settle("call-1", &assemble_ops(Intent::Create, &spec("cart")))
                .unwrap(),
        );
        screen.show(&painter.status("call-1", &SurfaceStatus::Building));
        assert_eq!(screen.skeletons(), ["a2ui-surface-call-1"]);

        screen.paint(
            &painter
                .settle("call-1", &assemble_ops(Intent::Create, &spec("receipt")))
                .unwrap(),
        );
        assert!(screen.skeletons().is_empty());
        assert_eq!(
            screen.showing("receipt"),
            [("a2ui-surface-call-1", json!("receipt"))]
        );

        // The cart is not painted over the receipt.
        let painted = painter
            .settle(
                "call-2",
                &[AgentMessage::update_data_model(
                    "cart",
                    "/title",
                    json!("x"),
                )],
            )
            .unwrap();
        assert_eq!(snapshot(&painted.activities[0]).0, "a2ui-surface-call-2");
    }

    /// A state stays until a surface is painted in its place, and a call that
    /// never showed one is painted as before.
    #[test]
    fn a_skeleton_is_one_calls_and_stays_until_that_call_paints() {
        let mut painter = SurfacePainter::new();
        let mut screen = Screen::default();
        screen.show(&painter.status("call-1", &SurfaceStatus::Building));

        // Another call's paint leaves it.
        screen.paint(
            &painter
                .settle("call-2", &assemble_ops(Intent::Create, &spec("cart")))
                .unwrap(),
        );
        assert_eq!(screen.skeletons(), ["a2ui-surface-call-1"]);
        // So does a batch of its own call that changes no surface.
        let painted = painter.settle("call-1", &[]).unwrap();
        assert!(painted.activities.is_empty());
        assert_eq!(screen.skeletons(), ["a2ui-surface-call-1"]);

        screen.paint(
            &painter
                .settle("call-1", &assemble_ops(Intent::Create, &spec("receipt")))
                .unwrap(),
        );
        assert!(screen.skeletons().is_empty());
    }

    /// What a client holds says which calls show a state. A server's request
    /// holds no activity, so a painter made from it knows of none.
    #[test]
    fn a_painter_made_from_a_conversation_notes_the_states_its_activities_show() {
        let activity = |id: &str, content: Value| {
            let Value::Object(content) = content else {
                panic!("an activity's content is an object");
            };
            Message::Activity(ag_ui::ActivityMessage {
                id: id.into(),
                activity_type: SURFACE_ACTIVITY_TYPE.into(),
                content,
                ..ag_ui::ActivityMessage::default()
            })
        };
        let created = SurfacePainter::new()
            .settle("call-1", &assemble_ops(Intent::Create, &spec("cart")))
            .unwrap();
        let Event::ActivitySnapshot(cart) = &created.activities[0] else {
            panic!("a paint is an activity snapshot");
        };
        let client = [
            Message::tool("m-1", "call-1", created.envelope.clone()),
            activity("a2ui-surface-call-1", Value::Object(cart.content.clone())),
            activity("a2ui-surface-call-2", json!({"status": "building"})),
            activity("some-other-activity", json!({"status": "building"})),
        ];
        let mut batch = vec![AgentMessage::update_data_model(
            "cart",
            "/title",
            json!("x"),
        )];
        batch.extend(assemble_ops(Intent::Create, &spec("receipt")));
        let ids = |painted: &Painted| -> Vec<String> {
            painted
                .activities
                .iter()
                .map(|event| snapshot(event).0.to_string())
                .collect()
        };

        // The call whose state is on screen paints in it.
        let mut painter = SurfacePainter::from_history(&client).unwrap();
        let painted = painter.settle("call-2", &batch).unwrap();
        assert_eq!(
            ids(&painted),
            ["a2ui-surface-call-2", "a2ui-surface-receipt-call-2"]
        );

        // The same conversation as a server is sent it holds no activity.
        let server = &client[..1];
        let mut painter = SurfacePainter::from_history(server).unwrap();
        assert_eq!(
            painter,
            SurfacePainter::with_surfaces(painter.surfaces().clone())
        );
        let painted = painter.settle("call-2", &batch).unwrap();
        assert_eq!(
            ids(&painted),
            ["a2ui-surface-cart-call-2", "a2ui-surface-receipt-call-2"]
        );
    }

    /// The processor accepts a delete of a surface that is not live and does
    /// nothing, and the agent layer returns it among an answer's messages. The
    /// painter follows: the batch is not refused for it.
    #[test]
    fn a_delete_of_a_surface_that_is_not_live_is_passed_over() {
        let mut batch = vec![AgentMessage::delete_surface("old")];
        batch.extend(assemble_ops(Intent::Create, &spec("s")));
        let mut painter = SurfacePainter::new();
        let painted = painter.settle("call-1", &batch).unwrap();

        // The result is the batch as sent.
        assert_eq!(
            painted.envelope,
            wrap_as_operations_envelope(&batch).unwrap()
        );
        // The one surface the batch changed is painted, under the call's own
        // activity: the delete's surface is not a second surface of the batch.
        let ids: Vec<&str> = painted
            .activities
            .iter()
            .map(|event| snapshot(event).0)
            .collect();
        assert_eq!(ids, ["a2ui-surface-call-1"]);
        assert!(painter.surfaces().get("old").is_none());
        assert!(painter.surfaces().get("s").is_some());

        // A surface that was deleted is not live either.
        painter
            .settle("call-2", &[AgentMessage::delete_surface("s")])
            .unwrap();
        let before = painter.clone();
        let again = painter
            .settle("call-3", &[AgentMessage::delete_surface("s")])
            .unwrap();
        assert!(again.activities.is_empty());
        assert_eq!(painter, before);

        // Anything else that names a surface that is not live is still refused.
        let orphan = [
            AgentMessage::delete_surface("old"),
            AgentMessage::update_data_model("old", "/title", json!("x")),
        ];
        assert!(painter.settle("call-4", &orphan).is_err());
        assert_eq!(painter, before);
    }

    /// The delete that is passed over may be all a batch holds.
    #[test]
    fn a_batch_of_one_delete_that_is_passed_over_is_a_result_and_no_activity() {
        let lone = [AgentMessage::delete_surface("old")];
        let mut painter = SurfacePainter::new();
        let before = painter.clone();
        let painted = painter.settle("call-1", &lone).unwrap();

        // The result is the batch as sent, and it is all there is to send.
        assert_eq!(
            serde_json::from_str::<Value>(&painted.envelope).unwrap(),
            json!({"a2ui_operations": [
                {"version": "v0.9", "deleteSurface": {"surfaceId": "old"}}
            ]})
        );
        assert!(painted.activities.is_empty(), "{:?}", painted.activities);
        assert_eq!(painter, before);

        // A skeleton the call showed stays: nothing was painted.
        let mut screen = Screen::default();
        screen.show(&painter.status("call-2", &SurfaceStatus::Building));
        screen.paint(&painter.settle("call-2", &lone).unwrap());
        assert_eq!(screen.skeletons(), ["a2ui-surface-call-2"]);
    }
}

#[cfg(all(test, feature = "ag-ui-server"))]
mod server_tests {
    use super::*;
    use crate::adapters::agui::{Intent, SurfaceSpec, assemble_ops};
    use crate::protocol::message::Component;
    use ag_ui::RunAgentInput;
    use ag_ui::server::RunContext;

    fn kinds(events: &[Event]) -> Vec<&str> {
        events
            .iter()
            .map(|event| event.event_type().as_str())
            .collect()
    }

    /// `paint` is `settle` in a run: the delete rides in the result, and the
    /// call's arguments name the surface the batch drew.
    #[test]
    fn a_delete_that_is_passed_over_is_painted_as_the_rest_of_its_batch() {
        let spec = SurfaceSpec::new("s").with_components(vec![
            Component::new("root", "Text").with("text", json!("Hi")),
        ]);
        let mut batch = vec![AgentMessage::delete_surface("old")];
        batch.extend(assemble_ops(Intent::Create, &spec));

        let (mut ctx, mut events) =
            RunContext::<()>::new(RunAgentInput::new("thread", "run")).unwrap();
        let mut painter = SurfacePainter::new();
        painter.paint(&mut ctx, &batch).unwrap();
        let sent = events.drain();
        assert_eq!(
            kinds(&sent),
            [
                "TOOL_CALL_START",
                "TOOL_CALL_ARGS",
                "TOOL_CALL_END",
                "TOOL_CALL_RESULT",
                "ACTIVITY_SNAPSHOT"
            ]
        );
        let Event::ToolCallArgs(arguments) = &sent[1] else {
            panic!("the second event carries the arguments");
        };
        let arguments: Value = serde_json::from_str(&arguments.delta).unwrap();
        assert_eq!(arguments["surfaceId"], "s");
        assert_eq!(arguments["components"][0]["id"], "root");
        let Event::ToolCallResult(result) = &sent[3] else {
            panic!("the fourth event is the result");
        };
        assert_eq!(
            result.content.to_text(),
            wrap_as_operations_envelope(&batch).unwrap()
        );

        // Alone, it is a call with a result and no activity.
        painter
            .paint(&mut ctx, &[AgentMessage::delete_surface("old")])
            .unwrap();
        let sent = events.drain();
        assert_eq!(
            kinds(&sent),
            [
                "TOOL_CALL_START",
                "TOOL_CALL_ARGS",
                "TOOL_CALL_END",
                "TOOL_CALL_RESULT"
            ]
        );
        let Event::ToolCallArgs(arguments) = &sent[1] else {
            panic!("the second event carries the arguments");
        };
        assert_eq!(
            serde_json::from_str::<Value>(&arguments.delta).unwrap(),
            json!({"surfaceId": "old"})
        );
    }

    /// The call `paint` makes is one `render_a2ui` accepts: `components` is
    /// required, and a list that is empty or has no root is refused upstream.
    #[test]
    fn the_call_carries_the_surface_whole_whatever_the_batch_changed() {
        let (mut ctx, mut events) =
            RunContext::<()>::new(RunAgentInput::new("thread", "run")).unwrap();
        let mut painter = SurfacePainter::new();
        let spec = SurfaceSpec::new("s")
            .with_components(vec![
                Component::new("root", "Column").with("children", json!(["title", "total"])),
                Component::new("title", "Text").with("text", json!("Cart")),
                Component::new("total", "Text").with("text", json!({"path": "/total"})),
            ])
            .with_data_model(json!({"total": 1}));
        let mut arguments_of = |painter: &mut SurfacePainter, batch: &[AgentMessage]| -> Value {
            painter.paint(&mut ctx, batch).unwrap();
            let sent = events.drain();
            let Event::ToolCallArgs(arguments) = &sent[1] else {
                panic!("the second event carries the arguments");
            };
            let Event::ToolCallResult(result) = &sent[3] else {
                panic!("the fourth event is the result");
            };
            // The result is the batch, as sent.
            assert_eq!(
                result.content.to_text(),
                wrap_as_operations_envelope(batch).unwrap()
            );
            serde_json::from_str(&arguments.delta).unwrap()
        };
        let ids = |arguments: &Value| -> Vec<String> {
            arguments["components"]
                .as_array()
                .map(|components| {
                    components
                        .iter()
                        .map(|component| component["id"].as_str().unwrap().to_string())
                        .collect()
                })
                .unwrap_or_default()
        };

        let created = arguments_of(&mut painter, &assemble_ops(Intent::Create, &spec));
        assert_eq!(ids(&created), ["root", "title", "total"]);
        assert_eq!(created["data"], json!({"total": 1}));

        // Data alone: the components are the surface's, and no `data`, since
        // the batch does not write the root.
        let data_only = arguments_of(
            &mut painter,
            &[AgentMessage::update_data_model("s", "/total", json!(2))],
        );
        assert_eq!(ids(&data_only), ["root", "title", "total"]);
        assert_eq!(data_only.get("data"), None);
        assert_eq!(
            data_only.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["components", "surfaceId"]
        );

        // One component of three: still the whole list, with the change in it.
        let one = arguments_of(
            &mut painter,
            &[AgentMessage::update_components(
                "s",
                vec![Component::new("title", "Text").with("text", json!("Basket"))],
            )],
        );
        assert_eq!(ids(&one), ["root", "title", "total"]);
        assert_eq!(one["components"][1]["text"], "Basket");

        // A surface with nothing drawn, and one that is gone, have nothing
        // to send.
        let empty = arguments_of(&mut painter, &[AgentMessage::create_surface("t", "c")]);
        assert_eq!(empty, json!({"surfaceId": "t"}));
        let deleted = arguments_of(&mut painter, &[AgentMessage::delete_surface("s")]);
        assert_eq!(deleted, json!({"surfaceId": "s"}));

        // A v1.0 creation that carries its surface: the call carries it too.
        let version = crate::A2uiVersion::V1_0;
        let create = crate::protocol::message::CreateSurface::new("v")
            .with_catalog_id(version.basic_catalog_id())
            .with_components(vec![
                Component::new("root", "Text").with("text", version.path("/total")),
            ])
            .with_data_model(json!({"total": 3}));
        let whole = arguments_of(
            &mut painter,
            &[
                AgentMessage::new(crate::protocol::message::AgentPayload::CreateSurface(
                    create,
                ))
                .with_version(version),
            ],
        );
        assert_eq!(ids(&whole), ["root"]);
        assert_eq!(whole["data"], json!({"total": 3}));
        // In v1.0 a `null` at the root removes, so there is no data to send.
        let emptied = arguments_of(
            &mut painter,
            &[AgentMessage::update_data_model("v", "/", Value::Null).with_version(version)],
        );
        assert_eq!(ids(&emptied), ["root"]);
        assert_eq!(emptied.get("data"), None);
        assert_eq!(
            painter
                .surfaces()
                .get("v")
                .unwrap()
                .data_model
                .to_json()
                .unwrap(),
            json!({})
        );
    }
}
