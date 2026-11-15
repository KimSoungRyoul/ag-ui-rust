//! Versioned data and lifecycle regressions independent of the legacy toolkit suite.
use ag_ui_a2ui::binding::MAX_VALUE_DEPTH;
use ag_ui_a2ui::surface::SurfaceStore;
use ag_ui_a2ui::{
    AgentMessage, AgentPayload, Catalog, Component, DataModel, DataModelUpdate, Error, ErrorCode,
    ModelValue, Validator,
};
use serde_json::{Value, json};

#[test]
fn null_and_omission_round_trip_and_replay_losslessly() {
    let set = AgentMessage::update_data_model("s", "/x", Value::Null);
    let remove = AgentMessage::remove_data_model_value("s", "/x");
    assert_eq!(
        serde_json::to_value(&set).unwrap()["updateDataModel"]["value"],
        Value::Null
    );
    assert!(
        serde_json::to_value(&remove).unwrap()["updateDataModel"]
            .get("value")
            .is_none()
    );
    for op in [set, remove] {
        let back: AgentMessage =
            serde_json::from_value(serde_json::to_value(&op).unwrap()).unwrap();
        assert_eq!(op, back);
    }
    let mut model = DataModel::from(json!({"items":[1,null,3],"x":null}));
    model.apply("/items/0", &DataModelUpdate::Remove).unwrap();
    assert_eq!(model.lookup("/items/0").unwrap(), None);
    assert_eq!(model.lookup("/items/1").unwrap(), Some(&ModelValue::Null));
    assert!(model.to_json().is_err());
    let snapshot = serde_json::to_value(&model).unwrap();
    assert_eq!(
        serde_json::from_value::<DataModel>(snapshot.clone()).unwrap(),
        model
    );
    let mut unsupported = snapshot;
    unsupported["snapshot_version"] = json!(2);
    assert!(serde_json::from_value::<DataModel>(unsupported).is_err());
    model.apply("/", &DataModelUpdate::Remove).unwrap();
    assert_eq!(model.root(), &ModelValue::Undefined);
    model
        .apply("/", &DataModelUpdate::Set(Value::Null))
        .unwrap();
    assert_eq!(model.to_json().unwrap(), Value::Null);
}

#[test]
fn invalid_pointer_or_growth_limit_is_atomic() {
    let mut model = DataModel::from(json!({"items":[0],"null":null}));
    let before = model.clone();
    for path in [
        "/items/0/x",
        "/bad~2/value",
        "/items/999999999",
        "/items/01",
    ] {
        assert!(
            model.apply(path, &DataModelUpdate::Set(json!(1))).is_err(),
            "{path}"
        );
        assert_eq!(model, before);
    }
    model
        .apply("/new/path", &DataModelUpdate::Set(Value::Null))
        .unwrap();
    assert_eq!(model.lookup("/new/path").unwrap(), Some(&ModelValue::Null));
}

/// A path `segments` levels deep, alternating object keys and array indexes so
/// the walk passes through both kinds of container.
fn deep_path(segments: usize) -> String {
    (0..segments)
        .map(|i| if i % 2 == 0 { "/a" } else { "/0" })
        .collect()
}

#[test]
fn a_path_one_segment_past_the_depth_cap_is_rejected_and_one_at_it_applies() {
    // Every segment is a level of nesting in the model, and the path arrives as
    // one flat string, so no JSON parse limit has bounded it.
    let mut model = DataModel::from(json!({"kept": true}));
    let before = model.clone();
    let too_deep = deep_path(MAX_VALUE_DEPTH + 1);
    for update in [DataModelUpdate::Set(json!(1)), DataModelUpdate::Remove] {
        let error = model.apply(&too_deep, &update).unwrap_err();
        assert!(matches!(error, Error::Pointer { .. }), "{error}");
        assert_eq!(model, before);
    }
    let error = model.lookup(&too_deep).unwrap_err();
    assert!(matches!(error, Error::Pointer { .. }), "{error}");

    // The boundary is pinned rather than probed from far away: one segment
    // fewer is a path the model accepts, through objects and arrays alike.
    let deepest = deep_path(MAX_VALUE_DEPTH);
    model
        .apply(&deepest, &DataModelUpdate::Set(json!("bottom")))
        .unwrap();
    assert_eq!(
        model.lookup(&deepest).unwrap(),
        Some(&ModelValue::String("bottom".into()))
    );
    let exported = model.to_json().unwrap();
    assert_eq!(exported.pointer(&deepest), Some(&json!("bottom")));
    assert_eq!(exported["kept"], json!(true));
    model.apply(&deepest, &DataModelUpdate::Remove).unwrap();
    assert_eq!(model.lookup(&deepest).unwrap(), None);
}

#[test]
fn an_over_deep_update_from_the_wire_is_an_ordinary_error() {
    let path = deep_path(MAX_VALUE_DEPTH + 1);

    // Model output reaches the data model through the validator...
    let catalog = Catalog::basic();
    let report = Validator::new(&catalog).validate_json_messages(&[
        json!({"version": "v0.9", "createSurface": {"surfaceId": "s", "catalogId": "c"}}),
        json!({"version": "v0.9", "updateComponents": {"surfaceId": "s", "components": [
            {"id": "root", "component": "Text", "text": "hi"}
        ]}}),
        json!({"version": "v0.9", "updateDataModel": {"surfaceId": "s", "path": path, "value": 1}}),
    ]);
    assert!(
        report
            .errors
            .iter()
            .any(|error| error.code == ErrorCode::InvalidValue && error.path == "messages[2]"),
        "{:?}",
        report.errors
    );

    // ...and replayed history through the surface store, which stays as it was.
    let mut store = SurfaceStore::new();
    store
        .apply(&AgentMessage::create_surface("s", "c"))
        .unwrap();
    let before = store.clone();
    let error = store
        .apply(&AgentMessage::update_data_model("s", path, json!(1)))
        .unwrap_err();
    assert!(matches!(error, Error::Pointer { .. }), "{error}");
    assert_eq!(store, before);
}

#[test]
fn surface_store_handles_replacement_separate_ids_and_lifecycle() {
    let mut store = SurfaceStore::new();
    for id in ["one", "two"] {
        store
            .apply(&AgentMessage::create_surface(id, "catalog"))
            .unwrap();
        store
            .apply(&AgentMessage::update_components(
                id,
                vec![Component::new("root", "Text").with("text", json!(id))],
            ))
            .unwrap();
    }
    let before = store.clone();
    assert!(
        store
            .apply(&AgentMessage::create_surface("one", "catalog"))
            .is_err()
    );
    assert_eq!(store, before);
    assert!(
        store
            .apply(&AgentMessage::update_components(
                "one",
                vec![
                    Component::new("root", "Text"),
                    Component::new("root", "Text")
                ]
            ))
            .is_err()
    );
    assert_eq!(store, before);
    store
        .apply(&AgentMessage::update_components(
            "one",
            vec![Component::new("root", "Text").with("text", json!("updated"))],
        ))
        .unwrap();
    assert_eq!(store.get("one").unwrap().components.len(), 1);
    store.apply(&AgentMessage::delete_surface("one")).unwrap();
    assert!(
        store
            .apply(&AgentMessage::update_data_model("one", "/", json!({})))
            .is_err()
    );
    store
        .apply(&AgentMessage::create_surface("one", "new-catalog"))
        .unwrap();
    assert_eq!(
        store.get("one").unwrap().catalog_id.as_deref(),
        Some("new-catalog")
    );
}

#[test]
fn full_stream_validation_is_surface_scoped_and_updates_need_prior() {
    let catalog = Catalog::basic();
    let validator = Validator::new(&catalog);
    let mut ops = Vec::new();
    for id in ["one", "two"] {
        ops.extend([
            AgentMessage::create_surface(id, "cat"),
            AgentMessage::update_components(
                id,
                vec![Component::new("root", "Text").with("text", json!(id))],
            ),
        ]);
    }
    ops.push(AgentMessage::update_components(
        "one",
        vec![Component::new("root", "Text").with("text", json!("again"))],
    ));
    assert!(validator.validate_messages(&ops).is_valid());
    assert!(!validator.validate_messages(&ops[1..2]).is_valid());
    let mut prior = SurfaceStore::new();
    for op in &ops {
        prior.apply(op).unwrap();
    }
    assert!(
        validator
            .validate_updates(
                &prior,
                &[AgentMessage::update_data_model("one", "/x", Value::Null)]
            )
            .is_ok()
    );
    let forward = vec![
        AgentMessage::create_surface("s", "cat"),
        AgentMessage::update_components(
            "s",
            vec![Component::new("root", "Card").with("child", json!("later"))],
        ),
        AgentMessage::update_components(
            "s",
            vec![Component::new("later", "Text").with("text", json!("arrived"))],
        ),
    ];
    assert!(validator.validate_messages(&forward).is_valid());
    assert!(!validator.validate_messages(&forward[..2]).is_valid());
}

#[test]
fn candidate_rpc_and_multiple_payloads_cannot_be_serialized_as_v09() {
    use ag_ui_a2ui::message::{CallRendererFunction, FunctionCall};
    let rpc = AgentMessage::new(AgentPayload::CallRendererFunction(CallRendererFunction {
        function_call_id: "f".into(),
        call_function: FunctionCall {
            call: "f".into(),
            args: None,
            catalog_id: None,
            return_type: None,
        },
    }));
    assert!(serde_json::to_value(rpc).is_err());
    assert!(serde_json::from_value::<AgentMessage>(json!({"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"c"},"deleteSurface":{"surfaceId":"s"}})).is_err());
}

#[cfg(feature = "ag-ui")]
#[test]
fn history_replay_uses_the_same_lossless_and_fallible_reducer() {
    use ag_ui::Message;
    use ag_ui_a2ui::SurfaceStore;
    use ag_ui_a2ui::adapters::agui::{surfaces_in, wrap_as_operations_envelope};

    let ops = vec![
        AgentMessage::create_surface("s", "c"),
        AgentMessage::update_components(
            "s",
            vec![Component::new("root", "Text").with("text", json!("x"))],
        ),
        AgentMessage::update_data_model("s", "/items", json!([1, null])),
        AgentMessage::remove_data_model_value("s", "/items/0"),
    ];
    let result = |id: &str, ops: &[AgentMessage]| {
        Message::tool(
            format!("m-{id}"),
            format!("call-{id}"),
            wrap_as_operations_envelope(ops).unwrap(),
        )
    };
    let history = vec![result("1", &ops)];
    let surfaces = surfaces_in(&history).unwrap();
    let prior = surfaces.get("s").unwrap();
    assert!(prior.data_model.to_json().is_err());

    // What rebuilds the surface elsewhere keeps the removed slot removed.
    let mut rebuilt = SurfaceStore::new();
    for message in prior.replay() {
        rebuilt.apply(&message).unwrap();
    }
    assert_eq!(rebuilt.get("s"), Some(prior));

    let mut broken = history;
    broken.push(result(
        "2",
        &[AgentMessage::update_data_model("s", "/items/bad", json!(1))],
    ));
    assert!(surfaces_in(&broken).is_err());
    let invalid_wire = [Message::tool(
        "m-1",
        "call-1",
        json!({"a2ui_operations": [
            {"version":"v0.9.1","updateDataModel":{"surfaceId":"s","path":23}}
        ]})
        .to_string(),
    )];
    assert!(surfaces_in(&invalid_wire).is_err());
}

#[test]
fn upserts_match_reference_renderer_container_and_sparse_array_behavior() {
    let mut model = DataModel::from(json!({"nullable":null,"items":[]}));
    for (path, value) in [
        ("/list/0", json!("first")),
        ("/nullable/name", json!("nested")),
        ("/items/2", json!("third")),
        ("/matrix/0/1", json!(3)),
    ] {
        model.apply(path, &DataModelUpdate::Set(value)).unwrap();
    }
    assert_eq!(
        model.lookup("/list").unwrap().unwrap().to_json().unwrap(),
        json!(["first"])
    );
    assert_eq!(
        model
            .lookup("/nullable")
            .unwrap()
            .unwrap()
            .to_json()
            .unwrap(),
        json!({"name":"nested"})
    );
    let Some(ModelValue::Array(items)) = model.lookup("/items").unwrap() else {
        panic!("expected array")
    };
    assert_eq!(items.len(), 3);
    assert_eq!(&items[..2], &[ModelValue::Undefined, ModelValue::Undefined]);
    assert_eq!(
        model
            .lookup("/matrix/0/1")
            .unwrap()
            .unwrap()
            .to_json()
            .unwrap(),
        json!(3)
    );
    model.apply("/items/4", &DataModelUpdate::Remove).unwrap();
    let Some(ModelValue::Array(items)) = model.lookup("/items").unwrap() else {
        panic!("expected array")
    };
    assert_eq!(items.len(), 5);
    let before = model.clone();
    assert!(
        model
            .apply_with_array_growth_limit("/new/2/4", &DataModelUpdate::Set(json!(1)), 7)
            .is_err()
    );
    assert_eq!(model, before);
    model
        .apply("/", &DataModelUpdate::Set(Value::Null))
        .unwrap();
    model
        .apply("/0", &DataModelUpdate::Set(json!("object key")))
        .unwrap();
    assert_eq!(model.to_json().unwrap(), json!({"0":"object key"}));
}
