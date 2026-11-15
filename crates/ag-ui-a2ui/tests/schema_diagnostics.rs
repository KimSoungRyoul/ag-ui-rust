#![cfg(feature = "schema-validation")]

use ag_ui_a2ui::{
    Catalog, Error, MessageProcessor,
    processor::RejectionKind,
    schema::SchemaBundle,
    schema_validation::{
        SchemaValidator, SurfaceValidator, validate_capabilities, validate_common_type,
    },
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

#[test]
fn a_value_is_held_to_one_common_type_on_its_own() {
    for (definition, accepted, refused) in [
        (
            "DynamicString",
            vec![json!("text"), json!({"path": "/name"})],
            vec![json!(3), json!({"path": "/name", "extra": 1})],
        ),
        (
            "DynamicValue",
            vec![
                json!("text"),
                json!(3),
                json!(true),
                json!(["a"]),
                json!({"call": "now"}),
            ],
            // No literal object: an object is a binding or a call, and one
            // that is both is neither.
            vec![json!({"label": "x"}), json!({"call": "now", "path": "/a"})],
        ),
        (
            "ChildList",
            vec![
                json!(["a", "b"]),
                json!({"componentId": "row", "path": "/items"}),
            ],
            vec![json!("a"), json!({"componentId": "row"})],
        ),
        (
            "Action",
            vec![
                json!({"event": {"name": "submit", "context": {"id": {"path": "/id"}}}}),
                json!({"functionCall": {"call": "openUrl", "args": {"url": "https://example.com"}}}),
            ],
            vec![
                json!({}),
                json!({"event": {"name": "a"}, "functionCall": {"call": "b"}}),
            ],
        ),
        (
            // Any function name, since there is no catalog to ask; only the
            // keys a call has.
            "FunctionCall",
            vec![
                json!({"call": "anything"}),
                json!({"call": "x", "args": {"a": 1}, "returnType": "string"}),
            ],
            vec![
                json!({"call": "x", "bogus": 1}),
                json!({"args": {}}),
                json!({"call": "x", "returnType": "nothing"}),
            ],
        ),
    ] {
        for value in accepted {
            validate_common_type(definition, &value)
                .unwrap_or_else(|error| panic!("{definition} refuses {value}: {error}"));
        }
        for value in refused {
            let error = validate_common_type(definition, &value).unwrap_err();
            let Error::Validation { errors } = error else {
                panic!("{definition} on {value}: expected validation errors, got {error}");
            };
            assert!(!errors.0.is_empty());
        }
    }
}

#[test]
fn a_common_type_failure_is_located_in_the_value_and_echoes_none_of_it() {
    let secret = "private content ".repeat(50);
    let value = json!({"event": {"name": "submit", "context": {"note": {"path": 7, "x": secret}}}});
    let Err(Error::Validation { errors }) = validate_common_type("Action", &value) else {
        panic!("a binding whose path is a number must fail");
    };
    assert!(errors.0.len() <= 16);
    assert!(
        errors
            .0
            .iter()
            .all(|error| error.path.is_empty() || error.path.starts_with('/')),
        "{errors}"
    );
    assert!(!errors.to_string().contains("private content"), "{errors}");
}

#[test]
fn a_definition_the_common_types_do_not_have_is_named_as_such() {
    let error = validate_common_type("DynamicColor", &json!("red")).unwrap_err();
    let Error::Catalog(reason) = error else {
        panic!("expected a catalog error, got {error}");
    };
    assert!(reason.contains("\"DynamicColor\""), "{reason}");
    assert!(reason.contains("DynamicString"), "{reason}");
}

/// The published VegaChart and MCP catalogs declare no `$defs/theme`, and the
/// envelope schema refers to it for a surface's theme. No validator could be
/// built for either: compiling failed on the reference.
#[test]
fn a_published_catalog_that_declares_no_theme_gets_a_validator() {
    for path in [
        "catalogs/vegachart/v0_9/catalog.json",
        "catalogs/mcp/catalog.json",
    ] {
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/conformance")
            .join(path);
        let document: Value =
            serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
        assert!(document.pointer("/$defs/theme").is_none(), "{path}");
        let mut bundle = SchemaBundle::basic().unwrap();
        bundle.catalog = document.clone();
        let schema = SchemaValidator::new(&bundle, &Default::default())
            .unwrap_or_else(|error| panic!("{path}: {error}"));

        let create = |theme: Option<Value>| {
            let mut message = json!({"version": "v0.9.1", "createSurface": {
                "surfaceId": "s", "catalogId": document["catalogId"]
            }});
            if let Some(theme) = theme {
                message["createSurface"]["theme"] = theme;
            }
            message
        };
        // No theme, and any theme that is an object.
        schema.validate_message(&create(None)).unwrap();
        schema
            .validate_message(&create(Some(
                json!({"primaryColor": "teal", "dense": true}),
            )))
            .unwrap();
        // A theme is an object, whatever the catalog says about it.
        for not_an_object in [json!("dark"), json!(["dark"]), json!(7)] {
            assert!(
                schema
                    .validate_message(&create(Some(not_an_object.clone())))
                    .is_err(),
                "{path}: {not_an_object}"
            );
        }

        // The stateful validator registers the same catalog.
        let mut surfaces = SurfaceValidator::new();
        surfaces.register(&bundle, &Default::default()).unwrap();
        surfaces.push(&create(Some(json!({"any": 1})))).unwrap();

        // A caller that hands the catalog in as a resource as well is not
        // told that it conflicts with itself.
        let id = document["$id"].as_str().unwrap().to_string();
        let resources = [(id, document.clone())].into_iter().collect();
        SchemaValidator::new(&bundle, &resources).unwrap();
    }

    // A catalog that declares a theme is still held to it.
    let schema =
        SchemaValidator::new(&SchemaBundle::basic().unwrap(), &Default::default()).unwrap();
    let off_pattern = json!({"version": "v0.9.1", "createSurface": {
        "surfaceId": "s",
        "catalogId": "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json",
        "theme": {"primaryColor": "not-a-hex-color"}
    }});
    assert!(schema.validate_message(&off_pattern).is_err());
}

/// A processor refuses what its catalog's schemas refuse once they are
/// registered, and locates it the way it locates everything else. Two
/// `process_messages` cases of the conformance suite rest on this: a theme
/// that is off the `pattern` in its catalog's theme schema, and a property a
/// component does not declare.
#[test]
fn a_processor_with_schemas_registered_makes_the_refusals_a_schema_engine_makes() {
    let bundle = SchemaBundle::basic().unwrap();
    let catalog_id = bundle.catalog_id().unwrap().to_string();
    let schema = SchemaValidator::new(&bundle, &Default::default()).unwrap();
    let processor = || {
        let mut processor = MessageProcessor::new();
        processor
            .register(Catalog::from_schema(&bundle.catalog).unwrap())
            .unwrap();
        processor
    };

    let off_pattern = json!({"version": "v0.9", "createSurface": {
        "surfaceId": "s", "catalogId": catalog_id,
        "theme": {"primaryColor": "not-a-hex-color"}
    }});
    let create = json!({"version": "v0.9", "createSurface": {
        "surfaceId": "s", "catalogId": catalog_id
    }});
    let undeclared = json!({"version": "v0.9", "updateComponents": {
        "surfaceId": "s", "components": [{
            "id": "root", "component": "Text", "text": "Hello", "secret": true
        }]
    }});
    for message in [&off_pattern, &undeclared] {
        assert!(schema.validate_message(message).is_err(), "{message}");
    }

    // Without schemas a processor checks what a catalog summary can say, and
    // neither of these is in it.
    let mut semantic = processor();
    semantic.process(&off_pattern).unwrap();
    semantic.process(&undeclared).unwrap();

    // With them, both are refused, each where it is.
    let mut checked = processor();
    checked.register_schema(&catalog_id, schema).unwrap();
    let refused = checked.process(&off_pattern).unwrap_err();
    assert_eq!(refused.kind, RejectionKind::Validation);
    assert_eq!(
        refused.errors()[0].path,
        "messages[0].createSurface.theme.primaryColor"
    );
    assert!(checked.surfaces().get("s").is_none());

    let refused = checked
        .process_all(&[create.clone(), undeclared.clone()])
        .unwrap_err();
    assert_eq!(refused.message_index, 1);
    assert_eq!(
        refused.errors()[0].path,
        "messages[1].updateComponents.components[0]"
    );
    assert!(refused.errors()[0].message.contains("secret"));
    assert!(
        checked.surfaces().get("s").is_none(),
        "a batch is all or nothing"
    );

    // A schema is for a catalog the processor holds.
    assert!(matches!(
        processor().register_schema(
            "another/catalog",
            SchemaValidator::new(&bundle, &Default::default()).unwrap()
        ),
        Err(Error::Catalog(_))
    ));
}

/// The component keys A2UI v1.0 introduces are refused before it, with or
/// without a schema engine, unless the catalog declares a property by that
/// name. The reference renderers do the same in every mode.
#[test]
fn the_component_keys_of_v1_0_are_refused_on_an_earlier_surface() {
    let catalog = Catalog::basic();
    let catalog_id = catalog.catalog_id.clone();
    let mut processor = MessageProcessor::new();
    processor.register(catalog).unwrap();
    processor
        .process(&json!({"version": "v0.9", "createSurface": {
            "surfaceId": "s", "catalogId": catalog_id
        }}))
        .unwrap();

    for key in ["@metadata", "@catalogId", "@accessibility", "@checks"] {
        let mut component = json!({"id": "root", "component": "Text", "text": "Hello"});
        component[key] = json!({});
        let refused = processor
            .process(&json!({"version": "v0.9", "updateComponents": {
                "surfaceId": "s", "components": [component]
            }}))
            .unwrap_err();
        assert_eq!(refused.kind, RejectionKind::Validation);
        assert_eq!(
            refused.errors()[0].path,
            format!("messages[0].updateComponents.components[0].{key}")
        );
    }

    // A catalog may declare a property by one of those names.
    let declares = Catalog::from_schema(&json!({
        "catalogId": "declares",
        "components": {"Note": {"type": "object", "properties": {
            "@metadata": {"type": "object"}
        }}}
    }))
    .unwrap();
    processor.register(declares).unwrap();
    processor
        .process_all(&[
            json!({"version": "v0.9", "createSurface": {"surfaceId": "n", "catalogId": "declares"}}),
            json!({"version": "v0.9", "updateComponents": {"surfaceId": "n", "components": [
                {"id": "root", "component": "Note", "@metadata": {"trace": "t1"}}
            ]}}),
        ])
        .unwrap();
}
