//! Where a run is: what it has open, and what a client that joins it is sent
//! first.
//!
//! A run that outlives the response that started it is read by clients that
//! were not there when it began: a second window on the same thread, a client
//! that lost its connection and came back. Such a client cannot be handed the
//! run's stream from where it now stands. A stream begins with `RUN_STARTED`,
//! or with `RUN_ERROR` for a run that failed before it began, and a client
//! rejects any other first event. It rejects content for a message it never
//! saw open in the same way.
//!
//! [`RunProgress`] folds the events of a run, as they are sent, into what the
//! run has open and what each open thing holds so far. From that it builds
//! the [`Prelude`]: the events that take a client to where the run is. The
//! run's later events then follow as they are.
//!
//! It needs no [`Runner`](crate::server::Runner). A server that builds its
//! own events observes each one here as it sends it, or replays the ones it
//! stored.
//!
//! ```
//! # use ag_ui::{Event, Message, TextMessageRole};
//! # use ag_ui::server::RunProgress;
//! // The run so far, as stored. The answer is half written.
//! let stored = [
//!     Event::run_started("thread-1", "run-1"),
//!     Event::step_started("answer"),
//!     Event::text_message_start("msg-2", TextMessageRole::Assistant),
//!     Event::text_message_content("msg-2", "The capital "),
//!     Event::text_message_content("msg-2", "of France"),
//! ];
//! let mut progress = RunProgress::new();
//! progress.replay(&stored)?;
//!
//! // The messages that are complete: here, the question the run answers.
//! let question = Message::user("msg-1", "What is the capital of France?");
//! let prelude = progress.prelude().messages(vec![question.clone()]).into_events();
//!
//! assert_eq!(
//!     prelude,
//!     [
//!         Event::run_started("thread-1", "run-1"),
//!         Event::messages_snapshot(vec![question]),
//!         Event::step_started("answer"),
//!         Event::text_message_start("msg-2", TextMessageRole::Assistant),
//!         Event::text_message_content("msg-2", "The capital of France"),
//!     ]
//! );
//! // Then every event the run sends from here on: " is Paris.", the end of
//! // the message, the end of the step, `RUN_FINISHED`.
//! # Ok::<(), ag_ui::server::VerificationError>(())
//! ```
//!
//! # What a prelude holds
//!
//! In this order, and nothing else:
//!
//! 1. `RUN_STARTED` as the run sent it, without the input echo. A prelude
//!    always begins with it.
//! 2. `MESSAGES_SNAPSHOT`, when [`Prelude::messages`] was given the
//!    conversation, and `STATE_SNAPSHOT`, when [`Prelude::state`] was given
//!    the state. Neither is sent unless asked for.
//! 3. `SUBAGENT_STARTED` for each subagent that is active. A finished one is
//!    restated, with the event that closed it, only when an active one names
//!    it as its parent: a client rejects an announcement whose parent it
//!    never saw.
//! 4. Each thing that is open, in the order they were opened. A step is its
//!    `STEP_STARTED` and a reasoning block its `REASONING_START`. A text
//!    message, a tool call and a reasoning message are their start and one
//!    content event carrying everything produced since.
//! 5. The terminal event, once the run has ended.
//!
//! A tool call that has ended and waits for its result adds nothing. It is
//! in the conversation, and a client accepts a `TOOL_CALL_RESULT` for a call
//! it did not see open.
//!
//! An entity that a `*_CHUNK` event opened is restated as one chunk, not as
//! a start and a content event. A chunk stream ends when something else
//! arrives in its lane, and an entity opened explicitly would not: the
//! client would still hold it open at `RUN_FINISHED`.
//!
//! Every restated event carries what the event it stands for carried: the
//! `subagentRunId`, the timestamp, and the metadata of the start merged with
//! that of the content events since, as a client merges them. `rawEvent` is
//! left out.
//!
//! # The prelude or the stored events, never both
//!
//! Replaying is not idempotent. A client appends: a `*_START` for an id it
//! already holds creates nothing, and the content event after it is added to
//! what is there. So a client is sent the prelude and then the events that
//! follow it, or the stored events from the first, and never the stored
//! events after a prelude that already accounts for them.
//!
//! # The conversation is yours to supply
//!
//! A prelude restates what is open. What is closed — the earlier turns, the
//! messages this run has finished, the tool calls that ended and their
//! results — is the conversation, and the server holds it: this module does
//! not rebuild a message list from events. Give it to
//! [`Prelude::messages`].
//!
//! A server that stores events and not messages folds them with the
//! client's `Applier` (`ag_ui::client::apply::Applier`, behind the `client`
//! feature), the one implementation of that fold:
//!
//! ```
//! # #[cfg(feature = "client")] {
//! use ag_ui::client::apply::Applier;
//! use ag_ui::server::RunProgress;
//! use ag_ui::{Event, Message, TextMessageRole, ToolCallStartEvent};
//!
//! // The request's history, and the run so far: the assistant said what it
//! // would do, and is half way through the arguments of a search.
//! let history = vec![Message::user("msg-1", "Find me a flight to Lisbon.")];
//! let call = ToolCallStartEvent::new("call-1", "search").with_parent_message_id("msg-2");
//! let stored = [
//!     Event::run_started("thread-1", "run-1"),
//!     Event::text_message_start("msg-2", TextMessageRole::Assistant),
//!     Event::text_message_content("msg-2", "Searching."),
//!     Event::text_message_end("msg-2"),
//!     Event::ToolCallStart(call.clone()),
//!     Event::tool_call_args("call-1", r#"{"to":"#),
//! ];
//!
//! // The messages, as a client that read the run from the start holds them.
//! let mut applier = Applier::new().with_messages(history.clone());
//! let mut progress = RunProgress::new();
//! progress.seed(&history);
//! for event in &stored {
//!     applier.apply(event)?;
//!     progress.observe(event)?;
//! }
//!
//! let prelude = progress.prelude().messages(applier.messages()).into_events();
//!
//! // The snapshot holds the call without its arguments, and the stream
//! // restates them: nothing reaches the client twice.
//! let Event::MessagesSnapshot(snapshot) = &prelude[1] else { unreachable!() };
//! let Message::Assistant(assistant) = &snapshot.messages[1] else { unreachable!() };
//! assert_eq!(assistant.content.as_deref(), Some("Searching."));
//! assert_eq!(assistant.tool_calls.as_ref().unwrap()[0].function.arguments, "");
//! assert_eq!(
//!     prelude[2..],
//!     [Event::ToolCallStart(call), Event::tool_call_args("call-1", r#"{"to":"#)]
//! );
//! # }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! An `Applier` applies the explicit form. A stream that carries `*_CHUNK`
//! events goes through the client's `ChunkNormalizer` first.
//!
//! # What is open is taken out of the messages
//!
//! A message list folded from the events holds the open message, partly
//! written, and the open tool call, with part of its arguments. Sent as it
//! is and followed by the start and everything so far, it would give the
//! client that text twice.
//!
//! So a prelude does one thing with the messages it is given, whatever they
//! hold: it takes out what it restates. An open text message keeps its place
//! in the list and whatever else it carries — the tool calls it made that
//! have ended — and loses its text. An open tool call stays in its assistant
//! message, without its arguments. An open reasoning message loses its
//! content. The start and the content event that follow then fill them in,
//! on every client and from any list: one that holds the open entities as
//! they stand, one that holds an earlier copy, one that leaves them out.
//!
//! The other way round was possible: keep the open entities in the
//! snapshot, set to what the run has produced, and send each start alone.
//! That works only for an entity the list already holds. One it leaves out
//! would have to be put in, which needs a place in the list, for a tool call
//! with no parent a message id that clients derive each in their own way,
//! and for reasoning would turn a snapshot that says nothing about
//! reasoning into one that declares all of it. Restating needs none of
//! that, so it is the only behaviour.
//!
//! A client that already holds a different copy of an open entity ends in
//! the same place. The snapshot replaces its copy of a message the snapshot
//! carries and drops one it leaves out, and the restated content then lands
//! on an empty entity. Two cases are beyond what a prelude can do:
//!
//! - **An entity opened again.** A start for an id that was open before —
//!   earlier in the run, or in the history — adds to what the entity held.
//!   The fold knows what was produced since that start and not what was
//!   there before it, so it takes out the supplied text only where that text
//!   ends with what was produced since. A list folded from the same events
//!   does.
//! - **Reasoning the snapshot is silent about.** A client keeps its
//!   reasoning messages through a snapshot that carries none, so a client
//!   that holds part of an open reasoning message keeps that part, and the
//!   restated content is added to it. Supply the reasoning messages in the
//!   list when clients come back holding some.
//!
//! # A client that holds the run up to a point
//!
//! [`Prelude::reopen_only`] is for a client whose position in the stream is
//! known, such as one that reconnects naming the last event it received.
//! Replay the stored events up to that point into a `RunProgress` and it
//! gives each start alone, with no snapshot: the client holds what is open
//! exactly as it stood there, and is sent the events it missed next.
//!
//! # What is not restated
//!
//! State, unless [`Prelude::state`] is given it: a `STATE_DELTA` that
//! follows needs the state it patches. Activities, which are messages and
//! belong in the list. A reasoning message that has ended inside a block
//! that has not: it is closed, and so part of the conversation.

use std::collections::HashSet;

use serde_json::Value;

use crate::event::BaseEvent;
use crate::metadata::merge_metadata_into;
use crate::server::error::VerificationError;
use crate::server::verify::Verifier;
use crate::streams::Streams;
use crate::{
    Event, InputContent, Message, MessageId, ReasoningMessageChunkEvent,
    ReasoningMessageStartEvent, ReasoningStartEvent, RunAgentInput, RunStartedEvent,
    StepStartedEvent, SubagentRunId, SubagentStartedEvent, TextMessageChunkEvent,
    TextMessageStartEvent, ToolCallChunkEvent, ToolCallId, ToolCallStartEvent, ToolContent,
    UserContent,
};

/// The events of a run so far, folded into where the run is.
///
/// Feed it every event of the run in the order they are sent, with
/// [`observe`](Self::observe) or [`replay`](Self::replay). It keeps no log:
/// what it holds is what is open, the text each open thing has produced, and
/// the ids the run has used.
///
/// It is also the ordering check. With the `verify` feature, `observe`
/// rejects what [`EventVerifier`](crate::server::verify) rejects and leaves
/// the fold as it was, so one `RunProgress` can stand where a server that
/// builds its own events has its verifier. Without the feature nothing is
/// checked and the fold takes what it is given.
///
/// See the [module docs](self) for what a prelude is and how it treats the
/// messages it is given.
#[derive(Clone, Debug, Default)]
pub struct RunProgress {
    verifier: Verifier,
    /// The chunk streams being assembled: what tells which events a chunk
    /// stands for, and which entities are open in the chunked spelling.
    streams: Streams,
    /// `RUN_STARTED`, without its input echo.
    started: Option<RunStartedEvent>,
    /// The terminal events: `RUN_FINISHED` or `RUN_ERROR`, or the first
    /// followed by the second.
    ended: Vec<Event>,
    /// What is open, in the order it was opened.
    open: Vec<Slot>,
    /// Every subagent the run announced, in the order they started.
    subagents: Vec<Announced>,
    seen: Seen,
}

/// Something a run has opened and not closed.
///
/// `start` is the event that opened it, without its `rawEvent`, and with the
/// metadata of the content events since merged into its own: what a client
/// that read them holds on the entity.
///
/// Where there is a `chunked` flag, it says the entity was opened by a
/// `*_CHUNK` event. Its `start` is then the start that chunk stood for, and
/// the entity has no end event to wait for: it ends when its chunk stream
/// does. An explicit content or end event naming it would be rejected.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum OpenEntity {
    /// A text message: `TEXT_MESSAGE_START` with no `TEXT_MESSAGE_END` yet.
    #[non_exhaustive]
    Message {
        /// The event that opened it.
        start: TextMessageStartEvent,
        /// The text produced since it was opened.
        content: String,
        /// Whether a `TEXT_MESSAGE_CHUNK` opened it.
        chunked: bool,
    },
    /// A tool call whose arguments are streaming: `TOOL_CALL_START` with no
    /// `TOOL_CALL_END` yet. A call that has ended and waits for its result
    /// is not open.
    #[non_exhaustive]
    ToolCall {
        /// The event that opened it.
        start: ToolCallStartEvent,
        /// The argument JSON produced since it was opened. Rarely valid JSON
        /// yet.
        args: String,
        /// Whether a `TOOL_CALL_CHUNK` opened it.
        chunked: bool,
    },
    /// A reasoning block: `REASONING_START` with no `REASONING_END` yet. The
    /// message inside it is a separate entry, open or not on its own.
    #[non_exhaustive]
    ReasoningBlock {
        /// The event that opened it.
        start: ReasoningStartEvent,
    },
    /// A reasoning message: `REASONING_MESSAGE_START` with no
    /// `REASONING_MESSAGE_END` yet.
    #[non_exhaustive]
    ReasoningMessage {
        /// The event that opened it.
        start: ReasoningMessageStartEvent,
        /// The reasoning text produced since it was opened.
        content: String,
        /// Whether a `REASONING_MESSAGE_CHUNK` opened it.
        chunked: bool,
    },
    /// A step: `STEP_STARTED` with no `STEP_FINISHED` yet.
    #[non_exhaustive]
    Step {
        /// The event that opened it.
        start: StepStartedEvent,
    },
    /// An active subagent: `SUBAGENT_STARTED` with neither
    /// `SUBAGENT_FINISHED` nor `SUBAGENT_ERROR` yet.
    #[non_exhaustive]
    Subagent {
        /// The event that announced it.
        start: SubagentStartedEvent,
    },
}

/// An open entity and what the fold knows about it beyond what it shows.
#[derive(Clone, Debug)]
struct Slot {
    entity: OpenEntity,
    /// Whether the id was in use before this opening, in the history or
    /// earlier in the run. A start for such an id adds to what the entity
    /// held, and what that was the fold does not know.
    reopened: bool,
}

/// One subagent's lifecycle, as the run sent it.
#[derive(Clone, Debug)]
struct Announced {
    start: SubagentStartedEvent,
    /// `SUBAGENT_FINISHED` or `SUBAGENT_ERROR`, once it has closed.
    end: Option<Event>,
}

/// The ids that have named an entity able to hold streamed content.
#[derive(Clone, Debug, Default)]
struct Seen {
    messages: HashSet<MessageId>,
    tool_calls: HashSet<ToolCallId>,
    reasoning: HashSet<MessageId>,
}

impl Seen {
    /// Records the ids a restated conversation carries.
    fn conversation(&mut self, messages: &[Message]) {
        for message in messages {
            let ids = match message {
                Message::Reasoning(_) => &mut self.reasoning,
                _ => &mut self.messages,
            };
            ids.insert(message.id().clone());
            if let Message::Assistant(assistant) = message {
                for call in assistant.tool_calls.iter().flatten() {
                    self.tool_calls.insert(call.id.clone());
                }
            }
        }
    }
}

impl RunProgress {
    /// The progress of a run that has sent nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Learns the conversation the run continues — the request's `messages`
    /// — before its first event.
    ///
    /// For the reason [`EventVerifier::seed`](crate::server::verify) gives:
    /// a result in the run may answer a call the history made. It also tells
    /// the fold which ids were in use before the run, which is what lets it
    /// recognise a message that is opened again.
    pub fn seed(&mut self, history: &[Message]) {
        self.verifier.seed(history);
        self.seen.conversation(history);
    }

    /// Checks one event the run is sending, and folds it in.
    ///
    /// # Errors
    ///
    /// With the `verify` feature, the rule the event broke. A rejected event
    /// was not sent, so it changes nothing here either.
    pub fn observe(&mut self, event: &Event) -> Result<(), VerificationError> {
        self.verifier.observe(event)?;
        self.fold(event);
        Ok(())
    }

    /// Observes the events a run has already sent, in order.
    ///
    /// For a server that reads them from its store: when a client joins, or
    /// when it takes over a run another process was driving. Afterwards the
    /// progress is where the run is, and [`observe`](Self::observe) carries
    /// on from there.
    ///
    /// # Errors
    ///
    /// The first event that breaks a rule, as `observe` reports it. The
    /// events before it stay folded in.
    pub fn replay<'a>(
        &mut self,
        events: impl IntoIterator<Item = &'a Event>,
    ) -> Result<(), VerificationError> {
        events.into_iter().try_for_each(|event| self.observe(event))
    }

    /// Whether the run has sent its `RUN_STARTED`.
    pub fn has_started(&self) -> bool {
        self.started.is_some()
    }

    /// Whether the run has sent a terminal event. Nothing follows one but a
    /// late `RUN_ERROR`, and a client that joins now is sent the prelude
    /// alone.
    pub fn has_ended(&self) -> bool {
        !self.ended.is_empty()
    }

    /// What the run has open, in the order it was opened.
    ///
    /// After `RUN_ERROR` this is what the run abandoned.
    ///
    /// ```
    /// # use ag_ui::{Event, TextMessageRole};
    /// # use ag_ui::server::{OpenEntity, RunProgress};
    /// let mut progress = RunProgress::new();
    /// progress.replay(&[
    ///     Event::run_started("thread-1", "run-1"),
    ///     Event::step_started("draft"),
    ///     Event::text_message_start("msg-1", TextMessageRole::Assistant),
    ///     Event::text_message_content("msg-1", "Dear "),
    ///     Event::text_message_content("msg-1", "Ada,"),
    /// ])?;
    ///
    /// let open: Vec<&OpenEntity> = progress.open().collect();
    /// assert!(matches!(open[0], OpenEntity::Step { start, .. } if start.step_name.as_str() == "draft"));
    /// assert!(matches!(open[1], OpenEntity::Message { content, .. } if content == "Dear Ada,"));
    /// # Ok::<(), ag_ui::server::VerificationError>(())
    /// ```
    pub fn open(&self) -> impl Iterator<Item = &OpenEntity> {
        self.open.iter().map(|slot| &slot.entity)
    }

    /// Starts the prelude for a client that joins the run where it now is.
    ///
    /// See the [module docs](self) for what it holds. It is empty for a run
    /// that has not started: there is nothing to catch up on, and the client
    /// reads the run from its first event.
    pub fn prelude(&self) -> Prelude<'_> {
        Prelude {
            progress: self,
            input: None,
            messages: None,
            state: None,
            restate: true,
        }
    }

    // ---- the fold -------------------------------------------------------

    /// Folds one accepted event in, reading it as the ordering rules do: a
    /// chunk as the events it stands for, and any event as first ending the
    /// chunk streams it ends.
    fn fold(&mut self, event: &Event) {
        let is_chunk = matches!(
            event,
            Event::TextMessageChunk(_) | Event::ToolCallChunk(_) | Event::ReasoningMessageChunk(_)
        );
        if !is_chunk && self.streams.is_idle() {
            return self.record(event, false);
        }
        let mut stood_for = Vec::new();
        if is_chunk {
            // The rules accepted it, so it can be placed. With the `verify`
            // feature off nothing vouches for it, and a chunk that cannot be
            // placed stands for nothing.
            if self.streams.expand(event.clone(), &mut stood_for).is_err() {
                return;
            }
        } else {
            self.streams.end_for(event, &mut stood_for);
        }
        for implied in &stood_for {
            self.record(implied, true);
        }
        if !is_chunk {
            self.record(event, false);
        }
    }

    /// Folds in one event of the explicit form. `chunked` says it is one a
    /// chunk stood for.
    fn record(&mut self, event: &Event, chunked: bool) {
        match event {
            Event::RunStarted(e) => {
                if let Some(input) = &e.input {
                    self.seen.conversation(&input.messages);
                }
                if self.started.is_none() {
                    // Field by field, so the input echo, the largest payload
                    // in the protocol, is not copied only to be dropped.
                    self.started = Some(RunStartedEvent {
                        base: restated(&e.base),
                        thread_id: e.thread_id.clone(),
                        run_id: e.run_id.clone(),
                        protocol_version: e.protocol_version.clone(),
                        parent_run_id: e.parent_run_id.clone(),
                        input: None,
                    });
                }
            }
            Event::RunFinished(_) | Event::RunError(_) => {
                let mut terminal = event.clone();
                terminal.base_mut().raw_event = None;
                self.ended.push(terminal);
            }
            Event::MessagesSnapshot(e) => self.seen.conversation(&e.messages),

            Event::TextMessageStart(e) => {
                let reopened = !self.seen.messages.insert(e.message_id.clone());
                let mut start = e.clone();
                start.base = restated(&e.base);
                self.push(
                    OpenEntity::Message {
                        start,
                        content: String::new(),
                        chunked,
                    },
                    reopened,
                );
            }
            Event::TextMessageContent(e) => {
                let found = self.open.iter_mut().find_map(|slot| match &mut slot.entity {
                    OpenEntity::Message { start, content, .. }
                        if start.message_id == e.message_id =>
                    {
                        Some((&mut start.base, content))
                    }
                    _ => None,
                });
                if let Some((base, content)) = found {
                    content.push_str(&e.delta);
                    merge_metadata_into(&mut base.metadata, e.base.metadata.as_ref());
                }
            }
            Event::TextMessageEnd(e) => self.close(|entity| {
                matches!(entity, OpenEntity::Message { start, .. } if start.message_id == e.message_id)
            }),

            Event::ToolCallStart(e) => {
                let reopened = !self.seen.tool_calls.insert(e.tool_call_id.clone());
                let mut start = e.clone();
                start.base = restated(&e.base);
                self.push(
                    OpenEntity::ToolCall {
                        start,
                        args: String::new(),
                        chunked,
                    },
                    reopened,
                );
            }
            Event::ToolCallArgs(e) => {
                let found = self.open.iter_mut().find_map(|slot| match &mut slot.entity {
                    OpenEntity::ToolCall { start, args, .. }
                        if start.tool_call_id == e.tool_call_id =>
                    {
                        Some((&mut start.base, args))
                    }
                    _ => None,
                });
                if let Some((base, args)) = found {
                    args.push_str(&e.delta);
                    merge_metadata_into(&mut base.metadata, e.base.metadata.as_ref());
                }
            }
            Event::ToolCallEnd(e) => self.close(|entity| {
                matches!(entity, OpenEntity::ToolCall { start, .. } if start.tool_call_id == e.tool_call_id)
            }),
            // A result is a tool message under the id it names: text
            // streamed into that id later adds to the result.
            Event::ToolCallResult(e) => {
                self.seen.messages.insert(e.message_id.clone());
            }

            Event::ReasoningStart(e) => {
                let mut start = e.clone();
                start.base = restated(&e.base);
                self.push(OpenEntity::ReasoningBlock { start }, false);
            }
            Event::ReasoningEnd(e) => self.close(|entity| {
                matches!(entity, OpenEntity::ReasoningBlock { start } if start.message_id == e.message_id)
            }),
            Event::ReasoningMessageStart(e) => {
                let reopened = !self.seen.reasoning.insert(e.message_id.clone());
                let mut start = e.clone();
                start.base = restated(&e.base);
                self.push(
                    OpenEntity::ReasoningMessage {
                        start,
                        content: String::new(),
                        chunked,
                    },
                    reopened,
                );
            }
            Event::ReasoningMessageContent(e) => {
                let found = self.open.iter_mut().find_map(|slot| match &mut slot.entity {
                    OpenEntity::ReasoningMessage { start, content, .. }
                        if start.message_id == e.message_id =>
                    {
                        Some((&mut start.base, content))
                    }
                    _ => None,
                });
                if let Some((base, content)) = found {
                    content.push_str(&e.delta);
                    merge_metadata_into(&mut base.metadata, e.base.metadata.as_ref());
                }
            }
            Event::ReasoningMessageEnd(e) => self.close(|entity| {
                matches!(entity, OpenEntity::ReasoningMessage { start, .. } if start.message_id == e.message_id)
            }),

            Event::StepStarted(e) => {
                let mut start = e.clone();
                start.base = restated(&e.base);
                self.push(OpenEntity::Step { start }, false);
            }
            // A step is its owner's: a subagent and its parent may each run
            // one of the same name.
            Event::StepFinished(e) => self.close(|entity| {
                matches!(entity, OpenEntity::Step { start }
                    if start.step_name == e.step_name && start.subagent_run_id == e.subagent_run_id)
            }),

            Event::SubagentStarted(e) => {
                let mut start = e.clone();
                start.base = restated(&e.base);
                self.subagents.push(Announced {
                    start: start.clone(),
                    end: None,
                });
                self.push(OpenEntity::Subagent { start }, false);
            }
            Event::SubagentFinished(e) => self.close_subagent(&e.subagent_run_id, event),
            Event::SubagentError(e) => self.close_subagent(&e.subagent_run_id, event),

            // State, activity, raw, custom and encrypted-value events open
            // and close nothing. A chunk arrives here as what it stood for.
            _ => {}
        }
    }

    fn push(&mut self, entity: OpenEntity, reopened: bool) {
        self.open.push(Slot { entity, reopened });
    }

    fn close(&mut self, is: impl Fn(&OpenEntity) -> bool) {
        if let Some(index) = self.open.iter().position(|slot| is(&slot.entity)) {
            self.open.remove(index);
        }
    }

    fn close_subagent(&mut self, id: &SubagentRunId, end: &Event) {
        self.close(
            |entity| matches!(entity, OpenEntity::Subagent { start } if &start.subagent_run_id == id),
        );
        let active =
            self.subagents.iter_mut().rev().find(|announced| {
                &announced.start.subagent_run_id == id && announced.end.is_none()
            });
        if let Some(announced) = active {
            let mut end = end.clone();
            end.base_mut().raw_event = None;
            announced.end = Some(end);
        }
    }

    // ---- the prelude ----------------------------------------------------

    /// Takes out of `messages` what a prelude restates: the text of each
    /// open message, the arguments of each open tool call, the content of
    /// each open reasoning message. Everything else stays where it is.
    fn take_out(&self, messages: &mut [Message]) {
        for slot in &self.open {
            match &slot.entity {
                OpenEntity::Message { start, content, .. } => {
                    let held = messages
                        .iter_mut()
                        .filter(|message| message.id() == &start.message_id)
                        .filter_map(streamed_text);
                    for text in held {
                        retract(text, content, slot.reopened);
                    }
                }
                OpenEntity::ToolCall { start, args, .. } => {
                    let calls = messages.iter_mut().filter_map(|message| match message {
                        Message::Assistant(assistant) => assistant.tool_calls.as_mut(),
                        _ => None,
                    });
                    for call in calls.flatten() {
                        if call.id == start.tool_call_id {
                            retract(&mut call.function.arguments, args, slot.reopened);
                        }
                    }
                }
                OpenEntity::ReasoningMessage { start, content, .. } => {
                    for message in messages.iter_mut() {
                        if let Message::Reasoning(reasoning) = message {
                            if reasoning.id == start.message_id {
                                retract(&mut reasoning.content, content, slot.reopened);
                            }
                        }
                    }
                }
                OpenEntity::ReasoningBlock { .. }
                | OpenEntity::Step { .. }
                | OpenEntity::Subagent { .. } => {}
            }
        }
    }

    /// The lifecycle of the subagents a joining client has to know: each
    /// active one, and each finished one that one of those descends from.
    ///
    /// In the order they started. A parent starts before its child, so
    /// every `parentSubagentRunId` names a subagent announced before it.
    fn subagent_lifecycle(&self, events: &mut Vec<Event>) {
        let mut needed: HashSet<&SubagentRunId> = HashSet::new();
        for announced in self.subagents.iter().rev() {
            let id = &announced.start.subagent_run_id;
            if announced.end.is_none() {
                needed.insert(id);
            }
            if needed.contains(id) {
                needed.extend(&announced.start.parent_subagent_run_id);
            }
        }
        for announced in &self.subagents {
            if needed.contains(&announced.start.subagent_run_id) {
                events.push(announced.start.clone().into());
                events.extend(announced.end.clone());
            }
        }
    }
}

/// The base fields a restated event carries: everything but `rawEvent`,
/// which is the provider event one event was translated from.
fn restated(base: &BaseEvent) -> BaseEvent {
    BaseEvent {
        timestamp: base.timestamp,
        raw_event: None,
        metadata: base.metadata.clone(),
    }
}

/// The text of a message that `TEXT_MESSAGE_CONTENT` adds to, where it has
/// one: what a client appends a delta to.
fn streamed_text(message: &mut Message) -> Option<&mut String> {
    fn last_text(parts: &mut [InputContent]) -> Option<&mut String> {
        match parts.last_mut() {
            Some(InputContent::Text(text)) => Some(&mut text.text),
            _ => None,
        }
    }
    match message {
        Message::Assistant(assistant) => assistant.content.as_mut(),
        Message::System(system) => Some(&mut system.content),
        Message::Developer(developer) => Some(&mut developer.content),
        Message::User(user) => match &mut user.content {
            UserContent::Text(text) => Some(text),
            UserContent::Parts(parts) => last_text(parts),
        },
        Message::Tool(tool) => match &mut tool.content {
            ToolContent::Text(text) => Some(text),
            ToolContent::Parts(parts) => last_text(parts),
        },
        // Reasoning is its own entity, and an activity holds no text.
        Message::Reasoning(_) | Message::Activity(_) => None,
    }
}

/// Leaves in `held` what an entity held before it was opened, given what it
/// has produced since.
fn retract(held: &mut String, since: &str, reopened: bool) {
    if !reopened {
        // Opened for the first time, so it held nothing: whatever the list
        // has under this id was produced since, however much of it.
        held.clear();
    } else if held.ends_with(since) {
        held.truncate(held.len() - since.len());
    }
}

impl OpenEntity {
    /// Appends the events that re-open this entity for a client: its start,
    /// and with `content` what it has produced since.
    fn restate(&self, content: bool, events: &mut Vec<Event>) {
        let since = |text: &String| (content && !text.is_empty()).then(|| text.clone());
        match self {
            Self::Message {
                start,
                content,
                chunked: true,
            } => {
                let mut chunk =
                    TextMessageChunkEvent::new(Some(start.message_id.clone()), since(content));
                chunk.base = start.base.clone();
                chunk.role = Some(start.role);
                chunk.name.clone_from(&start.name);
                chunk.subagent_run_id.clone_from(&start.subagent_run_id);
                events.push(chunk.into());
            }
            Self::Message {
                start,
                content,
                chunked: false,
            } => {
                events.push(start.clone().into());
                if let Some(delta) = since(content) {
                    events.push(tagged(
                        Event::text_message_content(start.message_id.clone(), delta),
                        &start.subagent_run_id,
                    ));
                }
            }
            Self::ToolCall {
                start,
                args,
                chunked: true,
            } => {
                let mut chunk = ToolCallChunkEvent::new(
                    Some(start.tool_call_id.clone()),
                    Some(start.tool_call_name.clone()),
                    since(args),
                );
                chunk.base = start.base.clone();
                chunk.parent_message_id.clone_from(&start.parent_message_id);
                chunk.subagent_run_id.clone_from(&start.subagent_run_id);
                events.push(chunk.into());
            }
            Self::ToolCall {
                start,
                args,
                chunked: false,
            } => {
                events.push(start.clone().into());
                if let Some(delta) = since(args) {
                    events.push(tagged(
                        Event::tool_call_args(start.tool_call_id.clone(), delta),
                        &start.subagent_run_id,
                    ));
                }
            }
            Self::ReasoningMessage {
                start,
                content,
                chunked: true,
            } => {
                let mut chunk =
                    ReasoningMessageChunkEvent::new(Some(start.message_id.clone()), since(content));
                chunk.base = start.base.clone();
                chunk.subagent_run_id.clone_from(&start.subagent_run_id);
                events.push(chunk.into());
            }
            Self::ReasoningMessage {
                start,
                content,
                chunked: false,
            } => {
                events.push(start.clone().into());
                if let Some(delta) = since(content) {
                    events.push(tagged(
                        Event::reasoning_message_content(start.message_id.clone(), delta),
                        &start.subagent_run_id,
                    ));
                }
            }
            Self::ReasoningBlock { start } => events.push(start.clone().into()),
            Self::Step { start } => events.push(start.clone().into()),
            // Announced with the other subagents, ahead of everything open.
            Self::Subagent { .. } => {}
        }
    }
}

/// `event`, attributed as the entity it continues is.
fn tagged(mut event: Event, owner: &Option<SubagentRunId>) -> Event {
    if let Some(owner) = owner {
        event.set_subagent_run_id(owner.clone());
    }
    event
}

/// The events that take a joining client to where a run is.
///
/// Built by [`RunProgress::prelude`]. Each method adds something to send;
/// nothing is sent that was not asked for. See the [module docs](self) for
/// the order and for the reasoning behind each choice.
#[derive(Clone, Debug)]
#[must_use = "a prelude is only a description until `into_events` builds it"]
pub struct Prelude<'a> {
    progress: &'a RunProgress,
    input: Option<RunAgentInput>,
    messages: Option<Vec<Message>>,
    state: Option<Value>,
    /// Whether each open entity is sent with what it has produced.
    restate: bool,
}

impl Prelude<'_> {
    /// Carries the conversation as `MESSAGES_SNAPSHOT`, right after
    /// `RUN_STARTED`.
    ///
    /// `messages` is everything the thread holds that is complete: the
    /// earlier turns and what this run has finished. It may hold what is
    /// open as well. The snapshot is these messages with [what the prelude
    /// restates taken out](self#what-is-open-is-taken-out-of-the-messages),
    /// so no list makes a client hold anything twice.
    ///
    /// A snapshot is the carrier to prefer when a client may already hold
    /// part of the conversation: it replaces the messages it carries and
    /// drops the ones it leaves out, where [`echo_input`](Self::echo_input)
    /// only adds.
    pub fn messages(mut self, messages: impl Into<Vec<Message>>) -> Self {
        self.messages = Some(messages.into());
        self.restate = true;
        self
    }

    /// Echoes a request on `RUN_STARTED`, as
    /// [`Runner::echo_input`](crate::server::Runner::echo_input) does on a
    /// run's own.
    ///
    /// A client adds the echoed messages whose ids it does not hold, and
    /// leaves the ones it holds as they are. So this carries the
    /// conversation to a client that holds none of it, and
    /// [`messages`](Self::messages) is for the rest. What the prelude
    /// restates is taken out of `input.messages` as it is out of a snapshot.
    ///
    /// The echo is the whole request: state, tools, context and forwarded
    /// properties as well as the messages.
    pub fn echo_input(mut self, input: RunAgentInput) -> Self {
        self.input = Some(input);
        self
    }

    /// Carries the state as `STATE_SNAPSHOT`, before anything is re-opened.
    ///
    /// For a run that publishes state: the `STATE_DELTA`s that follow patch
    /// the state the client holds, and a client that joins holds none.
    pub fn state(mut self, state: impl Into<Value>) -> Self {
        self.state = Some(state.into());
        self
    }

    /// Re-opens what is open without restating it, and sends no snapshot.
    ///
    /// For a client known to hold each open entity exactly as it stands at
    /// this point of the run: see [the module
    /// docs](self#a-client-that-holds-the-run-up-to-a-point). A snapshot
    /// would replace what that client holds, so the messages given to
    /// [`messages`](Self::messages) are dropped; calling `messages`
    /// afterwards goes back to restating.
    pub fn reopen_only(mut self) -> Self {
        self.messages = None;
        self.restate = false;
        self
    }

    /// The prelude, in the order to send it.
    pub fn into_events(self) -> Vec<Event> {
        let Self {
            progress,
            input,
            messages,
            state,
            restate,
        } = self;
        let mut events = Vec::new();
        if let Some(started) = &progress.started {
            let mut started = started.clone();
            started.input = input.map(|mut input| {
                if restate {
                    progress.take_out(&mut input.messages);
                }
                Box::new(input)
            });
            events.push(started.into());
            if let Some(mut messages) = messages {
                progress.take_out(&mut messages);
                events.push(Event::messages_snapshot(messages));
            }
            events.extend(state.map(Event::state_snapshot));
            progress.subagent_lifecycle(&mut events);
            for slot in &progress.open {
                slot.entity.restate(restate, &mut events);
            }
        }
        events.extend(progress.ended.iter().cloned());
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TextMessageRole;

    fn chunk(id: Option<&str>, delta: &str) -> Event {
        Event::text_message_chunk(id.map(MessageId::new), Some(delta.to_owned()))
    }

    /// Runs that open and close everything, in both spellings, with two
    /// lanes at once, and one that fails with things open.
    fn runs() -> Vec<Vec<Event>> {
        let s1 = |event: Event| event.with_subagent_run_id("s1");
        vec![
            vec![
                Event::run_started("t", "r"),
                Event::step_started("plan"),
                Event::subagent_started("s1", "helper"),
                s1(Event::step_started("plan")),
                Event::reasoning_start("r1"),
                Event::reasoning_message_start("r1"),
                Event::reasoning_message_content("r1", "hm"),
                s1(Event::text_message_start("m1", TextMessageRole::Assistant)),
                Event::tool_call_start("c1", "search"),
                Event::reasoning_message_end("r1"),
                Event::tool_call_args("c1", "{}"),
                s1(Event::text_message_content("m1", "hi")),
                Event::reasoning_end("r1"),
                Event::tool_call_end("c1"),
                s1(Event::text_message_end("m1")),
                Event::tool_call_result("m2", "c1", "ok"),
                s1(Event::step_finished("plan")),
                Event::subagent_finished_success("s1"),
                // The same id, open a second time.
                Event::text_message_start("m1", TextMessageRole::Assistant),
                Event::text_message_end("m1"),
                Event::step_finished("plan"),
                Event::run_finished_success("t", "r"),
            ],
            vec![
                Event::run_started("t", "r"),
                chunk(Some("m1"), "a"),
                s1(Event::tool_call_chunk(
                    Some("c1".into()),
                    Some("search".into()),
                    None,
                )),
                chunk(None, "b"),
                // Explicit, in the parent's lane: ends the parent's stream.
                Event::text_message_start("m2", TextMessageRole::Assistant),
                Event::reasoning_message_chunk(Some("r1".into()), Some("hm".into())),
                Event::messages_snapshot(Vec::new()),
                chunk(Some("m3"), "c"),
                Event::text_message_end("m2"),
                Event::run_finished_success("t", "r"),
            ],
            vec![
                Event::run_started("t", "r"),
                Event::step_started("plan"),
                Event::text_message_start("m1", TextMessageRole::Assistant),
                chunk(Some("m2"), "a"),
                Event::run_error("gone"),
            ],
        ]
    }

    #[cfg(feature = "verify")]
    #[test]
    fn the_fold_has_open_what_the_ordering_rules_have_open() {
        use crate::ordering::{OpenSets, Ordering, Policy};

        for run in runs() {
            let mut rules = Ordering::new(Policy::PRODUCER);
            let mut progress = RunProgress::new();
            for event in &run {
                rules.observe(event).expect("the run is well formed");
                progress.observe(event).expect("the run is well formed");

                let mut folded = OpenSets::default();
                for entity in progress.open() {
                    match entity {
                        OpenEntity::Message { start, .. } => {
                            folded.messages.push(&start.message_id);
                        }
                        OpenEntity::ToolCall { start, .. } => {
                            folded.tool_calls.push(&start.tool_call_id);
                        }
                        OpenEntity::ReasoningMessage { start, .. } => {
                            folded.reasoning_messages.push(&start.message_id);
                        }
                        OpenEntity::ReasoningBlock { start } => {
                            folded.reasoning_blocks.push(&start.message_id);
                        }
                        OpenEntity::Step { start } => folded.steps.push((
                            start.subagent_run_id.as_ref().map(SubagentRunId::as_str),
                            &start.step_name,
                        )),
                        OpenEntity::Subagent { start } => {
                            folded.subagents.push(&start.subagent_run_id);
                        }
                    }
                }
                assert_eq!(folded, rules.open(), "after {event:?}");
            }
        }
    }

    #[test]
    fn a_chunk_stream_is_open_in_the_chunked_spelling_until_it_ends() {
        let run = &runs()[1];
        let mut progress = RunProgress::new();
        for event in &run[..4] {
            progress.fold(event);
        }
        let spelled: Vec<(&str, bool)> = progress
            .open()
            .filter_map(|entity| match entity {
                OpenEntity::Message {
                    content, chunked, ..
                } => Some((content.as_str(), *chunked)),
                OpenEntity::ToolCall { args, chunked, .. } => Some((args.as_str(), *chunked)),
                _ => None,
            })
            .collect();
        assert_eq!(spelled, [("ab", true), ("", true)]);

        // The explicit start ends the parent's stream and opens `m2`.
        progress.fold(&run[4]);
        let open: Vec<&OpenEntity> = progress.open().collect();
        assert!(
            matches!(open[..], [
                OpenEntity::ToolCall { chunked: true, .. },
                OpenEntity::Message { start, chunked: false, .. },
            ] if start.message_id.as_str() == "m2"),
            "{open:?}"
        );
    }

    #[test]
    fn events_no_verifier_vouched_for_are_folded_without_harm() {
        // What `observe` rejects with the `verify` feature reaches the fold
        // without it.
        let mut progress = RunProgress::new();
        for event in [
            Event::text_message_content("never-opened", "x"),
            Event::text_message_end("never-opened"),
            Event::tool_call_args("never-opened", "x"),
            Event::step_finished("never-started"),
            Event::subagent_finished_success("never-started"),
            chunk(None, "continues nothing"),
            Event::tool_call_chunk(Some("unnamed".into()), None, None),
            Event::run_finished_success("t", "r"),
        ] {
            progress.fold(&event);
        }
        assert_eq!(progress.open().count(), 0);
        assert!(!progress.has_started() && progress.has_ended());
    }

    #[test]
    fn only_what_was_produced_since_the_start_is_taken_out() {
        let taken_out = |held: &str, since: &str, reopened: bool| {
            let mut held = held.to_owned();
            retract(&mut held, since, reopened);
            held
        };
        // Opened for the first time: nothing was there before, so nothing
        // stays, whichever copy the list holds.
        assert_eq!(taken_out("Hello wor", "Hello wor", false), "");
        assert_eq!(taken_out("Hel", "Hello wor", false), "");
        assert_eq!(taken_out("Hello world, and more", "Hello wor", false), "");
        // Opened again: what was there before stays.
        assert_eq!(taken_out("Earlier. Hello", " Hello", true), "Earlier.");
        assert_eq!(taken_out("Earlier.", " Hello", true), "Earlier.");
        assert_eq!(taken_out("Earlier.", "", true), "Earlier.");
    }
}
