---
title: validation
description: 의미 validator가 surface의 형태 너머로 무엇을 확인하는지, 어떤 error code를 보고하는지, vendoring된 conformance suite가 그에 대해 무엇을 말하는지.
---

JSON Schema는 `children`이 string array라고 말할 수 있습니다. 그 이상은 못 합니다. string
하나하나가 실제로 있는 component를 가리키는지, tree에 root가 있는지, `a → b → a`가 renderer가
끝내 다 그리지 못할 cycle인지는 말하지 못합니다. 그것을
[`ag_ui_a2ui::validate`](/ag-ui-rust/api/ag_ui_a2ui/validate/index.html)가 확인합니다.
여기서 "의미"는 그 뜻입니다.

```rust
use ag_ui_a2ui::validate::{ErrorCode, Validator};
use ag_ui_a2ui::{Catalog, Component};
use serde_json::json;

let report = Validator::new(&Catalog::basic()).validate(&[
    Component::new("root", "Card").with("child", json!("greeting")),
]);

assert_eq!(report.errors[0].code, ErrorCode::UnresolvedChild);
assert_eq!(report.errors[0].path, "components[0].child");
```

모든 실패는 `ValidationError`입니다. 세 가지를 담습니다. 기계가 읽는 `code`, component list
안의 위치를 짚는 `path`, 그리고 model이 바로 조치할 수 있는 문장인 `message`입니다.
validator는 첫 error에서 멈추지 않고 *모든* error를 모읍니다. 그래서 retry 한 번으로 전부
고칠 수 있습니다. [recovery loop](/ag-ui-rust/ko/a2ui/authoring/)가 동작하는 이유입니다.

## surface가 어디서 왔는지가 검사 방식을 정합니다

[`Validator`](/ag-ui-rust/api/ag_ui_a2ui/validate/struct.Validator.html)에는 entry point가
다섯입니다. 무엇을 건네받는지가 다릅니다:

| method | 입력 | 추가로 검사하는 것 |
| --- | --- | --- |
| `validate` | `&[Component]` | — |
| `validate_surface` | `&[Component]`와 data model | binding이 실제 data에 대해 해석되는지. |
| `validate_json` | model에서 온 날 `&[Value]` | 빠진 `id`와 빠진 `component`. typed component로는 표현할 수 없는 것들입니다. |
| `validate_messages` | `&[AgentMessage]` | operation stream 전체를 접어서. |
| `validate_json_messages` | wire에서 온 날 `&[Value]` | message envelope, JSON 중첩 깊이, function call 연쇄 깊이. |

마지막 둘은 stream을 접습니다. 모든 `createSurface`와 `updateComponents`의 component를 한데
모읍니다. `updateDataModel` operation을 재생해 data model을 복원합니다. contract는 자동으로
고릅니다. `createSurface`가 없는 stream은 incremental update로 봅니다.

`validate_json_messages`의 검사 셋은 거기에만 있습니다. 셋 다 typed message로 deserialize하면
사라지기 때문입니다. 셋 모두 개별 component가 아니라 문서 전체의 성질입니다.

```rust
use ag_ui_a2ui::Catalog;
use ag_ui_a2ui::validate::{ErrorCode, Validator};
use serde_json::json;

let messages = vec![json!({
    "version": "v1.0",
    "updateComponents": {"surfaceId": "cart"}
})];

let report = Validator::new(&Catalog::basic()).validate_json_messages(&messages);
let codes: Vec<ErrorCode> = report.errors.iter().map(|e| e.code).collect();

// 이 crate는 v0.9를 말하고, 그렇게 밝힙니다.
assert!(codes.contains(&ErrorCode::InvalidValue));
// `updateComponents`에는 `components` array가 필요합니다.
assert!(codes.contains(&ErrorCode::MissingField));
```

다른 toolkit은 그 envelope 검사를 JSON Schema engine으로 `server_to_client.json`에서 얻습니다.
이 crate는 protocol version 하나만 말합니다. 그래서 contract는 payload type에서 옮겨 적은
표입니다. 덕분에 실패가 호출자가 실제로 보낸 message 안의 위치를 담습니다. 호출자가 본 적
없는 schema 안의 path가 아닙니다. code는 schema 기반 toolkit이 보고하는 것과 같습니다.
호출자가 그 code로 분기하기 때문입니다.

## 전체 contract와 완화된 contract

surface를 만드는 payload는 contract 전체를 지켜야 합니다. `root`가 있어야 하고, 모든 child
reference가 그 payload 안에서 해석되어야 합니다. incremental `updateComponents`는 다릅니다. 그
component는 renderer가 이미 들고 있는 id를 정당하게 참조할 수 있습니다. root를 넣을 필요도
없습니다.

```rust
use ag_ui_a2ui::validate::{ErrorCode, Validator};
use ag_ui_a2ui::{Catalog, Component};
use serde_json::json;

let catalog = Catalog::basic();
let patch = [Component::new("heading", "Text").with("text", json!("Updated"))];

// 전체 contract는 root를 요구합니다.
let strict = Validator::new(&catalog).validate(&patch);
assert!(strict.errors.iter().any(|e| e.code == ErrorCode::NoRoot));

// incremental update는 아닙니다. tree의 나머지는 renderer가 이미 들고 있습니다.
assert!(Validator::incremental(&catalog).validate(&patch).is_valid());
```

`ValidateOptions::incremental_update()`는 그 규칙 둘만 완화합니다. `require_root`와
`allow_dangling_children`입니다. 중복 id와 cycle은 여전히 실패합니다. 어느 쪽이든 깨진
것이기 때문입니다.

두 preset 사이의 모든 검사는
[`ValidateOptions`](/ag-ui-rust/api/ag_ui_a2ui/validate/struct.ValidateOptions.html)의 개별
switch입니다. `Validator::with_options`가 그것을 받습니다:

| option | 기본값 | 무엇을 관장하는가 |
| --- | --- | --- |
| `root_id` | `"root"` | tree root가 가져야 하는 id. |
| `require_root` | `true` | 그 id를 가진 component가 있어야 하는지. |
| `allow_dangling_children` | `false` | child reference가 이 payload 밖을 가리켜도 되는지. |
| `check_component_types` | `true` | component type이 catalog에 있어야 하는지. catalog가 아무것도 정의하지 않으면 자동으로 off입니다. catalog가 주어지지 않았다는 뜻이기 때문입니다. |
| `check_required_props` | `true` | catalog가 필수로 표시한 property를 강제하는지. |
| `check_prop_types` | `true` | 값이 catalog가 선언한 JSON type과 맞는지. |
| `check_envelope` | `true` | message가 v0.9 wire contract를 만족하는지. 날 message entry point에만 해당합니다. |
| `check_bindings` | `true` | binding이 해석되는지, relative path가 list template 안에 있는지. |
| `check_binding_syntax` | `true` | absolute path가 문법적으로 유효한 JSON Pointer인지. |
| `max_depth` | `50` | component graph와 날 JSON의 최대 중첩 깊이. |
| `max_function_call_depth` | `5` | 중첩된 function call의 최대 연쇄 길이. |

`check_binding_syntax`는 `check_bindings`와 분리되어 있습니다. data model이 필요 없고, 거짓
양성을 낼 수도 없기 때문입니다. 잘못된 escape는 data가 무엇이든 해석되지 않습니다.

## error code

[`ErrorCode`](/ag-ui-rust/api/ag_ui_a2ui/validate/enum.ErrorCode.html) variant는 열넷입니다.
이 집합은 일부러 닫아 두었습니다. 호출자가 이 code로 분기하기 때문입니다. recovery loop나
renderer의 error channel이 그렇습니다. 그래서 하나를 추가하는 것은 호환성을 깨는 변경입니다.

| code | 보고되는 경우 |
| --- | --- |
| `empty_components` | payload가 surface를 선언했는데 component가 없습니다. |
| `missing_id` | component에 쓸 수 있는 `id`가 없습니다. |
| `missing_component_type` | component에 쓸 수 있는 `component` type 이름이 없습니다. |
| `duplicate_id` | component 둘이 `id`를 공유합니다. |
| `no_root` | root id를 가진 component가 없습니다. renderer가 그리기 시작할 곳이 없습니다. |
| `unknown_component` | component의 type이 surface의 catalog에 없습니다. |
| `missing_required_prop` | catalog가 필수로 표시한 property가 빠졌습니다. |
| `missing_field` | *protocol*이 message envelope에 요구하는 field가 빠졌습니다. |
| `invalid_value` | 값의 형태는 맞지만 protocol이 허용하지 않습니다. 이 crate가 말하지 않는 revision을 가리키는 `version`이 그 예입니다. |
| `type_mismatch` | 값의 JSON type이 틀렸습니다. |
| `unresolved_child` | child reference가 없는 component id를 가리킵니다. |
| `child_cycle` | child reference를 따라가면 출발한 자리로 돌아옵니다. |
| `unresolved_binding` | data binding이 surface의 data model에 대해 해석되지 않습니다. |
| `max_depth_exceeded` | 중첩이 설정한 최대치보다 깊습니다. |

`missing_field`와 `missing_required_prop`은 비슷해 보이지만 다릅니다. 앞의 것은 wire format이
고정하고, catalog가 무엇이든 성립합니다. 뒤의 것은 어떤 *catalog*가 선언한 property의
문제입니다.

`max_depth_exceeded`는 중첩 세 종류를 아우릅니다. component graph, 날 JSON, 연쇄된 function
call입니다. 셋 다 model이 만든 것입니다. 상한이 없으면 어느 것도 끝없이 깊어질 수 있습니다.

## 도달할 수 없는 component는 warning입니다

있기는 하지만 root에서 닿지 않는 component는 `errors`가 아니라
[`ValidationReport::unreachable`](/ag-ui-rust/api/ag_ui_a2ui/validate/struct.ValidationReport.html)에
보고됩니다:

```rust
use ag_ui_a2ui::{Catalog, Component, Validator};
use serde_json::json;

let report = Validator::new(&Catalog::basic()).validate(&[
    Component::new("root", "Text").with("text", json!("Hello")),
    Component::new("stray", "Text").with("text", json!("Nobody points at me")),
]);

assert!(report.is_valid());
assert_eq!(report.unreachable, vec!["stray".to_string()]);
```

명세는 renderer에게 부모가 나타날 때까지 component를 buffering하라고 합니다. 그래서 도달할 수
없는 component는 대개 깨진 tree가 아니라 절반만 stream된 tree입니다. 그래도 생성 model에 알려
줄 값어치는 있습니다. 그래서 버리지 않고 따로 보고합니다.

## 결과를 model에 넘기기

`ValidationReport::into_result`는 report를 `Error::Validation`으로 바꿉니다. 거기 담긴
`ValidationErrors`는 error를 한 줄에 하나씩 rendering합니다. retry prompt가 원하는
형식입니다.

```rust
use ag_ui_a2ui::{Catalog, Component, Error, Validator};
use serde_json::json;

let report = Validator::new(&Catalog::basic()).validate(&[
    Component::new("root", "Card").with("child", json!("missing")),
]);

let Err(Error::Validation { errors }) = report.into_result() else {
    panic!("this surface does not validate");
};

assert!(errors.to_string().starts_with("[unresolved_child] components[0].child:"));
```

## "unknown"의 뜻은 catalog가 정합니다

검증은 언제나 [`Catalog`](/ag-ui-rust/api/ag_ui_a2ui/catalog/struct.Catalog.html)
기준입니다. `Catalog::basic()`은 표준 18-component catalog입니다. v0.9
`basic_catalog.json`에서 옮겨 적었습니다. vendoring된 명세 문서를 parsing해 비교하는 test가
그것을 정직하게 유지합니다. `Catalog::from_schema`는 어떤 A2UI catalog 문서든 parsing합니다.
custom design system은 그렇게 기술합니다.

```rust
use ag_ui_a2ui::validate::{ErrorCode, Validator};
use ag_ui_a2ui::{Catalog, Component};
use serde_json::json;

let catalog = Catalog::from_schema(&json!({
    "catalogId": "https://example.com/design-system.json",
    "components": {
        "Chart": {
            "type": "object",
            "properties": {
                "columns": {"type": "integer"},
                "series": {"type": "array"}
            },
            "required": ["series"]
        }
    }
}))
.unwrap();

let report = Validator::new(&catalog).validate(&[
    Component::new("root", "Chart")
        .with("columns", json!("three"))
        .with("series", json!([1, 2])),
    Component::new("legend", "Sparkline"),
]);

let codes: Vec<ErrorCode> = report.errors.iter().map(|e| e.code).collect();
assert!(codes.contains(&ErrorCode::TypeMismatch));      // "three"는 integer가 아닙니다
assert!(codes.contains(&ErrorCode::UnknownComponent));  // 이 catalog에 Sparkline은 없습니다
```

그 검사에서 두 가지는 보기보다 좁습니다.

**component를 잇는 것은 structural property뿐입니다.** 명세는 catalog가 child reference를 맨
string이 아니라 `ComponentId`나 `ChildList`로 typing하도록 요구합니다. validator는 정확히
그것으로 어떤 field가 link인지 판단합니다. 그냥 `"type": "string"`이면 static text입니다. URL이나
label 같은 것이고, 그 값은 component id로 해석되지 않습니다.

**property type은 옮겨 담을 뿐 해석하지 않습니다.** 각 property는 자기 schema가 못 박은 JSON
type을 그대로 지닙니다. JSON Schema에서 가져오는 제약은 그것 하나뿐입니다. `pattern`,
`minimum`, `additionalProperties` 같은 나머지는 문서 자체를 검증하는 쪽에 맡깁니다. schema가
type을 말하지 않거나 여러 개를 말하면 제약이 없고, 거부하지 않습니다. 느슨하게 읽은 catalog가
거짓 실패로 바뀌면 안 되기 때문입니다. renderer가 해석하는 값은 통째로 건너뜁니다.
`{"path": …}` binding이나 function call입니다. wire에서의 type은 그것이 갖게 될 type을 말해
주지 않습니다.

구성 제약은 `Catalog::composition_violations`가 검사합니다. `allowedParents`와
`allowedChildren`이고, 일부러 `validate` 밖에 두었습니다. 명세는 그것들에 renderer 쪽 code를
따로 줍니다. `UNALLOWED_PARENT`와 `UNALLOWED_CHILD`이고, 구조 관련 code와 구별됩니다. basic
catalog는 구성 제약을 하나도 선언하지 않습니다. custom catalog에서만 문제가 됩니다.

## binding

`check_bindings`는 data model이 주어졌을 때 binding을 그것에 대해 해석합니다. 보고하는
실패는 셋이고 모두 `unresolved_binding`입니다. data에 없는 path, array가 아닌 것을 가리키는
template path, 그리고 list template 밖에 있는 component의 relative path입니다. relative path는
collection scope 안에서만 의미가 있습니다.

`check_binding_syntax`는 data가 필요 없는 쪽입니다. JSON Pointer 안에서 `~`는 `~0`으로, `/`는
`~1`로 써야 합니다. key에 든 날 `~`는 없는 것이 아니라 잘못된 것입니다.

## 깊이는 안전장치가 아니라 정책입니다

component graph는 언제나 반복으로 순회합니다. 명시적인 worklist를 씁니다. cycle detection,
reachability, scope 할당 모두 그렇습니다. 취향의 문제가 아닙니다. graph는 model이 만든
것이고 깊이에 제한이 없습니다. recursion으로 순회하면 요청을 실패시키는 대신 process를 죽입니다.

그래서 `MAX_DEPTH`(50)와 `MAX_FUNCTION_CALL_DEPTH`(5)는 *정책*입니다. renderer가 무엇을
그릴지에 대한 정책이지, 이 crate를 버티게 하는 장치가 아닙니다. 여기서는 값을 올려도
안전합니다. 다른 A2UI toolkit이 강제하는 한계와 같은 값입니다. 그래서 그중 하나가 받는
payload는 여기서도 받습니다.

## conformance suite가 말하는 것

A2UI project는 conformance test를 언어 중립적인 YAML로 배포합니다. 모든 SDK에 그것을 돌리라고
합니다. YAML을 읽고, 입력을 그 언어의 구현에 먹이고, 출력을 단언하는 식입니다. suite는
`crates/ag-ui-a2ui/tests/conformance/` 아래에 vendoring되어 있습니다. upstream commit
`44a420b6` 시점의 것이고, `tests/conformance.rs`가 구동합니다.

실행하는 방법:

```sh
cargo test -p ag-ui-a2ui --all-features -- --nocapture
```

```text
core/validator.yaml              cases  45   checks: 26 passed, 25 skipped, 0 failed
core/catalog.yaml                cases  24   checks: 23 passed,  1 skipped, 0 failed
core/accessibility.yaml          cases   4   checks:  0 passed,  4 skipped, 0 failed
agent/parser.yaml                cases  19   checks: 19 passed,  0 skipped, 0 failed
agent/inference_format.yaml      cases  19   checks: 17 passed,  2 skipped, 0 failed
agent/streaming_parser.yaml      cases  76   checks: 38 passed, 38 skipped, 0 failed

TOTAL: 123 passed, 70 skipped, 0 failed
```

`steps`가 있는 case는 step마다 check 하나로 셉니다. 그래서 군데군데 check 합계가 case 수를
넘습니다.

skip은 모두 이유를 달고 집계됩니다. 조용히 무시하는 것은 없습니다. 이 crate가 실제로 기대한
결과를 내놓아야만 통과로 셉니다. 70건의 내역입니다:

| 개수 | 이유 |
| ---: | --- |
| 63 | **v0.8 wire format.** v0.8은 component property를 type 이름 아래에 중첩합니다. message 이름도 다릅니다. 이 crate는 component가 flat한 v0.9를 구현합니다. |
| 4 | **renderer accessibility.** accessibility tree와 axe-core 규칙은 renderer의 몫입니다. 이 crate는 rendering하지 않습니다. |
| 2 | **prompt 안의 v0.8 schema bundle.** `generate_prompt` case 둘이 v0.8 schema 문서를 끼워 넣기를 요구합니다. 나머지 prompt case 여섯은 실행됩니다. |
| 1 | **예제 파일의 JSON Schema 검증.** 한 case는 `load_examples`가 호출자가 준 schema로 예제를 검증하기를 요구합니다. JSON Schema engine이 통째로 필요합니다. upstream의 schema 이야기 중 이 crate가 재현하지 않는 유일한 부분입니다. |

version gating은 version이 문제 되는 곳에만 적용합니다. `validate`와 `process_chunk`는 wire
format 연산이라 gating합니다. schema를 손보는 작업은 아닙니다. prune, render, load, 엄격 검증
완화는 protocol이 아니라 문서의 형태를 다룹니다. 그래서 그 case들은 v0.8에서도 돕니다.

:::note[harness가 일부러 upstream과 다르게 가는 두 곳]
`test_validate_orphaned_component_v09`는 root에서 닿지 않는 component에 error를 기대합니다.
이 crate는 위의 이유로 그것을 warning으로 보고합니다. 그래서 harness는 그 조건이 치명적이라는
것이 아니라 *탐지된다는* 것을 단언합니다. 다른 심각도로 대응시킨 기대는 이것 하나뿐입니다.

`validate` case에서 harness는 upstream의 범위에 맞춥니다. component type 검사와 필수 property
검사는 off입니다. 비교 대상 case에서 upstream이 그것을 JSON Schema에 맡기기 때문입니다.
binding 해석도 off입니다. upstream의 구조 validator는 binding을 보지 않습니다. Pointer syntax와
envelope, property type 검사는 on입니다. upstream도 그것들은 검사하기 때문입니다. 범위를
맞춰야 비교에 의미가 생깁니다.
:::
