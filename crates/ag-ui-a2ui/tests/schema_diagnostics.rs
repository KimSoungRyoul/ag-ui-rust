#![cfg(feature = "schema-validation")]

use ag_ui_a2ui::{
    Error,
    schema_validation::{SchemaValidator, SurfaceValidator},
    toolkit::schema::SchemaBundle,
};
use serde_json::json;

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
