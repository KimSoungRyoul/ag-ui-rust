//! The terminal client: one line in, one run out.
//!
//! Generic over its input and output rather than wired to `stdin`/`stdout`, so
//! the integration tests under `tests/` drive *this* code with a scripted
//! script and assert on the transcript it produces. A client whose printing is
//! only exercised by a human is a client whose printing breaks quietly.
//!
//! Reading is synchronous inside an async fn on purpose: this is a one-user
//! terminal, and nothing else needs the runtime while it waits for a keystroke.

use std::io::{self, BufRead, Write};

use ag_ui_a2ui::binding::Scope;
use ag_ui_a2ui::constants::ROOT_ID;
use ag_ui_a2ui::message::{AgentPayload, ChildList, Component};
use ag_ui_a2ui::toolkit::envelope::{is_operations_envelope, unwrap_operations_envelope};
use ag_ui_client::transport::Transport;
use ag_ui_client::{
    MessageChangeKind, MessageUpdate, ReasoningChangeKind, RunEnd, RunStream, Session, Update,
};
use ag_ui_core::{Interrupt, Message};
use futures_util::StreamExt as _;
use serde_json::{Value, json};

use crate::board::Board;

/// Where the conversation is read from and written to.
///
/// One type rather than a pair of arguments because of `echo`: a piped script
/// has to have its lines printed for the transcript to read as a conversation,
/// and a human at a terminal has already seen what they typed.
#[derive(Debug)]
pub struct Terminal<R, W> {
    input: R,
    output: W,
    echo: bool,
}

impl<R: BufRead, W: Write> Terminal<R, W> {
    /// A terminal that does not echo what it reads.
    pub fn new(input: R, output: W) -> Self {
        Self {
            input,
            output,
            echo: false,
        }
    }

    /// Echoes every line read, for a script arriving on a pipe.
    #[must_use]
    pub fn echoing(mut self) -> Self {
        self.echo = true;
        self
    }

    /// Unwraps the output sink — how a test reads back the transcript.
    pub fn into_output(self) -> W {
        self.output
    }

    /// Writes a prompt and reads one line. `None` at end of input.
    fn prompt(&mut self, label: &str) -> io::Result<Option<String>> {
        write!(self.output, "{label}")?;
        self.output.flush()?;

        let mut line = String::new();
        if self.input.read_line(&mut line)? == 0 {
            writeln!(self.output)?;
            return Ok(None);
        }
        if self.echo {
            writeln!(self.output, "{}", line.trim_end())?;
        }
        Ok(Some(line))
    }
}

// So every printing helper below can take a plain `&mut impl Write`.
impl<R, W: Write> Write for Terminal<R, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.output.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

/// Reads lines until the input ends, running one turn per line.
///
/// `quit` and end-of-input both stop it.
pub async fn converse<T: Transport>(
    session: &mut Session<T, Board>,
    terminal: &mut Terminal<impl BufRead, impl Write>,
) -> io::Result<()> {
    while let Some(line) = terminal.prompt("you> ")? {
        let said = line.trim().to_owned();
        if said.is_empty() {
            continue;
        }
        if said.eq_ignore_ascii_case("quit") || said.eq_ignore_ascii_case("exit") {
            break;
        }
        turn(session, &said, terminal).await?;
    }
    Ok(())
}

/// One user turn, including however many interrupts it takes to finish.
///
/// The loop is the human-in-the-loop round trip: a paused run is answered and
/// *resumed*, which is a second request in the same thread, and the resumed run
/// may pause again.
async fn turn<T: Transport>(
    session: &mut Session<T, Board>,
    said: &str,
    terminal: &mut Terminal<impl BufRead, impl Write>,
) -> io::Result<()> {
    // Each `drive` call ends the mutable borrow `send`/`resume` takes, which is
    // what lets the next one start.
    let mut pending = drive(session.send(said), terminal).await?;

    while let Some(interrupt) = pending {
        pending = if approved(&interrupt, terminal)? {
            drive(
                session.resume(&interrupt, json!({"confirm": true})),
                terminal,
            )
            .await?
        } else {
            drive(session.cancel(&interrupt), terminal).await?
        };
    }
    Ok(())
}

/// Consumes one run, printing it, and reports the interrupt it paused on.
async fn drive<T: Transport>(
    mut run: RunStream<'_, T, Board>,
    output: &mut impl Write,
) -> io::Result<Option<Interrupt>> {
    let mut pending = None;
    let mut printed = None;
    // The reply and a tool call's arguments print without a newline, so an
    // update that owns a whole line has to wait for the open one to close
    // rather than splice itself into it. The board does exactly that now: a
    // tool call publishes state *while it is open*, so the summary arrives
    // between the call's arguments and its closing bracket.
    let mut open_line = false;
    let mut board = None;

    while let Some(update) = run.next().await {
        match update {
            Update::Message(message) => open_line = print_message(output, &message, open_line)?,

            // Only the finished thought is printed: a reasoning block is
            // commentary, and streaming it interleaved with the reply is noise
            // in a terminal.
            //
            // The `printed` guard is a workaround, not taste. `ctx.think()`
            // emits REASONING_MESSAGE_END *and* REASONING_END, and the client
            // maps both to `ReasoningChangeKind::Ended` under the same id — so
            // the obvious spelling of this arm prints every thought twice.
            Update::Reasoning(reasoning)
                if reasoning.change == ReasoningChangeKind::Ended
                    && printed.as_ref() != Some(&reasoning.id) =>
            {
                writeln!(output, "  ~ {}", reasoning.text)?;
                printed = Some(reasoning.id);
            }

            // The typed state, already patched: `Board` came off the wire as a
            // STATE_SNAPSHOT or a STATE_DELTA and neither this line nor the
            // one above it can tell which.
            Update::State(state) => board = Some(state.summary()),

            Update::Interrupt(interrupt) => pending = Some(interrupt),
            Update::Error(error) => writeln!(output, "  !! {error}")?,
            Update::Done(RunEnd::Failed { message, .. }) => {
                writeln!(output, "  !! the run failed: {message}")?;
            }
            _ => {}
        }

        if !open_line {
            if let Some(summary) = board.take() {
                writeln!(output, "  [state] {summary}")?;
            }
        }
    }
    Ok(pending)
}

/// Prints one message change, and reports whether it left the line open.
fn print_message(
    output: &mut impl Write,
    update: &MessageUpdate,
    open_line: bool,
) -> io::Result<bool> {
    match &update.change {
        MessageChangeKind::Started => {
            write!(output, "  agent> ")?;
            output.flush()?;
            Ok(true)
        }
        // One delta, one word. Flushed so a slow agent reads as typing rather
        // than as a hang.
        MessageChangeKind::Content { delta } => {
            write!(output, "{delta}")?;
            output.flush()?;
            Ok(true)
        }
        MessageChangeKind::Ended => {
            writeln!(output)?;
            Ok(false)
        }

        MessageChangeKind::ToolCallStarted { name, .. } => {
            write!(output, "  · {name}(")?;
            Ok(true)
        }
        MessageChangeKind::ToolCallArgs { delta, .. } => {
            write!(output, "{delta}")?;
            Ok(true)
        }
        MessageChangeKind::ToolCallEnded { .. } => {
            writeln!(output, ")")?;
            Ok(false)
        }
        MessageChangeKind::ToolResult { .. } => {
            print_result(output, &update.message)?;
            Ok(false)
        }

        // Nothing printed, so the line is however the last print left it.
        _ => Ok(open_line),
    }
}

/// Prints a tool result — as a surface when it is one, as JSON otherwise.
fn print_result(output: &mut impl Write, message: &Message) -> io::Result<()> {
    let Message::Tool(tool) = message else {
        return Ok(());
    };
    let Ok(value) = serde_json::from_str::<Value>(&tool.content) else {
        writeln!(output, "    → {}", tool.content)?;
        return Ok(());
    };

    // The sniff every A2UI frontend does: a tool result either is an operations
    // envelope or is an ordinary result, and nothing else distinguishes them.
    if !is_operations_envelope(&value) {
        writeln!(output, "    → {value}")?;
        return Ok(());
    }

    match surface_lines(&value) {
        Some(lines) => {
            writeln!(output, "    ┌ a2ui surface")?;
            for line in lines {
                writeln!(output, "    │ {line}")?;
            }
            writeln!(output, "    └")
        }
        None => writeln!(output, "    → an A2UI envelope this client cannot draw"),
    }
}

/// Draws the surface in the envelope, or `None` if it carries no components.
///
/// A real renderer owns a widget toolkit and a reactive data model; this walks
/// the tree far enough to prove the surface arrived whole — every child
/// reference resolved, every binding evaluated through [`Scope`], the list
/// template instantiated once per task.
fn surface_lines(envelope: &Value) -> Option<Vec<String>> {
    let operations = unwrap_operations_envelope(envelope).ok()?;

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
                    // Entering the item's scope is what makes the template's
                    // relative paths (`label`, `done`) resolve.
                    let item = scope.item(&template.path, index);
                    draw(components, &item, &template.component_id, lines);
                }
            }
            None => lines.push(format!("<{} has no children>", component.component)),
        },
    }
}

/// Resolves one property through the data model: a literal, a `{"path": …}`
/// binding, or a `formatString` call.
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

/// Asks the human the interrupt's question. End of input declines, because the
/// interrupt exists to stop something destructive.
fn approved(
    interrupt: &Interrupt,
    terminal: &mut Terminal<impl BufRead, impl Write>,
) -> io::Result<bool> {
    let question = interrupt
        .message
        .as_deref()
        .unwrap_or("The agent is waiting for a decision.");
    writeln!(terminal, "  ?? {question}")?;

    let Some(answer) = terminal.prompt("  [y/N] ")? else {
        writeln!(terminal, "  (no answer — declining)")?;
        return Ok(false);
    };
    let answer = answer.trim();
    Ok(answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes"))
}
