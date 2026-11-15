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

/// The agent refuses the answer a renderer would, in either order.
#[cfg(feature = "agent")]
#[test]
fn an_answer_whose_tree_is_too_deep_is_refused_in_either_order() {
    use ag_ui_a2ui::agent::{A2uiGenerator, CatalogConfig, RendererCapabilities};

    let catalog = Catalog::basic();
    let generator = A2uiGenerator::new(vec![CatalogConfig::basic(A2uiVersion::V0_9).unwrap()]);
    let renderer = RendererCapabilities::supporting(A2uiVersion::V0_9, [&catalog.catalog_id]);
    let processor = generator.create_processor(Some(&renderer)).unwrap();
    let mut leaf_first = chain(80);
    leaf_first.reverse();
    for components in [chain(80), leaf_first] {
        let answer = format!(
            "<a2ui-json>{}</a2ui-json>",
            json!([
                {"version": "v0.9", "createSurface": {"surfaceId": "s", "catalogId": catalog.catalog_id}},
                {"version": "v0.9", "updateComponents": {"surfaceId": "s", "components": components}},
            ])
        );
        let refused = processor
            .parse_response(&answer)
            .expect_err("a tree deeper than a renderer draws");
        assert!(
            refused.to_string().contains("max_depth_exceeded"),
            "{refused}"
        );
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

/// A surface of `version` on its basic catalog, with one Text, after
/// `updates` to its data model.
fn surface_after(version: A2uiVersion, updates: &[AgentMessage]) -> SurfaceState {
    let catalog = Catalog::basic_for(version);
    let mut store = SurfaceStore::new();
    let mut messages = vec![
        AgentMessage::create_surface("s", catalog.catalog_id.clone()),
        AgentMessage::update_components(
            "s",
            vec![Component::new("root", "Text").with("text", json!("hi"))],
        ),
    ];
    messages.extend(updates.iter().cloned());
    for message in messages {
        store.apply(&message.with_version(version)).unwrap();
    }
    store.get("s").unwrap().clone()
}

/// The replay of `surface`, applied on a store and, as JSON, on a processor
/// that holds nothing: each must hold `surface` again. Gives the replay as
/// JSON.
fn replays(surface: &SurfaceState) -> Vec<Value> {
    let version = surface.spoken_version();
    let replay = surface.replay();
    let mut store = SurfaceStore::new();
    for message in &replay {
        store.apply(message).unwrap();
    }
    assert_eq!(store.get("s"), Some(surface));

    let wire: Vec<Value> = replay
        .iter()
        .map(|message| serde_json::to_value(message).unwrap())
        .collect();
    let mut processor = MessageProcessor::new();
    processor.register(Catalog::basic_for(version)).unwrap();
    for message in &wire {
        if let Err(rejection) = processor.process(message) {
            panic!("{version}: {message}: {}", rejection.error);
        }
    }
    assert_eq!(
        processor.surfaces().get("s").unwrap().data_model,
        surface.data_model,
        "{version}"
    );
    wire
}

/// A list that grew to a far index holds the slots before it undefined. A
/// replay writes what the model holds, not one message per slot it does not.
#[test]
fn a_replay_is_as_large_as_the_values_a_surface_holds() {
    for version in [A2uiVersion::V0_9, A2uiVersion::V1_0] {
        let set = |path: &str, value: Value| AgentMessage::update_data_model("s", path, value);
        let remove = |path: &str| AgentMessage::remove_data_model_value("s", path);
        let mut updates = vec![
            // A list grown to the last index, and two lists in one path.
            set("/far/10000", json!("last")),
            set("/grid/3/9000", json!({"cell": [1, null]})),
            // A list written whole, then a slot removed from it.
            set("/kept", json!(["a", null, "c", "d"])),
            remove("/kept/2"),
            // A list whose last slot is removed.
            set("/tail", json!([1, 2, 3])),
            remove("/tail/2"),
        ];
        // Removing past the end of a list that is not there changes nothing.
        updates.push(remove("/padded/9999"));
        let surface = surface_after(version, &updates);
        let held = surface.data_model.undefined_slots();
        assert!(held > 19_000, "{version}: {held}");

        let wire = replays(&surface);
        let bytes = serde_json::to_string(&wire).unwrap().len();
        assert!(wire.len() <= 16, "{version}: {} messages", wire.len());
        assert!(
            bytes < 4_000,
            "{version}: {bytes} bytes for {held} undefined slots"
        );
    }
}

/// Removing the root of a v0.9 data model leaves it undefined, and the
/// replay says so instead of leaving the empty object a new surface starts
/// with.
#[test]
fn a_removed_root_replays_as_removed() {
    let removed = surface_after(
        A2uiVersion::V0_9,
        &[
            AgentMessage::update_data_model("s", "/", json!({"a": 1})),
            AgentMessage::remove_data_model_value("s", "/"),
        ],
    );
    assert_eq!(
        removed.data_model.root(),
        &ag_ui_a2ui::ModelValue::Undefined
    );
    replays(&removed);
}

/// A model nests deeper than one message may: a path of up to 256 segments,
/// and a value under it. Its replay splits the model where a message would
/// nest past what a processor takes.
#[test]
fn a_deep_data_model_replays_as_messages_a_processor_accepts() {
    let path = |segments: usize| -> String { (0..segments).map(|at| format!("/k{at}")).collect() };
    let nested = |levels: usize| {
        let mut value = json!("bottom");
        for at in 0..levels {
            value = if at % 2 == 0 {
                json!([value])
            } else {
                json!({"n": value})
            };
        }
        value
    };
    for version in [A2uiVersion::V0_9, A2uiVersion::V1_0] {
        for (segments, levels) in [(60, 0), (60, 40), (200, 40), (256, 46), (10, 46)] {
            let surface = surface_after(
                version,
                &[
                    AgentMessage::update_data_model("s", path(segments), nested(levels)),
                    AgentMessage::update_data_model("s", "/side", json!([1, 2])),
                ],
            );
            let wire = replays(&surface);
            assert!(
                wire.len() <= 2 + (segments + levels) / 20,
                "{segments}+{levels}: {}",
                wire.len()
            );
        }
    }
}

/// Whatever updates built a model, its replay holds it again.
#[test]
fn any_data_model_replays_as_it_is() {
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut roll = |sides: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % sides as u64) as usize
    };
    let segments = ["a", "b", "0", "1", "4", "30"];
    let values = [
        json!(1),
        json!(null),
        json!([1, null, 3]),
        json!({"x": [0, 1]}),
        json!([]),
        json!({}),
    ];
    for version in [A2uiVersion::V0_9, A2uiVersion::V1_0] {
        for _ in 0..60 {
            let mut updates = Vec::new();
            for _ in 0..12 {
                let path: String = (0..1 + roll(4))
                    .map(|_| format!("/{}", segments[roll(segments.len())]))
                    .collect();
                updates.push(if roll(3) == 0 {
                    AgentMessage::remove_data_model_value("s", path)
                } else {
                    AgentMessage::update_data_model("s", path, values[roll(values.len())].clone())
                });
            }
            // Apply what applies; a write through a scalar is refused.
            let catalog = Catalog::basic_for(version);
            let mut store = SurfaceStore::new();
            store
                .apply(&AgentMessage::create_surface("s", catalog.catalog_id).with_version(version))
                .unwrap();
            for update in updates {
                let _ = store.apply(&update.with_version(version));
            }
            replays(store.get("s").unwrap());
        }
    }
}
