//! JSON that keeps the order its text was written in.
//!
//! A [`serde_json::Value`] sorts the keys of an object, unless the whole
//! build turns on `serde_json`'s `preserve_order` feature, which a library
//! cannot ask of everything that links it. Checking a message does not care.
//! A prompt does: it shows a model a catalog and the protocol's schemas, and a
//! schema reads best the way its author wrote it, `title` and `description`
//! before `properties`, a component's `id` before its other fields. The
//! reference SDKs show them that way, and the conformance suite compares the
//! prompt with theirs byte for byte.
//!
//! So what a prompt shows is kept twice: as a [`Value`], which is what
//! everything else reads, and as an [`Ordered`], which is what is written.

use std::fmt;

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Number, Value};

/// A JSON value whose objects keep the order of their keys.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Ordered {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<Ordered>),
    Object(Vec<(String, Ordered)>),
}

impl Ordered {
    /// Reads JSON text, keeping the order of every object's keys. A key that
    /// is written twice keeps its first place and its last value, which is
    /// the value a [`Value`] read from the same text holds.
    pub(crate) fn parse(text: &str) -> serde_json::Result<Self> {
        serde_json::from_str(text)
    }

    /// `value`, in the order it iterates: sorted, unless the build keeps
    /// insertion order.
    pub(crate) fn from_value(value: &Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(flag) => Self::Bool(*flag),
            Value::Number(number) => Self::Number(number.clone()),
            Value::String(text) => Self::String(text.clone()),
            Value::Array(items) => Self::Array(items.iter().map(Self::from_value).collect()),
            Value::Object(fields) => Self::Object(
                fields
                    .iter()
                    .map(|(key, value)| (key.clone(), Self::from_value(value)))
                    .collect(),
            ),
        }
    }

    /// The same JSON as a [`Value`].
    pub(crate) fn to_value(&self) -> Value {
        match self {
            Self::Null => Value::Null,
            Self::Bool(flag) => Value::Bool(*flag),
            Self::Number(number) => Value::Number(number.clone()),
            Self::String(text) => Value::String(text.clone()),
            Self::Array(items) => Value::Array(items.iter().map(Self::to_value).collect()),
            Self::Object(fields) => Value::Object(
                fields
                    .iter()
                    .map(|(key, value)| (key.clone(), value.to_value()))
                    .collect(),
            ),
        }
    }

    /// What is left of this value in `kept`, in this value's order.
    ///
    /// `kept` is this value with entries taken away: keys of objects, and
    /// whole items of lists. That is what pruning a schema does. A key `kept`
    /// has that this value never had is added after the others, and a value
    /// that is not this one with entries removed is taken from `kept` as it
    /// is, so the result is always `kept` as JSON.
    pub(crate) fn keeping(&self, kept: &Value) -> Self {
        match (self, kept) {
            (Self::Object(fields), Value::Object(kept)) => {
                let mut left: Vec<(String, Self)> = fields
                    .iter()
                    .filter_map(|(key, value)| Some((key.clone(), value.keeping(kept.get(key)?))))
                    .collect();
                for (key, value) in kept {
                    if !fields.iter().any(|(own, _)| own == key) {
                        left.push((key.clone(), Self::from_value(value)));
                    }
                }
                Self::Object(left)
            }
            (Self::Array(items), Value::Array(kept)) => {
                // `kept` is a subsequence: each of its items is the next of
                // these that it was pruned from.
                let mut items = items.iter();
                let mut left = Vec::with_capacity(kept.len());
                for value in kept {
                    let own = items.by_ref().find(|item| item.holds(value));
                    left.push(
                        own.map_or_else(|| Self::from_value(value), |own| own.keeping(value)),
                    );
                }
                Self::Array(left)
            }
            _ if self.to_value() == *kept => self.clone(),
            _ => Self::from_value(kept),
        }
    }

    /// Whether `kept` could be this value with entries taken away.
    fn holds(&self, kept: &Value) -> bool {
        match (self, kept) {
            (Self::Object(fields), Value::Object(kept)) => kept.iter().all(|(key, value)| {
                fields
                    .iter()
                    .any(|(own, held)| own == key && held.holds(value))
            }),
            (Self::Array(items), Value::Array(kept)) => {
                let mut items = items.iter();
                kept.iter()
                    .all(|value| items.by_ref().any(|item| item.holds(value)))
            }
            _ => self.to_value() == *kept,
        }
    }

    /// Sets `key` on an object: in place where the object has it, and first
    /// where it does not. Nothing happens to a value that is not an object.
    pub(crate) fn set_first(&mut self, key: &str, value: Self) {
        let Self::Object(fields) = self else {
            return;
        };
        match fields.iter_mut().find(|(own, _)| own == key) {
            Some((_, held)) => *held = value,
            None => fields.insert(0, (key.to_string(), value)),
        }
    }

    /// The value under `key`, of an object that has it.
    pub(crate) fn field(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Object(fields) => fields
                .iter()
                .find(|(own, _)| own == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    /// As [`Ordered::field`], to change the value in place.
    pub(crate) fn field_mut(&mut self, key: &str) -> Option<&mut Self> {
        match self {
            Self::Object(fields) => fields
                .iter_mut()
                .find(|(own, _)| own == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    /// The keys of an object, in order. None for any other value.
    pub(crate) fn keys(&self) -> Vec<&str> {
        match self {
            Self::Object(fields) => fields.iter().map(|(key, _)| key.as_str()).collect(),
            _ => Vec::new(),
        }
    }

    /// The value as compact JSON, in its own order.
    pub(crate) fn compact(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "null".to_string())
    }
}

impl Serialize for Ordered {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Null => serializer.serialize_unit(),
            Self::Bool(flag) => serializer.serialize_bool(*flag),
            Self::Number(number) => number.serialize(serializer),
            Self::String(text) => serializer.serialize_str(text),
            Self::Array(items) => {
                let mut list = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    list.serialize_element(item)?;
                }
                list.end()
            }
            Self::Object(fields) => {
                let mut map = serializer.serialize_map(Some(fields.len()))?;
                for (key, value) in fields {
                    map.serialize_entry(key, value)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Ordered {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Read;

        impl<'de> Visitor<'de> for Read {
            type Value = Ordered;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON value")
            }

            fn visit_unit<E: de::Error>(self) -> Result<Ordered, E> {
                Ok(Ordered::Null)
            }

            fn visit_none<E: de::Error>(self) -> Result<Ordered, E> {
                Ok(Ordered::Null)
            }

            fn visit_some<D: Deserializer<'de>>(self, inner: D) -> Result<Ordered, D::Error> {
                Ordered::deserialize(inner)
            }

            fn visit_bool<E: de::Error>(self, flag: bool) -> Result<Ordered, E> {
                Ok(Ordered::Bool(flag))
            }

            fn visit_i64<E: de::Error>(self, number: i64) -> Result<Ordered, E> {
                Ok(Ordered::Number(number.into()))
            }

            fn visit_u64<E: de::Error>(self, number: u64) -> Result<Ordered, E> {
                Ok(Ordered::Number(number.into()))
            }

            fn visit_f64<E: de::Error>(self, number: f64) -> Result<Ordered, E> {
                // JSON has no number that is not finite, so this is one.
                Ok(Number::from_f64(number).map_or(Ordered::Null, Ordered::Number))
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<Ordered, E> {
                Ok(Ordered::String(text.to_string()))
            }

            fn visit_string<E: de::Error>(self, text: String) -> Result<Ordered, E> {
                Ok(Ordered::String(text))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut list: A) -> Result<Ordered, A::Error> {
                let mut items = Vec::with_capacity(list.size_hint().unwrap_or(0).min(1024));
                while let Some(item) = list.next_element()? {
                    items.push(item);
                }
                Ok(Ordered::Array(items))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Ordered, A::Error> {
                let mut fields: Vec<(String, Ordered)> = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, Ordered>()? {
                    match fields.iter_mut().find(|(own, _)| *own == key) {
                        Some((_, held)) => *held = value,
                        None => fields.push((key, value)),
                    }
                }
                Ok(Ordered::Object(fields))
            }
        }

        deserializer.deserialize_any(Read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TEXT: &str = r##"{"title": "T", "$id": "x", "b": [3, 1.5, {"z": null, "a": true}], "a": {"y": 1, "x": "é\n"}}"##;

    #[test]
    fn text_is_written_back_in_the_order_it_was_read() {
        let ordered = Ordered::parse(TEXT).unwrap();
        assert_eq!(
            ordered.compact(),
            r##"{"title":"T","$id":"x","b":[3,1.5,{"z":null,"a":true}],"a":{"y":1,"x":"é\n"}}"##
        );
        // It is the same JSON as a value read from the same text.
        assert_eq!(
            ordered.to_value(),
            serde_json::from_str::<Value>(TEXT).unwrap()
        );
        assert_eq!(
            serde_json::to_string_pretty(&ordered)
                .unwrap()
                .lines()
                .nth(1),
            Some(r#"  "title": "T","#)
        );
    }

    #[test]
    fn a_key_written_twice_keeps_its_place_and_its_last_value() {
        let ordered = Ordered::parse(r#"{"b": 1, "a": 2, "b": 3}"#).unwrap();
        assert_eq!(ordered.compact(), r#"{"b":3,"a":2}"#);
    }

    #[test]
    fn what_pruning_leaves_keeps_the_order_of_what_it_was_pruned_from() {
        let ordered = Ordered::parse(
            r##"{"oneOf": [{"$ref": "#/c"}, {"$ref": "#/a"}, {"$ref": "#/b"}],
                "$defs": {"c": {"y": 1, "x": 2}, "a": {}, "b": {"k": [1, 2]}}}"##,
        )
        .unwrap();
        let mut pruned = ordered.to_value();
        pruned["oneOf"].as_array_mut().unwrap().remove(1);
        pruned["$defs"].as_object_mut().unwrap().remove("a");
        assert_eq!(
            ordered.keeping(&pruned).compact(),
            r##"{"oneOf":[{"$ref":"#/c"},{"$ref":"#/b"}],"$defs":{"c":{"y":1,"x":2},"b":{"k":[1,2]}}}"##
        );
        assert_eq!(ordered.keeping(&pruned).to_value(), pruned);

        // Whatever `kept` is, the result is `kept`.
        for other in [
            json!({"new": 1, "oneOf": []}),
            json!({"oneOf": [{"$ref": "#/b"}, {"$ref": "#/zzz"}]}),
            json!([1, 2]),
            json!("text"),
        ] {
            assert_eq!(ordered.keeping(&other).to_value(), other, "{other}");
        }
        // A key the original never had goes after the ones it had.
        assert_eq!(
            ordered
                .keeping(&json!({"added": 1, "$defs": {}, "oneOf": []}))
                .compact(),
            r#"{"oneOf":[],"$defs":{},"added":1}"#
        );
    }

    #[test]
    fn a_key_is_set_in_place_or_first() {
        let mut ordered = Ordered::parse(r#"{"b": 1, "a": 2}"#).unwrap();
        ordered.set_first("a", Ordered::String("x".into()));
        ordered.set_first("id", Ordered::String("y".into()));
        assert_eq!(ordered.compact(), r#"{"id":"y","b":1,"a":"x"}"#);
    }
}
