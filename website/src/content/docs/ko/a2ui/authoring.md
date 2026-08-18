---
title: surface 작성
description: toolkit feature — operation 만들기, transport용으로 감싸기, model에 prompt 보내기, 돌아온 것 parsing하기, 그리고 검증-retry loop.
---

`toolkit` feature는 "사용자가 UI를 요청했다"와 "유효한 A2UI가 wire에 올랐다" 사이의 모든
것입니다. 기본이 on이고, authoring agent가 시간을 보내는 곳입니다.

여기 있는 것을 통째로 쓸 의무는 없습니다. 어떤 surface를 그릴지 아는 agent에게는
`assemble_ops`와 `wrap_as_operations_envelope`면 충분합니다. prompt와 parser, recovery
조각은 model에게 설계를 맡기는 agent를 위한 것입니다.

| module | 하는 일 |
| --- | --- |
| [`negotiate`](/ag-ui-rust/api/ag_ui_a2ui/toolkit/negotiate/index.html) | surface가 어떤 catalog를 말할지 renderer와 합의합니다. |
| [`ops`](/ag-ui-rust/api/ag_ui_a2ui/toolkit/ops/index.html) | operation stream을 만듭니다. update일 때는 `createSurface`를 건너뜁니다. |
| [`envelope`](/ag-ui-rust/api/ag_ui_a2ui/toolkit/envelope/index.html) | operation을 transport용으로 감싸거나 실패를 보고합니다. |
| [`prompt`](/ag-ui-rust/api/ag_ui_a2ui/toolkit/prompt/index.html) | catalog와 문맥, 현재 surface로 생성 model의 prompt를 조립합니다. |
| [`parser`](/ag-ui-rust/api/ag_ui_a2ui/toolkit/parser/index.html) | model 응답에서 A2UI block을 도로 끄집어냅니다. |
| [`streaming`](/ag-ui-rust/api/ag_ui_a2ui/toolkit/streaming/index.html) | 같은 일을 점진적으로 합니다. 생성 중인 surface도 rendering됩니다. |
| [`history`](/ag-ui-rust/api/ag_ui_a2ui/toolkit/history/index.html) | 이전에 rendering한 surface를 복원해 편집할 수 있게 합니다. |
| [`recovery`](/ag-ui-rust/api/ag_ui_a2ui/toolkit/recovery/index.html) | 검증하고, error를 되먹이고, retry합니다. 최대 세 번. |
| [`schema`](/ag-ui-rust/api/ag_ui_a2ui/toolkit/schema/index.html) | schema 문서를 보관합니다. model에 필요한 만큼으로 잘라냅니다. |
| [`tools`](/ag-ui-rust/api/ag_ui_a2ui/toolkit/tools/index.html) | `generate_a2ui`와 `render_a2ui` tool 정의. |

## operation stream 만들기

surface는 `SurfaceSpec`으로 기술합니다. `assemble_ops`가 그것을 renderer가 기대하는 순서의
operation으로 바꿉니다. surface를 만들고, component를 정의하고, data를 공급하는 순서입니다.

```rust
use ag_ui_a2ui::Component;
use ag_ui_a2ui::toolkit::ops::{Intent, SurfaceSpec, assemble_ops};
use serde_json::json;

let spec = SurfaceSpec::new("cart")
    .with_components(vec![
        Component::new("root", "Column").with("children", json!(["heading", "items"])),
        Component::new("heading", "Text")
            .with("text", json!({"path": "/title"}))
            .with("variant", json!("h2")),
        Component::new("items", "List")
            .with("children", json!({"componentId": "row", "path": "/items"})),
        Component::new("row", "Text").with(
            "text",
            json!({"call": "formatString", "args": {"value": "${@index(offset: 1)}. ${name}"}}),
        ),
    ])
    .with_data_model(json!({
        "title": "Your cart",
        "items": [{"name": "Espresso"}, {"name": "Croissant"}],
    }));

// createSurface, updateComponents, updateDataModel.
assert_eq!(assemble_ops(Intent::Create, &spec).len(), 3);
// 같지만 createSurface가 빠집니다.
assert_eq!(assemble_ops(Intent::Update, &spec).len(), 2);
```

그 spec에서 짚어 둘 것이 셋입니다.

`items`는 child list의 **template** 형태를 씁니다. id array가 아니라
`{"componentId": ..., "path": ...}`입니다. 이 형태는 `/items`의 원소마다 `row`를 하나씩
만듭니다. collection scope를 여는 것도 이것입니다. 그래서 `row` 안의 `${name}`은 data model의
root가 아니라 현재 원소를 기준으로 해석됩니다.

기본값은 wire 상수에서 옵니다. catalog를 명시하지 않은 spec은 `BASIC_CATALOG_ID`를 씁니다.
`SurfaceSpec::default()`로 만든 spec은 surface id `dynamic-surface`를 대상으로 합니다.

`Intent::Update`는 겉치레 구분이 아닙니다. `createSurface`는 `surfaceId`를 할당하고 그
surface의 수명 동안 catalog를 고정합니다. renderer가 이미 들고 있는 surface에 다시 보내면
명세상 error입니다. `Intent::from_wire`는 알아보지 못하는 값에 기본값을 주지 않고 `None`을
반환합니다. "update"를 잘못 추측하면 살아 있는 surface를 다시 만들어 버리기 때문입니다.

:::caution[data model 전체를 교체하면 사용자 입력이 사라집니다]
`SurfaceSpec::data_path`의 기본값은 `/`입니다. data model 전체를 교체한다는 뜻입니다.
renderer의 양방향 binding은 사용자 입력을 그 model에 바로 씁니다. 살아 있는 surface를
update할 때는 실제로 의도한 좁은 path를 가리키십시오.
:::

```rust
use ag_ui_a2ui::message::AgentPayload;
use ag_ui_a2ui::toolkit::ops::{Intent, SurfaceSpec, assemble_ops};
use serde_json::json;

let spec = SurfaceSpec::new("cart")
    .with_data_model(json!("Ada"))
    .with_data_path("/user/name");

let ops = assemble_ops(Intent::Update, &spec);
let AgentPayload::UpdateDataModel(payload) = &ops[0].payload else {
    panic!("expected updateDataModel");
};
assert_eq!(payload.path, "/user/name");
```

## transport용으로 감싸기

`wrap_as_operations_envelope`는 `{"a2ui_operations": [...]}` object를 JSON string으로
만듭니다. 그 string은 손대지 않은 채 AG-UI tool result나 A2A data part, MCP tool result에
들어갑니다.

```rust
use ag_ui_a2ui::toolkit::envelope::{is_operations_envelope, wrap_as_operations_envelope};
use ag_ui_a2ui::{AgentMessage, Component};
use serde_json::{Value, json};

let json = wrap_as_operations_envelope(&[
    AgentMessage::create_surface("cart", "basic"),
    AgentMessage::update_components(
        "cart",
        vec![Component::new("root", "Text").with("text", json!("Your cart"))],
    ),
])
.unwrap();

let value: Value = serde_json::from_str(&json).unwrap();
assert!(is_operations_envelope(&value));
assert_eq!(value["a2ui_operations"][0]["version"], "v0.9");
```

`operations_envelope`는 같은 것을 `Value`로 반환합니다. 더 큰 payload에 끼워 넣는 호출자를
위한 것입니다. `unwrap_operations_envelope`는 그것을 도로 읽습니다.

생성이 실패하면 `wrap_error_envelope`를 보내십시오. 그리고 그것이 무엇을 담지 *않는지*
보십시오:

```rust
use ag_ui_a2ui::toolkit::envelope::{is_operations_envelope, wrap_error_envelope};
use ag_ui_a2ui::validate::{ErrorCode, ValidationError};
use serde_json::Value;

let errors = vec![ValidationError::new(
    ErrorCode::NoRoot,
    "components",
    "No component has id 'root'.",
)];

let json = wrap_error_envelope("cart", "could not build the surface", &errors).unwrap();
let value: Value = serde_json::from_str(&json).unwrap();

assert_eq!(value["error"], "could not build the surface");
assert_eq!(value["code"], "VALIDATION_FAILED");
assert_eq!(value["details"][0]["code"], "no_root");
// 없는 key가 핵심입니다. 실패가 frontend의 sniff에 답하면 안 됩니다.
assert!(!is_operations_envelope(&value));
```

빈 operation envelope는 쓸모없는 정도가 아니라 해롭습니다. 소비자는 `a2ui_operations` key로
payload가 A2UI인지 판단합니다. 실패가 그 key를 달고 오면 frontend의 대기 상태를 지웁니다.
게다가 나중에 history scan이 그것을 화면에 뜬 적 없는 surface로 재생합니다.

## model에 prompt 보내기

`PromptSpec`은 생성 model에 필요한 것을 모읍니다. `build_subagent_prompt`가 그것을
rendering합니다. 역할, 요청, workflow 규칙, catalog, 선택적인 few-shot 예제, 지금까지의 대화,
현재 화면의 surface, 응답 형식입니다.

기본으로 prompt는 `catalog.render_summary()`를 싣습니다. component type과 property를 압축한
설명입니다. `PromptSpec::with_schemas`로 `SchemaBundle`을 붙이면 정확한 JSON Schema 문서가
대신 실립니다. 정밀하지만 token이 훨씬 많이 듭니다. model에 catalog의 일부만 필요하면 bundle을
먼저 잘라내십시오.

기본 규칙은 `GENERATION_GUIDELINES`에서 옵니다. 이 crate의 validator가 무엇을 거부하는지에
맞춰져 있습니다. 모든 component에 고유한 `id`가 있어야 하고, 그중 하나는 `root`여야 합니다.
reference cycle이 없어야 합니다. binding은 함께 보내는 data를 가리켜야 합니다. 상대 path는
list template 안에서만 됩니다. 자체 style이 있는 application은 `PromptSpec::workflow_rules`로
규칙을 교체합니다.

## 이미 화면에 있는 것 편집하기

update는 agent가 무엇을 update하는지 알아야 쓸모가 있습니다. 그런데 agent는 run 사이에
아무것도 저장하지 않습니다. rendering한 surface는 transport를 타고 나갔고, renderer가 들고
있습니다. 남은 것은 대화입니다. `find_prior_surface_in`은 AG-UI thread에 있는 A2UI operation을
재생해서 그것들이 만든 surface를 알려 줍니다.

```rust
use ag_ui_a2ui::toolkit::ops::Intent;
use ag_ui_a2ui::toolkit::prompt::{PromptSpec, build_subagent_prompt};
use ag_ui_a2ui::{
    AgentMessage, Catalog, Component, find_prior_surface_in, wrap_as_operations_envelope,
};
use ag_ui_core::Message;
use serde_json::json;

let rendered = wrap_as_operations_envelope(&[
    AgentMessage::create_surface("cart", "basic"),
    AgentMessage::update_components(
        "cart",
        vec![Component::new("root", "Text").with("text", json!("Your cart"))],
    ),
])
.unwrap();

let thread = [
    Message::user("m-1", "show me my cart"),
    Message::tool("m-2", "call-1", rendered),
    Message::user("m-3", "add a checkout button"),
];

let prior = find_prior_surface_in(&thread).expect("the thread rendered a surface");
assert_eq!(prior.surface_id, "cart");
assert_eq!(prior.catalog_id.as_deref(), Some("basic"));

let catalog = Catalog::basic();
let spec = PromptSpec::new("You generate UI surfaces.", "add a checkout button", &catalog)
    .updating(&prior);

// `updating`은 intent를 바꾸고 기존 surface를 대상으로 삼습니다.
assert_eq!(spec.intent, Intent::Update);
assert!(build_subagent_prompt(&spec).contains("Your cart"));
```

scan은 encoding 두 가지를 알아봅니다. 실제로 둘 다 나오기 때문입니다. `a2ui_operations`
transport envelope와, assistant turn 안의 날 `<a2ui-json>` block입니다. surface를 고를 때는
최신부터 거슬러 올라갑니다. 그다음 정방향으로 재생합니다. 그래서 update는 사용자가 실제로
보고 있는 것을 대상으로 삼습니다. surface가 여러 개 살아 있으면 `find_prior_surface_by_id`로
`surfaceId` 하나에 한정합니다.

`ag-ui` feature가 없어도 같은 scan을 쓸 수 있습니다. `toolkit::history::find_prior_surface`이고,
crate 자체의 `HistoryMessage` type 위에서 동작합니다. AG-UI version은 그 위에 얹힌 `From`
구현과 한 줄짜리 wrapper입니다.

## model이 돌려준 것 parsing하기

A2UI가 structured output이 아니라 prompt로 생성되면, model은 산문을 돌려줍니다. A2UI는
`<a2ui-json>` tag 안에 있습니다. `parse_response`가 그것을 순서 있는 part로 나눕니다.

```rust
use ag_ui_a2ui::toolkit::parser::parse_response;

let response = r#"Here is your cart. <a2ui-json>[
    {"version": "v0.9", "createSurface": {"surfaceId": "cart", "catalogId": "basic"}}
]</a2ui-json>"#;

let parts = parse_response(response).unwrap();
assert_eq!(parts[0].text, "Here is your cart.");
assert!(parts[0].is_final);
assert_eq!(parts[0].a2ui.as_ref().unwrap().len(), 1);
```

이것은 string 분할이 아니라 scanner입니다. 그래야만 합니다. 닫는 tag는 JSON string literal
안에 정당하게 나올 수 있습니다. 내용에서 `</a2ui-json>`를 언급하는 `Text` component도 유효한
A2UI입니다. 그래서 scanner는 string 상태와 escape를 추적합니다.

`parse_and_fix`는 수리를 두 가지 합니다. 그것도 그냥 parsing이 실패한 뒤에만 합니다. smart
quote 정규화와 trailing comma 제거입니다. 둘 다 유효한 JSON의 의미를 바꿀 수 없어
안전합니다. 홀로 있는 object는 array로 감쌉니다. A2UI payload는 message의 list이기
때문입니다.

## recovery loop

prompt로 하는 생성은 schema로 제약되지 않습니다. 그래서 model이 이따금 앞뒤가 맞지 않는
surface를 돌려줍니다. 복구할 수 있습니다. validator가 무엇이 잘못됐는지 정확히 알려 주기
때문입니다. 그 문장은 model에 그대로 되돌려 주도록 쓰였습니다. `generate_with_recovery`는
검증 → 설명 → retry를 반복합니다. `MAX_A2UI_ATTEMPTS`, 즉 3회까지입니다.

```rust
use ag_ui_a2ui::catalog::Catalog;
use ag_ui_a2ui::toolkit::recovery::{RecoveryOptions, RecoveryStatus, generate_with_recovery};

fn response(components: &str) -> String {
    format!(
        r#"<a2ui-json>[
             {{"version":"v0.9","createSurface":{{"surfaceId":"cart","catalogId":"basic"}}}},
             {{"version":"v0.9","updateComponents":{{"surfaceId":"cart","components":{components}}}}}
           ]</a2ui-json>"#
    )
}

let catalog = Catalog::basic();
let mut statuses = Vec::new();
let mut attempt = 0;

let surface = generate_with_recovery(
    "build a cart summary",
    &catalog,
    &RecoveryOptions::default(),
    |prompt, _n| {
        attempt += 1;
        Ok(if attempt == 1 {
            assert!(!prompt.contains("Correction required"));
            // 정의한 적 없는 component를 참조합니다.
            response(r#"[{"id":"root","component":"Card","child":"missing"}]"#)
        } else {
            // retry prompt에 이제 validator의 지적이 실려 있습니다.
            assert!(prompt.contains("unresolved_child"));
            response(r#"[{"id":"root","component":"Text","text":"Your cart"}]"#)
        })
    },
    |activity| statuses.push(activity.status),
)
.unwrap();

assert_eq!(surface.attempts, 2);
assert_eq!(surface.components.len(), 1);
assert_eq!(
    statuses,
    vec![
        RecoveryStatus::Started,
        RecoveryStatus::Retrying,
        RecoveryStatus::Started,
        RecoveryStatus::Succeeded,
    ]
);
```

이 loop는 **동기**이고 model을 closure로 받습니다. 그래서 async runtime을 강요하지 않습니다.
blocking 호출을 그대로 감싸도 되고, host가 이미 쓰는 executor로 async client를 구동해도
됩니다. 모든 단계는 `on_activity`로 보고됩니다. activity type은 `a2ui_recovery`이고,
`RecoveryActivity::activity_type`이 그 상수입니다. 호출자는 그것으로 분기해서 멈춘 것처럼
보이는 대신 진행 상황을 보여 줄 수 있습니다. 그 보고로 무엇을 할지는 호출자가 정합니다.
toolkit 자체는 아무것도 emit하지 않습니다.

모든 시도가 실패하면 error는 `Error::RecoveryExhausted`입니다. 시도 횟수와 마지막 error
목록을 담습니다. `RecoveryOptions::for_update()`는 완화된
[validation contract](/ag-ui-rust/ko/a2ui/validation/)로 바꿔 끼웁니다. 기존 surface를 편집할 때 쓰는
것입니다.

## 대신 streaming하기

recovery loop는 생성 전체를 기다립니다. 그동안 사용자에게는 아무것도 닿지 않습니다. 지연을
내주고 검증-retry 안전망을 사는 셈입니다. `StreamParser`는 반대쪽 거래를 합니다. chunk를 밀어
넣으면, 그릴 만큼 tree가 도착하는 즉시 rendering 가능한 A2UI를 emit합니다.

```rust
use ag_ui_a2ui::catalog::Catalog;
use ag_ui_a2ui::toolkit::streaming::StreamParser;

let mut parser = StreamParser::new(Catalog::basic());

// 대화 text는 즉시 나옵니다.
let parts = parser.process_chunk("Building that now. <a2ui-json>[").unwrap();
assert_eq!(parts[0].text, "Building that now. ");

// message는 array 중간이라도 닫히는 순간 emit됩니다.
let parts = parser
    .process_chunk(r#"{"version":"v0.9","createSurface":{"surfaceId":"cart","catalogId":"basic"}},"#)
    .unwrap();
assert_eq!(parts[0].a2ui.as_ref().unwrap().len(), 1);
assert_eq!(parser.surface_id(), Some("cart"));
```

mechanism 넷이 이 일을 합니다. 이것이 한 byte씩 먹이는 JSON parser와 다른 이유입니다:

- **잘린 token 치유.** chunk 경계는 string 한가운데에 떨어질 수 있습니다. parser는 열린
  중괄호와 대괄호를 닫아 조각을 parsing 가능하게 만듭니다. 하지만 열린 *string*은 잘라도 되는
  key에 대해서만 닫습니다. `text`, `label`, `hint`, 그리고 `DEFAULT_CUTTABLE_KEYS`의 나머지
  넷입니다. `"id"`나 `"path"`를 일찍 닫으면 model이 쓴 적 없는 식별자나 binding을 지어내게
  됩니다. 그런 조각은 다음 chunk를 기다립니다.
- **placeholder 합성.** 부모는 대개 자식보다 먼저 도착합니다. 그래서 자식 reference를
  `loading_<id>`로 고쳐 쓰고 대역 component를 함께 emit합니다. renderer는 tree를 즉시
  배치하고, 진짜 component가 도착하면 바꿔 끼웁니다.
- **reachability 필터링.** `root`에서 닿는 component만 emit합니다. 부모보다 먼저 온
  component는 붙을 곳이 없습니다. 보내지 않고 cache해 두었다가 root에서 오는 path가 생기면
  보냅니다.
- **관문이 아니라 filter로서의 검증.** 부분 조각도 검증합니다. 아직 성립하지 않으면 조용히
  버립니다. 어떤 추가 입력으로도 고칠 수 없는 실패만 error입니다. reference cycle, 어느
  envelope에도 맞지 않는 message 같은 것입니다.

parser instance는 생성 하나의 상태를 들고 있습니다. 어떤 surface가 있는지, 어떤 component를
보았고 emit했는지, 지금까지의 data model입니다. 생성마다 새로 만드십시오.

정확성이 지연보다 중요하면 batch loop를 고르십시오. 사용자가 지켜보고 있으면 stream
parser입니다.

## catalog negotiation

`createSurface`는 surface의 catalog를 수명 동안 고정합니다. 그래서 선택은 model에 prompt를
보내기 전에 끝나야 합니다. `select_catalog`가 그 negotiation을 합니다. renderer의 선호 순서가
이깁니다. 결과를 그리는 쪽이 renderer이기 때문입니다.

```rust
use ag_ui_a2ui::constants::BASIC_CATALOG_ID;
use ag_ui_a2ui::toolkit::negotiate::{ClientCapabilities, select_catalog};
use serde_json::json;

let known = vec![
    json!({"catalogId": "https://example.com/design-system.json", "components": {}}),
    json!({"catalogId": BASIC_CATALOG_ID, "components": {}}),
];
let renderer = ClientCapabilities::supporting([BASIC_CATALOG_ID]);

let chosen = select_catalog(&known, &renderer, false).unwrap();
assert_eq!(chosen.catalog_id, BASIC_CATALOG_ID);
```

renderer는 자기만 가진 component를 위해 inline catalog 문서를 함께 보낼 수 있습니다.
`accepts_inline`을 넘기면 그 component들이 선택 결과에 병합됩니다. 선택된 catalog의
`catalogId`는 그대로입니다. 그 id가 양쪽이 합의한 것이고 wire에 실리는 것이기 때문입니다.
`CatalogRegistry`는 agent가 아는 catalog를 application용 이름으로 보관하고 wire id를 알려
줍니다.

## tool 정의 두 개

toolkit은 tool 두 개를 노출합니다. 둘은 층위가 다릅니다:

- `generate_a2ui`는 **planner를 향합니다**. orchestration model이 "이것을 새 surface로, 또는
  저 surface에 대한 편집으로 rendering하라"고 말할 때 호출합니다. 인자는 component가 아니라
  intent와 설명입니다.
- `render_a2ui`는 **안쪽의 structured output** tool입니다. 생성 model이 실제 surface를 emit할
  때 호출합니다. flat component list와 data model입니다.

둘을 떼어 놓으면 planner는 component catalog를 몰라도 됩니다. planner는 원하는 것을 기술하고,
안쪽 호출이 그것을 만듭니다.

```rust
use ag_ui_a2ui::Catalog;
use ag_ui_a2ui::toolkit::tools::{generate_a2ui_tool, render_a2ui_tool};
use ag_ui_core::Tool;

let catalog = Catalog::basic();
let tools: Vec<Tool> = vec![
    generate_a2ui_tool().into(),
    render_a2ui_tool(Some(&catalog)).into(),
];

assert_eq!(tools[0].name, "generate_a2ui");
assert_eq!(tools[1].name, "render_a2ui");
assert_eq!(tools[1].parameters["type"], "object");
```

`ToolDefinition`은 provider 중립입니다. `name`, `description`, 그리고 JSON Schema object인
`parameters`로 이루어집니다. `to_anthropic_value()`는 Messages API 형태로 rendering합니다.
거기서는 schema key가 `input_schema`입니다. 위의 `From<ToolDefinition> for ag_ui_core::Tool`
구현은 AG-UI 형태이고, `ag-ui` feature가 필요합니다.

surface가 만들어지면, 내보낼 만한지 결정하는 것은
[validation](/ag-ui-rust/ko/a2ui/validation/)입니다.
