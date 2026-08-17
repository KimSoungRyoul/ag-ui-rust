//! The message union and its multimodal content parts.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::JsonObject;
use crate::ids::{MessageId, ToolCallId};
use crate::tool::ToolCall;

/// Every role the protocol defines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub enum Role {
    /// Out-of-band instructions from the application developer.
    Developer,
    /// System prompt.
    System,
    /// Model output.
    Assistant,
    /// End-user input.
    User,
    /// The result of a tool call.
    Tool,
    /// A structured progress update rendered by the client.
    Activity,
    /// Model reasoning / chain-of-thought.
    Reasoning,
}

impl Role {
    /// The role string as it appears on the wire.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Developer => "developer",
            Self::System => "system",
            Self::Assistant => "assistant",
            Self::User => "user",
            Self::Tool => "tool",
            Self::Activity => "activity",
            Self::Reasoning => "reasoning",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where the bytes of a multimodal part live.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub enum InputContentSource {
    /// Inline data, typically base64.
    Data {
        /// The encoded payload.
        value: String,
        /// MIME type of the payload, for example `image/png`.
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    /// A URL the consumer fetches itself.
    Url {
        /// The URL.
        value: String,
        /// MIME type, when the producer knows it.
        #[serde(rename = "mimeType", default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
    },
}

/// A plain-text content part.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct TextInputContent {
    /// The text.
    pub text: String,
}

/// An image, audio, video or document content part.
///
/// The four modalities share a shape, so they share a struct; which one it is
/// is carried by the [`InputContent`] variant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct MediaInputContent {
    /// Where the bytes are.
    pub source: InputContentSource,
    /// Producer-defined extras (dimensions, page counts, alt text, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

impl MediaInputContent {
    /// Builds a media part from a source, with no metadata.
    pub fn new(source: InputContentSource) -> Self {
        Self {
            source,
            metadata: None,
        }
    }
}

/// The legacy `binary` content part.
///
/// Superseded by the modality-specific parts, but still accepted: at least one
/// of `id`, `url` or `data` must be set. That constraint is a runtime rule in
/// the upstream schema and is not encoded in this type — see
/// [`BinaryInputContent::has_payload`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct BinaryInputContent {
    /// MIME type of the payload.
    pub mime_type: String,
    /// Reference to a previously uploaded blob.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// URL to fetch the payload from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Inline payload, typically base64.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// Original file name, for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

impl BinaryInputContent {
    /// Whether the part carries a payload in any of the three accepted forms.
    pub fn has_payload(&self) -> bool {
        self.id.is_some() || self.url.is_some() || self.data.is_some()
    }
}

/// One part of a multimodal user message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub enum InputContent {
    /// Text.
    Text(TextInputContent),
    /// An image.
    Image(MediaInputContent),
    /// An audio clip.
    Audio(MediaInputContent),
    /// A video clip.
    Video(MediaInputContent),
    /// A document, for example a PDF.
    Document(MediaInputContent),
    /// The legacy catch-all binary part.
    Binary(BinaryInputContent),
}

impl InputContent {
    /// Builds a text part.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(TextInputContent { text: text.into() })
    }
}

/// The body of a user message: either a bare string or an ordered list of
/// multimodal parts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub enum UserContent {
    /// Plain text.
    Text(String),
    /// Multimodal parts.
    Parts(Vec<InputContent>),
}

impl Default for UserContent {
    fn default() -> Self {
        Self::Text(String::new())
    }
}

impl From<String> for UserContent {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for UserContent {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<Vec<InputContent>> for UserContent {
    fn from(value: Vec<InputContent>) -> Self {
        Self::Parts(value)
    }
}

/// Out-of-band instructions from the application developer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct DeveloperMessage {
    /// Message id.
    pub id: MessageId,
    /// The instructions.
    pub content: String,
    /// Optional display name for the author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Opaque provider payload for zero-data-retention modes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_value: Option<String>,
}

/// The system prompt.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct SystemMessage {
    /// Message id.
    pub id: MessageId,
    /// The prompt.
    pub content: String,
    /// Optional display name for the author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Opaque provider payload for zero-data-retention modes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_value: Option<String>,
}

/// Model output, optionally requesting tool calls.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct AssistantMessage {
    /// Message id.
    pub id: MessageId,
    /// The reply text. Absent when the turn is tool calls only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Optional display name for the author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Opaque provider payload for zero-data-retention modes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_value: Option<String>,
    /// Tool calls the assistant wants executed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

/// End-user input, possibly multimodal.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct UserMessage {
    /// Message id.
    pub id: MessageId,
    /// Text or multimodal parts.
    pub content: UserContent,
    /// Optional display name for the author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Opaque provider payload for zero-data-retention modes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_value: Option<String>,
}

/// The result of a tool call, fed back to the model.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ToolMessage {
    /// Message id.
    pub id: MessageId,
    /// The result, already rendered to a string.
    pub content: String,
    /// The call this result answers.
    pub tool_call_id: ToolCallId,
    /// Set when the tool failed; `content` then holds whatever partial output
    /// there was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Opaque provider payload for zero-data-retention modes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_value: Option<String>,
}

/// A structured progress update the client renders as it likes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ActivityMessage {
    /// Message id.
    pub id: MessageId,
    /// Client-defined activity discriminator, for example `"web_search"`.
    pub activity_type: String,
    /// The activity payload.
    #[cfg_attr(
        feature = "schemars",
        schemars(with = "std::collections::BTreeMap<String, serde_json::Value>")
    )]
    #[cfg_attr(feature = "utoipa", schema(value_type = Object))]
    pub content: JsonObject,
}

/// Model reasoning, shown separately from the reply.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ReasoningMessage {
    /// Message id.
    pub id: MessageId,
    /// The reasoning text. Empty when the provider only returns an encrypted
    /// blob.
    pub content: String,
    /// Opaque provider payload for zero-data-retention modes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_value: Option<String>,
}

/// A message in a thread, discriminated by its `role`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub enum Message {
    /// See [`DeveloperMessage`].
    Developer(DeveloperMessage),
    /// See [`SystemMessage`].
    System(SystemMessage),
    /// See [`AssistantMessage`].
    Assistant(AssistantMessage),
    /// See [`UserMessage`].
    User(UserMessage),
    /// See [`ToolMessage`].
    Tool(ToolMessage),
    /// See [`ActivityMessage`].
    Activity(ActivityMessage),
    /// See [`ReasoningMessage`].
    Reasoning(ReasoningMessage),
}

impl Message {
    /// Builds a user message carrying plain text.
    pub fn user(id: impl Into<MessageId>, content: impl Into<UserContent>) -> Self {
        Self::User(UserMessage {
            id: id.into(),
            content: content.into(),
            ..Default::default()
        })
    }

    /// Builds an assistant message carrying plain text.
    pub fn assistant(id: impl Into<MessageId>, content: impl Into<String>) -> Self {
        Self::Assistant(AssistantMessage {
            id: id.into(),
            content: Some(content.into()),
            ..Default::default()
        })
    }

    /// Builds a system message.
    pub fn system(id: impl Into<MessageId>, content: impl Into<String>) -> Self {
        Self::System(SystemMessage {
            id: id.into(),
            content: content.into(),
            ..Default::default()
        })
    }

    /// Builds a developer message.
    pub fn developer(id: impl Into<MessageId>, content: impl Into<String>) -> Self {
        Self::Developer(DeveloperMessage {
            id: id.into(),
            content: content.into(),
            ..Default::default()
        })
    }

    /// Builds a tool result message.
    pub fn tool(
        id: impl Into<MessageId>,
        tool_call_id: impl Into<ToolCallId>,
        content: impl Into<String>,
    ) -> Self {
        Self::Tool(ToolMessage {
            id: id.into(),
            content: content.into(),
            tool_call_id: tool_call_id.into(),
            ..Default::default()
        })
    }

    /// The message id, whatever the role.
    pub const fn id(&self) -> &MessageId {
        match self {
            Self::Developer(m) => &m.id,
            Self::System(m) => &m.id,
            Self::Assistant(m) => &m.id,
            Self::User(m) => &m.id,
            Self::Tool(m) => &m.id,
            Self::Activity(m) => &m.id,
            Self::Reasoning(m) => &m.id,
        }
    }

    /// The role, whatever the variant.
    pub const fn role(&self) -> Role {
        match self {
            Self::Developer(_) => Role::Developer,
            Self::System(_) => Role::System,
            Self::Assistant(_) => Role::Assistant,
            Self::User(_) => Role::User,
            Self::Tool(_) => Role::Tool,
            Self::Activity(_) => Role::Activity,
            Self::Reasoning(_) => Role::Reasoning,
        }
    }
}

macro_rules! message_from {
    ($($ty:ident => $variant:ident),* $(,)?) => {
        $(
            impl From<$ty> for Message {
                fn from(value: $ty) -> Self {
                    Self::$variant(value)
                }
            }
        )*
    };
}

message_from! {
    DeveloperMessage => Developer,
    SystemMessage => System,
    AssistantMessage => Assistant,
    UserMessage => User,
    ToolMessage => Tool,
    ActivityMessage => Activity,
    ReasoningMessage => Reasoning,
}
