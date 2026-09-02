//! The driver: one line in, one run out, and everything the client assembled
//! printed as it arrives.
//!
//! Generic over its input and output rather than wired to `stdin`/`stdout`, so
//! the integration tests drive *this* code with a scripted script and assert on
//! the transcript. A client whose printing is only exercised by a human is a
//! client whose printing breaks quietly.

use std::io::{self, BufRead, Write};

use ag_ui::client::interrupts::ResumeBuilder;
use ag_ui::client::transport::Transport;
use ag_ui::client::{
    InterruptExt as _, MessageChangeKind, MessageUpdate, ReasoningChangeKind, RunEnd, RunStream,
    Session, SubagentChangeKind, Update,
};
use ag_ui::{Interrupt, Message, MessageId, ResumeEntry, ToolCallId};
use futures_util::StreamExt as _;
use serde_json::json;

use crate::board::Board;
use crate::view;

/// How wide a tool result is allowed to print.
const CLIP: usize = 88;

/// What to do when a run pauses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Policy {
    /// Ask the person at the keyboard.
    #[default]
    Ask,
    /// Approve everything, unattended.
    Approve,
    /// Decline everything, unattended.
    Decline,
}

/// How the watcher behaves for one session.
#[derive(Clone, Copy, Debug, Default)]
pub struct Watch {
    /// What to do with interrupts.
    pub policy: Policy,
    /// Print each delta in brackets instead of joining them, so chunk
    /// normalization is visible in the transcript.
    pub fragments: bool,
    /// Draw one line per update, in arrival order, instead of grouping a tool
    /// call onto one line when it closes.
    ///
    /// The trade `ag-ui-client`'s [session docs] describe, made visible: the
    /// grouped view reads better and reorders anything that happened inside a
    /// call; this one is faithful and noisier. Neither is more correct — the
    /// arrival order is the only nesting there is, so a view that keeps it is
    /// the one that can show it.
    ///
    /// [session docs]: https://docs.rs/ag-ui-client/latest/ag_ui::client/session/index.html
    pub in_order: bool,
    /// Stop reading after this many updates and drop the stream — what a user
    /// hitting Ctrl-C does, and the only cancellation a client actually has.
    pub stop_after: Option<usize>,
}

/// Where the conversation is read from and written to.
///
/// One type rather than a pair of arguments because of `echo`: a piped script
/// has to have its lines printed for the transcript to read as a session, and a
/// human at a terminal has already seen what they typed.
#[derive(Debug)]
pub struct Console<R, W> {
    input: R,
    output: W,
    echo: bool,
}

impl<R: BufRead, W: Write> Console<R, W> {
    /// A console that does not echo what it reads.
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
impl<R, W: Write> Write for Console<R, W> {
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
pub async fn watch<T: Transport>(
    session: &mut Session<T, Board>,
    settings: Watch,
    console: &mut Console<impl BufRead, impl Write>,
) -> io::Result<()> {
    while let Some(line) = console.prompt("> ")? {
        let said = line.trim().to_owned();
        if said.is_empty() {
            continue;
        }
        if said.eq_ignore_ascii_case("quit") || said.eq_ignore_ascii_case("exit") {
            break;
        }
        turn(session, &said, settings, console).await?;
    }
    Ok(())
}

/// One turn, including however many pauses it takes to finish.
pub async fn turn<T: Transport>(
    session: &mut Session<T, Board>,
    said: &str,
    settings: Watch,
    console: &mut Console<impl BufRead, impl Write>,
) -> io::Result<()> {
    // Each `drive` call ends the mutable borrow `send`/`resume_many` takes,
    // which is what lets the next one start.
    let mut pending = drive(session.send(said), settings, console).await?;

    while !pending.is_empty() {
        // Every pending decision answered in *one* request. A run pauses on all
        // of them at once and only sees what the resuming request carries, so
        // answering one per request never terminates.
        let entries = answer(&pending, settings, console)?;
        pending = drive(session.resume_many(entries), settings, console).await?;
    }

    for line in view::panel(session) {
        writeln!(console, "{line}")?;
    }
    Ok(())
}

/// Consumes one run, printing it, and reports what it paused on.
async fn drive<T: Transport>(
    mut run: RunStream<'_, T, Board>,
    settings: Watch,
    out: &mut impl Write,
) -> io::Result<Vec<Interrupt>> {
    let mut pending = Vec::new();
    let mut seen = 0usize;
    let mut open = Open::default();

    while let Some(update) = run.next().await {
        seen += 1;

        // Anything that is not a message change interrupts the line a message
        // is streaming onto. A producer that never closes its last message —
        // legal, and what an unbracketed chunk stream does — would otherwise
        // run the next line into it.
        if !matches!(update, Update::Message(_)) {
            open.close_text(out)?;
        }

        match update {
            Update::Message(message) => open.print(out, &message, settings)?,

            // Once per thought. The protocol brackets one twice — the block and
            // the message inside it — and the client reports the lifecycle
            // rather than the framing, so this arm needs no dedupe.
            Update::Reasoning(reasoning) if reasoning.change == ReasoningChangeKind::Ended => {
                writeln!(out, "  think  {}", reasoning.text)?;
            }

            // Typed, and already patched: this arrived as a STATE_SNAPSHOT or a
            // STATE_DELTA and nothing here can tell which.
            Update::State(board) => writeln!(out, "  state  {}", board.summary())?,

            Update::Messages(messages) => {
                writeln!(out, "  reset  {} messages replaced", messages.len())?;
            }

            Update::Interrupt(interrupt) => {
                writeln!(
                    out,
                    "  pause  {} · {}",
                    interrupt.id,
                    interrupt.message.as_deref().unwrap_or("(no question)")
                )?;
                pending.push(interrupt);
            }

            // A delegate opening or closing. Its messages arrive as ordinary
            // `Update::Message`s in between, each carrying its id.
            Update::Subagent(subagent) => {
                writeln!(
                    out,
                    "  child  {} {}",
                    subagent.subagent.name,
                    delegated(&subagent.change)
                )?;
            }

            Update::Error(error) => writeln!(out, "  error  {error}")?,

            Update::Done(end) => writeln!(out, "  done   {}", ended(&end))?,

            _ => {}
        }

        // Dropping the stream is the whole of client-side cancellation: polling
        // it is what pulls bytes, so letting go stops the run at the far end.
        if settings.stop_after == Some(seen) {
            writeln!(out, "  stop   dropped the stream after {seen} updates")?;
            return Ok(pending);
        }
    }
    Ok(pending)
}

/// What happened to a subagent, in one word.
///
/// The `_` arm is not a shrug: [`SubagentChangeKind`] is `#[non_exhaustive]`,
/// so a kind this client was not written for prints rather than stops the
/// build.
fn delegated(change: &SubagentChangeKind) -> &'static str {
    match change {
        SubagentChangeKind::Started => "started",
        SubagentChangeKind::Resumed => "resumed",
        SubagentChangeKind::Finished => "finished",
        SubagentChangeKind::Suspended => "suspended",
        SubagentChangeKind::Failed => "failed",
        _ => "changed",
    }
}

/// How a run ended, in one phrase.
///
/// Three arms and no `_`: [`RunEnd`] is exhaustive, so a fourth way for a run
/// to end would stop this build rather than reach a user as a shrug. That is
/// the match a client most wants the compiler's help with — the arms decide
/// whether the prompt comes back, whether an answer is owed, and whether
/// anything failed.
fn ended(end: &RunEnd) -> String {
    match end {
        RunEnd::Success { .. } => "success".to_owned(),
        RunEnd::Interrupted { interrupts } => format!("interrupted on {}", interrupts.len()),
        RunEnd::Failed { message, code } => match code {
            Some(code) => format!("failed [{code}] {message}"),
            None => format!("failed {message}"),
        },
    }
}

/// What is part-printed on the current line.
///
/// # Why a renderer needs this
///
/// The change stream is per *event*, not per message: an
/// [`Update::Message`] says "this delta arrived for this id", and the ids
/// interleave. Two tool calls in flight — which a model does whenever it asks
/// for two things at once — arrive as `args(a) args(b) args(a) end(a) end(b)`,
/// so the obvious renderer that prints a prefix on `Started` and a newline on
/// `Ended` produces one garbled line. Text is streamed inline anyway, because
/// watching a reply type out is the point; a second text id simply closes the
/// first line and opens another.
///
/// # What buffering costs, and what it does not
///
/// A call is printed when it *closes*, so anything the agent emitted while it
/// was open — a `STATE_DELTA` published from inside the call, which
/// `ag-ui-server`'s handles allow — prints **before** the call line rather than
/// inside it.
///
/// That is a property of *this* rendering, not a limit of the client. Arrival
/// order carries the nesting; what cannot be had is a call drawn as one line
/// *and* kept in order, because the line cannot be written until the call
/// closes. [`Watch::in_order`] takes the other side of that trade and shows the
/// state between the call's arguments and its end, which is where the wire put
/// it. Legibility under parallel calls comes from tagging each line with the
/// call id — not from buffering, which was the wrong conclusion the first time
/// this was written down.
#[derive(Debug, Default)]
struct Open {
    /// The text message whose line is currently unterminated.
    text: Option<MessageId>,
    /// Tool calls opened and not yet closed, oldest first, with the fragments
    /// each has collected.
    calls: Vec<(ToolCallId, String, Vec<String>)>,
}

impl Open {
    /// Prints one message change, closing whatever it displaces.
    fn print(
        &mut self,
        out: &mut impl Write,
        update: &MessageUpdate,
        settings: Watch,
    ) -> io::Result<()> {
        match &update.change {
            MessageChangeKind::Started => self.open_text(out, &update.id),

            // One delta per event. In `fragments` mode each is bracketed, which
            // is what makes chunk normalization visible: the agent sent five
            // events and the message is one string.
            MessageChangeKind::Content { delta } => {
                if self.text.as_ref() != Some(&update.id) {
                    self.open_text(out, &update.id)?;
                }
                write!(out, "{}", mark(delta, settings))?;
                out.flush()
            }

            MessageChangeKind::Ended => self.close_text(out),

            MessageChangeKind::ToolCallStarted { tool_call_id, name } => {
                self.calls
                    .push((tool_call_id.clone(), name.clone(), Vec::new()));
                if settings.in_order {
                    self.close_text(out)?;
                    return writeln!(out, "  call   {name} ({})", short(tool_call_id));
                }
                Ok(())
            }
            MessageChangeKind::ToolCallArgs {
                tool_call_id,
                delta,
            } => {
                if settings.in_order {
                    self.close_text(out)?;
                    // Named, because in arrival order two calls' fragments are
                    // adjacent and the id is the only thing separating them.
                    return writeln!(
                        out,
                        "  args   ({}) {}",
                        short(tool_call_id),
                        mark(delta, settings)
                    );
                }
                if let Some(call) = self.call_mut(tool_call_id) {
                    call.2.push(delta.clone());
                }
                Ok(())
            }
            // The whole call on one line, however many events it took and
            // whatever else was in flight beside it.
            MessageChangeKind::ToolCallEnded { tool_call_id } => {
                let Some(index) = self.calls.iter().position(|call| &call.0 == tool_call_id) else {
                    return Ok(());
                };
                let (_, name, fragments) = self.calls.remove(index);
                self.close_text(out)?;

                if settings.in_order {
                    return writeln!(out, "  end    {name} ({})", short(tool_call_id));
                }

                let args: String = fragments
                    .iter()
                    .map(|fragment| mark(fragment, settings))
                    .collect();
                writeln!(out, "  call   {name} {args}")
            }

            MessageChangeKind::ToolResult { .. } => {
                self.close_text(out)?;
                print_result(out, &update.message)
            }

            _ => Ok(()),
        }
    }

    fn call_mut(&mut self, id: &ToolCallId) -> Option<&mut (ToolCallId, String, Vec<String>)> {
        self.calls.iter_mut().find(|call| &call.0 == id)
    }

    fn open_text(&mut self, out: &mut impl Write, id: &MessageId) -> io::Result<()> {
        self.close_text(out)?;
        self.text = Some(id.clone());
        write!(out, "  text   ")?;
        out.flush()
    }

    fn close_text(&mut self, out: &mut impl Write) -> io::Result<()> {
        if self.text.take().is_some() {
            writeln!(out)?;
        }
        Ok(())
    }
}

/// The tail of a call id, enough to tell two apart in one run.
///
/// Ids are the producer's, and this one only has to disambiguate within a
/// transcript, so the last segment is plenty and the whole thing is noise.
fn short(id: &ToolCallId) -> &str {
    let id = id.as_str();
    match id.rfind('-') {
        Some(index) => &id[index + 1..],
        None => id,
    }
}

/// One delta, bracketed when the transcript is meant to show fragmentation.
fn mark(delta: &str, settings: Watch) -> String {
    if settings.fragments {
        format!("[{delta}]")
    } else {
        delta.to_owned()
    }
}

/// Prints a tool result — as a drawn surface when it is one, clipped otherwise.
fn print_result(out: &mut impl Write, message: &Message) -> io::Result<()> {
    let Message::Tool(tool) = message else {
        return Ok(());
    };

    match view::surface_lines(&tool.content) {
        Some(lines) => {
            writeln!(out, "  surface")?;
            for line in lines {
                writeln!(out, "    {line}")?;
            }
            Ok(())
        }
        None => writeln!(out, "  result {}", view::clip(&tool.content, CLIP)),
    }
}

/// Answers every pending interrupt, in one batch.
fn answer(
    pending: &[Interrupt],
    settings: Watch,
    console: &mut Console<impl BufRead, impl Write>,
) -> io::Result<Vec<ResumeEntry>> {
    let mut builder = ResumeBuilder::new();

    for interrupt in pending {
        let approved = match settings.policy {
            Policy::Approve => true,
            Policy::Decline => false,
            Policy::Ask => ask(interrupt, console)?,
        };
        builder = if approved {
            builder.resolve(interrupt, json!({"confirm": true}))
        } else {
            builder.cancel(interrupt)
        };
        writeln!(
            console,
            "  answer {} · {}",
            interrupt.id,
            if approved { "approved" } else { "declined" }
        )?;
    }
    Ok(builder.build())
}

/// Asks the person at the keyboard. End of input declines: an interrupt exists
/// to stop something, and silence is not consent.
fn ask(interrupt: &Interrupt, console: &mut Console<impl BufRead, impl Write>) -> io::Result<bool> {
    let kind = if interrupt.is_tool_approval() {
        "approve"
    } else {
        "answer"
    };
    let label = format!("  {kind} {} [y/N] ", interrupt.id);

    let Some(reply) = console.prompt(&label)? else {
        writeln!(console, "  (no answer — declining)")?;
        return Ok(false);
    };
    let reply = reply.trim();
    Ok(reply.eq_ignore_ascii_case("y") || reply.eq_ignore_ascii_case("yes"))
}
