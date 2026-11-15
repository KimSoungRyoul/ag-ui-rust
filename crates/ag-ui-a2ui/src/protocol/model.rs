//! Lossless local A2UI data model. Undefined slots are never sent as JSON values.

use crate::protocol::binding::MAX_VALUE_DEPTH;
use crate::protocol::message::DataModelUpdate;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Maximum implicit new array slots allocated by one default update.
/// Use `DataModel::apply_with_array_growth_limit` to choose another bound.
pub const DEFAULT_ARRAY_GROWTH_LIMIT: usize = 65_536;

/// A local model node, including the value JSON cannot express.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum ModelValue {
    /// An absent root or a deleted array slot.
    Undefined,
    /// Explicit JSON null.
    Null,
    /// JSON boolean.
    Bool(bool),
    /// JSON number.
    Number(serde_json::Number),
    /// JSON string.
    String(String),
    /// Ordered slots; deleting one preserves the length.
    Array(Vec<ModelValue>),
    /// Object fields; deleting a field removes it.
    Object(BTreeMap<String, ModelValue>),
}

impl From<Value> for ModelValue {
    fn from(value: Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(v) => Self::Bool(v),
            Value::Number(v) => Self::Number(v),
            Value::String(v) => Self::String(v),
            Value::Array(v) => Self::Array(v.into_iter().map(Self::from).collect()),
            Value::Object(v) => {
                Self::Object(v.into_iter().map(|(k, v)| (k, Self::from(v))).collect())
            }
        }
    }
}

impl ModelValue {
    /// Exports exact JSON, failing rather than replacing undefined with null.
    pub fn to_json(&self) -> Result<Value> {
        self.export("")
    }
    fn export(&self, path: &str) -> Result<Value> {
        Ok(match self {
            Self::Undefined => return Err(Error::Undefined(path.into())),
            Self::Null => Value::Null,
            Self::Bool(v) => Value::Bool(*v),
            Self::Number(v) => Value::Number(v.clone()),
            Self::String(v) => Value::String(v.clone()),
            Self::Array(v) => Value::Array(
                v.iter()
                    .enumerate()
                    .map(|(i, v)| v.export(&format!("{path}/{i}")))
                    .collect::<Result<_>>()?,
            ),
            Self::Object(v) => Value::Object(
                v.iter()
                    .map(|(k, v)| {
                        Ok((
                            k.clone(),
                            v.export(&format!(
                                "{path}/{}",
                                k.replace('~', "~0").replace('/', "~1")
                            ))?,
                        ))
                    })
                    .collect::<Result<_>>()?,
            ),
        })
    }
}

/// A versioned, serializable local snapshot; never an A2UI wire payload.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DataModel {
    snapshot_version: u32,
    root: ModelValue,
}
impl<'de> Deserialize<'de> for DataModel {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Snapshot {
            snapshot_version: u32,
            root: ModelValue,
        }
        let value = Snapshot::deserialize(deserializer)?;
        if value.snapshot_version != 1 {
            return Err(serde::de::Error::custom(
                "unsupported A2UI data model snapshot version",
            ));
        }
        Ok(Self {
            snapshot_version: 1,
            root: value.root,
        })
    }
}
impl Default for DataModel {
    fn default() -> Self {
        Self::from(Value::Object(Default::default()))
    }
}
impl From<Value> for DataModel {
    fn from(value: Value) -> Self {
        Self {
            snapshot_version: 1,
            root: value.into(),
        }
    }
}
impl PartialEq<Value> for DataModel {
    fn eq(&self, other: &Value) -> bool {
        self.to_json().ok().as_ref() == Some(other)
    }
}
impl DataModel {
    /// The exact local root, including an undefined root after deletion.
    pub fn root(&self) -> &ModelValue {
        &self.root
    }
    /// Exact JSON export, rejecting undefined roots and slots.
    pub fn to_json(&self) -> Result<Value> {
        self.root.to_json()
    }
    /// Looks up an absolute JSON Pointer. Missing and undefined return `None`;
    /// explicit null returns `Some(ModelValue::Null)`. A malformed path, or one
    /// of more than [`MAX_VALUE_DEPTH`] segments, is an [`Error::Pointer`].
    pub fn lookup(&self, path: &str) -> Result<Option<&ModelValue>> {
        lookup(&self.root, path)
    }
    /// Applies an update atomically. Missing/null containers are created and
    /// numeric paths create arrays. Sparse gaps are undefined. Implicit array
    /// growth is bounded by [`DEFAULT_ARRAY_GROWTH_LIMIT`] per update, and a
    /// path of more than [`MAX_VALUE_DEPTH`] segments is an [`Error::Pointer`].
    pub fn apply(&mut self, path: &str, value: &DataModelUpdate) -> Result<()> {
        self.apply_with_array_growth_limit(path, value, DEFAULT_ARRAY_GROWTH_LIMIT)
    }
    /// Applies with an explicit allocation budget for newly created array slots.
    /// The budget covers all nested expansions in this update. An invalid path
    /// or exhausted budget leaves the original model unchanged.
    pub fn apply_with_array_growth_limit(
        &mut self,
        path: &str,
        value: &DataModelUpdate,
        max_new_slots: usize,
    ) -> Result<()> {
        self.root = self.updated(path, value, max_new_slots)?;
        Ok(())
    }
    /// Applies an update as [`DataModel::apply`] does, and reports what it
    /// changed.
    ///
    /// The report answers, for any path, whether the value there is different
    /// after the write: what a caller that keeps something derived from a few
    /// paths, such as the components bound to them, needs to know to leave the
    /// rest alone. A rejected update changes nothing and reports nothing.
    ///
    /// ```
    /// use ag_ui_a2ui::{DataModel, DataModelUpdate};
    /// use serde_json::json;
    ///
    /// let mut model = DataModel::from(json!({"user": {"name": "Ada", "age": 36}, "tags": []}));
    /// let change = model.apply_reporting("/user/name", &DataModelUpdate::Set(json!("Grace")))?;
    /// assert!(change.changed("/user/name")?);
    /// // A container changes when something in it does, up to the root.
    /// assert!(change.changed("/user")?);
    /// assert!(change.changed("/")?);
    /// // Nothing else does.
    /// assert!(!change.changed("/user/age")?);
    /// assert!(!change.changed("/tags")?);
    ///
    /// // Writing the value a path already holds changes nothing.
    /// let change = model.apply_reporting("/user/name", &DataModelUpdate::Set(json!("Grace")))?;
    /// assert!(change.is_unchanged());
    /// # Ok::<(), ag_ui_a2ui::Error>(())
    /// ```
    pub fn apply_reporting(&mut self, path: &str, value: &DataModelUpdate) -> Result<ModelChange> {
        let after = self.updated(path, value, DEFAULT_ARRAY_GROWTH_LIMIT)?;
        let before = std::mem::replace(&mut self.root, after.clone());
        Ok(ModelChange { before, after })
    }
    /// The root this model would have after an update. The model is untouched.
    fn updated(
        &self,
        path: &str,
        value: &DataModelUpdate,
        max_new_slots: usize,
    ) -> Result<ModelValue> {
        let tokens = tokens(path)?;
        let mut next = self.root.clone();
        let mut remaining = max_new_slots;
        if !tokens.is_empty() && matches!(next, ModelValue::Undefined | ModelValue::Null) {
            next = ModelValue::Object(BTreeMap::new());
        }
        apply(&mut next, &tokens, value, path, &mut remaining)?;
        Ok(next)
    }
}

/// What one write did to a [`DataModel`]: the model before it and after it.
///
/// [`DataModel::apply_reporting`] returns one.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelChange {
    before: ModelValue,
    after: ModelValue,
}

impl ModelChange {
    /// Whether the value at `path` is different after the write.
    ///
    /// That is so at the written path when the write stored a different value,
    /// at every path above it, and at the paths below it whose own value is
    /// different. A path the write does not reach is never changed, and neither
    /// is one that had no value and still has none. `path` is read as
    /// [`DataModel::lookup`] reads it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Pointer`] for a path [`DataModel::lookup`] rejects.
    pub fn changed(&self, path: &str) -> Result<bool> {
        Ok(lookup(&self.before, path)? != lookup(&self.after, path)?)
    }

    /// Whether the write left the whole model as it was.
    pub fn is_unchanged(&self) -> bool {
        self.before == self.after
    }
}

fn lookup<'a>(root: &'a ModelValue, path: &str) -> Result<Option<&'a ModelValue>> {
    let tokens = tokens(path)?;
    let mut node = root;
    for token in tokens {
        node = match node {
            ModelValue::Object(map) => match map.get(&token) {
                Some(v) => v,
                None => return Ok(None),
            },
            ModelValue::Array(array) => match array.get(index(&token, path)?) {
                Some(v) => v,
                None => return Ok(None),
            },
            _ => return Ok(None),
        };
    }
    Ok((!matches!(node, ModelValue::Undefined)).then_some(node))
}

fn tokens(path: &str) -> Result<Vec<String>> {
    if path.is_empty() || path == "/" {
        return Ok(vec![]);
    }
    if !path.starts_with('/') {
        return Err(Error::pointer(path, "expected an absolute JSON Pointer"));
    }
    // Every segment is a level of nesting in the model, which its derived
    // `Clone`, `Drop` and export recurse over. Counted before decoding: an
    // escaped slash is `~1`, so each `/` starts exactly one segment.
    if path.bytes().filter(|&byte| byte == b'/').count() > MAX_VALUE_DEPTH {
        return Err(Error::pointer(
            path,
            format!("path has more than {MAX_VALUE_DEPTH} segments"),
        ));
    }
    path[1..]
        .split('/')
        .map(|token| {
            let mut result = String::new();
            let mut chars = token.chars();
            while let Some(ch) = chars.next() {
                if ch == '~' {
                    match chars.next() {
                        Some('0') => result.push('~'),
                        Some('1') => result.push('/'),
                        _ => return Err(Error::pointer(path, "invalid JSON Pointer escape")),
                    }
                } else {
                    result.push(ch)
                }
            }
            Ok(result)
        })
        .collect()
}
fn is_numeric(token: &str) -> bool {
    !(token.is_empty() || token.len() > 1 && token.starts_with('0'))
        && token.bytes().all(|b| b.is_ascii_digit())
}
fn index(token: &str, path: &str) -> Result<usize> {
    if !is_numeric(token) {
        return Err(Error::pointer(path, "expected a canonical array index"));
    }
    token
        .parse()
        .map_err(|_| Error::pointer(path, "array index is too large"))
}
/// Walks to the target with a loop, not a stack frame per segment: the path is
/// remote input.
fn apply(
    root: &mut ModelValue,
    tokens: &[String],
    value: &DataModelUpdate,
    path: &str,
    remaining: &mut usize,
) -> Result<()> {
    let mut node = root;
    for (position, token) in tokens.iter().enumerate() {
        let last = position + 1 == tokens.len();
        if matches!(node, ModelValue::Undefined | ModelValue::Null) {
            *node = if is_numeric(token) {
                ModelValue::Array(Vec::new())
            } else {
                ModelValue::Object(BTreeMap::new())
            };
        }
        node = match node {
            ModelValue::Object(map) => {
                if last && matches!(value, DataModelUpdate::Remove) {
                    map.remove(token);
                    return Ok(());
                }
                map.entry(token.clone()).or_insert(ModelValue::Undefined)
            }
            ModelValue::Array(array) => {
                let i = index(token, path)?;
                if i >= array.len() {
                    let len = i
                        .checked_add(1)
                        .ok_or_else(|| Error::pointer(path, "array index is too large"))?;
                    let added = len - array.len();
                    if added > *remaining {
                        return Err(Error::pointer(
                            path,
                            "implicit array growth exceeds the configured slot budget",
                        ));
                    }
                    array
                        .try_reserve(added)
                        .map_err(|e| Error::pointer(path, e))?;
                    *remaining -= added;
                    array.resize(len, ModelValue::Undefined);
                }
                &mut array[i]
            }
            _ => return Err(Error::pointer(path, "path walks through a scalar value")),
        };
    }
    *node = match value {
        DataModelUpdate::Set(v) => v.clone().into(),
        DataModelUpdate::Remove => ModelValue::Undefined,
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn set(value: Value) -> DataModelUpdate {
        DataModelUpdate::Set(value)
    }

    #[test]
    fn a_write_changes_its_path_what_is_above_it_and_what_differs_below_it() {
        let mut model = DataModel::from(json!({
            "user": {"name": "Ada", "settings": {"theme": "dark", "size": 2}},
            "foo": 1,
            "foobar": 2,
        }));

        let change = model
            .apply_reporting("/user/settings", &set(json!({"theme": "light", "size": 2})))
            .unwrap();
        for changed in ["/user/settings", "/user", "/", "", "/user/settings/theme"] {
            assert!(change.changed(changed).unwrap(), "{changed}");
        }
        for same in [
            "/user/settings/size",
            "/user/name",
            "/foo",
            "/user/settings/absent",
        ] {
            assert!(!change.changed(same).unwrap(), "{same}");
        }
        assert!(!change.is_unchanged());

        // A key that only shares a prefix is another key.
        let change = model.apply_reporting("/foo", &set(json!(99))).unwrap();
        assert!(change.changed("/foo").unwrap());
        assert!(!change.changed("/foobar").unwrap());
    }

    #[test]
    fn a_write_that_stores_what_is_there_changes_nothing() {
        let mut model = DataModel::from(json!({"k": "v", "list": [1, 2]}));
        for (path, value) in [
            ("/k", json!("v")),
            ("/list", json!([1, 2])),
            ("/list/1", json!(2)),
        ] {
            let change = model.apply_reporting(path, &set(value)).unwrap();
            assert!(change.is_unchanged(), "{path}");
            assert!(!change.changed(path).unwrap(), "{path}");
            assert!(!change.changed("/").unwrap(), "{path}");
        }
        let change = model.apply_reporting("/k", &set(json!("w"))).unwrap();
        assert!(change.changed("/k").unwrap());
    }

    #[test]
    fn a_removal_changes_the_paths_that_lose_their_value() {
        let mut model = DataModel::from(json!({"user": {"settings": {"theme": "dark"}}, "n": 1}));
        let change = model
            .apply_reporting("/user", &DataModelUpdate::Remove)
            .unwrap();
        for gone in ["/user", "/user/settings", "/user/settings/theme", "/"] {
            assert!(change.changed(gone).unwrap(), "{gone}");
        }
        assert!(!change.changed("/n").unwrap());
        assert_eq!(model, json!({"n": 1}));
    }

    #[test]
    fn a_rejected_write_reports_nothing_and_leaves_the_model_alone() {
        let mut model = DataModel::from(json!({"name": "Ada"}));
        let before = model.clone();
        let error = model
            .apply_reporting("/name/first", &set(json!("A")))
            .unwrap_err();
        assert!(matches!(error, Error::Pointer { .. }), "{error}");
        assert_eq!(model, before);

        // The report reads a path the way the model does, errors included.
        let change = model.apply_reporting("/age", &set(json!(36))).unwrap();
        assert!(matches!(change.changed("age"), Err(Error::Pointer { .. })));
    }

    #[test]
    fn reporting_and_plain_apply_leave_the_same_model() {
        let updates = [
            ("/a/b/0/c", set(json!(1))),
            ("/a/b/2", set(json!(null))),
            ("/a/b/0", DataModelUpdate::Remove),
            ("/", set(json!({"fresh": true}))),
        ];
        let mut plain = DataModel::default();
        let mut reporting = DataModel::default();
        for (path, update) in &updates {
            plain.apply(path, update).unwrap();
            reporting.apply_reporting(path, update).unwrap();
            assert_eq!(plain, reporting, "{path}");
        }
    }
}
