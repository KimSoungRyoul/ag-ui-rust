//! The `formatString` template grammar: one parser, which
//! [`crate::binding`] makes public and evaluates through.
//!
//! This file is private. Its items are public as `ag_ui_a2ui::binding::…`,
//! which is their one path.

use serde_json::{Map, Number, Value};

use super::version::A2uiVersion;
use crate::error::{Error, Result};

/// Longest template [`parse_template`] reads, in characters.
///
/// A template is model output, and the work of parsing one grows with its
/// length. Every A2UI toolkit refuses a longer one, so a template accepted
/// here is accepted by a renderer.
pub const MAX_TEMPLATE_CHARS: usize = 10_000;

/// Most parts one template may have.
pub const MAX_TEMPLATE_PARTS: usize = 1_000;

/// Deepest nesting of one expression. The expression of an interpolation is at
/// level one; an interpolation written inside it, and each argument of a call,
/// is one level further down.
pub const MAX_TEMPLATE_DEPTH: usize = 100;

/// One part of a parsed `formatString` template.
///
/// A template is a sequence of these: the text between interpolations, and
/// what each `${…}` holds. An argument of a call is a part as well.
///
/// Non-exhaustive, so that a kind of part can be added without breaking a
/// `match` written outside this crate: such a `match` needs a wildcard arm.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum TemplatePart {
    /// A value written out: the text between interpolations, or a quoted
    /// string, a number, `true` or `false` inside one.
    Literal(Value),
    /// A data-model path as written: absolute when it starts with `/`,
    /// otherwise relative to the scope the template is evaluated in.
    Path(String),
    /// A function call.
    Call {
        /// The function's name, with its `@` for a system function such as
        /// `@index`.
        name: String,
        /// The named arguments, in the order written. A name written twice
        /// keeps its first place and its last value.
        args: Vec<(String, TemplatePart)>,
    },
}

impl TemplatePart {
    /// The part as the `DynamicValue` a component would carry in its place: a
    /// literal as itself, a path as `{"path": …}`, and a call as
    /// `{"call": …, "args": {…}, "returnType": "any"}`.
    ///
    /// [`Scope::resolve_dynamic`](crate::binding::Scope::resolve_dynamic)
    /// evaluates the result.
    ///
    /// That is the spelling of the v0.9 family. [`TemplatePart::to_json_in`]
    /// writes the part as another version does.
    pub fn to_json(&self) -> Value {
        self.to_json_in(A2uiVersion::DEFAULT)
    }

    /// The part as the `DynamicValue` a component of `version` would carry in
    /// its place.
    ///
    /// Before v1.0 a path is `{"path": …}` and a call is
    /// `{"call": …, "args": {…}, "returnType": "any"}`. In v1.0 they are
    /// `{"@path": …}` and `{"@call": …, "args": {…}}`: a v1.0 call states no
    /// return type on the wire.
    ///
    /// ```
    /// use ag_ui_a2ui::A2uiVersion;
    /// use ag_ui_a2ui::binding::parse_template;
    /// use serde_json::json;
    ///
    /// let parts = parse_template("${/name} is #${@index(offset: 1)}")?;
    /// assert_eq!(parts[0].to_json_in(A2uiVersion::V1_0), json!({"@path": "/name"}));
    /// assert_eq!(
    ///     parts[2].to_json_in(A2uiVersion::V1_0),
    ///     json!({"@call": "@index", "args": {"offset": 1}})
    /// );
    /// # Ok::<(), ag_ui_a2ui::Error>(())
    /// ```
    pub fn to_json_in(&self, version: A2uiVersion) -> Value {
        match self {
            Self::Literal(value) => value.clone(),
            Self::Path(path) => version.path(path.as_str()),
            Self::Call { name, args } => {
                let args: Map<String, Value> = args
                    .iter()
                    .map(|(name, value)| (name.clone(), value.to_json_in(version)))
                    .collect();
                let mut call = version.call(name.as_str(), Value::Object(args));
                // The v0.9 family states what a call returns; v1.0 leaves it
                // to the catalog.
                if !version.reserves_at_keys() {
                    call["returnType"] = Value::String("any".to_string());
                }
                call
            }
        }
    }
}

/// Parses a `formatString` template into its parts, without evaluating it.
///
/// ```text
/// template   = *( text / "\${" / "${" expression "}" )
/// expression = "${" expression "}" / string / number / "true" / "false" / "null"
///            / call / path
/// call       = name "(" [ argument *( "," argument ) [ "," ] ] ")"
/// argument   = name ":" expression
/// path       = 1*( XID_Continue / "/" / "." / "-" / "~0" / "~1" )
/// ```
///
/// - Text outside `${…}` is literal, and `\${` is a literal `${`. Adjacent
///   literal strings are one part, and an empty one is no part.
/// - A string is quoted with `'` or `"`. In it `\n`, `\t` and `\r` are the
///   control characters, and a backslash before any other character is that
///   character.
/// - A number is an optional sign, digits with at most one decimal point, and
///   an optional exponent: `42`, `-3.14`, `.5`, `1.`, `2.5E+4`. Written with no
///   point and no exponent it is an integer. One outside the range of a double
///   is an error.
/// - `null` is the empty string.
/// - A name is made of Unicode `XID_Continue` characters. A system function
///   starts with `@` and is ASCII: `@index`. A path is a name that may also
///   hold `/`, `.` and `-`, and the two JSON Pointer escapes: `~0` for a `~`
///   in a key and `~1` for a `/`. A `~` before anything else ends the path.
/// - Whitespace around an expression, an argument or a name is ignored.
///
/// This is the grammar the reference renderers parse, with one addition: the
/// escapes. A binding is a JSON Pointer everywhere else in the protocol, and
/// without them a template could not name a key that holds `~` or `/`. So a
/// template the reference parses, parses here; one that uses an escape is for
/// this crate alone. [`Scope::format_string`] evaluates through this parser.
///
/// ```
/// use ag_ui_a2ui::binding::{TemplatePart, parse_template};
/// use serde_json::json;
///
/// let parts = parse_template("Total: ${formatCurrency(value: /price, currency: 'USD')} for ${name}")?;
/// assert_eq!(
///     parts,
///     [
///         TemplatePart::Literal(json!("Total: ")),
///         TemplatePart::Call {
///             name: "formatCurrency".into(),
///             args: vec![
///                 ("value".into(), TemplatePart::Path("/price".into())),
///                 ("currency".into(), TemplatePart::Literal(json!("USD"))),
///             ],
///         },
///         TemplatePart::Literal(json!(" for ")),
///         TemplatePart::Path("name".into()),
///     ]
/// );
/// // A part is the value a component would carry in its place.
/// assert_eq!(parts[3].to_json(), json!({"path": "name"}));
///
/// assert!(parse_template("Hello ${/user/name").is_err());
/// # Ok::<(), ag_ui_a2ui::Error>(())
/// ```
///
/// # Errors
///
/// Returns [`Error::Binding`] for a template that does not parse: an
/// interpolation that is not closed, characters left over after an
/// expression, a malformed number or call, a template longer than
/// [`MAX_TEMPLATE_CHARS`] or of more than [`MAX_TEMPLATE_PARTS`] parts, or an
/// expression nested deeper than [`MAX_TEMPLATE_DEPTH`].
///
/// [`Scope::format_string`]: crate::binding::Scope::format_string
pub fn parse_template(template: &str) -> Result<Vec<TemplatePart>> {
    let chars: Vec<char> = template.chars().collect();
    if chars.len() > MAX_TEMPLATE_CHARS {
        return Err(fault(
            &chars,
            format!(
                "the template is {} characters long; the limit is {MAX_TEMPLATE_CHARS}",
                chars.len()
            ),
        ));
    }
    let mut parts = Parts::default();
    let mut scanner = Scanner::new(&chars);
    while let Some(ch) = scanner.peek(0) {
        if scanner.starts_with("${") {
            scanner.pos += 2;
            let body = scanner.interpolation_body()?;
            parts.push(parse_expression(body, 1)?);
        } else if scanner.starts_with("\\${") {
            scanner.pos += 3;
            parts.push_text("${");
        } else {
            scanner.pos += 1;
            parts.text.push(ch);
        }
        if parts.len() > MAX_TEMPLATE_PARTS {
            return Err(fault(
                &chars,
                format!("the template has more than {MAX_TEMPLATE_PARTS} parts"),
            ));
        }
    }
    Ok(parts.finish())
}

/// The parts read so far, with the literal text that is still growing.
#[derive(Default)]
struct Parts {
    done: Vec<TemplatePart>,
    text: String,
}

impl Parts {
    fn push_text(&mut self, text: &str) {
        self.text.push_str(text);
    }

    fn push(&mut self, part: TemplatePart) {
        match part {
            // A literal string is text, wherever it was written.
            TemplatePart::Literal(Value::String(text)) => self.text.push_str(&text),
            other => {
                self.flush();
                self.done.push(other);
            }
        }
    }

    fn flush(&mut self) {
        if !self.text.is_empty() {
            let text = std::mem::take(&mut self.text);
            self.done.push(TemplatePart::Literal(Value::String(text)));
        }
    }

    fn len(&self) -> usize {
        self.done.len() + usize::from(!self.text.is_empty())
    }

    fn finish(mut self) -> Vec<TemplatePart> {
        self.flush();
        self.done
    }
}

/// An error about `expression`, which is quoted cut to a readable length.
fn fault(expression: &[char], reason: impl std::fmt::Display) -> Error {
    const SHOWN: usize = 120;
    let mut shown: String = expression.iter().take(SHOWN).collect();
    if expression.len() > SHOWN {
        shown.push('…');
    }
    Error::binding(shown, reason)
}

fn is_name_char(ch: char) -> bool {
    unicode_ident::is_xid_continue(ch)
}

struct Scanner<'a> {
    chars: &'a [char],
    pos: usize,
}

impl<'a> Scanner<'a> {
    fn new(chars: &'a [char]) -> Self {
        Self { chars, pos: 0 }
    }

    fn peek(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn at_end(&self) -> bool {
        self.pos >= self.chars.len()
    }

    fn starts_with(&self, needle: &str) -> bool {
        needle
            .chars()
            .enumerate()
            .all(|(offset, want)| self.peek(offset) == Some(want))
    }

    fn skip_whitespace(&mut self) {
        while self.peek(0).is_some_and(char::is_whitespace) {
            self.pos += 1;
        }
    }

    fn fault(&self, reason: impl std::fmt::Display) -> Error {
        fault(self.chars, reason)
    }

    /// Reads from just after a `${` to its closing `}`, which is consumed, and
    /// returns what is between them.
    ///
    /// Braces are counted and quoted strings are stepped over, so
    /// `${formatDate(format: '{yyyy}')}` and a nested `${…}` both survive.
    fn interpolation_body(&mut self) -> Result<&'a [char]> {
        let start = self.pos;
        let mut open = 1usize;
        while open > 0 {
            let Some(ch) = self.peek(0) else {
                return Err(fault(
                    &self.chars[start..],
                    "unterminated interpolation: missing '}'",
                ));
            };
            self.pos += 1;
            match ch {
                '{' => open += 1,
                '}' => open -= 1,
                '\'' | '"' => {
                    while let Some(inner) = self.peek(0) {
                        self.pos += 1;
                        if inner == '\\' {
                            self.pos += 1;
                        } else if inner == ch {
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(&self.chars[start..self.pos - 1])
    }

    /// Consumes `keyword` when it is a whole word.
    fn keyword(&mut self, keyword: &str) -> bool {
        let whole = self.starts_with(keyword)
            && !self.peek(keyword.chars().count()).is_some_and(is_name_char);
        if whole {
            self.pos += keyword.chars().count();
        }
        whole
    }

    fn name(&mut self) -> String {
        let start = self.pos;
        while self.peek(0).is_some_and(is_name_char) {
            self.pos += 1;
        }
        self.chars[start..self.pos].iter().collect()
    }

    /// A path, a function name, or a system function name with its `@`.
    fn path_or_name(&mut self) -> Result<String> {
        let start = self.pos;
        if self.peek(0) == Some('@') {
            if !self
                .peek(1)
                .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
            {
                return Err(self.fault(
                    "'@' starts the name of a system function such as @index, and no name follows it",
                ));
            }
            self.pos += 1;
            while self
                .peek(0)
                .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_')
            {
                self.pos += 1;
            }
        } else {
            loop {
                match self.peek(0) {
                    Some(ch) if is_name_char(ch) || matches!(ch, '/' | '.' | '-') => self.pos += 1,
                    // The JSON Pointer escapes, and no other use of `~`.
                    Some('~') if matches!(self.peek(1), Some('0' | '1')) => self.pos += 2,
                    _ => break,
                }
            }
        }
        Ok(self.chars[start..self.pos].iter().collect())
    }

    fn string(&mut self) -> String {
        let quote = self.peek(0);
        self.pos += 1;
        let mut out = String::new();
        while let Some(ch) = self.peek(0) {
            self.pos += 1;
            if ch == '\\' {
                if let Some(escaped) = self.peek(0) {
                    self.pos += 1;
                    out.push(match escaped {
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        other => other,
                    });
                }
            } else if Some(ch) == quote {
                break;
            } else {
                out.push(ch);
            }
        }
        out
    }

    /// Whether a number starts here: a digit, or a `.` with a digit after it,
    /// either of them after an optional sign. The grammar has no operators, so
    /// a sign here can only belong to a literal.
    fn at_number(&self) -> bool {
        let digit = |offset| {
            self.peek(offset)
                .is_some_and(|ch: char| ch.is_ascii_digit())
        };
        let offset = usize::from(matches!(self.peek(0), Some('+' | '-')));
        digit(offset) || (self.peek(offset) == Some('.') && digit(offset + 1))
    }

    fn number(&mut self) -> Result<Number> {
        let start = self.pos;
        if matches!(self.peek(0), Some('+' | '-')) {
            self.pos += 1;
        }
        while self
            .peek(0)
            .is_some_and(|ch| ch.is_ascii_digit() || ch == '.')
        {
            self.pos += 1;
        }
        // A malformed exponent is consumed with the number, so that it is
        // reported as part of it and not as characters left over.
        if matches!(self.peek(0), Some('e' | 'E')) {
            self.pos += 1;
            if matches!(self.peek(0), Some('+' | '-')) {
                self.pos += 1;
            }
            while self.peek(0).is_some_and(|ch| ch.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        number_literal(&text).map_err(|reason| self.fault(reason))
    }
}

/// The value of a number literal, or why it is not one.
fn number_literal(text: &str) -> std::result::Result<Number, String> {
    let invalid = || format!("invalid number literal {text}");
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
        Some(at) => (&unsigned[..at], Some(&unsigned[at + 1..])),
        None => (unsigned, None),
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    let mantissa_ok = match mantissa.split_once('.') {
        // `5.` and `5.25`, or `.5`: digits on at least one side.
        Some((whole, fraction)) => {
            (digits(whole) && (fraction.is_empty() || digits(fraction)))
                || (whole.is_empty() && digits(fraction))
        }
        None => digits(mantissa),
    };
    let exponent_ok = exponent
        .is_none_or(|exponent| digits(exponent.strip_prefix(['+', '-']).unwrap_or(exponent)));
    if !mantissa_ok || !exponent_ok {
        return Err(invalid());
    }
    let float: f64 = text.parse().map_err(|_| invalid())?;
    if !float.is_finite() {
        return Err(format!("number literal {text} is out of range"));
    }
    if mantissa.contains('.') || exponent.is_some() {
        // `-0.0` is zero.
        let float = if float == 0.0 { 0.0 } else { float };
        return Number::from_f64(float).ok_or_else(invalid);
    }
    // An integer stays one: `@index(offset: 1)` must not become 1.0. One too
    // large for 64 bits is the nearest double, which is what it is in JSON.
    let signed = text.strip_prefix('+').unwrap_or(text);
    Ok(signed
        .parse::<i64>()
        .map(Number::from)
        .or_else(|_| signed.parse::<u64>().map(Number::from))
        .ok()
        .or_else(|| Number::from_f64(float))
        .unwrap_or_else(|| Number::from(0)))
}

/// One expression: what is between a `${` and its `}`.
fn parse_expression(body: &[char], depth: usize) -> Result<TemplatePart> {
    let start = body
        .iter()
        .position(|ch| !ch.is_whitespace())
        .unwrap_or(body.len());
    let end = body
        .iter()
        .rposition(|ch| !ch.is_whitespace())
        .map_or(start, |last| last + 1);
    let mut scanner = Scanner::new(&body[start..end]);
    let part = parse_operand(&mut scanner, depth)?;
    if !scanner.at_end() {
        let rest: String = scanner.chars[scanner.pos..].iter().collect();
        return Err(scanner.fault(format!(
            "unexpected characters after the expression: {rest:?}"
        )));
    }
    Ok(part)
}

/// An expression, or the value of one argument. Both ways an expression nests
/// pass through here, so this is where the depth is counted.
fn parse_operand(scanner: &mut Scanner<'_>, depth: usize) -> Result<TemplatePart> {
    if depth > MAX_TEMPLATE_DEPTH {
        return Err(scanner.fault(format!(
            "expressions nest deeper than {MAX_TEMPLATE_DEPTH} levels"
        )));
    }
    let empty = || TemplatePart::Literal(Value::String(String::new()));
    scanner.skip_whitespace();
    if scanner.at_end() {
        return Ok(empty());
    }
    if scanner.starts_with("${") {
        scanner.pos += 2;
        let body = scanner.interpolation_body()?;
        return parse_expression(body, depth + 1);
    }
    if matches!(scanner.peek(0), Some('\'' | '"')) {
        return Ok(TemplatePart::Literal(Value::String(scanner.string())));
    }
    if scanner.at_number() {
        return scanner
            .number()
            .map(|number| TemplatePart::Literal(Value::Number(number)));
    }
    if scanner.keyword("true") {
        return Ok(TemplatePart::Literal(Value::Bool(true)));
    }
    if scanner.keyword("false") {
        return Ok(TemplatePart::Literal(Value::Bool(false)));
    }
    if scanner.keyword("null") {
        return Ok(empty());
    }

    let token = scanner.path_or_name()?;
    scanner.skip_whitespace();
    if scanner.peek(0) == Some('(') {
        return parse_call(token, scanner, depth);
    }
    if token.starts_with('@') {
        return Err(scanner.fault(format!("expected '(' after the function name {token}")));
    }
    Ok(if token.is_empty() {
        empty()
    } else {
        TemplatePart::Path(token)
    })
}

/// The arguments of a call, from its `(`.
fn parse_call(name: String, scanner: &mut Scanner<'_>, depth: usize) -> Result<TemplatePart> {
    scanner.pos += 1;
    scanner.skip_whitespace();
    let mut args: Vec<(String, TemplatePart)> = Vec::new();
    while scanner.peek(0).is_some_and(|ch| ch != ')') {
        let argument = scanner.name();
        scanner.skip_whitespace();
        if scanner.peek(0) != Some(':') {
            return Err(scanner.fault(format!(
                "expected ':' after the argument name {argument:?} of {name}"
            )));
        }
        scanner.pos += 1;
        scanner.skip_whitespace();
        let value = parse_operand(scanner, depth + 1)?;
        match args.iter_mut().find(|(existing, _)| *existing == argument) {
            Some((_, slot)) => *slot = value,
            None => args.push((argument, value)),
        }
        scanner.skip_whitespace();
        if scanner.peek(0) == Some(',') {
            scanner.pos += 1;
            scanner.skip_whitespace();
        }
    }
    if scanner.peek(0) != Some(')') {
        return Err(scanner.fault(format!("expected ')' after the arguments of {name}")));
    }
    scanner.pos += 1;
    Ok(TemplatePart::Call { name, args })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(value: &str) -> TemplatePart {
        TemplatePart::Literal(json!(value))
    }

    fn path(value: &str) -> TemplatePart {
        TemplatePart::Path(value.into())
    }

    fn reason(template: &str) -> String {
        match parse_template(template) {
            Err(Error::Binding { reason, .. }) => reason,
            other => panic!("{template:?}: expected a binding error, got {other:?}"),
        }
    }

    #[test]
    fn text_and_interpolations_alternate_and_literal_strings_join() {
        assert_eq!(parse_template("").unwrap(), []);
        assert_eq!(parse_template("plain").unwrap(), [text("plain")]);
        assert_eq!(
            parse_template("a ${x} b ${/y/0} c").unwrap(),
            [text("a "), path("x"), text(" b "), path("/y/0"), text(" c")]
        );
        // An escaped marker, a quoted string and `null` are all literal text,
        // and literal text next to literal text is one part.
        assert_eq!(
            parse_template(r"\${x} ${'and'} ${null}${ }.").unwrap(),
            [text("${x} and .")]
        );
        assert_eq!(
            parse_template("${true}${42}").unwrap(),
            [
                TemplatePart::Literal(json!(true)),
                TemplatePart::Literal(json!(42))
            ]
        );
    }

    #[test]
    fn a_path_may_carry_the_json_pointer_escapes() {
        // A key that holds `/` or `~` is named `~1` or `~0` in a pointer, and
        // a template path is a pointer like every other binding.
        assert_eq!(
            parse_template("${/a~1b} ${c~0d/e} ${f(x: /g~10)}").unwrap(),
            [
                path("/a~1b"),
                text(" "),
                path("c~0d/e"),
                text(" "),
                TemplatePart::Call {
                    name: "f".into(),
                    args: vec![("x".into(), path("/g~10"))],
                },
            ]
        );
        // Nothing else follows a `~`.
        for stray in ["${/a~b}", "${/a~2}", "${~}", "${/a~}"] {
            let reason = reason(stray);
            assert!(
                reason.starts_with("unexpected characters after the expression"),
                "{stray}: {reason}"
            );
        }
    }

    #[test]
    fn calls_keep_their_arguments_in_order_and_nest() {
        let parts = parse_template("${outer(a: inner(b: 1), c: /p, d: 'x', e: ${q},)}").unwrap();
        let TemplatePart::Call { name, args } = &parts[0] else {
            panic!("expected a call: {parts:?}");
        };
        assert_eq!(name, "outer");
        assert_eq!(
            args.iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            ["a", "c", "d", "e"]
        );
        assert_eq!(
            parts[0].to_json(),
            json!({"call": "outer", "returnType": "any", "args": {
                "a": {"call": "inner", "returnType": "any", "args": {"b": 1}},
                "c": {"path": "/p"},
                "d": "x",
                "e": {"path": "q"},
            }})
        );
        // A name written twice keeps its first place and its last value.
        assert_eq!(
            parse_template("${f(a: 1, b: 2, a: 3)}").unwrap()[0].to_json()["args"],
            json!({"a": 3, "b": 2})
        );
        assert_eq!(
            parse_template("${@index(offset: 1)}").unwrap()[0].to_json(),
            json!({"call": "@index", "returnType": "any", "args": {"offset": 1}})
        );
    }

    #[test]
    fn a_number_is_an_integer_unless_it_is_written_as_a_decimal() {
        for (literal, expected) in [
            ("42", json!(42)),
            ("-42", json!(-42)),
            ("+1", json!(1)),
            ("007", json!(7)),
            ("-0", json!(0)),
            ("9223372036854775807", json!(i64::MAX)),
            ("-9223372036854775808", json!(i64::MIN)),
            ("18446744073709551615", json!(u64::MAX)),
            ("1.5", json!(1.5)),
            ("1.", json!(1.0)),
            (".5", json!(0.5)),
            ("-.5E-1", json!(-0.05)),
            ("1e5", json!(100000.0)),
            ("-0.0", json!(0.0)),
            ("1e-999", json!(0.0)),
        ] {
            assert_eq!(
                parse_template(&format!("${{{literal}}}")).unwrap(),
                [TemplatePart::Literal(expected)],
                "{literal}"
            );
        }
        for malformed in ["1.2.3", ".5.5", "1e", "1e+", "-.5e"] {
            let reason = reason(&format!("${{{malformed}}}"));
            assert!(reason.starts_with("invalid number literal"), "{reason}");
        }
        for huge in ["1e999", "-1e999", "2e308"] {
            let reason = reason(&format!("${{{huge}}}"));
            assert!(reason.ends_with("is out of range"), "{reason}");
        }
        // A sign or a dot with no digit after it is a path.
        assert_eq!(
            parse_template("${-a}${.foo}${-.}").unwrap(),
            [path("-a"), path(".foo"), path("-.")]
        );
    }

    #[test]
    fn a_malformed_template_says_what_is_wrong_with_it() {
        for (template, starts_with) in [
            ("hello ${world", "unterminated interpolation"),
            ("${'never closed}", "unterminated interpolation"),
            (
                "${true false}",
                "unexpected characters after the expression",
            ),
            ("${a b}", "unexpected characters after the expression"),
            ("${a½}", "unexpected characters after the expression"),
            ("${/a~2b}", "unexpected characters after the expression"),
            ("${a~}", "unexpected characters after the expression"),
            (
                "${add(a: 1, b: 2}",
                "expected ')' after the arguments of add",
            ),
            ("${add(a 1)}", "expected ':' after the argument name \"a\""),
            ("${f(1)}", "expected ':' after the argument name \"1\""),
            ("${@index}", "expected '(' after the function name @index"),
            ("${@}", "'@' starts the name of a system function"),
        ] {
            let reason = reason(template);
            assert!(reason.starts_with(starts_with), "{template}: {reason}");
        }
    }

    #[test]
    fn nesting_length_and_part_count_are_bounded() {
        let nested = |levels: usize| format!("{}x{}", "${".repeat(levels), "}".repeat(levels));
        assert_eq!(
            parse_template(&nested(MAX_TEMPLATE_DEPTH)).unwrap(),
            [path("x")]
        );
        assert!(reason(&nested(MAX_TEMPLATE_DEPTH + 1)).starts_with("expressions nest deeper"));

        let calls =
            |levels: usize| format!("${{{}1{}}}", "f(a: ".repeat(levels), ")".repeat(levels));
        // An argument is one level below its call, the innermost one included.
        assert!(parse_template(&calls(MAX_TEMPLATE_DEPTH - 1)).is_ok());
        assert!(reason(&calls(MAX_TEMPLATE_DEPTH)).starts_with("expressions nest deeper"));

        assert!(parse_template(&"a".repeat(MAX_TEMPLATE_CHARS)).is_ok());
        let long = reason(&"a".repeat(MAX_TEMPLATE_CHARS + 1));
        assert!(long.contains("10001 characters long"), "{long}");

        // Paths do not join, so each is a part of its own.
        assert_eq!(
            parse_template(&"${x}".repeat(MAX_TEMPLATE_PARTS))
                .unwrap()
                .len(),
            MAX_TEMPLATE_PARTS
        );
        let many = reason(&"${x}".repeat(MAX_TEMPLATE_PARTS + 1));
        assert!(many.contains("more than 1000 parts"), "{many}");
    }

    #[test]
    fn no_cut_of_a_template_makes_the_parser_panic() {
        // A template is model output, and arrives cut anywhere while it
        // streams. Every prefix and every suffix of these parses or is an
        // error; none of them is a panic.
        for template in [
            r#"a ${f(x: 'q\'}', y: "\\", z: ${${/p}}, w: -1.5e+3,)} \${ ${'\"#,
            "${@index(offset: +.5)}${true}${null}${größe(値: ½)}",
            r"${'\",
            "${\"${",
            "${f(a: ${'}'}, b: {})}",
        ] {
            let chars: Vec<char> = template.chars().collect();
            for cut in 0..=chars.len() {
                let _ = parse_template(&chars[..cut].iter().collect::<String>());
                let _ = parse_template(&chars[cut..].iter().collect::<String>());
            }
        }
    }

    #[test]
    fn an_error_quotes_a_long_expression_cut_short() {
        let template = format!("${{{} oops}}", "a".repeat(500));
        let Err(Error::Binding { expression, .. }) = parse_template(&template) else {
            panic!("expected a binding error");
        };
        assert_eq!(expression.chars().count(), 121);
        assert!(expression.ends_with('…'));
    }
}
