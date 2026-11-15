use ag_ui_a2ui::binding::{ModelScope, Scope};
use ag_ui_a2ui::{Catalog, Component, DataModel, ErrorCode, ValidateOptions, Validator};
use serde_json::json;

fn nested_components() -> Vec<Component> {
    vec![
        Component::new("root", "Column")
            .with("children", json!({"path":"/groups","componentId":"group"})),
        Component::new("group", "Column")
            .with("children", json!({"path":"members","componentId":"member"})),
        Component::new("member", "Text").with("text", json!({"path":"name"})),
    ]
}

#[test]
fn relative_pointer_escapes_and_empty_segments_are_preserved_in_both_scopes() {
    let data = json!({"groups":{"": [{"a/b":"slash", "a~b":"tilde", "":"empty"}]}});
    let model = DataModel::from(data.clone());
    let scope = Scope::root(&data).item("/groups/", 0);
    let lossless = ModelScope::root(&model).item("/groups/", 0);
    assert_eq!(scope.base(), "/groups//0");
    assert_eq!(lossless.base(), scope.base());
    for (path, expected) in [("a~1b", "slash"), ("a~0b", "tilde")] {
        assert_eq!(scope.resolve_string(path), expected);
        assert_eq!(
            lossless.resolve(path).unwrap().unwrap().to_json().unwrap(),
            json!(expected)
        );
    }
    assert!(scope.resolve("a~2b").is_none());
    assert!(lossless.resolve("a~2b").is_err());
}

/// The empty path and `.` are relative: inside a template they are the item
/// being drawn, not the whole model. Before, both scopes read the empty path as
/// the root, so a template over a list of lists iterated the root instead of
/// each inner list, and a `Text` bound to `""` showed the whole model.
#[test]
fn the_empty_path_inside_a_template_is_the_item_and_not_the_whole_model() {
    let catalog = Catalog::basic();
    let validator = Validator::with_options(
        &catalog,
        ValidateOptions {
            require_bound_values: true,
            ..Default::default()
        },
    );
    for own in ["", "."] {
        let components = vec![
            Component::new("root", "Column")
                .with("children", json!({"path": "/rows", "componentId": "row"})),
            Component::new("row", "Row")
                .with("children", json!({"path": own, "componentId": "cell"})),
            Component::new("cell", "Text").with("text", json!({"path": own})),
        ];
        let rows = json!({"rows": [["a", "b"], ["c"]]});
        let report = validator.validate_surface(&components, Some(&rows));
        assert!(report.is_valid(), "{own:?}: {report:?}");
        assert!(
            validator
                .validate_model(&components, &DataModel::from(rows))
                .is_valid()
        );

        // An inner item that is not a list is found where it is.
        let ragged = json!({"rows": [["a"], "not a list"]});
        let report = validator.validate_surface(&components, Some(&ragged));
        assert_eq!(report.errors.len(), 1, "{own:?}: {report:?}");
        assert_eq!(report.errors[0].code, ErrorCode::UnresolvedBinding);
        assert!(
            report.errors[0].message.contains("'/rows/1'"),
            "{}",
            report.errors[0].message
        );
    }

    // Outside a template both spellings are the whole model, and neither is
    // reported as a relative path that needs a template around it.
    for own in ["", "."] {
        let components = [Component::new("root", "Text").with("text", json!({"path": own}))];
        let report = validator.validate_surface(&components, Some(&json!({"a": 1})));
        assert!(report.is_valid(), "{own:?}: {report:?}");
    }
}

#[test]
fn nested_templates_validate_every_item_independent_of_component_order() {
    let catalog = Catalog::basic();
    let validator = Validator::with_options(
        &catalog,
        ValidateOptions {
            require_bound_values: true,
            ..Default::default()
        },
    );
    let mut components = nested_components();
    components.reverse();
    let data = json!({"groups":[{"members":[{"name":"Ada"}]},{"members":[{"name":"Lin"}]}]});
    assert!(
        validator
            .validate_surface(&components, Some(&data))
            .is_valid()
    );
    assert!(
        validator
            .validate_model(&components, &DataModel::from(data))
            .is_valid()
    );
    let missing = json!({"groups":[{"members":[{"name":"Ada"}]},{"members":[{}]}]});
    let report = validator.validate_surface(&components, Some(&missing));
    assert_eq!(report.errors.len(), 1, "{report:?}");
    assert_eq!(report.errors[0].path, "components[0].text");
    assert!(
        report.errors[0]
            .message
            .contains("/groups/1/members/0/name")
    );
}

#[test]
fn optional_existence_lint_does_not_hide_existing_collection_type_errors() {
    let catalog = Catalog::basic();
    let validator = Validator::new(&catalog);
    let components = nested_components();
    for data in [json!({}), json!({"groups":[]}), json!({"groups":[{}]})] {
        assert!(
            validator
                .validate_surface(&components, Some(&data))
                .is_valid()
        );
    }
    for bad in [
        json!({"groups":false}),
        json!({"groups":[{"members":[]},{"members":{}}]}),
    ] {
        let report = validator.validate_surface(&components, Some(&bad));
        assert_eq!(report.errors.len(), 1, "{report:?}");
        assert!(report.errors[0].message.contains("must point at an array"));
    }
}

#[test]
fn malformed_relative_pointers_fail_even_without_collection_data() {
    let catalog = Catalog::basic();
    let mut components = nested_components();
    components[2]
        .props
        .insert("text".into(), json!({"path":"bad~2escape"}));
    for report in [
        Validator::new(&catalog).validate(&components),
        Validator::new(&catalog).validate_surface(&components, Some(&json!({"groups":[]}))),
    ] {
        assert_eq!(report.errors.len(), 1, "{report:?}");
        assert_eq!(report.errors[0].code, ErrorCode::UnresolvedBinding);
        assert!(report.errors[0].message.contains("Invalid path syntax"));
    }
}

#[test]
fn nonnumeric_array_indices_fail_with_optional_missing_value_checks() {
    let catalog = Catalog::basic();
    let report = Validator::new(&catalog).validate_surface(
        &[Component::new("root", "Text").with("text", json!({"path":"/items/first"}))],
        Some(&json!({"items":["Ada"]})),
    );
    assert_eq!(report.errors.len(), 1, "{report:?}");
    assert!(report.errors[0].message.contains("canonical array index"));
}

#[test]
fn null_is_a_value_with_a_type_rather_than_a_missing_property() {
    let catalog = Catalog::basic();
    let report = Validator::new(&catalog)
        .validate(&[Component::new("root", "Text").with("text", json!(null))]);
    assert_eq!(report.errors.len(), 1, "{report:?}");
    assert_eq!(report.errors[0].code, ErrorCode::TypeMismatch);
}

#[test]
fn empty_collection_path_segment_is_preserved_by_validation() {
    let catalog = Catalog::basic();
    let validator = Validator::with_options(
        &catalog,
        ValidateOptions {
            require_bound_values: true,
            ..Default::default()
        },
    );
    let components = vec![
        Component::new("root", "Column").with(
            "children",
            json!({"path":"/groups/","componentId":"member"}),
        ),
        Component::new("member", "Text").with("text", json!({"path":"name"})),
    ];
    let data = json!({"groups":{"": [{"name":"Ada"}]}});
    assert!(
        validator
            .validate_surface(&components, Some(&data))
            .is_valid()
    );
    assert!(
        validator
            .validate_model(&components, &DataModel::from(data))
            .is_valid()
    );
}

#[test]
#[cfg(feature = "author")]
fn required_nullable_custom_property_is_present_but_still_type_checked() {
    use ag_ui_a2ui::{A2uiAuthor, A2uiVersion, schema::SchemaBundle};
    let bundle = SchemaBundle::from_inline_catalog(json!({
        "catalogId":"example.com:nullable",
        "components":{"Nullable":{"type":"object",
            "properties":{"value":{"type":["string","null"]}},"required":["value"]}}
    }))
    .unwrap();
    let author = A2uiAuthor::new(A2uiVersion::V0_9_1, bundle, Default::default()).unwrap();
    let mut messages = vec![
        json!({"version":"v0.9.1","createSurface":{"surfaceId":"s","catalogId":"example.com:nullable"}}),
        json!({"version":"v0.9.1","updateComponents":{"surfaceId":"s","components":[{"id":"root","component":"Nullable","value":null}]}}),
    ];
    author.validate_create("s", &messages).unwrap();
    messages[1]["updateComponents"]["components"][0]["value"] = json!(42);
    assert!(author.validate_create("s", &messages).is_err());
    messages[1]["updateComponents"]["components"][0]
        .as_object_mut()
        .unwrap()
        .remove("value");
    assert!(author.validate_create("s", &messages).is_err());
}

/// Collects the located codes of a failed validation.
fn located(error: ag_ui_a2ui::Error) -> Vec<(ErrorCode, String)> {
    let ag_ui_a2ui::Error::Validation { errors } = error else {
        panic!("expected a validation error, got {error:?}");
    };
    errors
        .0
        .into_iter()
        .map(|error| (error.code, error.path))
        .collect()
}

/// `validate_json_messages` names the message and the position in it that last
/// wrote a failing component. `validate_updates` used to name the component's
/// index in the whole folded surface instead, which is not a place in anything
/// the caller sent.
#[test]
fn validate_updates_names_the_message_that_wrote_the_component() {
    use ag_ui_a2ui::{AgentMessage, SurfaceStore};
    let catalog = Catalog::basic();
    let validator = Validator::new(&catalog);

    // Prior state built from three messages of its own, whose numbering must
    // not leak into the locators of the batch validated against it.
    let mut prior = SurfaceStore::new();
    for message in [
        AgentMessage::create_surface("s", "c"),
        AgentMessage::update_components(
            "s",
            vec![
                Component::new("root", "Column").with("children", json!(["a", "b"])),
                Component::new("a", "Text").with("text", json!("a")),
            ],
        ),
        AgentMessage::update_components(
            "s",
            vec![Component::new("b", "Text").with("text", json!("b"))],
        ),
    ] {
        prior.apply(&message).unwrap();
    }

    let updates = [
        AgentMessage::update_data_model("s", "/x", json!(1)),
        AgentMessage::update_components(
            "s",
            vec![
                Component::new("c", "Text").with("text", json!("c")),
                // Redefines the second component of the surface from the
                // second position of the second message.
                Component::new("a", "Card").with("child", json!("missing")),
            ],
        ),
    ];
    let error = validator.validate_updates(&prior, &updates).unwrap_err();
    assert_eq!(
        located(error),
        vec![(
            ErrorCode::UnresolvedChild,
            "messages[1].updateComponents.components[1].child".to_string()
        )]
    );

    // The candidate store of a successful batch carries no stale numbering
    // into the next one either.
    let next = validator
        .validate_updates(
            &prior,
            &[AgentMessage::update_components(
                "s",
                vec![Component::new("c", "Text").with("text", json!("c"))],
            )],
        )
        .unwrap();
    let error = validator
        .validate_updates(
            &next,
            &[AgentMessage::update_components(
                "s",
                vec![Component::new("c", "Card").with("child", json!("missing"))],
            )],
        )
        .unwrap_err();
    assert_eq!(
        located(error),
        vec![(
            ErrorCode::UnresolvedChild,
            "messages[0].updateComponents.components[0].child".to_string()
        )]
    );
}

/// A component the batch did not write has no message in it to name. It keeps
/// the locator its position in the surface gives it, which is also its position
/// in the prior state the caller supplied.
#[test]
fn validate_updates_locates_a_prior_component_by_its_position_in_the_surface() {
    use ag_ui_a2ui::{AgentMessage, SurfaceStore};
    let catalog = Catalog::basic();
    let mut prior = SurfaceStore::new();
    prior
        .apply(&AgentMessage::create_surface("s", "c"))
        .unwrap();
    prior
        .apply(&AgentMessage::update_components(
            "s",
            vec![
                Component::new("root", "Column").with("children", json!(["a", "ghost"])),
                Component::new("a", "Text").with("text", json!("a")),
            ],
        ))
        .unwrap();

    let error = Validator::new(&catalog)
        .validate_updates(
            &prior,
            &[AgentMessage::update_data_model("s", "/x", json!(1))],
        )
        .unwrap_err();
    assert_eq!(
        located(error),
        vec![(
            ErrorCode::UnresolvedChild,
            "components[0].children[1]".to_string()
        )]
    );
}

/// The stateful validator numbers the messages it accepts, so `push` and
/// `finish` locate a failure the same way.
#[test]
#[cfg(feature = "schema-validation")]
fn the_stateful_validator_names_the_message_that_wrote_the_component() {
    use ag_ui_a2ui::schema::SchemaBundle;
    use ag_ui_a2ui::schema_validation::SurfaceValidator;
    let bundle = SchemaBundle::basic().unwrap();
    let catalog_id = ag_ui_a2ui::constants::OFFICIAL_BASIC_CATALOG_ID;
    let update = |components: serde_json::Value| json!({"version": "v0.9", "updateComponents": {"surfaceId": "s", "components": components}});

    let mut stream = SurfaceValidator::new();
    stream.register(&bundle, &Default::default()).unwrap();
    stream
        .push(&json!({"version": "v0.9", "createSurface": {"surfaceId": "s", "catalogId": catalog_id}}))
        .unwrap();
    stream
        .push(&update(json!([
            {"id": "root", "component": "Column", "children": ["title", "later"]},
            {"id": "title", "component": "Text", "text": "Title"}
        ])))
        .unwrap();

    // `finish` holds the surface to the whole contract: the dangling child is
    // in the first component of the second message.
    assert_eq!(
        located(stream.finish().unwrap_err()),
        vec![(
            ErrorCode::UnresolvedChild,
            "messages[1].updateComponents.components[0].children[1]".to_string()
        )]
    );

    // `push` rejects a loop where it is written. The rejected message would
    // have been the third, and takes no number: the next one accepted is.
    let looping = update(json!([
        {"id": "later", "component": "Text", "text": "Later"},
        {"id": "title", "component": "Card", "child": "title"}
    ]));
    assert_eq!(
        located(stream.push(&looping).unwrap_err()),
        vec![(
            ErrorCode::ChildCycle,
            "messages[2].updateComponents.components[1].child".to_string()
        )]
    );
    stream
        .push(&update(json!([
            {"id": "later", "component": "Card", "child": "nowhere"}
        ])))
        .unwrap();
    assert_eq!(
        located(stream.finish().unwrap_err()),
        vec![(
            ErrorCode::UnresolvedChild,
            "messages[2].updateComponents.components[0].child".to_string()
        )]
    );

    // Resumed from observed state, nothing in that state has a message to
    // name, and the messages pushed from here are numbered from zero.
    let mut resumed = SurfaceValidator::from_state(stream.state().clone());
    resumed.register(&bundle, &Default::default()).unwrap();
    assert_eq!(
        located(resumed.finish().unwrap_err()),
        vec![(
            ErrorCode::UnresolvedChild,
            "components[2].child".to_string()
        )]
    );
    resumed
        .push(&update(json!([
            {"id": "extra", "component": "Card", "child": "also-nowhere"}
        ])))
        .unwrap();
    assert_eq!(
        located(resumed.finish().unwrap_err()),
        vec![
            (
                ErrorCode::UnresolvedChild,
                "components[2].child".to_string()
            ),
            (
                ErrorCode::UnresolvedChild,
                "messages[0].updateComponents.components[0].child".to_string()
            ),
        ]
    );
}

/// Composition failures are found on the folded surface too, and are located
/// the same way as the semantic ones.
#[test]
#[cfg(feature = "schema-validation")]
fn the_stateful_validator_locates_a_composition_failure_in_its_message() {
    use ag_ui_a2ui::schema::SchemaBundle;
    use ag_ui_a2ui::schema_validation::SurfaceValidator;
    let child_list = "https://a2ui.org/specification/v0_9/common_types.json#/$defs/ChildList";
    let bundle = SchemaBundle::from_inline_catalog(json!({
        "catalogId": "example.com:menu",
        "components": {
            "Menu": {
                "type": "object",
                "allowedChildren": ["MenuItem"],
                "properties": {"children": {"$ref": child_list}},
                "required": ["children"]
            },
            "MenuItem": {"type": "object", "properties": {"label": {"type": "string"}}},
            "Text": {"type": "object", "properties": {"text": {"type": "string"}}}
        }
    }))
    .unwrap();

    let mut stream = SurfaceValidator::new();
    stream.register(&bundle, &Default::default()).unwrap();
    for message in [
        json!({"version": "v0.9", "createSurface": {"surfaceId": "s", "catalogId": "example.com:menu"}}),
        json!({"version": "v0.9", "updateComponents": {"surfaceId": "s", "components": [
            {"id": "item", "component": "MenuItem", "label": "One"},
            {"id": "root", "component": "Menu", "children": ["item", "t"]}
        ]}}),
        json!({"version": "v0.9", "updateComponents": {"surfaceId": "s", "components": [
            {"id": "t", "component": "Text", "text": "not a menu item"}
        ]}}),
    ] {
        stream.push(&message).unwrap();
    }

    let error = stream.finish().unwrap_err();
    let ag_ui_a2ui::Error::Catalog(detail) = &error else {
        panic!("expected a composition failure, got {error:?}");
    };
    assert!(
        detail.contains(r#"path: "messages[1].updateComponents.components[1].children[1]""#),
        "{detail}"
    );
}

/// The specification writes `Icon.name` as
/// `oneOf: [{"enum": […]}, {svgPath}, DataBinding]`. `Catalog::from_schema`
/// read only a property's own `enum`, so a catalog parsed from a document had
/// no enum check for a property shaped like that, where the built-in basic
/// catalog, transcribed by hand, did.
#[test]
fn a_custom_catalog_checks_an_enum_nested_in_a_one_of() {
    let catalog = Catalog::from_schema(&json!({
        "catalogId": "example.com:badges",
        "components": {
            "Badge": {
                "type": "object",
                "properties": {
                    "icon": {"oneOf": [
                        {"type": "string", "enum": ["check", "close"]},
                        {"type": "object", "properties": {"svgPath": {"type": "string"}}},
                        {"$ref": "common_types.json#/$defs/DataBinding"}
                    ]}
                }
            }
        }
    }))
    .unwrap();
    let validator = Validator::new(&catalog);
    let badge = |icon: serde_json::Value| vec![Component::new("root", "Badge").with("icon", icon)];

    let report = validator.validate(&badge(json!("tick")));
    assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
    assert_eq!(report.errors[0].code, ErrorCode::InvalidValue);
    assert_eq!(report.errors[0].path, "components[0].icon");
    assert!(
        report.errors[0]
            .message
            .contains("Use one of: check, close."),
        "{}",
        report.errors[0].message
    );

    // A listed literal, the object alternative and a binding are all the
    // schema's own alternatives, and none of them is a literal off the list.
    for accepted in [
        json!("check"),
        json!({"svgPath": "M0 0h24v24H0z"}),
        json!({"path": "/icon"}),
    ] {
        let report = validator.validate(&badge(accepted.clone()));
        assert!(report.is_valid(), "{accepted}: {:?}", report.errors);
    }
}

/// The same check through the specification's own catalog documents, which
/// is what the basic catalog of each version is read from.
#[test]
fn the_published_basic_catalogs_reject_an_icon_they_do_not_list() {
    for &version in ag_ui_a2ui::A2uiVersion::ALL {
        let catalog = Catalog::basic_for(version);
        let options = ValidateOptions {
            check_required_props: false,
            ..ValidateOptions::full_surface()
        };
        let validator = Validator::with_options(&catalog, options).speaking(version);

        let report =
            validator.validate(&[Component::new("root", "Icon").with("name", json!("rocket"))]);
        assert_eq!(
            report
                .errors
                .iter()
                .map(|error| (error.code, error.path.as_str()))
                .collect::<Vec<_>>(),
            vec![(ErrorCode::InvalidValue, "components[0].name")],
            "{version}"
        );
        for accepted in [json!("accountCircle"), json!({"svgPath": "M0 0"})] {
            let report =
                validator.validate(&[Component::new("root", "Icon").with("name", accepted)]);
            assert!(report.is_valid(), "{version}: {:?}", report.errors);
        }
    }
}

/// `Error::Pointer` carries the offending path, and its message is fed back to
/// a model. A path long enough to be refused for its length is exactly the one
/// that used to make the longest message; the message now quotes a bounded
/// part of it, on every route a model reads.
#[test]
fn a_very_long_path_does_not_make_a_very_long_message() {
    use ag_ui_a2ui::DataModelUpdate;
    use ag_ui_a2ui::error::MAX_SHOWN_PATH_CHARS;
    let catalog = Catalog::basic();
    let long = "/segment".repeat(10_000);
    // What a message may spend on a path: the bound, the ellipsis and the
    // stated length, with room for the sentence around them.
    let budget = MAX_SHOWN_PATH_CHARS + 200;

    // Straight from the data model.
    let error = DataModel::default()
        .apply(&long, &DataModelUpdate::Set(json!(1)))
        .unwrap_err();
    let message = error.to_string();
    assert!(message.len() < budget, "{} bytes", message.len());
    assert!(message.contains("(80000 characters)"), "{message}");
    assert!(
        message.ends_with("path has more than 256 segments"),
        "{message}"
    );

    // Through whole-stream validation, as one of the errors a retry prompt lists.
    let report = Validator::new(&catalog).validate_json_messages(&[
        json!({"version": "v0.9", "createSurface": {"surfaceId": "s", "catalogId": "c"}}),
        json!({"version": "v0.9", "updateDataModel": {"surfaceId": "s", "path": long, "value": 1}}),
    ]);
    let refused = report
        .errors
        .iter()
        .find(|error| error.path == "messages[1]")
        .expect("the update is refused");
    assert!(refused.message.len() < budget, "{}", refused.message.len());

    // Through a binding, in each sentence that quotes one.
    let options = ValidateOptions {
        require_bound_values: true,
        ..ValidateOptions::full_surface()
    };
    let validator = Validator::with_options(&catalog, options);
    let relative = long.trim_start_matches('/');
    for (path, sentence) in [
        (long.clone(), "invalid data-model pointer"),
        (relative.to_string(), "is not inside a list template"),
        (format!("/~bad{long}"), "is not a valid JSON Pointer"),
        (
            format!("/missing{}", "/x".repeat(100)),
            "is missing or undefined",
        ),
    ] {
        let components = [Component::new("root", "Text").with("text", json!({"path": path}))];
        let report = validator.validate_surface(&components, Some(&json!({})));
        let found = report
            .errors
            .iter()
            .find(|error| error.message.contains(sentence))
            .unwrap_or_else(|| panic!("no error says {sentence:?}: {:?}", report.errors));
        assert_eq!(found.code, ErrorCode::UnresolvedBinding);
        assert!(
            found.message.len() < budget,
            "{sentence}: {}",
            found.message.len()
        );
        assert!(found.message.contains("characters)"), "{}", found.message);
    }

    let components = [
        Component::new("root", "List").with(
            "children",
            json!({"componentId": "row", "path": format!("/scalar{}", "/x".repeat(100))}),
        ),
        Component::new("row", "Text").with("text", json!("row")),
    ];
    let mut model = json!({"scalar": {}});
    let mut node = &mut model["scalar"];
    for _ in 0..99 {
        node["x"] = json!({});
        node = &mut node["x"];
    }
    node["x"] = json!("not a list");
    let report = validator.validate_surface(&components, Some(&model));
    let found = report
        .errors
        .iter()
        .find(|error| error.message.contains("must point at an array"))
        .unwrap_or_else(|| panic!("{:?}", report.errors));
    assert!(found.message.len() < budget, "{}", found.message.len());
    assert!(found.message.contains("characters)"), "{}", found.message);
}

/// Raw messages against prior state: the stateful form of
/// `validate_json_messages`, which is that function started from nothing.
#[test]
fn raw_messages_validate_against_prior_state() {
    use ag_ui_a2ui::SurfaceStore;
    let catalog = Catalog::basic();
    let validator = Validator::new(&catalog);
    let first = [
        json!({"version": "v0.9", "createSurface": {"surfaceId": "s", "catalogId": "c"}}),
        json!({"version": "v0.9", "updateComponents": {"surfaceId": "s", "components": [
            {"id": "root", "component": "Column", "children": ["title"]},
            {"id": "title", "component": "Text", "text": "Title"}
        ]}}),
    ];

    // From an empty store it is the whole-stream entry point.
    let (report, state) = validator.validate_json_updates(&SurfaceStore::new(), &first);
    assert_eq!(report, validator.validate_json_messages(&first));
    assert!(report.is_valid(), "{:?}", report.errors);
    assert_eq!(state.get("s").unwrap().components.len(), 2);

    // An update to a surface the batch did not create is checked against the
    // surface the prior state holds, and located in this batch.
    let update = |components: serde_json::Value| json!({"version": "v0.9", "updateComponents": {"surfaceId": "s", "components": components}});
    let alone = validator.validate_json_messages(&[update(json!([
        {"id": "title", "component": "Text", "text": "New title"}
    ]))]);
    assert!(
        !alone.is_valid(),
        "without prior state the surface does not exist"
    );

    let (report, next) = validator.validate_json_updates(
        &state,
        &[update(
            json!([{"id": "title", "component": "Text", "text": "New title"}]),
        )],
    );
    assert!(report.is_valid(), "{:?}", report.errors);
    assert_eq!(
        next.get("s").unwrap().components[1].prop("text"),
        Some(&json!("New title"))
    );

    let (report, candidate) = validator.validate_json_updates(
        &state,
        &[update(
            json!([{"id": "title", "component": "Card", "child": "missing"}]),
        )],
    );
    assert_eq!(
        report
            .errors
            .iter()
            .map(|error| (error.code, error.path.as_str()))
            .collect::<Vec<_>>(),
        vec![(
            ErrorCode::UnresolvedChild,
            "messages[0].updateComponents.components[0].child"
        )]
    );
    // The candidate holds the batch; the prior state is the caller's to keep.
    assert_ne!(candidate, state);
    assert_eq!(state.get("s").unwrap().components[1].component, "Text");

    // An envelope failure stops before anything is folded.
    let (report, untouched) =
        validator.validate_json_updates(&state, &[json!({"updateComponents": {}})]);
    assert!(!report.is_valid());
    assert_eq!(untouched, state);
}
