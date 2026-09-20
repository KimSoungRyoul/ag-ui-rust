//! Typed action properties embedded in components, before renderer resolution.
//! These are distinct from `message::Action`, which is a resolved user interaction.

use crate::message::FunctionCall;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A component action sends an event or invokes a renderer-local function.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ComponentAction {
    /// An interaction to send to the agent after binding its context.
    Event(ActionEvent),
    /// A function to execute in the renderer.
    FunctionCall(FunctionCall),
}

impl ComponentAction {
    /// Builds an event action with no context bindings.
    pub fn event(name: impl Into<String>) -> Self {
        Self::Event(ActionEvent {
            name: name.into(),
            context: Map::new(),
        })
    }
}

/// Event name and context bindings declared by a component action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionEvent {
    /// Interaction name dispatched by the host application.
    pub name: String,
    /// Values or bindings resolved by the renderer at interaction time.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub context: Map<String, Value>,
}
