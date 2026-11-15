#![cfg(feature = "author")]
use ag_ui_a2ui::schema::SchemaBundle;
use ag_ui_a2ui::schema_validation::SchemaValidator;
use ag_ui_a2ui::{A2uiAuthor, A2uiVersion, Error, ErrorCode};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    future::Future,
    task::{Context, Poll, Waker},
};
fn ready<F: Future>(future: F) -> F::Output {
    let mut context = Context::from_waker(Waker::noop());
    let mut future = std::pin::pin!(future);
    match future.as_mut().poll(&mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("fixture unexpectedly waited"),
    }
}
fn ops() -> Vec<Value> {
    vec![
        json!({"version":"v0.9.1","createSurface":{"surfaceId":"s","catalogId":ag_ui_a2ui::constants::OFFICIAL_BASIC_CATALOG_ID}}),
        json!({"version":"v0.9.1","updateComponents":{"surfaceId":"s","components":[{"id":"root","component":"Text","text":"Hello"}]}}),
        json!({"version":"v0.9.1","updateDataModel":{"surfaceId":"s","path":"/","value":{"memo":null,"items":[1,null]}}}),
    ]
}
#[test]
fn official_schema_resolves_locally_and_rejects_invalid_raw_documents() {
    let schema = SchemaValidator::new(&SchemaBundle::basic().unwrap(), &BTreeMap::new()).unwrap();
    for message in ops() {
        schema.validate_message(&message).unwrap();
    }
    let mut wrong_enum = ops()[1].clone();
    wrong_enum["updateComponents"]["components"][0]["variant"] = json!("banana");
    let mut unknown_prop = ops()[1].clone();
    unknown_prop["updateComponents"]["components"][0]["secret"] = json!(true);
    let mut extra_payload = ops()[0].clone();
    extra_payload["deleteSurface"] = json!({"surfaceId":"s"});
    let mut empty = ops()[1].clone();
    empty["updateComponents"]["components"] = json!([]);
    let mut missing_version = ops()[0].clone();
    missing_version.as_object_mut().unwrap().remove("version");
    for invalid in [
        wrong_enum,
        unknown_prop,
        extra_payload,
        empty,
        missing_version,
    ] {
        assert!(schema.validate_message(&invalid).is_err(), "{invalid}");
    }
    let mut bundle = SchemaBundle::basic().unwrap();
    bundle.s2c["$ref"] = json!("https://unregistered.invalid/schema.json");
    assert!(SchemaValidator::new(&bundle, &BTreeMap::new()).is_err());
}
#[test]
fn create_and_edit_pin_target_version_catalog_and_lifecycle() {
    let author = A2uiAuthor::basic(A2uiVersion::V0_9_1).unwrap();
    let initial = author.validate_create("s", &ops()).unwrap();
    for field in ["surfaceId", "catalogId"] {
        let mut wrong = ops();
        wrong[0]["createSurface"][field] = json!("other");
        assert!(author.validate_create("s", &wrong).is_err());
    }
    let mut wrong = ops();
    wrong[1]["version"] = json!("v0.9");
    assert!(author.validate_create("s", &wrong).is_err());
    assert!(author.validate_edit(&initial, &ops()).is_err());
    assert!(
        author
            .validate_edit(
                &initial,
                &[json!({"version":"v0.9.1","deleteSurface":{"surfaceId":"s"}})]
            )
            .is_err()
    );
    let edited = author
        .validate_edit(
            &initial,
            &[json!({"version":"v0.9.1","updateDataModel":{"surfaceId":"s","path":"/items/0"}})],
        )
        .unwrap();
    assert!(edited.data_model().to_json().is_err());
    let saved: Vec<Value> = edited
        .history()
        .iter()
        .map(|op| serde_json::to_value(op).unwrap())
        .collect();
    assert_eq!(
        author.validate_create("s", &saved).unwrap().data_model(),
        edited.data_model()
    );
}
#[test]
fn async_generation_corrects_documents_and_preserves_provider_errors() {
    let author = A2uiAuthor::basic(A2uiVersion::V0_9_1).unwrap();
    let future = author
        .create("s", "greet")
        .generate(|prompt, attempt| async move {
            if attempt == 1 {
                Ok("invalid generated output".into())
            } else {
                assert!(prompt.contains("Correction required"));
                Ok(serde_json::to_string(&ops()).unwrap())
            }
        });
    fn assert_send<T: Send>(_: &T) {}
    assert_send(&future);
    let surface = ready(future).unwrap();
    assert_eq!(surface.attempts(), 2);
    let calls = std::rc::Rc::new(std::cell::Cell::new(0));
    let callback_calls = calls.clone();
    let error = ready(author.create("s", "greet").generate(move |_, _| {
        let calls = callback_calls.clone();
        async move {
            calls.set(calls.get() + 1);
            Err(Error::generation(std::io::Error::other(
                "provider unavailable",
            )))
        }
    }))
    .unwrap_err();
    assert_eq!(calls.get(), 1);
    assert!(matches!(error, Error::Generation(_)));
    assert!(std::error::Error::source(&error).is_some());
}

/// Located the way a renderer's refusals are: `(code, path)` of each failure.
fn located(error: Error) -> Vec<(ErrorCode, String)> {
    match error {
        Error::Validation { errors } => errors
            .as_slice()
            .iter()
            .map(|error| (error.code, error.path.clone()))
            .collect(),
        other => panic!("expected a validation error, got {other:?}"),
    }
}

/// What a batch breaks of the request, as opposed to of A2UI, is reported the
/// same way: a validation error where the message has to change.
#[test]
fn a_batch_that_is_not_what_was_asked_for_is_located_in_the_message() {
    let author = A2uiAuthor::basic(A2uiVersion::V0_9_1).unwrap();
    let invalid = |path: &str| vec![(ErrorCode::InvalidValue, path.to_string())];

    let mut other_surface = ops();
    other_surface[0]["createSurface"]["surfaceId"] = json!("other");
    assert_eq!(
        located(
            author
                .validate_create("s", &other_surface[..1])
                .unwrap_err()
        ),
        invalid("messages[0].createSurface.surfaceId")
    );

    let mut other_version = ops();
    other_version[1]["version"] = json!("v0.9");
    assert_eq!(
        located(author.validate_create("s", &other_version).unwrap_err()),
        invalid("messages[1].version")
    );

    let mut deleted = ops();
    deleted.push(json!({"version":"v0.9.1","deleteSurface":{"surfaceId":"s"}}));
    assert_eq!(
        located(author.validate_create("s", &deleted).unwrap_err()),
        invalid("messages[3]")
    );

    // A surface nothing is drawn into is not a surface to hand back.
    assert_eq!(
        located(author.validate_create("s", &ops()[..1]).unwrap_err()),
        [(ErrorCode::EmptyComponents, "response".to_string())]
    );

    let initial = author.validate_create("s", &ops()).unwrap();
    let recreated = author.validate_edit(&initial, &ops()[..1]).unwrap_err();
    assert_eq!(located(recreated), invalid("messages[0]"));
}

/// A component without an `id` is reported where it is, not as a message that
/// did not decode.
#[test]
fn a_component_without_an_id_is_located_and_fed_back_where_it_is() {
    let author = A2uiAuthor::basic(A2uiVersion::V0_9_1).unwrap();
    let mut nameless = ops();
    nameless[1]["updateComponents"]["components"] = json!([
        {"id": "root", "component": "Column", "children": ["title"]},
        {"component": "Text", "text": "Hello"},
    ]);
    let found = located(author.validate_create("s", &nameless).unwrap_err());
    assert!(
        found
            .iter()
            .all(|(_, path)| path.starts_with("messages[1].updateComponents.components[1]")),
        "{found:?}"
    );

    let answer = serde_json::to_string(&nameless).unwrap();
    let surface = ready(
        author
            .create("s", "greet")
            .generate(move |prompt, attempt| {
                let answer = answer.clone();
                async move {
                    if attempt == 1 {
                        return Ok(answer);
                    }
                    assert!(
                        prompt.contains("at messages[1].updateComponents.components[1]"),
                        "{prompt}"
                    );
                    Ok(serde_json::to_string(&ops()).unwrap())
                }
            }),
    )
    .unwrap();
    assert_eq!(surface.attempts(), 2);
}

/// The attempts are the author's to set; the default is three.
#[test]
fn the_number_of_attempts_is_the_authors_and_never_zero() {
    for (limit, expected) in [(None, 3), (Some(1), 1), (Some(5), 5), (Some(0), 1)] {
        let mut author = A2uiAuthor::basic(A2uiVersion::V0_9_1).unwrap();
        if let Some(limit) = limit {
            author = author.with_max_attempts(limit);
        }
        let calls = std::cell::Cell::new(0);
        let error = ready(author.create("s", "greet").generate(|_, attempt| {
            calls.set(attempt);
            async { Ok("not A2UI".to_string()) }
        }))
        .unwrap_err();
        assert_eq!(calls.get(), expected);
        let Error::RecoveryExhausted { attempts, last } = error else {
            panic!("expected RecoveryExhausted");
        };
        assert_eq!(attempts, expected);
        assert_eq!(last.as_slice()[0].path, "response");
    }
}

/// The author is the agent layer with a fixed target: its prompt is the
/// layer's snippet under the request, and an answer may come tagged or bare.
#[test]
fn the_prompt_is_the_agent_layers_and_an_answer_may_be_tagged_or_bare() {
    let author = A2uiAuthor::basic(A2uiVersion::V0_9_1).unwrap();
    let prompt = author.create("s", "greet").prompt();
    assert!(prompt.starts_with("Create exactly one surface"));
    assert!(prompt.contains("Target surfaceId: s"));
    assert!(prompt.contains("Exact version: v0.9.1"));
    assert!(prompt.contains(author.processor().prompt_snippet()));

    let bare = serde_json::to_string(&ops()).unwrap();
    let tagged = format!("Here you go.\n<a2ui-json>{bare}</a2ui-json>");
    for answer in [bare, tagged] {
        let surface = ready(
            author
                .create("s", "greet")
                .generate(|_, _| std::future::ready(Ok(answer.clone()))),
        )
        .unwrap();
        assert_eq!(surface.attempts(), 1);
        assert_eq!(surface.operations().len(), 3);
    }

    let initial = author.validate_create("s", &ops()).unwrap();
    let edit = author.edit(&initial, "rename").prompt();
    assert!(edit.starts_with("Edit the existing surface"));
    assert!(edit.contains("Observed prior operations"));
}

#[test]
fn send_data_model_policy_corrects_missing_flags_and_survives_edits() {
    let author = A2uiAuthor::basic(A2uiVersion::V0_9_1)
        .unwrap()
        .with_send_data_model(true);
    let initial = ready(author.create("s", "show an input form").generate(
        |prompt, attempt| async move {
            assert!(prompt.contains("Required createSurface.sendDataModel: true"));
            let mut messages = ops();
            if attempt > 1 {
                assert!(prompt.contains("messages[0].createSurface.sendDataModel"));
                assert!(prompt.contains("must be true"));
                messages[0]["createSurface"]["sendDataModel"] = json!(true);
            }
            Ok(serde_json::to_string(&messages).unwrap())
        },
    ))
    .unwrap();
    assert_eq!(initial.attempts(), 2);
    let edit = [json!({"version":"v0.9.1","updateDataModel":{
        "surfaceId":"s","path":"/notes","value":"user input"
    }})];
    let edited = author.validate_edit(&initial, &edit).unwrap();
    let saved: Vec<_> = edited
        .history()
        .iter()
        .map(|op| serde_json::to_value(op).unwrap())
        .collect();
    assert_eq!(saved[0]["createSurface"]["sendDataModel"], true);
    assert_eq!(
        author.validate_create("s", &saved).unwrap().data_model(),
        edited.data_model()
    );

    let unrestricted = A2uiAuthor::basic(A2uiVersion::V0_9_1).unwrap();
    let missing_flag = unrestricted.validate_create("s", &ops()).unwrap();
    assert!(author.validate_edit(&missing_flag, &edit).is_err());
    let error =
        ready(author.edit(&missing_flag, "edit").generate(|_, _| async {
            panic!("incompatible prior must fail before provider call")
        }))
        .unwrap_err();
    assert!(error.to_string().contains("must be true"));
}

#[test]
fn disabled_data_model_sync_accepts_omission_but_rejects_true() {
    let author = A2uiAuthor::basic(A2uiVersion::V0_9_1).unwrap();
    let disabled = author.clone().with_send_data_model(false);
    assert!(
        disabled
            .create("s", "form")
            .prompt()
            .contains("sendDataModel: false")
    );
    for flag in [None, Some(false), Some(true)] {
        let mut messages = ops();
        if let Some(flag) = flag {
            messages[0]["createSurface"]["sendDataModel"] = json!(flag);
        }
        assert!(author.validate_create("s", &messages).is_ok());
        assert_eq!(
            disabled.validate_create("s", &messages).is_ok(),
            flag != Some(true)
        );
    }
}

#[test]
fn strict_binding_lint_distinguishes_undefined_from_null() {
    use ag_ui_a2ui::{Catalog, Component, DataModel, DataModelUpdate, Validator};
    let components = vec![Component::new("root", "Text").with("text", json!({"path":"/items/0"}))];
    let mut model = DataModel::from(json!({"items":[null]}));
    let catalog = Catalog::basic();
    let validator = Validator::with_options(
        &catalog,
        ag_ui_a2ui::ValidateOptions {
            require_bound_values: true,
            ..Default::default()
        },
    );
    assert!(validator.validate_model(&components, &model).is_valid());
    model.apply("/items/0", &DataModelUpdate::Remove).unwrap();
    assert!(!validator.validate_model(&components, &model).is_valid());
}

#[test]
fn registered_multi_catalog_stream_is_transactional_and_finalization_checks_pending_refs() {
    use ag_ui_a2ui::schema_validation::SurfaceValidator;
    let bundle = SchemaBundle::basic().unwrap();
    let mut stream = SurfaceValidator::new();
    stream.register(&bundle, &BTreeMap::new()).unwrap();
    stream.push(&ops()[0]).unwrap();
    assert!(stream.finish().is_err());
    let mut root = ops()[1].clone();
    root["updateComponents"]["components"] =
        json!([{"id":"root","component":"Card","child":"later"}]);
    stream.push(&root).unwrap();
    assert!(stream.finish().is_err());
    let before = stream.state().clone();
    let mut bad = root.clone();
    bad["updateComponents"]["components"][0]["child"] = json!("root");
    assert!(stream.push(&bad).is_err());
    assert_eq!(stream.state(), &before);
    stream.push(&json!({"version":"v0.9.1","updateComponents":{"surfaceId":"s","components":[{"id":"later","component":"Text","text":"arrived"}]}})).unwrap();
    stream.finish().unwrap();
    let mut other = bundle.clone();
    other.catalog["catalogId"] = json!("example.com:other");
    stream.register(&other, &BTreeMap::new()).unwrap();
    stream.push(&json!({"version":"v0.9.1","createSurface":{"surfaceId":"other","catalogId":"example.com:other"}})).unwrap();
    stream.push(&json!({"version":"v0.9.1","updateComponents":{"surfaceId":"other","components":[{"id":"root","component":"Text","text":"independent"}]}})).unwrap();
    stream.finish().unwrap();
    assert_eq!(stream.state().surfaces().count(), 2);
    let mut resumed = SurfaceValidator::from_state(stream.state().clone());
    resumed.register(&bundle, &BTreeMap::new()).unwrap();
    resumed.register(&other, &BTreeMap::new()).unwrap();
    resumed.push(&json!({"version":"v0.9.1","updateDataModel":{"surfaceId":"other","path":"/memo","value":null}})).unwrap();
    resumed.finish().unwrap();
}

#[test]
fn an_edit_revalidates_prior_components_against_the_current_contract() {
    let mut weak_bundle = SchemaBundle::basic().unwrap();
    weak_bundle.catalog["components"]["Text"]["allOf"][2]["properties"]["variant"] =
        json!({"type":"string"});
    let weak = A2uiAuthor::new(A2uiVersion::V0_9_1, weak_bundle, BTreeMap::new()).unwrap();
    let mut messages = ops();
    messages[1]["updateComponents"]["components"][0]["variant"] = json!("banana");
    let prior = weak.validate_create("s", &messages).unwrap();
    let strict = A2uiAuthor::basic(A2uiVersion::V0_9_1).unwrap();
    assert!(strict.validate_edit(&prior, &[ops()[2].clone()]).is_err());
    assert!(strict.validate_edit(&prior, &[ops()[1].clone()]).is_err());
    let generated =
        ready(strict.edit(&prior, "repair").generate(|_, _| async {
            panic!("incompatible prior must fail before provider call")
        }));
    assert!(generated.is_err());
}

#[test]
fn inline_capabilities_keep_function_array_theme_and_local_resources() {
    use ag_ui_a2ui::agent::{CatalogConfig, RendererCapabilities, resolve_catalogs};
    use ag_ui_a2ui::schema_validation::validate_capabilities;
    let wire = json!({"v0.9":{"supportedCatalogIds":["example.com:inline"],"inlineCatalogs":[{
        "catalogId":"example.com:inline",
        "components":{"Label":{"type":"object","properties":{"value":{"$ref":"https://local.example/label.json"}},"required":["value"]}},
        "functions":[{"name":"label","parameters":{"type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false},"returnType":"string"}],
        "theme":{"accent":{"type":"string","enum":["blue","green"]}}
    }]}});
    validate_capabilities(&wire).unwrap();
    let caps = RendererCapabilities::from_json(&wire).unwrap();
    // An agent that accepts a renderer's own catalogs gets this one active
    // beside what it registered, as the renderer sent it.
    let registered = [CatalogConfig::basic(A2uiVersion::V0_9).unwrap()];
    assert!(resolve_catalogs(&registered, Some(&caps), false).is_err());
    let active = resolve_catalogs(&registered, Some(&caps), true).unwrap();
    assert_eq!(active.len(), 1);
    let selected = active[0].document().clone();
    assert!(selected["functions"].is_array());
    assert_eq!(
        selected["theme"],
        wire["v0.9"]["inlineCatalogs"][0]["theme"]
    );
    let bundle = SchemaBundle::from_inline_catalog(selected).unwrap();
    let resources = BTreeMap::from([(
        "https://local.example/label.json".into(),
        json!({"$schema":"https://json-schema.org/draft/2020-12/schema","type":"string","minLength":1}),
    )]);
    let author = A2uiAuthor::new(A2uiVersion::V0_9_1, bundle, resources).unwrap();
    let messages = vec![
        json!({"version":"v0.9.1","createSurface":{"surfaceId":"s","catalogId":"example.com:inline","theme":{"accent":"blue"}}}),
        json!({"version":"v0.9.1","updateComponents":{"surfaceId":"s","components":[{"id":"root","component":"Label","value":"local"}]}}),
    ];
    author.validate_create("s", &messages).unwrap();
    let mut invalid = wire;
    invalid["v0.9"]["inlineCatalogs"][0]["functions"][0]["returnType"] = json!("invalid");
    assert!(validate_capabilities(&invalid).is_err());
}
