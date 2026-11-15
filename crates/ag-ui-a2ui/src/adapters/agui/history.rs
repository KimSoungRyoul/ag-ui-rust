//! Reading surfaces back out of an AG-UI conversation.
//!
//! An agent on AG-UI stores nothing between runs: what the user is looking at
//! is in the messages the client sends back. Two kinds of message carry a
//! surface, and nothing else is read:
//!
//! - a **tool result** whose content is an operations envelope: the batch one
//!   tool call produced, which for an update is the update alone;
//! - an **`a2ui-surface` activity** whose content is one: the whole surface,
//!   as it stood after the batch.
//!
//! A client's own message list has both. A server's does not: AG-UI clients
//! leave activity messages out of the next request, this crate's included, so
//! what a server is sent is the tool results. Both give the same surfaces,
//! because the tool results are the record: where a tool call's result
//! carries operations, the activity that call owns is not read. It says again
//! what the results say, and it is replaced in place while its run goes on, so
//! it can be ahead of the results that follow it in the list.
//!
//! An activity is read when its call left no operations in a result. AG-UI's
//! middleware paints a `render_a2ui` call a model streamed that way: the
//! call's result is `{"status": "rendered"}`, and the surface is in the
//! activity alone.
//!
//! Text is not read. A reasoning message, an assistant message or a user
//! message that quotes an envelope is talk about a surface, and replaying it
//! would draw one nobody sent.

use std::collections::BTreeSet;

use ag_ui::Message;
use serde_json::Value;

use super::activity::SURFACE_ACTIVITY_TYPE;
use super::envelope::{is_operations_envelope, unwrap_operations_envelope};
use crate::error::Result;
use crate::protocol::message::{AgentMessage, AgentPayload};
use crate::protocol::surface::{SurfaceState, SurfaceStore};

/// What a message carries, by where it was found.
enum Carried {
    /// A tool result: what one call changed.
    Batch(Vec<AgentMessage>),
    /// A surface activity: what the surfaces it names are.
    Snapshot(Vec<AgentMessage>),
}

fn carried(message: &Message) -> Result<Option<Carried>> {
    match message {
        Message::Tool(tool) => {
            let Ok(value) = serde_json::from_str::<Value>(&tool.content.to_text()) else {
                return Ok(None);
            };
            if !is_operations_envelope(&value) {
                return Ok(None);
            }
            Ok(Some(Carried::Batch(unwrap_operations_envelope(&value)?)))
        }
        Message::Activity(activity) if activity.activity_type == SURFACE_ACTIVITY_TYPE => {
            let value = Value::Object(activity.content.clone());
            // A surface that is still being built, or that failed, is the same
            // activity with a status in place of operations.
            if !is_operations_envelope(&value) {
                return Ok(None);
            }
            Ok(Some(Carried::Snapshot(unwrap_operations_envelope(&value)?)))
        }
        _ => Ok(None),
    }
}

/// The A2UI operations a message carries, or `None` when it carries none.
///
/// This is the test every AG-UI frontend makes of a message before it draws:
/// a tool result or an `a2ui-surface` activity whose content is an operations
/// envelope is a surface, and anything else is not.
///
/// ```
/// use ag_ui::Message;
/// use ag_ui_a2ui::AgentMessage;
/// use ag_ui_a2ui::adapters::agui::{operations_in, wrap_as_operations_envelope};
///
/// let envelope = wrap_as_operations_envelope(&[AgentMessage::create_surface("board", "basic")])?;
/// let result = Message::tool("m-1", "call-1", envelope.clone());
/// assert_eq!(operations_in(&result).unwrap().len(), 1);
///
/// // An ordinary result, and an assistant that only quotes an envelope.
/// assert!(operations_in(&Message::tool("m-2", "call-2", r#"{"id": 1}"#)).is_none());
/// assert!(operations_in(&Message::assistant("m-3", envelope)).is_none());
/// # Ok::<(), ag_ui_a2ui::Error>(())
/// ```
pub fn operations_in(message: &Message) -> Option<Vec<AgentMessage>> {
    match carried(message) {
        Ok(Some(Carried::Batch(operations) | Carried::Snapshot(operations))) => Some(operations),
        _ => None,
    }
}

/// The surfaces a conversation has drawn: the operations its messages carry,
/// folded in order.
///
/// A tool result is applied as the change it is. A surface activity is not
/// read when the tool call that owns it has such a result in `messages`, since
/// the results already say it; otherwise it is taken as the surface whole, in
/// place of whatever the messages before it left. So a client's list, which
/// has both, and a server's, which has the tool results alone, give the same
/// store, at any point of a run.
///
/// ```
/// use ag_ui::Message;
/// use ag_ui_a2ui::adapters::agui::{surfaces_in, wrap_as_operations_envelope};
/// use ag_ui_a2ui::{AgentMessage, Component};
/// use serde_json::json;
///
/// let created = wrap_as_operations_envelope(&[
///     AgentMessage::create_surface("board", "basic"),
///     AgentMessage::update_components(
///         "board",
///         vec![Component::new("root", "Text").with("text", json!({"path": "/title"}))],
///     ),
/// ])?;
/// let updated = wrap_as_operations_envelope(&[
///     AgentMessage::update_data_model("board", "/title", json!("Today")),
/// ])?;
/// let thread = [
///     Message::user("m-1", "show me the board"),
///     Message::tool("m-2", "call-1", created),
///     Message::user("m-3", "call it Today"),
///     Message::tool("m-4", "call-2", updated),
/// ];
///
/// let surfaces = surfaces_in(&thread)?;
/// let board = surfaces.get("board").expect("the thread drew a board");
/// assert_eq!(board.components.len(), 1);
/// assert_eq!(board.data_model.to_json()?, json!({"title": "Today"}));
/// # Ok::<(), ag_ui_a2ui::Error>(())
/// ```
///
/// # Errors
///
/// Returns [`Error::Json`](crate::Error::Json) when an envelope's operations
/// do not decode, and what [`SurfaceStore::apply`] returns when one does not
/// apply: a conversation that cannot be replayed is not half replayed.
pub fn surfaces_in(messages: &[Message]) -> Result<SurfaceStore> {
    let mut read = Vec::with_capacity(messages.len());
    // The tool calls whose results carry operations.
    let mut recorded = BTreeSet::new();
    for message in messages {
        let carried = carried(message)?;
        if let (Message::Tool(tool), Some(Carried::Batch(_))) = (message, &carried) {
            recorded.insert(tool.tool_call_id.as_str());
        }
        read.push(carried);
    }

    let mut store = SurfaceStore::new();
    for (message, carried) in messages.iter().zip(read) {
        let operations = match carried {
            None => continue,
            Some(Carried::Batch(operations)) => operations,
            Some(Carried::Snapshot(operations)) => {
                if repeats_a_result(message, &operations, &recorded) {
                    continue;
                }
                for operation in &operations {
                    if let AgentPayload::CreateSurface(create) = &operation.payload {
                        store.forget(&create.surface_id);
                    }
                }
                operations
            }
        };
        for operation in &operations {
            store.apply(operation)?;
        }
    }
    Ok(store)
}

/// Whether a surface activity belongs to a tool call whose result is in the
/// conversation: its id is `a2ui-surface-<tool call id>`, or
/// `a2ui-surface-<surface id>-<tool call id>` for one of its surfaces.
fn repeats_a_result(
    activity: &Message,
    operations: &[AgentMessage],
    recorded: &BTreeSet<&str>,
) -> bool {
    let id = activity.id().as_str();
    let Some(owner) = id
        .strip_prefix(SURFACE_ACTIVITY_TYPE)
        .and_then(|rest| rest.strip_prefix('-'))
    else {
        return false;
    };
    recorded.contains(owner)
        || operations
            .iter()
            .filter_map(AgentMessage::surface_id)
            .filter_map(|surface| owner.strip_prefix(surface)?.strip_prefix('-'))
            .any(|call| recorded.contains(call))
}

/// The surface a conversation touched last: what the user is looking at.
///
/// It is the surface the last operation in `messages` names, as every message
/// before and including it left it. A surface that was deleted comes back with
/// `deleted` set, since "it was there and is gone" is not "there was none".
/// `None` when the conversation drew no surface.
///
/// ```
/// use ag_ui::Message;
/// use ag_ui_a2ui::AgentMessage;
/// use ag_ui_a2ui::adapters::agui::{latest_surface, wrap_as_operations_envelope};
///
/// let drawn = |surface: &str| {
///     wrap_as_operations_envelope(&[AgentMessage::create_surface(surface, "basic")]).unwrap()
/// };
/// let thread = [
///     Message::tool("m-1", "call-1", drawn("cart")),
///     Message::tool("m-2", "call-2", drawn("receipt")),
/// ];
/// assert_eq!(latest_surface(&thread)?.unwrap().surface_id, "receipt");
/// assert!(latest_surface(&[Message::user("m-1", "hello")])?.is_none());
/// # Ok::<(), ag_ui_a2ui::Error>(())
/// ```
///
/// # Errors
///
/// As [`surfaces_in`].
pub fn latest_surface(messages: &[Message]) -> Result<Option<SurfaceState>> {
    let mut latest = None;
    for message in messages {
        if let Some(Carried::Batch(operations) | Carried::Snapshot(operations)) = carried(message)?
        {
            if let Some(surface_id) = operations.iter().rev().find_map(AgentMessage::surface_id) {
                latest = Some(surface_id.to_string());
            }
        }
    }
    let Some(latest) = latest else {
        return Ok(None);
    };
    Ok(surfaces_in(messages)?.get(&latest).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::agui::{
        Intent, SurfacePainter, SurfaceSpec, assemble_ops, error_envelope,
        wrap_as_operations_envelope,
    };
    use crate::protocol::message::Component;
    use ag_ui::{ActivityMessage, Event};
    use serde_json::json;

    fn spec(surface: &str, title: &str) -> SurfaceSpec {
        SurfaceSpec::new(surface)
            .with_components(vec![
                Component::new("root", "Column").with("children", json!(["label"])),
                Component::new("label", "Text").with("text", json!({"path": "/title"})),
            ])
            .with_data_model(json!({"title": title}))
    }

    fn tool(id: &str, operations: &[AgentMessage]) -> Message {
        Message::tool(
            format!("m-{id}"),
            format!("call-{id}"),
            wrap_as_operations_envelope(operations).unwrap(),
        )
    }

    /// The messages a client holds after a painter's events: the tool result,
    /// then each activity.
    fn painted(
        painter: &mut SurfacePainter,
        id: &str,
        operations: &[AgentMessage],
    ) -> Vec<Message> {
        let call = format!("call-{id}");
        let painted = painter.settle(&call, operations).unwrap();
        let mut messages = vec![Message::tool(format!("m-{id}"), call, painted.envelope)];
        for event in painted.activities {
            let Event::ActivitySnapshot(snapshot) = event else {
                panic!("a painter emits activity snapshots");
            };
            messages.push(Message::Activity(ActivityMessage {
                id: snapshot.message_id,
                activity_type: snapshot.activity_type,
                content: snapshot.content,
                ..ActivityMessage::default()
            }));
        }
        messages
    }

    #[test]
    fn a_surface_is_read_from_a_tool_result_and_nothing_else() {
        let envelope =
            wrap_as_operations_envelope(&assemble_ops(Intent::Create, &spec("cart", "Cart")))
                .unwrap();
        let surfaces = surfaces_in(&[
            Message::user("m-1", "show me the cart"),
            Message::tool("m-2", "call-1", envelope.clone()),
        ])
        .unwrap();
        assert_eq!(surfaces.get("cart").unwrap().components.len(), 2);

        // The same envelope as talk is not a surface, whoever says it.
        for quoted in [
            Message::user("m-1", envelope.clone()),
            Message::assistant("m-1", envelope.clone()),
            Message::system("m-1", envelope.clone()),
            Message::Reasoning(ag_ui::ReasoningMessage {
                id: "m-1".into(),
                content: envelope.clone(),
                ..ag_ui::ReasoningMessage::default()
            }),
        ] {
            assert!(operations_in(&quoted).is_none(), "{quoted:?}");
            assert!(surfaces_in(&[quoted]).unwrap().get("cart").is_none());
        }
    }

    #[test]
    fn a_failure_and_an_ordinary_result_draw_nothing() {
        for content in [
            error_envelope("could not build the surface"),
            r#"{"id": 1}"#.to_string(),
            "not JSON at all".to_string(),
        ] {
            let message = Message::tool("m-1", "call-1", content);
            assert!(operations_in(&message).is_none());
            assert_eq!(surfaces_in(&[message]).unwrap(), SurfaceStore::new());
        }
    }

    /// A server is sent the tool results alone; a client holds the activities
    /// too. Both are the same surfaces, across a create and two updates.
    #[test]
    fn a_servers_view_and_a_clients_view_are_the_same_surfaces() {
        let mut painter = SurfacePainter::new();
        let mut client: Vec<Message> = vec![Message::user("u-1", "show me the cart")];
        client.extend(painted(
            &mut painter,
            "1",
            &assemble_ops(Intent::Create, &spec("cart", "Cart")),
        ));
        // A later run starts from what the server was sent.
        let server: Vec<Message> = client
            .iter()
            .filter(|message| !matches!(message, Message::Activity(_)))
            .cloned()
            .collect();
        let mut painter = SurfacePainter::from_history(&server).unwrap();
        client.extend(painted(
            &mut painter,
            "2",
            &[AgentMessage::update_data_model(
                "cart",
                "/title",
                json!("Your cart"),
            )],
        ));
        client.extend(painted(
            &mut painter,
            "3",
            &[AgentMessage::update_components(
                "cart",
                vec![Component::new("label", "Text").with("text", json!("fixed"))],
            )],
        ));

        let server: Vec<Message> = client
            .iter()
            .filter(|message| !matches!(message, Message::Activity(_)))
            .cloned()
            .collect();
        assert_eq!(server.len(), 4, "a user message and three tool results");
        assert_eq!(client.len(), 7, "and an activity after each result");

        let seen_by_client = surfaces_in(&client).unwrap();
        assert_eq!(seen_by_client, surfaces_in(&server).unwrap());
        assert_eq!(&seen_by_client, painter.surfaces());
        let cart = seen_by_client.get("cart").unwrap();
        assert_eq!(
            cart.data_model.to_json().unwrap(),
            json!({"title": "Your cart"})
        );
        assert_eq!(cart.components[1].prop("text"), Some(&json!("fixed")));

        // The activities alone are enough too: each is a whole surface.
        let activities: Vec<Message> = client
            .iter()
            .filter(|message| matches!(message, Message::Activity(_)))
            .cloned()
            .collect();
        assert_eq!(surfaces_in(&activities).unwrap(), seen_by_client);
    }

    /// What a client holds while a run paints one surface several times: the
    /// tool results in order, and one activity, replaced in place, where the
    /// first paint put it.
    fn within_one_run(batches: &[Vec<AgentMessage>]) -> (SurfacePainter, Vec<Vec<Message>>) {
        let mut painter = SurfacePainter::new();
        let mut held: Vec<Message> = Vec::new();
        let mut seen = Vec::new();
        for (index, batch) in batches.iter().enumerate() {
            let mut arrived = painted(&mut painter, &index.to_string(), batch).into_iter();
            // The tool result arrives first, and a client may read then.
            held.push(arrived.next().unwrap());
            seen.push(held.clone());
            for activity in arrived {
                match held.iter_mut().find(|known| known.id() == activity.id()) {
                    Some(known) => *known = activity,
                    None => held.push(activity),
                }
            }
            seen.push(held.clone());
        }
        (painter, seen)
    }

    /// The activity a run replaces in place is ahead of the tool results that
    /// follow it. Reading it and then the results would apply each change
    /// twice, which is an error as soon as a change cannot be made twice.
    #[test]
    fn an_activity_replaced_in_place_is_not_read_ahead_of_its_results() {
        let created = assemble_ops(Intent::Create, &spec("cart", "Cart"));
        for later in [
            // A surface that is updated and then deleted.
            vec![
                vec![AgentMessage::update_data_model(
                    "cart",
                    "/title",
                    json!("x"),
                )],
                vec![AgentMessage::delete_surface("cart")],
            ],
            // A container that changes kind under a path written before.
            vec![
                vec![AgentMessage::update_data_model(
                    "cart",
                    "/note/text",
                    json!("x"),
                )],
                vec![AgentMessage::update_data_model(
                    "cart",
                    "/note",
                    json!([1, 2]),
                )],
            ],
        ] {
            let batches: Vec<Vec<AgentMessage>> =
                std::iter::once(created.clone()).chain(later).collect();
            let (painter, seen) = within_one_run(&batches);
            let last = seen.last().unwrap();
            assert_eq!(
                last.iter()
                    .filter(|message| matches!(message, Message::Activity(_)))
                    .count(),
                1,
                "one activity, replaced in place"
            );
            assert_eq!(&surfaces_in(last).unwrap(), painter.surfaces());

            // And at every point on the way, the client reads what a server
            // sent the same tool results would.
            for held in &seen {
                let server: Vec<Message> = held
                    .iter()
                    .filter(|message| !matches!(message, Message::Activity(_)))
                    .cloned()
                    .collect();
                assert_eq!(surfaces_in(held).unwrap(), surfaces_in(&server).unwrap());
            }
        }
    }

    /// AG-UI's middleware paints a streamed `render_a2ui` call as an activity
    /// and answers the call with `{"status": "rendered"}`: the activity is the
    /// only place the surface is. A later update that does come in a tool
    /// result applies to it.
    #[test]
    fn an_activity_whose_call_left_no_operations_is_the_surface() {
        let mut painter = SurfacePainter::new();
        let drawn = painted(
            &mut painter,
            "1",
            &assemble_ops(Intent::Create, &spec("cart", "Cart")),
        );
        let thread = vec![
            Message::tool("m-1", "call-1", r#"{"status": "rendered"}"#),
            drawn[1].clone(),
            Message::tool(
                "m-2",
                "call-2",
                wrap_as_operations_envelope(&[AgentMessage::update_data_model(
                    "cart",
                    "/title",
                    json!("Your cart"),
                )])
                .unwrap(),
            ),
        ];
        let surfaces = surfaces_in(&thread).unwrap();
        let cart = surfaces.get("cart").expect("the activity drew a cart");
        assert_eq!(cart.components.len(), 2);
        assert_eq!(
            cart.data_model.to_json().unwrap(),
            json!({"title": "Your cart"})
        );
        assert_eq!(latest_surface(&thread).unwrap().as_ref(), Some(cart));
    }

    /// Two surfaces in one batch are painted under ids that name the surface.
    #[test]
    fn the_activities_of_a_batch_of_two_surfaces_repeat_its_result() {
        let mut both = assemble_ops(Intent::Create, &spec("cart", "Cart"));
        both.extend(assemble_ops(Intent::Create, &spec("receipt", "Receipt")));
        let batches = vec![
            both,
            vec![AgentMessage::update_data_model(
                "receipt",
                "/title",
                json!("Paid"),
            )],
            vec![AgentMessage::delete_surface("receipt")],
        ];
        let (painter, seen) = within_one_run(&batches);
        let last = seen.last().unwrap();
        let ids: Vec<&str> = last
            .iter()
            .filter(|message| matches!(message, Message::Activity(_)))
            .map(|message| message.id().as_str())
            .collect();
        assert_eq!(
            ids,
            ["a2ui-surface-cart-call-0", "a2ui-surface-receipt-call-0"]
        );
        assert_eq!(&surfaces_in(last).unwrap(), painter.surfaces());
    }

    #[test]
    fn an_activity_that_is_still_building_is_not_a_surface() {
        let mut content = ag_ui::JsonObject::new();
        content.insert("status".into(), json!("building"));
        let building = Message::Activity(ActivityMessage {
            id: "a2ui-surface-call-1".into(),
            activity_type: SURFACE_ACTIVITY_TYPE.into(),
            content,
            ..ActivityMessage::default()
        });
        assert!(operations_in(&building).is_none());
        assert_eq!(surfaces_in(&[building]).unwrap(), SurfaceStore::new());
    }

    #[test]
    fn a_conversation_that_cannot_be_replayed_is_an_error() {
        // An update for a surface nothing created.
        let orphan = tool(
            "1",
            &[AgentMessage::update_data_model(
                "cart",
                "/title",
                json!("x"),
            )],
        );
        assert!(surfaces_in(&[orphan]).is_err());
        // An envelope whose operations are not operations.
        let malformed = Message::tool("m-1", "call-1", r#"{"a2ui_operations": [{"nope": 1}]}"#);
        assert!(operations_in(&malformed).is_none());
        assert!(surfaces_in(&[malformed]).is_err());
    }

    #[test]
    fn a_deleted_surface_stays_deleted() {
        let history = [
            tool("1", &assemble_ops(Intent::Create, &spec("cart", "Cart"))),
            tool("2", &[AgentMessage::delete_surface("cart")]),
        ];
        let surfaces = surfaces_in(&history).unwrap();
        assert!(surfaces.get("cart").is_some_and(|surface| surface.deleted));
    }
}
