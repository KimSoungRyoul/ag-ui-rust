//! State: snapshots, patches, and what happens when a patch does not apply.

use ag_ui_client::apply::{Applier, Changed};
use ag_ui_client::transport::ReplayTransport;
use ag_ui_client::{Error, Session, Update};
use ag_ui_core::{Event, PatchOperation};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;

#[test]
fn a_snapshot_replaces_the_state_and_a_delta_patches_it() {
    let mut applier = Applier::new();
    assert_eq!(applier.state(), &json!({}));

    let changed = applier
        .apply(&Event::state_snapshot(
            json!({ "items": ["a"], "count": 1 }),
        ))
        .expect("applies");
    assert_eq!(changed, Changed::State);

    applier
        .apply(&Event::state_delta(vec![
            PatchOperation::add("/items/-", json!("b")),
            PatchOperation::replace("/count", json!(2)),
            PatchOperation::add("/nested", json!({ "ok": true })),
        ]))
        .expect("applies");

    assert_eq!(
        applier.state(),
        &json!({ "items": ["a", "b"], "count": 2, "nested": { "ok": true } })
    );

    // A snapshot after a delta replaces everything, including keys the delta
    // added.
    applier
        .apply(&Event::state_snapshot(json!({ "count": 9 })))
        .expect("applies");
    assert_eq!(applier.state(), &json!({ "count": 9 }));
}

#[test]
fn a_patch_that_cannot_apply_is_an_error_and_changes_nothing() {
    let mut applier = Applier::new().with_state(json!({ "count": 1 }));

    let error = applier
        .apply(&Event::state_delta(vec![PatchOperation::replace(
            "/missing/deeply",
            json!(2),
        )]))
        .expect_err("replacing a path that does not exist must fail");

    assert!(
        matches!(&error, Error::Patch { target, .. } if target == "state"),
        "unexpected error: {error:?}"
    );
    assert!(error.to_string().contains("state patch failed"));
    // The state is exactly what it was: no half-applied patch.
    assert_eq!(applier.state(), &json!({ "count": 1 }));
}

#[test]
fn a_failed_operation_rolls_back_the_ones_before_it() {
    // RFC 6902 patches are all-or-nothing. The first operation here succeeds
    // and must still be undone when the second fails.
    let mut applier = Applier::new().with_state(json!({ "count": 1 }));

    applier
        .apply(&Event::state_delta(vec![
            PatchOperation::add("/added", json!(true)),
            PatchOperation::test("/count", json!(99)),
        ]))
        .expect_err("the test operation should fail");

    assert_eq!(applier.state(), &json!({ "count": 1 }));
}

#[test]
fn a_malformed_json_pointer_is_rejected_before_anything_is_mutated() {
    let mut applier = Applier::new().with_state(json!({ "count": 1 }));

    let error = applier
        .apply(&Event::state_delta(vec![PatchOperation::add(
            "not-a-pointer",
            json!(1),
        )]))
        .expect_err("a pointer must start with a slash");

    assert!(
        error.to_string().contains("invalid patch document"),
        "unexpected error: {error}"
    );
    assert_eq!(applier.state(), &json!({ "count": 1 }));
}

#[test]
fn an_activity_patch_that_cannot_apply_is_an_error_naming_the_activity() {
    let mut applier = Applier::new();
    let mut content = ag_ui_core::JsonObject::new();
    content.insert("status".into(), json!("running"));
    applier
        .apply(&Event::activity_snapshot("act-1", "web_search", content))
        .expect("applies");

    let error = applier
        .apply(&Event::activity_delta(
            "act-1",
            "web_search",
            vec![PatchOperation::replace("/nope", json!(1))],
        ))
        .expect_err("replacing a missing key must fail");

    assert!(
        matches!(&error, Error::Patch { target, .. } if target == "activity act-1"),
        "unexpected error: {error:?}"
    );

    // And the activity kept the content it had.
    let ag_ui_core::Message::Activity(activity) = &applier.messages()[0] else {
        panic!("expected an activity message");
    };
    assert_eq!(activity.content["status"], json!("running"));
}

#[test]
fn an_activity_snapshot_can_merge_instead_of_replacing() {
    let mut applier = Applier::new();
    let mut first = ag_ui_core::JsonObject::new();
    first.insert("query".into(), json!("weather"));
    first.insert("hits".into(), json!(0));
    applier
        .apply(&Event::activity_snapshot("act-1", "web_search", first))
        .expect("applies");

    let mut second = ag_ui_core::JsonObject::new();
    second.insert("hits".into(), json!(3));
    applier
        .apply(&Event::ActivitySnapshot(
            ag_ui_core::ActivitySnapshotEvent {
                replace: false,
                ..ag_ui_core::ActivitySnapshotEvent::new("act-1", "web_search", second)
            },
        ))
        .expect("applies");

    let ag_ui_core::Message::Activity(activity) = &applier.messages()[0] else {
        panic!("expected an activity message");
    };
    assert_eq!(activity.content["query"], json!("weather"));
    assert_eq!(activity.content["hits"], json!(3));
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
struct Counter {
    count: u32,
}

#[test]
fn the_state_can_be_read_as_a_caller_defined_type() {
    let mut applier = Applier::new();
    applier
        .apply(&Event::state_snapshot(json!({ "count": 3 })))
        .expect("applies");

    assert_eq!(
        applier.state_as::<Counter>().expect("deserializes"),
        Counter { count: 3 }
    );

    applier
        .apply(&Event::state_snapshot(json!({ "count": "three" })))
        .expect("applies");
    let error = applier
        .state_as::<Counter>()
        .expect_err("a string is not a u32");
    assert!(matches!(error, Error::State(_)), "unexpected: {error:?}");
    // The raw state is still exactly what the agent sent.
    assert_eq!(applier.state(), &json!({ "count": "three" }));
}

#[tokio::test]
async fn a_session_reports_a_failed_patch_and_keeps_the_state_it_had() {
    let transport = ReplayTransport::new([
        Event::run_started("thread-1", "run-1"),
        Event::state_snapshot(json!({ "count": 1 })),
        Event::state_delta(vec![PatchOperation::replace("/missing/deeply", json!(2))]),
        Event::state_delta(vec![PatchOperation::replace("/count", json!(2))]),
        Event::run_finished_success("thread-1", "run-1"),
    ]);
    let mut session = Session::<_>::new(transport, "thread-1");

    let updates: Vec<_> = session.send("go").collect().await;
    let errors: Vec<String> = updates
        .iter()
        .filter_map(|update| match update {
            Update::Error(error) => Some(error.to_string()),
            _ => None,
        })
        .collect();

    assert_eq!(errors.len(), 1, "expected exactly one failure: {errors:?}");
    assert!(errors[0].contains("state patch failed"));

    // The run carried on, and the later delta still applied.
    assert_eq!(session.raw_state(), &json!({ "count": 2 }));
    let states: Vec<_> = updates
        .iter()
        .filter(|update| matches!(update, Update::State(_)))
        .collect();
    assert_eq!(states.len(), 2);
}

#[tokio::test]
async fn a_state_that_does_not_fit_the_typed_view_is_reported_without_losing_it() {
    // The agent publishes a partial state first. A strict type cannot hold it,
    // and saying so beats leaving the caller wondering why `state()` is `None`.
    let transport = ReplayTransport::new([
        Event::run_started("thread-1", "run-1"),
        Event::state_snapshot(json!({ "unrelated": true })),
        Event::state_snapshot(json!({ "count": 7 })),
        Event::run_finished_success("thread-1", "run-1"),
    ]);
    let mut session = Session::<_, Counter>::new(transport, "thread-1");

    let updates: Vec<_> = session.send("go").collect().await;
    let errors: Vec<String> = updates
        .iter()
        .filter_map(|update| match update {
            Update::Error(error) => Some(error.to_string()),
            _ => None,
        })
        .collect();

    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("does not match the expected type"));

    // The typed view caught up on the next snapshot, and the raw state was
    // never wrong.
    assert_eq!(session.state(), Some(&Counter { count: 7 }));
    assert_eq!(session.raw_state(), &json!({ "count": 7 }));
}
