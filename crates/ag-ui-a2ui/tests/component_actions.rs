#[cfg(test)]
mod tests {
    use ag_ui_a2ui::component_action::{ActionEvent, ComponentAction as Action};
    use ag_ui_a2ui::constants::BASIC_CATALOG_ID;
    use ag_ui_a2ui::message::{
        AgentMessage as Operation, AgentPayload as OperationKind, Component, DeleteSurface,
        FunctionCall,
    };
    use serde::Serialize;
    use serde_json::json;
    use serde_json::{Map, Value};

    fn v<T: Serialize>(value: &T) -> Value {
        serde_json::to_value(value).unwrap_or(Value::Null)
    }

    #[test]
    fn create_surface_matches_spec() {
        // createSurface 예시 형태.
        let op = Operation::create_surface("product-list", BASIC_CATALOG_ID);
        assert_eq!(
            v(&op),
            json!({
                "version": "v0.9",
                "createSurface": {
                    "surfaceId": "product-list",
                    "catalogId": "https://a2ui.org/specification/v0_9/basic_catalog.json"
                }
            })
        );
    }

    #[test]
    fn basic_catalog_id_is_the_renderer_constant() {
        // 정확 문자열 비교 대상. 스펙 사이트 쪽 문자열과 혼동 금지.
        assert_eq!(
            BASIC_CATALOG_ID,
            "https://a2ui.org/specification/v0_9/basic_catalog.json"
        );
        assert_ne!(
            BASIC_CATALOG_ID,
            "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json"
        );
    }

    #[test]
    fn update_components_matches_spec() {
        let op = Operation::update_components(
            "s1",
            vec![
                Component::new("root", "Card").with("child", serde_json::json!("body")),
                Component::new("body", "Text").with("text", serde_json::json!("안녕")),
            ],
        );
        assert_eq!(
            v(&op),
            json!({
                "version": "v0.9",
                "updateComponents": {
                    "surfaceId": "s1",
                    "components": [
                        {"id": "root", "component": "Card", "child": "body"},
                        {"id": "body", "component": "Text", "text": "안녕"}
                    ]
                }
            })
        );
    }

    #[test]
    fn update_data_model_replaces_whole_model() {
        let op = Operation::update_data_model("s1", "/", json!({"summary": "x"}));
        assert_eq!(
            v(&op),
            json!({
                "version": "v0.9",
                "updateDataModel": {
                    "surfaceId": "s1",
                    "path": "/",
                    "value": {"summary": "x"}
                }
            })
        );
    }

    #[test]
    fn delete_surface_matches_spec() {
        let op = Operation::new(OperationKind::DeleteSurface(DeleteSurface {
            surface_id: "s1".to_owned(),
        }));
        assert_eq!(
            v(&op),
            json!({"version": "v0.9", "deleteSurface": {"surfaceId": "s1"}})
        );
    }

    #[test]
    fn button_action_event_is_an_object() {
        // event 는 반드시 객체. {"event":"name"} 은 틀림.
        let button = Component::new("btn", "Button")
            .with("child", serde_json::json!("btn-label"))
            .with("action", serde_json::json!(Action::event("spike_approve")));
        assert_eq!(
            v(&button),
            json!({
                "id": "btn",
                "component": "Button",
                "child": "btn-label",
                "action": {"event": {"name": "spike_approve"}}
            })
        );
    }

    #[test]
    fn data_binding_uses_path_object() {
        // 절대 경로는 데이터 모델 루트 기준.
        let text =
            Component::new("summary", "Text").with("text", serde_json::json!({"path": "/summary"}));
        assert_eq!(
            v(&text),
            json!({"id": "summary", "component": "Text", "text": {"path": "/summary"}})
        );
    }

    #[test]
    fn operation_round_trips() {
        // flatten + 외부 태깅 enum 의 역직렬화까지 동작해야 한다.
        let op = Operation::create_surface("s1", BASIC_CATALOG_ID);
        let wire = serde_json::to_string(&op).unwrap_or_default();
        assert_eq!(serde_json::from_str::<Operation>(&wire).ok(), Some(op));
    }

    #[test]
    fn operation_field_order_puts_version_first() {
        // 와이어 바이트는 to_string 순서를 따른다 (to_value 는 BTreeMap 이라 정렬됨).
        let op = Operation::create_surface("s1", "cat");
        let wire = serde_json::to_string(&op).unwrap_or_default();
        assert!(
            wire.starts_with(r#"{"version":"v0.9","createSurface":"#),
            "{wire}"
        );
    }
    #[test]
    fn component_event_preserves_bindings_and_rejects_scalar_event_payloads() {
        let event = Action::Event(ActionEvent {
            name: "submit".into(),
            context: Map::from_iter([("answer".into(), json!({"path":"/answer"}))]),
        });
        assert_eq!(
            v(&event),
            json!({"event":{"name":"submit","context":{"answer":{"path":"/answer"}}}})
        );
        assert!(serde_json::from_value::<Action>(json!({"event":"submit"})).is_err());
        assert_eq!(serde_json::from_value::<Action>(v(&event)).unwrap(), event);
    }
    #[test]
    fn component_function_actions_use_the_standard_function_schema() {
        let action = Action::FunctionCall(FunctionCall {
            call: "openUrl".into(),
            args: Some(Map::from_iter([(
                "url".into(),
                json!("https://example.com"),
            )])),
            catalog_id: None,
            return_type: None,
        });
        assert_eq!(
            v(&action),
            json!({"functionCall":{"call":"openUrl","args":{"url":"https://example.com"}}})
        );
        assert_eq!(
            serde_json::from_value::<Action>(v(&action)).unwrap(),
            action
        );
    }
}
