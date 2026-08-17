//! JSON Patch operations must match RFC 6902 exactly.

use ag_ui_core::*;
use serde_json::json;

fn every_operation() -> Vec<(PatchOperation, serde_json::Value)> {
    vec![
        (
            PatchOperation::add("/items/-", json!("new")),
            json!({ "op": "add", "path": "/items/-", "value": "new" }),
        ),
        (
            PatchOperation::remove("/items/0"),
            json!({ "op": "remove", "path": "/items/0" }),
        ),
        (
            PatchOperation::replace("/counter", json!(2)),
            json!({ "op": "replace", "path": "/counter", "value": 2 }),
        ),
        (
            PatchOperation::mv("/a", "/b"),
            json!({ "op": "move", "from": "/a", "path": "/b" }),
        ),
        (
            PatchOperation::copy("/a", "/b"),
            json!({ "op": "copy", "from": "/a", "path": "/b" }),
        ),
        (
            PatchOperation::test("/counter", json!(1)),
            json!({ "op": "test", "path": "/counter", "value": 1 }),
        ),
    ]
}

#[test]
fn operations_match_the_rfc_6902_wire_format() {
    for (operation, expected) in every_operation() {
        assert_eq!(serde_json::to_value(&operation).unwrap(), expected);
        assert_eq!(
            serde_json::from_value::<PatchOperation>(expected).unwrap(),
            operation
        );
    }
}

#[test]
fn operations_round_trip() {
    for (operation, _) in every_operation() {
        let text = serde_json::to_string(&operation).unwrap();
        let back: PatchOperation = serde_json::from_str(&text).unwrap();
        assert_eq!(back, operation);
        assert_eq!(serde_json::to_string(&back).unwrap(), text);
    }
}

#[test]
fn accessors_report_the_shape_of_each_operation() {
    let add = PatchOperation::add("/a", json!(1));
    assert_eq!(add.op(), "add");
    assert_eq!(add.path(), "/a");
    assert_eq!(add.from(), None);
    assert_eq!(add.value(), Some(&json!(1)));

    let moved = PatchOperation::mv("/a", "/b");
    assert_eq!(moved.op(), "move");
    assert_eq!(moved.path(), "/b");
    assert_eq!(moved.from(), Some("/a"));
    assert_eq!(moved.value(), None);
}

#[test]
fn a_null_value_is_a_value_not_an_omission() {
    let operation = PatchOperation::add("/nullable", serde_json::Value::Null);
    assert_eq!(
        serde_json::to_string(&operation).unwrap(),
        r#"{"op":"add","path":"/nullable","value":null}"#
    );
    assert_eq!(
        serde_json::from_str::<PatchOperation>(r#"{"op":"add","path":"/nullable","value":null}"#)
            .unwrap(),
        operation
    );
}

#[test]
fn a_patch_document_round_trips_inside_a_state_delta() {
    let patch: JsonPatch = every_operation()
        .into_iter()
        .map(|(operation, _)| operation)
        .collect();

    let event = Event::state_delta(patch.clone());
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "STATE_DELTA");
    assert_eq!(json["delta"].as_array().unwrap().len(), 6);

    let back: Event = serde_json::from_value(json).unwrap();
    assert_eq!(back, event);

    let Event::StateDelta(payload) = back else {
        panic!("wrong variant");
    };
    assert_eq!(payload.delta, patch);
}

#[test]
fn a_patch_document_round_trips_inside_an_activity_delta() {
    let event = Event::activity_delta(
        "msg-1",
        "web_search",
        vec![PatchOperation::replace("/status", json!("done"))],
    );

    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["patch"][0]["op"], "replace");
    assert_eq!(serde_json::from_value::<Event>(json).unwrap(), event);
}

#[test]
fn an_unknown_operation_is_rejected() {
    assert!(serde_json::from_str::<PatchOperation>(r#"{"op":"increment","path":"/a"}"#).is_err());
    // `add` without a value is not a valid operation either.
    assert!(serde_json::from_str::<PatchOperation>(r#"{"op":"add","path":"/a"}"#).is_err());
}
