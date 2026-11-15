//! Turning what the client assembled into lines a person reads.
//!
//! Every helper here names a [`Thread`] without bounding its transport. That
//! is not incidental: an application's view layer only ever *reads* a thread,
//! and a `T: Transport` bound on the type would force every one of these
//! signatures to repeat a constraint none of them use. `ag_ui::client` keeps the
//! bound on the impl blocks that make requests, and this module is what spends
//! that.

use ag_ui::Message;
use ag_ui::client::Thread;
use ag_ui_a2ui::adapters::agui::latest_surface;
use ag_ui_a2ui::binding::Scope;
use ag_ui_a2ui::{Catalog, Component, Placed, SurfaceState};
use serde_json::Value;

use crate::board::Board;

/// The run the thread last heard about, for the footer.
///
/// No `T: Transport` — this only reads.
pub fn run_id<T, S>(thread: &Thread<T, S>) -> String {
    thread
        .applier()
        .run_id()
        .map_or_else(|| "—".to_owned(), |id| id.as_str().to_owned())
}

/// How many messages the conversation holds.
pub fn message_count<T, S>(thread: &Thread<T, S>) -> usize {
    thread.messages().len()
}

/// The panel drawn after each run: the board, then where it came from.
///
/// Names `Thread<T, Board>` — a concrete state, still no transport bound.
pub fn panel<T>(thread: &Thread<T, Board>) -> Vec<String> {
    let mut lines = vec!["┌ board".to_owned()];

    match thread.state() {
        Ok(board) if !board.tasks.is_empty() => {
            lines.push(format!("│ {}", board.summary()));
            lines.extend(board.tasks.iter().map(|task| format!("│ {}", task.line())));
        }
        Ok(_) => lines.push("│ (empty)".to_owned()),
        Err(error) => lines.push(format!("│ (invalid board state: {error})")),
    }

    let surface = match surface_in_history(thread.messages()) {
        Ok(Some(prior)) => format!(
            " · surface {} ({})",
            prior.surface_id,
            prior.components.len()
        ),
        Ok(None) => String::new(),
        // Not "no surface": the conversation holds operations that do not
        // apply in order, and every later run will be sent them.
        Err(error) => format!(" · surfaces do not replay ({error})"),
    };
    lines.push(format!(
        "└ run {} · {} messages{surface}",
        run_id(thread),
        message_count(thread)
    ));
    lines
}

/// The surface the conversation is carrying, recovered from history.
///
/// The client's half of what the agent does to decide create-versus-update: the
/// A2UI operations are in the transcript, so anything holding the transcript can
/// replay them. No hand-written message mapping and no fold of the operations —
/// that is what the adapter is for.
///
/// `Ok(None)` is a conversation with no surface on screen.
///
/// # Errors
///
/// A conversation whose operations do not apply in order, a surface created
/// twice, say, does not replay. That is an agent's defect or the adapter's,
/// and it is returned, not folded into "no surface": a watcher that hid it
/// would show a second turn that silently lost its surface.
pub fn surface_in_history(messages: &[Message]) -> ag_ui_a2ui::Result<Option<SurfaceState>> {
    Ok(latest_surface(messages)?.filter(|surface| !surface.deleted))
}

/// The surface the conversation shows once `message` is part of it.
///
/// A tool result that updates a surface carries only what changed; what a
/// person sees is the surface it leaves.
///
/// # Errors
///
/// As [`surface_in_history`].
pub fn surface_after(
    messages: &[Message],
    message: &Message,
) -> ag_ui_a2ui::Result<Option<SurfaceState>> {
    let mut conversation = messages.to_vec();
    if !conversation.iter().any(|known| known.id() == message.id()) {
        conversation.push(message.clone());
    }
    surface_in_history(&conversation)
}

/// The most components [`surface_lines`] places before it stops.
///
/// An outline can be far larger than the surface it comes from: components
/// that share children are placed once for every route to them. The watcher
/// draws a surface an agent it did not write sent, so it takes what fits.
pub const DRAWN_COMPONENTS: usize = 500;

/// Draws a surface, or `None` if it has no components to draw.
///
/// A real renderer owns a widget toolkit; this asks the surface for its
/// outline — the components the root reaches, each in the scope its bindings
/// resolve in, the list template instantiated once per item — and prints the
/// ones that are text. That is far enough to prove the surface arrived whole.
/// The outline is walked as far as [`DRAWN_COMPONENTS`], and a last line says
/// so when the surface has more.
pub fn surface_lines(surface: &SurfaceState) -> Option<Vec<String>> {
    if surface.components.is_empty() {
        return None;
    }
    let data = surface.data_model.to_json().ok()?;
    let catalog = Catalog::basic();
    let mut outline = surface.outline(&catalog, &data);
    let mut lines: Vec<String> = outline
        .by_ref()
        .take(DRAWN_COMPONENTS)
        .filter_map(|placed| line(&placed))
        .collect();
    if outline.next().is_some() {
        lines.push(format!(
            "… more than {DRAWN_COMPONENTS} components; the rest is not drawn"
        ));
    }
    Some(lines)
}

/// The line one placed component draws as; containers draw nothing themselves.
fn line(placed: &Placed<'_>) -> Option<String> {
    let Placed {
        component, scope, ..
    } = placed;
    match component.component.as_str() {
        "Text" => Some(bound(scope, component, "text")),
        "CheckBox" => {
            let mark = if bound(scope, component, "value") == "true" {
                "x"
            } else {
                " "
            };
            Some(format!("[{mark}] {}", bound(scope, component, "label")))
        }
        _ => None,
    }
}

/// Resolves one property: a literal, a `{"path": …}` binding, or a call.
fn bound(scope: &Scope<'_>, component: &Component, key: &str) -> String {
    let Some(raw) = component.prop(key) else {
        return String::new();
    };
    match scope.resolve_dynamic(raw) {
        Ok(Value::String(text)) => text,
        Ok(Value::Null) => String::new(),
        Ok(other) => other.to_string(),
        Err(error) => format!("<{error}>"),
    }
}

/// Shortens a payload so one tool result is one line.
pub fn clip(text: &str, width: usize) -> String {
    let flat = text.replace('\n', " ");
    if flat.chars().count() <= width {
        return flat;
    }
    let kept: String = flat.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ag_ui::client::transport::ReplayTransport;

    /// The compile-time half of this module's claim: a helper that names a
    /// thread needs no transport bound. If the bound migrates back onto
    /// `Thread`, this file stops compiling.
    #[test]
    fn view_helpers_read_a_thread_without_bounding_its_transport() {
        let thread: Thread<ReplayTransport, Board> = Thread::builder(ReplayTransport::new([]), "t")
            .build()
            .expect("valid initial board");
        assert_eq!(message_count(&thread), 0);
        assert_eq!(run_id(&thread), "—");
        assert_eq!(panel(&thread)[1], "│ (empty)");
    }

    #[test]
    fn a_result_that_is_not_a_surface_draws_nothing() {
        let ordinary = [
            Message::tool("m-1", "call-1", r#"{"id":1}"#),
            Message::tool("m-2", "call-2", "not json at all"),
        ];
        assert!(surface_in_history(&ordinary).unwrap().is_none());
        assert!(
            surface_after(&ordinary[..1], &ordinary[1])
                .unwrap()
                .is_none()
        );
    }

    /// "The conversation does not replay" is not "the conversation has no
    /// surface". An agent that creates its surface again on every turn makes
    /// the first, and a view that folded it into the second would show a
    /// second turn that lost its surface and a run that says `done success`.
    #[test]
    fn a_conversation_that_does_not_replay_is_shown_as_that() {
        use ag_ui_a2ui::AgentMessage;
        use ag_ui_a2ui::adapters::agui::wrap_as_operations_envelope;

        let created =
            wrap_as_operations_envelope(&[AgentMessage::create_surface("board", "basic")]).unwrap();
        let twice = vec![
            Message::tool("m-1", "call-1", created.clone()),
            Message::tool("m-2", "call-2", created),
        ];
        assert!(surface_in_history(&twice[..1]).unwrap().is_some());
        let error = surface_in_history(&twice).unwrap_err().to_string();
        assert!(error.contains("already exists"), "{error}");
        assert!(surface_after(&twice[..1], &twice[1]).is_err());

        let thread: Thread<ReplayTransport, Board> = Thread::builder(ReplayTransport::new([]), "t")
            .messages(twice)
            .build()
            .expect("valid initial board");
        let footer = panel(&thread).pop().unwrap();
        assert!(
            footer.contains("2 messages · surfaces do not replay (")
                && footer.contains("already exists"),
            "{footer}"
        );
    }

    #[test]
    fn a_surface_is_drawn_as_the_update_leaves_it() {
        use ag_ui_a2ui::adapters::agui::{
            Intent, SurfaceSpec, assemble_ops, wrap_as_operations_envelope,
        };
        use serde_json::json;

        let spec = SurfaceSpec::new("board")
            .with_components(vec![
                Component::new("root", "List")
                    .with("children", json!({"componentId": "row", "path": "/tasks"})),
                Component::new("row", "Text").with("text", json!({"path": "line"})),
            ])
            .with_data_model(json!({"tasks": [{"line": "one"}]}));
        let result = |id: &str, intent: Intent, spec: &SurfaceSpec| {
            Message::tool(
                format!("m-{id}"),
                format!("call-{id}"),
                wrap_as_operations_envelope(&assemble_ops(intent, spec)).unwrap(),
            )
        };
        let created = result("1", Intent::Create, &spec);
        let updated = result(
            "2",
            Intent::Update,
            &spec
                .clone()
                .with_data_model(json!({"tasks": [{"line": "one"}, {"line": "two"}]})),
        );

        let first = surface_after(&[], &created)
            .expect("a conversation that replays")
            .expect("a surface");
        assert_eq!(surface_lines(&first).unwrap(), ["one"]);
        // Whether or not the conversation already holds the message.
        for known in [vec![created.clone()], vec![created, updated.clone()]] {
            let second = surface_after(&known, &updated)
                .expect("a conversation that replays")
                .expect("a surface");
            assert_eq!(surface_lines(&second).unwrap(), ["one", "two"]);
        }
    }

    /// Twenty levels of two components, each naming both of the next: 41
    /// components, and an outline of two million. The watcher draws what fits
    /// and says the rest is not drawn.
    #[test]
    fn a_surface_whose_outline_is_far_larger_than_itself_is_drawn_in_part() {
        use ag_ui_a2ui::{AgentMessage, SurfaceStore};
        use serde_json::json;

        let level = |at: usize| [format!("a{at}"), format!("b{at}")];
        let mut components =
            vec![Component::new("root", "Column").with("children", json!(level(1)))];
        for at in 1..20 {
            for id in level(at) {
                components
                    .push(Component::new(id, "Column").with("children", json!(level(at + 1))));
            }
        }
        for id in level(20) {
            components.push(Component::new(id, "Text").with("text", json!("leaf")));
        }
        let mut store = SurfaceStore::new();
        store
            .apply(&AgentMessage::create_surface("wide", "basic"))
            .unwrap();
        store
            .apply(&AgentMessage::update_components("wide", components))
            .unwrap();

        let lines = surface_lines(store.get("wide").unwrap()).unwrap();
        let (note, drawn) = lines.split_last().unwrap();
        assert!(note.starts_with("… more than 500 components"), "{note}");
        assert!(!drawn.is_empty() && drawn.iter().all(|line| line == "leaf"));
        assert!(drawn.len() < DRAWN_COMPONENTS);
    }

    #[test]
    fn clipping_counts_characters_not_bytes() {
        assert_eq!(clip("héllo wörld", 7), "héllo …");
        assert_eq!(clip("short", 40), "short");
        assert_eq!(clip("two\nlines", 40), "two lines");
    }
}
