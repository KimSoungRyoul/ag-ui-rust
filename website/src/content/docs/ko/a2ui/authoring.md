---
title: 화면 작성
description: 비동기 모델로 A2UI를 생성·수정하고, 검증된 결과를 AG-UI로 전송하며 catalog를 합의하는 방법.
---

`A2uiAuthor`는 `author` feature로 사용합니다. `ctx.send_a2ui`도 필요하면 `ag-ui-server`를 켭니다.
Author는 버전·catalog·스키마 설정을 불변으로 공유하고, 각 요청은 독립적인 생성 상태를 가집니다.

## 비동기 모델로 생성하기

```rust
use ag_ui_a2ui::{A2uiAuthor, A2uiVersion, Result};
use serde_json::json;

async fn create_cart() -> Result<ag_ui_a2ui::ValidatedSurface> {
    let author = A2uiAuthor::basic(A2uiVersion::V0_9_1)?;
    author.create("cart", "Show the cart title")
        .generate(|prompt, attempt| async move {
            // Replace this deterministic fixture with model.complete(&prompt).await.
            if attempt == 1 {
                return Ok("invalid generated document".into());
            }
            assert!(prompt.contains("Correction required"));
            Ok(json!([
                {"version":"v0.9.1","createSurface":{
                    "surfaceId":"cart",
                    "catalogId":"https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json"
                }},
                {"version":"v0.9.1","updateComponents":{
                    "surfaceId":"cart",
                    "components":[{"id":"root","component":"Text","text":"Your cart"}]
                }}
            ]).to_string())
        }).await
}
```

예제의 결정적 응답 대신 기존 모델 클라이언트의 비동기 호출을 넣습니다. 콜백에는 소유한 `String` prompt와
1부터 시작하는 시도 번호가 전달됩니다. 응답은 JSON 배열, 메시지 하나, 또는 `<a2ui-json>` 태그 내부 JSON입니다.
잘못 생성된 문서는 검증 오류를 포함한 교정 prompt로 재시도합니다.
Provider 자체의 오류는 즉시 반환하며, `Error::generation`으로 원래 오류 source를 보존할 수 있습니다.
네트워크 재시도는 호출자가 결정합니다.

Executor나 필수 `Send` 조건을 강제하지 않습니다. Send 콜백·future는 AG-UI 서버 agent에서 사용할 수 있고,
local 콜백은 `Rc`를 캡처할 수 있습니다. 모델 핸들을 공유한다면 시도마다 핸들을 clone하여 `async move`로 넘깁니다.

## 기존 화면 수정하기

```rust
use ag_ui_a2ui::{A2uiAuthor, Result, ValidatedSurface};
use serde_json::json;

async fn edit_cart(author: &A2uiAuthor, prior: &ValidatedSurface) -> Result<ValidatedSurface> {
    author.edit(prior, "Set memo to null")
        .generate(|_prompt, _attempt| async {
            Ok(json!([{"version":"v0.9.1","updateDataModel":{
                "surfaceId":"cart","path":"/memo","value":null
            }}]).to_string())
        }).await
}
```

Edit은 기존 surface의 update만 허용합니다. 다른 ID·버전·catalog, create, delete는 거부합니다.
기존 컴포넌트와 theme도 현재 author의 스키마로 다시 검증합니다. 사용자의 다른 입력을 보존하려면 필요한 데이터 path만 수정합니다.
Prior는 관찰된 상태이므로 renderer의 현재 입력보다 오래됐을 수 있습니다. 이 API가 동시 편집을 동기화하지는 않습니다.

`ValidatedSurface`는 private 필드와 읽기 전용 getter를 제공합니다.
`operations()`는 이번에 전송할 batch이고, `history()`는 이전 batch를 포함한 전체 관찰 이력입니다.
이력을 저장한 뒤 복원할 때는 raw JSON 배열로 읽고 `author.validate_create(surface_id, &messages)`를 호출합니다.
검사하지 않은 JSON을 `ValidatedSurface`로 역직렬화할 수 없습니다.

## AG-UI로 전송하기

```rust
use ag_ui::server::{AgentState, Result, RunContext};
use ag_ui_a2ui::{ValidatedSurface, agui::A2uiRunContextExt};

fn send<S: AgentState>(ctx: &mut RunContext<S>, surface: &ValidatedSurface) -> Result<()> {
    ctx.send_a2ui(surface)
}
```

`render_a2ui` 도구 호출과 `a2ui_operations` 결과를 발행합니다. 성공 반환은 로컬 이벤트 enqueue를 확인하며,
renderer의 수신·반영을 확인한 것은 아닙니다. 전송 오류가 나도 앞선 이벤트가 나갔을 수 있으므로 전체 batch를 자동 재전송하지 마세요.
이 경로는 전체 생성·검증 후 전송하며, 부분 화면 전송과 생성 재시도를 한 API에 섞지 않습니다.

## 수동 작성과 이력 복원

`AgentMessage`·`SurfaceSpec`과 저수준 parser는 계속 사용할 수 있습니다.
`generate_with_recovery_async`도 소유한 prompt를 받는 비동기 콜백을 지원하지만,
사용자가 catalog·검증 옵션을 연결하며 전체 JSON Schema 검증을 보장하는 고수준 author와는 범위가 다릅니다.
부분 갱신을 복원하려면 `RecoveryOptions::prior`에 관찰한 `SurfaceStore`를 넣습니다.
갱신용 옵션만으로 없는 이력을 추정하지 않습니다.

`try_find_prior_surface`와 `try_find_prior_surface_by_id`는 잘못된 메시지·수명·pointer를 오류로 반환합니다.
선택적인 discovery helper는 복원 실패 시 `None`을 반환하므로, 부재와 실패를 구분해야 하는 곳에서는 fallible API를 사용합니다.
History 복원과 검증은 같은 surface 상태 갱신 코드를 사용합니다.

## Catalog 합의

```rust
use ag_ui_a2ui::toolkit::negotiate::{ClientCapabilitiesWire, select_catalog_schema};
use serde_json::json;

let wire = ClientCapabilitiesWire::from_json(&json!({
    "v0.9":{"supportedCatalogIds":["example.com:cards"]}
})).unwrap();
let chosen = select_catalog_schema(
    &[json!({"catalogId":"example.com:cards","components":{}})],
    &wire.v0_9,
    false,
).unwrap();
assert_eq!(chosen["catalogId"], "example.com:cards");
```

`from_json`은 `schema-validation` feature가 필요하며 공식 capabilities 스키마로 먼저 검증합니다.
Inline catalog를 받기로 한 경우 components·함수 배열·theme를 포함한 문서 전체를 선택하고,
클라이언트가 그 catalog의 ID를 광고해야 합니다. 기존 ID에 충돌하는 정의가 있으면 오류입니다.
빈 capabilities를 서버 기본 catalog로 조용히 바꾸지 않습니다.

Inline 생성은 `SchemaBundle::from_inline_catalog(selected)`로 전체 bundle을 만듭니다.
이 메서드는 wire catalog ID를 유지하면서 컴포넌트·함수 union을 준비합니다.
`A2uiAuthor::new`에 이 bundle과 참조하는 로컬 리소스를 전달합니다.
URL 모양의 `$ref`도 자동 조회하지 않습니다.
