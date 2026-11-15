//! The adapters on A2UI v1.0, through their public API.
//!
//! What a host meets when a frontend, or an A2A client, speaks v1.0: how the
//! version is chosen, what the tool and the capabilities say, and that a
//! surface painted in v1.0 is read back as one.

#![cfg(any(feature = "ag-ui", feature = "a2a"))]

use ag_ui_a2ui::A2uiVersion;
use serde_json::json;

const BASIC: &str = "https://a2ui.org/specification/v1_0/catalogs/basic/catalog.json";

#[cfg(feature = "ag-ui")]
mod agui {
    use super::*;
    use ag_ui::{ActivityMessage, Event, Message};
    use ag_ui_a2ui::adapters::agui::{
        CatalogContext, Intent, RenderArgs, SurfacePainter, SurfaceSpec, assemble_ops,
        basic_catalog, basic_catalog_id, latest_surface, operations_in, render_a2ui_tool_for,
        surfaces_in,
    };
    use ag_ui_a2ui::agent::{A2uiGenerator, CatalogConfig, ResponsePart};
    use ag_ui_a2ui::constants::A2UI_OPERATIONS_KEY;
    use ag_ui_a2ui::validate::Validator;
    use ag_ui_a2ui::{AgentMessage, AgentPayload};
    use serde_json::Value;

    /// The operations an activity snapshot carries.
    fn shown(event: &Event) -> Vec<Value> {
        let Event::ActivitySnapshot(snapshot) = event else {
            panic!("a painter emits activity snapshots, not {event:?}");
        };
        snapshot.content[A2UI_OPERATIONS_KEY]
            .as_array()
            .expect("an activity carries its operations")
            .clone()
    }

    fn kinds(operations: &[Value]) -> Vec<&str> {
        operations
            .iter()
            .map(|operation| {
                [
                    "createSurface",
                    "updateComponents",
                    "updateDataModel",
                    "deleteSurface",
                ]
                .into_iter()
                .find(|kind| operation.get(kind).is_some())
                .expect("an operation of a kind this test knows")
            })
            .collect()
    }

    /// The messages a client holds after a painter's events: the tool result,
    /// then each activity.
    fn conversation(painter: &mut SurfacePainter, operations: &[AgentMessage]) -> Vec<Message> {
        let painted = painter.settle("call-1", operations).unwrap();
        let mut messages = vec![Message::tool("m-1", "call-1", painted.envelope)];
        for event in painted.activities {
            let Event::ActivitySnapshot(snapshot) = event else {
                panic!("a painter emits activity snapshots");
            };
            messages.push(Message::Activity(ActivityMessage {
                id: snapshot.message_id,
                activity_type: snapshot.activity_type,
                content: snapshot.content,
                ..ActivityMessage::default()
            }));
        }
        messages
    }

    #[test]
    fn a_frontend_that_declares_1_0_is_served_in_1_0() {
        // The catalog entry of the request's context says which protocol the
        // frontend's renderer speaks.
        let context = CatalogContext::new(json!({"catalogId": BASIC, "protocolVersion": "1.0"}));
        let version = context.protocol_version();
        assert_eq!(version, A2uiVersion::V1_0);
        assert_eq!(basic_catalog_id(version), BASIC);

        // Its capabilities are a v1.0 renderer's, and negotiate a v1.0
        // session.
        let capabilities = context.capabilities(version).unwrap();
        assert_eq!(
            capabilities.to_json(),
            json!({"1.0": {"supportedCatalogIds": [BASIC]}})
        );
        let generator = A2uiGenerator::new(vec![CatalogConfig::basic(version).unwrap()]);
        let processor = generator.create_processor(Some(&capabilities)).unwrap();
        assert_eq!(processor.active_catalogs()[0].version(), version);

        // The tool says so to the model that fills it in.
        let tool = render_a2ui_tool_for(version);
        assert!(
            tool.description.contains("A2UI 1.0"),
            "{}",
            tool.description
        );

        // The model's answer creates the surface whole, in one message.
        let answer = format!(
            r#"<a2ui-json>[{{"version": "1.0", "createSurface": {{
              "surfaceId": "order",
              "catalogId": "{BASIC}",
              "dataModel": {{"title": "Your order"}},
              "components": [
                {{"id": "root", "component": "Column", "children": ["title"]}},
                {{"id": "title", "component": "Text", "text": {{"@path": "/title"}}}}
              ]
            }}}}]</a2ui-json>"#
        );
        let operations: Vec<AgentMessage> = processor
            .parse_response(&answer)
            .unwrap()
            .into_iter()
            .flat_map(|part| match part {
                ResponsePart::A2ui(messages) => messages,
                ResponsePart::Text(_) => Vec::new(),
            })
            .collect();
        assert_eq!(operations.len(), 1);

        let mut painter = SurfacePainter::new();
        let painted = painter.settle("call-1", &operations).unwrap();
        // The tool result is the batch as the model wrote it.
        let result: Value = serde_json::from_str(&painted.envelope).unwrap();
        assert_eq!(
            kinds(result[A2UI_OPERATIONS_KEY].as_array().unwrap()),
            ["createSurface"]
        );
        // The activity is the surface as three operations, which is how
        // AG-UI's middleware writes a surface in either protocol, each one
        // in v1.0.
        let activity = shown(&painted.activities[0]);
        assert_eq!(
            kinds(&activity),
            ["createSurface", "updateComponents", "updateDataModel"]
        );
        assert!(
            activity
                .iter()
                .all(|operation| operation["version"] == "1.0"),
            "{activity:?}"
        );
        assert_eq!(
            activity[1]["updateComponents"]["components"][1]["text"],
            json!({"@path": "/title"})
        );
        assert_eq!(
            painter.surfaces().get("order").unwrap().spoken_version(),
            version
        );
    }

    #[test]
    fn a_v1_0_surface_is_assembled_painted_and_read_back_in_v1_0() {
        // What the model filled `render_a2ui` in with, under the catalog and
        // the protocol the host chose.
        let args: RenderArgs = serde_json::from_value(json!({
            "surfaceId": "cart",
            "components": [
                {"id": "root", "component": "Column", "children": ["total"]},
                {"id": "total", "component": "Text", "text": {"@path": "/total"}},
            ],
            "data": {"total": "12.00"},
        }))
        .unwrap();
        let version = A2uiVersion::V1_0;
        let spec = args
            .into_spec(basic_catalog_id(version))
            .with_version(version);
        let operations = assemble_ops(Intent::Create, &spec);
        assert!(
            operations
                .iter()
                .all(|operation| operation.version == "1.0")
        );

        // The catalog the adapter gives for that id holds what was assembled.
        let catalog = basic_catalog(version);
        assert_eq!(catalog.catalog_id, BASIC);
        Validator::new(&catalog)
            .validate(&spec.components)
            .into_result()
            .unwrap();

        let mut painter = SurfacePainter::new();
        let mut messages = conversation(&mut painter, &operations);

        // Read back from the conversation, the surface speaks v1.0.
        assert_eq!(operations_in(&messages[0]).unwrap(), operations);
        let surfaces = surfaces_in(&messages).unwrap();
        let cart = surfaces.get("cart").unwrap();
        assert_eq!(cart.spoken_version(), version);
        assert_eq!(cart.catalog_id.as_deref(), Some(BASIC));
        assert_eq!(
            cart.data_model.to_json().unwrap(),
            json!({"total": "12.00"})
        );
        assert_eq!(latest_surface(&messages).unwrap().as_ref(), Some(cart));

        // A later turn writes `null`, which removes in v1.0: the painter that
        // starts from the conversation applies it as a v1.0 surface does.
        let mut painter = SurfacePainter::from_history(&messages).unwrap();
        let removal =
            AgentMessage::update_data_model("cart", "/total", Value::Null).with_version(version);
        let painted = painter.settle("call-2", &[removal]).unwrap();
        messages.push(Message::tool("m-2", "call-2", painted.envelope));
        let cart = latest_surface(&messages).unwrap().unwrap();
        assert_eq!(cart.data_model.to_json().unwrap(), json!({}));
        // The activity is replaced in place, by the surface as it now is.
        let activity = shown(&painted.activities[0]);
        assert!(
            activity
                .iter()
                .all(|operation| operation["version"] == "1.0")
        );
        assert!(
            activity.iter().all(|operation| {
                operation
                    .pointer("/updateDataModel/value")
                    .is_none_or(|value| value == &json!({}))
            }),
            "{activity:?}"
        );
    }

    /// AG-UI's envelope carries the operations that change a surface, and
    /// its middleware names no function call, so there is no place to put
    /// one: a batch that holds a call is refused, and nothing is painted.
    #[test]
    fn a_function_call_is_refused_and_not_dropped() {
        use ag_ui_a2ui::Error;
        use ag_ui_a2ui::adapters::agui::operations_envelope;
        use ag_ui_a2ui::message::{CallRendererFunction, FunctionCall};

        let version = A2uiVersion::V1_0;
        let spec = SurfaceSpec::new("cart").with_version(version);
        let mut batch = assemble_ops(Intent::Create, &spec);
        batch.push(AgentMessage::call_renderer_function(
            CallRendererFunction::new(
                "call-1",
                FunctionCall::new("getLocale").with_catalog_id(BASIC),
            ),
        ));

        let mut painter = SurfacePainter::new();
        let refused = painter.settle("call-1", &batch).unwrap_err();
        let Error::Validation { errors } = &refused else {
            panic!("a validation error, not {refused:?}");
        };
        let [error] = errors.as_slice() else {
            panic!("one error: {errors:?}");
        };
        assert_eq!(error.path, "messages[1].callRendererFunction");
        assert!(error.message.contains("function call"), "{}", error.message);
        // Nothing of the batch was taken.
        assert!(painter.surfaces().get("cart").is_none());
        assert!(operations_envelope(&batch).is_err());
        assert!(operations_envelope(&batch[..1]).is_ok());
    }

    #[test]
    fn a_creation_that_carries_its_surface_is_one_operation_in_a_tool_result() {
        let version = A2uiVersion::V1_0;
        let create = ag_ui_a2ui::message::CreateSurface::new("note")
            .with_catalog_id(BASIC)
            .with_components(vec![
                ag_ui_a2ui::Component::new("root", "Text").with("text", json!("Saved")),
            ]);
        let operations =
            [AgentMessage::new(AgentPayload::CreateSurface(create)).with_version(version)];

        let messages = conversation(&mut SurfacePainter::new(), &operations);
        let note = latest_surface(&messages).unwrap().unwrap();
        assert_eq!(note.spoken_version(), version);
        assert_eq!(note.components.len(), 1);
        // The frontend is shown a creation, the components after it, and the
        // data model, which holds nothing.
        let Message::Activity(activity) = &messages[1] else {
            panic!("the painter's activity follows the tool result");
        };
        assert_eq!(
            kinds(activity.content[A2UI_OPERATIONS_KEY].as_array().unwrap()),
            ["createSurface", "updateComponents", "updateDataModel"]
        );
    }
}

#[cfg(feature = "a2a")]
mod a2a {
    use super::*;
    use ag_ui_a2ui::AgentMessage;
    use ag_ui_a2ui::adapters::a2a;
    use ag_ui_a2ui::agent::{A2uiGenerator, CatalogConfig};

    #[test]
    fn an_agent_that_advertises_1_0_is_activated_in_1_0() {
        let version = A2uiVersion::V1_0;
        let generator = A2uiGenerator::new(vec![CatalogConfig::basic(version).unwrap()]);

        // On the agent card: one extension for each version the agent
        // speaks, with what the generator advertises.
        let advertised = generator.capabilities();
        let ids: Vec<&str> = advertised
            .supported_catalog_ids
            .iter()
            .map(String::as_str)
            .collect();
        let card = [
            a2a::agent_extension("0.9.1", false, &[]),
            a2a::agent_extension("1.0", advertised.accepts_inline_catalogs, &ids),
        ];
        assert_eq!(card[1]["uri"], "https://a2ui.org/a2a-extension/a2ui/v1.0");
        assert_eq!(card[1]["params"], json!({"supportedCatalogIds": [BASIC]}));
        let uris: Vec<&str> = card
            .iter()
            .map(|extension| extension["uri"].as_str().unwrap())
            .collect();

        // A client that asks for both is answered in the newer.
        let requested = [a2a::extension_uri("0.9.1"), a2a::extension_uri("1.0")];
        let (uri, activated) = a2a::activate(&requested, &uris).unwrap();
        assert_eq!((uri, activated), (uris[1], version));
        // One that asks only for the older is answered in it.
        let (_, older) = a2a::activate(&requested[..1], &uris).unwrap();
        assert_eq!(older, A2uiVersion::V0_9_1);
        // A version this crate does not speak activates nothing.
        let later = [a2a::extension_uri("2.0")];
        assert_eq!(a2a::activate(&later, &later), None);

        // The renderer's capabilities are read under the keys of v1.0.
        let message = json!({
            "metadata": {"a2uiRendererCapabilities": {"1.0": {"supportedCatalogIds": [BASIC]}}},
            "parts": [],
        });
        let capabilities = a2a::renderer_capabilities(&message, activated)
            .unwrap()
            .unwrap();
        let processor = generator.create_processor(Some(&capabilities)).unwrap();
        assert_eq!(processor.active_catalogs()[0].catalog_id(), BASIC);
        // Read as v0.9, the same message states no capabilities.
        assert_eq!(
            a2a::renderer_capabilities(&message, A2uiVersion::V0_9_1).unwrap(),
            None
        );

        // The answer: a data part for each v1.0 message.
        let message = AgentMessage::delete_surface("cart").with_version(activated);
        let part = a2a::data_part(&serde_json::to_value(&message).unwrap(), None);
        assert_eq!(part["metadata"]["mimeType"], a2a::MIME_TYPE);
        assert_eq!(part["data"]["version"], "1.0");
    }

    /// Over A2A a data part carries one A2UI message, whichever it is: the
    /// four of a function call travel as any other.
    #[test]
    fn a_data_part_carries_a_function_call() {
        use ag_ui_a2ui::agent::{AgentFunction, CatalogDocument, RendererMessageRouter, Routed};
        use ag_ui_a2ui::message::{CallRendererFunction, FunctionCall};

        // The agent's call to the renderer, as a part of its answer.
        let call = AgentMessage::call_renderer_function(CallRendererFunction::new(
            "call-1",
            FunctionCall::new("getLocale").with_catalog_id(BASIC),
        ));
        let part = a2a::data_part(&serde_json::to_value(&call).unwrap(), None);
        assert!(a2a::is_a2ui_part(&part));
        assert_eq!(part["metadata"]["mimeType"], a2a::MIME_TYPE);
        assert_eq!(
            part["data"]["callRendererFunction"]["functionCallId"],
            "call-1"
        );

        // The renderer's call to the agent, as a part of its message.
        let incoming = json!({"version": "1.0", "callAgentFunction": {
            "surfaceId": "checkout",
            "functionCallId": "fx-1",
            "callFunction": {"@call": "fetchFxRate", "catalogId": BASIC, "args": {}}
        }});
        let parts = [
            json!({"kind": "text", "text": "hello"}),
            a2a::data_part(&incoming, None),
        ];
        let [message] = a2a::a2ui_messages(&parts)[..] else {
            panic!("one A2UI message among the parts");
        };
        let catalog = CatalogDocument::basic(A2uiVersion::V1_0)
            .unwrap()
            .with_agent_functions([AgentFunction::new("fetchFxRate", json!({"type": "object"}))])
            .unwrap();
        let router = RendererMessageRouter::new(&[catalog]).unwrap();
        let Routed::Call(handed) = router.route(message).unwrap() else {
            panic!("a call to run");
        };
        // The answer goes back as a part of its own.
        let answer = a2a::data_part(
            &serde_json::to_value(handed.value(json!(1.08))).unwrap(),
            None,
        );
        assert_eq!(answer["data"]["agentFunctionResponse"]["value"], 1.08);
    }
}
