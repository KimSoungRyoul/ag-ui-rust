//! Finding and cleaning `<a2ui-json>` blocks in what a model wrote.
//!
//! Tags are matched without regard to case, an open tag may carry attributes,
//! and a tag written inside a JSON string does not count. All offsets are byte
//! offsets into the text they were found in.

use crate::agent::format::RawPart;

pub(super) const OPEN_TAG: &str = crate::constants::A2UI_OPEN_TAG;
pub(super) const CLOSE_TAG: &str = crate::constants::A2UI_CLOSE_TAG;
const OPEN_NAME: &str = "<a2ui-json";
const CLOSE_NAME: &str = "</a2ui-json";

fn starts_with_ignore_case(text: &str, at: usize, prefix: &str) -> bool {
    text.as_bytes()
        .get(at..at + prefix.len())
        .is_some_and(|found| found.eq_ignore_ascii_case(prefix.as_bytes()))
}

/// An open tag that starts at `at`: `<a2ui-json>`, or `<a2ui-json` followed
/// by whitespace and attributes up to `>`. Returns where the tag ends.
pub(super) fn open_tag_at(text: &str, at: usize) -> Option<usize> {
    if !starts_with_ignore_case(text, at, OPEN_NAME) {
        return None;
    }
    let after = at + OPEN_NAME.len();
    let rest = &text[after..];
    match rest.chars().next()? {
        '>' => Some(after + 1),
        next if next.is_whitespace() => rest.find('>').map(|end| after + end + 1),
        _ => None,
    }
}

/// The first open tag at or after `from`: where it starts and ends.
pub(super) fn find_open_tag(text: &str, from: usize) -> Option<(usize, usize)> {
    let mut at = from;
    while let Some(offset) = text.get(at..)?.find('<') {
        let start = at + offset;
        if let Some(end) = open_tag_at(text, start) {
            return Some((start, end));
        }
        at = start + 1;
    }
    None
}

/// A close tag that starts at `at`: `</a2ui-json`, optional whitespace, `>`.
/// Returns where the tag ends.
pub(super) fn close_tag_at(text: &str, at: usize) -> Option<usize> {
    if !starts_with_ignore_case(text, at, CLOSE_NAME) {
        return None;
    }
    let after = at + CLOSE_NAME.len();
    let rest = &text[after..];
    let blank = rest.len() - rest.trim_start().len();
    rest[blank..].starts_with('>').then_some(after + blank + 1)
}

fn has_close_tag(text: &str) -> bool {
    text.match_indices('<')
        .any(|(at, _)| close_tag_at(text, at).is_some())
}

/// Whether the text from `at` to its end could be the start of an open tag
/// that carries attributes: `<a2ui-json`, whitespace, and no `>` yet.
pub(super) fn open_tag_start_at(text: &str, at: usize) -> bool {
    if !starts_with_ignore_case(text, at, OPEN_NAME) {
        return false;
    }
    let rest = &text[at + OPEN_NAME.len()..];
    rest.chars().next().is_some_and(char::is_whitespace) && !rest.contains('>')
}

/// Whether the text from `at` to its end is a close tag missing only its `>`.
fn close_tag_start_at(text: &str, at: usize) -> bool {
    starts_with_ignore_case(text, at, CLOSE_NAME)
        && text[at + CLOSE_NAME.len()..].trim_start().is_empty()
}

/// Whether the text from `at` to its end is a proper prefix of `tag`.
pub(super) fn is_tag_prefix(text: &str, at: usize, tag: &str) -> bool {
    let rest = &text.as_bytes()[at..];
    rest.len() < tag.len() && tag.as_bytes()[..rest.len()].eq_ignore_ascii_case(rest)
}

/// Where a tail of `text` could start an open tag, else its length.
pub(super) fn possible_tag_start(text: &str) -> usize {
    text.match_indices('<')
        .map(|(at, _)| at)
        .find(|at| is_tag_prefix(text, *at, OPEN_TAG) || open_tag_start_at(text, *at))
        .unwrap_or(text.len())
}

/// Finds the close tag of a block as its text arrives.
///
/// Each scan goes on from where the last one stopped, so across the scans of
/// a longer and longer text each character is read once.
#[derive(Debug, Clone, Default)]
pub(super) struct BlockScanner {
    /// Where the block's body starts.
    start: usize,
    /// Where the next scan starts.
    at: usize,
    in_string: bool,
    /// The character at `at` follows a backslash inside a string.
    escaped: bool,
}

impl BlockScanner {
    pub(super) fn new(start: usize) -> Self {
        Self {
            start,
            at: start,
            ..Self::default()
        }
    }

    /// Scans `content`, the text the last scan read and more.
    ///
    /// Returns the close tag's offset, or `None` if the block is still open,
    /// and the length of the body that can be read: up to the close tag, or up
    /// to a tail that could start one.
    pub(super) fn scan(&mut self, content: &str) -> (Option<usize>, usize) {
        let bytes = content.as_bytes();
        let mut at = self.at;
        while at < bytes.len() {
            if self.escaped {
                at += content[at..].chars().next().map_or(1, char::len_utf8);
                self.escaped = false;
                continue;
            }
            if self.in_string {
                // Jumps to the next quote or escape.
                match bytes[at..]
                    .iter()
                    .position(|byte| *byte == b'"' || *byte == b'\\')
                {
                    None => at = bytes.len(),
                    Some(offset) => {
                        if bytes[at + offset] == b'\\' {
                            self.escaped = true;
                        } else {
                            self.in_string = false;
                        }
                        at += offset + 1;
                    }
                }
                continue;
            }
            match bytes[at] {
                b'"' => self.in_string = true,
                b'<' => {
                    if close_tag_at(content, at).is_some() {
                        self.at = at;
                        return (Some(at), at - self.start);
                    }
                    if is_tag_prefix(content, at, CLOSE_TAG) || close_tag_start_at(content, at) {
                        // Scanned again from here once more text arrives.
                        self.at = at;
                        return (None, at - self.start);
                    }
                }
                _ => {}
            }
            at += 1;
        }
        self.at = at;
        (None, bytes.len() - self.start)
    }
}

/// Finds the close tag of a block whose body starts at `start`.
fn scan_block(content: &str, start: usize) -> Option<usize> {
    BlockScanner::new(start).scan(content).0
}

fn is_fence_word(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '-'
}

/// Strips whitespace and one Markdown code fence around a block body.
pub(super) fn clean(text: &str) -> &str {
    let mut cleaned = text.trim();
    if let Some(rest) = cleaned.strip_prefix("```") {
        cleaned = rest.trim_start_matches(is_fence_word).trim_start();
    }
    if let Some(rest) = cleaned.trim_end_matches(is_fence_word).strip_suffix("```") {
        cleaned = rest.trim_end();
    }
    cleaned = cleaned.trim();
    if cleaned.trim_matches('`').is_empty() {
        ""
    } else {
        cleaned
    }
}

/// [`clean`], and the backticks a model leaves when it writes the tags as
/// inline code. No direct JSON payload starts or ends with one.
pub(super) fn clean_raw(text: &str) -> &str {
    clean(text).trim_matches('`').trim()
}

fn add_text(parts: &mut Vec<RawPart>, text: &str) {
    let cleaned = clean(text);
    if !cleaned.is_empty() {
        parts.push(RawPart::Text(cleaned.to_string()));
    }
}

/// Splits a finished answer into its text and its blocks.
///
/// At most one block whose opening tag was omitted is recovered, when a
/// closing tag ends it: it is taken to start at a Markdown code fence, or at
/// the first line that begins a JSON array or object.
pub(super) fn unwrap(content: &str) -> Vec<RawPart> {
    let mut parts = Vec::new();
    let mut at = 0;
    let mut recovered = false;
    while at < content.len() {
        let open = find_open_tag(content, at);
        let search_end = open.map_or(content.len(), |(start, _)| start);
        if !recovered && search_end > at {
            if let Some(next) = recover_unopened(&mut parts, content, at, search_end) {
                recovered = true;
                at = next;
                continue;
            }
        }
        let Some((open_start, open_end)) = open else {
            break;
        };
        add_text(&mut parts, content[at..open_start].trim_end_matches('`'));
        let Some(close_start) = scan_block(content, open_end) else {
            parts.push(RawPart::Block {
                raw: clean_raw(&content[open_end..]).to_string(),
                closed: false,
            });
            return parts;
        };
        parts.push(RawPart::block(clean_raw(&content[open_end..close_start])));
        at = close_tag_at(content, close_start).unwrap_or(content.len());
        at += content[at..].len() - content[at..].trim_start_matches('`').len();
    }
    if !recovered && at < content.len() {
        if let Some(next) = recover_unopened(&mut parts, content, at, content.len()) {
            at = next;
        }
    }
    add_text(&mut parts, &content[at..]);
    parts
}

/// The start of every line of `text`.
fn line_starts(text: &str) -> impl Iterator<Item = usize> + '_ {
    std::iter::once(0).chain(text.match_indices('\n').map(|(at, _)| at + 1))
}

/// A Markdown fence that opens at the line starting at `at`: optional
/// indentation, three backticks, an optional language, and the rest of the
/// line if it is blank. Returns where the fence ends.
fn open_fence_at(text: &str, at: usize) -> Option<usize> {
    let line = &text[at..];
    let rest = line.trim_start_matches([' ', '\t']).strip_prefix("```")?;
    let rest = rest
        .trim_start_matches(|ch: char| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        .trim_start_matches([' ', '\t']);
    let rest = rest.strip_prefix('\r').unwrap_or(rest);
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    Some(text.len() - rest.len())
}

/// Every fence that opens a line of `text`, in order: where it starts and
/// ends.
fn open_fences(text: &str) -> Vec<(usize, usize)> {
    let mut fences = Vec::new();
    let mut after = 0;
    for start in line_starts(text) {
        if start < after {
            continue;
        }
        if let Some(end) = open_fence_at(text, start) {
            fences.push((start, end));
            after = end;
        }
    }
    fences
}

/// `body` without a Markdown fence that closes it.
fn without_closing_fence(body: &str) -> &str {
    match body.trim_end().strip_suffix("```") {
        Some(rest) => rest.trim_end_matches([' ', '\t']),
        None => body,
    }
}

/// The first line at or after `from` that begins a JSON array or object: `[`
/// followed by `{`, `]` or the end of the line, or `{` followed by `"`, `}`
/// or the end of the line.
fn json_start(text: &str, from: usize) -> Option<usize> {
    line_starts(text)
        .filter(|start| *start >= from)
        .find(|start| {
            let line = text[*start..].trim_start_matches([' ', '\t']);
            let mut chars = line.chars();
            let follows: &[char] = match chars.next() {
                Some('[') => &['{', ']'],
                Some('{') => &['"', '}'],
                _ => return false,
            };
            let rest = chars.as_str();
            // The rest of the line is blank, or the next thing written is one of
            // the characters that can follow.
            let line_end = rest.find('\n').unwrap_or(rest.len());
            rest[..line_end].trim().is_empty()
                || rest.trim_start().starts_with(follows)
                || rest.trim_start().is_empty()
        })
}

/// Recovers one block in `content[start..search_end]` whose opening tag the
/// model left out. Returns where reading goes on, after its closing tag.
fn recover_unopened(
    parts: &mut Vec<RawPart>,
    content: &str,
    start: usize,
    search_end: usize,
) -> Option<usize> {
    let segment = &content[start..search_end];
    if !has_close_tag(segment) {
        return None;
    }

    let fences = open_fences(segment);
    for (fence_start, fence_end) in fences.iter().copied().step_by(2) {
        let Some(close_start) = scan_block(segment, fence_end) else {
            continue;
        };
        let body = without_closing_fence(&segment[fence_end..close_start]);
        if !open_fences(body).is_empty() {
            continue;
        }
        let raw = clean_raw(body);
        if raw.is_empty() {
            continue;
        }
        add_text(parts, &segment[..fence_start]);
        parts.push(RawPart::block(raw));
        return Some(start + close_tag_at(segment, close_start)?);
    }

    // Prose fences come in pairs. A JSON start is looked for after the last
    // pair that closed before a close tag.
    let mut search_from = 0;
    for pair in fences.chunks_exact(2) {
        if scan_block(segment, pair[1].1).is_none() {
            break;
        }
        search_from = pair[1].1;
    }

    let payload_start = json_start(segment, search_from)?;
    let close_start = scan_block(segment, payload_start)?;
    let raw = clean_raw(&segment[payload_start..close_start]);
    if raw.is_empty() {
        return None;
    }
    add_text(parts, &segment[..payload_start]);
    parts.push(RawPart::block(raw));
    Some(start + close_tag_at(segment, close_start)?)
}

/// Writes parts back out as a model would have written them.
pub(super) fn wrap(parts: &[RawPart]) -> String {
    parts
        .iter()
        .map(|part| match part {
            RawPart::Text(text) => text.clone(),
            RawPart::Block { raw, .. } => format!("{OPEN_TAG}\n{raw}\n{CLOSE_TAG}"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Where the payload of a block still arriving starts: after leading
/// whitespace and after the line of a leading Markdown fence.
///
/// `None` while nothing but the start of a fence has arrived, or while the
/// fence's line is not complete. `end` is where the part of `block` that can
/// be read ends.
pub(super) fn payload_start(block: &str, end: usize) -> Option<usize> {
    let readable = &block[..end];
    let mut start = readable.len() - readable.trim_start().len();
    let rest = &readable[start..];
    if rest.len() <= 3 && "```".starts_with(rest) {
        return None;
    }
    if rest.starts_with("```") {
        return rest.find('\n').map(|newline| start + newline + 1);
    }
    if rest.starts_with('`') {
        start += rest.len()
            - rest
                .trim_start_matches(|ch: char| ch == '`' || ch.is_whitespace())
                .len();
    }
    Some(start)
}

/// Where a Markdown fence that ends `block[start..end]` begins, else `end`.
///
/// A fence still arriving may have fewer than three backticks. It is found by
/// reading back from the end.
pub(super) fn trailing_fence_start(block: &str, start: usize, end: usize) -> usize {
    let payload = &block[start..end];
    let before_word = payload.trim_end().trim_end_matches(is_fence_word);
    let mut before_ticks = before_word;
    for _ in 0..3 {
        match before_ticks.strip_suffix('`') {
            Some(rest) => before_ticks = rest,
            None => break,
        }
    }
    if before_ticks.len() == before_word.len() {
        return end;
    }
    start + before_ticks.trim_end().len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(text: &str) -> RawPart {
        RawPart::Text(text.to_string())
    }

    #[test]
    fn tags_are_matched_whatever_their_case_and_attributes() {
        assert_eq!(open_tag_at("<a2ui-json>x", 0), Some(11));
        assert_eq!(open_tag_at("<A2UI-JSON v=\"1\">x", 0), Some(17));
        assert_eq!(open_tag_at("<a2ui-jsonx>", 0), None);
        assert_eq!(open_tag_at("<a2ui-json", 0), None);
        assert_eq!(close_tag_at("</a2ui-json >", 0), Some(13));
        assert_eq!(close_tag_at("</a2ui-json x>", 0), None);
        assert_eq!(find_open_tag("a < b <a2ui-json>", 0), Some((6, 17)));
    }

    #[test]
    fn a_tail_that_could_start_a_tag_is_found() {
        assert_eq!(possible_tag_start("text <a2"), 5);
        assert_eq!(possible_tag_start("text <a2ui-json attr"), 5);
        assert_eq!(possible_tag_start("a < b"), 5);
        assert_eq!(possible_tag_start("text <b>"), 8);
    }

    #[test]
    fn a_close_tag_inside_a_string_does_not_end_the_block() {
        let content = r#"[{"text": "</a2ui-json>", "more": "a\"b"}]</a2ui-json>"#;
        assert_eq!(scan_block(content, 0), Some(42));
    }

    #[test]
    fn the_scanner_goes_on_where_it_stopped() {
        let mut scanner = BlockScanner::new(0);
        let mut content = String::from(r#"[{"text": "a\"#);
        assert_eq!(scanner.scan(&content), (None, content.len()));
        // The escaped quote arrives in the next chunk, and stays in the string.
        content.push_str(r#"" still a string </a2ui-json>"}]</a2ui"#);
        let (close, readable) = scanner.scan(&content);
        assert_eq!(close, None);
        assert_eq!(&content[readable..], "</a2ui");
        content.push_str("-json>");
        assert_eq!(scanner.scan(&content).0, Some(readable));
    }

    #[test]
    fn cleaning_strips_one_fence_and_text_made_of_backticks() {
        assert_eq!(clean("  ```json\n[1]\n```  "), "[1]");
        assert_eq!(clean("```\n[1]"), "[1]");
        assert_eq!(clean("[1]\n```json"), "[1]");
        assert_eq!(clean("``"), "");
        assert_eq!(clean("Use `Text`"), "Use `Text`");
        assert_eq!(clean_raw("`[1]`"), "[1]");
    }

    #[test]
    fn unwrapping_keeps_text_and_blocks_in_order() {
        assert_eq!(
            unwrap(
                "Before\n<a2ui-json>[1]</a2ui-json>\n\n<a2ui-json>\n```json\n[2]\n```\n</a2ui-json> after"
            ),
            [
                text("Before"),
                RawPart::block("[1]"),
                RawPart::block("[2]"),
                text("after"),
            ]
        );
        assert_eq!(unwrap(""), []);
        assert_eq!(unwrap("just text"), [text("just text")]);
        assert_eq!(
            unwrap("`<a2ui-json>`\n[1]\n`</a2ui-json>`"),
            [RawPart::block("[1]")]
        );
    }

    #[test]
    fn a_block_left_open_is_not_closed() {
        assert_eq!(
            unwrap("Here.\n<a2ui-json>[1"),
            [
                text("Here."),
                RawPart::Block {
                    raw: "[1".into(),
                    closed: false
                }
            ]
        );
    }

    #[test]
    fn one_block_without_its_opening_tag_is_recovered() {
        assert_eq!(
            unwrap("Here it is.\n```json\n[1]\n</a2ui-json>\n```"),
            [text("Here it is."), RawPart::block("[1]")]
        );
        assert_eq!(
            unwrap("Here it is.\n```json\n[1]\n```\n</a2ui-json>"),
            [text("Here it is."), RawPart::block("[1]")]
        );
        assert_eq!(
            unwrap("Sure (see [v1]):\n[{\"a\": 1}]\n</a2ui-json>"),
            [text("Sure (see [v1]):"), RawPart::block("[{\"a\": 1}]")]
        );
        assert_eq!(
            unwrap("Close every block with </a2ui-json> when done."),
            [text("Close every block with </a2ui-json> when done.")]
        );
    }

    #[test]
    fn wrapping_writes_the_tags_back() {
        let parts = [text("Here."), RawPart::block("[1]")];
        assert_eq!(wrap(&parts), "Here.\n<a2ui-json>\n[1]\n</a2ui-json>");
        assert_eq!(unwrap(&wrap(&parts)), parts);
        assert_eq!(wrap(&[]), "");
    }

    #[test]
    fn a_payload_starts_after_a_fence_line_once_the_line_is_whole() {
        assert_eq!(payload_start("  [1", 4), Some(2));
        assert_eq!(payload_start("``", 2), None);
        assert_eq!(payload_start("", 0), None);
        assert_eq!(payload_start("```json", 7), None);
        assert_eq!(payload_start("```json\n[1", 10), Some(8));
        assert_eq!(payload_start("` [1", 4), Some(2));
    }

    #[test]
    fn a_fence_that_ends_a_payload_is_left_out_of_it() {
        assert_eq!(trailing_fence_start("[1]\n```", 0, 7), 3);
        assert_eq!(trailing_fence_start("[1]\n``", 0, 6), 3);
        assert_eq!(trailing_fence_start("[1]`", 0, 4), 3);
        assert_eq!(trailing_fence_start("[true", 0, 5), 5);
        assert_eq!(trailing_fence_start("[1]", 0, 3), 3);
    }
}
