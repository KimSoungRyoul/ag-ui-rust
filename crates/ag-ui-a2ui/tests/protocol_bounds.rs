//! What the protocol modules bound, whatever a payload says: how deep a
//! component tree may nest, how much an outline places, how long checking
//! the bindings of nested lists takes, how much a data model holds that no
//! message wrote, and how much replaying a surface writes.
//!
//! Each input is sized so that the test is fast when the bound holds.

use ag_ui_a2ui::processor::MessageProcessor;
use ag_ui_a2ui::{A2uiVersion, Catalog, Component, ErrorCode, ValidateOptions, Validator};
use serde_json::json;

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
