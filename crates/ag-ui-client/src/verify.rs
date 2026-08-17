//! Client-side protocol verification.
//!
//! The TypeScript SDK puts its verifier on the *client*, and that is the right
//! instinct for a consumer: the events arrive from someone else's process, and
//! a stream that breaks the rules should produce one clear error rather than a
//! confused UI. This module is that check, as an ordering state machine.
//!
//! [`crate::Session`] runs it by default. Turn it off with
//! [`SessionBuilder::verify`](crate::SessionBuilder::verify) when talking to a
//! producer whose quirks you have decided to live with.
//!
//! ```
//! use ag_ui_client::verify::Verifier;
//! use ag_ui_core::Event;
//!
//! let mut verifier = Verifier::new();
//! verifier.verify(&Event::run_started("thread-1", "run-1"))?;
//!
//! // Content for a message that was never opened.
//! let orphan = Event::text_message_content("msg-1", "Hello");
//! assert!(verifier.verify(&orphan).is_err());
//! # Ok::<(), ag_ui_client::Error>(())
//! ```
//!
//! # The rules
//!
//! 1. `RUN_STARTED` opens the stream, and does so exactly once. Only `RAW` and
//!    `CUSTOM` may precede it.
//! 2. `RUN_FINISHED` and `RUN_ERROR` close it. Nothing may follow.
//! 3. `TEXT_MESSAGE_CONTENT` and `TEXT_MESSAGE_END` require an open message
//!    with the same id, and `TEXT_MESSAGE_START` may not open a second one.
//! 4. The same, for `TOOL_CALL_*` and for `REASONING_MESSAGE_*`.
//! 5. While a message, tool call or reasoning message is open, no *other*
//!    message-stream event may interleave. State, activity, step, raw and
//!    custom events may: a producer that streams chunks legitimately publishes
//!    state between two fragments of one message.
//! 6. `STEP_FINISHED` requires a matching `STEP_STARTED`, and step names do not
//!    nest with themselves.
//! 7. Everything open must be closed before `RUN_FINISHED`.
//! 8. An `interrupt` outcome must carry at least one interrupt — the one rule
//!    the type system cannot express, checked by
//!    [`RunOutcome::validate`](ag_ui_core::RunOutcome::validate).
//!
//! Rule 5 is deliberately narrower than the TypeScript verifier's, which
//! forbids *any* event between a start and its end. That rule fails against
//! real chunk-streaming producers, and a verifier that cries wolf gets turned
//! off.

// The THINKING_* events are deprecated but a verifier still has to recognise
// them.
#![allow(deprecated)]

use ag_ui_core::{Event, EventType, MessageId, StepName, ToolCallId};

use crate::error::{Error, Result};

/// An ordering state machine for one run's event stream.
///
/// One verifier per run: it is stateful, and its state is that run's progress.
#[derive(Clone, Debug, Default)]
pub struct Verifier {
    started: bool,
    finished: bool,
    text: Option<MessageId>,
    tool: Option<ToolCallId>,
    reasoning: Option<MessageId>,
    steps: Vec<StepName>,
}

impl Verifier {
    /// A verifier for a stream that has not started yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the run has reached a terminal event.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Checks one event against the rules, and records it.
    ///
    /// # Errors
    ///
    /// [`Error::Protocol`], naming the rule that was broken.
    pub fn verify(&mut self, event: &Event) -> Result<()> {
        let kind = event.event_type();

        if self.finished {
            return Err(Error::protocol(format!(
                "{kind} arrived after the run had already finished"
            )));
        }

        // RAW and CUSTOM are outside the protocol's vocabulary by definition,
        // so they are outside its ordering too.
        if matches!(kind, EventType::Raw | EventType::Custom) {
            return Ok(());
        }

        if !self.started && kind != EventType::RunStarted {
            return Err(Error::protocol(format!(
                "{kind} arrived before RUN_STARTED"
            )));
        }

        match event {
            Event::RunStarted(_) => {
                if self.started {
                    return Err(Error::protocol("RUN_STARTED arrived twice in one stream"));
                }
                self.started = true;
            }

            Event::TextMessageStart(e) => {
                self.no_open_stream(kind)?;
                self.text = Some(e.message_id.clone());
            }
            Event::TextMessageContent(e) => self.expect_text(&e.message_id, kind)?,
            Event::TextMessageEnd(e) => {
                self.expect_text(&e.message_id, kind)?;
                self.text = None;
            }

            Event::ToolCallStart(e) => {
                self.no_open_stream(kind)?;
                self.tool = Some(e.tool_call_id.clone());
            }
            Event::ToolCallArgs(e) => self.expect_tool(&e.tool_call_id, kind)?,
            Event::ToolCallEnd(e) => {
                self.expect_tool(&e.tool_call_id, kind)?;
                self.tool = None;
            }
            Event::ToolCallResult(_) => self.no_open_stream(kind)?,

            Event::ReasoningMessageStart(e) => {
                self.no_open_stream(kind)?;
                self.reasoning = Some(e.message_id.clone());
            }
            Event::ReasoningMessageContent(e) => self.expect_reasoning(&e.message_id, kind)?,
            Event::ReasoningMessageEnd(e) => {
                self.expect_reasoning(&e.message_id, kind)?;
                self.reasoning = None;
            }

            Event::StepStarted(e) => {
                if self.steps.contains(&e.step_name) {
                    return Err(Error::protocol(format!(
                        "STEP_STARTED for {:?}, which is already running",
                        e.step_name.as_str()
                    )));
                }
                self.steps.push(e.step_name.clone());
            }
            Event::StepFinished(e) => match self.steps.iter().position(|name| name == &e.step_name)
            {
                Some(index) => {
                    self.steps.remove(index);
                }
                None => {
                    return Err(Error::protocol(format!(
                        "STEP_FINISHED for {:?}, which never started",
                        e.step_name.as_str()
                    )));
                }
            },

            Event::RunFinished(e) => {
                // Recorded before the checks: the run is over whether or not it
                // ended tidily, and reporting the untidiness twice — once here
                // and again from `finish` — helps nobody.
                self.finished = true;
                self.expect_all_closed("RUN_FINISHED")?;
                if let Some(outcome) = &e.outcome {
                    outcome.validate()?;
                }
            }
            Event::RunError(_) => {
                // A failing run is allowed to abandon whatever it had open —
                // that is what failing means.
                self.finished = true;
            }

            // Chunk events are self-contained; expanding them into brackets is
            // [`crate::chunks`]'s job, and it runs before this one.
            _ => {}
        }

        Ok(())
    }

    /// Checks the stream ended where it was supposed to.
    ///
    /// # Errors
    ///
    /// [`Error::Protocol`] when the transport ended the stream before the agent
    /// finished the run — a truncated response, which otherwise looks exactly
    /// like a short answer.
    pub fn finish(&self) -> Result<()> {
        if !self.started {
            return Err(Error::protocol("the stream ended before RUN_STARTED"));
        }
        if !self.finished {
            return Err(Error::protocol(
                "the stream ended before RUN_FINISHED or RUN_ERROR",
            ));
        }
        Ok(())
    }

    // ---- rules ----------------------------------------------------------

    fn expect_text(&self, id: &MessageId, kind: EventType) -> Result<()> {
        match &self.text {
            Some(open) if open == id => Ok(()),
            Some(open) => Err(Error::protocol(format!(
                "{kind} for message {:?}, but message {:?} is the open one",
                id.as_str(),
                open.as_str()
            ))),
            None => Err(Error::protocol(format!(
                "{kind} for message {:?}, which was never opened",
                id.as_str()
            ))),
        }
    }

    fn expect_tool(&self, id: &ToolCallId, kind: EventType) -> Result<()> {
        match &self.tool {
            Some(open) if open == id => Ok(()),
            Some(open) => Err(Error::protocol(format!(
                "{kind} for tool call {:?}, but tool call {:?} is the open one",
                id.as_str(),
                open.as_str()
            ))),
            None => Err(Error::protocol(format!(
                "{kind} for tool call {:?}, which was never opened",
                id.as_str()
            ))),
        }
    }

    fn expect_reasoning(&self, id: &MessageId, kind: EventType) -> Result<()> {
        match &self.reasoning {
            Some(open) if open == id => Ok(()),
            Some(open) => Err(Error::protocol(format!(
                "{kind} for reasoning message {:?}, but reasoning message {:?} is the open one",
                id.as_str(),
                open.as_str()
            ))),
            None => Err(Error::protocol(format!(
                "{kind} for reasoning message {:?}, which was never opened",
                id.as_str()
            ))),
        }
    }

    /// Rejects an event that opens a stream while another one is open.
    fn no_open_stream(&self, kind: EventType) -> Result<()> {
        if let Some(id) = &self.text {
            return Err(Error::protocol(format!(
                "{kind} arrived while message {:?} was still open",
                id.as_str()
            )));
        }
        if let Some(id) = &self.tool {
            return Err(Error::protocol(format!(
                "{kind} arrived while tool call {:?} was still open",
                id.as_str()
            )));
        }
        if let Some(id) = &self.reasoning {
            return Err(Error::protocol(format!(
                "{kind} arrived while reasoning message {:?} was still open",
                id.as_str()
            )));
        }
        Ok(())
    }

    fn expect_all_closed(&self, what: &str) -> Result<()> {
        if let Some(id) = &self.text {
            return Err(Error::protocol(format!(
                "{what} arrived while message {:?} was still open",
                id.as_str()
            )));
        }
        if let Some(id) = &self.tool {
            return Err(Error::protocol(format!(
                "{what} arrived while tool call {:?} was still open",
                id.as_str()
            )));
        }
        if let Some(id) = &self.reasoning {
            return Err(Error::protocol(format!(
                "{what} arrived while reasoning message {:?} was still open",
                id.as_str()
            )));
        }
        if let Some(name) = self.steps.first() {
            return Err(Error::protocol(format!(
                "{what} arrived while step {:?} was still running",
                name.as_str()
            )));
        }
        Ok(())
    }
}

/// Verifies a whole run in one call.
///
/// The streaming form is [`Verifier`]; this is the convenience for recorded
/// streams and tests.
pub fn verify_all<'a>(events: impl IntoIterator<Item = &'a Event>) -> Result<()> {
    let mut verifier = Verifier::new();
    for event in events {
        verifier.verify(event)?;
    }
    verifier.finish()
}
