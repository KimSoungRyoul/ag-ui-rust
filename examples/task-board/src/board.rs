//! The board: the shared state, the tools that change it, and the A2UI surface
//! that draws it.
//!
//! Nothing here knows about AG-UI events. The state is a plain `serde` struct
//! that [`ag_ui_server::RunContext`] publishes as `STATE_SNAPSHOT` /
//! `STATE_DELTA`, the tools are [`ag_ui_core::Tool`] definitions the client
//! offers, and the surface is an [`ag_ui_a2ui`] component tree. Keeping the
//! domain free of the protocol is what makes [`crate::agent`] short.

use ag_ui_a2ui::message::Component;
use ag_ui_a2ui::toolkit::ops::SurfaceSpec;
use ag_ui_core::Tool;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The A2UI surface every render targets.
pub const SURFACE_ID: &str = "task-board";

/// Adds one task.
pub const ADD_TASK: &str = "add_task";
/// Marks one task done.
pub const COMPLETE_TASK: &str = "complete_task";
/// Puts a minute estimate on one task.
pub const ESTIMATE: &str = "estimate";
/// Removes every task. Destructive, so the agent asks first.
pub const CLEAR_BOARD: &str = "clear_board";

/// One item on the board.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    /// Stable within a thread: ids come from [`Board::next_id`], which the
    /// client carries back in `RunAgentInput::state` on the next run.
    pub id: u32,
    /// What the user typed.
    pub title: String,
    /// Minutes, once somebody has estimated it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_minutes: Option<u32>,
    /// Whether it is finished.
    pub done: bool,
}

impl Task {
    /// How the task reads on one line.
    ///
    /// Carries no done marker: on the surface that is the checkbox's job, and
    /// duplicating it there was the first thing the terminal showed.
    pub fn label(&self) -> String {
        let estimate = match self.estimate_minutes {
            Some(minutes) => format!(" · {minutes}m"),
            None => String::new(),
        };
        format!("#{} {}{estimate}", self.id, self.title)
    }
}

/// Everything the user and the agent share.
///
/// This is `Agent::State`, so it round-trips: the agent publishes it, the
/// client mirrors it, and the client sends it back with the next run. Ids and
/// estimates therefore survive a run boundary without the agent storing
/// anything.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Board {
    /// Every task, in the order they were added.
    #[serde(default)]
    pub tasks: Vec<Task>,
    /// The id the next task will get.
    #[serde(default)]
    pub next_id: u32,
}

impl Board {
    /// Appends a task and returns it.
    pub fn add(&mut self, title: impl Into<String>) -> &Task {
        self.next_id += 1;
        self.tasks.push(Task {
            id: self.next_id,
            title: title.into(),
            estimate_minutes: None,
            done: false,
        });
        self.tasks.last().expect("just pushed")
    }

    /// Finds a task by id, or by a case-insensitive substring of its title.
    ///
    /// `complete 2` and `complete book the room` both have to work, because
    /// both are what a person types.
    pub fn find(&self, needle: &str) -> Option<&Task> {
        self.index_of(needle).map(|index| &self.tasks[index])
    }

    fn index_of(&self, needle: &str) -> Option<usize> {
        let needle = needle.trim();
        if let Ok(id) = needle.parse::<u32>() {
            if let Some(index) = self.tasks.iter().position(|task| task.id == id) {
                return Some(index);
            }
        }
        let lowered = needle.to_lowercase();
        if lowered.is_empty() {
            return None;
        }
        self.tasks
            .iter()
            .position(|task| task.title.to_lowercase().contains(&lowered))
    }

    /// Marks a task done. Returns it, or `None` if nothing matched.
    pub fn complete(&mut self, needle: &str) -> Option<&Task> {
        let index = self.index_of(needle)?;
        self.tasks[index].done = true;
        Some(&self.tasks[index])
    }

    /// Puts an estimate on a task. Returns it, or `None` if nothing matched.
    pub fn estimate(&mut self, needle: &str, minutes: u32) -> Option<&Task> {
        let index = self.index_of(needle)?;
        self.tasks[index].estimate_minutes = Some(minutes);
        Some(&self.tasks[index])
    }

    /// Removes every task and returns how many there were. Ids keep counting,
    /// so a cleared board does not reuse them.
    pub fn clear(&mut self) -> usize {
        let removed = self.tasks.len();
        self.tasks.clear();
        removed
    }

    /// How many tasks are not done.
    pub fn open(&self) -> usize {
        self.tasks.iter().filter(|task| !task.done).count()
    }

    /// How many tasks are done.
    pub fn done(&self) -> usize {
        self.tasks.iter().filter(|task| task.done).count()
    }

    /// Total estimated minutes over the tasks that still have to be done.
    pub fn remaining_minutes(&self) -> u32 {
        self.tasks
            .iter()
            .filter(|task| !task.done)
            .filter_map(|task| task.estimate_minutes)
            .sum()
    }

    /// The status line under the heading.
    pub fn summary(&self) -> String {
        if self.tasks.is_empty() {
            return "nothing on the board".to_owned();
        }
        let mut summary = format!("{} open · {} done", self.open(), self.done());
        let minutes = self.remaining_minutes();
        if minutes > 0 {
            summary.push_str(&format!(" · {minutes}m to go"));
        }
        summary
    }
}

/// The tools the client offers and the agent executes.
///
/// The client sends these on every run and the agent reads them back out of
/// [`RunContext::tools`](ag_ui_server::RunContext::tools), so a run that names
/// a tool the client never offered is a bug this example can actually catch.
pub fn tools() -> Vec<Tool> {
    vec![
        Tool::new(
            ADD_TASK,
            "Add one task to the board.",
            json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "What has to be done."},
                },
                "required": ["title"],
            }),
        ),
        Tool::new(
            COMPLETE_TASK,
            "Mark a task done, by id or by a piece of its title.",
            json!({
                "type": "object",
                "properties": {
                    "task": {"type": "string", "description": "Task id, or part of the title."},
                },
                "required": ["task"],
            }),
        ),
        Tool::new(
            ESTIMATE,
            "Put a minute estimate on a task.",
            json!({
                "type": "object",
                "properties": {
                    "task": {"type": "string", "description": "Task id, or part of the title."},
                    "minutes": {"type": "integer", "minimum": 1},
                },
                "required": ["task", "minutes"],
            }),
        ),
        Tool::new(
            CLEAR_BOARD,
            "Remove every task. Destructive: ask the human first.",
            json!({"type": "object", "properties": {}}),
        ),
    ]
}

/// The board as an A2UI surface: a card, a heading, a status line, and one
/// two-way bound checkbox per task.
///
/// The component tree is fixed and the data model is what moves, which is the
/// shape A2UI is built for — a renderer that already has this tree redraws from
/// `updateDataModel` alone.
pub fn surface(board: &Board) -> SurfaceSpec {
    SurfaceSpec::new(SURFACE_ID)
        .with_components(vec![
            Component::new("root", "Card").with("child", json!("body")),
            Component::new("body", "Column").with("children", json!(["heading", "status", "list"])),
            Component::new("heading", "Text")
                .with("text", json!({"path": "/title"}))
                .with("variant", json!("h2")),
            Component::new("status", "Text")
                .with("text", json!({"path": "/summary"}))
                .with("variant", json!("caption")),
            Component::new("list", "List")
                // A child *template*, not a child list: one instance per
                // element of `/tasks`, each with its own scope.
                .with("children", json!({"componentId": "task", "path": "/tasks"})),
            Component::new("task", "CheckBox")
                // Relative paths, resolved inside the template's scope.
                .with("label", json!({"path": "label"}))
                .with("value", json!({"path": "done"})),
        ])
        .with_data_model(data_model(board))
}

/// What the surface's bindings read.
pub fn data_model(board: &Board) -> Value {
    json!({
        "title": "Workshop board",
        "summary": board.summary(),
        "tasks": board
            .tasks
            .iter()
            .map(|task| json!({"label": task.label(), "done": task.done}))
            .collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ag_ui_a2ui::catalog::Catalog;
    use ag_ui_a2ui::validate::Validator;

    fn board() -> Board {
        let mut board = Board::default();
        board.add("draft the agenda");
        board.add("book the room");
        board.estimate("2", 45).expect("task 2 exists");
        board.complete("agenda").expect("matched by title");
        board
    }

    #[test]
    fn tasks_are_found_by_id_and_by_title() {
        let board = board();
        assert_eq!(board.find("1").map(|task| task.id), Some(1));
        assert_eq!(board.find("BOOK").map(|task| task.id), Some(2));
        assert_eq!(board.find("nothing like this"), None);
    }

    #[test]
    fn a_cleared_board_does_not_reuse_ids() {
        let mut board = board();
        assert_eq!(board.clear(), 2);
        assert_eq!(board.add("start over").id, 3);
    }

    #[test]
    fn the_summary_counts_only_the_work_that_is_left() {
        let board = board();
        assert_eq!(board.summary(), "1 open · 1 done · 45m to go");
        assert_eq!(Board::default().summary(), "nothing on the board");
    }

    /// The agent ships this tree to a renderer it cannot see, so the only
    /// check available before it leaves is the catalog's own.
    #[test]
    fn the_surface_validates_against_the_basic_catalog() {
        let board = board();
        let spec = surface(&board);
        let model = spec.data_model.clone().expect("a data model");

        let report =
            Validator::new(&Catalog::basic()).validate_surface(&spec.components, Some(&model));
        assert!(report.is_valid(), "{:?}", report.errors);
        assert!(report.unreachable.is_empty(), "{:?}", report.unreachable);
    }
}
