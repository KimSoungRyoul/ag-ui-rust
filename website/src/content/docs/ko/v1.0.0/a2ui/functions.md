---
title: A2UI 함수 호출
description: A2UI 1.0에서 양쪽 함수 호출을 분류하고 응답을 추적합니다.
---

A2UI 1.0에는 두 방향의 함수 호출 메시지가 있습니다. 앱은 handler를 실행하고
시간을 인자로 넘깁니다. SDK는 메시지를 분류하고 응답을 기다리는 호출을 추적합니다.

| 호출 방향 | 요청 | 응답 |
| --- | --- | --- |
| Renderer에서 agent로 | `callAgentFunction` | `agentFunctionResponse` |
| Agent에서 renderer로 | `callRendererFunction` | `rendererFunctionResponse` |

AG-UI adapter는 함수 호출 메시지가 있는 surface batch를 거부합니다. 이 흐름에는
[A2A data part](/ag-ui-rust/ko/v1.0.0/a2ui/versions/)처럼 A2UI 메시지를 그대로 전달하는
전송 경로를 사용합니다.

## Renderer의 호출에 응답하기

Agent가 보유한 v1.0 catalog에 `CatalogDocument::with_agent_functions`로 함수를
선언합니다. 선언은 prompt에 포함되며 router가 해당 호출을 인식하는 데 쓰입니다.
`A2uiRequestProcessor::renderer_messages()`를 호출하거나 활성 catalog에서 router를
구성합니다. `schema-validation` feature를 켠 경우 `checking_schemas()`를 호출하면
앱으로 전달하기 전에 인자를 검증하고 기본값을 채웁니다.

```rust
use ag_ui_a2ui::{AgentMessage, A2uiVersion};
use ag_ui_a2ui::agent::{AgentFunction, CatalogDocument, RendererMessageRouter, Routed};
use serde_json::json;

let catalog = CatalogDocument::basic(A2uiVersion::V1_0)?.with_agent_functions([
    AgentFunction::new("suggestTitle", json!({"type": "object"})).returning("string"),
])?;
let router = RendererMessageRouter::new(&[catalog.clone()])?;
let message = json!({"version": "1.0", "callAgentFunction": {
    "surfaceId": "planner",
    "functionCallId": "title-1",
    "callFunction": {"@call": "suggestTitle", "catalogId": catalog.catalog_id(), "args": {}}
}});
let reply: Option<AgentMessage> = match router.route(&message)? {
    Routed::Call(call) => Some(call.value(json!("Weeknight meals"))),
    Routed::Reply(reply) => Some(reply),
    Routed::FunctionResponse(_) | Routed::Action(_) | Routed::Error(_) => None,
    _ => None,
};
assert_eq!(serde_json::to_value(reply)?["agentFunctionResponse"]["functionCallId"], "title-1");
# Ok::<(), Box<dyn std::error::Error>>(())
```

반환된 응답을 renderer로 보냅니다. `Routed::FunctionResponse`는 발신 호출
테이블로 전달합니다. Action과 renderer 오류는 앱의 UI 이벤트 경로로 처리합니다.
호출을 실행할지와 어떤 값을 응답할지는 앱이 결정합니다.

## Renderer 함수 호출하기

Session의 `A2uiRequestProcessor::renderer_calls()`로 `RendererCalls`를 얻습니다.
Agent가 부르는 함수는 catalog ID가 있어야 하고, `agentOnly` 또는
`rendererOrAgent`를 허용해야 합니다. 사용자 조작을 요구하는 함수는 호출할 수 없습니다.
`RendererCalls::call`은 보낼 메시지를 만들고 응답 대기에 등록합니다.

```rust
use std::time::Duration;
use ag_ui_a2ui::agent::{RendererCall, RendererCalls};
use ag_ui_a2ui::message::FunctionResponse;
use serde_json::json;

// The example renderer already has the device catalog and implements getLocale.
let mut calls = RendererCalls::new().with_id_prefix("planner-");
let call = RendererCall::new("example.com:device", "getLocale");
let outbound = calls.call(call, Duration::from_secs(0))?;
let json = serde_json::to_value(outbound)?;
let id = json["callRendererFunction"]["functionCallId"].as_str().unwrap();
let response = FunctionResponse::value(id, json!("ko"));
let settled = calls.resolve(&response).unwrap();
assert_eq!(settled.outcome.value(), Some(&json!("ko")));
# Ok::<(), Box<dyn std::error::Error>>(())
```

대화마다 테이블 하나를 사용합니다. 여러 테이블을 쓰면 `with_id_prefix`로 서로 다른
접두사를 지정합니다. 앱이나 모델이 ID를 직접 만들면 중복되지 않게 관리합니다.
모델이 생성한 호출 메시지는 보내기 전에 각각 `track`에 넘깁니다. 응답은
`resolve`로 처리합니다. 일치하는 호출이 없으면 모델의 turn에 대한 응답일 수 있습니다.
기본 한도는 30초와 대기 호출 64개입니다. `next_deadline()`에 맞춰 앱이 작업을
예약하고 시간이 되면 `expire(now)`를 호출합니다. Binding, check, action 안의
catalog 함수 평가는 renderer가 맡습니다.
