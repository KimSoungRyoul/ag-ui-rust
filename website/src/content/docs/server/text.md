---
title: Streaming text
description: Emitting an assistant message as it is generated, and the handle that makes the event order a compile-time concern.
---

An assistant message is three events on the wire — `TEXT_MESSAGE_START`, some number of
`TEXT_MESSAGE_CONTENT`, then `TEXT_MESSAGE_END` — all carrying the same message id. Handing
an agent three raw emit calls means trusting it to close what it opened, in order, on every
path including the early return. `ag_ui::server` hands out an RAII handle instead.

## The whole message at once

When you already have the text, `say` emits all three:

```rust
use ag_ui::{Event, RunAgentInput, TextMessageRole};
use ag_ui::server::RunContext;

fn main() -> ag_ui::server::Result<()> {
    // `RunContext::new` is the unit-test harness: a context, and the receiving
    // end of its event stream. Inside an agent this is just `ctx`.
    let (mut ctx, mut events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;

    let id = ctx.say("Hello from Rust.")?;

    assert_eq!(id.as_str(), "r-msg-1");
    assert_eq!(
        events.drain(),
        vec![
            Event::text_message_start("r-msg-1", TextMessageRole::Assistant),
            Event::text_message_content("r-msg-1", "Hello from Rust."),
            Event::text_message_end("r-msg-1"),
        ]
    );
    Ok(())
}
```

Message ids are derived from the run id and a counter — `r-msg-1` above — rather than being
UUIDs. The protocol asks for opaque strings, this crate takes no `uuid` dependency, and a
deterministic id makes a recorded stream diffable. `message_with_id` takes your own when you
need one.

## Streaming it as it arrives

`assistant_message()` emits `TEXT_MESSAGE_START` and returns a handle. `delta` appends
content; `end` closes it:

```rust
use ag_ui::{Event, RunAgentInput, TextMessageRole};
use ag_ui::server::RunContext;

fn main() -> ag_ui::server::Result<()> {
    let (mut ctx, mut events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;

    let mut message = ctx.assistant_message()?;
    for word in ["Hello", ", ", "world"] {
        message.delta(word)?;
    }
    message.end()?;

    assert_eq!(
        events.drain(),
        vec![
            Event::text_message_start("r-msg-1", TextMessageRole::Assistant),
            Event::text_message_content("r-msg-1", "Hello"),
            Event::text_message_content("r-msg-1", ", "),
            Event::text_message_content("r-msg-1", "world"),
            Event::text_message_end("r-msg-1"),
        ]
    );
    Ok(())
}
```

That is the shape a model stream maps onto directly: one `delta` per chunk the provider
hands you, no buffering, and the client draws the words as they land.

`assistant_message` is `message(TextMessageRole::Assistant)`. The role is on
`TEXT_MESSAGE_START` and the other three variants — `Developer`, `System`, `User` — are
there for an agent replaying a transcript rather than generating one.

## `end()` is optional; the terminator is not

The handle emits `TEXT_MESSAGE_END` on `Drop` if `end` was not called. Forgetting it, or
returning early through a `?` halfway through the message, still produces a well-formed
stream:

```rust
use ag_ui::{Event, EventType, RunAgentInput};
use ag_ui::server::{Error, RunContext};

fn write_it(ctx: &mut RunContext<()>) -> ag_ui::server::Result<()> {
    let mut message = ctx.assistant_message()?;
    message.delta("Looking that up")?;
    // The message is still open, and this returns.
    Err(Error::agent("the weather service is down"))
}

fn main() -> ag_ui::server::Result<()> {
    let (mut ctx, mut events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;
    assert!(write_it(&mut ctx).is_err());

    let types: Vec<EventType> = events.drain().iter().map(Event::event_type).collect();
    assert_eq!(
        types,
        [
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            // Emitted by `Drop`, on the way out of the failing function.
            EventType::TextMessageEnd,
        ]
    );
    Ok(())
}
```

`end()` is still worth calling when you want to see the error: `Drop` has nowhere to report
one, so it swallows it.

## Why there is no `.await` on `delta`

`msg.delta(text)?` is synchronous, which is unusual for an emitter API — the TypeScript and
.NET SDKs both `await` theirs, and the first draft of this crate copied them.

It cannot coexist with the guarantee above. `Drop` cannot be async in Rust, so a handle
cannot `await` while emitting its terminator. Either the terminator is automatic and the
emit path is synchronous, or the emit path is async and every agent has to remember to close
its own messages. This SDK picks the first. Emitters push into an unbounded channel and the
transport drains it; nothing blocks, and nothing is buffered waiting for a reader.

The practical consequence is a pleasant one: after calling an agent's code, everything it
emitted is already queued, which is why the assertions above are plain `drain()` calls with
no runtime in sight.

## Two messages at once do not compile

The handle borrows the run context mutably for as long as it lives, so opening a second
message while the first is open is a borrow-check error rather than a protocol violation a
frontend discovers later:

```rust,compile_fail
use ag_ui::server::RunContext;

fn interleave(ctx: &mut RunContext<()>) {
    let mut first = ctx.assistant_message().unwrap();
    // error[E0499]: cannot borrow `*ctx` as mutable more than once at a time
    let mut second = ctx.assistant_message().unwrap();
    first.delta("a").unwrap();
    second.delta("b").unwrap();
}
```

That block is `compile_fail`, so this page's build fails if it ever starts compiling. The
same example lives in `crates/ag-ui/src/server/emit/mod.rs` as a `compile_fail,E0499`
doctest, which is the executable proof that the guarantee is still there — weaken the
emitter API and that test goes red.

:::caution
Stable rustdoc does not enforce the error code on a `compile_fail` doctest; it only checks
that the block fails to compile, for any reason at all — a typo would do. CI therefore runs
the doctests on nightly as well, which does enforce it.
:::

## What an open message can still do

The borrow forbids a second *block*, not work. A handle holds two fields of the run context
— the event sink and the state — rather than the context itself, so the state is reachable
while the message is open:

```rust
use ag_ui::RunAgentInput;
use ag_ui::server::RunContext;
use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
struct Progress {
    words: u32,
}

fn main() -> ag_ui::server::Result<()> {
    let (mut ctx, mut events) = RunContext::<Progress>::new(RunAgentInput::new("t", "r"))?;

    let mut message = ctx.assistant_message()?;
    message.delta("Hello")?;
    message.state_mut().words += 1;
    // STATE_SNAPSHOT, with the message still open. `STATE_*` is unordered on
    // the wire, so this is a legal stream.
    message.publish_state()?;
    message.end()?;

    assert_eq!(ctx.state().words, 1);
    assert_eq!(events.drain().len(), 4);
    Ok(())
}
```

`message.emit(event)` is the general form, for the unordered families — `STATE_*`,
`ACTIVITY_*`, `CUSTOM`, `RAW` — that may legally interleave with a message. Opening a second
message through it is a protocol violation the
[ordering verifier](/ag-ui-rust/server/errors/) rejects at the point of emission.

What the handle cannot do is open another block: it holds no run context to open one with,
and the context it came from stays borrowed until it drops.

## Reasoning

Model reasoning you want the client to render has its own family, and its own handle.
`REASONING_*` nests a message inside a block, so the handle brackets four events rather than
two:

```text
REASONING_START          ← on creation
REASONING_MESSAGE_START  ← on creation
REASONING_MESSAGE_CONTENT × n
REASONING_MESSAGE_END    ← on end() or Drop
REASONING_END            ← on end() or Drop
```

```rust
use ag_ui::{Event, EventType, RunAgentInput};
use ag_ui::server::RunContext;

fn main() -> ag_ui::server::Result<()> {
    let (mut ctx, mut events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;

    // The one-shot form, like `say`.
    ctx.think("The user wants a title.")?;

    // The streaming form, like `assistant_message`.
    let mut reasoning = ctx.reasoning()?;
    reasoning.delta("Checking the board first")?;
    reasoning.end()?;

    let types: Vec<EventType> = events.drain().iter().map(Event::event_type).collect();
    assert_eq!(types.len(), 10);
    assert_eq!(types[0], EventType::ReasoningStart);
    assert_eq!(types[9], EventType::ReasoningEnd);
    Ok(())
}
```

Reasoning a provider returns only as an opaque blob — the zero-data-retention case, where
the signature has to be replayed on the next request for the model to stay coherent — goes
through `reasoning.encrypted_value(…)` instead, which emits `REASONING_ENCRYPTED_VALUE`.

## Chunks

The `*_CHUNK` family is unbracketed by definition: a chunk carries its own id and needs no
start and no end. It exists for provider adapters that cannot know a message ended until the
next one begins, so there is nothing for an RAII handle to close and this crate offers no
handle for it. Emit one through `ctx.emit` when you need it:

```rust
use ag_ui::{Event, MessageId, RunAgentInput};
use ag_ui::server::RunContext;

fn main() -> ag_ui::server::Result<()> {
    let (mut ctx, mut events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;

    ctx.emit(Event::text_message_chunk(
        Some(MessageId::new("chunk-1")),
        Some("a whole update in one event".to_owned()),
    ))?;

    assert_eq!(events.drain().len(), 1);
    Ok(())
}
```

The verifier knows chunks are self-contained, so it registers the id rather than rejecting
the event for having no start.

## API

- [`RunContext::say`](/ag-ui-rust/api/ag_ui/server/struct.RunContext.html#method.say),
  [`assistant_message`](/ag-ui-rust/api/ag_ui/server/struct.RunContext.html#method.assistant_message),
  [`message_with_id`](/ag-ui-rust/api/ag_ui/server/struct.RunContext.html#method.message_with_id)
- [`ag_ui::server::MessageHandle`](/ag-ui-rust/api/ag_ui/server/struct.MessageHandle.html)
- [`ag_ui::server::ReasoningHandle`](/ag-ui-rust/api/ag_ui/server/struct.ReasoningHandle.html)
- [`ag_ui::server::emit`](/ag-ui-rust/api/ag_ui/server/emit/index.html) — the module that
  explains the typestate design
