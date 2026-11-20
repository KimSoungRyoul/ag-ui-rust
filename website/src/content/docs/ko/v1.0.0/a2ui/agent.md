---
title: Agent 계층 사용
description: 요청을 준비하고 모델 답변을 검증하고 A2UI stream을 처리합니다.
---

[정해진 surface 하나](/ag-ui-rust/ko/v1.0.0/a2ui/authoring/)를 작성하려면
`A2uiAuthor`를 사용합니다. 프레임워크가 모델 호출을 관리하거나 catalog 합의,
답변의 점진적 처리가 필요하면 기본 `agent` feature의 API를 사용합니다.

1. Catalog, 예제, 출력 형식을 지정해 `A2uiGenerator`를 한 번 구성합니다.
   `DirectJson`은 A2UI JSON을 읽습니다.
2. 버전에 맞는 renderer capability를 읽고 요청마다 `create_processor`를
   호출합니다. 앱의 prompt에 `A2uiRequestProcessor::prompt_snippet()`을
   넣습니다. 활성 catalog가 답변에서 쓸 수 있는 항목을 정합니다.
3. Renderer가 보유한 surface에 대한 `RendererState`를 관리합니다. 앱에서 모델을
   호출한 뒤 답변을 `RetryDriver::judge`에 전달합니다. `Done`이면 텍스트와 A2UI
   메시지를 보낼 수 있습니다. `Retry`의 피드백으로 모델에 다시 요청합니다.
   `GiveUp`이면 오류를 바탕으로 해당 turn을 마칩니다.

```rust
use ag_ui_a2ui::A2uiVersion;
use ag_ui_a2ui::agent::{A2uiGenerator, CatalogConfig, RendererCapabilities, ResponsePart, RetryDriver, Verdict};

let version = A2uiVersion::V1_0;
let generator = A2uiGenerator::new(vec![CatalogConfig::basic(version)?]);
let capabilities = RendererCapabilities::supporting(version, [version.basic_catalog_id()]);
let processor = generator.create_processor(Some(&capabilities))?;
let system = format!("You help users plan meals.\n\n{}", processor.prompt_snippet());
let mut state = processor.create_state();
let mut retry = RetryDriver::new(&processor, 3);
let request = "Plan three meals.";
let mut prompt = request.to_owned();
let mut model_replies = ["<a2ui-json>broken</a2ui-json>", "Here are three meal ideas."].into_iter();

let parts = loop {
    // In an application, call the model with system and prompt here.
    assert!(!system.is_empty());
    assert!(!prompt.is_empty());
    let answer = model_replies.next().expect("the fixture answers twice");
    match retry.judge(answer, &mut state) {
        Verdict::Done(parts) => break parts,
        Verdict::Retry(feedback) => prompt = feedback.prompt(request),
        Verdict::GiveUp(error) => return Err(error),
        _ => return Err(ag_ui_a2ui::Error::parse("unsupported retry verdict")),
    }
};
assert_eq!(parts, [ResponsePart::Text("Here are three meal ideas.".into())]);
# Ok::<(), ag_ui_a2ui::Error>(())
```

완결된 답변의 검증에 실패해도 기존 renderer 상태는 유지됩니다.
모델 클라이언트, 실행 시점, 재시도 횟수와 전송은 앱이 관리합니다.

## 답변을 stream으로 받기

현재 상태를 바탕으로 `ResponseStream`을 엽니다. `feed`가 내보낸
`StreamPart::A2ui` 메시지를 순서대로 보냅니다. 마지막 chunk 이후에도
`close`가 반환한 부분을 처리합니다. 이후 block이 거부되어도 이미 보낸
메시지는 renderer에서 적용되었을 수 있습니다.

```rust
use ag_ui_a2ui::A2uiVersion;
use ag_ui_a2ui::agent::{A2uiGenerator, CatalogConfig, RendererCapabilities, StreamOptions, StreamPart};

let version = A2uiVersion::V1_0;
let generator = A2uiGenerator::new(vec![CatalogConfig::basic(version)?]);
let capabilities = RendererCapabilities::supporting(version, [version.basic_catalog_id()]);
let processor = generator.create_processor(Some(&capabilities))?;
let mut state = processor.create_state();
let mut stream = processor.open_stream(&state, StreamOptions::default())?;
let catalog = A2uiVersion::V1_0.basic_catalog_id();
let opening = format!(
    r#"Here are three meal ideas.
<a2ui-json>[{{"version":"1.0","createSurface":{{"surfaceId":"meals","catalogId":"{catalog}","#,
);
let rest = r#""components":[{"id":"root","component":"Text","text":"Soup, curry, noodles"}]}}]</a2ui-json>"#;
let mut messages = Vec::new();
for chunk in [opening.as_str(), rest] {
    for part in stream.feed(chunk, &mut state)? {
        if let StreamPart::A2ui(batch) = part {
            messages.extend(batch);
        }
    }
}
for part in stream.close(&mut state)? {
    if let StreamPart::A2ui(batch) = part {
        messages.extend(batch);
    }
}
assert_eq!(messages[0].surface_id(), Some("meals"));
assert!(state.surface("meals").is_some());
# Ok::<(), ag_ui_a2ui::Error>(())
```

v1.0 stream의 component는 정의가 닫힐 때 나옵니다. 점진적 파싱 중
`updateDataModel`은 `path`가 확인되어야 먼저 나올 수 있습니다.
`path`를 생략하면 메시지가 닫힌 뒤에 나옵니다. 갱신을 점진적으로 보여줄 때는
모델이 `value`보다 `path`를 먼저 쓰도록 요청합니다.
