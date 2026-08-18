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
//!    with the same id, and `TEXT_MESSAGE_START` may not re-open an id that is
//!    already open.
//! 4. The same, for `TOOL_CALL_*` and for `REASONING_MESSAGE_*`.
//! 5. `TOOL_CALL_RESULT` may not answer a call that has not ended.
//! 6. `STEP_FINISHED` requires a matching `STEP_STARTED`, and step names do not
//!    nest with themselves.
//! 7. Everything open must be closed before `RUN_FINISHED`.
//! 8. An `interrupt` outcome must carry at least one interrupt — the one rule
//!    the type system cannot express, checked by
//!    [`RunOutcome::validate`](https://kimsoungryoul.github.io/ag-ui-rust/api/ag_ui_core/outcome/enum.RunOutcome.html#method.validate).
//!
//! What is deliberately *not* a rule: that one stream must close before the
//! next opens. Everything here is keyed by id, exactly as the TypeScript
//! verifier keys its `activeMessages` / `activeToolCalls` maps. Two messages
//! may stream at once, two tool calls may stream at once, and a tool call may
//! open inside the message that narrates it — which is what every provider
//! doing parallel tool calls actually sends. Events outside these families
//! (state, activity, step, raw, custom) are unordered and never close
//! anything.

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
    /// What is open, by id — several at once is legal, the same id twice is
    /// not. `Vec` rather than a set so a complaint names whichever was opened
    /// first, which is the one a human is looking for.
    text: Vec<MessageId>,
    tool: Vec<ToolCallId>,
    reasoning: Vec<MessageId>,
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
                self.not_already_open(&self.text, &e.message_id, "message", kind)?;
                self.text.push(e.message_id.clone());
            }
            Event::TextMessageContent(e) => self.expect_text(&e.message_id, kind)?,
            Event::TextMessageEnd(e) => {
                self.expect_text(&e.message_id, kind)?;
                self.text.retain(|open| open != &e.message_id);
            }

            Event::ToolCallStart(e) => {
                self.not_already_open(&self.tool, &e.tool_call_id, "tool call", kind)?;
                self.tool.push(e.tool_call_id.clone());
            }
            Event::ToolCallArgs(e) => self.expect_tool(&e.tool_call_id, kind)?,
            Event::ToolCallEnd(e) => {
                self.expect_tool(&e.tool_call_id, kind)?;
                self.tool.retain(|open| open != &e.tool_call_id);
            }
            // The call this answers has to be over. Anything *else* still
            // streaming is none of this event's business — a result arriving
            // while the assistant keeps narrating is ordinary.
            Event::ToolCallResult(e) => {
                if self.tool.contains(&e.tool_call_id) {
                    return Err(Error::protocol(format!(
                        "{kind} for tool call {:?}, which has not ended yet",
                        e.tool_call_id.as_str()
                    )));
                }
            }

            Event::ReasoningMessageStart(e) => {
                self.not_already_open(&self.reasoning, &e.message_id, "reasoning message", kind)?;
                self.reasoning.push(e.message_id.clone());
            }
            Event::ReasoningMessageContent(e) => self.expect_reasoning(&e.message_id, kind)?,
            Event::ReasoningMessageEnd(e) => {
                self.expect_reasoning(&e.message_id, kind)?;
                self.reasoning.retain(|open| open != &e.message_id);
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
        if self.text.contains(id) {
            return Ok(());
        }
        Err(Error::protocol(format!(
            "{kind} for message {:?}, which was never opened",
            id.as_str()
        )))
    }

    fn expect_tool(&self, id: &ToolCallId, kind: EventType) -> Result<()> {
        if self.tool.contains(id) {
            return Ok(());
        }
        Err(Error::protocol(format!(
            "{kind} for tool call {:?}, which was never opened",
            id.as_str()
        )))
    }

    fn expect_reasoning(&self, id: &MessageId, kind: EventType) -> Result<()> {
        if self.reasoning.contains(id) {
            return Ok(());
        }
        Err(Error::protocol(format!(
            "{kind} for reasoning message {:?}, which was never opened",
            id.as_str()
        )))
    }

    /// Rejects a start for an id that is already streaming.
    ///
    /// The whole of the concurrency rule: two ids may overlap, one id may not
    /// overlap itself.
    fn not_already_open<T: PartialEq + AsRef<str>>(
        &self,
        open: &[T],
        id: &T,
        what: &str,
        kind: EventType,
    ) -> Result<()> {
        if open.contains(id) {
            return Err(Error::protocol(format!(
                "{kind} for {what} {:?}, which is already open",
                id.as_ref()
            )));
        }
        Ok(())
    }

    fn expect_all_closed(&self, what: &str) -> Result<()> {
        if let Some(id) = self.text.first() {
            return Err(Error::protocol(format!(
                "{what} arrived while message {:?} was still open",
                id.as_str()
            )));
        }
        if let Some(id) = self.tool.first() {
            return Err(Error::protocol(format!(
                "{what} arrived while tool call {:?} was still open",
                id.as_str()
            )));
        }
        if let Some(id) = self.reasoning.first() {
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
