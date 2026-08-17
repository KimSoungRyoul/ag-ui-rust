//! JSON Patch operations, as defined by [RFC 6902].
//!
//! `STATE_DELTA` and `ACTIVITY_DELTA` carry a patch document: an array of these
//! operations, applied in order to the previous snapshot.
//!
//! [RFC 6902]: https://datatracker.ietf.org/doc/html/rfc6902

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A JSON Patch document: an ordered list of operations.
pub type JsonPatch = Vec<PatchOperation>;

/// A single [RFC 6902] operation.
///
/// The serde representation is the RFC wire format exactly: an object with an
/// `op` discriminator, a `path` JSON Pointer, and — depending on the operation
/// — a `value` or a `from` pointer.
///
/// ```
/// # use ag_ui_core::PatchOperation;
/// let op = PatchOperation::Replace {
///     path: "/counter".into(),
///     value: serde_json::json!(2),
/// };
/// assert_eq!(
///     serde_json::to_string(&op).unwrap(),
///     r#"{"op":"replace","path":"/counter","value":2}"#
/// );
/// ```
///
/// [RFC 6902]: https://datatracker.ietf.org/doc/html/rfc6902
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub enum PatchOperation {
    /// Inserts `value` at `path`, shifting array elements right when `path`
    /// ends in an array index or `-`.
    Add {
        /// JSON Pointer to the location to add.
        path: String,
        /// The value to insert. `null` is a legitimate value, not an omission.
        value: Value,
    },

    /// Removes the value at `path`.
    Remove {
        /// JSON Pointer to the location to remove.
        path: String,
    },

    /// Replaces the value at `path`, which must already exist.
    Replace {
        /// JSON Pointer to the location to overwrite.
        path: String,
        /// The replacement value.
        value: Value,
    },

    /// Moves the value at `from` to `path`.
    Move {
        /// JSON Pointer to the source location.
        from: String,
        /// JSON Pointer to the destination.
        path: String,
    },

    /// Copies the value at `from` to `path`.
    Copy {
        /// JSON Pointer to the source location.
        from: String,
        /// JSON Pointer to the destination.
        path: String,
    },

    /// Asserts that the value at `path` equals `value`; a failed test aborts
    /// the whole patch.
    Test {
        /// JSON Pointer to the location to test.
        path: String,
        /// The value the location is expected to hold.
        value: Value,
    },
}

impl PatchOperation {
    /// Builds an [`Add`](PatchOperation::Add) operation.
    pub fn add(path: impl Into<String>, value: impl Into<Value>) -> Self {
        Self::Add {
            path: path.into(),
            value: value.into(),
        }
    }

    /// Builds a [`Remove`](PatchOperation::Remove) operation.
    pub fn remove(path: impl Into<String>) -> Self {
        Self::Remove { path: path.into() }
    }

    /// Builds a [`Replace`](PatchOperation::Replace) operation.
    pub fn replace(path: impl Into<String>, value: impl Into<Value>) -> Self {
        Self::Replace {
            path: path.into(),
            value: value.into(),
        }
    }

    /// Builds a [`Move`](PatchOperation::Move) operation.
    pub fn mv(from: impl Into<String>, path: impl Into<String>) -> Self {
        Self::Move {
            from: from.into(),
            path: path.into(),
        }
    }

    /// Builds a [`Copy`](PatchOperation::Copy) operation.
    pub fn copy(from: impl Into<String>, path: impl Into<String>) -> Self {
        Self::Copy {
            from: from.into(),
            path: path.into(),
        }
    }

    /// Builds a [`Test`](PatchOperation::Test) operation.
    pub fn test(path: impl Into<String>, value: impl Into<Value>) -> Self {
        Self::Test {
            path: path.into(),
            value: value.into(),
        }
    }

    /// The `op` string as it appears on the wire.
    pub const fn op(&self) -> &'static str {
        match self {
            Self::Add { .. } => "add",
            Self::Remove { .. } => "remove",
            Self::Replace { .. } => "replace",
            Self::Move { .. } => "move",
            Self::Copy { .. } => "copy",
            Self::Test { .. } => "test",
        }
    }

    /// The JSON Pointer this operation targets.
    pub fn path(&self) -> &str {
        match self {
            Self::Add { path, .. }
            | Self::Remove { path }
            | Self::Replace { path, .. }
            | Self::Move { path, .. }
            | Self::Copy { path, .. }
            | Self::Test { path, .. } => path,
        }
    }

    /// The source pointer of a `move` or `copy`, or `None` for other operations.
    pub fn from(&self) -> Option<&str> {
        match self {
            Self::Move { from, .. } | Self::Copy { from, .. } => Some(from),
            _ => None,
        }
    }

    /// The payload of an `add`, `replace` or `test`, or `None` for other
    /// operations.
    pub const fn value(&self) -> Option<&Value> {
        match self {
            Self::Add { value, .. } | Self::Replace { value, .. } | Self::Test { value, .. } => {
                Some(value)
            }
            _ => None,
        }
    }
}
