//! The direct JSON format: the model writes A2UI messages themselves.
//!
//! A payload is a JSON array of messages between `<a2ui-json>` and
//! `</a2ui-json>` tags, with conversational text around it:
//!
//! ```text
//! Here is your form.
//! <a2ui-json>
//! [{"version": "v0.9", "createSurface": {…}}, {"version": "v0.9", "updateComponents": {…}}]
//! </a2ui-json>
//! ```
//!
//! Reading it is mostly reading JSON, so what this module adds is what may be
//! repaired before reading, because a model that writes almost-JSON is the
//! common case: a trailing comma, a curly quote picked up from prose, one
//! message where a list was asked for, a Markdown fence around the payload,
//! the tags written as inline code, an opening tag left out.
//!
//! ```
//! use ag_ui_a2ui::agent::{DirectJson, OutputFormat, RawPart};
//!
//! let format = DirectJson::new();
//! let answer = "Done.\n<a2ui-json>\n```json\n[{\"deleteSurface\": {\"surfaceId\": \"s\"},},]\n```\n</a2ui-json>";
//!
//! let parts = format.unwrap(answer);
//! assert_eq!(parts[0], RawPart::Text("Done.".into()));
//! let RawPart::Block { raw, closed: true } = &parts[1] else { panic!() };
//! let messages = format.compile(raw, true)?;
//! assert_eq!(messages[0]["deleteSurface"]["surfaceId"], "s");
//! # Ok::<(), ag_ui_a2ui::Error>(())
//! ```

mod blocks;
mod json;
mod prompt;
mod reader;

use std::collections::BTreeSet;

use serde_json::Value;

use self::json::Healing;
use super::catalog::CatalogDocument;
use super::format::{AnswerReader, OutputFormat, RawPart};
use crate::error::{Error, Result};
use crate::protocol::message::AgentMessage;
use crate::protocol::operation::Operation;

/// The key of the list a reader holds back until each item has closed, when
/// asked to: [`DirectJson::holding_components_until_closed`].
const COMPONENTS: &str = "components";

/// The direct JSON format. See the [module documentation](self).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirectJson {
    allowed_messages: Option<Vec<Operation>>,
    progressive_keys: BTreeSet<String>,
    hold_components: bool,
}

impl DirectJson {
    /// The format as a model is asked for it by default: every operation
    /// described, and nothing healed while an answer streams.
    pub fn new() -> Self {
        Self::default()
    }

    /// Restricts the envelope schema the prompt shows to these operations.
    ///
    /// This changes what the model is told, not what is accepted: a message
    /// of another operation still reads.
    #[must_use]
    pub fn allowing_messages(mut self, operations: impl IntoIterator<Item = Operation>) -> Self {
        self.allowed_messages = Some(operations.into_iter().collect());
        self
    }

    /// Names the properties whose string value may be ended early while an
    /// answer streams, so that prose shows as it is written.
    ///
    /// The set names properties whose value is text the model is writing.
    /// Ids, paths, component names and child references are outside it:
    /// ending one early would invent an identifier the model never wrote, so
    /// a message cut inside one waits. A string that starts as a URL is never
    /// ended early either. [`DEFAULT_CUTTABLE_KEYS`] is a reasonable set for
    /// the basic catalog.
    ///
    /// Empty by default, which turns healing off.
    ///
    /// [`DEFAULT_CUTTABLE_KEYS`]: crate::schema::DEFAULT_CUTTABLE_KEYS
    #[must_use]
    pub fn with_progressive_keys<I, S>(mut self, keys: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.progressive_keys = keys.into_iter().map(Into::into).collect();
        self
    }

    /// Whether a stream emits a component only once its own JSON object has
    /// closed.
    ///
    /// Off by default: a component is emitted while it arrives, with its
    /// strings under progressive keys ended early, and again each time a
    /// chunk changes it. On, a component still arriving is left out of its
    /// list, and progressive keys heal only values outside components, such
    /// as a data model.
    #[must_use]
    pub fn holding_components_until_closed(mut self, hold: bool) -> Self {
        self.hold_components = hold;
        self
    }
}

/// Reads the body of a closed block as a JSON message or a list of them.
fn compile_block(block: &str) -> Result<Vec<Value>> {
    let cleaned = blocks::clean_raw(block);
    if cleaned.is_empty() {
        return Err(Error::parse("the direct JSON block is empty"));
    }
    match json::read_json(cleaned) {
        Ok(Value::Array(messages)) => Ok(messages),
        Ok(message) => Ok(vec![message]),
        Err(reason) => Err(Error::parse(format!(
            "the direct JSON block is not JSON: {reason}"
        ))),
    }
}

fn unclosed_block(reason: impl std::fmt::Display) -> Error {
    Error::parse(reason)
}

impl OutputFormat for DirectJson {
    fn name(&self) -> &'static str {
        "direct_json"
    }

    fn prompt_snippet(
        &self,
        catalogs: &[CatalogDocument],
        examples: &[Vec<AgentMessage>],
    ) -> Result<String> {
        prompt::prompt_snippet(catalogs, examples, self.allowed_messages.as_deref())
    }

    fn unwrap(&self, answer: &str) -> Vec<RawPart> {
        blocks::unwrap(answer)
    }

    fn wrap(&self, parts: &[RawPart]) -> String {
        blocks::wrap(parts)
    }

    /// # Errors
    ///
    /// Returns [`Error::Parse`] when the block is not closed, is empty, or is
    /// not JSON once what can be repaired has been.
    fn compile(&self, block: &str, closed: bool) -> Result<Vec<Value>> {
        if !closed {
            return Err(unclosed_block(format!(
                "The response ended inside an unclosed {} block.",
                blocks::OPEN_TAG
            )));
        }
        compile_block(block)
    }

    fn decompile(&self, messages: &[AgentMessage]) -> Result<String> {
        Ok(serde_json::to_string_pretty(messages)?)
    }

    fn reader(&self, wrapped: bool) -> Option<Box<dyn AnswerReader>> {
        let healing = Healing {
            progressive_keys: self.progressive_keys.clone(),
            whole_item_keys: if self.hold_components {
                BTreeSet::from([COMPONENTS.to_string()])
            } else {
                BTreeSet::new()
            },
        };
        Some(Box::new(reader::DirectJsonReader::new(
            wrapped,
            healing,
            self.hold_components,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_block_is_a_list_of_messages_or_one_message() {
        let format = DirectJson::new();
        assert_eq!(
            format.compile(r#"[{"a": 1}, {"b": 2}]"#, true).unwrap(),
            [json!({"a": 1}), json!({"b": 2})]
        );
        assert_eq!(
            format.compile("`{\"a\": 1}`", true).unwrap(),
            [json!({"a": 1})]
        );
    }

    #[test]
    fn a_block_that_cannot_be_read_is_a_parse_error() {
        let format = DirectJson::new();
        for (block, closed) in [("", true), ("here is your surface", true), ("[1]", false)] {
            assert!(
                matches!(format.compile(block, closed), Err(Error::Parse(_))),
                "{block:?}"
            );
        }
    }

    #[test]
    fn content_is_a_block_and_complete_content_a_closed_one() {
        let format = DirectJson::new();
        let open = "<a2ui-json>[1";
        assert!(format.has_content(open, false));
        assert!(!format.has_content(open, true));
        assert!(format.has_content("[{\"a\": 1}]\n</a2ui-json>", true));
        assert!(!format.has_content("Close with </a2ui-json>.", false));
    }

    #[test]
    fn what_is_decompiled_compiles_back() {
        let format = DirectJson::new();
        let messages = vec![
            AgentMessage::create_surface("s", "c"),
            AgentMessage::update_data_model("s", "/title", json!("Hello")),
            AgentMessage::delete_surface("s"),
        ];
        let written = format.decompile(&messages).unwrap();
        assert!(written.starts_with("[\n  {\n"));
        let read: Vec<AgentMessage> = format
            .compile(&written, true)
            .unwrap()
            .into_iter()
            .map(|message| serde_json::from_value(message).unwrap())
            .collect();
        assert_eq!(read, messages);
    }
}
