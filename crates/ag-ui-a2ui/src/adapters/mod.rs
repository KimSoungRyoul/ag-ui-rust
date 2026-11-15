//! A2UI over a transport.
//!
//! The protocol modules and the agent layer know nothing of how a message
//! reaches a renderer. An adapter is where one transport's conventions live:
//! what an A2UI message is wrapped in, where a renderer's capabilities and a
//! user's action arrive, how a surface is found again in a conversation.
//!
//! | Adapter | Feature | Transport |
#![cfg_attr(
    feature = "ag-ui",
    doc = "| [`agui`] | `ag-ui`, and `ag-ui-server` to paint from a run | AG-UI, as AG-UI's own A2UI middleware and toolkit speak it. |"
)]
#![cfg_attr(
    not(feature = "ag-ui"),
    doc = "| `agui` | `ag-ui` (off in this build) | AG-UI, as AG-UI's own A2UI middleware and toolkit speak it. |"
)]
#![cfg_attr(
    feature = "a2a",
    doc = "| [`a2a`] | `a2a` | A2A: data parts, the agent-card extension, capabilities in message metadata. No A2A library is needed. |"
)]
#![cfg_attr(
    not(feature = "a2a"),
    doc = "| `a2a` | `a2a` (off in this build) | A2A: data parts, the agent-card extension, capabilities in message metadata. |"
)]

#[cfg(feature = "a2a")]
pub mod a2a;
#[cfg(feature = "ag-ui")]
pub mod agui;
