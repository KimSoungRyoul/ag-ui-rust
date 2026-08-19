---
title: 개요
description: A2UI가 무엇인지, 왜 AG-UI와 별개의 protocol인지, surface가 AG-UI run에 어떻게 실려 가는지, 이 crate가 명세의 어느 revision을 말하는지.
---

[A2UI](https://a2ui.org)는 선언적인 agent 주도 UI protocol입니다. agent가 *surface*를
기술하는 JSON을 stream합니다. surface는 component의 flat list와 그것들이 bind하는
data입니다. renderer는 그것을 그립니다. `ag-ui-a2ui`는 그 교환의 **agent 쪽 절반**입니다.

이 crate는 pixel을 그리지 않습니다. tree를 배치하지도, runtime에 UI를 평가하지도
않습니다. A2UI를 만들고 검증하고 transport용으로 감쌀 뿐입니다. rendering은 완전히 다른
program입니다. widget toolkit과 event loop, reactive data model이 있어야 합니다. 그리고
wire 반대편에 있습니다.

```rust
use ag_ui_a2ui::{Catalog, Component, Validator};
use serde_json::json;

let catalog = Catalog::basic();
let components = vec![
    Component::new("root", "Column").with("children", json!(["title", "count"])),
    Component::new("title", "Text").with("text", json!("Your cart")),
    Component::new("count", "Text").with("text", json!({"path": "/items"})),
];

let report = Validator::new(&catalog).validate_surface(&components, Some(&json!({"items": 2})));
assert!(report.is_valid());
```

## A2UI는 별개의 protocol입니다

A2UI는 AG-UI의 일부가 아닙니다. 자체 명세와 자체 version 번호를 가집니다. 다른 언어로 된
자체 toolkit도 있습니다. `ag-ui-a2ui`가 이 workspace에 있는 이유는 하나입니다. 사용자 앞에
form을 띄우려는 AG-UI agent에게 필요하기 때문입니다. AG-UI가 A2UI를 정의해서가 아닙니다.
그 분리는 말이 아니라 code로 강제됩니다. `ag_ui_a2ui::agui` 밖에는 AG-UI를 아는 code가
없습니다.

그 경계를 긋는 것은 Cargo feature 두 개입니다:

| feature | 기본값 | 무엇인가 |
| --- | --- | --- |
| `toolkit` | on | agent 쪽 authoring: operation builder, catalog negotiation, prompt assembly, stream parsing, recovery loop. |
| `ag-ui` | on | `ag-ui`와의 interop: AG-UI message에서 만드는 history entry, offer 가능한 tool로 바뀌는 toolkit tool 정의. `toolkit`을 함의합니다. |

`ag-ui`를 끄면 `ag-ui` dependency도 함께 사라집니다:

```toml
[dependencies.ag-ui-a2ui]
version = "0.1"
default-features = false
features = ["toolkit"]
```

남는 것은 A2A나 MCP 위에서 구동하는 crate입니다. 이 crate가 만드는 envelope는 평범한 JSON
string입니다. media type을 요구하는 transport를 위해 `ag_ui_a2ui::constants::MIME_TYPE`가
`application/a2ui+json`으로 있습니다. 전체 matrix는
[feature flag](/ag-ui-rust/ko/reference/features/)를 보십시오.

## component model은 flat list입니다

component는 flat adjacency list로 전송됩니다. 부모와 자식은 중첩이 아니라 **id reference**로
이어집니다. `Card`는 자식을 id로 지목하고, `Column`은 id array를 담습니다. renderer는 모든
component를 map에 담아 두었다가 render 시점에 tree를 다시 세웁니다.

그 indirection이 protocol을 streaming 가능하게 만듭니다. agent는 component를 어떤 순서로든
정의할 수 있습니다. renderer는 id가 `root`인 component가 도착하는 즉시 그리기 시작합니다.
명세가 그 id를 못 박습니다. component list 중 하나에 `id: "root"`인 component가 반드시 하나
있어야 합니다. 그것이 [validator](/ag-ui-rust/ko/a2ui/validation/)가 확인하는 모든 것의
기준점입니다.

message envelope 열 개가 이 모두를 실어 나릅니다. agent에서 여섯 개, 반대로 네 개입니다:

| 방향 | payload key |
| --- | --- |
| agent → renderer | `createSurface`, `updateComponents`, `updateDataModel`, `deleteSurface`, `callRendererFunction`, `agentFunctionResponse` |
| renderer → agent | `action`, `callAgentFunction`, `rendererFunctionResponse`, `error` |

각 message는 `version` discriminator와 payload key 정확히 하나를 담습니다.
`ag_ui_a2ui::message`가 그 열 개를 모두 옮겨 놓은 곳입니다. `AgentMessage`에는 authoring
agent가 가장 자주 보내는 네 개를 위한 constructor가 있습니다.

## surface가 AG-UI run에 실려 가는 방식

A2UI는 message가 renderer에 어떻게 닿는지 말하지 않습니다. 그래서 toolkit들이 스스로
합의해야 했습니다. 합의한 것은 JSON object 하나입니다. key는 `a2ui_operations`이고,
operation array를 담습니다. frontend는 그 key가 있는지만 보고 payload가 A2UI인지
판별합니다.

```rust
use ag_ui_a2ui::toolkit::ops::{Intent, SurfaceSpec, assemble_ops};
use ag_ui_a2ui::{Component, wrap_as_operations_envelope};
use serde_json::json;

let spec = SurfaceSpec::new("cart")
    .with_components(vec![Component::new("root", "Text").with("text", json!("Your cart"))]);

let envelope = wrap_as_operations_envelope(&assemble_ops(Intent::Create, &spec)).unwrap();
assert!(envelope.starts_with(r#"{"a2ui_operations":["#));
```

envelope는 JSON **string**입니다. 그래서 더 감싸지 않고 AG-UI tool result나 A2A data part,
MCP tool result에 그대로 들어갑니다. AG-UI 위에서는 `render_a2ui`라는 tool call이
운반자입니다. 그 result가 envelope입니다:

```rust
use ag_ui_a2ui::constants::RENDER_A2UI_TOOL_NAME;
use ag_ui_a2ui::toolkit::ops::{Intent, SurfaceSpec, assemble_ops};
use ag_ui_a2ui::{Component, wrap_as_operations_envelope};
use ag_ui::RunOutcome;
use ag_ui::serve::{Agent, Error, Result, RunContext};
use serde_json::json;

struct Merchant;

impl Agent for Merchant {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let spec = SurfaceSpec::new("cart").with_components(vec![
            Component::new("root", "Text").with("text", json!("Your cart")),
        ]);
        let envelope = wrap_as_operations_envelope(&assemble_ops(Intent::Create, &spec))
            .map_err(Error::agent)?;

        let mut call = ctx.tool_call(RENDER_A2UI_TOOL_NAME)?;
        call.args_json(&json!({ "surfaceId": "cart" }))?;
        call.result(envelope)?;

        ctx.say("Here is your cart.")?;
        Ok(RunOutcome::Success)
    }
}
```

`render_a2ui`는 agent가 스스로 답하는 call입니다. client가 offer한 적이 없습니다. client가
실행할 것이 없기 때문입니다. frontend는 tool을 실행하지 않고 그 result를 그립니다. 그래서
`ag_ui::serve`는 `RunAgentInput.tools`를 allow-list가 아니라 capability list로 취급합니다.
그 list에 없는 이름으로 call을 emit해도 형식이 올바른 stream입니다. ordering verifier는 그에
대해 아무 말도 하지 않습니다. protocol이 제약하는 것은 ordering이고, 검사되는 것도
그것입니다.

`e2e/tests/a2ui_surface.rs`가 이것을 정직하게 유지합니다. agent가 toolkit으로 surface를
만들어 tool result로 보냅니다. 진짜 `ag_ui::client`가 진짜 port로 그것을 받습니다. 반대편으로
나온 operation이 들어간 것과 같은지, 그리고 authoring 대상이었던 catalog로 여전히
검증되는지를 단언합니다.

:::note[실패는 빈 surface가 아닙니다]
생성이 실패하면 `wrap_error_envelope`가 `error` key를 담은 payload를 만듭니다.
`a2ui_operations` key는 **없습니다**. 의도한 것입니다. 그 key가 곧 content sniff이기
때문입니다. 빈 list와 함께 그 key를 실으면 실패한 생성과 rendering된 생성을 구별할 수 없게
됩니다. 나중에 thread를 재생해 사용자가 무엇을 보고 있는지 알아내는 history scan에게도
마찬가지입니다.
:::

## 이 crate는 v0.9를 말합니다

모든 message에 `"version": "v0.9"`가 찍힙니다.

A2UI 명세 자체는 이미 v1.0으로 갔습니다. 출시된 toolkit들은 아닙니다. TypeScript와 .NET,
Python 모두 wire에는 `v0.9`를 찍습니다. .NET의 constants file은 그 값들을 어긋나면 안 되는
언어 간 wire contract로 표시해 두었습니다. 지금 v1.0 wire 값을 구현하면 그 어느 것과도
interop되지 않습니다. 그래서 이 crate는 생태계가 실제로 말하는 것에 고정합니다. toolkit들이
옮겨 가면 v1.0은 feature 뒤에 들어갑니다.

이 고정은 장식이 아닙니다. `Validator`는 다른 version을 선언한 message를 `invalid_value`로
보고합니다. vendoring된 conformance suite의 v0.8 case는 이식하지 않고 건너뜁니다. skip
70건 중 63건이 그것입니다.

## crate의 생김새

| module | 담긴 것 |
| --- | --- |
| [`message`](/ag-ui-rust/api/ag_ui_a2ui/message/index.html) | protocol envelope 열 개와 `Component`, `ChildList`, data model update semantics. |
| [`catalog`](/ag-ui-rust/api/ag_ui_a2ui/catalog/index.html) | surface가 담을 수 있는 것. `Catalog::basic()`은 표준 18-component catalog입니다. `Catalog::from_schema`는 custom catalog를 parsing합니다. |
| [`validate`](/ag-ui-rust/api/ag_ui_a2ui/validate/index.html) | JSON Schema로 표현할 수 없는 의미 검사. 생성 model이 자주 틀리는 envelope 검사와 property type 검사도 함께. |
| [`binding`](/ag-ui-rust/api/ag_ui_a2ui/binding/index.html) | JSON Pointer 해석, template scope, `formatString` interpolation syntax. |
| [`constants`](/ag-ui-rust/api/ag_ui_a2ui/constants/index.html) | 언어를 가로지르는 wire 값: envelope key, protocol version, tool 이름 둘. |
| [`toolkit`](/ag-ui-rust/api/ag_ui_a2ui/toolkit/index.html) *(feature)* | "사용자가 UI를 요청했다"와 "유효한 A2UI가 wire에 올랐다" 사이의 모든 것. |
| [`agui`](/ag-ui-rust/api/ag_ui_a2ui/agui/index.html) *(feature)* | AG-UI glue. crate의 나머지는 AG-UI의 존재를 모릅니다. |

더 깊이 들어가는 page가 둘 있습니다. toolkit은
[surface 작성](/ag-ui-rust/ko/a2ui/authoring/)입니다. 무엇이 검사되고 conformance suite가
그에 대해 무엇을 말하는지는 [validation](/ag-ui-rust/ko/a2ui/validation/)입니다.
