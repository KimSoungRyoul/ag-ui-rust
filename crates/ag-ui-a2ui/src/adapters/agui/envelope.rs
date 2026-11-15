//! The tool-result envelope: `{"a2ui_operations": [...]}`, and the two shapes
//! of failure.
//!
//! A2UI itself says nothing about how messages reach the renderer. On AG-UI a
//! batch of operations is one JSON object keyed by [`A2UI_OPERATIONS_KEY`], in
//! the content of a tool result, and whatever reads a tool result tests for
//! exactly that key to decide whether it is a surface. The envelope is a JSON
//! *string*, because that is what a tool result carries.
//!
//! # Function calls
//!
//! The envelope carries the operations that change a surface, and no other
//! message. AG-UI's middleware knows four (`A2UIMessageType`,
//! `middlewares/a2ui-middleware/src/types.ts:155-161` at
//! `ag-ui-protocol/ag-ui` commit `2330368a`), and neither it nor AG-UI's
//! toolkit names `callRendererFunction`, `callAgentFunction` or their
//! responses anywhere. So this adapter has no mapping for a function call and
//! invents none: [`operations_envelope`] refuses a batch that holds one, and
//! with it [`SurfacePainter::settle`](super::SurfacePainter::settle) and
//! `paint`, rather than drop the call or put it where no frontend reads it.
//! A call travels by a transport that carries A2UI messages as they are, such
//! as A2A, where a data part holds any message (`adapters::a2a`).
//!
//! A failure is a different object, never an empty envelope: a result that
//! carried the key would be read as a surface. A tool that could not produce a
//! surface answers `{"error": …}` ([`error_envelope`]), and one that gave up
//! after its attempts adds `"code": "a2ui_recovery_exhausted"` and the
//! attempts ([`recovery_exhausted_envelope`]), which is the shape AG-UI's
//! middleware turns into a failed surface.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::agent::Feedback;
use crate::constants::A2UI_OPERATIONS_KEY;
use crate::error::{Error, Result};
use crate::protocol::message::AgentMessage;
use crate::protocol::validate::{ErrorCode, ValidationError};

/// The `code` of the envelope a tool returns when its attempts are used up.
pub const RECOVERY_EXHAUSTED_CODE: &str = "a2ui_recovery_exhausted";

/// Wraps operations in the envelope, as a JSON string ready for a tool result.
///
/// ```
/// use ag_ui_a2ui::AgentMessage;
/// use ag_ui_a2ui::adapters::agui::wrap_as_operations_envelope;
///
/// let json = wrap_as_operations_envelope(&[AgentMessage::delete_surface("s1")]).unwrap();
/// assert!(json.starts_with(r#"{"a2ui_operations":["#));
/// ```
///
/// # Errors
///
/// Returns [`Error::Json`](crate::Error::Json) if an operation cannot be
/// serialized.
pub fn wrap_as_operations_envelope(operations: &[AgentMessage]) -> Result<String> {
    Ok(serde_json::to_string(&operations_envelope(operations)?)?)
}

/// The envelope as a [`Value`]: the content of a surface activity, or part of
/// a larger payload.
///
/// # Errors
///
/// Returns [`Error::Json`](crate::Error::Json) if an operation cannot be
/// serialized, and [`Error::Validation`](crate::Error::Validation) when one
/// is a function call (`callRendererFunction`, `agentFunctionResponse`),
/// which the envelope has no place for. AG-UI's middleware knows the four
/// operations that change a surface (`A2UIMessageType`,
/// `middlewares/a2ui-middleware/src/types.ts:155-161` at
/// `ag-ui-protocol/ag-ui` commit `2330368a`) and names no function call, so
/// this adapter has no mapping for one and invents none: the batch is refused,
/// and the call is neither dropped nor put where no frontend reads it. See
/// [the adapter's documentation](super#function-calls).
pub fn operations_envelope(operations: &[AgentMessage]) -> Result<Value> {
    let calls: Vec<ValidationError> = operations
        .iter()
        .enumerate()
        .filter(|(_, operation)| !operation.payload.operation().changes_a_surface())
        .map(|(index, operation)| {
            let key = operation.payload.operation();
            ValidationError::new(
                ErrorCode::InvalidValue,
                format!("messages[{index}].{key}"),
                format!(
                    "'{key}' is a function call, and AG-UI's A2UI operations envelope carries \
                     only the operations that change a surface. Send it by a transport that \
                     carries A2UI messages, such as A2A."
                ),
            )
        })
        .collect();
    if !calls.is_empty() {
        return Err(Error::Validation {
            errors: calls.into(),
        });
    }
    let operations = serde_json::to_value(operations)?;
    let mut envelope = Map::new();
    envelope.insert(A2UI_OPERATIONS_KEY.to_string(), operations);
    Ok(Value::Object(envelope))
}

/// Whether a value is an operations envelope: the key is there, and holds an
/// array.
///
/// A failure from [`error_envelope`] or [`recovery_exhausted_envelope`] does
/// not have the key, which is what keeps it from being read as a surface.
pub fn is_operations_envelope(value: &Value) -> bool {
    value.get(A2UI_OPERATIONS_KEY).is_some_and(Value::is_array)
}

/// Reads operations back out of an envelope.
///
/// # Errors
///
/// Returns [`Error::Parse`](crate::Error::Parse) if the value is not an
/// envelope, or [`Error::Json`](crate::Error::Json) if its operations do not
/// deserialize.
pub fn unwrap_operations_envelope(value: &Value) -> Result<Vec<AgentMessage>> {
    let operations = value.get(A2UI_OPERATIONS_KEY).ok_or_else(|| {
        crate::Error::parse(format!("payload has no '{A2UI_OPERATIONS_KEY}' key"))
    })?;
    Ok(serde_json::from_value(operations.clone())?)
}

/// What a tool returns when it cannot produce a surface: `{"error": message}`.
///
/// ```
/// use ag_ui_a2ui::adapters::agui::{error_envelope, is_operations_envelope};
///
/// let value: serde_json::Value = serde_json::from_str(&error_envelope("no catalog")).unwrap();
/// assert_eq!(value, serde_json::json!({"error": "no catalog"}));
/// assert!(!is_operations_envelope(&value));
/// ```
pub fn error_envelope(message: &str) -> String {
    json!({"error": message}).to_string()
}

/// One attempt at generating a surface, as a failure reports it.
///
/// A later version may add a field, so one is built with
/// [`Attempt::rejected`], [`Attempt::failed`] or [`Attempt::stood`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Attempt {
    /// Which attempt, counting from one.
    pub attempt: u32,
    /// Whether the attempt's answer stood.
    pub ok: bool,
    /// What was wrong with it; empty for one that stood.
    pub errors: Vec<ValidationError>,
}

impl Attempt {
    /// The attempt a [`RetryDriver`](crate::agent::RetryDriver) turned down
    /// with `feedback`.
    pub fn rejected(feedback: &Feedback) -> Self {
        Self::failed(feedback.attempt(), feedback.errors().to_vec())
    }

    /// Attempt number `attempt`, counting from one, whose answer was turned
    /// down for `errors`.
    pub fn failed(attempt: u32, errors: Vec<ValidationError>) -> Self {
        Self {
            attempt,
            ok: false,
            errors,
        }
    }

    /// Attempt number `attempt`, counting from one, whose answer stood.
    pub fn stood(attempt: u32) -> Self {
        Self {
            attempt,
            ok: true,
            errors: Vec::new(),
        }
    }
}

/// What a tool returns when every attempt was rejected: the error, the code
/// [`RECOVERY_EXHAUSTED_CODE`], and the attempts.
///
/// ```
/// use ag_ui_a2ui::adapters::agui::{Attempt, recovery_exhausted_envelope};
///
/// let attempts = [Attempt::failed(1, Vec::new())];
/// let value: serde_json::Value =
///     serde_json::from_str(&recovery_exhausted_envelope(&attempts).unwrap()).unwrap();
/// assert_eq!(value["error"], "Failed to generate valid A2UI after 1 attempt(s)");
/// assert_eq!(value["code"], "a2ui_recovery_exhausted");
/// assert_eq!(value["attempts"][0]["attempt"], 1);
/// ```
///
/// # Errors
///
/// Returns [`Error::Json`](crate::Error::Json) if an attempt cannot be
/// serialized.
pub fn recovery_exhausted_envelope(attempts: &[Attempt]) -> Result<String> {
    Ok(json!({
        "error": format!("Failed to generate valid A2UI after {} attempt(s)", attempts.len()),
        "code": RECOVERY_EXHAUSTED_CODE,
        "attempts": serde_json::to_value(attempts)?,
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::message::Component;
    use crate::protocol::validate::ErrorCode;

    fn ops() -> Vec<AgentMessage> {
        vec![
            AgentMessage::create_surface("s1", "cat"),
            AgentMessage::update_components(
                "s1",
                vec![Component::new("root", "Text").with("text", json!("hi"))],
            ),
        ]
    }

    #[test]
    fn the_envelope_round_trips() {
        let json = wrap_as_operations_envelope(&ops()).unwrap();
        let value: Value = serde_json::from_str(&json).unwrap();
        assert!(is_operations_envelope(&value));
        assert_eq!(unwrap_operations_envelope(&value).unwrap(), ops());
    }

    #[test]
    fn the_envelope_key_is_the_frontend_sniff() {
        let value: Value =
            serde_json::from_str(&wrap_as_operations_envelope(&ops()).unwrap()).unwrap();
        assert!(value.get("a2ui_operations").is_some());
        assert!(!is_operations_envelope(&json!({"operations": []})));
        assert!(!is_operations_envelope(&json!({"a2ui_operations": "nope"})));
    }

    #[test]
    fn an_empty_batch_is_still_a_valid_envelope() {
        let json = wrap_as_operations_envelope(&[]).unwrap();
        assert_eq!(json, r#"{"a2ui_operations":[]}"#);
    }

    #[test]
    fn a_failure_does_not_satisfy_the_frontend_sniff() {
        let attempts = [Attempt {
            attempt: 1,
            ok: false,
            errors: vec![ValidationError::new(
                ErrorCode::NoRoot,
                "components",
                "No component has id 'root'.",
            )],
        }];
        for failure in [
            error_envelope("could not build the surface"),
            recovery_exhausted_envelope(&attempts).unwrap(),
        ] {
            let value: Value = serde_json::from_str(&failure).unwrap();
            // A failure that carried the key would be replayed later as a
            // surface that was never rendered.
            assert!(!is_operations_envelope(&value), "{value}");
            assert!(unwrap_operations_envelope(&value).is_err());
        }
    }

    #[test]
    fn an_exhausted_recovery_carries_its_attempts_as_upstream_reads_them() {
        let attempts = [
            Attempt {
                attempt: 1,
                ok: false,
                errors: vec![ValidationError::new(
                    ErrorCode::UnresolvedChild,
                    "messages[1].updateComponents.components[0].child",
                    "'gone' is not defined.",
                )],
            },
            Attempt {
                attempt: 2,
                ok: false,
                errors: Vec::new(),
            },
        ];
        let value: Value =
            serde_json::from_str(&recovery_exhausted_envelope(&attempts).unwrap()).unwrap();
        assert_eq!(
            value["error"],
            "Failed to generate valid A2UI after 2 attempt(s)"
        );
        assert_eq!(value["code"], RECOVERY_EXHAUSTED_CODE);
        assert_eq!(
            value["attempts"][0],
            json!({"attempt": 1, "ok": false, "errors": [{
                "code": "unresolved_child",
                "path": "messages[1].updateComponents.components[0].child",
                "message": "'gone' is not defined.",
            }]})
        );
    }

    #[test]
    fn unwrapping_a_non_envelope_is_an_error() {
        assert!(unwrap_operations_envelope(&json!({"nope": []})).is_err());
    }
}
