---
title: Use the agent layer
description: Prepare a request, validate a model answer, and consume streamed A2UI.
---

Use `A2uiAuthor` for [a single fixed surface](/ag-ui-rust/v1.0.0/a2ui/authoring/).
Use the default `agent` feature when a framework manages the model loop, needs
catalog negotiation, or handles model output as it arrives.

1. Configure `A2uiGenerator` once with catalogs, examples and the output format.
   `DirectJson` reads A2UI JSON.
2. Read the renderer's version-specific capabilities and call
   `create_processor` for a request. Use `A2uiRequestProcessor::prompt_snippet()`
   in the application prompt. Its active catalogs determine what the reply can use.
3. Keep `RendererState` for the surfaces the renderer still holds. Call your model
   client and pass the answer to `RetryDriver::judge`. A `Done` verdict gives you text
   and A2UI messages to send. `Retry` provides feedback for the next model request.
   On `GiveUp`, use the error to decide how to end the turn.

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

A rejected complete answer leaves prior renderer state intact. The host
controls its model client, scheduling, retry budgets, and message delivery.

## Stream a reply

To consume chunks, open a `ResponseStream` for the current state. Send
`StreamPart::A2ui` messages in order as `feed` yields them, and process parts
from `close` after the last chunk. Treat already delivered parts as applied.
The stream cannot retract those messages when it rejects a later block.

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

A v1.0 stream holds each component until its definition closes. During
progressive parsing, an `updateDataModel` can emit early once its `path` is known.
If the message omits `path`, it emits when the message closes. Have the model
write `path` before `value` when the renderer should see the update build up.
