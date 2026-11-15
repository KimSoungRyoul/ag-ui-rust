//! Official renderer-to-agent schema validation before typed decoding.
//!
//! The v0.9 family calls these documents client-to-server; v1.0 calls them
//! renderer-to-agent. One validator holds messages to one version's.

use crate::{
    A2uiVersion, ClientDataModel, Error, ErrorCode, RendererMessage, Result, ValidationError,
};
use serde_json::Value;

/// Compiled, local-only schemas for renderer messages and metadata of one
/// protocol version. Reuse them when accepting multiple messages from a
/// renderer.
#[derive(Debug)]
pub struct ClientSchemaValidator {
    message: jsonschema::Validator,
    data_model: jsonschema::Validator,
}

impl ClientSchemaValidator {
    /// Compiles the pinned official schemas of v0.9.1, which accept both
    /// discriminators of the v0.9 family:
    /// [`ClientSchemaValidator::for_version`] with [`A2uiVersion::V0_9_1`].
    pub fn new() -> Result<Self> {
        Self::for_version(A2uiVersion::V0_9_1)
    }

    /// Compiles the pinned official schemas of `version`. Nothing is
    /// retrieved: what a schema refers to is registered from the documents
    /// this crate ships.
    ///
    /// ```
    /// use ag_ui_a2ui::A2uiVersion;
    /// use ag_ui_a2ui::client_schema::ClientSchemaValidator;
    /// use serde_json::json;
    ///
    /// let action = |version: &str| json!({"version": version, "action": {
    ///     "name": "submit", "surfaceId": "s", "sourceComponentId": "button",
    ///     "timestamp": "2026-09-28T00:00:00Z", "context": {}
    /// }});
    /// let v1_0 = ClientSchemaValidator::for_version(A2uiVersion::V1_0)?;
    /// v1_0.validate_message(&action("1.0"))?;
    /// assert!(v1_0.validate_message(&action("v0.9.1")).is_err());
    /// # Ok::<(), ag_ui_a2ui::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error only if a document this crate ships does not compile.
    pub fn for_version(version: A2uiVersion) -> Result<Self> {
        let (message, data_model, resources): (&str, &str, &[&str]) = match version {
            A2uiVersion::V0_9 => (
                include_str!("../../schemas/v0_9/client_to_server.json"),
                include_str!("../../schemas/v0_9/client_data_model.json"),
                &[],
            ),
            A2uiVersion::V0_9_1 => (
                include_str!("../../schemas/v0_9_1/client_to_server.json"),
                include_str!("../../schemas/v0_9_1/client_data_model.json"),
                &[],
            ),
            A2uiVersion::V1_0 => (
                include_str!("../../schemas/v1_0/renderer_to_agent.json"),
                include_str!("../../schemas/v1_0/renderer_data_model.json"),
                &[include_str!("../../schemas/v1_0/common_types.json")],
            ),
        };
        let compile = |source: &str| -> Result<_> {
            let schema: Value = serde_json::from_str(source)?;
            crate::protocol::schema_validation::compile_with(version, &schema, resources)
        };
        Ok(Self {
            message: compile(message)?,
            data_model: compile(data_model)?,
        })
    }

    /// Checks the original JSON, including required action context and error fields.
    pub fn validate_message(&self, message: &Value) -> Result<()> {
        validate(&self.message, message)
    }

    /// Validates the raw message before serde can fill in defaults.
    pub fn decode_message(&self, message: &Value) -> Result<RendererMessage> {
        self.validate_message(message)?;
        Ok(serde_json::from_value(message.clone())?)
    }

    /// Checks raw data-model metadata (`a2uiClientDataModel`, or
    /// `a2uiRendererDataModel` from v1.0) without applying it to any surface.
    pub fn validate_data_model(&self, metadata: &Value) -> Result<()> {
        validate(&self.data_model, metadata)
    }

    /// Validates and decodes client snapshots without changing server state.
    pub fn decode_data_model(&self, metadata: &Value) -> Result<ClientDataModel> {
        self.validate_data_model(metadata)?;
        Ok(serde_json::from_value(metadata.clone())?)
    }
}

fn validate(schema: &jsonschema::Validator, value: &Value) -> Result<()> {
    let errors: Vec<_> = schema
        .iter_errors(value)
        .map(|error| {
            ValidationError::new(
                ErrorCode::InvalidValue,
                error.instance_path().to_string(),
                crate::protocol::schema_validation::message(&error),
            )
        })
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(Error::Validation {
            errors: errors.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn renderer_extensions_survive_validated_roundtrips() {
        let validator = ClientSchemaValidator::new().unwrap();
        for raw in [
            json!({"version":"v0.9.1","error":{
                "code":"UNALLOWED_PARENT","surfaceId":"s","message":"Invalid parent",
                "parentComponentId":"p","childComponentId":"c","details":{"expected":["Column"]}
            }}),
            json!({"version":"v0.9","action":{
                "name":"submit","surfaceId":"s","sourceComponentId":"button",
                "timestamp":"2026-09-28T00:00:00Z","context":{},"requestId":"r1"
            }}),
        ] {
            let decoded = validator.decode_message(&raw).unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), raw);
        }
    }

    #[test]
    fn generic_extension_fields_preserve_non_string_values_and_null() {
        let validator = ClientSchemaValidator::new().unwrap();
        for value in [Value::Null, json!({"detail":"renderer-specific"})] {
            for raw in [
                json!({"version":"v0.9.1","error":{
                    "code":"CUSTOM_ERROR","surfaceId":"s","message":"bad",
                    "path":value,"functionCallId":value
                }}),
                json!({"version":"v0.9.1","action":{
                    "name":"submit","surfaceId":"s","sourceComponentId":"button",
                    "timestamp":"2026-09-28T00:00:00Z","context":{},"userMessage":value
                }}),
            ] {
                let decoded = validator.decode_message(&raw).unwrap();
                assert_eq!(serde_json::to_value(decoded).unwrap(), raw);
            }
        }
    }

    #[test]
    fn raw_validation_catches_required_fields_before_serde_defaults() {
        let validator = ClientSchemaValidator::new().unwrap();
        for raw in [
            json!({"version":"v0.9.1","error":{"code":"X","message":"bad"}}),
            json!({"version":"v0.9.1","error":{
                "code":"VALIDATION_FAILED","surfaceId":"s","message":"bad"
            }}),
            json!({"version":"v0.9.1","action":{
                "name":"submit","surfaceId":"s","sourceComponentId":"root",
                "timestamp":"2026-09-28T00:00:00Z"
            }}),
        ] {
            assert!(serde_json::from_value::<RendererMessage>(raw.clone()).is_ok());
            assert!(matches!(
                validator.decode_message(&raw),
                Err(Error::Validation { .. })
            ));
        }
    }

    #[test]
    fn validation_errors_are_closed_but_generic_errors_allow_details() {
        let mut raw = json!({"version":"v0.9.1","error":{
            "code":"VALIDATION_FAILED","surfaceId":"s","message":"bad","path":"/text"
        }});
        let parsed = RendererMessage::from_json(&raw).unwrap();
        let crate::RendererPayload::Error(error) = parsed.payload else {
            panic!("expected validation error");
        };
        assert_eq!(error.path(), Some("/text"));
        raw["error"]["details"] = json!({"expected":"string"});
        assert!(RendererMessage::from_json(&raw).is_err());
        raw["error"]["code"] = json!("INVALID_PROPERTY");
        assert!(RendererMessage::from_json(&raw).is_ok());
    }

    #[test]
    fn action_timestamps_must_have_the_required_format() {
        let raw = json!({"version":"v0.9.1","action":{
            "name":"submit","surfaceId":"s","sourceComponentId":"root",
            "timestamp":"tomorrow","context":{}
        }});
        assert!(RendererMessage::from_json(&raw).is_err());
    }

    #[test]
    fn client_data_models_preserve_form_input_and_validate_raw_metadata() {
        let validator = ClientSchemaValidator::new().unwrap();
        let raw = json!({"version":"v0.9.1","surfaces":{
            "board-a":{"note":"don’t lose my input", "rows":[{"name":"Ada"}], "optional":null}
        }});
        let model = validator.decode_data_model(&raw).unwrap();
        assert_eq!(model.surfaces["board-a"]["note"], "don’t lose my input");
        assert_eq!(serde_json::to_value(model).unwrap(), raw);
        assert!(ClientDataModel::from_json(&raw).is_ok());

        for invalid in [
            json!({"version":"v1.0","surfaces":{}}),
            json!({"version":"v0.9.1"}),
            json!({"version":"v0.9.1","surfaces":{"board-a":null}}),
            json!({"version":"v0.9.1","surfaces":{"board-a":[]}}),
            json!({"version":"v0.9.1","surfaces":{},"extra":true}),
        ] {
            assert!(matches!(
                validator.decode_data_model(&invalid),
                Err(Error::Validation { .. })
            ));
            assert!(serde_json::from_value::<ClientDataModel>(invalid).is_err());
        }
    }
}
