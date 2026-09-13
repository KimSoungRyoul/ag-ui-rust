//! Migration aliases for the renamed conversation API. New code uses [`crate::client::Thread`].
pub use super::thread::*;
/// Previous name for [`Thread`]. Request and state methods use the new contracts.
pub type Session<T, S = serde_json::Value> = Thread<T, S>;
/// Previous name for [`ThreadBuilder`].
pub type SessionBuilder<T, S = serde_json::Value> = ThreadBuilder<T, S>;
