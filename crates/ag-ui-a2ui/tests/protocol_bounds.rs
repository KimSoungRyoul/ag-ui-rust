//! What the protocol modules bound, whatever a payload says: how deep a
//! component tree may nest, how much an outline places, how long checking
//! the bindings of nested lists takes, how much a data model holds that no
//! message wrote, and how much replaying a surface writes.
//!
//! Each input is sized so that the test is fast when the bound holds.

use ag_ui_a2ui::processor::MessageProcessor;
use ag_ui_a2ui::{
    A2uiVersion, AgentMessage, Catalog, Component, ErrorCode, SurfaceState, SurfaceStore,
    ValidateOptions, Validator,
};
use serde_json::{Value, json};

/// A surface `s` holding `components` and the data `data`.
fn surface(components: Vec<Component>, data: Value) -> SurfaceState {
    let mut store = SurfaceStore::new();
    for message in [
        AgentMessage::create_surface("s", "basic"),
        AgentMessage::update_components("s", components),
        AgentMessage::update_data_model("s", "/", data),
    ] {
        store.apply(&message).unwrap();
    }
    store.get("s").unwrap().clone()
}

/// `levels` levels of two Columns under the root, each Column naming both of
/// the next level, and two Texts at the bottom: `2 * levels + 1` components,
/// whose outline places `2^(levels + 1) - 1`.
fn shared_children(levels: usize) -> Vec<Component> {
    let level = |at: usize| [format!("a{at}"), format!("b{at}")];
    let mut components = vec![Component::new("root", "Column").with("children", json!(level(1)))];
    for at in 1..levels {
        for id in level(at) {
            components.push(Component::new(id, "Column").with("children", json!(level(at + 1))));
        }
    }
    for id in level(levels) {
        components.push(Component::new(id, "Text").with("text", json!("leaf")));
    }
    components
}

/// A surface the validator accepts whole may outline to far more than its
/// size. The outline places one component at a time, so a caller takes what
/// it draws and the rest is never placed.
#[test]
fn an_outline_places_only_what_its_caller_takes() {
    let catalog = Catalog::basic();
    let wide = surface(shared_children(20), json!({}));
    assert_eq!(wide.components.len(), 41);
    let report = Validator::new(&catalog).validate(&wide.components);
    assert!(
        report.is_valid() && report.unreachable.is_empty(),
        "{report:?}"
    );

    // Two million placements in all. The first thousand go down the left
    // edge and back up, in drawing order.
    let data = json!({});
    let first: Vec<_> = wide
        .outline(&catalog, &data)
        .take(1_000)
        .map(|placed| (placed.depth, placed.component.id.clone()))
        .collect();
    assert_eq!(first.len(), 1_000);
    assert_eq!(first[0], (0, "root".to_string()));
    assert_eq!(first[1], (1, "a1".to_string()));
    assert_eq!(first[20], (20, "a20".to_string()));
    assert_eq!(first[21], (20, "b20".to_string()));
    assert_eq!(first[22], (19, "b19".to_string()));

    // A small one is placed whole, and counts as the formula says.
    let small = surface(shared_children(6), json!({}));
    assert_eq!(small.outline(&catalog, &data).count(), (1 << 7) - 1);

    // Lists in lists multiply the items the same way.
    let over = |child: &str| json!({"componentId": child, "path": "/items"});
    let nested = surface(
        vec![
            Component::new("root", "List").with("children", over("outer")),
            Component::new("outer", "List").with("children", over("inner")),
            Component::new("inner", "List").with("children", over("leaf")),
            Component::new("leaf", "Text").with("text", json!({"path": "name"})),
        ],
        json!({"items": vec![json!({"name": "x"}); 200]}),
    );
    let data = nested.data_model.to_json().unwrap();
    let leaves: Vec<_> = nested
        .outline(&catalog, &data)
        .filter(|placed| placed.component.id == "leaf")
        .take(500)
        .map(|placed| placed.scope.base().to_string())
        .collect();
    assert_eq!(leaves.len(), 500);
    assert_eq!(leaves[0], "/items/0");
    assert_eq!(leaves[499], "/items/99");
}

/// A chain of `cards` Cards under the root: `root`, then `c1` to
/// `c<cards - 1>`, each the child of the one before, and a Text at the end.
/// The root is listed first.
fn chain(cards: usize) -> Vec<Component> {
    let name = |at: usize| match at {
        0 => "root".to_string(),
        at if at == cards => "leaf".to_string(),
        at => format!("c{at}"),
    };
    let mut components: Vec<Component> = (0..cards)
        .map(|at| Component::new(name(at), "Card").with("child", json!(name(at + 1))))
        .collect();
    components.push(Component::new("leaf", "Text").with("text", json!("end")));
    components
}

/// Where each error of `code` is, as `components[i].child` names it.
fn located(report: &ag_ui_a2ui::ValidationReport, code: ErrorCode) -> Vec<String> {
    report
        .errors
        .iter()
        .filter(|error| error.code == code)
        .map(|error| error.path.clone())
        .collect()
}

/// Depth is how far below the root a component is drawn, so it is measured
/// from the root, wherever in the list the root is.
#[test]
fn the_depth_limit_does_not_depend_on_the_order_the_components_are_listed_in() {
    for version in [A2uiVersion::V0_9, A2uiVersion::V1_0] {
        let catalog = Catalog::basic_for(version);
        let validator = Validator::new(&catalog);
        let root_first = chain(80);
        let mut leaf_first = root_first.clone();
        leaf_first.reverse();

        for components in [&root_first, &leaf_first] {
            // `c50` is 50 below the root, and its child would be 51.
            let c50 = components
                .iter()
                .position(|component| component.id == "c50")
                .unwrap();
            let report = validator.validate(components);
            assert_eq!(
                located(&report, ErrorCode::MaxDepthExceeded),
                [format!("components[{c50}].child")],
                "{version}, {} first",
                components[0].id
            );

            // A renderer refuses the surface either way, and so does the
            // processor that keeps the agent's copy of it.
            let mut processor = MessageProcessor::new();
            processor.register(catalog.clone()).unwrap();
            processor
                .process(&json!({
                    "version": version.as_str(),
                    "createSurface": {"surfaceId": "s", "catalogId": catalog.catalog_id},
                }))
                .unwrap();
            let rejection = processor
                .process(&json!({
                    "version": version.as_str(),
                    "updateComponents": {"surfaceId": "s", "components": components},
                }))
                .expect_err("a chain deeper than a renderer draws");
            assert!(
                rejection
                    .error
                    .to_string()
                    .contains("Global recursion limit exceeded"),
                "{version}: {}",
                rejection.error
            );
        }

        // Under the limit, the order makes no difference either.
        let mut shallow = chain(50);
        assert!(validator.validate(&shallow).is_valid(), "{version}");
        shallow.reverse();
        assert!(validator.validate(&shallow).is_valid(), "{version}");
    }
}

/// A loop is found wherever it is, reachable from the root or not, and a long
/// one is a loop and not a depth overrun.
#[test]
fn a_loop_is_found_wherever_it_is_listed() {
    let catalog = Catalog::basic();
    let validator = Validator::with_options(&catalog, ValidateOptions::default());
    let mut components = vec![
        Component::new("leaf", "Text").with("text", json!("x")),
        Component::new("root", "Card").with("child", json!("leaf")),
    ];
    // A loop of 60 the root does not reach, listed before it.
    for at in 0..60 {
        components.insert(
            0,
            Component::new(format!("l{at}"), "Card")
                .with("child", json!(format!("l{}", (at + 1) % 60))),
        );
    }
    let report = validator.validate(&components);
    assert_eq!(
        located(&report, ErrorCode::ChildCycle).len(),
        1,
        "{report:?}"
    );
    assert!(
        located(&report, ErrorCode::MaxDepthExceeded).is_empty(),
        "{report:?}"
    );
}

/// A processor of `version` that has created surface `s` on the basic catalog.
fn processor_with_surface(version: A2uiVersion) -> MessageProcessor {
    let catalog = Catalog::basic_for(version);
    let mut processor = MessageProcessor::new();
    let create = json!({
        "version": version.as_str(),
        "createSurface": {"surfaceId": "s", "catalogId": catalog.catalog_id},
    });
    processor.register(catalog).unwrap();
    processor.process(&create).unwrap();
    processor
}

/// What a data model holds that no message wrote is bounded over all the
/// updates, not per update. Each of these messages is a few dozen bytes and
/// grows six lists to the last index a write may name, which one update may
/// do; the first leaves 60,000 undefined slots, and the next is refused.
#[test]
fn a_data_model_holds_a_bounded_number_of_slots_no_message_wrote() {
    for version in [A2uiVersion::V0_9, A2uiVersion::V1_0] {
        let mut processor = processor_with_surface(version);
        let grow = |key: &str| {
            json!({
                "version": version.as_str(),
                "updateDataModel": {
                    "surfaceId": "s",
                    "path": format!("/{key}{}", "/10000".repeat(6)),
                    "value": 1,
                },
            })
        };
        let held = |processor: &MessageProcessor| {
            processor
                .surfaces()
                .get("s")
                .unwrap()
                .data_model
                .undefined_slots()
        };
        processor.process(&grow("k0")).unwrap();
        assert_eq!(held(&processor), 60_000, "{version}");

        let rejection = processor
            .process(&grow("k1"))
            .expect_err("a second message past the bound");
        assert!(
            rejection.error.to_string().contains("undefined list slots"),
            "{version}: {}",
            rejection.error
        );
        assert_eq!(held(&processor), 60_000, "{version}: nothing applied");
    }
}
