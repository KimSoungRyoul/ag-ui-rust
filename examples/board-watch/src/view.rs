//! Turning what the client assembled into lines a person reads.
//!
//! Every helper here names a [`Session`] without bounding its transport. That
//! is not incidental: an application's view layer only ever *reads* a session,
//! and a `T: Transport` bound on the type would force every one of these
//! signatures to repeat a constraint none of them use. `ag-ui-client` keeps the
//! bound on the impl blocks that make requests, and this module is what spends
//! that.

use ag_ui_a2ui::agui::find_prior_surface_in;
use ag_ui_a2ui::binding::Scope;
use ag_ui_a2ui::constants::ROOT_ID;
use ag_ui_a2ui::toolkit::envelope::{is_operations_envelope, unwrap_operations_envelope};
use ag_ui_a2ui::toolkit::history::PriorSurface;
use ag_ui_a2ui::{AgentPayload, ChildList, Component};
use ag_ui_client::Session;
use ag_ui_core::Message;
use serde_json::Value;

use crate::board::Board;

/// The run the session last heard about, for the footer.
///
/// No `T: Transport` — this only reads.
pub fn run_id<T, S>(session: &Session<T, S>) -> String {
    session
        .applier()
        .run_id()
        .map_or_else(|| "—".to_owned(), |id| id.as_str().to_owned())
}

/// How many messages the conversation holds.
pub fn message_count<T, S>(session: &Session<T, S>) -> usize {
    session.messages().len()
}

/// The panel drawn after each run: the board, then where it came from.
///
/// Names `Session<T, Board>` — a concrete state, still no transport bound.
pub fn panel<T>(session: &Session<T, Board>) -> Vec<String> {
    let mut lines = vec!["┌ board".to_owned()];

    match session.state() {
        Some(board) if !board.tasks.is_empty() => {
            lines.push(format!("│ {}", board.summary()));
            lines.extend(board.tasks.iter().map(|task| format!("│ {}", task.line())));
        }
        // `None` is not "empty": it means no STATE_* event has arrived at all,
        // and a view that renders the two the same way hides a broken agent.
        Some(_) => lines.push("│ (empty)".to_owned()),
        None => lines.push("│ (no state published)".to_owned()),
    }

    let surface = match surface_in_history(session.messages()) {
        Some(prior) => format!(
            " · surface {} ({})",
            prior.surface_id,
            prior.components.len()
        ),
        None => String::new(),
    };
    lines.push(format!(
        "└ run {} · {} messages{surface}",
        run_id(session),
        message_count(session)
    ));
    lines
}

/// The surface the conversation is carrying, recovered from history.
///
/// The client's half of what the agent does to decide create-versus-update: the
/// A2UI operations are in the transcript, so anything holding the transcript can
/// replay them. No hand-written message mapping — that is what the toolkit's
/// `ag-ui` feature is for.
pub fn surface_in_history(messages: &[Message]) -> Option<PriorSurface> {
    find_prior_surface_in(messages).filter(|prior| !prior.deleted)
}

/// Draws an `a2ui_operations` envelope, or `None` if it is not one.
///
/// The sniff every A2UI front-end does: a tool result either carries the
/// envelope key or is an ordinary result, and nothing else tells them apart.
pub fn surface_lines(payload: &str) -> Option<Vec<String>> {
    let value: Value = serde_json::from_str(payload).ok()?;
    if !is_operations_envelope(&value) {
        return None;
    }

    let operations = unwrap_operations_envelope(&value).ok()?;
    let mut components: Vec<Component> = Vec::new();
    let mut data = Value::Null;
    for operation in &operations {
        match &operation.payload {
            AgentPayload::UpdateComponents(payload) => components.clone_from(&payload.components),
            AgentPayload::UpdateDataModel(payload) => data = payload.value.clone(),
            _ => {}
        }
    }
    if components.is_empty() {
        return None;
    }

    let mut lines = Vec::new();
    draw(&components, &Scope::root(&data), ROOT_ID, &mut lines);
    Some(lines)
}

/// Appends the lines one component draws as, children included.
///
/// A real renderer owns a widget toolkit; this walks far enough to prove the
/// surface arrived whole — every child reference resolved, every binding
/// evaluated, the list template instantiated once per item.
fn draw(components: &[Component], scope: &Scope<'_>, id: &str, lines: &mut Vec<String>) {
    let Some(component) = components.iter().find(|component| component.id == id) else {
        lines.push(format!("<no component {id}>"));
        return;
    };

    match component.component.as_str() {
        "Text" => lines.push(bound(scope, component, "text")),
        "CheckBox" => {
            let mark = if bound(scope, component, "value") == "true" {
                "x"
            } else {
                " "
            };
            lines.push(format!("[{mark}] {}", bound(scope, component, "label")));
        }
        "Card" => {
            if let Some(child) = component.prop("child").and_then(Value::as_str) {
                draw(components, scope, child, lines);
            }
        }
        // Every container in the basic catalog spells its children the same
        // way, so one arm covers them.
        _ => match component.prop("children").and_then(ChildList::from_value) {
            Some(ChildList::Ids(ids)) => {
                for child in ids {
                    draw(components, scope, &child, lines);
                }
            }
            Some(ChildList::Template(template)) => {
                let count = scope
                    .resolve(&template.path)
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                for index in 0..count {
                    // Entering the item scope is what makes the template's
                    // relative paths resolve.
                    let item = scope.item(&template.path, index);
                    draw(components, &item, &template.component_id, lines);
                }
            }
            None => lines.push(format!("<{} has no children>", component.component)),
        },
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
    use ag_ui_client::transport::ReplayTransport;

    /// The compile-time half of this module's claim: a helper that names a
    /// session needs no transport bound. If the bound migrates back onto
    /// `Session`, this file stops compiling.
    #[test]
    fn view_helpers_read_a_session_without_bounding_its_transport() {
        let session: Session<ReplayTransport, Board> = Session::new(ReplayTransport::new([]), "t");
        assert_eq!(message_count(&session), 0);
        assert_eq!(run_id(&session), "—");
        assert_eq!(panel(&session)[1], "│ (no state published)");
    }

    #[test]
    fn a_result_that_is_not_a_surface_draws_nothing() {
        assert!(surface_lines(r#"{"id":1}"#).is_none());
        assert!(surface_lines("not json at all").is_none());
    }

    #[test]
    fn clipping_counts_characters_not_bytes() {
        assert_eq!(clip("héllo wörld", 7), "héllo …");
        assert_eq!(clip("short", 40), "short");
        assert_eq!(clip("two\nlines", 40), "two lines");
    }
}
