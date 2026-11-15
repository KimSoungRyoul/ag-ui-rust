#![cfg(feature = "schema-validation")]

use ag_ui_a2ui::{
    Error,
    schema_validation::{SchemaValidator, SurfaceValidator, validate_capabilities},
    toolkit::schema::SchemaBundle,
};
use serde_json::{Value, json};

const TEXT_VARIANTS: &str = "/components/Text/allOf/2/properties/variant/enum";

fn variant_diagnostic(bundle: &SchemaBundle) -> ag_ui_a2ui::ValidationError {
    let schema = SchemaValidator::new(bundle, &Default::default()).unwrap();
    let message = json!({"version":"v0.9.1","updateComponents":{"surfaceId":"s","components":[
        {"id":"root","component":"Text","text":"Title","variant":"huge"}
    ]}});
    let Err(Error::Validation { errors }) = schema.validate_message(&message) else {
        panic!("an unknown variant must fail");
    };
    errors
        .0
        .iter()
        .find(|error| error.path == "/updateComponents/components/0/variant")
        .unwrap_or_else(|| panic!("no diagnostic at the variant: {errors}"))
        .clone()
}

// The retry loop hands these diagnostics to the model that wrote the message.
// jsonschema 0.61 names two allowed values and counts the rest; the model needs
// all of them to pick a valid one.
#[test]
fn enum_failures_name_every_allowed_value() {
    let bundle = SchemaBundle::basic().unwrap();
    let allowed = bundle
        .catalog
        .pointer(TEXT_VARIANTS)
        .and_then(Value::as_array)
        .expect("the basic catalog's Text variants");
    assert!(allowed.len() > 2, "this needs more values than 0.61 names");
    let error = variant_diagnostic(&bundle);
    for value in allowed {
        assert!(
            error.message.contains(&value.to_string()),
            "{value} is missing from {:?}",
            error.message
        );
    }
    assert!(
        !error.message.contains("other candidates"),
        "{}",
        error.message
    );
}

#[test]
fn a_huge_enum_is_still_bounded() {
    let mut bundle = SchemaBundle::basic().unwrap();
    let values: Vec<_> = (0..500).map(|i| format!("variant-{i}")).collect();
    *bundle.catalog.pointer_mut(TEXT_VARIANTS).unwrap() = json!(values);
    let error = variant_diagnostic(&bundle);
    assert!(
        error
            .message
            .starts_with("value is not one of [\"variant-0\"")
    );
    assert_eq!(error.message.chars().count(), 385, "{}", error.message);
    assert!(error.message.ends_with('…'));
}

#[test]
fn capability_enum_failures_name_every_allowed_value() {
    let capabilities = json!({"v0.9":{"supportedCatalogIds":["c"],"inlineCatalogs":[{
        "catalogId":"c","functions":[{"name":"f","parameters":{},"returnType":"date"}]
    }]}});
    let Err(Error::Catalog(message)) = validate_capabilities(&capabilities) else {
        panic!("an unknown return type must fail");
    };
    for value in [
        "string", "number", "boolean", "array", "object", "any", "void",
    ] {
        assert!(message.contains(&format!("\"{value}\"")), "{message}");
    }
}

#[test]
fn required_properties_and_enums_report_the_matching_component_branch() {
    let schema =
        SchemaValidator::new(&SchemaBundle::basic().unwrap(), &Default::default()).unwrap();
    let message = json!({"version":"v0.9.1","updateComponents":{"surfaceId":"s","components":[
        {"id":"label","component":"Text","text":"secret data ".repeat(1000)},
        {"id":"root","component":"Button","child":"label"},
        {"id":"heading","component":"Text","text":"Title","variant":"invalid"}
    ]}});
    let Err(Error::Validation { errors }) = schema.validate_message(&message) else {
        panic!("expected a schema failure");
    };
    assert_eq!(errors.0.len(), 2, "{errors}");
    assert!(
        errors
            .0
            .iter()
            .any(|error| error.path == "/updateComponents/components/1"
                && error.message.contains("\"action\" is a required property"))
    );
    assert!(errors.0.iter().any(
        |error| error.path == "/updateComponents/components/2/variant"
            && error.message.contains("h1")
    ));
    assert!(!errors.to_string().contains("secret data"));
    assert!(
        errors
            .0
            .iter()
            .all(|error| error.message.chars().count() <= 385)
    );
}

#[test]
fn unexpected_properties_are_reported_once_per_component() {
    let schema =
        SchemaValidator::new(&SchemaBundle::basic().unwrap(), &Default::default()).unwrap();
    let message = json!({"version":"v0.9.1","updateComponents":{"surfaceId":"s","components":[
        {"id":"root","component":"Text","text":"hi","bogus":1,"more":2}
    ]}});
    let Err(Error::Validation { errors }) = schema.validate_message(&message) else {
        panic!("expected a schema failure");
    };
    assert_eq!(errors.0.len(), 1, "{errors}");
    assert_eq!(errors.0[0].path, "/updateComponents/components/0");
    assert!(errors.0[0].message.contains("'bogus', 'more'"), "{errors}");
}

#[test]
fn a_child_nested_inline_fails_the_schema_once() {
    let bundle = SchemaBundle::basic().unwrap();
    let schema = SchemaValidator::new(&bundle, &Default::default()).unwrap();
    let message = json!({"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
        {"id":"root","component":"Column","children":[{"id":"t","component":"Text","text":"hi"}]}
    ]}});
    let Err(Error::Validation { errors }) = schema.validate_message(&message) else {
        panic!("an inline child must fail the schema");
    };
    // The array arm of `ChildList` is the one the value tried; the template arm
    // failing on type alone is not a second defect.
    assert_eq!(errors.0.len(), 1, "{errors}");
    assert_eq!(
        errors.0[0].path,
        "/updateComponents/components/0/children/0"
    );

    // The stateful validator runs the schema first, so it stops there too.
    let mut surfaces = SurfaceValidator::new();
    surfaces.register(&bundle, &Default::default()).unwrap();
    surfaces
        .push(&json!({"version":"v0.9","createSurface":{
            "surfaceId":"s","catalogId": bundle.catalog["catalogId"]
        }}))
        .unwrap();
    let Err(Error::Validation { errors: pushed }) = surfaces.push(&message) else {
        panic!("an inline child must not reach the surface state");
    };
    assert_eq!(pushed, errors);
    assert!(surfaces.state().get("s").unwrap().components.is_empty());
}

#[test]
fn diagnostics_do_not_change_acceptance_and_fallbacks_are_bounded() {
    let schema =
        SchemaValidator::new(&SchemaBundle::basic().unwrap(), &Default::default()).unwrap();
    let valid = json!({"version":"v0.9.1","updateComponents":{"surfaceId":"s","components":[
        {"id":"root","component":"Button","child":"label","action":{"event":{"name":"submit"}}}
    ]}});
    schema.validate_message(&valid).unwrap();
    let invalid = json!({"unknown": "private content ".repeat(1000)});
    let Err(Error::Validation { errors }) = schema.validate_message(&invalid) else {
        panic!("invalid envelope must fail");
    };
    assert!(!errors.0.is_empty());
    assert!(errors.0.len() <= 16);
    assert!(
        errors
            .0
            .iter()
            .all(|error| error.message.chars().count() <= 385)
    );
    assert!(!errors.to_string().contains("private content"));
}
