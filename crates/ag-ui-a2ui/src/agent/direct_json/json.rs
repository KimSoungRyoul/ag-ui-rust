//! Reads the JSON a model writes, which is often almost JSON.
//!
//! Both readers accept what a model commonly gets wrong and a strict decoder
//! rejects: a trailing comma before `]` or `}`, and a raw line break or tab
//! inside a string. When the text does not read as it is, they read it again
//! with curly quotes straightened, so a curly quote inside a string that is
//! otherwise valid JSON is kept.
//!
//! [`read_json`] reads a complete document. [`read_partial`] reads the start
//! of one, for a block that is still arriving: it closes the brackets the text
//! has open, and ends a string early when the key it belongs to is one of the
//! caller's progressive keys.
//!
//! Neither is recursive beyond [`MAX_NESTING`]: the text is model-generated,
//! and nesting is the one thing in it that could exhaust the stack.

use std::collections::BTreeSet;

use serde_json::{Map, Number, Value};

use crate::agent::format::Candidate;
use crate::protocol::binding::MAX_VALUE_DEPTH;

/// Deepest nesting either reader follows.
const MAX_NESTING: usize = MAX_VALUE_DEPTH;

/// Keys whose string value is an identifier: a raw line break inside one is
/// where the model's output was wrapped, not part of the name.
const NEWLINE_STRIPPED_KEYS: [&str; 7] = [
    "id",
    "component",
    "child",
    "surfaceId",
    "catalogId",
    "@catalogId",
    "componentId",
];

/// A string that starts like this names a resource. Cut short it names
/// another one, so it is never ended early.
const URL_PREFIXES: [&str; 3] = ["http://", "https://", "data:"];

/// Reads the complete JSON document `text`.
///
/// # Errors
///
/// Returns why `text` is not a single JSON value.
pub(super) fn read_json(text: &str) -> Result<Value, String> {
    let read = |text: &str| match Reader::new(text, None).document() {
        Ok(value) => Ok(value),
        Err(Stop::Invalid(reason)) => Err(reason),
        Err(Stop::Unfinished) => Err("unexpected end of the JSON text".to_string()),
    };
    read(text).or_else(|reason| {
        let straight = straighten_quotes(text);
        if straight == text {
            Err(reason)
        } else {
            read(&straight)
        }
    })
}

/// What a partial read heals.
#[derive(Debug, Clone, Default)]
pub(super) struct Healing {
    /// Keys whose string value is ended early while it arrives.
    pub(super) progressive_keys: BTreeSet<String>,
    /// Keys whose list value holds only the items that have closed.
    pub(super) whole_item_keys: BTreeSet<String>,
}

/// Reads the start of a payload, a list of messages or one message, into the
/// messages it holds so far. Nothing comes back for text that is not the
/// start of a JSON payload.
pub(super) fn read_partial(text: &str, healing: &Healing) -> Vec<Candidate> {
    let read = |text: &str| Reader::new(text, Some(healing)).message_items();
    match read(text) {
        Ok(messages) => messages,
        Err(_) => {
            let straight = straighten_quotes(text);
            if straight == text {
                Vec::new()
            } else {
                read(&straight).unwrap_or_default()
            }
        }
    }
}

fn straighten_quotes(text: &str) -> String {
    text.replace(['\u{201c}', '\u{201d}'], "\"")
        .replace(['\u{2018}', '\u{2019}'], "'")
}

/// Why a read stopped.
#[derive(Debug)]
enum Stop {
    /// The text ended before a value that cannot be healed.
    Unfinished,
    /// The text is not JSON.
    Invalid(String),
}

type Read<T> = Result<T, Stop>;

/// A recursive-descent reader of JSON and of the start of JSON.
struct Reader<'a> {
    text: &'a str,
    bytes: &'a [u8],
    at: usize,
    /// `None` reads a complete document; `Some` reads text that may end
    /// anywhere.
    healing: Option<&'a Healing>,
    /// Whether the value last read was closed for the text rather than by it.
    cut: bool,
    /// Whether the read is inside a data model value, where keys and strings
    /// are the application's and nothing is an identifier.
    in_data_model: bool,
    depth: usize,
}

impl<'a> Reader<'a> {
    fn new(text: &'a str, healing: Option<&'a Healing>) -> Self {
        Self {
            text,
            bytes: text.as_bytes(),
            at: 0,
            healing,
            cut: false,
            in_data_model: false,
            depth: 0,
        }
    }

    fn at_end(&self) -> bool {
        self.at >= self.bytes.len()
    }

    fn skip_space(&mut self) {
        while !self.at_end() && matches!(self.bytes[self.at], b' ' | b'\t' | b'\n' | b'\r') {
            self.at += 1;
        }
    }

    fn fail<T>(&self, reason: impl std::fmt::Display) -> Read<T> {
        Err(Stop::Invalid(format!(
            "invalid JSON at {}: {reason}",
            self.at
        )))
    }

    /// The text ended where a value was expected.
    fn end<T>(&self) -> Read<T> {
        if self.healing.is_some() {
            Err(Stop::Unfinished)
        } else {
            self.fail("unexpected end of the JSON text")
        }
    }

    /// Closes a list, a map or a string the text left open.
    fn healed<T>(&mut self, value: T) -> Read<T> {
        if self.healing.is_none() {
            return self.end();
        }
        self.cut = true;
        Ok(value)
    }

    fn enter(&mut self) -> Read<()> {
        self.depth += 1;
        if self.depth > MAX_NESTING {
            return self.fail(format!("nesting deeper than {MAX_NESTING}"));
        }
        Ok(())
    }

    fn document(&mut self) -> Read<Value> {
        self.skip_space();
        let value = self.value(false, false)?;
        self.skip_space();
        if !self.at_end() {
            return self.fail("unexpected text after the JSON value");
        }
        Ok(value)
    }

    fn message_items(&mut self) -> Read<Vec<Candidate>> {
        self.skip_space();
        if self.at_end() {
            return Ok(Vec::new());
        }
        if self.bytes[self.at] != b'[' {
            self.cut = false;
            return match self.value(false, false) {
                Ok(message) => {
                    let closed = !self.cut;
                    self.skip_space();
                    if !self.at_end() {
                        return self.fail("unexpected text after the JSON value");
                    }
                    Ok(vec![Candidate { message, closed }])
                }
                Err(Stop::Unfinished) => Ok(Vec::new()),
                Err(invalid) => Err(invalid),
            };
        }
        self.at += 1;
        let mut messages = Vec::new();
        let mut after_item = false;
        loop {
            if after_item {
                self.skip_space();
                if self.at_end() {
                    return Ok(messages);
                }
                match self.bytes[self.at] {
                    b',' => self.at += 1,
                    b']' => {}
                    _ => return self.fail("expected ',' or ']'"),
                }
            }
            after_item = true;
            self.skip_space();
            if self.at_end() {
                return Ok(messages);
            }
            if self.bytes[self.at] == b']' {
                self.at += 1;
                self.skip_space();
                if !self.at_end() {
                    return self.fail("unexpected text after the JSON value");
                }
                return Ok(messages);
            }
            self.cut = false;
            match self.value(false, false) {
                Ok(message) => messages.push(Candidate {
                    message,
                    closed: !self.cut,
                }),
                Err(Stop::Unfinished) => return Ok(messages),
                Err(invalid) => return Err(invalid),
            }
        }
    }

    fn value(&mut self, healable: bool, strip_newlines: bool) -> Read<Value> {
        if self.at_end() {
            return self.end();
        }
        match self.bytes[self.at] {
            b'{' => self.object(None),
            b'[' => self.array(false, false),
            b'"' => self.string(healable, strip_newlines).map(Value::String),
            b'-' | b'0'..=b'9' => self.number(),
            _ => {
                let rest = &self.bytes[self.at..];
                for (word, value) in [
                    ("true", Value::Bool(true)),
                    ("false", Value::Bool(false)),
                    ("null", Value::Null),
                ] {
                    if rest.starts_with(word.as_bytes()) {
                        self.at += word.len();
                        return Ok(value);
                    }
                    if self.healing.is_some() && word.as_bytes().starts_with(rest) {
                        return self.end();
                    }
                }
                let found = self.text[self.at..].chars().next().unwrap_or_default();
                self.fail(format!("unexpected character '{found}'"))
            }
        }
    }

    fn object(&mut self, parent_key: Option<&str>) -> Read<Value> {
        self.enter()?;
        self.at += 1;
        let mut object = Map::new();
        loop {
            self.skip_space();
            if self.at_end() {
                self.depth -= 1;
                return self.healed(Value::Object(object));
            }
            match self.bytes[self.at] {
                b'}' => {
                    self.at += 1;
                    self.depth -= 1;
                    return Ok(Value::Object(object));
                }
                b'"' => {}
                _ => return self.fail("expected a key"),
            }
            let key = self.string(false, !self.in_data_model)?;
            self.skip_space();
            if self.at_end() {
                return self.end();
            }
            if self.bytes[self.at] != b':' {
                return self.fail(format!("expected ':' after key '{key}'"));
            }
            self.at += 1;
            self.skip_space();

            let is_data_value = !self.in_data_model
                && matches!(
                    (parent_key, key.as_str()),
                    (Some("updateDataModel"), "value") | (Some("createSurface"), "dataModel")
                );
            if is_data_value {
                self.in_data_model = true;
            }
            let next = self.bytes.get(self.at).copied();
            let value = match next {
                Some(b'[') => {
                    let whole_items = self
                        .healing
                        .is_some_and(|healing| healing.whole_item_keys.contains(&key));
                    self.array(whole_items, key == "children" && !self.in_data_model)
                }
                Some(b'{') => self.object(Some(&key)),
                _ => {
                    let healable = self
                        .healing
                        .is_some_and(|healing| healing.progressive_keys.contains(&key));
                    let identifier =
                        NEWLINE_STRIPPED_KEYS.contains(&key.as_str()) && !self.in_data_model;
                    self.value(healable, identifier)
                }
            };
            if is_data_value {
                self.in_data_model = false;
            }
            object.insert(key, value?);

            self.skip_space();
            if self.at_end() {
                self.depth -= 1;
                return self.healed(Value::Object(object));
            }
            match self.bytes[self.at] {
                b',' => self.at += 1,
                b'}' => {}
                _ => return self.fail("expected ',' or '}'"),
            }
        }
    }

    fn array(&mut self, whole_items: bool, strip_item_newlines: bool) -> Read<Value> {
        self.enter()?;
        self.at += 1;
        let mut items = Vec::new();
        let mut after_item = false;
        loop {
            if after_item {
                self.skip_space();
                if self.at_end() {
                    self.depth -= 1;
                    return self.healed(Value::Array(items));
                }
                match self.bytes[self.at] {
                    b',' => self.at += 1,
                    b']' => {}
                    _ => return self.fail("expected ',' or ']'"),
                }
            }
            after_item = true;
            self.skip_space();
            if self.at_end() {
                self.depth -= 1;
                return self.healed(Value::Array(items));
            }
            if self.bytes[self.at] == b']' {
                self.at += 1;
                self.depth -= 1;
                return Ok(Value::Array(items));
            }
            if whole_items {
                // An item still arriving is left out, and the list ends at
                // the last one that closed.
                let cut_before = std::mem::replace(&mut self.cut, false);
                match self.value(false, strip_item_newlines) {
                    Ok(item) if !self.cut => {
                        self.cut = cut_before;
                        items.push(item);
                    }
                    Ok(_) | Err(Stop::Unfinished) => {
                        self.depth -= 1;
                        return self.healed(Value::Array(items));
                    }
                    Err(invalid) => return Err(invalid),
                }
            } else {
                items.push(self.value(false, strip_item_newlines)?);
            }
        }
    }

    fn string(&mut self, healable: bool, strip_newlines: bool) -> Read<String> {
        self.at += 1;
        let end = self.bytes.len();
        let mut buffer = String::new();
        while self.at < end {
            let byte = self.bytes[self.at];
            if byte == b'"' {
                self.at += 1;
                return Ok(buffer);
            }
            if byte != b'\\' {
                if strip_newlines && matches!(byte, b'\r' | b'\n') {
                    self.at += 1;
                    continue;
                }
                // Takes the run of plain characters up to the next quote or
                // escape at once.
                let run = self.bytes[self.at..]
                    .iter()
                    .position(|byte| {
                        matches!(byte, b'"' | b'\\')
                            || (strip_newlines && matches!(byte, b'\r' | b'\n'))
                    })
                    .map_or(end, |offset| self.at + offset);
                buffer.push_str(&self.text[self.at..run]);
                self.at = run;
                continue;
            }
            let Some(escaped) = self.text[self.at + 1..].chars().next() else {
                self.at = end;
                break;
            };
            match escaped {
                '"' | '\\' | '/' => buffer.push(escaped),
                'b' => buffer.push('\u{8}'),
                'f' => buffer.push('\u{c}'),
                'n' => buffer.push('\n'),
                'r' => buffer.push('\r'),
                't' => buffer.push('\t'),
                'u' => {
                    if self.at + 6 > end {
                        self.at = end;
                        continue;
                    }
                    let mut code = self.hex4(self.at + 2)?;
                    if (0xD800..=0xDBFF).contains(&code) {
                        let tail = &self.bytes[self.at + 6..end.min(self.at + 12)];
                        if self.at + 12 > end
                            && self.healing.is_some()
                            && (b"\\u".starts_with(tail) || tail.starts_with(b"\\u"))
                        {
                            // The other half of the pair has not arrived.
                            self.at = end;
                            continue;
                        }
                        if self.at + 12 <= end && tail.starts_with(b"\\u") {
                            let low = self.hex4(self.at + 8)?;
                            if (0xDC00..=0xDFFF).contains(&low) {
                                code = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                                self.at += 6;
                            }
                        }
                    }
                    // Half a pair on its own is not a character.
                    buffer.push(char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER));
                    self.at += 4;
                }
                // What a model most often means by this is a backslash of
                // its own, in LaTeX or a Windows path. Say so: the error is
                // read by the model that has to write the answer again.
                other => {
                    return self.fail(format!(
                        "invalid escape '\\{other}'. Unescaped backslash found: inside a JSON \
                         string a backslash is written '\\\\', as in '\\\\{other}'"
                    ));
                }
            }
            self.at += 2;
        }
        if healable && self.healing.is_some() {
            // A URL that is still arriving names the wrong resource, so it
            // waits for its closing quote like any value that cannot heal.
            if !URL_PREFIXES.iter().any(|prefix| buffer.starts_with(prefix)) {
                self.cut = true;
                return Ok(buffer);
            }
        }
        self.end()
    }

    fn hex4(&self, at: usize) -> Read<u32> {
        self.text
            .get(at..at + 4)
            .filter(|digits| digits.bytes().all(|digit| digit.is_ascii_hexdigit()))
            .and_then(|digits| u32::from_str_radix(digits, 16).ok())
            .map_or_else(|| self.fail("invalid unicode escape"), Ok)
    }

    fn number(&mut self) -> Read<Value> {
        let start = self.at;
        if self.bytes[self.at] == b'-' {
            self.at += 1;
        }
        while !self.at_end()
            && (self.bytes[self.at].is_ascii_digit()
                || matches!(self.bytes[self.at], b'+' | b'-' | b'.' | b'e' | b'E'))
        {
            self.at += 1;
        }
        if self.at_end() && self.healing.is_some() {
            // More digits may follow.
            return self.end();
        }
        let literal = &self.text[start..self.at];
        let number = if is_json_number(literal) {
            literal
                .parse::<Number>()
                .ok()
                .or_else(|| literal.parse::<f64>().ok().and_then(Number::from_f64))
        } else {
            None
        };
        match number {
            Some(number) => Ok(Value::Number(number)),
            None => self.fail(format!("invalid number '{literal}'")),
        }
    }
}

/// Whether `literal` is a number as JSON writes one:
/// `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`.
fn is_json_number(literal: &str) -> bool {
    let digits = |text: &str| {
        text.len()
            - text
                .trim_start_matches(|ch: char| ch.is_ascii_digit())
                .len()
    };
    let rest = literal.strip_prefix('-').unwrap_or(literal);
    let whole = digits(rest);
    if whole == 0 || (whole > 1 && rest.starts_with('0')) {
        return false;
    }
    let mut rest = &rest[whole..];
    if let Some(fraction) = rest.strip_prefix('.') {
        let count = digits(fraction);
        if count == 0 {
            return false;
        }
        rest = &fraction[count..];
    }
    if let Some(exponent) = rest.strip_prefix(['e', 'E']) {
        let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
        let count = digits(exponent);
        if count == 0 {
            return false;
        }
        rest = &exponent[count..];
    }
    rest.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn healing(progressive: &[&str], whole: &[&str]) -> Healing {
        Healing {
            progressive_keys: progressive.iter().map(|key| (*key).to_string()).collect(),
            whole_item_keys: whole.iter().map(|key| (*key).to_string()).collect(),
        }
    }

    fn partial(text: &str, progressive: &[&str]) -> Vec<(Value, bool)> {
        read_partial(text, &healing(progressive, &[]))
            .into_iter()
            .map(|candidate| (candidate.message, candidate.closed))
            .collect()
    }

    #[test]
    fn a_complete_document_reads_as_json_does() {
        let text = r#"{"a": [1, -2.5, 3e2, true, false, null], "b": {"c": "d\né😀"}}"#;
        assert_eq!(
            read_json(text).unwrap(),
            serde_json::from_str::<Value>(text).unwrap()
        );
    }

    #[test]
    fn what_a_model_gets_wrong_is_repaired() {
        assert_eq!(read_json("[1, 2,]").unwrap(), json!([1, 2]));
        assert_eq!(read_json(r#"{"a": 1,}"#).unwrap(), json!({"a": 1}));
        assert_eq!(
            read_json("{\"text\": \"two\nlines\ttabbed\"}").unwrap(),
            json!({"text": "two\nlines\ttabbed"})
        );
        assert_eq!(
            read_json("{\u{201c}a\u{201d}: \u{201c}b\u{201d}}").unwrap(),
            json!({"a": "b"})
        );
        // A curly quote inside a string that reads as it is stays.
        assert_eq!(
            read_json("{\"a\": \"\u{201e}b\u{201c}\"}").unwrap(),
            json!({"a": "\u{201e}b\u{201c}"})
        );
    }

    #[test]
    fn a_line_break_inside_an_identifier_is_where_the_output_wrapped() {
        assert_eq!(
            read_json("{\"surface\nId\": \"s\n1\", \"text\": \"a\nb\", \"children\": [\"x\ny\"]}")
                .unwrap(),
            json!({"surfaceId": "s1", "text": "a\nb", "children": ["xy"]})
        );
        // A data model is the application's: nothing in it is an identifier.
        assert_eq!(
            read_json("{\"updateDataModel\": {\"value\": {\"id\": \"a\nb\", \"k\ney\": 1}}}")
                .unwrap(),
            json!({"updateDataModel": {"value": {"id": "a\nb", "k\ney": 1}}})
        );
    }

    #[test]
    fn text_that_is_not_json_says_where() {
        for (text, reason) in [
            ("here is your surface", "unexpected character 'h'"),
            ("[1 2]", "expected ',' or ']'"),
            ("{\"a\" 1}", "expected ':' after key 'a'"),
            ("{a: 1}", "expected a key"),
            ("[1] trailing", "unexpected text after the JSON value"),
            ("[1", "unexpected end of the JSON text"),
            ("\"a\\qb\"", "invalid escape '\\q'"),
            (
                r#"{"formula": "\approx \Delta x"}"#,
                r"Unescaped backslash found: inside a JSON string a backslash is written '\\', as in '\\a'",
            ),
            ("[01]", "invalid number '01'"),
            ("[1.]", "invalid number '1.'"),
            ("", "unexpected end of the JSON text"),
        ] {
            let error = read_json(text).unwrap_err();
            assert!(error.contains(reason), "{text}: {error}");
        }
    }

    #[test]
    fn nesting_is_bounded() {
        let deep = "[".repeat(MAX_NESTING + 1);
        assert!(read_json(&deep).unwrap_err().contains("nesting deeper"));
        // The list of messages is not nesting: its bracket is the payload's.
        let deeper = "[".repeat(MAX_NESTING + 2);
        assert_eq!(read_partial(&deeper, &Healing::default()), []);
    }

    #[test]
    fn a_partial_read_closes_what_the_text_left_open() {
        assert_eq!(
            partial(r#"[{"a": {"b": [1, 2"#, &[]),
            // The number may have more digits, so its message waits.
            []
        );
        assert_eq!(
            partial(r#"[{"a": {"b": [1, 2,"#, &[]),
            [(json!({"a": {"b": [1, 2]}}), false)]
        );
        assert_eq!(
            partial(r#"[{"a": 1}, {"b": 2}"#, &[]),
            [(json!({"a": 1}), true), (json!({"b": 2}), true)]
        );
        assert_eq!(partial(r#"{"a": 1"#, &[]), []);
        assert_eq!(partial(r#"{"a": 1,"#, &[]), [(json!({"a": 1}), false)]);
        assert_eq!(partial("", &[]), []);
        assert_eq!(partial("[", &[]), []);
    }

    #[test]
    fn only_a_string_under_a_progressive_key_ends_early() {
        let text = r#"[{"id": "root", "text": "Hel"#;
        assert_eq!(partial(text, &[]), []);
        assert_eq!(
            partial(text, &["text"]),
            [(json!({"id": "root", "text": "Hel"}), false)]
        );
        // An id is not prose, and neither is a key.
        assert_eq!(partial(r#"[{"id": "ro"#, &["text"]), []);
        assert_eq!(partial(r#"[{"id": "root", "te"#, &["text"]), []);
        // A URL cut short is another URL.
        assert_eq!(partial(r#"[{"text": "https://exa"#, &["text"]), []);
        // A literal still arriving waits.
        assert_eq!(partial(r#"[{"a": tr"#, &[]), []);
    }

    #[test]
    fn an_escape_cut_in_half_is_left_out_of_a_healed_string() {
        assert_eq!(
            partial(r#"[{"text": "a\"#, &["text"]),
            [(json!({"text": "a"}), false)]
        );
        assert_eq!(
            partial(r#"[{"text": "a\u00"#, &["text"]),
            [(json!({"text": "a"}), false)]
        );
        // Half a surrogate pair waits for the other half.
        for cut in [
            r#"[{"text": "a\ud83d"#,
            r#"[{"text": "a\ud83d\"#,
            r#"[{"text": "a\ud83d\ude"#,
        ] {
            assert_eq!(partial(cut, &["text"]), [(json!({"text": "a"}), false)]);
        }
        assert_eq!(
            partial(r#"[{"text": "a😀"#, &["text"]),
            [(json!({"text": "a\u{1f600}"}), false)]
        );
    }

    #[test]
    fn a_whole_item_list_holds_only_the_items_that_closed() {
        let healing = healing(&["text"], &["components"]);
        let read = |text: &str| -> Vec<(Value, bool)> {
            read_partial(text, &healing)
                .into_iter()
                .map(|candidate| (candidate.message, candidate.closed))
                .collect()
        };
        assert_eq!(
            read(r#"[{"components": [{"id": "a", "text": "x"}, {"id": "b", "text": "y"#),
            [(json!({"components": [{"id": "a", "text": "x"}]}), false)]
        );
        assert_eq!(
            read(r#"[{"components": [{"id": "a", "text": "x"#),
            [(json!({"components": []}), false)]
        );
    }

    #[test]
    fn text_that_is_not_the_start_of_json_reads_as_nothing() {
        assert_eq!(partial("here is your surface", &[]), []);
        assert_eq!(partial("[1 2", &[]), []);
        assert_eq!(
            partial("[{\u{201c}a\u{201d}: 1},", &[]),
            [(json!({"a": 1}), true)]
        );
    }
}
