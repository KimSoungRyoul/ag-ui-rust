//! The agent: one `impl Agent`, and the command parser it runs on.
//!
//! Deterministic by default. The board moves because the user typed `add` and
//! not because a model decided to call a tool, which is what makes the
//! transcripts in `README.md` and the tests under `tests/` assertable to the
//! character. `--llm` swaps only the *phrasing* of the reply — see
//! [`crate::llm`].
//!
//! What one run emits, in order:
//!
//! ```text
//! STEP_STARTED board
//!   REASONING_*                       what the agent made of the message
//!   TOOL_CALL_START/ARGS              once per mutation
//!     STATE_SNAPSHOT | STATE_DELTA    the board moving, with the call open
//!   TOOL_CALL_END/RESULT
//!   TEXT_MESSAGE_START/CONTENT*/END   the reply, a word per delta
//!   TOOL_CALL_* render_a2ui           the board as an A2UI surface
//! STEP_FINISHED board
//! ```

use ag_ui::serve::{Agent, Error, Result, RunContext};
use ag_ui::{Interrupt, JsonObject, Message, ResumeStatus, RunOutcome};
use ag_ui_a2ui::constants::RENDER_A2UI_TOOL_NAME;
use ag_ui_a2ui::find_prior_surface_in;
use ag_ui_a2ui::toolkit::envelope::wrap_as_operations_envelope;
use ag_ui_a2ui::toolkit::ops::{Intent, assemble_ops};
use serde_json::json;

use crate::board::{self, Board};
use crate::llm::Voice;

/// The interrupt id the clear confirmation round trips on.
pub const CLEAR_INTERRUPT: &str = "confirm-clear";

/// The workshop assistant.
#[derive(Debug, Default)]
pub struct TaskBoard {
    /// Absent unless `--llm` found a key. The board never depends on it.
    voice: Option<Voice>,
}

impl TaskBoard {
    /// The deterministic agent. No network, no key, no model.
    pub fn scripted() -> Self {
        Self { voice: None }
    }

    /// The same agent, with the reply text phrased by a model.
    pub fn with_voice(voice: Voice) -> Self {
        Self { voice: Some(voice) }
    }
}

impl Agent for TaskBoard {
    type State = Board;

    async fn run(&self, ctx: &mut RunContext<Board>) -> Result<RunOutcome> {
        let said = ctx.last_user_text().unwrap_or_default();
        let command = Command::parse(&said);
        let intent = surface_intent(ctx.messages());

        // Only this request's answer counts, so the pending decision is read
        // before anything is emitted. `Copy`ing the status out ends the borrow
        // of `ctx` that `resume_for` takes — the emitters below all want it
        // mutably.
        let answer = ctx.resume_for(CLEAR_INTERRUPT).map(|entry| entry.status);

        if command == Command::Clear && answer.is_none() {
            return ask_before_clearing(ctx);
        }

        let mut step = ctx.step("board")?;
        step.think(command.plan(answer))?;

        let report = apply(&mut step, &command, answer)?;
        let reply = self.phrase(&mut step, &said, report.reply).await?;
        stream(&mut step, &reply)?;

        if report.render {
            render(&mut step, intent)?;
        }
        Ok(RunOutcome::Success)
    }
}

impl TaskBoard {
    /// The reply text, phrased by the model when there is one.
    ///
    /// A model that fails does not fail the run: the scripted sentence is
    /// already correct, and the failure is said out loud as reasoning rather
    /// than swallowed.
    async fn phrase(
        &self,
        ctx: &mut RunContext<Board>,
        said: &str,
        scripted: String,
    ) -> Result<String> {
        let Some(voice) = &self.voice else {
            return Ok(scripted);
        };
        // Racing the model against cancellation is what makes closing the
        // terminal stop the run rather than pay for a completion nobody reads.
        let phrased = ctx
            .until_cancelled(voice.phrase(said, &scripted))
            .await
            .ok_or(Error::Cancelled)?;

        match phrased {
            Ok(text) if !text.trim().is_empty() => Ok(text),
            Ok(_) => Ok(scripted),
            Err(error) => {
                ctx.think(format!(
                    "the model did not answer ({error}); saying it plainly"
                ))?;
                Ok(scripted)
            }
        }
    }
}

/// What one command did, once it has been done.
struct Report {
    /// The sentence the agent says about it.
    reply: String,
    /// Whether the board changed enough to be worth redrawing.
    render: bool,
}

/// Runs the command: one tool call and one state publish per mutation.
///
/// # Why every branch works while its call is open
///
/// A [`ToolCallHandle`](ag_ui::serve::ToolCallHandle) reaches the run state, so
/// each branch announces the call, moves the board under it, and only then
/// reports the result. That is the order a client sees: the call in flight, the
/// board changing, the result closing it — which for a slow tool is the reason
/// to stream a call at all.
///
/// The protocol allows it because `STATE_*` is unordered, so a publish between
/// `TOOL_CALL_START` and `TOOL_CALL_END` is a well-formed stream. The server's
/// verifier agrees, and `tests/flows.rs` pins the resulting order.
fn apply(
    ctx: &mut RunContext<Board>,
    command: &Command,
    answer: Option<ResumeStatus>,
) -> Result<Report> {
    match command {
        Command::Add(titles) => {
            let mut added = Vec::new();
            for title in titles {
                let mut call = offered(ctx, board::ADD_TASK)?;
                call.args_json(&json!({"title": title}))?;

                let task = call.state_mut().add(title).clone();
                // One publish per task, so a two-task message makes the server
                // choose an encoding twice: the first publish is a snapshot,
                // and the second is a STATE_DELTA only if the patch comes out
                // smaller than the board. A client mirroring this has to
                // survive both, and `tests/flows.rs` pins both.
                call.publish_state()?;

                call.result_json(&json!({"id": task.id, "title": task.title}))?;
                added.push(format!("#{} {}", task.id, task.title));
            }
            Ok(Report {
                reply: format!("Added {}. {}", added.join(", "), ctx.state().summary()),
                render: true,
            })
        }

        Command::Complete(needle) => {
            let mut call = offered(ctx, board::COMPLETE_TASK)?;
            call.args_json(&json!({"task": needle}))?;

            let done = call.state_mut().complete(needle).cloned();
            match done {
                Some(task) => {
                    call.publish_state()?;
                    call.result_json(&json!({"id": task.id, "title": task.title, "done": true}))?;
                    Ok(Report {
                        reply: format!(
                            "Done: #{} {}. {}",
                            task.id,
                            task.title,
                            ctx.state().summary()
                        ),
                        render: true,
                    })
                }
                None => {
                    call.result_json(&json!({"error": "no such task", "task": needle}))?;
                    Ok(Report {
                        reply: format!("Nothing on the board matches \"{needle}\"."),
                        render: false,
                    })
                }
            }
        }

        Command::Estimate { task, minutes } => {
            let mut call = offered(ctx, board::ESTIMATE)?;
            call.args_json(&json!({"task": task, "minutes": minutes}))?;

            let estimated = call.state_mut().estimate(task, *minutes).cloned();
            match estimated {
                Some(estimated) => {
                    call.publish_state()?;
                    call.result_json(&json!({"id": estimated.id, "minutes": minutes}))?;
                    Ok(Report {
                        reply: format!(
                            "#{} is {minutes}m. {}",
                            estimated.id,
                            ctx.state().summary()
                        ),
                        render: true,
                    })
                }
                None => {
                    call.result_json(&json!({"error": "no such task", "task": task}))?;
                    Ok(Report {
                        reply: format!("Nothing on the board matches \"{task}\"."),
                        render: false,
                    })
                }
            }
        }

        // Reached only on a resumed run: the unanswered case returned an
        // interrupt before any of this.
        Command::Clear => match answer {
            Some(ResumeStatus::Resolved) => {
                let mut call = offered(ctx, board::CLEAR_BOARD)?;
                call.args_json(&json!({}))?;

                let removed = call.state_mut().clear();
                call.publish_state()?;

                call.result_json(&json!({"removed": removed}))?;
                Ok(Report {
                    reply: format!("Cleared {removed} task(s). The board is empty."),
                    render: true,
                })
            }
            _ => Ok(Report {
                reply: format!("Left the board alone. {}", ctx.state().summary()),
                render: true,
            }),
        },

        // The reply stays a sentence and the surface does the drawing. That
        // split is the whole reason A2UI rides alongside the text.
        Command::List => Ok(Report {
            reply: match ctx.state().tasks.len() {
                0 => "The board is empty. Try: add draft the agenda".to_owned(),
                count => format!("{count} task(s) on the board — {}.", ctx.state().summary()),
            },
            render: true,
        }),

        Command::Help => Ok(Report {
            reply: HELP.to_owned(),
            render: false,
        }),
    }
}

/// Opens a tool call, having checked the client actually offered that tool.
///
/// This is a rule this agent adopts, not one the protocol imposes: the offered
/// list says what the client *can execute*, so calling something absent from it
/// is legal and `render_a2ui` below does exactly that. But these four tools move
/// the board on the client's behalf, so one the client cannot run is a bug it
/// would otherwise discover as a widget it cannot draw.
fn offered<'a>(
    ctx: &'a mut RunContext<Board>,
    name: &str,
) -> Result<ag_ui::serve::ToolCallHandle<'a, Board>> {
    if ctx.tool(name).is_none() {
        return Err(Error::agent(format!("the client offered no {name} tool")));
    }
    ctx.tool_call(name)
}

/// What the agent says when it does not understand.
const HELP: &str = "I keep a task board. Try: add draft the agenda, book the room · \
complete 1 · estimate 2 45 · list · clear";

/// Pauses the run on the one destructive command.
fn ask_before_clearing(ctx: &mut RunContext<Board>) -> Result<RunOutcome> {
    let count = ctx.state().tasks.len();
    let mut step = ctx.step("confirm")?;
    step.think("clearing cannot be undone, so a human decides")?;
    stream(
        &mut step,
        &format!("Clearing drops {count} task(s) and cannot be undone."),
    )?;
    drop(step);

    Ok(RunOutcome::interrupt(vec![clear_interrupt(count)]))
}

/// The question the client renders, with the schema its answer must satisfy.
fn clear_interrupt(count: usize) -> Interrupt {
    let mut schema = JsonObject::new();
    schema.insert("type".to_owned(), json!("object"));
    schema.insert(
        "properties".to_owned(),
        json!({"confirm": {"type": "boolean"}}),
    );
    schema.insert("required".to_owned(), json!(["confirm"]));

    Interrupt {
        id: CLEAR_INTERRUPT.to_owned(),
        reason: "tool_approval".to_owned(),
        message: Some(format!("Clear the board? {count} task(s) will be removed.")),
        response_schema: Some(schema),
        ..Default::default()
    }
}

/// Streams one assistant message, a word per `TEXT_MESSAGE_CONTENT`.
fn stream(ctx: &mut RunContext<Board>, text: &str) -> Result<()> {
    let mut message = ctx.assistant_message()?;
    for word in text.split_inclusive(' ') {
        message.delta(word)?;
    }
    message.end()
}

/// Ships the board as an A2UI surface, in a tool result envelope.
///
/// The one call that does not go through [`offered`]: `render_a2ui` is the
/// carrier the A2UI toolkits agreed on, not a tool a frontend offers, and
/// neither AG-UI nor this SDK says whether an agent may call a tool that was
/// never offered. It does here, as `e2e/tests/a2ui_surface.rs` does.
fn render(ctx: &mut RunContext<Board>, intent: Intent) -> Result<()> {
    let spec = board::surface(ctx.state());
    let envelope =
        wrap_as_operations_envelope(&assemble_ops(intent, &spec)).map_err(Error::agent)?;

    let mut call = ctx.tool_call(RENDER_A2UI_TOOL_NAME)?;
    call.args_json(&json!({"surfaceId": board::SURFACE_ID}))?;
    call.result(envelope)?;
    Ok(())
}

/// `Create` the first time the thread renders a surface, `Update` afterwards.
///
/// The agent stores nothing between runs, so the answer comes from the
/// conversation the client sent: the toolkit replays the operations already in
/// history and reports what the user is looking at.
fn surface_intent(messages: &[Message]) -> Intent {
    match find_prior_surface_in(messages) {
        Some(prior) if !prior.deleted => Intent::Update,
        _ => Intent::Create,
    }
}

/// What the user asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// `add draft the agenda, book the room` — one task per comma.
    Add(Vec<String>),
    /// `complete 1`, `complete agenda`.
    Complete(String),
    /// `estimate 2 45`.
    Estimate {
        /// Id or title fragment.
        task: String,
        /// Minutes.
        minutes: u32,
    },
    /// `list`, `board`, `show`.
    List,
    /// `clear`, `reset` — the destructive one.
    Clear,
    /// Anything else.
    Help,
}

impl Command {
    /// Reads one line of chat.
    pub fn parse(said: &str) -> Self {
        let said = said.trim();
        let (verb, rest) = match said.split_once(char::is_whitespace) {
            Some((verb, rest)) => (verb, rest.trim()),
            None => (said, ""),
        };

        match verb.to_lowercase().as_str() {
            "add" | "todo" if !rest.is_empty() => Self::Add(
                rest.split(',')
                    .map(str::trim)
                    .filter(|title| !title.is_empty())
                    .map(str::to_owned)
                    .collect(),
            ),
            "complete" | "done" | "finish" if !rest.is_empty() => Self::Complete(rest.to_owned()),
            "estimate" | "est" => match rest.rsplit_once(char::is_whitespace) {
                Some((task, minutes)) => match minutes.trim_end_matches('m').parse() {
                    Ok(minutes) => Self::Estimate {
                        task: task.trim().to_owned(),
                        minutes,
                    },
                    Err(_) => Self::Help,
                },
                None => Self::Help,
            },
            "list" | "board" | "show" => Self::List,
            "clear" | "reset" => Self::Clear,
            _ => Self::Help,
        }
    }

    /// The one line of reasoning the run publishes about it.
    ///
    /// `answer` is what came back from the interrupt, when this run is a
    /// resumed one — the only command whose plan depends on it is [`Self::Clear`].
    fn plan(&self, answer: Option<ResumeStatus>) -> String {
        match self {
            Self::Add(titles) => format!("adding {} task(s)", titles.len()),
            Self::Complete(needle) => format!("looking for the task matching \"{needle}\""),
            Self::Estimate { task, minutes } => format!("putting {minutes}m on \"{task}\""),
            Self::List => "reading the board back".to_owned(),
            Self::Clear => match answer {
                Some(ResumeStatus::Resolved) => "a human approved clearing the board".to_owned(),
                _ => "a human declined, so the board stays".to_owned(),
            },
            Self::Help => "that is not a command I know".to_owned(),
        }
    }
}
