//! The client's own view of the agent's state.
//!
//! Deliberately *not* the server's type. A front-end team is handed a JSON
//! shape, not a crate, and writes the struct it wants to render from — so this
//! is a second, independent declaration of the same wire contract, and the
//! integration tests are what keep the two honest about each other.
//!
//! It carries only what the view draws. `nextId` is on the wire and absent
//! here, which is the case worth having: `Session` deserializes state into `S`
//! by value, so a client that models less than the agent publishes has to keep
//! working, and a client that models it *wrongly* has to say so.

use serde::{Deserialize, Serialize};

/// One item on the board.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    /// Stable within a thread.
    pub id: u32,
    /// What the user asked for.
    pub title: String,
    /// Minutes, once somebody has estimated it.
    #[serde(default)]
    pub estimate_minutes: Option<u32>,
    /// Whether it is finished.
    #[serde(default)]
    pub done: bool,
}

impl Task {
    /// How the task reads on one line, checkbox included.
    pub fn line(&self) -> String {
        let mark = if self.done { "x" } else { " " };
        let estimate = match self.estimate_minutes {
            Some(minutes) => format!(" · {minutes}m"),
            None => String::new(),
        };
        format!("[{mark}] #{} {}{estimate}", self.id, self.title)
    }
}

/// The board as this client models it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Board {
    /// Every task, in the order the agent published them.
    #[serde(default)]
    pub tasks: Vec<Task>,
}

impl Board {
    /// How many tasks are not done.
    pub fn open(&self) -> usize {
        self.tasks.iter().filter(|task| !task.done).count()
    }

    /// How many tasks are done.
    pub fn done(&self) -> usize {
        self.tasks.iter().filter(|task| task.done).count()
    }

    /// The one-line status the watcher prints on every state event.
    pub fn summary(&self) -> String {
        if self.tasks.is_empty() {
            return "empty".to_owned();
        }
        format!("{} open · {} done", self.open(), self.done())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The contract this client is written against, as the agent publishes it.
    #[test]
    fn state_the_agent_publishes_deserializes_into_the_view_model() {
        let published = json!({
            "tasks": [
                {"id": 1, "title": "draft the agenda", "done": false},
                {"id": 2, "title": "book the room", "estimateMinutes": 45, "done": true},
            ],
            // Modelled by the agent, not by this client. Extra keys are ignored
            // rather than fatal, which is what lets the two evolve apart.
            "nextId": 2,
        });

        let board: Board = serde_json::from_value(published).expect("the published shape");
        assert_eq!(board.summary(), "1 open · 1 done");
        assert_eq!(board.tasks[1].line(), "[x] #2 book the room · 45m");
    }

    #[test]
    fn an_empty_state_object_is_an_empty_board() {
        let board: Board = serde_json::from_value(json!({})).expect("the empty shape");
        assert_eq!(board, Board::default());
        assert_eq!(board.summary(), "empty");
    }
}
