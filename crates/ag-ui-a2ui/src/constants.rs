//! Wire-contract constants shared across every A2UI toolkit.
//!
//! # These values are a cross-language wire contract
//!
//! Every shipping A2UI toolkit (TypeScript, .NET, Python) hard-codes the same
//! strings, and the .NET constants file declares them a "cross-language wire
//! contract" that "must not diverge". A renderer written against the TypeScript
//! toolkit will silently fail to see this crate's output if any of them change.
//!
//! **Do not change these to match a newer spec revision on its own.** The A2UI
//! specification is at v1.0, but the interoperable toolkits still stamp
//! [`PROTOCOL_VERSION`] `"v0.9"` — so this crate does too. Changing a value here
//! is a breaking protocol change that has to land in every language at once.

/// Envelope key carrying a batch of A2UI operations over the transport.
///
/// The frontend detects an A2UI payload by looking for exactly this key, so it
/// doubles as the content sniff.
pub const A2UI_OPERATIONS_KEY: &str = "a2ui_operations";

/// `catalogId` for the standard basic component catalog.
///
/// Note that the upstream specification repository serves the same catalog
/// document under `.../v0_9/catalogs/basic/catalog.json`, and the document's own
/// `catalogId` field carries that longer URI. The value below is what the
/// shipping toolkits negotiate with, and a `catalogId` is an opaque identifier
/// rather than a resolvable URL, so this is the one that matters on the wire.
pub const BASIC_CATALOG_ID: &str = "https://a2ui.org/specification/v0_9/basic_catalog.json";

/// `surfaceId` used when the caller does not supply one.
pub const DEFAULT_SURFACE_ID: &str = "dynamic-surface";

/// Value stamped into the `version` field of every message this crate emits.
pub const PROTOCOL_VERSION: &str = "v0.9";

/// Planner-facing tool name: "build me a surface for this request".
pub const GENERATE_A2UI_TOOL_NAME: &str = "generate_a2ui";

/// Inner structured-output tool name the generating model calls to emit a surface.
pub const RENDER_A2UI_TOOL_NAME: &str = "render_a2ui";

/// Total generation attempts before the recovery loop gives up.
pub const MAX_A2UI_ATTEMPTS: u32 = 3;

/// Activity type used to report recovery-loop progress to the caller.
pub const A2UI_RECOVERY_ACTIVITY_TYPE: &str = "a2ui_recovery";

/// MIME type for a standalone A2UI payload (A2A parts, MCP resources, HTTP bodies).
pub const MIME_TYPE: &str = "application/a2ui+json";

/// The `id` the root of a surface's component tree must have.
///
/// Fixed by the specification: "One of the components in one of the component
/// lists MUST have an `id` of `root`".
pub const ROOT_ID: &str = "root";

/// Opening tag an LLM wraps a raw A2UI JSON block in.
pub const A2UI_OPEN_TAG: &str = "<a2ui-json>";

/// Closing tag an LLM wraps a raw A2UI JSON block in.
pub const A2UI_CLOSE_TAG: &str = "</a2ui-json>";

/// Component type name reserved for the implicit surface container.
///
/// `createSurface` instantiates it with `child: "root"`; it can never appear in
/// an `updateComponents` list. It is meaningful in composition constraints:
/// `allowedParents: ["Surface"]` restricts a component to being the root.
pub const SURFACE_COMPONENT: &str = "Surface";
