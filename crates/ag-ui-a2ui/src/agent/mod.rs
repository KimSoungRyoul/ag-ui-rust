//! What an agent framework delegates A2UI to (feature `agent`, on by default).
//!
//! The framework owns the model call. This layer supplies everything around
//! it: the instructions that go into the prompt, the reading of the answer,
//! validation against the catalogs the renderer said it can draw, the mirror
//! of what the renderer holds, and the decision whether to ask again. Nothing
//! here calls a model, awaits, spawns or does network I/O, so it fits any
//! framework, executor and transport.
//!
//! It is the one generation path of the crate: one prompt snippet, one reader
//! for each output format, one retry decision.
//!
//! | Piece | What it is |
//! |---|---|
//! | [`CatalogConfig`] | A catalog the agent registered, and the transformers it applies before a model or a validator sees it. |
//! | [`RendererCapabilities`], [`resolve_catalogs`] | What the renderer said it can draw, and which catalogs that makes active. |
//! | [`A2uiGenerator`] | What the agent knows for its whole life: catalogs, examples, format. Built once. |
//! | [`A2uiRequestProcessor`] | Everything one request needs: the active catalogs, the prompt snippet, and the reading of an answer, whole or streamed. |
//! | [`OutputFormat`] | The notation a model writes in. [`DirectJson`] is the one implemented. |
//! | [`RendererState`] | The agent's mirror of the renderer's surfaces: the protocol's own [`MessageProcessor`](crate::processor::MessageProcessor), with the active catalogs registered. |
//! | [`RetryDriver`] | Says "done", "ask again with this feedback" or "give up". The caller makes the call. |
//! | [`ResponseStream`], [`StreamedAnswer`] | An answer read as it arrives. |
//!
//! # One turn
//!
//! ```
//! # use ag_ui_a2ui::{A2uiVersion, Error};
//! # use ag_ui_a2ui::agent::{
//! #     A2uiGenerator, CatalogConfig, RendererCapabilities, ResponsePart, RetryDriver, Verdict,
//! # };
//! # async fn call_model(_system: &str, prompt: &str) -> Result<String, Error> {
//! #     Ok(if prompt.contains("Correction required") {
//! #         "Nothing to show after all.".to_string()
//! #     } else {
//! #         "<a2ui-json>[{\"version\": \"v0.9.1\"}]</a2ui-json>".to_string()
//! #     })
//! # }
//! # async fn turn(
//! #     metadata: serde_json::Value,
//! #     role: &str,
//! #     request: &str,
//! # ) -> Result<Vec<ResponsePart>, Error> {
//! let generator = A2uiGenerator::new(vec![CatalogConfig::basic(A2uiVersion::V0_9_1)?]); // once
//! let capabilities = RendererCapabilities::from_metadata(Some(&metadata), A2uiVersion::V0_9_1)?;
//! let processor = generator.create_processor(capabilities.as_ref())?; // per request
//! let system = format!("{role}\n\n{}", processor.prompt_snippet());
//! let mut state = processor.create_state(); // chat: per turn. canvas: kept for the conversation
//! let mut retry = RetryDriver::new(&processor, 3);
//! let mut prompt = request.to_string();
//! let parts = loop {
//!     let answer = call_model(&system, &prompt).await?; // the framework's call
//!     match retry.judge(&answer, &mut state) {
//!         Verdict::Done(parts) => break parts,
//!         Verdict::Retry(feedback) => prompt = feedback.prompt(request),
//!         Verdict::GiveUp(error) => return Err(error),
//!     }
//! };
//! // `parts` is the text and the A2UI messages to send; `state` is what the renderer now holds.
//! # Ok(parts)
//! # }
//! # use std::future::Future;
//! # let metadata = serde_json::json!({"a2uiClientCapabilities": {"v0.9": {"supportedCatalogIds": [
//! #     "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json"
//! # ]}}});
//! # let mut turn = std::pin::pin!(turn(metadata, "You are a helpful agent.", "Show a greeting."));
//! # let mut context = std::task::Context::from_waker(std::task::Waker::noop());
//! # let std::task::Poll::Ready(parts) = turn.as_mut().poll(&mut context) else { panic!() };
//! # assert_eq!(parts?, [ResponsePart::Text("Nothing to show after all.".into())]);
//! # Ok::<(), Error>(())
//! ```
//!
//! To show the answer while it is still being written, open a stream instead
//! of judging the finished text:
//!
//! ```
//! # use ag_ui_a2ui::A2uiVersion;
//! # use ag_ui_a2ui::agent::{CatalogDocument, A2uiRequestProcessor, DirectJson, StreamOptions, StreamPart};
//! # let processor = A2uiRequestProcessor::new(
//! #     vec![CatalogDocument::basic(A2uiVersion::V0_9)?],
//! #     Vec::new(),
//! #     std::sync::Arc::new(DirectJson::new()),
//! # )?;
//! # let chunks = ["Here ", "you go."];
//! let mut state = processor.create_state();
//! let mut stream = processor.open_stream(&state, StreamOptions::default())?;
//! for chunk in chunks {
//!     for part in stream.feed(chunk, &mut state)? {
//!         match part {
//!             StreamPart::Text(text) => print!("{text}"),
//!             StreamPart::A2ui(messages) => { /* send them: the state already holds them */ }
//!             StreamPart::Rejected(error) => { /* only with AfterRejectedBlock::Continue */ }
//!         }
//!     }
//! }
//! stream.close(&mut state)?;
//! # Ok::<(), ag_ui_a2ui::Error>(())
//! ```
//!
//! # What a rejected answer leaves behind
//!
//! An answer read whole is all or nothing: after
//! [`A2uiRequestProcessor::parse_response_in`] fails, and after
//! [`RetryDriver::judge`] says anything but [`Verdict::Done`], the state is
//! exactly what it was. A streamed answer cannot be, since its parts are
//! handed on as they arrive; what it keeps to instead, and how to go back, is
//! in the documentation of [`ResponseStream`].
//!
//! # Errors
//!
//! The layer reports with [`crate::Error`], and the variant says whose fault
//! it is.
//!
//! | Variant | What it is | Category of the A2UI conformance suites |
//! |---|---|---|
//! | [`Error::Parse`](crate::Error::Parse) | The answer's text could not be read: a block that is not the format's notation, or one the answer ended inside. | `ParseError` |
//! | [`Error::Validation`](crate::Error::Validation) | The answer reads, and a renderer in this state would reject it. Every failure carries a locator into the answer and a sentence a model can act on. A fault of a surface's life (created while live, updated while not) is one `invalid_value` at `messages[i]`; so is a message that does not decode. | `ValidationError`, with its kinds `IntegrityError` and `RecursionError` |
//! | [`Error::Catalog`](crate::Error::Catalog) | The catalogs are the problem, and no answer can fix that: one that does not load, capabilities that leave none active, a state that holds other catalogs than the processor. | `CatalogError` |
//!
//! The first two are the model's to fix, and [`RetryDriver`] feeds them back.
//! A `createSurface` that names a catalog the session does not hold is one of
//! them, an `invalid_value` at its `catalogId`: the name came from the answer.

mod answer;
mod capabilities;
mod catalog;
mod direct_json;
mod format;
mod processor;
mod retry;
mod state;
mod stream;

pub use answer::{AnswerPart, StreamedAnswer};
pub use capabilities::{CLIENT_CAPABILITIES_KEY, RendererCapabilities, resolve_catalogs};
pub use catalog::{CatalogConfig, CatalogDocument, CatalogTransformer};
pub use direct_json::DirectJson;
pub use format::{
    AnswerReader, Candidate, OutputFormat, RawPart, ReaderEvent, ResponseError, ResponsePart,
    StreamPart,
};
pub use processor::{A2uiGenerator, A2uiRequestProcessor};
pub use retry::{Feedback, RetryDriver, Verdict};
pub use state::{RendererState, validate_payload};
pub use stream::{AfterRejectedBlock, ResponseStream, StreamOptions};

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A framework keeps these in one struct across `.await` points and moves
    /// them between tasks.
    #[test]
    fn the_long_lived_types_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<A2uiGenerator>();
        assert_send_sync::<A2uiRequestProcessor>();
        assert_send_sync::<RendererState>();
        assert_send_sync::<RetryDriver>();
        assert_send_sync::<ResponseStream>();
        assert_send_sync::<StreamedAnswer>();
        assert_send_sync::<Arc<dyn OutputFormat>>();
        assert_send_sync::<Box<dyn AnswerReader>>();
        assert_send_sync::<CatalogConfig>();
        assert_send_sync::<RendererCapabilities>();
    }

    /// And none of them borrows: each can outlive what it was made from.
    #[test]
    fn the_long_lived_types_borrow_nothing() {
        fn assert_static<T: 'static>() {}
        assert_static::<A2uiRequestProcessor>();
        assert_static::<RendererState>();
        assert_static::<RetryDriver>();
        assert_static::<ResponseStream>();
        assert_static::<StreamedAnswer>();
    }
}
