//! Deciding what to do with a model's answer, without calling the model.

use super::format::ResponsePart;
use super::processor::A2uiRequestProcessor;
use super::state::RendererState;
use crate::error::{Error, prompt_with_corrections};
use crate::protocol::validate::{ErrorCode, ValidationError};

/// Judges a model's answers to one request, one attempt after another.
///
/// The driver is told what the model answered and says one of three things:
/// the answer stands, ask again with this feedback, or give up. The caller
/// makes every call to the model, on whatever executor and client it has;
/// nothing here awaits, sleeps or does I/O.
///
/// The driver owns a copy of the processor, which is cheap, and borrows
/// nothing, so it can be kept in the same struct as the state across `.await`
/// points.
///
/// ```
/// use ag_ui_a2ui::A2uiVersion;
/// use ag_ui_a2ui::agent::{A2uiGenerator, CatalogConfig, RendererCapabilities, RetryDriver, Verdict};
///
/// let generator = A2uiGenerator::new(vec![CatalogConfig::basic(A2uiVersion::V0_9)?]);
/// let renderer = RendererCapabilities::supporting(
///     A2uiVersion::V0_9,
///     ["https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json"],
/// );
/// let processor = generator.create_processor(Some(&renderer))?;
/// let mut state = processor.create_state();
/// let mut retry = RetryDriver::new(&processor, 3);
///
/// // The first answer names a component the catalog does not have.
/// let bad = r#"<a2ui-json>[
///   {"version": "v0.9", "createSurface": {"surfaceId": "s",
///     "catalogId": "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json"}},
///   {"version": "v0.9", "updateComponents": {"surfaceId": "s", "components": [
///     {"id": "root", "component": "Sparkline"}]}}
/// ]</a2ui-json>"#;
/// let Verdict::Retry(feedback) = retry.judge(bad, &mut state) else { panic!() };
/// assert!(state.surface("s").is_none(), "a rejected answer leaves the state as it was");
/// let next_prompt = feedback.prompt("Show a greeting.");
/// assert!(next_prompt.contains("[unknown_component] at messages[1].updateComponents.components[0].component"));
///
/// // The corrected one stands, and is in the state.
/// let good = bad.replace(r#""component": "Sparkline""#, r#""component": "Text", "text": "Hello""#);
/// let Verdict::Done(parts) = retry.judge(&good, &mut state) else { panic!() };
/// assert_eq!(parts.len(), 1);
/// assert!(state.surface("s").is_some());
/// # Ok::<(), ag_ui_a2ui::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct RetryDriver {
    processor: A2uiRequestProcessor,
    max_attempts: u32,
    attempts: u32,
}

/// What to do with an answer.
#[derive(Debug)]
pub enum Verdict {
    /// The answer stands. Its messages have been applied to the state, in
    /// order, and are what the agent sends.
    Done(Vec<ResponsePart>),
    /// The answer was rejected and the state is as it was. Ask the model
    /// again, telling it what was wrong.
    Retry(Feedback),
    /// The answer was rejected and the state is as it was. Asking again will
    /// not help: the attempts are used up ([`Error::RecoveryExhausted`], with
    /// the last attempt's errors), or the failure is not the model's to fix.
    GiveUp(Error),
}

/// Why an attempt was rejected, to be told to the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feedback {
    errors: Vec<ValidationError>,
    attempt: u32,
}

impl Feedback {
    /// What was wrong with the attempt, each failure located in the answer.
    pub fn errors(&self) -> &[ValidationError] {
        &self.errors
    }

    /// Which attempt was rejected, counting from one.
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    /// The prompt for the next attempt: `original`, followed by the errors as
    /// a request for a corrected answer.
    ///
    /// Always build it from the original prompt, not from the previous
    /// attempt's: the model is told what is wrong with its latest answer, and
    /// corrections do not pile up.
    pub fn prompt(&self, original: &str) -> String {
        prompt_with_corrections(original, &self.errors)
    }
}

impl RetryDriver {
    /// A driver that gives up after `max_attempts` rejected answers. At least
    /// one attempt is always judged.
    pub fn new(processor: &A2uiRequestProcessor, max_attempts: u32) -> Self {
        Self {
            processor: processor.clone(),
            max_attempts: max_attempts.max(1),
            attempts: 0,
        }
    }

    /// How many answers have been judged.
    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    /// Judges the model's answer, reading it into `state`.
    ///
    /// The answer is read whole, as
    /// [`A2uiRequestProcessor::parse_response_in`] reads one, so it is all or
    /// nothing: after [`Verdict::Retry`] and [`Verdict::GiveUp`], `state` is
    /// exactly what it was before the call.
    pub fn judge(&mut self, answer: &str, state: &mut RendererState) -> Verdict {
        self.attempts += 1;
        let fits = self.processor.accepts_state(state);
        let rejected = match self.processor.parse_response_in(answer, state) {
            Ok(parts) => return Verdict::Done(parts),
            Err(rejected) => rejected.error,
        };
        if !fits {
            // The state holds other catalogs than the processor. No answer
            // can change that.
            return Verdict::GiveUp(rejected);
        }
        let errors = match rejected {
            Error::Validation { errors } => errors.0,
            // A block that could not be read: the model's to fix, told as is.
            other => vec![ValidationError::new(
                ErrorCode::InvalidValue,
                "response",
                other.to_string(),
            )],
        };
        if self.attempts >= self.max_attempts {
            return Verdict::GiveUp(Error::RecoveryExhausted {
                attempts: self.attempts,
                last: errors.into(),
            });
        }
        Verdict::Retry(Feedback {
            errors,
            attempt: self.attempts,
        })
    }
}
