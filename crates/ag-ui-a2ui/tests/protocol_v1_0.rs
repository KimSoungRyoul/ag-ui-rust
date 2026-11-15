//! A2UI v1.0 through the protocol modules, with no feature on: a surface
//! that draws from two catalogs, the keys v1.0 reserves, how a value is
//! removed, where a component may sit, and what a v0.9 surface goes on doing
//! beside it.
//!
//! The conformance suite holds the same rules to upstream's cases
//! (`tests/conformance.rs`, behind `schema-validation`). These are the rules
//! as a caller of this crate meets them.

use ag_ui_a2ui::binding::{Scope, collect_bindings_in};
use ag_ui_a2ui::processor::{MessageProcessor, RejectionKind};
use ag_ui_a2ui::{
    A2uiVersion, AgentMessage, Catalog, Component, ErrorCode, SurfaceStore, ValidateOptions,
    Validator,
};
use serde_json::{Value, json};

const CHARTS: &str = "example.com:charts";

/// A second catalog: one component, a function a renderer runs and one only
/// the agent may call.
fn charts() -> Catalog {
    Catalog::from_schema(&json!({
        "catalogId": CHARTS,
        "protocolVersion": "1.0",
        "components": {
            "LineChart": {
                "type": "object",
                "properties": {
                    "component": {"const": "LineChart"},
                    "series": {"$ref": "common_types.json#/$defs/DynamicValue"},
                    "caption": {"$ref": "common_types.json#/$defs/DynamicString"}
                },
                "required": ["component", "series"]
            }
        },
        "functions": {
            "rollingMean": {"returnType": "number"},
            "fetchQuote": {"returnType": "number", "allowedCallers": "agentOnly"}
        }
    }))
    .unwrap()
}

/// A processor that holds every message to the whole contract, with the v1.0
/// basic catalog and [`charts`].
fn processor() -> (MessageProcessor, String) {
    let basic = Catalog::basic_for(A2uiVersion::V1_0);
    let id = basic.catalog_id.clone();
    let mut processor = MessageProcessor::with_options(ValidateOptions {
        check_bindings: false,
        ..ValidateOptions::full_surface()
    });
    processor.register(basic).unwrap();
    processor.register(charts()).unwrap();
    (processor, id)
}

fn dashboard(basic: &str) -> Value {
    json!({
        "version": "1.0",
        "createSurface": {
            "surfaceId": "dash",
            "catalogId": basic,
            "dataModel": {"title": "Sales", "points": [1, 2, 3]},
            "components": [
                {"id": "root", "component": "Column", "children": ["title", "chart"]},
                {"id": "title", "component": "Text", "text": {"@path": "/title"}},
                {
                    "id": "chart",
                    "component": "LineChart",
                    "@catalogId": CHARTS,
                    "series": {"@path": "/points"},
                    "caption": {"@call": "formatString", "args": {"value": "${/title}: ${count}"}}
                }
            ]
        }
    })
}

fn update(surface: &str, components: Value) -> Value {
    json!({"version": "1.0", "updateComponents": {"surfaceId": surface, "components": components}})
}

#[test]
fn a_surface_is_created_whole_and_draws_from_two_catalogs() {
    let (mut processor, basic) = processor();
    processor.process(&dashboard(&basic)).unwrap();

    let surface = processor.surfaces().get("dash").unwrap();
    assert_eq!(surface.spoken_version(), A2uiVersion::V1_0);
    assert_eq!(surface.catalog_id.as_deref(), Some(basic.as_str()));
    assert_eq!(
        surface.data_model.to_json().unwrap(),
        json!({"title": "Sales", "points": [1, 2, 3]})
    );
    let read_from: Vec<_> = surface
        .components
        .iter()
        .map(|component| (component.id.as_str(), surface.catalog_of(component)))
        .collect();
    assert_eq!(
        read_from,
        [
            ("root", Some(basic.as_str())),
            ("title", Some(basic.as_str())),
            ("chart", Some(CHARTS)),
        ]
    );
    assert_eq!(
        surface.components[2].catalog_id(A2uiVersion::V1_0),
        Some(CHARTS)
    );
    assert!(processor.report("dash").unwrap().is_valid());

    // An outline resolves a binding as the surface writes one.
    let data = surface.data_model.to_json().unwrap();
    let basic_catalog = Catalog::basic_for(A2uiVersion::V1_0);
    let drawn: Vec<_> = surface
        .outline(&basic_catalog, &data)
        .map(|placed| (placed.depth, placed.component.id.as_str(), placed.scope))
        .collect();
    assert_eq!(
        drawn
            .iter()
            .map(|(depth, id, _)| (*depth, *id))
            .collect::<Vec<_>>(),
        [(0, "root"), (1, "title"), (1, "chart")]
    );
    let title = surface.components[1].prop("text").unwrap();
    assert_eq!(drawn[1].2.version(), A2uiVersion::V1_0);
    assert_eq!(drawn[1].2.resolve_dynamic(title).unwrap(), json!("Sales"));

    // The store replays the surface in its own version: the creation, then
    // its components and its data.
    let replayed = serde_json::to_value(surface.replay()).unwrap();
    assert_eq!(replayed.as_array().unwrap().len(), 3);
    assert_eq!(replayed[0]["version"], "1.0");
    assert_eq!(replayed[0]["createSurface"]["catalogId"], basic.as_str());
    let mut rebuilt = SurfaceStore::new();
    for message in surface.replay() {
        rebuilt.apply(&message).unwrap();
    }
    let rebuilt = rebuilt.get("dash").unwrap();
    assert_eq!(rebuilt.components, surface.components);
    assert_eq!(rebuilt.data_model, surface.data_model);
}

#[test]
fn a_surface_with_no_default_catalog_has_each_component_name_its_own() {
    let (mut processor, basic) = processor();
    processor
        .process(&json!({
            "version": "1.0",
            "createSurface": {
                "surfaceId": "s",
                "components": [
                    {"id": "root", "component": "Text", "@catalogId": basic, "text": "Hi"}
                ]
            }
        }))
        .unwrap();
    assert_eq!(processor.surfaces().get("s").unwrap().catalog_id, None);

    let refused = processor
        .process(&update(
            "s",
            json!([{"id": "root", "component": "Text", "text": "Hi"}]),
        ))
        .unwrap_err();
    assert_eq!(refused.kind, RejectionKind::Catalog, "{refused}");
    assert_eq!(refused.errors().len(), 0);
    assert!(
        refused.to_string().contains("no default catalogId"),
        "{refused}"
    );
}

#[test]
fn a_catalog_that_is_not_held_or_is_written_for_another_version_is_refused() {
    let (mut processor, basic) = processor();
    processor
        .register(
            Catalog::from_schema(&json!({
                "catalogId": "example.com:old",
                "protocolVersion": "v0.9",
                "components": {"Badge": {"type": "object"}}
            }))
            .unwrap(),
        )
        .unwrap();
    processor.process(&dashboard(&basic)).unwrap();

    for (named, says) in [
        ("example.com:missing", "example.com:missing"),
        ("example.com:old", "Protocol version mismatch"),
    ] {
        let refused = processor
            .process(&update(
                "dash",
                json!([{"id": "title", "component": "Badge", "@catalogId": named}]),
            ))
            .unwrap_err();
        assert_eq!(refused.kind, RejectionKind::Catalog, "{named}: {refused}");
        assert!(refused.to_string().contains(says), "{named}: {refused}");
    }
    // A refusal changes nothing.
    let surface = processor.surfaces().get("dash").unwrap();
    assert_eq!(surface.components[1].component, "Text");

    // A surface is created in the format of its catalog, too.
    let refused = processor
        .process(&json!({
            "version": "1.0",
            "createSurface": {"surfaceId": "other", "catalogId": "example.com:old"}
        }))
        .unwrap_err();
    assert_eq!(refused.kind, RejectionKind::Catalog, "{refused}");
}

#[test]
fn a_function_call_is_read_from_the_catalog_it_names_then_from_the_surface() {
    let (mut processor, basic) = processor();
    processor.process(&dashboard(&basic)).unwrap();
    let caption = |call: Value| {
        update(
            "dash",
            json!([{
                "id": "chart",
                "component": "LineChart",
                "@catalogId": CHARTS,
                "series": {"@path": "/points"},
                "caption": call
            }]),
        )
    };

    // `formatString` is the basic catalog's, which is the surface's own; the
    // chart's catalog has no say in it. `rollingMean` has to be named.
    for accepted in [
        json!({"@call": "formatString", "args": {"value": "Mean"}}),
        json!({"@call": "rollingMean", "catalogId": CHARTS, "args": {"window": 3}}),
    ] {
        processor.process(&caption(accepted)).unwrap();
    }
    for (refused, says) in [
        (
            json!({"@call": "rollingMean", "args": {}}),
            "Unrecognized function 'rollingMean'",
        ),
        (
            json!({"@call": "formatString", "catalogId": CHARTS, "args": {"value": "x"}}),
            "Unrecognized function 'formatString'",
        ),
        (
            json!({"@call": "fetchQuote", "catalogId": CHARTS, "args": {}}),
            "agentOnly",
        ),
        (
            json!({"@call": "roll-mean", "catalogId": CHARTS, "args": {}}),
            "valid UAX #31 identifier",
        ),
    ] {
        let rejection = processor.process(&caption(refused.clone())).unwrap_err();
        let said = rejection.to_string();
        assert!(said.contains(says), "{refused}: {said}");
    }
}

#[test]
fn the_index_of_an_item_is_asked_only_where_a_template_draws_it() {
    let (mut processor, basic) = processor();
    processor
        .process(&json!({
            "version": "1.0",
            "createSurface": {
                "surfaceId": "list",
                "catalogId": basic,
                "dataModel": {"rows": [{"name": "a"}, {"name": "b"}]},
                "components": [
                    {"id": "root", "component": "List", "children": {"componentId": "row", "path": "/rows"}},
                    {"id": "row", "component": "Text", "text": {"@call": "@index", "args": {"offset": 1}}}
                ]
            }
        }))
        .unwrap();

    let refused = processor
        .process(&update(
            "list",
            json!([{"id": "root", "component": "Text", "text": {"@call": "@index"}}]),
        ))
        .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("collection template iteration scope"),
        "{refused}"
    );
    // No catalog declares a function whose name starts with `@`.
    let refused = processor
        .process(&update(
            "list",
            json!([{"id": "row", "component": "Text", "text": {"@call": "@count"}}]),
        ))
        .unwrap_err();
    assert!(
        refused.to_string().contains("is not a system function"),
        "{refused}"
    );
}

#[test]
fn a_key_that_starts_with_one_at_sign_is_the_protocols() {
    let (mut processor, basic) = processor();
    processor.process(&dashboard(&basic)).unwrap();
    let series = |value: Value| {
        update(
            "dash",
            json!([{
                "id": "chart",
                "component": "LineChart",
                "@catalogId": CHARTS,
                "series": value
            }]),
        )
    };

    for (key, at) in [
        ("@if", "components[0].series.@if"),
        ("@", "components[0].series.@"),
        ("@metadata", "components[0].series.@metadata"),
    ] {
        let refused = processor
            .process(&series(json!({key: true, "then": "x"})))
            .unwrap_err();
        let [error] = refused.errors() else {
            panic!("{key}: one error, got {:?}", refused.errors());
        };
        assert_eq!(error.code, ErrorCode::ReservedKey, "{key}");
        assert!(error.path.ends_with(at), "{key}: {}", error.path);
    }
    // The same key on the component itself, where only the envelope's are.
    let refused = processor
        .process(&update(
            "dash",
            json!([{"id": "title", "component": "Text", "text": "x", "@style": "bold"}]),
        ))
        .unwrap_err();
    assert_eq!(refused.errors()[0].code, ErrorCode::ReservedKey);

    // A literal key is written doubled, and read back single.
    let literal = json!({"@@type": "Series", "values": {"@path": "/points"}});
    processor.process(&series(literal.clone())).unwrap();
    let data = json!({"points": [1, 2, 3]});
    let scope = Scope::root(&data).speaking(A2uiVersion::V1_0);
    assert_eq!(
        scope.resolve_dynamic(&literal).unwrap(),
        json!({"@type": "Series", "values": [1, 2, 3]})
    );
    // In v0.9 the same object is data as it stands: `@` means nothing there,
    // and `@path` is not how that version writes a binding.
    assert_eq!(
        Scope::root(&data).resolve_dynamic(&literal).unwrap(),
        literal
    );
    assert_eq!(
        collect_bindings_in(A2uiVersion::V1_0, &literal).len(),
        1,
        "the binding under `values`"
    );
    assert!(collect_bindings_in(A2uiVersion::V0_9, &literal).is_empty());
}

#[test]
fn the_envelope_of_a_component_is_checked_as_the_version_has_it() {
    let (mut processor, basic) = processor();
    processor.process(&dashboard(&basic)).unwrap();
    let title = |extra: Value| {
        let mut component = json!({"id": "title", "component": "Text", "text": "x"});
        component
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        update("dash", json!([component]))
    };

    processor
        .process(&title(json!({
            "@accessibility": {"label": "Title"},
            "@metadata": {"extensions": {"com_example_trace": "t1"}}
        })))
        .unwrap();
    let surface = processor.surfaces().get("dash").unwrap();
    let stored = &surface.components[1];
    assert_eq!(
        stored.metadata(A2uiVersion::V1_0),
        Some(&json!({"extensions": {"com_example_trace": "t1"}}))
    );
    assert_eq!(
        stored.accessibility(A2uiVersion::V1_0),
        Some(&json!({"label": "Title"}))
    );

    for (extra, at) in [
        // `Text` takes no check rules.
        (
            json!({"@checks": [{"condition": true, "message": "no"}]}),
            ".@checks",
        ),
        // An extension is named by an identifier.
        (
            json!({"@metadata": {"extensions": {"com.example.trace": 1}}}),
            ".@metadata.extensions.com.example.trace",
        ),
        // The same key twice, with the `@` and as the drafts wrote it.
        (
            json!({"@accessibility": {"label": "a"}, "accessibility": {"label": "b"}}),
            ".accessibility",
        ),
        // A catalog is named by a string.
        (json!({"@catalogId": 7}), ".@catalogId"),
    ] {
        let refused = processor.process(&title(extra.clone())).unwrap_err();
        let located: Vec<_> = refused.errors().iter().map(|e| e.path.as_str()).collect();
        assert!(
            located.iter().any(|path| path.ends_with(at)),
            "{extra}: {located:?}"
        );
    }
}

#[test]
fn a_value_is_removed_with_null_and_a_removal_of_nothing_does_nothing() {
    let (mut processor, basic) = processor();
    processor.process(&dashboard(&basic)).unwrap();
    let data = |path: &str, value: Value| json!({"version": "1.0", "updateDataModel": {"surfaceId": "dash", "path": path, "value": value}});
    let model = |processor: &MessageProcessor| {
        let surface = processor.surfaces().get("dash").unwrap();
        surface.data_model.to_json().unwrap()
    };

    processor.process(&data("/title", Value::Null)).unwrap();
    assert_eq!(model(&processor), json!({"points": [1, 2, 3]}));
    // Nothing is there to remove, and nothing is made on the way to it.
    for absent in ["/gone/deep/path", "/points/10"] {
        processor.process(&data(absent, Value::Null)).unwrap();
        assert_eq!(model(&processor), json!({"points": [1, 2, 3]}), "{absent}");
    }
    processor.process(&data("/", Value::Null)).unwrap();
    assert_eq!(model(&processor), json!({}));

    // `value` is required: leaving it out is how v0.9 removes.
    let refused = processor
        .process(&json!({"version": "1.0", "updateDataModel": {"surfaceId": "dash", "path": "/x"}}))
        .unwrap_err();
    let [error] = refused.errors() else {
        panic!("one error: {:?}", refused.errors());
    };
    assert_eq!(
        (error.code, error.path.as_str()),
        (ErrorCode::MissingField, "messages[0].updateDataModel.value")
    );

    // A typed removal is written as its version writes one.
    let removal = |version| {
        serde_json::to_value(
            AgentMessage::remove_data_model_value("dash", "/x").with_version(version),
        )
        .unwrap()
    };
    assert_eq!(
        removal(A2uiVersion::V1_0)["updateDataModel"],
        json!({"surfaceId": "dash", "path": "/x", "value": null})
    );
    assert_eq!(
        removal(A2uiVersion::V0_9)["updateDataModel"],
        json!({"surfaceId": "dash", "path": "/x"})
    );
}

/// A catalog whose `Header` sits only in a `Box`, and whose `Box` holds only
/// `Header`s and `Text`s, written for `version`.
fn layout(version: &str) -> Catalog {
    Catalog::from_schema(&json!({
        "catalogId": "example.com:layout",
        "protocolVersion": version,
        "components": {
            "Box": {
                "type": "object",
                "allowedChildren": ["Header", "Text"],
                "properties": {"children": {"$ref": "common_types.json#/$defs/ChildList"}}
            },
            "Header": {"type": "object", "allowedParents": ["Box"]},
            "Text": {"type": "object"},
            "Video": {"type": "object"}
        }
    }))
    .unwrap()
}

#[test]
fn a_component_sits_where_its_catalog_lets_it() {
    let mut processor = MessageProcessor::with_options(ValidateOptions::full_surface());
    processor.register(layout("1.0")).unwrap();
    let create = |components: Value| {
        json!({
            "version": "1.0",
            "createSurface": {
                "surfaceId": "s", "catalogId": "example.com:layout", "components": components
            }
        })
    };

    for (components, code, wire, at, says) in [
        (
            json!([{"id": "root", "component": "Header"}]),
            ErrorCode::UnallowedParent,
            "UNALLOWED_PARENT",
            "messages[0].createSurface.components[0]",
            "Component 'root' (Header) cannot be placed under parent 'Surface' (Surface). \
             Allowed parents: ['Box'].",
        ),
        (
            json!([
                {"id": "root", "component": "Box", "children": ["clip"]},
                {"id": "clip", "component": "Video"}
            ]),
            ErrorCode::UnallowedChild,
            "UNALLOWED_CHILD",
            "messages[0].createSurface.components[0].children[0]",
            "Container 'root' (Box) cannot contain child 'clip' (Video). Allowed children: \
             ['Header', 'Text'].",
        ),
    ] {
        let refused = processor.process(&create(components)).unwrap_err();
        assert_eq!(refused.kind, RejectionKind::Validation);
        let [error] = refused.errors() else {
            panic!("one error: {:?}", refused.errors());
        };
        assert_eq!((error.code, error.path.as_str()), (code, at));
        assert_eq!(error.message, says);
        let reply = serde_json::to_value(refused.renderer_error()).unwrap();
        assert_eq!(reply["error"]["code"], wire, "{reply}");
    }
    processor
        .process(&create(json!([
            {"id": "root", "component": "Box", "children": ["head"]},
            {"id": "head", "component": "Header"}
        ])))
        .unwrap();

    // A validator says the same of the same messages.
    let catalog = layout("1.0");
    let (report, _) = Validator::new(&catalog).validate_json_updates(
        &SurfaceStore::new(),
        &[create(json!([{"id": "root", "component": "Header"}]))],
    );
    assert_eq!(
        report.errors.iter().map(|e| e.code).collect::<Vec<_>>(),
        [ErrorCode::UnallowedParent]
    );
}

/// `allowedParents` and `allowedChildren` are part of the v1.0 catalog
/// definition. A v0.9 surface is not refused for them; the catalog still
/// answers a caller that asks.
#[test]
fn composition_is_not_a_reason_to_refuse_a_v0_9_surface() {
    let catalog = layout("v0.9");
    let mut processor = MessageProcessor::with_options(ValidateOptions::full_surface());
    processor.register(catalog.clone()).unwrap();
    let components = vec![
        Component::new("root", "Box").with("children", json!(["clip"])),
        Component::new("clip", "Video"),
    ];
    processor
        .process_all(&[
            json!({"version": "v0.9", "createSurface": {"surfaceId": "s", "catalogId": "example.com:layout"}}),
            json!({"version": "v0.9", "updateComponents": {"surfaceId": "s", "components": components}}),
        ])
        .unwrap();
    let violations = catalog.composition_violations(&components);
    assert_eq!(violations.len(), 1, "{violations:?}");
}

#[test]
fn a_v0_9_surface_is_read_as_before_beside_a_v1_0_one() {
    let (mut processor, basic) = processor();
    let v0_9 = Catalog::basic();
    let v0_9_id = v0_9.catalog_id.clone();
    processor.register(v0_9).unwrap();
    processor.process(&dashboard(&basic)).unwrap();
    processor
        .process_all(&[
            json!({"version": "v0.9", "createSurface": {"surfaceId": "old", "catalogId": v0_9_id}}),
            json!({"version": "v0.9", "updateComponents": {"surfaceId": "old", "components": [
                {"id": "root", "component": "Text", "text": {"path": "/title"}}
            ]}}),
            json!({"version": "v0.9", "updateDataModel": {"surfaceId": "old", "path": "/title", "value": null}}),
        ])
        .unwrap();
    let old = processor.surfaces().get("old").unwrap();
    assert_eq!(old.spoken_version(), A2uiVersion::V0_9);
    // `null` is a value in v0.9, and stays.
    assert_eq!(old.data_model.to_json().unwrap(), json!({"title": null}));

    // Each is refused, and for the reason it gives.
    for (message, why) in [
        // A message of the other family, for a surface that speaks this one.
        (
            json!({"version": "1.0", "updateDataModel": {"surfaceId": "old", "path": "/x", "value": 1}}),
            "was created as v0.9; a 1.0 message cannot update it",
        ),
        (
            json!({"version": "v0.9", "updateDataModel": {"surfaceId": "dash", "path": "/x", "value": 1}}),
            "was created as 1.0; a v0.9 message cannot update it",
        ),
        // What only v1.0 has, on a v0.9 surface.
        (
            json!({"version": "v0.9", "updateComponents": {"surfaceId": "old", "components": [
                {"id": "root", "component": "Text", "text": "x", "@catalogId": CHARTS}
            ]}}),
            "'@catalogId' is a component key of a later A2UI version",
        ),
        (
            json!({"version": "v0.9", "createSurface": {"surfaceId": "new"}}),
            "'createSurface' requires the field 'catalogId'",
        ),
        (
            json!({"version": "v0.9", "createSurface": {
                "surfaceId": "new", "catalogId": v0_9_id, "components": [{"id": "root", "component": "Text", "text": "x"}]
            }}),
            "'components' is not a field of 'createSurface' in A2UI v0.9",
        ),
        // And the other way: a theme is not a field of a v1.0 creation.
        (
            json!({"version": "1.0", "createSurface": {"surfaceId": "new", "catalogId": basic, "theme": {}}}),
            "'theme' is not a field of 'createSurface' in A2UI 1.0",
        ),
        // A v1.0 surface is not drawn from a v0.9 catalog.
        (
            json!({"version": "1.0", "createSurface": {"surfaceId": "new", "catalogId": v0_9_id}}),
            "Protocol version mismatch",
        ),
    ] {
        let refused = processor.process(&message).unwrap_err().to_string();
        assert!(refused.contains(why), "{message}: {refused}");
    }
    assert!(processor.surfaces().get("new").is_none());

    // The two surfaces are reported to a client each in its own version.
    let store = processor.surfaces();
    let sent = |version| {
        store
            .client_data_model(version)
            .unwrap()
            .map(|model| serde_json::to_value(model).unwrap())
    };
    assert_eq!(
        sent(A2uiVersion::V0_9),
        None,
        "`old` did not ask to be sent"
    );
    assert_eq!(sent(A2uiVersion::V1_0), None, "nor did `dash`");
}

#[test]
fn a_validator_over_several_catalogs_reads_typed_components_too() {
    let basic = Catalog::basic_for(A2uiVersion::V1_0);
    let charts = charts();
    let validator = Validator::over([&basic, &charts], ValidateOptions::full_surface())
        .speaking(A2uiVersion::V1_0);
    let surface = [
        Component::new("root", "Column").with("children", json!(["chart"])),
        Component::new("chart", "LineChart")
            .with("@catalogId", json!(CHARTS))
            .with("series", json!([1, 2])),
    ];
    // Several catalogs and no surface: nothing says which is the default.
    let report = validator.validate(&surface);
    assert_eq!(
        report.errors.iter().map(|e| e.code).collect::<Vec<_>>(),
        [ErrorCode::UnknownCatalog],
        "{:?}",
        report.errors
    );

    // On a surface, the surface says.
    let mut store = SurfaceStore::new();
    let create = ag_ui_a2ui::message::CreateSurface::new("s")
        .with_catalog_id(&basic.catalog_id)
        .with_components(surface.to_vec());
    let message = AgentMessage::new(ag_ui_a2ui::AgentPayload::CreateSurface(create))
        .with_version(A2uiVersion::V1_0);
    let next = validator.validate_updates(&store, &[message]).unwrap();
    store = next;
    assert_eq!(store.get("s").unwrap().components.len(), 2);
}
