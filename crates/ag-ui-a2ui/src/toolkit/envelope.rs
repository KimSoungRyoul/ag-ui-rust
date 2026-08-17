//! The transport envelope: `{"a2ui_operations": [...]}`.
//!
//! A2UI itself says nothing about how messages reach the renderer. In practice
//! every toolkit wraps a batch of operations in a single JSON object keyed by
//! [`A2UI_OPERATIONS_KEY`], and the frontend sniffs for exactly that key to
//! decide whether a payload is A2UI. The envelope is emitted as a JSON *string*
//! because that is what fits in a tool result, an assistant message, or an A2A
//! data part without further wrapping.

use serde_json::{Map, Value, json};

use crate::constants::A2UI_OPERATIONS_KEY;
use crate::error::Result;
use crate::message::AgentMessage;
use crate::validate::ValidationError;

/// Wraps operations in the envelope, as a JSON string ready for transport.
///
/// ```
/// use ag_ui_a2ui::message::AgentMessage;
/// use ag_ui_a2ui::toolkit::envelope::wrap_as_operations_envelope;
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

/// The envelope as a [`Value`], for callers that embed it in a larger payload.
///
/// # Errors
///
/// Returns [`Error::Json`](crate::Error::Json) if an operation cannot be
/// serialized.
pub fn operations_envelope(operations: &[AgentMessage]) -> Result<Value> {
    let operations = serde_json::to_value(operations)?;
    let mut envelope = Map::new();
    envelope.insert(A2UI_OPERATIONS_KEY.to_string(), operations);
    Ok(Value::Object(envelope))
}

/// Builds an envelope reporting that a surface could not be produced.
///
/// The operations list is present but empty so a frontend keyed on
/// [`A2UI_OPERATIONS_KEY`] still recognizes the payload and can clear any
/// pending state, with the failure carried alongside in the shape the
/// specification defines for renderer-bound errors: `code`, `surfaceId`, `path`
/// and `message`, plus the full validation list under `details` for callers that
/// want to route on the codes.
///
/// # Errors
///
/// Returns [`Error::Json`](crate::Error::Json) if the error list cannot be
/// serialized.
pub fn wrap_error_envelope(
    surface_id: &str,
    message: &str,
    errors: &[ValidationError],
) -> Result<String> {
    let mut error = Map::new();
    error.insert("code".to_string(), json!("VALIDATION_FAILED"));
    error.insert("surfaceId".to_string(), json!(surface_id));
    error.insert("message".to_string(), json!(message));
    // The spec's `path` is a single locator; use the first failure's, which is
    // the one a reader should look at first.
    error.insert(
        "path".to_string(),
        json!(errors.first().map_or("components", |e| e.path.as_str())),
    );
    error.insert("details".to_string(), serde_json::to_value(errors)?);

    let mut envelope = Map::new();
    envelope.insert(A2UI_OPERATIONS_KEY.to_string(), json!([]));
    envelope.insert("error".to_string(), Value::Object(error));
    Ok(serde_json::to_string(&Value::Object(envelope))?)
}

/// Whether a value is an A2UI operations envelope.
///
/// This is the frontend's content sniff: presence of the key, carrying an array.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Component;
    use crate::validate::ErrorCode;

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
    fn the_error_envelope_keeps_the_key_and_carries_the_codes() {
        let errors = vec![
            ValidationError::new(
                ErrorCode::NoRoot,
                "components",
                "No component has id 'root'.",
            ),
            ValidationError::new(
                ErrorCode::UnresolvedChild,
                "components[1].child",
                "'gone' is not defined.",
            ),
        ];
        let json = wrap_error_envelope("s1", "could not build the surface", &errors).unwrap();
        let value: Value = serde_json::from_str(&json).unwrap();

        assert!(is_operations_envelope(&value));
        assert_eq!(value["a2ui_operations"], json!([]));
        assert_eq!(value["error"]["code"], "VALIDATION_FAILED");
        assert_eq!(value["error"]["surfaceId"], "s1");
        assert_eq!(value["error"]["path"], "components");
        assert_eq!(value["error"]["details"][1]["code"], "unresolved_child");
    }

    #[test]
    fn an_error_envelope_with_no_errors_still_has_a_path() {
        let json = wrap_error_envelope("s1", "model returned nothing", &[]).unwrap();
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["error"]["path"], "components");
    }

    #[test]
    fn unwrapping_a_non_envelope_is_an_error() {
        assert!(unwrap_operations_envelope(&json!({"nope": []})).is_err());
    }
}
