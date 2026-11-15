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

use std::collections::BTreeMap;

use ag_ui::{Event, JsonObject, Message, MessageId};
use serde_json::{Value, json};

use super::envelope::{Attempt, operations_envelope, wrap_as_operations_envelope};
use super::history::surfaces_in;
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
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// The event that puts a tool call's surface activity in `status`.
///
/// ```
/// use ag_ui::Event;
/// use ag_ui_a2ui::adapters::agui::{SurfaceStatus, surface_status};
///
/// let Event::ActivitySnapshot(building) = surface_status("call-1", &SurfaceStatus::Building)
/// else {
///     unreachable!()
/// };
/// assert_eq!(building.message_id.as_str(), "a2ui-surface-call-1");
/// assert_eq!(building.activity_type, "a2ui-surface");
/// assert_eq!(building.content["status"], "building");
/// assert!(building.replace);
/// ```
pub fn surface_status(tool_call_id: &str, status: &SurfaceStatus) -> Event {
    Event::activity_snapshot(
        surface_activity_id(tool_call_id),
        SURFACE_ACTIVITY_TYPE,
        status.content(),
    )
}

/// What painting a batch produces: the tool result, and the events after it.
#[derive(Debug, Clone, PartialEq)]
pub struct Painted {
    /// The content of the tool result: the batch, as an operations envelope.
    pub envelope: String,
    /// One `ACTIVITY_SNAPSHOT` for each surface the batch touched, to be sent
    /// after the tool result.
    pub activities: Vec<Event>,
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
}

impl SurfacePainter {
    /// A painter for a frontend that holds no surface.
    pub fn new() -> Self {
        Self::default()
    }

    /// A painter for a frontend that holds what `messages` drew: the messages
    /// of the request, on a server.
    ///
    /// # Errors
    ///
    /// As [`surfaces_in`].
    pub fn from_history(messages: &[Message]) -> Result<Self> {
        Ok(Self::with_surfaces(surfaces_in(messages)?))
    }

    /// A painter for a frontend that holds `surfaces`.
    pub fn with_surfaces(surfaces: SurfaceStore) -> Self {
        Self {
            surfaces,
            painted: BTreeMap::new(),
        }
    }

    /// What the frontend holds.
    pub fn surfaces(&self) -> &SurfaceStore {
        &self.surfaces
    }

    /// Applies `operations` and gives what to send for them, as the result of
    /// the tool call `tool_call_id`.
    ///
    /// A surface this painter has not painted before goes under
    /// `a2ui-surface-<tool call id>`, the id the call's
    /// [`surface_status`] used, or under
    /// `a2ui-surface-<surface id>-<tool call id>` when the batch touches
    /// several surfaces or that id is another surface's.
    ///
    /// # Errors
    ///
    /// Returns what [`SurfaceStore::apply`] returns when an operation does not
    /// apply to what the frontend holds. Nothing is changed then, and nothing
    /// is to be sent.
    pub fn settle(&mut self, tool_call_id: &str, operations: &[AgentMessage]) -> Result<Painted> {
        let mut next = self.surfaces.clone();
        let mut touched: Vec<&str> = Vec::new();
        for operation in operations {
            next.apply(operation)?;
            if let Some(surface_id) = operation.surface_id() {
                if !touched.contains(&surface_id) {
                    touched.push(surface_id);
                }
            }
        }
        let envelope = wrap_as_operations_envelope(operations)?;

        let own = surface_activity_id(tool_call_id);
        let mut painted = self.painted.clone();
        let mut activities = Vec::with_capacity(touched.len());
        for surface_id in &touched {
            let Some(surface) = next.get(surface_id) else {
                continue;
            };
            let message_id = match painted.get(*surface_id) {
                Some(owner) => owner.clone(),
                None if touched.len() == 1 && !painted.values().any(|id| *id == own) => own.clone(),
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
            painted.insert((*surface_id).to_string(), message_id);
        }
        self.surfaces = next;
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
        let painted = self
            .settle(id.as_str(), operations)
            .map_err(ag_ui::server::Error::agent)?;
        let mut call =
            ctx.tool_call_with_id(id.clone(), crate::constants::RENDER_A2UI_TOOL_NAME)?;
        call.args_json(&render_arguments(operations))?;
        call.result(painted.envelope)?;
        for activity in painted.activities {
            ctx.emit(activity)?;
        }
        Ok(id)
    }
}

/// The arguments a `render_a2ui` call would have had for `operations`: the
/// first surface they name, with its components and root data when the batch
/// carries them.
#[cfg(feature = "ag-ui-server")]
fn render_arguments(operations: &[AgentMessage]) -> Value {
    use crate::protocol::message::{AgentPayload, DataModelUpdate};

    let surface_id = operations.iter().find_map(AgentMessage::surface_id);
    let mut arguments = JsonObject::new();
    arguments.insert("surfaceId".into(), json!(surface_id.unwrap_or_default()));
    for operation in operations {
        if operation.surface_id() != surface_id {
            continue;
        }
        match &operation.payload {
            AgentPayload::UpdateComponents(update) => {
                arguments.insert("components".into(), json!(update.components));
            }
            AgentPayload::UpdateDataModel(update) if matches!(update.path.as_str(), "" | "/") => {
                if let DataModelUpdate::Set(value) = &update.value {
                    arguments.insert("data".into(), value.clone());
                }
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
        let (id, building) = {
            let event = surface_status("call-1", &SurfaceStatus::Building);
            let (id, content) = snapshot(&event);
            (id.to_string(), content.clone())
        };
        assert_eq!(id, "a2ui-surface-call-1");
        assert_eq!(Value::Object(building), json!({"status": "building"}));

        let retrying = surface_status(
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

        let failed = surface_status(
            "call-1",
            &SurfaceStatus::Failed {
                error: "Failed to generate valid A2UI after 1 attempt(s)".into(),
                attempts: vec![Attempt {
                    attempt: 1,
                    ok: false,
                    errors,
                }],
                max_attempts: 1,
            },
        );
        let content = snapshot(&failed).1;
        assert_eq!(content["status"], "failed");
        assert_eq!(content["attempts"][0]["errors"][0]["code"], "no_root");
        assert_eq!(content["maxAttempts"], 1);

        // The paint that follows takes the same slot.
        let painted = SurfacePainter::new()
            .settle("call-1", &assemble_ops(Intent::Create, &spec("cart")))
            .unwrap();
        assert_eq!(snapshot(&painted.activities[0]).0, id);
    }
}
