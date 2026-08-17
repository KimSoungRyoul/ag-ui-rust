//! Decoding `text/event-stream`.
//!
//! The encoder lives in `ag-ui-core`; this is the other half. It is a wire
//! format parser fed by a network, so it assumes nothing about what it is
//! handed: bytes arrive in arbitrary chunks that split lines, split UTF-8
//! sequences, and end without the terminating blank line the format calls for.
//!
//! What it handles, because real servers and proxies do all of it:
//!
//! - `data:` repeated over several lines, rejoined with `\n` — the format's own
//!   way of carrying a payload that contains newlines.
//! - Comment lines (`: keep-alive`), which proxies inject to hold a connection
//!   open and which dispatch nothing.
//! - A body that ends without a blank line: [`SseDecoder::finish`] dispatches
//!   the last frame rather than dropping it.
//! - `\n`, `\r\n` and lone `\r` line endings, including a `\r\n` split across
//!   two chunks.
//! - A leading UTF-8 byte-order mark.
//! - A field with no colon, an empty field value, and unknown field names.
//! - A frame with no `data` field, which the format says not to dispatch.
//!
//! What it refuses: invalid UTF-8, and a single frame larger than
//! [`SseDecoder::max_frame_size`] — an unterminated line is otherwise an
//! unbounded allocation driven by the other end.
//!
//! ```
//! use ag_ui_client::transport::SseDecoder;
//!
//! let mut decoder = SseDecoder::new();
//! decoder.push(b": keep-alive\n\ndata: {\"type\":\"RUN_ERROR\",\"mes")?;
//! assert!(decoder.next_frame()?.is_none());
//!
//! decoder.push(b"sage\":\"boom\"}\n\n")?;
//! let frame = decoder.next_frame()?.expect("a complete frame");
//! assert_eq!(frame.into_event()?.event_type().as_str(), "RUN_ERROR");
//! # Ok::<(), ag_ui_client::Error>(())
//! ```

use std::collections::VecDeque;

use ag_ui_core::Event;
use futures_core::Stream;
use futures_util::StreamExt;

use crate::error::{Error, Result};

/// The default cap on one frame, before the decoder gives up: 8 MiB.
pub const DEFAULT_MAX_FRAME_SIZE: usize = 8 * 1024 * 1024;

/// One decoded `text/event-stream` frame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SseFrame {
    /// The `event:` field, if the server sent one. AG-UI does not use it.
    pub event: Option<String>,
    /// The `data:` field, with the trailing newline removed and multiple
    /// `data:` lines rejoined with `\n`.
    pub data: String,
    /// The `id:` field, if the server sent one.
    pub id: Option<String>,
    /// The `retry:` field, in milliseconds, if the server sent one.
    pub retry: Option<u64>,
}

impl SseFrame {
    /// Parses the frame's payload as an AG-UI event.
    pub fn into_event(self) -> Result<Event> {
        serde_json::from_str(&self.data).map_err(Error::from)
    }
}

/// An incremental `text/event-stream` decoder.
///
/// Push bytes, pull frames. It owns a buffer for the partial line at the end of
/// each chunk, which is why it has to be a struct and not a function.
#[derive(Clone, Debug)]
pub struct SseDecoder {
    buffer: Vec<u8>,
    data: String,
    event: Option<String>,
    id: Option<String>,
    retry: Option<u64>,
    max_frame_size: usize,
    at_start: bool,
}

impl Default for SseDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl SseDecoder {
    /// A decoder with the default frame size cap.
    pub fn new() -> Self {
        Self {
            buffer: Vec::new(),
            data: String::new(),
            event: None,
            id: None,
            retry: None,
            max_frame_size: DEFAULT_MAX_FRAME_SIZE,
            at_start: true,
        }
    }

    /// Sets the cap on a single frame.
    #[must_use]
    pub fn with_max_frame_size(mut self, bytes: usize) -> Self {
        self.max_frame_size = bytes;
        self
    }

    /// The cap on a single frame, in bytes.
    pub fn max_frame_size(&self) -> usize {
        self.max_frame_size
    }

    /// Feeds the decoder a chunk of the response body.
    ///
    /// # Errors
    ///
    /// [`Error::Decode`] when the buffered frame exceeds
    /// [`SseDecoder::max_frame_size`] — a server that never sends a line break
    /// would otherwise grow this buffer without bound.
    pub fn push(&mut self, bytes: &[u8]) -> Result<()> {
        let size = self.buffer.len() + self.data.len() + bytes.len();
        if size > self.max_frame_size {
            return Err(Error::decode(format!(
                "frame exceeded the {} byte limit before a line break",
                self.max_frame_size
            )));
        }
        self.buffer.extend_from_slice(bytes);
        Ok(())
    }

    /// Pulls the next complete frame, if the bytes pushed so far contain one.
    ///
    /// # Errors
    ///
    /// [`Error::Decode`] when the bytes are not UTF-8.
    pub fn next_frame(&mut self) -> Result<Option<SseFrame>> {
        while let Some(line) = self.take_line(false)? {
            if let Some(frame) = self.process(&line) {
                return Ok(Some(frame));
            }
        }
        Ok(None)
    }

    /// Drains the decoder at the end of the stream.
    ///
    /// A body that stops without the blank line that terminates its last frame
    /// is common enough — a closed connection, a server that forgets — that
    /// dropping the frame would lose real events. Anything still buffered is
    /// dispatched here.
    ///
    /// # Errors
    ///
    /// [`Error::Decode`] when the trailing bytes are not UTF-8.
    pub fn finish(&mut self) -> Result<Vec<SseFrame>> {
        let mut frames = Vec::new();
        while let Some(line) = self.take_line(true)? {
            if let Some(frame) = self.process(&line) {
                frames.push(frame);
            }
        }
        if let Some(frame) = self.dispatch() {
            frames.push(frame);
        }
        Ok(frames)
    }

    /// Takes one complete line, minus its terminator.
    ///
    /// At `eof` the trailing bytes count as a line even without a terminator.
    /// Otherwise a `\r` at the very end of the buffer is held back: it may yet
    /// turn out to be the first half of a `\r\n` split across two chunks.
    fn take_line(&mut self, eof: bool) -> Result<Option<String>> {
        let Some(position) = self.buffer.iter().position(|b| *b == b'\n' || *b == b'\r') else {
            if eof && !self.buffer.is_empty() {
                let line = std::mem::take(&mut self.buffer);
                return self.decode_line(line).map(Some);
            }
            return Ok(None);
        };

        let is_cr = self.buffer[position] == b'\r';
        if is_cr && position + 1 == self.buffer.len() && !eof {
            return Ok(None);
        }

        let width = if is_cr && self.buffer.get(position + 1) == Some(&b'\n') {
            2
        } else {
            1
        };
        let rest = self.buffer.split_off(position + width);
        let mut line = std::mem::replace(&mut self.buffer, rest);
        line.truncate(position);
        self.decode_line(line).map(Some)
    }

    fn decode_line(&mut self, line: Vec<u8>) -> Result<String> {
        let mut line = String::from_utf8(line)
            .map_err(|error| Error::decode(format!("stream is not UTF-8: {error}")))?;
        if self.at_start {
            self.at_start = false;
            if let Some(stripped) = line.strip_prefix('\u{feff}') {
                line = stripped.to_owned();
            }
        }
        Ok(line)
    }

    /// Applies one line to the frame being built, dispatching on a blank line.
    fn process(&mut self, line: &str) -> Option<SseFrame> {
        if line.is_empty() {
            return self.dispatch();
        }
        if line.starts_with(':') {
            // A comment. Heartbeats are the usual reason one is here.
            return None;
        }

        let (field, value) = match line.find(':') {
            Some(index) => (&line[..index], strip_one_space(&line[index + 1..])),
            // A field name with no colon has an empty value.
            None => (line, ""),
        };

        match field {
            "data" => {
                self.data.push_str(value);
                self.data.push('\n');
            }
            "event" => self.event = Some(value.to_owned()),
            "id" => self.id = Some(value.to_owned()),
            "retry" => {
                if let Ok(milliseconds) = value.parse() {
                    self.retry = Some(milliseconds);
                }
            }
            // Unknown fields are ignored, as the format requires.
            _ => {}
        }
        None
    }

    /// Emits the buffered frame, if it has a payload.
    fn dispatch(&mut self) -> Option<SseFrame> {
        let event = self.event.take();
        let id = self.id.take();
        let retry = self.retry.take();
        let mut data = std::mem::take(&mut self.data);
        if data.is_empty() {
            // No data field: the format says this dispatches nothing. A frame
            // of pure comments or a stray blank line lands here.
            return None;
        }
        data.pop();
        Some(SseFrame {
            event,
            data,
            id,
            retry,
        })
    }
}

/// Removes the single optional space after a field's colon.
fn strip_one_space(value: &str) -> &str {
    value.strip_prefix(' ').unwrap_or(value)
}

/// Decodes a stream of byte chunks into a stream of events.
///
/// This is the adapter between a transport's body stream and the rest of the
/// crate. Errors from the byte stream become [`Error::Transport`] items and end
/// the stream; a frame whose payload is not a valid event becomes an error item
/// and the stream continues, because one malformed event should not silence the
/// rest of the run.
pub fn decode_events<S, B, E>(chunks: S) -> impl Stream<Item = Result<Event>>
where
    S: Stream<Item = core::result::Result<B, E>>,
    B: AsRef<[u8]>,
    E: std::error::Error + Send + Sync + 'static,
{
    let state = Decoding {
        chunks: Box::pin(chunks),
        decoder: SseDecoder::new(),
        ready: VecDeque::new(),
        done: false,
    };

    futures_util::stream::unfold(state, |mut state| async move {
        loop {
            if let Some(item) = state.ready.pop_front() {
                return Some((item, state));
            }
            if state.done {
                return None;
            }

            match state.chunks.next().await {
                Some(Ok(bytes)) => {
                    if let Err(error) = state.decoder.push(bytes.as_ref()) {
                        state.done = true;
                        return Some((Err(error), state));
                    }
                    loop {
                        match state.decoder.next_frame() {
                            Ok(Some(frame)) => state.ready.push_back(frame.into_event()),
                            Ok(None) => break,
                            Err(error) => {
                                state.done = true;
                                state.ready.push_back(Err(error));
                                break;
                            }
                        }
                    }
                }
                Some(Err(error)) => {
                    state.done = true;
                    return Some((Err(Error::transport(error)), state));
                }
                None => {
                    state.done = true;
                    match state.decoder.finish() {
                        Ok(frames) => state
                            .ready
                            .extend(frames.into_iter().map(SseFrame::into_event)),
                        Err(error) => state.ready.push_back(Err(error)),
                    }
                }
            }
        }
    })
}

/// The state [`decode_events`] carries between polls.
struct Decoding<S> {
    chunks: std::pin::Pin<Box<S>>,
    decoder: SseDecoder,
    ready: VecDeque<Result<Event>>,
    done: bool,
}
