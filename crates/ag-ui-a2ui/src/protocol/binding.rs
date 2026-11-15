//! Data-model binding: JSON Pointer resolution, scopes, and `formatString`.
//!
//! A2UI keeps UI structure and application state apart. Components reference
//! state through [JSON Pointers][rfc6901], resolved against the surface's data
//! model at render time.
//!
//! # Absolute and relative paths
//!
//! A2UI deliberately extends RFC 6901 with *relative* paths to make template
//! iteration expressible:
//!
//! - A path starting with `/` is **absolute** and always resolves from the root
//!   of the data model, wherever the component sits in the tree.
//! - A path **not** starting with `/` is **relative** and resolves inside the
//!   current collection scope. A container whose `children` is a template over
//!   `/employees` opens one scope per element, so `name` inside the template
//!   resolves to `/employees/0/name`, `/employees/1/name`, and so on.
//! - The empty path and `.` are relative too, and name the scope itself: the
//!   element a template is drawing, which is how a template over a list of
//!   strings shows each one.
//!
//! [`Scope`] models exactly this: [`Scope::root`] for the default scope and
//! [`Scope::item`] to descend into one element of a bound collection.
//! [`absolute_path`] is the rule on its own, for a caller that holds a scope's
//! path and no data.
//!
//! # How a binding is written
//!
//! Paths resolve the same way in every protocol version. What differs is the
//! object that carries one. Before v1.0 a binding is `{"path": …}` and a
//! function call `{"call": …, "args": …}`. In v1.0 they are `{"@path": …}`
//! and `{"@call": …, "args": …}`, a plain `{"path": …}` is a literal map, and
//! a key that starts with one `@` is reserved: a literal one is written
//! doubled, `@@type`. A list template, `{"componentId": …, "path": …}`, is
//! the same in both. [`Scope::speaking`] and [`collect_bindings_in`] take the
//! version; [`Scope::root`] and [`collect_bindings`] read the v0.9 family.
//!
//! ```
//! use ag_ui_a2ui::A2uiVersion;
//! use ag_ui_a2ui::binding::Scope;
//! use serde_json::json;
//!
//! let data = json!({"user": {"name": "Ada"}});
//! let v1_0 = Scope::root(&data).speaking(A2uiVersion::V1_0);
//! assert_eq!(v1_0.resolve_dynamic(&json!({"@path": "/user/name"}))?, json!("Ada"));
//! // A literal map, with an escaped key.
//! assert_eq!(
//!     v1_0.resolve_dynamic(&json!({"path": "/var/log", "@@type": "File"}))?,
//!     json!({"path": "/var/log", "@type": "File"})
//! );
//! # Ok::<(), ag_ui_a2ui::Error>(())
//! ```
//!
//! # Stringification
//!
//! Whenever a non-string is interpolated into text, the conversion is fixed by
//! spec so every renderer agrees: numbers and booleans use their standard
//! representation, `null` and missing paths become `""`, and objects and arrays
//! are stringified as compact JSON.
//!
//! ```
//! use ag_ui_a2ui::binding::Scope;
//! use serde_json::json;
//!
//! let data = json!({"company": "Acme", "employees": [{"name": "Alice"}, {"name": "Bob"}]});
//! let root = Scope::root(&data);
//! let item = root.item("/employees", 1);
//!
//! // Relative inside the collection scope, absolute escapes back to the root.
//! assert_eq!(item.format_string("${name} @ ${/company} (#${@index(offset: 1)})").unwrap(),
//!            "Bob @ Acme (#2)");
//! ```
//!
//! # Depth
//!
//! Resolution walks a `serde_json::Value`, and a walk over a tree is a stack
//! frame per level unless something stops it. Two things do. Anything arriving
//! over the wire has already passed through `serde_json`, which refuses to
//! parse deeper than 127 levels, so a `Value` from a model or a transport is
//! bounded before this module sees it. On top of that,
//! [`Scope::resolve_dynamic`] and [`collect_bindings`] carry their own cap
//! ([`MAX_VALUE_DEPTH`]) for values built in memory, which `serde_json` does not
//! check. The component *graph* — where depth is genuinely unbounded — is walked
//! iteratively in [`crate::validate`] instead.
//!
//! # Templates
//!
//! A `formatString` template has one grammar, and [`parse_template`] is it:
//! the template in, its parts out, nothing evaluated. [`Scope::format_string`]
//! parses with it and evaluates the parts, so what parses and what evaluates
//! cannot differ. The parser bounds a template's length, its number of parts
//! and how deep an expression nests, because a template is model output too.
//!
//! [rfc6901]: https://datatracker.ietf.org/doc/html/rfc6901

use std::collections::BTreeMap;

use jsonptr::Pointer;
use serde_json::{Map, Value};

pub use super::template::{
    MAX_TEMPLATE_CHARS, MAX_TEMPLATE_DEPTH, MAX_TEMPLATE_PARTS, TemplatePart, parse_template,
};
use super::version::A2uiVersion;
use crate::error::{Error, Result};

/// Deepest nesting these walks will descend into a value, and the most
/// segments a [`DataModel`](crate::DataModel) path may have.
///
/// Well past `serde_json`'s own parse limit of 127, so it never fires on a
/// value that arrived as text; it exists for values assembled in memory, which
/// nothing else bounds. A path is the exception: it arrives as one flat string
/// that no parse limit counts, yet each segment is a level of nesting in the
/// model. [`DataModel::apply`](crate::DataModel::apply) and
/// [`DataModel::lookup`](crate::DataModel::lookup) reject a longer one with
/// [`Error::Pointer`].
pub const MAX_VALUE_DEPTH: usize = 256;

/// Whether a string parses as an RFC 6901 JSON Pointer.
///
/// The rule that catches real mistakes is the escape alphabet: `~` may only be
/// followed by `0` or `1`, so a path written with a raw `~` in a key is invalid
/// rather than merely absent.
pub(crate) fn pointer_is_valid(path: &str) -> bool {
    Pointer::parse(path).is_ok()
}

/// The absolute data-model path a binding path resolves to in a scope.
///
/// `scope` is the absolute path of the scope the binding sits in: `""` or `/`
/// for the root scope, `/employees/1` inside the second item of a template
/// over `/employees`. `path` is the path as the component wrote it.
///
/// - A path that starts with `/` is absolute. It is returned as it is,
///   whatever the scope.
/// - Any other path is relative, and is joined to the scope.
/// - The empty path and `.` name the scope itself: the item a template is
///   drawing, or the whole model in the root scope.
///
/// The root is written `/`, the way `updateDataModel` writes it, and
/// [`DataModel::lookup`](crate::DataModel::lookup) and
/// [`DataModel::apply`](crate::DataModel::apply) take the result as it is.
/// [`Scope::resolve_pointer`] resolves the same way and writes the root as the
/// empty RFC 6901 pointer instead.
///
/// No segment is rewritten. A scope that ends in `/` ends in the empty key, as
/// RFC 6901 reads it, so `name` under `/user/` is `/user//name`. Only `/` on
/// its own is the root.
///
/// ```
/// use ag_ui_a2ui::binding::absolute_path;
///
/// assert_eq!(absolute_path("/employees/1", "name"), "/employees/1/name");
/// assert_eq!(absolute_path("/employees/1", "/company"), "/company");
/// // The scope itself: the item, or the whole model in the root scope.
/// assert_eq!(absolute_path("/employees/1", ""), "/employees/1");
/// assert_eq!(absolute_path("/employees/1", "."), "/employees/1");
/// assert_eq!(absolute_path("/", "."), "/");
/// assert_eq!(absolute_path("", "name"), "/name");
/// ```
pub fn absolute_path(scope: &str, path: &str) -> String {
    if path.starts_with('/') {
        return path.to_string();
    }
    let scope = if scope == "/" { "" } else { scope };
    if names_the_scope(path) {
        return if scope.is_empty() {
            "/".to_string()
        } else {
            scope.to_string()
        };
    }
    format!("{scope}/{path}")
}

/// Whether a path is one of the two spellings of "the scope itself".
pub(crate) fn names_the_scope(path: &str) -> bool {
    path.is_empty() || path == "."
}

/// [`absolute_path`] as an RFC 6901 pointer, where the whole document is the
/// empty string.
fn scoped_pointer(base: &str, path: &str) -> String {
    let absolute = absolute_path(base, path);
    if absolute == "/" {
        String::new()
    } else {
        absolute
    }
}

/// Renders a value as a string using A2UI's fixed conversion rules.
///
/// `None` (an unresolved path) and `Value::Null` both become `""`.
pub fn stringify(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => n.to_string(),
        // Objects and arrays are compact JSON so every renderer agrees.
        Some(other) => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// Supplies renderer-side functions to `formatString` beyond the built-ins.
///
/// This crate does not render, so it evaluates only what it can decide on its
/// own: data-model paths and `@index`. Pass a resolver to
/// [`Scope::format_string_with`] to evaluate catalog functions such as
/// `formatDate` or `formatCurrency`.
pub trait FunctionResolver {
    /// Evaluates `name` with the given already-resolved arguments.
    ///
    /// Return `None` for a function this resolver does not know; the caller then
    /// reports it as an unknown function.
    fn call(&self, name: &str, args: &Map<String, Value>) -> Option<Value>;
}

impl<F> FunctionResolver for F
where
    F: Fn(&str, &Map<String, Value>) -> Option<Value>,
{
    fn call(&self, name: &str, args: &Map<String, Value>) -> Option<Value> {
        self(name, args)
    }
}

struct NoFunctions;

impl FunctionResolver for NoFunctions {
    fn call(&self, _name: &str, _args: &Map<String, Value>) -> Option<Value> {
        None
    }
}

/// An evaluation scope: a data model plus the collection context bindings
/// resolve against.
///
/// The root scope resolves relative paths from `/`. Entering a template item
/// with [`Scope::item`] pushes the item's pointer as the new base and records
/// the iteration index for [`@index`](Scope::index).
#[derive(Debug, Clone)]
pub struct Scope<'a> {
    data: &'a Value,
    base: String,
    index: Option<usize>,
    version: A2uiVersion,
}

impl<'a> Scope<'a> {
    /// The root scope of a data model, reading dynamic values as the v0.9
    /// family writes them ([`A2uiVersion::DEFAULT`]). For a surface of
    /// another version, say so with [`Scope::speaking`].
    pub fn root(data: &'a Value) -> Self {
        Self {
            data,
            base: String::new(),
            index: None,
            version: A2uiVersion::DEFAULT,
        }
    }

    /// The same scope, reading dynamic values as `version` writes them: which
    /// key makes an object a binding or a call, and whether `@` is reserved.
    /// Scopes entered from it with [`Scope::item`] speak the same version.
    #[must_use]
    pub fn speaking(mut self, version: A2uiVersion) -> Self {
        self.version = version;
        self
    }

    /// The version this scope reads dynamic values in.
    pub fn version(&self) -> A2uiVersion {
        self.version
    }

    /// The data model this scope reads from.
    pub fn data(&self) -> &'a Value {
        self.data
    }

    /// The absolute pointer this scope resolves relative paths against.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// The iteration index, when this scope is a collection item.
    pub fn index(&self) -> Option<usize> {
        self.index
    }

    /// Descends into element `index` of the collection at `collection_path`.
    ///
    /// `collection_path` is itself resolved through this scope, so nested
    /// templates compose.
    #[must_use]
    pub fn item(&self, collection_path: &str, index: usize) -> Scope<'a> {
        let base = self.resolve_pointer(collection_path);
        Scope {
            data: self.data,
            base: format!("{base}/{index}"),
            index: Some(index),
            version: self.version,
        }
    }

    /// Turns an A2UI path (absolute or relative) into an absolute JSON Pointer.
    ///
    /// Resolved as [`absolute_path`] resolves it, against [`Scope::base`]: the
    /// empty path and `.` are this scope itself. The whole data model, which
    /// is `/` or the root scope itself, is the empty pointer.
    pub fn resolve_pointer(&self, path: &str) -> String {
        scoped_pointer(&self.base, path)
    }

    /// Resolves a path to a value in the data model, or `None` if absent.
    pub fn resolve(&self, path: &str) -> Option<&'a Value> {
        let pointer = self.resolve_pointer(path);
        if pointer.is_empty() {
            return Some(self.data);
        }
        Pointer::parse(&pointer)
            .ok()
            .and_then(|p| p.resolve(self.data).ok())
    }

    /// Resolves a path and stringifies the result.
    pub fn resolve_string(&self, path: &str) -> String {
        stringify(self.resolve(path))
    }

    /// Resolves a `DynamicValue`: a literal, a binding or a function call, as
    /// this scope's version writes them (`{"path": …}` and `{"call": …}`, or
    /// `{"@path": …}` and `{"@call": …}` from v1.0).
    ///
    /// Objects and arrays are walked recursively so nested bindings inside an
    /// action `context` resolve too. Where `@` is reserved, a doubled `@@` at
    /// the start of a literal key is unescaped to one.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Binding`] if a `formatString` template is malformed or
    /// names a function this crate cannot evaluate, and, where `@` is
    /// reserved, for a key that starts with one `@` and is not a directive.
    pub fn resolve_dynamic(&self, value: &Value) -> Result<Value> {
        self.resolve_dynamic_with(value, &NoFunctions)
    }

    /// [`Scope::resolve_dynamic`] with catalog functions supplied by the caller.
    ///
    /// # Errors
    ///
    /// See [`Scope::resolve_dynamic`].
    pub fn resolve_dynamic_with(
        &self,
        value: &Value,
        functions: &dyn FunctionResolver,
    ) -> Result<Value> {
        self.resolve_dynamic_at(value, functions, 0)
    }

    fn resolve_dynamic_at(
        &self,
        value: &Value,
        functions: &dyn FunctionResolver,
        depth: usize,
    ) -> Result<Value> {
        if depth > MAX_VALUE_DEPTH {
            return Err(Error::binding(
                "<value>",
                format!("value nests deeper than {MAX_VALUE_DEPTH} levels"),
            ));
        }
        match value {
            Value::Object(map) => {
                if let Some(Value::String(path)) = map.get(self.version.path_key()) {
                    // `{componentId, path}` is a child template, not a binding.
                    if !map.contains_key("componentId") {
                        return Ok(self.resolve(path).cloned().unwrap_or(Value::Null));
                    }
                }
                if let Some(Value::String(name)) = map.get(self.version.call_key()) {
                    let mut args = Map::new();
                    if let Some(Value::Object(raw)) = map.get("args") {
                        for (key, raw_value) in raw {
                            args.insert(
                                key.clone(),
                                self.resolve_dynamic_at(raw_value, functions, depth + 1)?,
                            );
                        }
                    }
                    return self.call_function(name, &args, functions);
                }
                let mut out = Map::new();
                for (key, raw_value) in map {
                    let literal = match literal_key(self.version, key) {
                        Some(literal) => literal,
                        None => {
                            return Err(Error::binding(
                                key,
                                "a key that starts with one '@' is reserved; write a literal \
                                 one doubled, as '@@'",
                            ));
                        }
                    };
                    out.insert(
                        literal.to_string(),
                        self.resolve_dynamic_at(raw_value, functions, depth + 1)?,
                    );
                }
                Ok(Value::Object(out))
            }
            Value::Array(items) => items
                .iter()
                .map(|item| self.resolve_dynamic_at(item, functions, depth + 1))
                .collect::<Result<Vec<_>>>()
                .map(Value::Array),
            other => Ok(other.clone()),
        }
    }

    fn call_function(
        &self,
        name: &str,
        args: &Map<String, Value>,
        functions: &dyn FunctionResolver,
    ) -> Result<Value> {
        match name {
            "formatString" => {
                let template = stringify(args.get("value"));
                self.format_string_with(&template, functions)
                    .map(Value::String)
            }
            "@index" => self.eval_index(args).map(Value::from),
            _ => functions.call(name, args).ok_or_else(|| {
                Error::binding(name, "function is not evaluable outside a renderer")
            }),
        }
    }

    fn eval_index(&self, args: &Map<String, Value>) -> Result<i64> {
        // Per spec, `@index` is only meaningful inside a collection scope.
        let index = self
            .index
            .ok_or_else(|| Error::binding("@index", "used outside of a template iteration scope"))?
            as i64;
        let offset = match args.get("offset") {
            None | Some(Value::Null) => 0,
            Some(Value::Number(n)) => n
                .as_i64()
                .ok_or_else(|| Error::binding("@index", "offset must be an integer"))?,
            Some(_) => return Err(Error::binding("@index", "offset must be a number")),
        };
        // The template comes from the model, so the offset is remote input.
        // A wrapping add would hand the renderer a nonsense index; a plain one
        // would abort the process on a debug build.
        index
            .checked_add(offset)
            .ok_or_else(|| Error::binding("@index", "offset is out of range"))
    }

    /// Interpolates a `formatString` template against this scope.
    ///
    /// The template is parsed by [`parse_template`], which states the grammar:
    /// `${…}` holds an absolute path (`${/user/name}`), a relative path
    /// (`${name}`), a literal, a nested `${…}`, or a function call
    /// (`${@index(offset: 1)}`), and a literal `${` is written `\${`. Each
    /// part is then evaluated and stringified by [`stringify()`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Binding`] for a template [`parse_template`] rejects, or
    /// a function this crate cannot evaluate.
    pub fn format_string(&self, template: &str) -> Result<String> {
        self.format_string_with(template, &NoFunctions)
    }

    /// [`Scope::format_string`] with catalog functions supplied by the caller.
    ///
    /// # Errors
    ///
    /// See [`Scope::format_string`].
    pub fn format_string_with(
        &self,
        template: &str,
        functions: &dyn FunctionResolver,
    ) -> Result<String> {
        let mut out = String::with_capacity(template.len());
        for part in parse_template(template)? {
            let value = self.evaluate(&part, functions)?;
            out.push_str(&stringify(Some(&value)));
        }
        Ok(out)
    }

    /// The value of one parsed part. The parser bounds how deep a part nests,
    /// so this may recurse over it.
    fn evaluate(&self, part: &TemplatePart, functions: &dyn FunctionResolver) -> Result<Value> {
        match part {
            TemplatePart::Literal(value) => Ok(value.clone()),
            TemplatePart::Path(path) => Ok(self.resolve(path).cloned().unwrap_or(Value::Null)),
            TemplatePart::Call { name, args } => {
                let mut resolved = Map::new();
                for (argument, value) in args {
                    resolved.insert(argument.clone(), self.evaluate(value, functions)?);
                }
                self.call_function(name, &resolved, functions)
            }
        }
    }
}

/// The key of a literal map as it is meant, or `None` for one the version
/// reserves.
///
/// Where `@` is reserved ([`A2uiVersion::reserves_at_keys`]), `@@type` is the
/// literal `@type`, and a key that starts with one `@` is a directive and no
/// literal at all. Elsewhere every key is itself.
pub(crate) fn literal_key(version: A2uiVersion, key: &str) -> Option<&str> {
    if !version.reserves_at_keys() {
        return Some(key);
    }
    match key.strip_prefix('@') {
        None => Some(key),
        Some(rest) if rest.starts_with('@') => Some(rest),
        Some(_) => None,
    }
}

/// Every data-model path referenced anywhere inside a JSON value, read as the
/// v0.9 family writes a binding: [`collect_bindings_in`] for
/// [`A2uiVersion::DEFAULT`].
pub fn collect_bindings(value: &Value) -> Vec<Binding> {
    collect_bindings_in(A2uiVersion::DEFAULT, value)
}

/// Every data-model path referenced anywhere inside a JSON value, read as
/// `version` writes a binding.
///
/// Walks the value looking for bindings (`{"path": "..."}`, or
/// `{"@path": "..."}` from v1.0) and for `{"componentId", "path"}` child
/// templates, whose `path` points at a collection rather than at a bound
/// value. Paths are returned in encounter order with duplicates removed, keyed
/// by the property path they were found at so a validator can report a precise
/// locator.
///
/// ```
/// use ag_ui_a2ui::A2uiVersion;
/// use ag_ui_a2ui::binding::collect_bindings_in;
/// use serde_json::json;
///
/// let component = json!({
///     "text": {"@path": "/user/name"},
///     "children": {"componentId": "row", "path": "/items"},
///     "details": {"path": "/var/log"}
/// });
/// let found = collect_bindings_in(A2uiVersion::V1_0, &component);
/// let paths: Vec<(&str, bool)> = found.iter().map(|b| (b.path.as_str(), b.is_collection)).collect();
/// // The literal map under `details` is not a binding in v1.0.
/// assert_eq!(paths, [("/items", true), ("/user/name", false)]);
/// ```
pub fn collect_bindings_in(version: A2uiVersion, value: &Value) -> Vec<Binding> {
    let mut out = Vec::new();
    let mut seen = BTreeMap::new();
    walk_bindings(version, value, String::new(), 0, &mut out, &mut seen);
    out
}

/// One data-model path reference found inside a component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// Where in the component the reference was found, e.g. `text` or
    /// `action.event.context.email`.
    pub location: String,
    /// The path exactly as written on the wire.
    pub path: String,
    /// Whether the path is a template `path`, iterated rather than read.
    pub is_collection: bool,
}

fn walk_bindings(
    version: A2uiVersion,
    value: &Value,
    location: String,
    depth: usize,
    out: &mut Vec<Binding>,
    seen: &mut BTreeMap<(String, String), ()>,
) {
    // Stop rather than fail: a binding buried past this depth is unreachable in
    // practice, and the validator reports the nesting itself.
    if depth > MAX_VALUE_DEPTH {
        return;
    }
    match value {
        Value::Object(map) => {
            // A template is written with a bare `path` in every version; a
            // binding with the version's own key, which may be the same one.
            let is_collection = map.contains_key("componentId");
            let path_key = if is_collection {
                "path"
            } else {
                version.path_key()
            };
            if let Some(Value::String(path)) = map.get(path_key) {
                let key = (location.clone(), path.clone());
                if seen.insert(key, ()).is_none() {
                    out.push(Binding {
                        location: location.clone(),
                        path: path.clone(),
                        is_collection,
                    });
                }
                if is_collection {
                    return;
                }
            }
            for (key, child) in map {
                if key == path_key {
                    continue;
                }
                let next = if location.is_empty() {
                    key.clone()
                } else {
                    format!("{location}.{key}")
                };
                walk_bindings(version, child, next, depth + 1, out, seen);
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                walk_bindings(
                    version,
                    item,
                    format!("{location}[{index}]"),
                    depth + 1,
                    out,
                    seen,
                );
            }
        }
        _ => {}
    }
}

/// Binding scope over the lossless model, preserving null versus missing/undefined.
#[derive(Debug, Clone)]
pub struct ModelScope<'a> {
    model: &'a crate::DataModel,
    prefix: String,
}
impl<'a> ModelScope<'a> {
    /// Starts at the model root.
    pub fn root(model: &'a crate::DataModel) -> Self {
        Self {
            model,
            prefix: String::new(),
        }
    }
    /// Resolves absolute bindings from the root and relative bindings within this scope.
    pub fn resolve(&self, path: &str) -> Result<Option<&'a crate::ModelValue>> {
        self.model.lookup(&self.absolute(path))
    }
    /// Opens an item scope in a collection, without inventing a missing item.
    pub fn item(&self, path: &str, index: usize) -> Self {
        let collection = self.absolute(path);
        Self {
            model: self.model,
            prefix: format!("{collection}/{index}"),
        }
    }
    /// The absolute pointer to this scope's item, or an empty string for the root.
    pub fn base(&self) -> &str {
        &self.prefix
    }
    pub(crate) fn absolute(&self, path: &str) -> String {
        scoped_pointer(&self.prefix, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn model() -> Value {
        json!({
            "company": "Acme Corp",
            "count": 3,
            "flag": true,
            "nested": {"a": [1, 2]},
            "employees": [
                {"name": "Alice", "role": "Engineer"},
                {"name": "Bob", "role": "Designer"}
            ]
        })
    }

    #[test]
    fn a_v1_0_scope_reads_at_keys_and_leaves_bare_ones_as_data() {
        let data = model();
        let v0_9 = Scope::root(&data);
        let v1_0 = Scope::root(&data).speaking(A2uiVersion::V1_0);
        assert_eq!(v1_0.version(), A2uiVersion::V1_0);
        assert_eq!(v1_0.item("/employees", 0).version(), A2uiVersion::V1_0);

        // The same binding, each in its own spelling.
        assert_eq!(
            v0_9.resolve_dynamic(&json!({"path": "/company"})).unwrap(),
            json!("Acme Corp")
        );
        assert_eq!(
            v1_0.resolve_dynamic(&json!({"@path": "/company"})).unwrap(),
            json!("Acme Corp")
        );
        // And each reads the other's as a map.
        assert_eq!(
            v1_0.resolve_dynamic(&json!({"path": "/company", "call": "audit"}))
                .unwrap(),
            json!({"path": "/company", "call": "audit"})
        );
        assert_eq!(
            v0_9.resolve_dynamic(&json!({"@path": "/company"})).unwrap(),
            json!({"@path": "/company"})
        );

        // A call, with its arguments resolved in the same spelling.
        let item = v1_0.item("/employees", 1);
        assert_eq!(
            item.resolve_dynamic(&json!({"@call": "@index", "args": {"offset": 1}}))
                .unwrap(),
            json!(2)
        );
        assert_eq!(
            item.resolve_dynamic(&json!({
                "@call": "formatString",
                "args": {"value": "${name} @ ${/company}"}
            }))
            .unwrap(),
            json!("Bob @ Acme Corp")
        );

        // `@@` is the literal `@`; one `@` that is no directive is reserved.
        assert_eq!(
            v1_0.resolve_dynamic(&json!({"@@path": "/static", "@@": 1, "nested": {"@@type": "x"}}))
                .unwrap(),
            json!({"@path": "/static", "@": 1, "nested": {"@type": "x"}})
        );
        for reserved in [
            json!({"@if": true}),
            json!({"@": 1}),
            json!({"a": {"@metadata": {}}}),
        ] {
            assert!(v1_0.resolve_dynamic(&reserved).is_err(), "{reserved}");
            assert!(v0_9.resolve_dynamic(&reserved).is_ok(), "{reserved}");
        }
    }

    #[test]
    fn bindings_are_collected_in_the_spelling_of_the_version() {
        let component = json!({
            "text": {"@path": "/user/name"},
            "label": {"path": "/user/label"},
            "children": {"componentId": "tpl", "path": "/items"},
            "action": {"event": {"context": {"email": {"@path": "/form/email"}}}}
        });
        let paths = |version: A2uiVersion| -> Vec<(String, bool)> {
            collect_bindings_in(version, &component)
                .into_iter()
                .map(|binding| (binding.path, binding.is_collection))
                .collect()
        };
        assert_eq!(
            paths(A2uiVersion::V1_0),
            [
                ("/form/email".to_string(), false),
                ("/items".to_string(), true),
                ("/user/name".to_string(), false)
            ]
        );
        assert_eq!(
            paths(A2uiVersion::V0_9),
            [
                ("/items".to_string(), true),
                ("/user/label".to_string(), false)
            ]
        );
        assert_eq!(
            collect_bindings(&component),
            collect_bindings_in(A2uiVersion::V0_9_1, &component)
        );
    }

    #[test]
    fn absolute_paths_resolve_from_the_root_in_any_scope() {
        let data = model();
        let root = Scope::root(&data);
        let scope = root.item("/employees", 1);
        assert_eq!(scope.resolve_string("/company"), "Acme Corp");
        assert_eq!(root.resolve_string("/company"), "Acme Corp");
    }

    #[test]
    fn relative_paths_resolve_inside_the_collection_scope() {
        let data = model();
        let root = Scope::root(&data);
        for (index, expected) in [(0, "Alice"), (1, "Bob")] {
            let item = root.item("/employees", index);
            assert_eq!(item.resolve_string("name"), expected);
            assert_eq!(item.base(), format!("/employees/{index}"));
        }
    }

    #[test]
    fn nested_templates_compose_scopes() {
        let data = json!({"groups": [{"items": [{"label": "inner"}]}]});
        let root = Scope::root(&data);
        let group = root.item("/groups", 0);
        let item = group.item("items", 0);
        assert_eq!(item.base(), "/groups/0/items/0");
        assert_eq!(item.resolve_string("label"), "inner");
    }

    #[test]
    fn the_empty_path_and_dot_name_the_scope_itself() {
        let data = json!({"tags": ["a", "b"], "matrix": [[1, 2], [3]]});
        let root = Scope::root(&data);

        // In the root scope that is the whole model.
        for path in ["", ".", "/"] {
            assert_eq!(root.resolve(path), Some(&data), "{path:?}");
            assert_eq!(root.resolve_pointer(path), "", "{path:?}");
        }

        // In an item scope it is the item, which is how a template over a
        // list of strings shows each one. `/` still leaves the scope.
        let item = root.item("/tags", 1);
        for path in ["", "."] {
            assert_eq!(item.resolve(path), Some(&json!("b")), "{path:?}");
            assert_eq!(item.resolve_pointer(path), "/tags/1", "{path:?}");
        }
        assert_eq!(item.resolve("/"), Some(&data));

        // A template over the item itself iterates the item, not the model.
        let row = root.item("/matrix", 0);
        assert_eq!(row.item("", 1).base(), "/matrix/0/1");
        assert_eq!(row.item(".", 1).resolve(""), Some(&json!(2)));

        // The lossless scope resolves the same way.
        let model = crate::DataModel::from(data.clone());
        let item = ModelScope::root(&model).item("/tags", 0);
        for path in ["", "."] {
            let value = item.resolve(path).unwrap().unwrap();
            assert_eq!(value.to_json().unwrap(), json!("a"), "{path:?}");
        }
        let cell = ModelScope::root(&model).item("/matrix", 1).item("", 0);
        assert_eq!(cell.base(), "/matrix/1/0");
    }

    #[test]
    fn an_absolute_path_is_the_scope_joined_with_a_relative_path() {
        for (scope, path, expected) in [
            ("/user", "name", "/user/name"),
            ("/user", "address/city", "/user/address/city"),
            ("/user", "/list/0", "/list/0"),
            ("/", "user/name", "/user/name"),
            ("", "foo", "/foo"),
            ("/user", "", "/user"),
            ("/user", ".", "/user"),
            ("/", "", "/"),
            ("", ".", "/"),
            ("/user", "/", "/"),
            // Only the two exact spellings name the scope.
            ("/user", "./name", "/user/./name"),
            // A trailing slash is the empty key, and is kept.
            ("/bar/", "foo", "/bar//foo"),
            ("/user/", "", "/user/"),
        ] {
            assert_eq!(absolute_path(scope, path), expected, "{scope:?} + {path:?}");
        }
    }

    #[test]
    fn stringification_follows_the_spec_table() {
        let data = model();
        let scope = Scope::root(&data);
        assert_eq!(scope.resolve_string("/count"), "3");
        assert_eq!(scope.resolve_string("/flag"), "true");
        assert_eq!(scope.resolve_string("/missing"), "");
        assert_eq!(scope.resolve_string("/nested"), r#"{"a":[1,2]}"#);
        assert_eq!(scope.resolve_string("/nested/a"), "[1,2]");
        assert_eq!(stringify(Some(&Value::Null)), "");
    }

    #[test]
    fn format_string_mixes_text_paths_and_escapes() {
        let data = model();
        let scope = Scope::root(&data);
        assert_eq!(
            scope
                .format_string("Hello, ${/company}! You have ${/count} messages.")
                .unwrap(),
            "Hello, Acme Corp! You have 3 messages."
        );
        assert_eq!(
            scope.format_string(r"literal \${/company}").unwrap(),
            "literal ${/company}"
        );
        assert_eq!(
            scope.format_string("no expressions").unwrap(),
            "no expressions"
        );
        assert_eq!(scope.format_string("${/missing}").unwrap(), "");
    }

    #[test]
    fn format_string_supports_index_with_offset_and_nesting() {
        let data = model();
        let root = Scope::root(&data);
        let scope = root.item("/employees", 1);
        assert_eq!(scope.format_string("#${@index()}").unwrap(), "#1");
        assert_eq!(scope.format_string("#${@index(offset: 1)}").unwrap(), "#2");
        assert_eq!(scope.format_string("${${name}}").unwrap(), "Bob");
    }

    #[test]
    fn an_index_offset_that_would_overflow_is_reported_rather_than_wrapping() {
        let data = model();
        let root = Scope::root(&data);
        let scope = root.item("/employees", 1);
        // The template is model output, so the offset is remote input: an
        // extreme one must not take the process down (debug) or silently wrap
        // to a nonsense index (release).
        let error = scope
            .format_string("${@index(offset: 9223372036854775807)}")
            .unwrap_err();
        assert!(matches!(error, Error::Binding { .. }), "{error}");

        // Everything that does fit still evaluates, in both directions.
        assert_eq!(scope.format_string("${@index(offset: -1)}").unwrap(), "0");
        assert_eq!(scope.format_string("${@index(offset: 1)}").unwrap(), "2");
        assert_eq!(
            scope
                .format_string("${@index(offset: -9223372036854775808)}")
                .unwrap(),
            "-9223372036854775807"
        );
    }

    #[test]
    fn format_string_evaluates_what_parse_template_parses() {
        let data = model();
        let scope = Scope::root(&data);
        // String escapes, number forms and `null` are the parser's.
        assert_eq!(
            scope
                .format_string(r"${'a\tb'}|${+1}|${.5}|${null}|${ true }")
                .unwrap(),
            "a\tb|1|0.5||true"
        );
        // What does not parse is an error, where it used to be read as a path
        // that resolved to nothing.
        for malformed in ["${/count extra}", "${count:}", "${/count~}"] {
            let error = scope.format_string(malformed).unwrap_err();
            assert!(
                matches!(error, Error::Binding { .. }),
                "{malformed}: {error}"
            );
        }
        // A key that holds `/` or `~` is reached by its pointer escape, in a
        // template as in any other binding.
        let escaped = json!({"a/b": "slash", "items": [{"c~d": "tilde"}]});
        let root = Scope::root(&escaped);
        assert_eq!(root.format_string("${/a~1b}").unwrap(), "slash");
        assert_eq!(
            root.item("/items", 0).format_string("${c~0d}").unwrap(),
            "tilde"
        );
        // Nesting is the parser's bound, one hundred levels, not ten.
        let nested = format!("{}/count{}", "${".repeat(50), "}".repeat(50));
        assert_eq!(scope.format_string(&nested).unwrap(), "3");
        let too_deep = format!(
            "{}/count{}",
            "${".repeat(MAX_TEMPLATE_DEPTH + 1),
            "}".repeat(MAX_TEMPLATE_DEPTH + 1)
        );
        assert!(scope.format_string(&too_deep).is_err());

        // Evaluating the parts by hand gives what evaluating the template does.
        let template = "${/company}: ${/count} of ${/nested/a}";
        let by_hand: String = parse_template(template)
            .unwrap()
            .iter()
            .map(|part| stringify(Some(&scope.resolve_dynamic(&part.to_json()).unwrap())))
            .collect();
        assert_eq!(by_hand, scope.format_string(template).unwrap());
        assert_eq!(by_hand, "Acme Corp: 3 of [1,2]");
    }

    #[test]
    fn index_outside_a_collection_scope_is_an_error() {
        let data = model();
        let scope = Scope::root(&data);
        let err = scope.format_string("${@index()}").unwrap_err();
        assert!(matches!(err, Error::Binding { .. }));
    }

    #[test]
    fn unterminated_and_unknown_expressions_are_errors() {
        let data = model();
        let scope = Scope::root(&data);
        assert!(scope.format_string("${/company").is_err());
        assert!(
            scope
                .format_string("${formatDate(value: '2026-01-01')}")
                .is_err()
        );
    }

    #[test]
    fn caller_supplied_functions_are_used() {
        let data = model();
        let scope = Scope::root(&data);
        let upper = |name: &str, args: &Map<String, Value>| -> Option<Value> {
            (name == "upper").then(|| Value::String(stringify(args.get("value")).to_uppercase()))
        };
        assert_eq!(
            scope
                .format_string_with("${upper(value: ${/company})}", &upper)
                .unwrap(),
            "ACME CORP"
        );
    }

    #[test]
    fn resolve_dynamic_walks_bindings_and_calls() {
        let data = model();
        let scope = Scope::root(&data);
        let resolved = scope
            .resolve_dynamic(&json!({
                "literal": "static",
                "bound": {"path": "/company"},
                "formatted": {"call": "formatString", "args": {"value": "n=${/count}"}}
            }))
            .unwrap();
        assert_eq!(
            resolved,
            json!({"literal": "static", "bound": "Acme Corp", "formatted": "n=3"})
        );
    }

    #[test]
    fn collect_bindings_separates_templates_from_value_bindings() {
        let component = json!({
            "text": {"path": "/user/name"},
            "children": {"componentId": "tpl", "path": "/items"},
            "action": {"event": {"context": {"email": {"path": "/form/email"}}}}
        });
        let bindings = collect_bindings(&component);
        let found: Vec<_> = bindings
            .iter()
            .map(|b| (b.location.as_str(), b.path.as_str(), b.is_collection))
            .collect();
        assert!(found.contains(&("text", "/user/name", false)));
        assert!(found.contains(&("children", "/items", true)));
        assert!(found.contains(&("action.event.context.email", "/form/email", false)));
    }

    #[test]
    fn a_value_nested_past_the_cap_errors_instead_of_recursing_away() {
        let data = json!({"name": "Ada"});
        let scope = Scope::root(&data);

        // Deeper than MAX_VALUE_DEPTH, but shallow enough that `Value`'s own
        // recursive Clone and Drop still cope — past that point the type itself
        // is the limit, not this crate.
        let mut value = json!({"path": "/name"});
        for _ in 0..(MAX_VALUE_DEPTH + 50) {
            value = json!({"nested": value});
        }
        let error = scope.resolve_dynamic(&value).unwrap_err();
        assert!(matches!(error, Error::Binding { .. }), "{error}");

        // Just inside the cap still resolves.
        let mut value = json!({"path": "/name"});
        for _ in 0..10 {
            value = json!({"nested": value});
        }
        assert!(scope.resolve_dynamic(&value).is_ok());
    }

    /// Nests a binding `wrappers` objects deep. The outermost object sits at
    /// depth 0, so `wrappers` is the depth the innermost one is reached at.
    fn nested(wrappers: usize) -> Value {
        let mut value = json!({"path": "/name"});
        for _ in 0..wrappers {
            value = json!({"nested": value});
        }
        value
    }

    #[test]
    fn the_value_depth_cap_fires_one_level_past_it_and_not_before() {
        let data = json!({"name": "Ada"});
        let scope = Scope::root(&data);

        // Off-by-one here is the difference between rejecting input the spec
        // allows and letting a deeper one through, so the boundary is pinned
        // rather than probed from far away.
        assert!(scope.resolve_dynamic(&nested(MAX_VALUE_DEPTH)).is_ok());
        assert!(scope.resolve_dynamic(&nested(MAX_VALUE_DEPTH + 1)).is_err());

        // `collect_bindings` shares the cap but stops rather than failing, so
        // the binding at the bottom is simply not reported.
        assert_eq!(collect_bindings(&nested(MAX_VALUE_DEPTH)).len(), 1);
        assert!(collect_bindings(&nested(MAX_VALUE_DEPTH + 1)).is_empty());
    }

    #[test]
    fn collecting_bindings_stops_at_the_cap_rather_than_recursing_away() {
        let mut value = json!({"path": "/deep"});
        for _ in 0..(MAX_VALUE_DEPTH + 50) {
            value = json!({"nested": value});
        }
        // No panic, no overflow; the buried binding is simply not reported, and
        // the validator flags the nesting itself.
        assert!(collect_bindings(&value).is_empty());
    }

    #[test]
    fn pointer_escapes_round_trip() {
        assert_eq!(
            Scope::root(&json!({"a/b":{"c~d":1}})).resolve("/a~1b/c~0d"),
            Some(&json!(1))
        );
        let data = json!({"a/b": 7});
        assert_eq!(Scope::root(&data).resolve_string("/a~1b"), "7");
    }
}
