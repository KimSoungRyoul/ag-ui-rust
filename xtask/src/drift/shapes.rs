//! Every protocol shape that is not an event.
//!
//! The event comparison reads the event union and nothing else, so a wrong key
//! on a capabilities object, a message, an outcome, a tool or a patch passed
//! every run. One did: `MultiAgentCapabilities` wrote `subAgents` where AG-UI
//! 1.0 says `subagents`, and the check stayed green until someone read it.
//!
//! Here every `$defs` entry outside the event union is named in [`MAPPING`]
//! with the Rust type that speaks it, and the two are compared the way events
//! are — field names, requiredness and wire kinds — plus union members and
//! enum values. A definition with no row fails the check, so a shape upstream
//! adds cannot go unread. A difference this SDK keeps on purpose is a row in
//! [`EXCEPTIONS`] with its reason. It is printed on every run, and a row that
//! no longer matches anything fails, so the list cannot outlive what it
//! explains.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::baseline::Field;
use super::rust_src::{RustDef, RustField, RustType, RustTypes};
use super::text::wrap;
use super::{FieldDelta, bare_type, optionality, rust_wire_kind};

/// One non-event `$defs` entry, as far as a Rust type can be compared with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shape {
    /// The JSON kind of the definition itself (`object`, `string`, ...).
    pub kind: String,
    /// Properties, collected through `allOf` and `$ref`, with requiredness.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<Field>,
    /// Set when the definition is a `oneOf` of named definitions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub union: Option<Union>,
    /// The members of a string `enum`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
}

/// A union of named definitions and the property that tells them apart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Union {
    /// The property every member pins to a string constant. Empty when the
    /// members share none.
    pub discriminator: String,
    pub members: Vec<Member>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    /// The member's `$defs` name.
    pub schema: String,
    /// The constant it pins the discriminator to.
    pub tag: String,
}

/// How a definition is spoken in Rust.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rust {
    /// A struct whose fields are the definition's properties.
    Struct(&'static str),
    /// A tagged enum whose variants are the union's members.
    Union(&'static str),
    /// The variant — or variants, merged — of that tagged enum that carry
    /// this member's tag. A newtype variant's fields are its payload's.
    Variant(&'static str),
    /// A unit enum whose variants are the string enum's values.
    Values(&'static str),
    /// A type used by name; only its wire kind is compared.
    Alias(&'static str),
    /// Composed into other definitions through `allOf`, and compared there.
    Mixin,
}

/// Every non-event definition in the AG-UI 1.0 schema, and its Rust side.
pub const MAPPING: &[(&str, Rust)] = &[
    ("Attributable", Rust::Mixin),
    ("SubagentRunId", Rust::Alias("SubagentRunId")),
    ("Metadata", Rust::Alias("JsonObject")),
    ("State", Rust::Alias("Value")),
    ("TextMessageRole", Rust::Values("TextMessageRole")),
    ("Role", Rust::Values("Role")),
    (
        "ReasoningEncryptedValueSubtype",
        Rust::Values("ReasoningEncryptedValueSubtype"),
    ),
    ("RunFinishedOutcome", Rust::Union("RunOutcome")),
    ("RunFinishedSuccessOutcome", Rust::Variant("RunOutcome")),
    ("RunFinishedInterruptOutcome", Rust::Variant("RunOutcome")),
    ("RunFinishedCancelledOutcome", Rust::Variant("RunOutcome")),
    ("SubagentFinishedOutcome", Rust::Union("SubagentOutcome")),
    (
        "SubagentFinishedSuccessOutcome",
        Rust::Variant("SubagentOutcome"),
    ),
    (
        "SubagentFinishedSuspendedOutcome",
        Rust::Variant("SubagentOutcome"),
    ),
    ("Interrupt", Rust::Struct("Interrupt")),
    ("ResumeEntry", Rust::Struct("ResumeEntry")),
    ("TokenUsage", Rust::Struct("TokenUsage")),
    ("Message", Rust::Union("Message")),
    ("BaseMessage", Rust::Mixin),
    ("DeveloperMessage", Rust::Variant("Message")),
    ("SystemMessage", Rust::Variant("Message")),
    ("AssistantMessage", Rust::Variant("Message")),
    ("UserMessage", Rust::Variant("Message")),
    ("ToolMessage", Rust::Variant("Message")),
    ("ActivityMessage", Rust::Variant("Message")),
    ("ReasoningMessage", Rust::Variant("Message")),
    ("ToolCall", Rust::Struct("ToolCall")),
    ("FunctionCall", Rust::Struct("FunctionCall")),
    ("ContentPart", Rust::Union("InputContent")),
    ("TextPart", Rust::Variant("InputContent")),
    ("ImagePart", Rust::Variant("InputContent")),
    ("AudioPart", Rust::Variant("InputContent")),
    ("VideoPart", Rust::Variant("InputContent")),
    ("DocumentPart", Rust::Variant("InputContent")),
    ("PartSource", Rust::Union("InputContentSource")),
    ("DataSource", Rust::Variant("InputContentSource")),
    ("UrlSource", Rust::Variant("InputContentSource")),
    ("FileSource", Rust::Variant("InputContentSource")),
    ("Context", Rust::Struct("Context")),
    ("Tool", Rust::Struct("Tool")),
    ("RunAgentInput", Rust::Struct("RunAgentInput")),
    ("SubagentInfo", Rust::Struct("SubagentInfo")),
    ("IdentityCapabilities", Rust::Struct("IdentityCapabilities")),
    (
        "TransportCapabilities",
        Rust::Struct("TransportCapabilities"),
    ),
    ("ToolsCapabilities", Rust::Struct("ToolsCapabilities")),
    ("OutputCapabilities", Rust::Struct("OutputCapabilities")),
    ("StateCapabilities", Rust::Struct("StateCapabilities")),
    (
        "MultiAgentCapabilities",
        Rust::Struct("MultiAgentCapabilities"),
    ),
    (
        "ReasoningCapabilities",
        Rust::Struct("ReasoningCapabilities"),
    ),
    (
        "MultimodalInputCapabilities",
        Rust::Struct("MultimodalInputCapabilities"),
    ),
    (
        "MultimodalOutputCapabilities",
        Rust::Struct("MultimodalOutputCapabilities"),
    ),
    (
        "MultimodalCapabilities",
        Rust::Struct("MultimodalCapabilities"),
    ),
    (
        "ExecutionCapabilities",
        Rust::Struct("ExecutionCapabilities"),
    ),
    (
        "HumanInTheLoopCapabilities",
        Rust::Struct("HumanInTheLoopCapabilities"),
    ),
    ("AgentCapabilities", Rust::Struct("AgentCapabilities")),
    ("JsonPatch", Rust::Alias("JsonPatch")),
    ("JsonPatchOperation", Rust::Union("PatchOperation")),
    ("AddOperation", Rust::Variant("PatchOperation")),
    ("RemoveOperation", Rust::Variant("PatchOperation")),
    ("ReplaceOperation", Rust::Variant("PatchOperation")),
    ("MoveOperation", Rust::Variant("PatchOperation")),
    ("CopyOperation", Rust::Variant("PatchOperation")),
    ("TestOperation", Rust::Variant("PatchOperation")),
    ("JsonPointer", Rust::Alias("String")),
];

/// The ways a shape and its Rust type can disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Difference {
    /// In the schema, not in Rust: a field, a union member or an enum value.
    MissingInRust,
    /// In Rust, not in the schema.
    NotUpstream,
    /// Required on one side, optional on the other.
    Optionality,
    /// On both sides, as different JSON kinds.
    WireType,
    /// A Rust type this check cannot place on the wire.
    TypeReview,
    /// The Rust enum is tagged by another property than the union is.
    Discriminator,
    /// The Rust type [`MAPPING`] names is not there, or not that kind of type.
    RustType,
    /// A definition with no row in [`MAPPING`].
    Unmapped,
    /// A [`MAPPING`] row naming no definition.
    NotInSchema,
}

impl Difference {
    fn label(self) -> &'static str {
        match self {
            Self::MissingInRust => "missing in Rust",
            Self::NotUpstream => "not upstream",
            Self::Optionality => "optionality",
            Self::WireType => "wire type",
            Self::TypeReview => "type review needed",
            Self::Discriminator => "discriminator",
            Self::RustType => "rust type",
            Self::Unmapped => "unmapped",
            Self::NotInSchema => "not in schema",
        }
    }
}

/// One disagreement, before it is sorted into drift or a known difference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The `$defs` name.
    pub shape: String,
    /// The field, member or value it is about; empty for the whole shape.
    pub subject: String,
    pub difference: Difference,
    pub detail: String,
}

/// A difference this SDK keeps on purpose, with the reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exception {
    pub shape: &'static str,
    pub subject: &'static str,
    pub difference: Difference,
    pub why: &'static str,
}

const PATCH_VALUE: &str = "Read leniently: an omitted `value` reads as JSON null, because \
    JSON.stringify drops an `undefined` value and a JavaScript producer sends exactly that; \
    failing the whole STATE_DELTA over it would fail the run. Always written. See \
    crates/ag-ui/src/patch.rs.";

/// The differences this SDK keeps on purpose. Each is printed on every run;
/// one that stops matching fails the check until it is deleted here.
pub const EXCEPTIONS: &[Exception] = &[
    Exception {
        shape: "ContentPart",
        subject: "binary",
        difference: Difference::NotUpstream,
        why: "InputContent::Binary still reads and writes the 0.x `binary` part, which 1.0 \
              retired from ContentPart (upstream upgrades it with a shim instead). A peer \
              validating against 1.0 rejects a message that carries one.",
    },
    Exception {
        shape: "ToolCall",
        subject: "type",
        difference: Difference::Optionality,
        why: "Read leniently: `#[serde(default)]` takes a missing `type` as `function`. \
              Always written, so what this SDK sends still validates.",
    },
    Exception {
        shape: "AddOperation",
        subject: "value",
        difference: Difference::Optionality,
        why: PATCH_VALUE,
    },
    Exception {
        shape: "ReplaceOperation",
        subject: "value",
        difference: Difference::Optionality,
        why: PATCH_VALUE,
    },
    Exception {
        shape: "TestOperation",
        subject: "value",
        difference: Difference::Optionality,
        why: PATCH_VALUE,
    },
];

/// Everything the shape comparison found.
#[derive(Debug, Default)]
pub struct ShapeReport {
    /// Shapes compared with a Rust type of their own.
    pub compared: usize,
    /// Shapes compared only through the shapes that compose them.
    pub mixins: usize,
    pub rust_files: usize,
    pub rust_types: usize,
    /// Where each shape was found on the Rust side, for headings.
    pub located: BTreeMap<String, String>,
    pub drift: Vec<Finding>,
    pub known: Vec<(Finding, Exception)>,
    pub stale: Vec<Exception>,
    /// `(1.0 name, Rust name)` where the two differ. Informational.
    pub renamed: Vec<(&'static str, &'static str)>,
}

impl ShapeReport {
    pub fn is_clean(&self) -> bool {
        self.drift.is_empty() && self.stale.is_empty()
    }
}

/// Compares every non-event shape with the Rust type [`MAPPING`] names.
pub fn compare(shapes: &BTreeMap<String, Shape>, rust: &RustTypes) -> ShapeReport {
    compare_with(shapes, rust, MAPPING, EXCEPTIONS)
}

fn compare_with(
    shapes: &BTreeMap<String, Shape>,
    rust: &RustTypes,
    mapping: &[(&'static str, Rust)],
    exceptions: &[Exception],
) -> ShapeReport {
    let mut report = ShapeReport {
        rust_files: rust.files.len(),
        rust_types: rust.types.len(),
        ..ShapeReport::default()
    };
    let mut findings = Vec::new();

    for (name, shape) in shapes {
        let Some(&(schema, side)) = mapping.iter().find(|(schema, _)| *schema == name.as_str())
        else {
            findings.push(finding(
                name,
                "",
                Difference::Unmapped,
                "no row in MAPPING names the Rust type that speaks it",
            ));
            continue;
        };
        if side == Rust::Mixin {
            report.mixins += 1;
            continue;
        }
        report.compared += 1;
        if let Rust::Struct(ty) | Rust::Union(ty) | Rust::Values(ty) = side {
            if ty != schema {
                report.renamed.push((schema, ty));
            }
        }
        let mut out = Compared {
            name,
            rust,
            findings: &mut findings,
        };
        if let Some(located) = out.compare(shape, side, shapes) {
            report.located.insert(name.clone(), located);
        }
    }
    for (schema, _) in mapping {
        if !shapes.contains_key(*schema) {
            findings.push(finding(
                schema,
                "",
                Difference::NotInSchema,
                "MAPPING names it, but the schema no longer defines it",
            ));
        }
    }

    for found in findings {
        match exceptions.iter().find(|exception| {
            exception.shape == found.shape
                && exception.subject == found.subject
                && exception.difference == found.difference
        }) {
            Some(exception) => report.known.push((found, *exception)),
            None => report.drift.push(found),
        }
    }
    report.stale = exceptions
        .iter()
        .filter(|exception| !report.known.iter().any(|(_, known)| known == *exception))
        .copied()
        .collect();
    report
}

fn finding(shape: &str, subject: &str, difference: Difference, detail: &str) -> Finding {
    Finding {
        shape: shape.to_owned(),
        subject: subject.to_owned(),
        difference,
        detail: detail.to_owned(),
    }
}

/// One shape's comparison, collecting into the shared findings.
struct Compared<'a> {
    name: &'a str,
    rust: &'a RustTypes,
    findings: &'a mut Vec<Finding>,
}

impl<'a> Compared<'a> {
    fn push(&mut self, subject: &str, difference: Difference, detail: impl Into<String>) {
        self.findings.push(Finding {
            shape: self.name.to_owned(),
            subject: subject.to_owned(),
            difference,
            detail: detail.into(),
        });
    }

    /// Compares one shape; returns where its Rust side was found.
    fn compare(
        &mut self,
        shape: &Shape,
        side: Rust,
        shapes: &BTreeMap<String, Shape>,
    ) -> Option<String> {
        match side {
            Rust::Mixin => None,
            Rust::Alias(ty) => {
                match kind_of(ty, self.rust) {
                    Some(kind) if kind == shape.kind => {}
                    Some(kind) => self.push(
                        "",
                        Difference::WireType,
                        format!("schema: {}, Rust: {kind} (`{ty}`)", shape.kind),
                    ),
                    None => self.push(
                        "",
                        Difference::TypeReview,
                        format!("schema: {}, Rust: `{ty}`", shape.kind),
                    ),
                }
                Some(format!("`{ty}`"))
            }
            Rust::Struct(ty) => {
                let found = self.lookup(ty)?;
                let RustDef::Struct(fields) = &found.def else {
                    self.push(
                        "",
                        Difference::RustType,
                        format!("`{ty}` is an enum, not a struct"),
                    );
                    return None;
                };
                self.fields(&shape.fields, fields);
                Some(format!("{ty} ({})", found.file))
            }
            Rust::Values(ty) => {
                let found = self.lookup(ty)?;
                let RustDef::Enum { variants, .. } = &found.def else {
                    self.push(
                        "",
                        Difference::RustType,
                        format!("`{ty}` is a struct, not an enum"),
                    );
                    return None;
                };
                let wires = variants
                    .iter()
                    .map(|variant| variant.wire.as_str())
                    .collect::<BTreeSet<_>>();
                for value in &shape.values {
                    if !wires.contains(value.as_str()) {
                        self.push(value, Difference::MissingInRust, "enum value");
                    }
                }
                for wire in wires {
                    if !shape.values.iter().any(|value| value == wire) {
                        self.push(wire, Difference::NotUpstream, "enum value");
                    }
                }
                Some(format!("{ty} ({})", found.file))
            }
            Rust::Union(ty) => {
                let found = self.lookup(ty)?;
                let RustDef::Enum { tag, variants } = &found.def else {
                    self.push(
                        "",
                        Difference::RustType,
                        format!("`{ty}` is a struct, not an enum"),
                    );
                    return None;
                };
                let Some(union) = &shape.union else {
                    self.push(
                        "",
                        Difference::RustType,
                        "the schema no longer defines it as a union of named shapes",
                    );
                    return None;
                };
                if union.discriminator.is_empty() {
                    self.push(
                        "",
                        Difference::Discriminator,
                        "its members share no constant property to tell them apart",
                    );
                } else if tag.as_deref() != Some(union.discriminator.as_str()) {
                    self.push(
                        &union.discriminator,
                        Difference::Discriminator,
                        format!(
                            "Rust tags `{ty}` by {}",
                            tag.as_deref()
                                .map_or("nothing".to_owned(), |tag| format!("`{tag}`"))
                        ),
                    );
                }
                let wires = variants
                    .iter()
                    .map(|variant| variant.wire.as_str())
                    .collect::<BTreeSet<_>>();
                for member in &union.members {
                    if !wires.contains(member.tag.as_str()) {
                        self.push(
                            &member.tag,
                            Difference::MissingInRust,
                            format!("union member ({})", member.schema),
                        );
                    }
                }
                for wire in wires {
                    if !union.members.iter().any(|member| member.tag == wire) {
                        self.push(wire, Difference::NotUpstream, "union member");
                    }
                }
                Some(format!("{ty} ({})", found.file))
            }
            Rust::Variant(ty) => {
                let Some((discriminator, tag)) = membership(self.name, shapes) else {
                    self.push(
                        "",
                        Difference::RustType,
                        "MAPPING reads it as a union member, but no union in the schema lists it",
                    );
                    return None;
                };
                let found = self.lookup(ty)?;
                let RustDef::Enum { variants, .. } = &found.def else {
                    self.push(
                        "",
                        Difference::RustType,
                        format!("`{ty}` is a struct, not an enum"),
                    );
                    return None;
                };
                let carrying = variants
                    .iter()
                    .filter(|variant| variant.wire == tag)
                    .collect::<Vec<_>>();
                if carrying.is_empty() {
                    // The union's own comparison names the missing member.
                    return None;
                }
                let mut each = Vec::new();
                let mut payloads = Vec::new();
                for variant in carrying {
                    match (&variant.fields, &variant.payload) {
                        (Some(fields), _) => each.push(fields.clone()),
                        (None, Some(payload)) => {
                            let found = self.lookup(payload)?;
                            let RustDef::Struct(fields) = &found.def else {
                                self.push(
                                    "",
                                    Difference::RustType,
                                    format!("`{payload}` is an enum, not a struct"),
                                );
                                return None;
                            };
                            payloads.push(format!("{payload}, {}", found.file));
                            each.push(fields.clone());
                        }
                        (None, None) => each.push(Vec::new()),
                    }
                }
                let fields = shape
                    .fields
                    .iter()
                    .filter(|field| field.name != discriminator)
                    .cloned()
                    .collect::<Vec<_>>();
                self.fields(&fields, &merge(&each));
                Some(match payloads.as_slice() {
                    [] => format!("{ty} `{tag}` ({})", found.file),
                    _ => format!("{ty} `{tag}` ({})", payloads.join("; ")),
                })
            }
        }
    }

    fn lookup(&mut self, ty: &str) -> Option<&'a RustType> {
        let rust: &'a RustTypes = self.rust;
        match rust.get(ty) {
            Ok(found) => Some(found),
            Err(why) => {
                self.push("", Difference::RustType, why);
                None
            }
        }
    }

    fn fields(&mut self, schema: &[Field], rust: &[RustField]) {
        let types = self.rust;
        let delta = FieldDelta::between_with(schema, rust, &|ty: &str| kind_of(ty, types));
        for field in &delta.missing {
            let detail = format!("upstream: {}", optionality(field.required));
            self.push(&field.name, Difference::MissingInRust, detail);
        }
        for field in &delta.extra {
            let detail = format!("Rust: {}", optionality(field.required));
            self.push(&field.name, Difference::NotUpstream, detail);
        }
        for (name, up, rs) in &delta.optionality {
            let detail = format!("upstream: {}, Rust: {}", optionality(*up), optionality(*rs));
            self.push(name, Difference::Optionality, detail);
        }
        for (name, expected, actual) in &delta.type_mismatch {
            let detail = format!("schema: {expected}, Rust: {actual}");
            self.push(name, Difference::WireType, detail);
        }
        for (name, expected, ty) in &delta.type_unchecked {
            let detail = format!("schema: {expected}, Rust: {ty}");
            self.push(name, Difference::TypeReview, detail);
        }
    }
}

/// The union that lists `name`, as `(discriminator, tag)`.
fn membership(name: &str, shapes: &BTreeMap<String, Shape>) -> Option<(String, String)> {
    shapes.values().find_map(|shape| {
        let union = shape.union.as_ref()?;
        let member = union.members.iter().find(|member| member.schema == name)?;
        Some((union.discriminator.clone(), member.tag.clone()))
    })
}

/// Several variants sharing one tag read as one shape: a field is there if
/// any of them has it, and required only if all of them require it.
/// `RunOutcome` writes `success` from two variants, one of which carries
/// `pendingToolCallIds`, so on the wire that field is optional.
fn merge(variants: &[Vec<RustField>]) -> Vec<RustField> {
    let mut merged: Vec<RustField> = Vec::new();
    for field in variants.iter().flatten() {
        if !merged.iter().any(|seen| seen.name == field.name) {
            merged.push(field.clone());
        }
    }
    for field in &mut merged {
        field.required = variants.iter().all(|fields| {
            fields
                .iter()
                .any(|other| other.name == field.name && other.required)
        });
    }
    merged
}

/// The wire kind of a Rust field type, falling back to what the scan found
/// under that name: a struct with named fields or an internally tagged enum
/// is an object, a unit-only enum is a string.
fn kind_of(ty: &str, rust: &RustTypes) -> Option<String> {
    if let Some(kind) = rust_wire_kind(ty) {
        return Some(kind);
    }
    let found = rust.get(&bare_type(ty)).ok()?;
    match &found.def {
        RustDef::Struct(fields) if !fields.is_empty() => Some("object".into()),
        RustDef::Enum { tag: Some(_), .. } => Some("object".into()),
        RustDef::Enum {
            tag: None,
            variants,
        } if !variants.is_empty()
            && variants.iter().all(|variant| {
                variant.payload.is_none() && variant.fields.as_ref().is_some_and(Vec::is_empty)
            }) =>
        {
            Some("string".into())
        }
        _ => None,
    }
}

/// The comparison, as text.
pub fn render(report: &ShapeReport, scanned: &str) -> String {
    let mut out = String::from("\nshapes\n");
    out.push_str(&format!(
        "  upstream  {} $defs outside the event union ({} mixins compared through the shapes \
         that use them)\n",
        report.compared + report.mixins,
        report.mixins
    ));
    out.push_str(&format!(
        "  rust      {scanned}  ({} files, {} types)\n",
        report.rust_files, report.rust_types
    ));

    if !report.drift.is_empty() {
        let mut by_shape: BTreeMap<&str, Vec<&Finding>> = BTreeMap::new();
        for found in &report.drift {
            by_shape.entry(&found.shape).or_default().push(found);
        }
        out.push_str(&format!("\nSHAPE MISMATCHES — {}\n", by_shape.len()));
        out.push_str(
            "  The schema defines these shapes one way and the Rust type that speaks them\n  \
             another.\n\n",
        );
        for (shape, found) in &by_shape {
            match report.located.get(*shape) {
                Some(located) => out.push_str(&format!("    {shape}  ->  {located}\n")),
                None => out.push_str(&format!("    {shape}\n")),
            }
            for found in found {
                out.push_str(&format!(
                    "        {:<19}{:<24}{}\n",
                    found.difference.label(),
                    found.subject,
                    found.detail
                ));
            }
        }
        out.push_str(
            "\n  Fix: correct the Rust type. A new $defs entry needs a row in MAPPING; a\n  \
             difference kept on purpose needs a row in EXCEPTIONS with the reason. Both are\n  \
             in xtask/src/drift/shapes.rs.\n",
        );
    }

    if !report.stale.is_empty() {
        out.push_str(&format!("\nSTALE EXCEPTIONS — {}\n", report.stale.len()));
        out.push_str(
            "  These named exceptions no longer match any difference. Delete them from\n  \
             EXCEPTIONS in xtask/src/drift/shapes.rs, so the list stays true.\n\n",
        );
        for exception in &report.stale {
            out.push_str(&format!(
                "    {}  ({})\n",
                subject_of(exception.shape, exception.subject),
                exception.difference.label()
            ));
        }
    }

    if !report.known.is_empty() {
        out.push_str(&format!(
            "\nKNOWN DIFFERENCES — {} (named exceptions, not failures)\n",
            report.known.len()
        ));
        // One reason often covers several fields; print it once, under all.
        let mut reasons: Vec<(&str, Vec<&Finding>)> = Vec::new();
        for (found, exception) in &report.known {
            match reasons.iter_mut().find(|(why, _)| *why == exception.why) {
                Some((_, found_for)) => found_for.push(found),
                None => reasons.push((exception.why, vec![found])),
            }
        }
        for (why, found) in reasons {
            for found in found {
                out.push_str(&format!(
                    "    {} — {}, {}\n",
                    subject_of(&found.shape, &found.subject),
                    found.difference.label(),
                    found.detail
                ));
            }
            out.push_str(&wrap(why, "        ", 80));
        }
    }

    if !report.renamed.is_empty() {
        out.push_str(&format!(
            "\nNAMED DIFFERENTLY — {} (informational: the wire names match)\n",
            report.renamed.len()
        ));
        for (schema, rust) in &report.renamed {
            out.push_str(&format!("    {schema:<32}{rust}\n"));
        }
    }

    out.push('\n');
    if report.is_clean() {
        out.push_str(&format!(
            "OK  {} $defs shapes match the baseline{}.\n",
            report.compared,
            match report.known.len() {
                0 => String::new(),
                n => format!(", apart from {n} known difference(s)"),
            }
        ));
    } else {
        let shapes = report
            .drift
            .iter()
            .map(|found| found.shape.as_str())
            .collect::<BTreeSet<_>>()
            .len();
        out.push_str(&format!(
            "FAILED  {shapes} shape(s) with mismatches, {} stale exception(s).\n",
            report.stale.len()
        ));
    }
    out
}

fn subject_of(shape: &str, subject: &str) -> String {
    if subject.is_empty() {
        shape.to_owned()
    } else {
        format!("{shape}.{subject}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drift::rust_src::RustVariant;

    fn field(name: &str, required: bool, kind: &str) -> Field {
        Field {
            name: name.into(),
            required,
            kind: Some(kind.into()),
        }
    }

    fn rust_field(name: &str, required: bool, ty: &str) -> RustField {
        RustField {
            name: name.into(),
            required,
            ty: ty.into(),
        }
    }

    fn object(fields: Vec<Field>) -> Shape {
        Shape {
            kind: "object".into(),
            fields,
            union: None,
            values: vec![],
        }
    }

    fn structure(name: &str, fields: Vec<RustField>) -> RustType {
        RustType {
            name: name.into(),
            file: "crates/ag-ui/src/x.rs".into(),
            def: RustDef::Struct(fields),
        }
    }

    fn types(types: Vec<RustType>) -> RustTypes {
        RustTypes {
            files: vec!["crates/ag-ui/src/x.rs".into()],
            types,
        }
    }

    fn multi_agent(key: &str) -> (BTreeMap<String, Shape>, RustTypes) {
        let shapes = BTreeMap::from([(
            "MultiAgentCapabilities".to_owned(),
            object(vec![
                field("supported", false, "boolean"),
                field("subagents", false, "array"),
            ]),
        )]);
        let rust = types(vec![structure(
            "MultiAgentCapabilities",
            vec![
                rust_field("supported", false, "Option<bool>"),
                rust_field(key, false, "Option<Vec<SubagentInfo>>"),
            ],
        )]);
        (shapes, rust)
    }

    const MULTI_AGENT: &[(&str, Rust)] = &[(
        "MultiAgentCapabilities",
        Rust::Struct("MultiAgentCapabilities"),
    )];

    /// The miss this module exists for: the 0.x key on the 1.0 shape.
    #[test]
    fn the_subagents_key_is_drift() {
        let (shapes, rust) = multi_agent("subAgents");
        let report = compare_with(&shapes, &rust, MULTI_AGENT, &[]);
        assert!(!report.is_clean());
        assert_eq!(
            report
                .drift
                .iter()
                .map(|found| (found.subject.as_str(), found.difference))
                .collect::<Vec<_>>(),
            [
                ("subagents", Difference::MissingInRust),
                ("subAgents", Difference::NotUpstream)
            ]
        );
        let text = render(&report, "crates/ag-ui/src");
        assert!(text.contains("SHAPE MISMATCHES — 1"), "{text}");
        assert!(text.contains("missing in Rust    subagents"), "{text}");
        assert!(text.contains("FAILED"), "{text}");

        let (shapes, rust) = multi_agent("subagents");
        let report = compare_with(&shapes, &rust, MULTI_AGENT, &[]);
        assert!(report.is_clean(), "{:?}", report.drift);
        assert!(render(&report, "crates/ag-ui/src").contains("OK  1 $defs shapes match"));
    }

    #[test]
    fn a_shape_with_no_mapping_row_is_drift_and_so_is_a_row_with_no_shape() {
        let (shapes, rust) = multi_agent("subagents");
        let report = compare_with(&shapes, &rust, &[("Gone", Rust::Mixin)], &[]);
        let differences = report
            .drift
            .iter()
            .map(|found| (found.shape.as_str(), found.difference))
            .collect::<Vec<_>>();
        assert_eq!(
            differences,
            [
                ("MultiAgentCapabilities", Difference::Unmapped),
                ("Gone", Difference::NotInSchema)
            ]
        );
    }

    #[test]
    fn an_exception_is_printed_not_failed_and_a_stale_one_fails() {
        let (shapes, rust) = multi_agent("subagents");
        let mut shapes = shapes;
        shapes
            .get_mut("MultiAgentCapabilities")
            .unwrap()
            .fields
            .push(field("handoffs", true, "boolean"));
        let rust_with = |required| {
            let mut rust = rust.clone();
            let RustDef::Struct(fields) = &mut rust.types[0].def else {
                unreachable!()
            };
            fields.push(rust_field("handoffs", required, "bool"));
            rust
        };
        let exception = Exception {
            shape: "MultiAgentCapabilities",
            subject: "handoffs",
            difference: Difference::Optionality,
            why: "on purpose",
        };

        let report = compare_with(&shapes, &rust_with(false), MULTI_AGENT, &[exception]);
        assert!(report.is_clean(), "{:?}", report.drift);
        assert_eq!(report.known.len(), 1);
        let text = render(&report, "crates/ag-ui/src");
        assert!(text.contains("KNOWN DIFFERENCES — 1"), "{text}");
        assert!(text.contains("on purpose"), "{text}");
        assert!(text.contains("apart from 1 known difference"), "{text}");

        // Fixed in Rust: the exception explains nothing any more.
        let report = compare_with(&shapes, &rust_with(true), MULTI_AGENT, &[exception]);
        assert!(!report.is_clean());
        assert_eq!(report.stale, [exception]);
        assert!(render(&report, "crates/ag-ui/src").contains("STALE EXCEPTIONS — 1"));
    }

    fn union_of(discriminator: &str, members: &[(&str, &str)]) -> Shape {
        Shape {
            kind: "object".into(),
            fields: vec![],
            union: Some(Union {
                discriminator: discriminator.into(),
                members: members
                    .iter()
                    .map(|(schema, tag)| Member {
                        schema: (*schema).into(),
                        tag: (*tag).into(),
                    })
                    .collect(),
            }),
            values: vec![],
        }
    }

    fn tagged(name: &str, tag: &str, variants: Vec<RustVariant>) -> RustType {
        RustType {
            name: name.into(),
            file: "crates/ag-ui/src/outcome.rs".into(),
            def: RustDef::Enum {
                tag: Some(tag.into()),
                variants,
            },
        }
    }

    fn variant(wire: &str, fields: Option<Vec<RustField>>, payload: Option<&str>) -> RustVariant {
        RustVariant {
            wire: wire.into(),
            payload: payload.map(str::to_owned),
            fields,
        }
    }

    /// Two Rust variants writing one tag read as one member: the field only
    /// one of them carries is optional on the wire.
    #[test]
    fn variants_sharing_a_tag_merge_and_extra_members_are_named() {
        let shapes = BTreeMap::from([
            (
                "Outcome".to_owned(),
                union_of(
                    "type",
                    &[("Success", "success"), ("Cancelled", "cancelled")],
                ),
            ),
            (
                "Success".to_owned(),
                object(vec![
                    field("type", true, "string"),
                    field("pendingToolCallIds", false, "array"),
                ]),
            ),
            (
                "Cancelled".to_owned(),
                object(vec![field("type", true, "string")]),
            ),
        ]);
        let rust = types(vec![tagged(
            "RunOutcome",
            "type",
            vec![
                variant("success", Some(vec![]), None),
                variant(
                    "success",
                    Some(vec![rust_field(
                        "pendingToolCallIds",
                        true,
                        "Vec<ToolCallId>",
                    )]),
                    None,
                ),
                variant("cancelled", Some(vec![]), None),
                variant("paused", Some(vec![]), None),
            ],
        )]);
        let mapping = &[
            ("Outcome", Rust::Union("RunOutcome")),
            ("Success", Rust::Variant("RunOutcome")),
            ("Cancelled", Rust::Variant("RunOutcome")),
        ];
        let report = compare_with(&shapes, &rust, mapping, &[]);
        assert_eq!(
            report
                .drift
                .iter()
                .map(|found| (
                    found.shape.as_str(),
                    found.subject.as_str(),
                    found.difference
                ))
                .collect::<Vec<_>>(),
            [("Outcome", "paused", Difference::NotUpstream)]
        );
        assert_eq!(report.renamed, [("Outcome", "RunOutcome")]);
    }

    #[test]
    fn a_newtype_variant_is_compared_through_its_payload() {
        let shapes = BTreeMap::from([
            ("Part".to_owned(), union_of("type", &[("TextPart", "text")])),
            (
                "TextPart".to_owned(),
                object(vec![
                    field("type", true, "string"),
                    field("text", true, "string"),
                    field("source", true, "object"),
                ]),
            ),
        ]);
        let rust = types(vec![
            tagged(
                "InputContent",
                "kind",
                vec![variant("text", None, Some("TextInputContent"))],
            ),
            structure(
                "TextInputContent",
                vec![
                    rust_field("text", true, "String"),
                    rust_field("source", true, "InputContentSource"),
                ],
            ),
            tagged("InputContentSource", "type", vec![]),
        ]);
        let mapping = &[
            ("Part", Rust::Union("InputContent")),
            ("TextPart", Rust::Variant("InputContent")),
        ];
        let report = compare_with(&shapes, &rust, mapping, &[]);
        // The tag property is wrong; the payload's fields, and the kind of a
        // field typed by another scanned enum, are right.
        assert_eq!(
            report
                .drift
                .iter()
                .map(|found| (found.shape.as_str(), found.difference))
                .collect::<Vec<_>>(),
            [("Part", Difference::Discriminator)]
        );
        assert!(report.located["TextPart"].contains("TextInputContent"));
    }

    #[test]
    fn string_enum_values_and_alias_kinds_are_compared() {
        let shapes = BTreeMap::from([
            (
                "Role".to_owned(),
                Shape {
                    kind: "string".into(),
                    fields: vec![],
                    union: None,
                    values: vec!["user".into(), "tool".into()],
                },
            ),
            (
                "Metadata".to_owned(),
                Shape {
                    kind: "object".into(),
                    fields: vec![],
                    union: None,
                    values: vec![],
                },
            ),
        ]);
        let rust = types(vec![RustType {
            name: "Role".into(),
            file: "crates/ag-ui/src/message.rs".into(),
            def: RustDef::Enum {
                tag: None,
                variants: vec![
                    variant("user", Some(vec![]), None),
                    variant("bot", Some(vec![]), None),
                ],
            },
        }]);
        let mapping = &[
            ("Role", Rust::Values("Role")),
            ("Metadata", Rust::Alias("String")),
        ];
        let report = compare_with(&shapes, &rust, mapping, &[]);
        assert_eq!(
            report
                .drift
                .iter()
                .map(|found| (
                    found.shape.as_str(),
                    found.subject.as_str(),
                    found.difference
                ))
                .collect::<Vec<_>>(),
            [
                ("Metadata", "", Difference::WireType),
                ("Role", "tool", Difference::MissingInRust),
                ("Role", "bot", Difference::NotUpstream),
            ]
        );
    }

    #[test]
    fn unsigned_and_signed_integers_are_different_wire_kinds() {
        let shapes = BTreeMap::from([(
            "TokenUsage".to_owned(),
            object(vec![
                field("inputTokens", false, "integer>=0"),
                field("budget", false, "integer>=0"),
            ]),
        )]);
        let rust = types(vec![structure(
            "TokenUsage",
            vec![
                rust_field("inputTokens", false, "Option<u64>"),
                rust_field("budget", false, "Option<i64>"),
            ],
        )]);
        let report = compare_with(
            &shapes,
            &rust,
            &[("TokenUsage", Rust::Struct("TokenUsage"))],
            &[],
        );
        assert_eq!(report.drift.len(), 1, "{:?}", report.drift);
        assert_eq!(report.drift[0].subject, "budget");
        assert_eq!(report.drift[0].detail, "schema: integer>=0, Rust: integer");
    }

    /// The table is checked against the real schema and the real crate in
    /// `drift-check` itself; this keeps it honest about the schema alone.
    #[test]
    fn mapping_names_every_non_event_definition_once() {
        let schema = include_str!("../../../crates/ag-ui/src/protocol/schema-1.0.json");
        let surface = super::super::schema::extract(schema).unwrap();
        let named = MAPPING
            .iter()
            .map(|(schema, _)| *schema)
            .collect::<BTreeSet<_>>();
        assert_eq!(named.len(), MAPPING.len(), "a definition is mapped twice");
        assert_eq!(
            named,
            surface
                .shapes
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
        );
    }
}
