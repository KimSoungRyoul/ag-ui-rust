//! Reading a direct JSON answer as it arrives.
//!
//! Chunk boundaries fall wherever the model's transport put them, so the
//! reader holds back anything a later chunk could change:
//!
//! - Text outside a block is handed on as it arrives, less a trailing run of
//!   whitespace and less any tail that could start a tag. Both are held for
//!   the next chunk.
//! - A tag split across chunks never leaks into the text. Backticks around a
//!   tag are dropped, as they are when an answer is read whole.
//! - The open block is read again on every chunk, with what the text has open
//!   closed for it, and handed on as the messages it holds so far.

use super::blocks::{
    BlockScanner, CLOSE_TAG, OPEN_TAG, close_tag_at, find_open_tag, is_tag_prefix, open_tag_at,
    open_tag_start_at, payload_start, possible_tag_start, trailing_fence_start,
};
use super::json::{Healing, read_partial};
use super::{compile_block, unclosed_block};
use crate::agent::format::{AnswerReader, Candidate, ReaderEvent};

/// Emits the text of a streamed answer that lies between its blocks.
///
/// The text between two blocks is one run. A run is emitted as it arrives,
/// less the whitespace at its edges: leading whitespace is dropped at the
/// start of a run, and trailing whitespace is held until more text of the run
/// follows, so a run ends where its last non-blank character does, as the
/// text parts of an answer read whole do.
#[derive(Debug, Default)]
struct TextRun {
    started: bool,
    held_space: String,
}

impl TextRun {
    /// Emits `text` as the next piece of the run. Nothing is emitted for a
    /// piece that is blank; its whitespace is held for the next piece.
    fn emit(&mut self, events: &mut Vec<ReaderEvent>, text: &str) {
        let body = if self.started {
            text
        } else {
            text.trim_start()
        }
        .trim_end();
        if body.is_empty() {
            if self.started {
                self.held_space.push_str(text);
            }
            return;
        }
        let start = text.len()
            - if self.started {
                text
            } else {
                text.trim_start()
            }
            .len();
        events.push(ReaderEvent::Text(format!("{}{body}", self.held_space)));
        self.held_space = text[start + body.len()..].to_string();
        self.started = true;
    }

    /// Ends the run, as a block does; whitespace held at its end is dropped.
    fn reset(&mut self) {
        self.started = false;
        self.held_space.clear();
    }
}

/// Reads one direct JSON answer, chunk by chunk.
#[derive(Debug)]
pub(super) struct DirectJsonReader {
    wrapped: bool,
    healing: Healing,
    whole_components: bool,
    closed: bool,
    /// Text outside blocks that could still start a tag.
    held: String,
    /// Backticks at the end of the text so far, dropped if a tag follows.
    held_backticks: String,
    /// A block just closed: backticks that follow belong to its tag.
    strip_close_backticks: bool,
    text: TextRun,
    in_block: bool,
    /// The body of the open block, as far as it has arrived.
    block: String,
    scanner: BlockScanner,
}

impl DirectJsonReader {
    pub(super) fn new(wrapped: bool, healing: Healing, whole_components: bool) -> Self {
        Self {
            wrapped,
            healing,
            whole_components,
            closed: false,
            held: String::new(),
            held_backticks: String::new(),
            strip_close_backticks: false,
            text: TextRun::default(),
            in_block: !wrapped,
            block: String::new(),
            scanner: BlockScanner::new(0),
        }
    }

    /// The open block as it reads up to `end`, or `None` while nothing but
    /// the start of a Markdown fence has arrived.
    fn read(&self, end: usize) -> Option<Vec<Candidate>> {
        let start = payload_start(&self.block, end)?;
        let stop = trailing_fence_start(&self.block, start, end);
        Some(read_partial(&self.block[start..stop], &self.healing))
    }

    fn read_block(&self, events: &mut Vec<ReaderEvent>, end: usize) {
        if let Some(candidates) = self.read(end).filter(|candidates| !candidates.is_empty()) {
            events.push(ReaderEvent::Block(candidates));
        }
    }

    fn end_block(&mut self) {
        self.in_block = !self.wrapped;
        self.block.clear();
        self.scanner = BlockScanner::new(0);
    }
}

impl AnswerReader for DirectJsonReader {
    fn feed(&mut self, chunk: &str) -> Vec<ReaderEvent> {
        let mut events = Vec::new();
        if self.closed {
            return events;
        }
        if !self.wrapped {
            self.block.push_str(chunk);
            self.read_block(&mut events, self.block.len());
            return events;
        }

        let mut input = std::mem::take(&mut self.held);
        input.push_str(chunk);
        if self.strip_close_backticks {
            let stripped = input.trim_start_matches('`');
            if stripped.len() != input.len() {
                input = stripped.to_string();
            }
            if !input.is_empty() {
                self.strip_close_backticks = false;
            }
        }
        if !self.in_block && !self.held_backticks.is_empty() && !input.is_empty() {
            let opens_tag = open_tag_at(&input, 0).is_some()
                || open_tag_start_at(&input, 0)
                || is_tag_prefix(&input, 0, OPEN_TAG);
            let backticks = std::mem::take(&mut self.held_backticks);
            if !opens_tag {
                input.insert_str(0, &backticks);
            }
        }

        while !input.is_empty() {
            if self.in_block {
                self.block.push_str(&input);
                input.clear();
                let (close_start, readable) = self.scanner.scan(&self.block);
                let Some(close_start) = close_start else {
                    self.read_block(&mut events, readable);
                    break;
                };
                let close_end = close_tag_at(&self.block, close_start).unwrap_or(self.block.len());
                let rest = &self.block[close_end..];
                if rest.starts_with('`') {
                    input = rest.trim_start_matches('`').to_string();
                } else {
                    input = rest.to_string();
                    self.strip_close_backticks = input.is_empty();
                }
                events.push(ReaderEvent::BlockEnd(compile_block(
                    &self.block[..close_start],
                )));
                self.end_block();
                continue;
            }

            if let Some((open_start, open_end)) = find_open_tag(&input, 0) {
                let before = input[..open_start].trim_end_matches('`').to_string();
                self.text.emit(&mut events, &before);
                self.text.reset();
                self.held_backticks.clear();
                self.in_block = true;
                input = input[open_end..].to_string();
                continue;
            }

            let mut hold = possible_tag_start(&input);
            if hold < input.len() {
                hold = input[..hold].trim_end_matches('`').len();
            }
            self.held = input[hold..].to_string();
            let mut emit = &input[..hold];
            if self.held.is_empty() {
                let without = emit.trim_end_matches('`');
                self.held_backticks = emit[without.len()..].to_string();
                emit = without;
            }
            self.text.emit(&mut events, emit);
            break;
        }
        events
    }

    fn close(&mut self) -> Vec<ReaderEvent> {
        let mut events = Vec::new();
        if self.closed {
            return events;
        }
        self.closed = true;
        if !self.wrapped {
            events.push(ReaderEvent::BlockEnd(compile_block(&self.block)));
            return events;
        }
        if self.in_block {
            events.push(ReaderEvent::BlockEnd(Err(unclosed_block(format!(
                "The response ended inside an {OPEN_TAG} block, before its {CLOSE_TAG} tag."
            )))));
            return events;
        }
        let backticks = std::mem::take(&mut self.held_backticks);
        if !backticks.is_empty() {
            self.text.emit(&mut events, &backticks);
        }
        let held = std::mem::take(&mut self.held);
        if !held.is_empty() {
            self.text.emit(&mut events, &held);
        }
        events
    }

    fn in_block(&self) -> bool {
        !self.closed && self.in_block
    }

    fn components_are_whole(&self) -> bool {
        self.whole_components
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reader() -> DirectJsonReader {
        DirectJsonReader::new(true, Healing::default(), false)
    }

    /// The events of a run of chunks, as short strings.
    fn seen(reader: &mut DirectJsonReader, chunks: &[&str]) -> Vec<String> {
        let mut seen = Vec::new();
        let mut note = |events: Vec<ReaderEvent>| {
            for event in events {
                seen.push(match event {
                    ReaderEvent::Text(text) => format!("text {text:?}"),
                    ReaderEvent::Block(candidates) => format!(
                        "block {}",
                        candidates
                            .iter()
                            .map(|candidate| format!(
                                "{}{}",
                                candidate.message,
                                if candidate.closed { "" } else { "…" }
                            ))
                            .collect::<Vec<_>>()
                            .join(" ")
                    ),
                    ReaderEvent::BlockEnd(Ok(messages)) => {
                        format!("end {}", serde_json::Value::Array(messages))
                    }
                    ReaderEvent::BlockEnd(Err(error)) => format!("end error {error}"),
                });
            }
        };
        for chunk in chunks {
            note(reader.feed(chunk));
        }
        note(reader.close());
        seen
    }

    #[test]
    fn text_is_handed_on_as_it_arrives_less_its_edges() {
        assert_eq!(
            seen(&mut reader(), &["  Here ", "it is: ", " \n"]),
            ["text \"Here\"", "text \" it is:\""]
        );
    }

    #[test]
    fn a_tag_split_across_chunks_never_reaches_the_text() {
        assert_eq!(
            seen(
                &mut reader(),
                &[
                    "Before <a2",
                    "ui-json>[1",
                    "]</a2ui",
                    "-json> after <",
                    "b>"
                ]
            ),
            [
                "text \"Before\"",
                "block 1",
                "end [1]",
                "text \"after\"",
                "text \" <b>\"",
            ]
        );
    }

    #[test]
    fn backticks_around_the_tags_are_dropped() {
        assert_eq!(
            seen(
                &mut reader(),
                &["`", "<a2ui-json>`[1]`", "</a2ui-json>", "`"]
            ),
            ["block 1", "end [1]"]
        );
        assert_eq!(
            seen(&mut reader(), &["Use `x`", " here"]),
            ["text \"Use `x\"", "text \"` here\""]
        );
    }

    #[test]
    fn the_open_block_is_read_as_far_as_it_goes() {
        let mut reader = DirectJsonReader::new(
            true,
            Healing {
                progressive_keys: ["text".to_string()].into(),
                ..Healing::default()
            },
            false,
        );
        assert_eq!(
            seen(
                &mut reader,
                &[
                    "<a2ui-json>[{\"a\": 1}, {\"text\": \"He",
                    "llo\"}",
                    "]</a2ui-json>"
                ]
            ),
            [
                "block {\"a\":1} {\"text\":\"He\"}…",
                "block {\"a\":1} {\"text\":\"Hello\"}",
                "end [{\"a\":1},{\"text\":\"Hello\"}]",
            ]
        );
    }

    #[test]
    fn a_block_still_open_when_the_answer_ends_is_an_error() {
        let mut reader = reader();
        assert!(matches!(
            reader.feed("Text <a2ui-json>[").as_slice(),
            [ReaderEvent::Text(text)] if text == "Text"
        ));
        assert!(reader.in_block());
        let events = reader.close();
        assert!(matches!(
            events.as_slice(),
            [ReaderEvent::BlockEnd(Err(crate::Error::Parse(_)))]
        ));
        assert!(!reader.in_block());
        assert!(reader.close().is_empty());
    }

    #[test]
    fn an_unwrapped_answer_is_one_block_until_it_ends() {
        let mut reader = DirectJsonReader::new(false, Healing::default(), false);
        assert!(reader.in_block());
        assert_eq!(
            seen(&mut reader, &["```json\n[{\"a\": 1},", " {\"b\": 2}]\n```"]),
            [
                "block {\"a\":1}",
                "block {\"a\":1} {\"b\":2}",
                "end [{\"a\":1},{\"b\":2}]",
            ]
        );
    }
}
