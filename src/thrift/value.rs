//! Named, schema-less dynamic value — the Rust counterpart of the loose JS
//! objects linejs hands back after `rename_thrift`.
//!
//! Struct fields are keyed by their IDL name (or by the numeric field id when
//! no schema knows the field), enums are rendered as their variant name, and
//! strings that are not valid UTF-8 stay as [`Value::Binary`].

use std::fmt;
use std::ops::Index;

use indexmap::IndexMap;
use serde::de::{self, DeserializeOwned, IntoDeserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{Error, Result};

pub type Map = IndexMap<String, Value>;

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    Int(i64),
    Double(f64),
    String(String),
    Binary(Vec<u8>),
    List(Vec<Value>),
    /// Thrift `map<K, V>` with arbitrary key values.
    Map(Vec<(Value, Value)>),
    /// Thrift struct, keyed by field name.
    Struct(Map),
}

static NULL: Value = Value::Null;

impl Value {
    pub fn object() -> Self {
        Value::Struct(Map::new())
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Struct(m) => m.get(key).filter(|v| !v.is_null()),
            Value::Map(entries) => entries
                .iter()
                .find(|(k, _)| k.key_string().as_deref() == Some(key))
                .map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        match self {
            Value::Struct(m) => m.get_mut(key),
            _ => None,
        }
    }

    /// Insert into a struct value (turns `Null` into an empty struct first).
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<Value>) {
        if self.is_null() {
            *self = Value::object();
        }
        if let Value::Struct(m) = self {
            m.insert(key.into(), value.into());
        }
    }

    pub fn remove(&mut self, key: &str) -> Option<Value> {
        match self {
            Value::Struct(m) => m.shift_remove(key),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            Value::Binary(b) => std::str::from_utf8(b).ok(),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::String(s) => Some(s.as_bytes()),
            Value::Binary(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(v) => Some(*v),
            Value::Double(v) => Some(*v as i64),
            Value::String(s) => s.parse().ok(),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(v) => Some(*v as f64),
            Value::Double(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            Value::Int(v) => Some(*v != 0),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_struct(&self) -> Option<&Map> {
        match self {
            Value::Struct(m) => Some(m),
            _ => None,
        }
    }

    pub fn as_map(&self) -> Option<&[(Value, Value)]> {
        match self {
            Value::Map(m) => Some(m),
            _ => None,
        }
    }

    /// Iterate a struct or a map as `(string key, value)` pairs.
    pub fn entries(&self) -> Vec<(String, &Value)> {
        match self {
            Value::Struct(m) => m.iter().map(|(k, v)| (k.clone(), v)).collect(),
            Value::Map(m) => m.iter().filter_map(|(k, v)| k.key_string().map(|k| (k, v))).collect(),
            _ => Vec::new(),
        }
    }

    /// `map<string,string>` → `IndexMap<String,String>` convenience.
    pub fn string_map(&self) -> IndexMap<String, String> {
        self.entries()
            .into_iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
            .collect()
    }

    pub(crate) fn key_string(&self) -> Option<String> {
        match self {
            Value::String(s) => Some(s.clone()),
            Value::Int(i) => Some(i.to_string()),
            Value::Bool(b) => Some(b.to_string()),
            Value::Double(d) => Some(d.to_string()),
            Value::Binary(b) => std::str::from_utf8(b).ok().map(str::to_string),
            _ => None,
        }
    }

    /// Convert into JSON. Binary values become base64 strings.
    pub fn to_json(&self) -> serde_json::Value {
        use base64::Engine;
        match self {
            Value::Null => serde_json::Value::Null,
            Value::Bool(b) => (*b).into(),
            Value::Int(i) => (*i).into(),
            Value::Double(d) => serde_json::Number::from_f64(*d).map(Into::into).unwrap_or(serde_json::Value::Null),
            Value::String(s) => s.clone().into(),
            Value::Binary(b) => base64::engine::general_purpose::STANDARD.encode(b).into(),
            Value::List(l) => l.iter().map(Value::to_json).collect(),
            Value::Map(m) => m
                .iter()
                .map(|(k, v)| (k.key_string().unwrap_or_default(), v.to_json()))
                .collect::<serde_json::Map<_, _>>()
                .into(),
            Value::Struct(m) => {
                m.iter().map(|(k, v)| (k.clone(), v.to_json())).collect::<serde_json::Map<_, _>>().into()
            }
        }
    }

    /// Deserialize into a typed Rust structure.
    pub fn deserialize_into<T: DeserializeOwned>(&self) -> Result<T> {
        T::deserialize(self.clone()).map_err(|e| Error::Thrift(e.to_string()))
    }
}

impl Index<&str> for Value {
    type Output = Value;
    fn index(&self, key: &str) -> &Value {
        self.get(key).unwrap_or(&NULL)
    }
}

impl Index<usize> for Value {
    type Output = Value;
    fn index(&self, idx: usize) -> &Value {
        match self {
            Value::List(l) => l.get(idx).unwrap_or(&NULL),
            _ => &NULL,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_json())
    }
}

macro_rules! from_int {
    ($($t:ty),*) => {$(
        impl From<$t> for Value {
            fn from(v: $t) -> Self { Value::Int(v as i64) }
        }
    )*};
}
from_int!(i8, i16, i32, i64, u8, u16, u32, usize);

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}
impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Double(v)
    }
}
impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::String(v.to_string())
    }
}
impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::String(v)
    }
}
impl From<&String> for Value {
    fn from(v: &String) -> Self {
        Value::String(v.clone())
    }
}
impl From<Vec<u8>> for Value {
    fn from(v: Vec<u8>) -> Self {
        Value::Binary(v)
    }
}
impl From<Map> for Value {
    fn from(v: Map) -> Self {
        Value::Struct(v)
    }
}
impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(v: Option<T>) -> Self {
        v.map(Into::into).unwrap_or(Value::Null)
    }
}
impl<T: Into<Value>> From<Vec<T>> for Value
where
    T: NotU8,
{
    fn from(v: Vec<T>) -> Self {
        Value::List(v.into_iter().map(Into::into).collect())
    }
}

/// Marker so `Vec<u8>` maps to binary while other vectors map to lists.
pub trait NotU8 {}
impl NotU8 for Value {}
impl NotU8 for String {}
impl NotU8 for &str {}
impl NotU8 for i32 {}
impl NotU8 for i64 {}
impl NotU8 for Vec<u8> {}

impl From<serde_json::Value> for Value {
    fn from(v: serde_json::Value) -> Self {
        match v {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(b) => Value::Bool(b),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Value::Int(i)
                } else if let Some(u) = n.as_u64() {
                    Value::Int(u as i64)
                } else {
                    Value::Double(n.as_f64().unwrap_or_default())
                }
            }
            serde_json::Value::String(s) => Value::String(s),
            serde_json::Value::Array(a) => Value::List(a.into_iter().map(Into::into).collect()),
            serde_json::Value::Object(o) => Value::Struct(o.into_iter().map(|(k, v)| (k, v.into())).collect()),
        }
    }
}

/// Build a [`Value`] with JSON syntax: `tv!({"to": mid, "text": "hi"})`.
#[macro_export]
macro_rules! tv {
    ($($json:tt)+) => {
        $crate::thrift::Value::from($crate::__serde_json::json!($($json)+))
    };
}

// ---------------------------------------------------------------------------
// serde integration
// ---------------------------------------------------------------------------

impl Serialize for Value {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Value::Null => s.serialize_none(),
            Value::Bool(b) => s.serialize_bool(*b),
            Value::Int(i) => s.serialize_i64(*i),
            Value::Double(d) => s.serialize_f64(*d),
            Value::String(v) => s.serialize_str(v),
            Value::Binary(b) => s.serialize_bytes(b),
            Value::List(l) => {
                let mut seq = s.serialize_seq(Some(l.len()))?;
                for v in l {
                    seq.serialize_element(v)?;
                }
                seq.end()
            }
            Value::Map(m) => {
                let mut map = s.serialize_map(Some(m.len()))?;
                for (k, v) in m {
                    map.serialize_entry(&k.key_string().unwrap_or_default(), v)?;
                }
                map.end()
            }
            Value::Struct(m) => {
                let mut map = s.serialize_map(Some(m.len()))?;
                for (k, v) in m {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Value {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Value;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("any value")
            }
            fn visit_bool<E>(self, v: bool) -> std::result::Result<Value, E> {
                Ok(Value::Bool(v))
            }
            fn visit_i64<E>(self, v: i64) -> std::result::Result<Value, E> {
                Ok(Value::Int(v))
            }
            fn visit_u64<E>(self, v: u64) -> std::result::Result<Value, E> {
                Ok(Value::Int(v as i64))
            }
            fn visit_f64<E>(self, v: f64) -> std::result::Result<Value, E> {
                Ok(Value::Double(v))
            }
            fn visit_str<E>(self, v: &str) -> std::result::Result<Value, E> {
                Ok(Value::String(v.to_string()))
            }
            fn visit_string<E>(self, v: String) -> std::result::Result<Value, E> {
                Ok(Value::String(v))
            }
            fn visit_bytes<E>(self, v: &[u8]) -> std::result::Result<Value, E> {
                Ok(Value::Binary(v.to_vec()))
            }
            fn visit_byte_buf<E>(self, v: Vec<u8>) -> std::result::Result<Value, E> {
                Ok(Value::Binary(v))
            }
            fn visit_none<E>(self) -> std::result::Result<Value, E> {
                Ok(Value::Null)
            }
            fn visit_unit<E>(self) -> std::result::Result<Value, E> {
                Ok(Value::Null)
            }
            fn visit_some<D: Deserializer<'de>>(self, d: D) -> std::result::Result<Value, D::Error> {
                Value::deserialize(d)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> std::result::Result<Value, A::Error> {
                let mut out = Vec::new();
                while let Some(v) = a.next_element()? {
                    out.push(v);
                }
                Ok(Value::List(out))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> std::result::Result<Value, A::Error> {
                let mut out = Map::new();
                while let Some((k, v)) = a.next_entry::<String, Value>()? {
                    out.insert(k, v);
                }
                Ok(Value::Struct(out))
            }
        }
        d.deserialize_any(V)
    }
}

/// Error type for deserializing out of a [`Value`].
#[derive(Debug)]
pub struct DeError(String);
impl fmt::Display for DeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for DeError {}
impl de::Error for DeError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        DeError(msg.to_string())
    }
}

impl<'de> IntoDeserializer<'de, DeError> for Value {
    type Deserializer = Value;
    fn into_deserializer(self) -> Value {
        self
    }
}

impl<'de> Deserializer<'de> for Value {
    type Error = DeError;

    fn deserialize_any<V: Visitor<'de>>(self, v: V) -> std::result::Result<V::Value, DeError> {
        match self {
            Value::Null => v.visit_none(),
            Value::Bool(b) => v.visit_bool(b),
            Value::Int(i) => v.visit_i64(i),
            Value::Double(d) => v.visit_f64(d),
            Value::String(s) => v.visit_string(s),
            Value::Binary(b) => match String::from_utf8(b) {
                Ok(s) => v.visit_string(s),
                Err(e) => v.visit_byte_buf(e.into_bytes()),
            },
            Value::List(l) => v.visit_seq(de::value::SeqDeserializer::new(l.into_iter())),
            Value::Map(m) => v.visit_map(de::value::MapDeserializer::new(
                m.into_iter().map(|(k, val)| (Value::String(k.key_string().unwrap_or_default()), val)),
            )),
            Value::Struct(m) => {
                v.visit_map(de::value::MapDeserializer::new(m.into_iter().map(|(k, val)| (Value::String(k), val))))
            }
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, v: V) -> std::result::Result<V::Value, DeError> {
        match self {
            Value::Null => v.visit_none(),
            other => v.visit_some(other),
        }
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, v: V) -> std::result::Result<V::Value, DeError> {
        match self {
            Value::Binary(b) => v.visit_byte_buf(b),
            Value::String(s) => v.visit_byte_buf(s.into_bytes()),
            other => other.deserialize_any(v),
        }
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, v: V) -> std::result::Result<V::Value, DeError> {
        self.deserialize_bytes(v)
    }

    fn deserialize_string<V: Visitor<'de>>(self, v: V) -> std::result::Result<V::Value, DeError> {
        match self {
            Value::Int(i) => v.visit_string(i.to_string()),
            Value::Binary(b) => v.visit_string(String::from_utf8_lossy(&b).into_owned()),
            other => other.deserialize_any(v),
        }
    }

    fn deserialize_str<V: Visitor<'de>>(self, v: V) -> std::result::Result<V::Value, DeError> {
        self.deserialize_string(v)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        v: V,
    ) -> std::result::Result<V::Value, DeError> {
        match self {
            Value::String(s) => v.visit_enum(s.into_deserializer()),
            Value::Int(i) => v.visit_enum(i.to_string().into_deserializer()),
            other => Err(DeError(format!("expected enum, got {other}"))),
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        v: V,
    ) -> std::result::Result<V::Value, DeError> {
        v.visit_newtype_struct(self)
    }

    fn deserialize_unit<V: Visitor<'de>>(self, v: V) -> std::result::Result<V::Value, DeError> {
        v.visit_unit()
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char
        unit_struct seq tuple tuple_struct map struct identifier ignored_any
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_and_deserialize() {
        let v = tv!({"mid": "u1", "n": 3, "nested": {"list": [1, 2]}});
        assert_eq!(v["mid"].as_str(), Some("u1"));
        assert_eq!(v["nested"]["list"][1].as_i64(), Some(2));
        assert!(v["missing"]["x"].is_null());

        #[derive(Deserialize)]
        struct S {
            mid: String,
            n: i32,
            #[serde(default)]
            other: Option<String>,
        }
        let s: S = v.deserialize_into().unwrap();
        assert_eq!(s.mid, "u1");
        assert_eq!(s.n, 3);
        assert!(s.other.is_none());
    }
}
