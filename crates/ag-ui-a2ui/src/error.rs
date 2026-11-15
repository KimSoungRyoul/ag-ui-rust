//! Error types for the A2UI protocol layer.

use std::fmt;

/// Convenient alias for fallible A2UI operations.
pub type Result<T> = std::result::Result<T, Error>;

/// The most characters of a data-model path an error message quotes.
///
/// Well past any path a surface has reason to use, so an ordinary message is
/// unchanged; a longer path is cut here and its full length stated instead.
pub const MAX_SHOWN_PATH_CHARS: usize = 120;

/// A data-model path as an error message shows it: whole when it is short,
/// cut to [`MAX_SHOWN_PATH_CHARS`] characters with its length when it is not.
///
/// `{}` writes the path as it is and `{:?}` quotes it, so a message keeps the
/// punctuation it had before there was a bound.
pub(crate) struct ShownPath<'a>(pub(crate) &'a str);

impl ShownPath<'_> {
    /// The part that is shown, and the full length when that is not all of it.
    fn head(&self) -> (&str, Option<usize>) {
        match self.0.char_indices().nth(MAX_SHOWN_PATH_CHARS) {
            Some((cut, _)) => (&self.0[..cut], Some(self.0.chars().count())),
            None => (self.0, None),
        }
    }
}

impl fmt::Display for ShownPath<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.head() {
            (head, Some(length)) => write!(f, "{head}… ({length} characters)"),
            (whole, None) => f.write_str(whole),
        }
    }
}

impl fmt::Debug for ShownPath<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.head() {
            (head, Some(length)) => write!(f, "{head:?}… ({length} characters)"),
            (whole, None) => write!(f, "{whole:?}"),
        }
    }
}

/// Everything that can go wrong while producing, validating, or transporting
/// A2UI.
///
/// The variants are deliberately coarse: A2UI is a wire protocol, so the useful
/// detail almost always lives in the payload (a [`crate::validate::ValidationError`]
/// list, a JSON Pointer, the offending literal) rather than in the discriminant.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The bytes were not JSON, or were not JSON of the expected shape.
    #[error("invalid A2UI JSON: {0}")]
    Json(#[from] serde_json::Error),

    /// An LLM response could not be split into conversational text and A2UI
    /// blocks, or a block did not contain usable JSON.
    #[error("A2UI parse error: {0}")]
    Parse(String),

    /// A syntactically well-formed payload that violates A2UI semantics.
    ///
    /// Carries the full machine-readable error list so a caller can feed it
    #[cfg_attr(
        feature = "agent",
        doc = "straight back to a model; see [`RetryDriver`](crate::agent::RetryDriver)."
    )]
    #[cfg_attr(
        not(feature = "agent"),
        doc = "straight back to a model; see `agent::RetryDriver`, behind the `agent` feature."
    )]
    #[error("A2UI validation failed with {} error(s):\n{errors}", errors.len())]
    Validation {
        /// The full machine-readable error list.
        errors: ValidationErrors,
    },

    /// A JSON Pointer was malformed, or resolved to a location that cannot be
    /// written (for example indexing past the end of an array).
    ///
    /// The message shows at most [`MAX_SHOWN_PATH_CHARS`] characters of the
    /// pointer and says how long it was. A pointer is remote input, the error
    /// is fed back to a model, and the pointer that is too long to accept is
    /// the one that would make the longest message. The field keeps it whole.
    #[error("invalid data-model pointer {:?}: {reason}", ShownPath(.pointer))]
    Pointer {
        /// The offending pointer, exactly as it appeared on the wire.
        pointer: String,
        /// Why it could not be resolved.
        reason: String,
    },

    /// A `${...}` expression in a `formatString` template could not be parsed
    /// or evaluated.
    #[error("invalid binding expression {expression:?}: {reason}")]
    Binding {
        /// The expression body, without the surrounding `${` and `}`.
        expression: String,
        /// Why it could not be evaluated.
        reason: String,
    },

    /// A catalog document could not be interpreted as an A2UI catalog.
    #[error("invalid catalog: {0}")]
    Catalog(String),

    /// The provider failed; its original error remains available as the source.
    #[error("A2UI provider generation failed: {0}")]
    Generation(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// A model cannot be exported as JSON without losing undefined values.
    #[error("A2UI model contains an undefined value at {0}")]
    Undefined(String),

    /// A model failed to produce a valid surface within
    /// [`MAX_A2UI_ATTEMPTS`](crate::constants::MAX_A2UI_ATTEMPTS) attempts.
    #[error("A2UI generation gave up after {attempts} attempt(s); last errors: {last}")]
    RecoveryExhausted {
        /// How many generation attempts were made.
        attempts: u32,
        /// The validation errors from the final attempt.
        last: ValidationErrors,
    },
}

impl Error {
    /// Preserves a provider error without treating it as model-generated invalid JSON.
    pub fn generation(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Generation(Box::new(error))
    }

    /// Builds a [`Error::Parse`] from anything printable.
    pub fn parse(reason: impl fmt::Display) -> Self {
        Self::Parse(reason.to_string())
    }

    /// Builds a [`Error::Catalog`] from anything printable.
    pub fn catalog(reason: impl fmt::Display) -> Self {
        Self::Catalog(reason.to_string())
    }

    /// Builds a [`Error::Pointer`] for a pointer that could not be resolved.
    pub fn pointer(pointer: impl Into<String>, reason: impl fmt::Display) -> Self {
        Self::Pointer {
            pointer: pointer.into(),
            reason: reason.to_string(),
        }
    }

    /// Builds a [`Error::Binding`] for an expression that could not be evaluated.
    pub fn binding(expression: impl Into<String>, reason: impl fmt::Display) -> Self {
        Self::Binding {
            expression: expression.into(),
            reason: reason.to_string(),
        }
    }
}

/// A list of semantic validation errors, rendered one per line.
///
/// Newtype rather than a bare `Vec` so that [`Error::Validation`] can carry a
/// `Display` impl an LLM can read directly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidationErrors(pub Vec<crate::protocol::validate::ValidationError>);

impl ValidationErrors {
    /// Number of errors in the list.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the list is empty (i.e. the payload validated cleanly).
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Borrows the underlying errors.
    pub fn as_slice(&self) -> &[crate::protocol::validate::ValidationError] {
        &self.0
    }
}

/// The errors of a rejected attempt, as a request for a corrected one.
///
/// One line per error, code first so a model can see the kind at a glance,
/// then the locator, then the sentence explaining the fix. Empty for no
/// errors. The retry prompt of every layer is built from this.
#[cfg(feature = "agent")]
pub(crate) fn correction_request(errors: &[crate::protocol::validate::ValidationError]) -> String {
    if errors.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "The previous attempt was rejected. Fix every problem below and return the corrected \
         A2UI, not a diff:\n",
    );
    for error in errors {
        out.push_str(&format!(
            "- [{}] at {}: {}\n",
            error.code, error.path, error.message
        ));
    }
    out.trim_end().to_string()
}

/// `prompt`, followed by the [`correction_request`] for `errors`. The prompt
/// itself when there is nothing to correct.
#[cfg(feature = "agent")]
pub(crate) fn prompt_with_corrections(
    prompt: &str,
    errors: &[crate::protocol::validate::ValidationError],
) -> String {
    if errors.is_empty() {
        return prompt.to_string();
    }
    format!(
        "{prompt}\n\n## Correction required\n{}",
        correction_request(errors)
    )
}

impl From<Vec<crate::protocol::validate::ValidationError>> for ValidationErrors {
    fn from(errors: Vec<crate::protocol::validate::ValidationError>) -> Self {
        Self(errors)
    }
}

impl fmt::Display for ValidationErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, error) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str("\n")?;
            }
            write!(f, "{error}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_pointer_is_quoted_whole_as_it_always_was() {
        let error = Error::pointer("/items/nope", "expected a canonical array index");
        assert_eq!(
            error.to_string(),
            "invalid data-model pointer \"/items/nope\": expected a canonical array index"
        );

        // Up to the limit nothing is cut, and escapes are still escaped.
        let at_the_limit = format!("/{}", "a".repeat(MAX_SHOWN_PATH_CHARS - 1));
        assert_eq!(
            Error::pointer(at_the_limit.clone(), "r").to_string(),
            format!("invalid data-model pointer {at_the_limit:?}: r")
        );
        assert_eq!(
            Error::pointer("/a\"b", "r").to_string(),
            "invalid data-model pointer \"/a\\\"b\": r"
        );
    }

    #[test]
    fn a_long_pointer_is_cut_in_the_message_and_whole_in_the_field() {
        let pointer = "/a".repeat(5_000);
        let error = Error::pointer(pointer.clone(), "path has more than 256 segments");
        let message = error.to_string();
        assert_eq!(
            message,
            format!(
                "invalid data-model pointer {:?}… (10000 characters): \
                 path has more than 256 segments",
                &pointer[..MAX_SHOWN_PATH_CHARS]
            )
        );
        assert!(message.len() < 250, "{} bytes", message.len());

        let Error::Pointer { pointer: kept, .. } = error else {
            panic!("expected a pointer error");
        };
        assert_eq!(kept, pointer, "the bound is on the message, not the data");
    }

    #[test]
    fn a_cut_lands_between_characters_and_counts_characters() {
        // Three bytes each: a cut by bytes would split one.
        let pointer = format!("/{}", "値".repeat(200));
        let shown = ShownPath(&pointer).to_string();
        assert_eq!(
            shown,
            format!(
                "/{}… (201 characters)",
                "値".repeat(MAX_SHOWN_PATH_CHARS - 1)
            )
        );
    }
}
