//! The ordering state machine.
//!
//! `TEXT_MESSAGE_CONTENT` without a preceding `TEXT_MESSAGE_START` is a bug
//! that currently surfaces as a confused frontend, three network hops from
//! where it was caused. Neither the TypeScript SDK (which verifies on the
//! client) nor the .NET one (which does not verify) catches it on the server.
//! This crate does, on by default.
//!
//! # What it rejects
//!
//! | Rule | Rejected |
//! | --- | --- |
//! | [`RunEnded`] | anything after `RUN_FINISHED` / `RUN_ERROR` |
//! | [`DuplicateRunStarted`] | a second `RUN_STARTED` |
//! | [`DuplicateStart`] | opening a message, reasoning block, tool call or step whose id is already open |
//! | [`NotOpen`] | content or a terminator for something that was never opened |
//! | [`UnknownId`] | `TOOL_CALL_RESULT` for a call id that was never introduced |
//! | [`OutOfOrder`] | `TOOL_CALL_RESULT` before the call's `TOOL_CALL_END` |
//! | [`OpenAtFinish`] | `RUN_FINISHED` while a message, reasoning block, tool call or step is open |
//!
//! `RUN_ERROR` is exempt from [`OpenAtFinish`]: a run that blew up mid-message
//! could not have closed it.
//!
//! [`RunEnded`]: crate::Rule::RunEnded
//! [`DuplicateRunStarted`]: crate::Rule::DuplicateRunStarted
//! [`DuplicateStart`]: crate::Rule::DuplicateStart
//! [`NotOpen`]: crate::Rule::NotOpen
//! [`UnknownId`]: crate::Rule::UnknownId
//! [`OutOfOrder`]: crate::Rule::OutOfOrder
//! [`OpenAtFinish`]: crate::Rule::OpenAtFinish
//!
//! # What it lets through
//!
//! The `*_CHUNK` events are self-contained by design, so a chunk carrying a new
//! id registers that id rather than being rejected for having no start. The
//! deprecated `THINKING_*` family is not tracked at all. State, activity, raw
//! and custom events are unordered.
//!
//! # Cost
//!
//! A handful of `HashSet`s and one lookup per event. Turning the `verify`
//! feature off replaces the whole state machine with a zero-sized type whose
//! `observe` is an inlined `Ok(())`. In debug builds a rejection additionally
//! carries a dump of everything still open, which is the expensive part and is
//! why it is debug-only.

#[cfg(feature = "verify")]
pub(crate) use enabled::Verifier;

#[cfg(not(feature = "verify"))]
pub(crate) use disabled::Verifier;

#[cfg(feature = "verify")]
mod enabled {
    use std::collections::HashSet;
    use std::fmt::Write as _;

    use ag_ui_core::{Event, MessageId, StepName, ToolCallId};

    use crate::error::{Rule, VerificationError};

    /// Builds a rejection, appending the open-entity dump in debug builds.
    fn reject(
        event: &Event,
        rule: Rule,
        detail: impl Into<String>,
        open: impl FnOnce() -> String,
    ) -> VerificationError {
        let mut detail = detail.into();
        if cfg!(debug_assertions) {
            detail.push_str(&open());
        }
        VerificationError::new(event.event_type(), rule, detail)
    }

    /// Tracks what is open so misordered events can be named precisely.
    #[derive(Debug, Default)]
    pub(crate) struct Verifier {
        started: bool,
        ended: bool,
        messages: HashSet<MessageId>,
        reasoning: HashSet<MessageId>,
        reasoning_messages: HashSet<MessageId>,
        tool_calls: HashSet<ToolCallId>,
        known_tool_calls: HashSet<ToolCallId>,
        steps: HashSet<StepName>,
    }

    impl Verifier {
        pub(crate) fn new() -> Self {
            Self::default()
        }

        /// Checks `event` against the state machine and folds it in.
        pub(crate) fn observe(&mut self, event: &Event) -> Result<(), VerificationError> {
            if self.ended {
                return Err(self.fail(event, Rule::RunEnded, "the run already ended"));
            }

            match event {
                Event::RunStarted(_) => {
                    if self.started {
                        return Err(self.fail(
                            event,
                            Rule::DuplicateRunStarted,
                            "the run already started",
                        ));
                    }
                    self.started = true;
                }
                Event::RunFinished(_) => {
                    if let Some(detail) = self.first_open() {
                        return Err(self.fail(event, Rule::OpenAtFinish, detail));
                    }
                    self.ended = true;
                }
                Event::RunError(_) => self.ended = true,

                Event::TextMessageStart(payload) => {
                    self.open(event, Kind::Message, &payload.message_id)?;
                }
                Event::TextMessageContent(payload) => {
                    self.require(event, Kind::Message, &payload.message_id)?;
                }
                Event::TextMessageEnd(payload) => {
                    self.close(event, Kind::Message, &payload.message_id)?;
                }
                Event::TextMessageChunk(payload) => {
                    if let Some(id) = &payload.message_id {
                        // Self-contained: a chunk needs no bracketing events.
                        self.messages.remove(id);
                    }
                }

                Event::ReasoningStart(payload) => {
                    self.open(event, Kind::Reasoning, &payload.message_id)?;
                }
                Event::ReasoningEnd(payload) => {
                    self.close(event, Kind::Reasoning, &payload.message_id)?;
                }
                Event::ReasoningMessageStart(payload) => {
                    self.open(event, Kind::ReasoningMessage, &payload.message_id)?;
                }
                Event::ReasoningMessageContent(payload) => {
                    self.require(event, Kind::ReasoningMessage, &payload.message_id)?;
                }
                Event::ReasoningMessageEnd(payload) => {
                    self.close(event, Kind::ReasoningMessage, &payload.message_id)?;
                }
                Event::ReasoningMessageChunk(payload) => {
                    if let Some(id) = &payload.message_id {
                        self.reasoning_messages.remove(id);
                    }
                }

                Event::ToolCallStart(payload) => {
                    let id = &payload.tool_call_id;
                    if !self.tool_calls.insert(id.clone()) {
                        return Err(self.fail(
                            event,
                            Rule::DuplicateStart,
                            format!("tool call {id:?} is already open"),
                        ));
                    }
                    self.known_tool_calls.insert(id.clone());
                }
                Event::ToolCallArgs(payload) => {
                    let id = &payload.tool_call_id;
                    if !self.tool_calls.contains(id) {
                        return Err(self.fail(
                            event,
                            Rule::NotOpen,
                            format!("tool call {id:?} is not open"),
                        ));
                    }
                }
                Event::ToolCallEnd(payload) => {
                    let id = &payload.tool_call_id;
                    if !self.tool_calls.remove(id) {
                        return Err(self.fail(
                            event,
                            Rule::NotOpen,
                            format!("tool call {id:?} is not open"),
                        ));
                    }
                }
                Event::ToolCallChunk(payload) => {
                    if let Some(id) = &payload.tool_call_id {
                        self.tool_calls.remove(id);
                        self.known_tool_calls.insert(id.clone());
                    }
                }
                Event::ToolCallResult(payload) => {
                    let id = &payload.tool_call_id;
                    if !self.known_tool_calls.contains(id) {
                        return Err(self.fail(
                            event,
                            Rule::UnknownId,
                            format!("tool call {id:?} was never started"),
                        ));
                    }
                    if self.tool_calls.contains(id) {
                        return Err(self.fail(
                            event,
                            Rule::OutOfOrder,
                            format!("tool call {id:?} has no TOOL_CALL_END yet"),
                        ));
                    }
                }

                Event::StepStarted(payload) => {
                    let name = &payload.step_name;
                    if !self.steps.insert(name.clone()) {
                        return Err(self.fail(
                            event,
                            Rule::DuplicateStart,
                            format!("step {name:?} is already open"),
                        ));
                    }
                }
                Event::StepFinished(payload) => {
                    let name = &payload.step_name;
                    if !self.steps.remove(name) {
                        return Err(self.fail(
                            event,
                            Rule::NotOpen,
                            format!("step {name:?} is not open"),
                        ));
                    }
                }

                _ => {}
            }

            Ok(())
        }

        fn open(
            &mut self,
            event: &Event,
            kind: Kind,
            id: &MessageId,
        ) -> Result<(), VerificationError> {
            if !self.set_mut(kind).insert(id.clone()) {
                return Err(self.fail(
                    event,
                    Rule::DuplicateStart,
                    format!("{} {id:?} is already open", kind.noun()),
                ));
            }
            Ok(())
        }

        fn require(
            &mut self,
            event: &Event,
            kind: Kind,
            id: &MessageId,
        ) -> Result<(), VerificationError> {
            if !self.set_mut(kind).contains(id) {
                return Err(self.fail(
                    event,
                    Rule::NotOpen,
                    format!("{} {id:?} is not open", kind.noun()),
                ));
            }
            Ok(())
        }

        fn close(
            &mut self,
            event: &Event,
            kind: Kind,
            id: &MessageId,
        ) -> Result<(), VerificationError> {
            if !self.set_mut(kind).remove(id) {
                return Err(self.fail(
                    event,
                    Rule::NotOpen,
                    format!("{} {id:?} is not open", kind.noun()),
                ));
            }
            Ok(())
        }

        fn set_mut(&mut self, kind: Kind) -> &mut HashSet<MessageId> {
            match kind {
                Kind::Message => &mut self.messages,
                Kind::Reasoning => &mut self.reasoning,
                Kind::ReasoningMessage => &mut self.reasoning_messages,
            }
        }

        fn fail(&self, event: &Event, rule: Rule, detail: impl Into<String>) -> VerificationError {
            reject(event, rule, detail, || self.dump())
        }

        /// The first thing still open at `RUN_FINISHED`, if any.
        fn first_open(&self) -> Option<String> {
            if let Some(id) = self.messages.iter().next() {
                return Some(format!("message {id:?} is still open"));
            }
            if let Some(id) = self.reasoning_messages.iter().next() {
                return Some(format!("reasoning message {id:?} is still open"));
            }
            if let Some(id) = self.reasoning.iter().next() {
                return Some(format!("reasoning block {id:?} is still open"));
            }
            if let Some(id) = self.tool_calls.iter().next() {
                return Some(format!("tool call {id:?} is still open"));
            }
            if let Some(name) = self.steps.iter().next() {
                return Some(format!("step {name:?} is still open"));
            }
            None
        }

        /// Debug-build-only dump of everything currently open.
        fn dump(&self) -> String {
            let mut out = String::new();
            let mut push = |label: &str, values: Vec<String>| {
                if !values.is_empty() {
                    let _ = write!(out, " {label}={:?}", values);
                }
            };
            push("messages", strings(&self.messages));
            push("reasoning", strings(&self.reasoning));
            push("reasoning_messages", strings(&self.reasoning_messages));
            push("tool_calls", strings(&self.tool_calls));
            push("steps", strings(&self.steps));
            if out.is_empty() {
                " [nothing open]".to_owned()
            } else {
                format!(" [open:{out}]")
            }
        }
    }

    fn strings<T: ToString>(set: &HashSet<T>) -> Vec<String> {
        let mut values: Vec<String> = set.iter().map(ToString::to_string).collect();
        values.sort();
        values
    }

    /// The three id-keyed things a message id can open.
    #[derive(Clone, Copy, Debug)]
    enum Kind {
        Message,
        Reasoning,
        ReasoningMessage,
    }

    impl Kind {
        const fn noun(self) -> &'static str {
            match self {
                Self::Message => "message",
                Self::Reasoning => "reasoning block",
                Self::ReasoningMessage => "reasoning message",
            }
        }
    }
}

#[cfg(not(feature = "verify"))]
mod disabled {
    use ag_ui_core::Event;

    use crate::error::VerificationError;

    /// The `verify` feature is off: every check compiles away.
    #[derive(Debug, Default)]
    pub(crate) struct Verifier;

    impl Verifier {
        #[inline]
        pub(crate) fn new() -> Self {
            Self
        }

        #[inline]
        pub(crate) fn observe(&mut self, _event: &Event) -> Result<(), VerificationError> {
            Ok(())
        }
    }
}

#[cfg(all(test, feature = "verify"))]
mod tests {
    use super::*;
    use ag_ui_core::{Event, TextMessageRole};

    use crate::error::Rule;

    fn verifier() -> Verifier {
        let mut verifier = Verifier::new();
        verifier
            .observe(&Event::run_started("t", "r"))
            .expect("RUN_STARTED must be accepted");
        verifier
    }

    #[test]
    fn a_well_formed_run_passes() {
        let mut verifier = verifier();
        for event in [
            Event::step_started("plan"),
            Event::text_message_start("m1", TextMessageRole::Assistant),
            Event::text_message_content("m1", "hi"),
            Event::text_message_end("m1"),
            Event::tool_call_start("c1", "search"),
            Event::tool_call_args("c1", "{}"),
            Event::tool_call_end("c1"),
            Event::tool_call_result("m2", "c1", "ok"),
            Event::step_finished("plan"),
            Event::run_finished_success("t", "r"),
        ] {
            verifier
                .observe(&event)
                .unwrap_or_else(|error| panic!("{event:?} should be accepted: {error}"));
        }
    }

    #[test]
    fn debug_builds_include_the_open_dump() {
        let mut verifier = verifier();
        verifier
            .observe(&Event::text_message_start("m1", TextMessageRole::Assistant))
            .expect("start");
        let error = verifier
            .observe(&Event::text_message_content("m2", "hi"))
            .expect_err("m2 was never opened");
        assert_eq!(error.rule, Rule::NotOpen);
        assert!(
            error.detail.contains("m1"),
            "debug dump should name the open message: {}",
            error.detail
        );
    }
}
