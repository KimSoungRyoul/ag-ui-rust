//! The protocol version axis.
//!
//! [`A2uiVersion`] is the one place that knows which A2UI versions exist and
//! how each is spelled on the wire. Nothing else in the crate compares a
//! version string: a `version` field is read with [`A2uiVersion::parse`] and
//! written with [`A2uiVersion::as_str`].
//!
//! Behaviour that differs from one version to the next is a method here too,
//! so the rest of the crate asks the version a question instead of matching on
//! it:
//!
//! | Question | Method |
//! |---|---|
//! | Which operations may a message carry? | [`A2uiVersion::operations`] |
//! | Which fields does an operation's payload have, and which are required? | `A2uiVersion::envelope` (crate-internal) |
//! | May a message of that version act on a surface created under this one? | [`A2uiVersion::accepts`] |
//!
//! Adding a version is therefore a change to this file: a variant, its
//! discriminator, and an arm in each method.
//!
//! ```
//! use ag_ui_a2ui::{A2uiVersion, Operation};
//!
//! // Reading and naming.
//! let version = A2uiVersion::parse("v0.9.1").expect("a version this crate speaks");
//! assert_eq!(version.as_str(), "v0.9.1");
//! assert_eq!(A2uiVersion::parse("v0.8"), None);
//!
//! // The versions and the operations are slices, so either list can grow.
//! let versions: &'static [A2uiVersion] = A2uiVersion::ALL;
//! let operations: &'static [Operation] = Operation::ALL;
//! assert!(versions.contains(&A2uiVersion::DEFAULT));
//! assert_eq!(version.operations(), operations);
//!
//! // A version-dependent question is a method, not a string comparison.
//! assert!(version.operations().contains(&Operation::UpdateDataModel));
//! assert!(A2uiVersion::V0_9.accepts(version), "v0.9.1 may update a v0.9 surface");
//! ```

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::catalog::PropType;
use super::operation::{EnvelopeField, Operation};
use crate::error::Error;

/// Supported A2UI server-message profiles. Candidate v1.0 RPC is excluded.
///
/// Non-exhaustive, so that a version can be added without breaking a `match`
/// written outside this crate: such a `match` needs a wildcard arm. Ask the
/// version a question ([`A2uiVersion::operations`], [`A2uiVersion::accepts`])
/// rather than matching on it where one fits.
///
/// ```compile_fail,E0004
/// use ag_ui_a2ui::A2uiVersion;
///
/// // No wildcard arm. This would stop compiling the day a version is added,
/// // so it does not compile today either.
/// fn discriminator(version: A2uiVersion) -> &'static str {
///     match version {
///         A2uiVersion::V0_9 => "v0.9",
///         A2uiVersion::V0_9_1 => "v0.9.1",
///     }
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum A2uiVersion {
    /// v0.9 wire discriminator, also accepted by the v0.9.1 schema.
    V0_9,
    /// v0.9.1 discriminator.
    V0_9_1,
}

/// Versions that share one wire format and may meet on one surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    /// `v0.9` and `v0.9.1`: the v0.9.1 schema accepts either discriminator.
    V0_9,
}

impl A2uiVersion {
    /// Every version this crate speaks, oldest first.
    ///
    /// A slice rather than an array: an array's length is part of its type,
    /// so adding a version would change the type of this constant.
    pub const ALL: &'static [Self] = &[Self::V0_9, Self::V0_9_1];

    /// The version a constructor stamps when the caller names none.
    pub const DEFAULT: Self = Self::V0_9;

    /// The exact wire discriminator.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V0_9 => "v0.9",
            Self::V0_9_1 => "v0.9.1",
        }
    }

    /// Reads a wire discriminator.
    ///
    /// `None` for anything this crate does not speak, including spellings of a
    /// supported version the wire does not use (`0.9`, `V0.9`).
    pub fn parse(discriminator: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|version| version.as_str() == discriminator)
    }

    /// The operations a message of this version may carry, in envelope order.
    pub fn operations(self) -> &'static [Operation] {
        match self.family() {
            Family::V0_9 => Operation::ALL,
        }
    }

    /// Whether a message of version `message` may act on a surface created
    /// under this one.
    ///
    /// The v0.9.1 schema accepts either v0.9-family discriminator, so the two
    /// may be mixed on one surface.
    pub fn accepts(self, message: Self) -> bool {
        self.family() == message.family()
    }

    /// The fields of an operation's payload in this version: name, JSON type,
    /// and whether the protocol requires it.
    ///
    /// Transcribed from the payload structs in [`crate::message`], which
    /// are the port of the wire format. Fields the specification leaves untyped
    /// (an `updateDataModel` value, a function's arguments) are simply absent:
    /// this is the envelope contract, not a schema for everything inside it.
    pub(crate) fn envelope(self, operation: Operation) -> &'static [EnvelopeField] {
        match self.family() {
            Family::V0_9 => match operation {
                Operation::CreateSurface => &[
                    ("surfaceId", PropType::String, true),
                    ("catalogId", PropType::String, true),
                    ("theme", PropType::Object, false),
                    ("sendDataModel", PropType::Boolean, false),
                ],
                Operation::UpdateComponents => &[
                    ("surfaceId", PropType::String, true),
                    ("components", PropType::Array, true),
                ],
                Operation::UpdateDataModel => &[
                    ("surfaceId", PropType::String, true),
                    ("path", PropType::String, false),
                ],
                Operation::DeleteSurface => &[("surfaceId", PropType::String, true)],
            },
        }
    }

    fn family(self) -> Family {
        match self {
            Self::V0_9 | Self::V0_9_1 => Family::V0_9,
        }
    }
}

impl fmt::Display for A2uiVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for A2uiVersion {
    type Err = Error;

    /// # Errors
    ///
    /// Returns [`Error::Parse`] for a discriminator [`A2uiVersion::parse`]
    /// does not recognize.
    fn from_str(discriminator: &str) -> Result<Self, Self::Err> {
        Self::parse(discriminator).ok_or_else(|| Error::parse("unsupported A2UI version"))
    }
}

impl Serialize for A2uiVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for A2uiVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let discriminator = String::deserialize(deserializer)?;
        Self::parse(&discriminator)
            .ok_or_else(|| serde::de::Error::custom("unsupported A2UI version"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_version_round_trips_through_its_discriminator() {
        for &version in A2uiVersion::ALL {
            assert_eq!(A2uiVersion::parse(version.as_str()), Some(version));
            assert_eq!(version.as_str().parse::<A2uiVersion>().unwrap(), version);
            assert_eq!(version.to_string(), version.as_str());
            // serde spells a version exactly as `as_str` does: one list of names.
            assert_eq!(serde_json::to_value(version).unwrap(), version.as_str());
            assert_eq!(
                serde_json::from_value::<A2uiVersion>(json!(version.as_str())).unwrap(),
                version
            );
        }
    }

    #[test]
    fn a_discriminator_this_crate_does_not_speak_is_not_a_version() {
        for other in ["", "0.9", "V0.9", "v0.8", "v0.9.2", "1.0", "v1.0", " v0.9"] {
            assert_eq!(A2uiVersion::parse(other), None, "{other:?}");
            assert!(matches!(
                other.parse::<A2uiVersion>(),
                Err(Error::Parse(reason)) if reason == "unsupported A2UI version"
            ));
            assert!(serde_json::from_value::<A2uiVersion>(json!(other)).is_err());
        }
        assert!(serde_json::from_value::<A2uiVersion>(json!(0.9)).is_err());
    }

    #[test]
    fn the_default_is_one_of_the_supported_versions() {
        assert!(A2uiVersion::ALL.contains(&A2uiVersion::DEFAULT));
        assert_eq!(crate::constants::PROTOCOL_VERSION, "v0.9");
        assert_eq!(
            crate::constants::PROTOCOL_VERSION,
            A2uiVersion::DEFAULT.as_str()
        );
    }

    #[test]
    fn the_v0_9_family_shares_surfaces_operations_and_envelopes() {
        for &created in A2uiVersion::ALL {
            for &message in A2uiVersion::ALL {
                assert!(created.accepts(message), "{created} surface, {message}");
            }
            assert_eq!(created.operations(), Operation::ALL);
            for &operation in Operation::ALL {
                let fields = created.envelope(operation);
                assert_eq!(
                    fields[0],
                    ("surfaceId", PropType::String, true),
                    "every {created} operation addresses a surface"
                );
            }
        }
    }
}
