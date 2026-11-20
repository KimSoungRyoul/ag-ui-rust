---
title: 버전과 catalog 선택
description: Agent session, AG-UI, A2A에서 A2UI wire 버전과 catalog를 선택합니다.
---

A2UI 버전은 AG-UI 프로토콜 버전 및 SDK 릴리스 번호와 별개입니다.
기본값은 `A2uiVersion::V0_9`입니다. `A2uiVersion::LATEST`는 `V1_0`을
가리킵니다. Renderer가 지원할 때 v1.0을 명시적으로 선택합니다.

| 항목 | v0.9 계열 | v1.0 계열 |
| --- | --- | --- |
| 메시지 `version` | `"v0.9"` 또는 `"v0.9.1"` | `"1.0"` |
| 동적 binding과 함수 참조 | `path`, `call` | `@path`, `@call` |
| Data model 삭제 | `value` 생략 | `value`에 `null` 지정 |
| Surface 생성 | Component와 data를 별도 메시지로 갱신. `theme` 선택 가능. | `components`와 `dataModel` 포함 가능. 기본 `catalogId`는 선택 사항이며 component마다 바꿀 수 있음. |
| Metadata의 capability | `a2uiClientCapabilities`, `v0.9` namespace | `a2uiRendererCapabilities`, `1.0` namespace |

Surface는 생성한 wire 계열을 유지합니다. v0.9.1 메시지로 v0.9 surface를 갱신할 수
있습니다. v1.0 메시지는 v1.0 surface에 사용합니다. 버전에 맞는 표기는
`version.path()`, `version.call()`, `version.component_keys()`로 얻습니다.
v1.0에서 동적 값의 맨 앞 `@`는 예약되어 있으므로 문자 자체는 `@@`로 적습니다.
Component는 `@catalogId`로 catalog를 선택할 수 있습니다. 여러 catalog를 사용하면
의미 검사를 적용하며 [JSON Schema 검증에 알려진 한계](/ag-ui-rust/ko/v1.0.0/a2ui/validation/#검증-범위)가 있습니다.

## 계층별 버전 정하기

1. 독립 agent session은 generator에 `CatalogConfig::basic(version)`을 등록합니다.
   한 session은 한 계열을 사용합니다. 메시지를 직접 만들 때는
   `Catalog::basic_for(version)`과 `SchemaBundle::for_version(version)`으로
   공개 catalog와 schema를 얻습니다.
2. AG-UI를 사용하면 frontend catalog에서 `CatalogContext::protocol_version()`을 읽습니다.
   해당 버전을 `CatalogContext::capabilities`, `render_a2ui_tool_for`,
   `SurfaceSpec::with_version`에 전달합니다. v0.9 frontend의 `BASIC_CATALOG_ID`와
   공개 `OFFICIAL_BASIC_CATALOG_ID`는 서로 다른 ID입니다. `adapters::agui::basic_catalog(version)`과
   `basic_catalog_id(version)`는 adapter가 쓰는 ID를 선택합니다.
3. A2A를 사용하면 `a2a`를 켜고 지원 버전마다 extension을 등록합니다.
   `activate`가 선택한 버전으로 응답 메시지를 작성합니다.

```rust
use ag_ui_a2ui::{A2uiVersion, adapters::a2a};

let old = a2a::extension_uri("0.9.1");
let new = a2a::extension_uri("1.0");
let extensions = [old, new.clone()];
let (uri, version) = a2a::activate(&extensions, &extensions).unwrap();
assert_eq!(uri, new);
assert_eq!(version, A2uiVersion::V1_0);
```

A2A의 `data_part`는 v1.0 [함수 호출](/ag-ui-rust/ko/v1.0.0/a2ui/functions/)을
포함해 A2UI 메시지를 JSON으로 전달합니다. AG-UI adapter는 surface operation을 전달합니다.

v0.9 계열에서도 alpha.3 이후에 SDK 동작이 바뀌었습니다. 없는 경로를 지우면
모델이 그대로 유지됩니다. `DirectJson` prompt 문자열을 고정해 검사하는 fixture도
갱신해야 합니다. Checkout을 옮길 때는
[migration guide](https://github.com/KimSoungRyoul/ag-ui-rust/blob/v1.0.0/docs/migration-0.5.md#changes-in-100)를 참고합니다.
