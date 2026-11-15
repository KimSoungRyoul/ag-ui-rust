//! Saving a conversation, and restoring one.

use std::collections::HashSet;
use std::sync::Arc;

use crate::client::{
    agent::RemoteAgent,
    apply::{Applier, Subagent},
    error::{Error, Result},
    transport::Transport,
};
use crate::{Interrupt, Message, MessageId, ReasoningMessage, RunId, ThreadId, ToolCallId};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use super::pending::{ResumeSubmission, SubmissionStatus, validate_responses};
use super::run::RunEnd;
use super::{Thread, random_id, unique};

/// The format [`Thread::snapshot`] writes. Version 1 kept reasoning in a list
/// of its own; [`Thread::restore`] still reads it.
const VERSION: u32 = 2;

fn current_protocol_version() -> Option<String> {
    Some(crate::input::PROTOCOL_VERSION.to_owned())
}

/// A versioned local snapshot. It contains conversation data, never a transport,
/// observer or running future. Validate it through `restore` before using it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThreadSnapshot {
    /// Snapshot format version (currently 2). Version 1 is still restored.
    pub version: u32,
    /// The remote conversation identifier.
    pub thread_id: ThreadId,
    /// Protocol version this thread declares to its peer. `None` pins a
    /// legacy peer; snapshots predating this field restore the current version.
    #[serde(default = "current_protocol_version")]
    pub protocol_version: Option<String>,
    /// Materialized conversation, reasoning and activity messages included.
    pub messages: Vec<Message>,
    /// Current raw shared state.
    pub state: Value,
    /// Version 1 only: the reasoning that format kept apart from `messages`.
    /// A snapshot of the current version leaves it empty and does not write
    /// it. [`Thread::restore`] moves a version 1 list into the conversation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasoning: Vec<ReasoningMessage>,
    /// Announced subagent lifecycle state.
    pub subagents: Vec<Subagent>,
    /// Questions which have not been confirmed as answered.
    pub interrupts: Vec<Interrupt>,
    /// Tool calls whose interrupt decisions were accepted by a later run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decided_interrupt_tool_call_ids: Vec<ToolCallId>,
    /// A submitted decision whose acceptance has not been confirmed.
    pub submission: Option<ResumeSubmission>,
    /// Previously dispatched local run IDs, used to prevent reuse.
    pub run_ids: Vec<RunId>,
    /// The last locally observed termination, if any.
    pub last_run_end: Option<RunEnd>,
    /// A dispatched local run which had not ended when this snapshot was taken.
    pub active_run_id: Option<RunId>,
}

impl<T, S> Thread<T, S> {
    /// Save local conversation data. Transport settings and callbacks are excluded.
    pub fn snapshot(&self) -> ThreadSnapshot {
        let mut run_ids: Vec<_> = self.run_ids.iter().cloned().collect();
        run_ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        let mut decided_interrupt_tool_call_ids: Vec<_> = self
            .decided_interrupt_tool_call_ids
            .iter()
            .cloned()
            .collect();
        decided_interrupt_tool_call_ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        ThreadSnapshot {
            version: VERSION,
            thread_id: self.thread_id.clone(),
            protocol_version: self.protocol_version.clone(),
            messages: self.messages().to_vec(),
            state: self.raw_state().clone(),
            reasoning: Vec::new(),
            subagents: self.subagents().to_vec(),
            interrupts: self.interrupts.clone(),
            decided_interrupt_tool_call_ids,
            submission: self.submission.clone(),
            run_ids,
            last_run_end: self.last_run_end.clone(),
            active_run_id: self.active_run_id.clone(),
        }
    }
}

impl<T, S: DeserializeOwned> Thread<T, S> {
    /// Restore local state using a new transport. No server query is performed.
    ///
    /// A version 1 snapshot kept reasoning in a list of its own, and nothing
    /// in it says where a reasoning message stood among the others. Its
    /// entries are put before every message of the conversation, in the
    /// order the list held them. Reasoning comes before the reply it led to,
    /// and there each entry still does; after the messages, each would follow
    /// its own reply and stand in front of whatever the agent says next. From
    /// there they are sent with the next run, like any other reasoning
    /// message.
    ///
    /// Two kinds of entry are not moved. One whose id a message of the
    /// conversation already has, because an id names one message. And one
    /// with no content, encrypted value or metadata: that is what version 1
    /// kept for a `REASONING_START` whose id no reasoning message shared, and
    /// it holds no reasoning.
    pub fn restore(transport: T, snapshot: ThreadSnapshot) -> Result<Self>
    where
        T: Transport,
    {
        Self::restore_shared(Arc::new(transport), snapshot)
    }

    pub(crate) fn restore_shared(transport: Arc<T>, mut snapshot: ThreadSnapshot) -> Result<Self> {
        validate_snapshot(&snapshot)?;
        if snapshot.active_run_id.take().is_some() {
            snapshot.last_run_end = Some(RunEnd::Aborted);
        }
        let typed = S::deserialize(&snapshot.state).map_err(Error::State)?;
        if let Some(submission) = &mut snapshot.submission {
            submission.status = SubmissionStatus::Unconfirmed;
        }
        for subagent in &mut snapshot.subagents {
            if subagent.status == crate::client::SubagentStatus::Running {
                subagent.status = crate::client::SubagentStatus::Aborted;
            }
        }
        let mut applier = Applier::new()
            .with_messages(conversation(snapshot.messages, snapshot.reasoning))
            .with_state(snapshot.state);
        applier.restore_auxiliary(snapshot.subagents, snapshot.interrupts.clone());
        Ok(Self {
            agent: RemoteAgent::new(transport),
            thread_id: snapshot.thread_id,
            applier,
            state: Ok(typed),
            tools: Vec::new(),
            context: Vec::new(),
            forwarded_props: Value::Null,
            protocol_version: snapshot.protocol_version,
            verify: true,
            interrupts: snapshot.interrupts,
            decided_interrupt_tool_call_ids: snapshot
                .decided_interrupt_tool_call_ids
                .into_iter()
                .collect(),
            submission: snapshot.submission,
            run_ids: snapshot.run_ids.into_iter().collect(),
            next_run_id: None,
            last_run_end: snapshot.last_run_end,
            active_run_id: None,
            observer: None,
            id_generator: Box::new(random_id),
            diagnostic_limit: 100,
        })
    }
}

/// The messages of a snapshot with a version 1 reasoning list put in front of
/// them; see [`Thread::restore`] for why there, and for the two kinds of
/// entry left behind.
fn conversation(messages: Vec<Message>, reasoning: Vec<ReasoningMessage>) -> Vec<Message> {
    if reasoning.is_empty() {
        return messages;
    }
    let taken: HashSet<&MessageId> = messages.iter().map(Message::id).collect();
    let mut conversation: Vec<Message> = reasoning
        .into_iter()
        .filter(|entry| {
            let holds_nothing = entry.content.is_empty()
                && entry.encrypted_value.is_none()
                && entry.metadata.is_none();
            !holds_nothing && !taken.contains(&entry.id)
        })
        .map(Message::Reasoning)
        .collect();
    conversation.extend(messages);
    conversation
}

fn validate_snapshot(snapshot: &ThreadSnapshot) -> Result<()> {
    match snapshot.version {
        1 => unique(
            snapshot.reasoning.iter().map(|message| message.id.as_str()),
            "reasoning",
        )?,
        // One store: a second list could only disagree with the first.
        VERSION if snapshot.reasoning.is_empty() => {}
        VERSION => {
            return Err(Error::Snapshot(
                "a version 2 snapshot holds reasoning in `messages`, so `reasoning` must be empty"
                    .into(),
            ));
        }
        other => return Err(Error::Snapshot(format!("unsupported version {other}"))),
    }
    if snapshot.thread_id.as_str().is_empty() {
        return Err(Error::Snapshot("empty thread ID".into()));
    }
    unique(
        snapshot
            .messages
            .iter()
            .map(|message| message.id().as_str()),
        "message",
    )?;
    unique(
        snapshot
            .subagents
            .iter()
            .map(|subagent| subagent.run_id.as_str()),
        "subagent",
    )?;
    unique(
        snapshot
            .interrupts
            .iter()
            .map(|interrupt| interrupt.id.as_str()),
        "interrupt",
    )?;
    unique(
        snapshot
            .decided_interrupt_tool_call_ids
            .iter()
            .map(ToolCallId::as_str),
        "decided interrupt tool call",
    )?;
    unique(snapshot.run_ids.iter().map(|id| id.as_str()), "run")?;
    if let Some(id) = &snapshot.active_run_id {
        if !snapshot.run_ids.contains(id) {
            return Err(Error::Snapshot(
                "active run refers to an unknown run ID".into(),
            ));
        }
    }
    for subagent in &snapshot.subagents {
        let mut seen = HashSet::from([subagent.run_id.as_str()]);
        let mut parent = subagent.parent_subagent_run_id.as_ref();
        while let Some(id) = parent {
            if !seen.insert(id.as_str()) {
                return Err(Error::Snapshot("cyclic subagent ancestry".into()));
            }
            let ancestor = snapshot
                .subagents
                .iter()
                .find(|subagent| &subagent.run_id == id)
                .ok_or_else(|| Error::Snapshot(format!("unknown parent subagent {id}")))?;
            parent = ancestor.parent_subagent_run_id.as_ref();
        }
    }
    if let Some(submission) = &snapshot.submission {
        if submission.status == SubmissionStatus::InFlight
            && snapshot.active_run_id.as_ref() != Some(&submission.run_id)
        {
            return Err(Error::Snapshot(
                "in-flight submission must reference the active run".into(),
            ));
        }
        if !snapshot.run_ids.contains(&submission.run_id) {
            return Err(Error::Snapshot(
                "submission refers to an unknown run ID".into(),
            ));
        }
        validate_responses(&snapshot.interrupts, &submission.entries, false)
            .map_err(|error| Error::Snapshot(error.to_string()))?;
    }
    Ok(())
}
