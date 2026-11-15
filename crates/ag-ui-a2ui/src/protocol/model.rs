//! Lossless local A2UI data model. Undefined slots are never sent as JSON values.

use crate::protocol::binding::MAX_VALUE_DEPTH;
use crate::protocol::message::DataModelUpdate;
use crate::protocol::version::A2uiVersion;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Maximum implicit new array slots allocated by one default update.
/// Use `DataModel::apply_with_array_growth_limit` to choose another bound.
///
/// It counts every slot an update adds, in all the lists its path creates or
/// extends, where [`MAX_LIST_INDEX`] bounds each index on its own. One list
/// cannot reach it. It is the bound that applies when one path grows several
/// lists at once (`/a/9999/9999/…`), and when a caller lowers it.
pub const DEFAULT_ARRAY_GROWTH_LIMIT: usize = 65_536;

/// The most undefined list slots one [`DataModel`] holds, in all its lists
/// together.
///
/// A slot is undefined when no message wrote it: a list grows to the index a
/// path names with undefined slots before it, and a removed slot stays in its
/// place, undefined. [`DEFAULT_ARRAY_GROWTH_LIMIT`] bounds what one update
/// adds; this bounds what the updates add between them, so that the size of
/// a model follows the size of what it was sent. An update that would take
/// the model past it is refused, and the model is left as it was. Use
/// [`DataModel::apply_with_undefined_slot_limit`] to choose another bound.
pub const DEFAULT_UNDEFINED_SLOT_LIMIT: usize = 65_536;

/// Largest list index a write may name.
///
/// The reference renderers refuse a write to a higher index, so an agent that
/// accepted one would hold a list the renderer does not. A read is not
/// bounded: a higher index reads as no value, or as the element when a list
/// that was written whole is that long.
pub const MAX_LIST_INDEX: usize = 10_000;

/// Path segments [`DataModel`] refuses, in a read as in a write.
///
/// In JavaScript a path through one of these reaches the prototype of an
/// object and not a key of it, so a JavaScript renderer refuses the path, and
/// the reference renderers refuse it in every language so that a payload
/// behaves the same in all of them. An agent that accepted such a path would
/// hold a state the renderer does not.
///
/// Only the path is checked. A value written at another path may have keys of
/// these names, and is stored as it is.
pub const FORBIDDEN_PATH_SEGMENTS: [&str; 3] = ["__proto__", "constructor", "prototype"];

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
    /// How many list slots in `root` are undefined. Kept up to date as
    /// updates are applied; not part of a snapshot, which is read back by
    /// counting.
    #[serde(skip)]
    undefined: usize,
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
            undefined: undefined_in(&value.root),
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
        // JSON has no undefined.
        Self {
            snapshot_version: 1,
            root: value.into(),
            undefined: 0,
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
    /// explicit null returns `Some(ModelValue::Null)`. A malformed path, one
    /// of more than [`MAX_VALUE_DEPTH`] segments, or one with a segment in
    /// [`FORBIDDEN_PATH_SEGMENTS`] is an [`Error::Pointer`].
    pub fn lookup(&self, path: &str) -> Result<Option<&ModelValue>> {
        lookup(&self.root, path)
    }
    /// Applies an update atomically. Missing/null containers are created and
    /// numeric paths create arrays. Sparse gaps are undefined. A path of more
    /// than [`MAX_VALUE_DEPTH`] segments is an [`Error::Pointer`].
    ///
    /// # What a renderer would refuse
    ///
    /// The model is the agent's copy of what a renderer holds, so it refuses
    /// the two kinds of path the reference renderers refuse, with
    /// [`Error::Pointer`]: any path with a segment in
    /// [`FORBIDDEN_PATH_SEGMENTS`], and a write that names a list index above
    /// [`MAX_LIST_INDEX`]. Accepting either would leave the agent with a state
    /// the renderer does not have. A removal that names such an index is not
    /// a write: nothing is there, and it changes nothing, as below.
    ///
    /// ```
    /// use ag_ui_a2ui::model::MAX_LIST_INDEX;
    /// use ag_ui_a2ui::{DataModel, DataModelUpdate};
    /// use serde_json::json;
    ///
    /// let mut model = DataModel::from(json!({"items": ["a"]}));
    /// let past = format!("/items/{}", MAX_LIST_INDEX + 1);
    /// assert!(model.apply(&past, &DataModelUpdate::Set(json!("x"))).is_err());
    /// model.apply(&past, &DataModelUpdate::Remove)?;
    /// assert_eq!(model, json!({"items": ["a"]}));
    /// # Ok::<(), ag_ui_a2ui::Error>(())
    /// ```
    ///
    /// # How far a list may grow
    ///
    /// Two bounds, and the stricter one applies. No index a write names may
    /// be above [`MAX_LIST_INDEX`]; that is the renderers' bound. And the
    /// update may add no more than [`DEFAULT_ARRAY_GROWTH_LIMIT`] slots in
    /// all; that is this model's own, and with the default it is reached only
    /// by a path that grows several lists at once.
    ///
    /// And what the updates add between them is bounded: an update that would
    /// leave the model holding more than [`DEFAULT_UNDEFINED_SLOT_LIMIT`]
    /// undefined slots is refused ([`DataModel::undefined_slots`]). An update
    /// that does not add to them is never refused for them.
    ///
    /// # Removing what is not there
    ///
    /// Removing a path that holds nothing changes nothing: no container is
    /// created on the way to it, and a list is not padded out to an index
    /// past its end. That is what the protocol's conformance suite asks of a
    /// v0.9 model (`core/data_model.yaml`,
    /// `test_data_model_delete_out_of_range_list_index_is_noop` and
    /// `test_data_model_delete_absent_deep_path_is_noop`) and what the
    /// renderer does (`@a2ui/web_core` 0.15.0, `src/state/data-model.js`:
    /// "Deleting an index past the end of the array is a no-op"). A slot that
    /// is there keeps its place when it is removed, and is undefined.
    ///
    /// [`DataModel::apply_as`] applies an update as a surface's own version
    /// does, which differs for `null` and for the root.
    pub fn apply(&mut self, path: &str, value: &DataModelUpdate) -> Result<()> {
        self.apply_with_array_growth_limit(path, value, DEFAULT_ARRAY_GROWTH_LIMIT)
    }
    /// Applies an update as a surface of `version` takes it.
    ///
    /// What differs is a removal. From v1.0 a `null` written at the path is
    /// a removal ([`A2uiVersion::null_removes`]): that is how the message is
    /// written, so an update that sets `null` is applied as the renderer that
    /// receives it applies it; and removing the root leaves an empty object.
    /// A `null` inside a value that is written is stored in every version.
    /// Before v1.0 this is [`DataModel::apply`].
    ///
    /// In every version, removing a path that holds nothing changes nothing.
    ///
    /// ```
    /// use ag_ui_a2ui::{A2uiVersion, DataModel, DataModelUpdate};
    /// use serde_json::json;
    ///
    /// let mut model = DataModel::from(json!({"items": ["a", "b", "c"]}));
    /// let remove = DataModelUpdate::Remove;
    ///
    /// // Past the end of the list, and under a key that is not there.
    /// model.apply_as(A2uiVersion::V1_0, "/items/10", &remove)?;
    /// model.apply_as(A2uiVersion::V1_0, "/absent/child", &remove)?;
    /// assert_eq!(model, json!({"items": ["a", "b", "c"]}));
    ///
    /// // A slot that is there keeps its place, and the root is emptied.
    /// model.apply_as(A2uiVersion::V1_0, "/items/1", &remove)?;
    /// assert_eq!(model.lookup("/items/1")?, None);
    /// assert_eq!(model.lookup("/items/2")?, Some(&json!("c").into()));
    /// model.apply_as(A2uiVersion::V1_0, "/", &remove)?;
    /// assert_eq!(model, json!({}));
    ///
    /// // Setting `null` removes in v1.0, and is stored before it.
    /// let null = DataModelUpdate::Set(json!(null));
    /// let mut v1_0 = DataModel::from(json!({"name": "Ada", "tags": [null]}));
    /// let mut v0_9 = v1_0.clone();
    /// v1_0.apply_as(A2uiVersion::V1_0, "/name", &null)?;
    /// v0_9.apply_as(A2uiVersion::V0_9, "/name", &null)?;
    /// assert_eq!(v1_0, json!({"tags": [null]}));
    /// assert_eq!(v0_9, json!({"name": null, "tags": [null]}));
    /// # Ok::<(), ag_ui_a2ui::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// As [`DataModel::apply`].
    pub fn apply_as(
        &mut self,
        version: A2uiVersion,
        path: &str,
        value: &DataModelUpdate,
    ) -> Result<()> {
        // What the message says on the wire is what it does here.
        let removal = DataModelUpdate::Remove;
        let value = match value {
            DataModelUpdate::Set(Value::Null) if version.null_removes() => &removal,
            other => other,
        };
        // From v1.0 a removed root is an empty object.
        if version.null_removes()
            && matches!(value, DataModelUpdate::Remove)
            && tokens(path)?.is_empty()
        {
            self.root = ModelValue::Object(BTreeMap::new());
            self.undefined = 0;
            return Ok(());
        }
        self.apply(path, value)
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
        (self.root, self.undefined) =
            self.updated(path, value, max_new_slots, DEFAULT_UNDEFINED_SLOT_LIMIT)?;
        Ok(())
    }
    /// Applies as [`DataModel::apply`] does, with another bound than
    /// [`DEFAULT_UNDEFINED_SLOT_LIMIT`] on the undefined slots the model may
    /// hold once the update is applied.
    ///
    /// ```
    /// use ag_ui_a2ui::{DataModel, DataModelUpdate};
    /// use serde_json::json;
    ///
    /// let mut model = DataModel::default();
    /// let one = DataModelUpdate::Set(json!(1));
    /// // `/a/5` leaves five undefined slots before it, and `/b/5` five more.
    /// model.apply_with_undefined_slot_limit("/a/5", &one, 8)?;
    /// assert_eq!(model.undefined_slots(), 5);
    /// assert!(model.apply_with_undefined_slot_limit("/b/5", &one, 8).is_err());
    /// assert_eq!(model.undefined_slots(), 5, "a refused update changes nothing");
    /// // Filling a slot takes one away, whatever the bound.
    /// model.apply_with_undefined_slot_limit("/a/0", &one, 0)?;
    /// assert_eq!(model.undefined_slots(), 4);
    /// # Ok::<(), ag_ui_a2ui::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// As [`DataModel::apply`], with `max_undefined_slots` in place of the
    /// default bound.
    pub fn apply_with_undefined_slot_limit(
        &mut self,
        path: &str,
        value: &DataModelUpdate,
        max_undefined_slots: usize,
    ) -> Result<()> {
        (self.root, self.undefined) =
            self.updated(path, value, DEFAULT_ARRAY_GROWTH_LIMIT, max_undefined_slots)?;
        Ok(())
    }
    /// How many list slots of the model are undefined: slots before an index
    /// a write grew a list to, and slots that were removed.
    ///
    /// ```
    /// use ag_ui_a2ui::{DataModel, DataModelUpdate};
    /// use serde_json::json;
    ///
    /// let mut model = DataModel::from(json!({"items": ["a", "b"]}));
    /// model.apply("/items/1", &DataModelUpdate::Remove)?;
    /// model.apply("/items/4", &DataModelUpdate::Set(json!("e")))?;
    /// assert_eq!(model.undefined_slots(), 3);
    /// # Ok::<(), ag_ui_a2ui::Error>(())
    /// ```
    pub fn undefined_slots(&self) -> usize {
        self.undefined
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
        let (after, undefined) = self.updated(
            path,
            value,
            DEFAULT_ARRAY_GROWTH_LIMIT,
            DEFAULT_UNDEFINED_SLOT_LIMIT,
        )?;
        let before = std::mem::replace(&mut self.root, after.clone());
        self.undefined = undefined;
        Ok(ModelChange { before, after })
    }
    /// The root this model would have after an update, and how many of its
    /// slots would be undefined. The model is untouched.
    fn updated(
        &self,
        path: &str,
        value: &DataModelUpdate,
        max_new_slots: usize,
        max_undefined_slots: usize,
    ) -> Result<(ModelValue, usize)> {
        let tokens = tokens(path)?;
        // Removing what is not there changes nothing: no container is made on
        // the way to it, and no list is padded out to its index.
        if matches!(value, DataModelUpdate::Remove)
            && !tokens.is_empty()
            && lookup(&self.root, path)?.is_none()
        {
            return Ok((self.root.clone(), self.undefined));
        }
        let mut next = self.root.clone();
        let mut remaining = max_new_slots;
        let mut undefined = self.undefined;
        if !tokens.is_empty() && matches!(next, ModelValue::Undefined | ModelValue::Null) {
            next = ModelValue::Object(BTreeMap::new());
        }
        apply(
            &mut next,
            &tokens,
            value,
            path,
            &mut remaining,
            &mut undefined,
        )?;
        // Only an update that adds undefined slots is held to the bound, so
        // a model past it, read from a snapshot or given a lower bound, can
        // still be filled in.
        if undefined > max_undefined_slots && undefined > self.undefined {
            return Err(Error::pointer(
                path,
                format!(
                    "the data model would hold {undefined} undefined list slots, more than the \
                     {max_undefined_slots} it may hold in all"
                ),
            ));
        }
        Ok((next, undefined))
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
            if FORBIDDEN_PATH_SEGMENTS.contains(&result.as_str()) {
                return Err(Error::pointer(
                    path,
                    format!("forbidden path segment {result:?}: a JavaScript renderer refuses it"),
                ));
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
/// How many list slots within `value` are undefined, not counting `value`
/// itself. A loop and not a stack frame per level.
fn undefined_in(value: &ModelValue) -> usize {
    let mut undefined = 0;
    let mut stack = vec![value];
    while let Some(value) = stack.pop() {
        match value {
            ModelValue::Array(slots) => {
                for slot in slots {
                    if matches!(slot, ModelValue::Undefined) {
                        undefined += 1;
                    } else {
                        stack.push(slot);
                    }
                }
            }
            ModelValue::Object(fields) => stack.extend(fields.values()),
            _ => {}
        }
    }
    undefined
}

/// Walks to the target with a loop, not a stack frame per segment: the path is
/// remote input.
///
/// `undefined` is how many list slots of the model are undefined, and is kept
/// as the walk changes them: slots a list grows by, a slot walked through
/// that becomes a container, what the written value replaces, and the slot a
/// removal leaves.
fn apply(
    root: &mut ModelValue,
    tokens: &[String],
    value: &DataModelUpdate,
    path: &str,
    remaining: &mut usize,
    undefined: &mut usize,
) -> Result<()> {
    let mut node = root;
    // Whether `node` is a slot of a list, which is what can be undefined.
    let mut in_list = false;
    for (position, token) in tokens.iter().enumerate() {
        let last = position + 1 == tokens.len();
        if matches!(node, ModelValue::Undefined | ModelValue::Null) {
            if in_list && matches!(node, ModelValue::Undefined) {
                *undefined = undefined.saturating_sub(1);
            }
            *node = if is_numeric(token) {
                ModelValue::Array(Vec::new())
            } else {
                ModelValue::Object(BTreeMap::new())
            };
        }
        node = match node {
            ModelValue::Object(map) => {
                if last && matches!(value, DataModelUpdate::Remove) {
                    if let Some(removed) = map.remove(token) {
                        *undefined = undefined.saturating_sub(undefined_in(&removed));
                    }
                    return Ok(());
                }
                in_list = false;
                map.entry(token.clone()).or_insert(ModelValue::Undefined)
            }
            ModelValue::Array(array) => {
                let i = index(token, path)?;
                if i > MAX_LIST_INDEX {
                    return Err(Error::pointer(
                        path,
                        format!(
                            "list index {i} is above {MAX_LIST_INDEX}, the largest a renderer \
                             accepts in a write"
                        ),
                    ));
                }
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
                    *undefined += added;
                    array.resize(len, ModelValue::Undefined);
                }
                in_list = true;
                &mut array[i]
            }
            _ => return Err(Error::pointer(path, "path walks through a scalar value")),
        };
    }
    let written = match value {
        DataModelUpdate::Set(v) => v.clone().into(),
        DataModelUpdate::Remove => ModelValue::Undefined,
    };
    let replaced = std::mem::replace(node, written);
    let slot = |value: &ModelValue| usize::from(in_list && matches!(value, ModelValue::Undefined));
    // What the write replaced goes, and a written value is JSON, which holds
    // nothing undefined.
    *undefined = undefined.saturating_sub(undefined_in(&replaced) + slot(&replaced)) + slot(node);
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
    fn removing_what_is_not_there_does_nothing_in_any_version() {
        let start = json!({"items": ["a", "b", "c"], "user": {"name": "Ada"}, "text": "x"});
        // Past the end of a list, and under a key that is not there: no list
        // is padded and no container is made.
        for path in ["/items/5", "/absent/child/path"] {
            for version in A2uiVersion::ALL {
                let mut model = DataModel::from(start.clone());
                model
                    .apply_as(*version, path, &DataModelUpdate::Remove)
                    .unwrap();
                assert_eq!(model, DataModel::from(start.clone()), "{version}: {path}");
            }
            let mut model = DataModel::from(start.clone());
            model.apply(path, &DataModelUpdate::Remove).unwrap();
            assert_eq!(model, DataModel::from(start.clone()), "{path}");
        }

        // Under a scalar there is nothing to remove either, where a write
        // there is an error in every version.
        let mut model = DataModel::from(start.clone());
        model
            .apply_as(A2uiVersion::V1_0, "/text/inner", &DataModelUpdate::Remove)
            .unwrap();
        assert_eq!(model, DataModel::from(start.clone()));
        assert!(
            model
                .apply_as(A2uiVersion::V1_0, "/text/inner", &set(json!(1)))
                .is_err()
        );
        // A path that is not a pointer is refused before anything is looked up.
        assert!(
            model
                .apply_as(A2uiVersion::V1_0, "items/0", &DataModelUpdate::Remove)
                .is_err()
        );

        // What is there is removed the same way in both: a key goes, a slot
        // keeps its place.
        for version in [A2uiVersion::V0_9_1, A2uiVersion::V1_0] {
            let mut model = DataModel::from(start.clone());
            model
                .apply_as(version, "/user/name", &DataModelUpdate::Remove)
                .unwrap();
            model
                .apply_as(version, "/items/1", &DataModelUpdate::Remove)
                .unwrap();
            assert_eq!(model.lookup("/user").unwrap(), Some(&json!({}).into()));
            assert_eq!(model.lookup("/items/1").unwrap(), None);
            assert_eq!(
                model.lookup("/items/2").unwrap(),
                Some(&json!("c").into()),
                "{version}"
            );
        }

        // The root: undefined before v1.0, an empty object from it.
        let mut v0_9 = DataModel::from(start.clone());
        v0_9.apply_as(A2uiVersion::V0_9, "/", &DataModelUpdate::Remove)
            .unwrap();
        assert_eq!(v0_9.root(), &ModelValue::Undefined);
        let mut v1_0 = DataModel::from(start);
        v1_0.apply_as(A2uiVersion::V1_0, "/", &DataModelUpdate::Remove)
            .unwrap();
        assert_eq!(v1_0, json!({}));
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

    fn reason(error: Error) -> String {
        match error {
            Error::Pointer { reason, .. } => reason,
            other => panic!("expected a pointer error, got {other}"),
        }
    }

    #[test]
    fn a_path_a_javascript_renderer_refuses_is_refused_here() {
        let mut model = DataModel::from(json!({"user": {"name": "Ada"}, "items": [{}]}));
        let before = model.clone();
        for path in [
            "/__proto__/polluted",
            "/constructor/prototype/polluted",
            "/user/prototype",
            "/user/__proto__",
            "/items/0/constructor",
        ] {
            for update in [set(json!("x")), DataModelUpdate::Remove] {
                let refused = reason(model.apply(path, &update).unwrap_err());
                assert!(
                    refused.starts_with("forbidden path segment"),
                    "{path}: {refused}"
                );
                assert_eq!(model, before, "{path}");
            }
            // A renderer refuses the read as well.
            let refused = reason(model.lookup(path).unwrap_err());
            assert!(
                refused.starts_with("forbidden path segment"),
                "{path}: {refused}"
            );
        }
        assert!(model.apply_reporting("/__proto__", &set(json!(1))).is_err());

        // A key that only contains one of the names is a key like any other.
        model
            .apply("/prototypes/constructor_name", &set(json!("fine")))
            .unwrap();
        // Only the path is checked: a value may have such a key, and is
        // stored whole. The key cannot then be reached by its own path.
        model
            .apply("/site", &set(json!({"constructor": "Acme"})))
            .unwrap();
        assert_eq!(
            model.lookup("/site").unwrap().unwrap().to_json().unwrap(),
            json!({"constructor": "Acme"})
        );
        assert!(model.lookup("/site/constructor").is_err());
    }

    #[test]
    fn a_write_names_no_list_index_above_the_one_a_renderer_accepts() {
        let mut model = DataModel::from(json!({"items": ["a", "b", "c"]}));
        let last = format!("/items/{MAX_LIST_INDEX}");
        model.apply(&last, &set(json!("last"))).unwrap();
        assert_eq!(
            model.lookup(&last).unwrap(),
            Some(&ModelValue::String("last".into()))
        );

        let before = model.clone();
        for path in [
            "/items/10001",
            "/items/10001/name",
            "/items/999999999",
            "/fresh/10001",
            "/fresh/20000/0",
        ] {
            let refused = reason(model.apply(path, &set(json!("x"))).unwrap_err());
            assert!(refused.starts_with("list index"), "{path}: {refused}");
            assert_eq!(model, before, "{path}");
            // A removal there finds nothing to remove, and is not a write.
            model.apply(path, &DataModelUpdate::Remove).unwrap();
            assert_eq!(model, before, "{path}");
        }
        // A read is not a write: past the end there is nothing to find.
        assert_eq!(model.lookup("/items/10001").unwrap(), None);
    }

    #[test]
    fn the_growth_budget_still_counts_every_slot_an_update_adds() {
        // One list cannot reach the default budget now. A path that grows
        // several lists at once can: six lists of 10,000 slots fit in 65,536,
        // and seven do not.
        let grow = |lists: usize| format!("/deep{}", "/9999".repeat(lists));
        let mut model = DataModel::default();
        model.apply(&grow(6), &set(json!(1))).unwrap();

        let mut model = DataModel::default();
        let refused = reason(model.apply(&grow(7), &set(json!(1))).unwrap_err());
        assert!(refused.contains("slot budget"), "{refused}");
        assert_eq!(model, DataModel::default());

        // A budget a caller lowers is the stricter bound for a short list.
        let refused = reason(
            model
                .apply_with_array_growth_limit("/short/5", &set(json!(1)), 3)
                .unwrap_err(),
        );
        assert!(refused.contains("slot budget"), "{refused}");
        // And raising it does not lift the index bound.
        let refused = reason(
            model
                .apply_with_array_growth_limit("/long/10001", &set(json!(1)), usize::MAX)
                .unwrap_err(),
        );
        assert!(refused.starts_with("list index"), "{refused}");
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

    /// The count of undefined slots is kept as each update is applied, not
    /// counted again. Whatever the updates, it is what counting would give.
    #[test]
    fn the_count_of_undefined_slots_is_what_counting_gives() {
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut roll = |sides: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % sides as u64) as usize
        };
        let segments = ["a", "b", "0", "1", "3", "7"];
        let values = [
            json!(1),
            json!(null),
            json!([1, 2]),
            json!({"x": [0, 1, 2]}),
            json!([]),
        ];
        for version in [A2uiVersion::V0_9, A2uiVersion::V1_0] {
            let mut model = DataModel::default();
            for _ in 0..2_000 {
                let depth = 1 + roll(4);
                let path: String = (0..depth)
                    .map(|_| format!("/{}", segments[roll(segments.len())]))
                    .collect();
                let path = if roll(20) == 0 { "/".to_string() } else { path };
                let update = if roll(3) == 0 {
                    DataModelUpdate::Remove
                } else {
                    set(values[roll(values.len())].clone())
                };
                let before = model.clone();
                match model.apply_as(version, &path, &update) {
                    Ok(()) => {}
                    Err(_) => assert_eq!(model, before, "{path}"),
                }
                assert_eq!(
                    model.undefined_slots(),
                    undefined_in(model.root()),
                    "{version} after {path} {update:?}"
                );
            }
        }

        // A snapshot read back is counted.
        let mut model = DataModel::default();
        model.apply("/list/4", &set(json!(1))).unwrap();
        let read: DataModel =
            serde_json::from_value(serde_json::to_value(&model).unwrap()).unwrap();
        assert_eq!(read.undefined_slots(), 4);
        assert_eq!(read, model);
    }

    /// Each update may grow its lists up to the per-update budget, and the
    /// updates together up to the model's bound.
    #[test]
    fn the_undefined_slots_of_a_model_are_bounded_in_all_and_not_per_update() {
        // Six lists grown to the last index a write may name: 60,000
        // undefined slots, under both bounds.
        let six = |key: &str| format!("/{key}{}", format!("/{MAX_LIST_INDEX}").repeat(6));
        let mut model = DataModel::default();
        model.apply(&six("first"), &set(json!(1))).unwrap();
        assert_eq!(model.undefined_slots(), 6 * MAX_LIST_INDEX);

        // A second such update is under the per-update budget too, and is
        // refused for what the model would then hold.
        let before = model.clone();
        let refused = reason(model.apply(&six("second"), &set(json!(1))).unwrap_err());
        assert!(refused.contains("undefined list slots"), "{refused}");
        assert_eq!(model, before);
        assert!(
            model
                .apply_reporting(&six("second"), &set(json!(1)))
                .is_err()
        );
        for version in [A2uiVersion::V0_9, A2uiVersion::V1_0] {
            assert!(
                model
                    .apply_as(version, &six("second"), &set(json!(1)))
                    .is_err()
            );
        }
        assert_eq!(model, before);

        // Replacing what holds them gives them back.
        model.apply("/first", &set(json!("gone"))).unwrap();
        assert_eq!(model.undefined_slots(), 0);
        model.apply(&six("second"), &set(json!(1))).unwrap();

        // An update that adds none is not refused, past the bound or not.
        model
            .apply_with_undefined_slot_limit("/second/0", &set(json!(2)), 10)
            .unwrap();
        assert_eq!(model.undefined_slots(), 6 * MAX_LIST_INDEX - 1);
        // One that adds some is, past a lower bound.
        let refused = reason(
            model
                .apply_with_undefined_slot_limit("/third/3", &set(json!(1)), 10)
                .unwrap_err(),
        );
        assert!(refused.contains("more than the 10"), "{refused}");
    }
}
