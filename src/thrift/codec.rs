//! Conversion between named [`Value`]s and wire [`TValue`]s, driven by the
//! IDL [`Schema`].

use super::schema::{BaseType, Named, Schema, StructDef, TypeRef};
use super::value::{Map, Value};
use super::wire::{TType, TValue};
use crate::error::{Error, Result};

// ---------------------------------------------------------------------------
// Encoding (Value -> TValue)
// ---------------------------------------------------------------------------

/// Encode `value` as the struct `struct_name`. Unknown keys are ignored (the
/// same as linejs' generated writers); `Null` fields are skipped.
pub fn encode_struct(schema: &Schema, struct_name: &str, value: &Value) -> Result<Vec<(i16, TValue)>> {
    let def = schema
        .get_struct(struct_name)
        .ok_or_else(|| Error::Thrift(format!("unknown struct `{struct_name}`")))?;
    encode_with_def(schema, def, value)
}

fn encode_with_def(schema: &Schema, def: &StructDef, value: &Value) -> Result<Vec<(i16, TValue)>> {
    let mut out = Vec::new();
    match value {
        Value::Null => {}
        Value::Struct(map) => {
            for field in &def.fields {
                let v = map.get(&field.name).or_else(|| map.get(&field.id.to_string()));
                let Some(v) = v else { continue };
                if let Some(tv) = encode_field(schema, &field.ty, v)
                    .map_err(|e| Error::Thrift(format!("{}.{}: {e}", def.name, field.name)))?
                {
                    out.push((field.id, tv));
                }
            }
            for key in map.keys() {
                if def.field_by_name(key).is_none() && key.parse::<i16>().map_or(true, |id| def.field_by_id(id).is_none())
                {
                    tracing::debug!(struct_name = %def.name, field = %key, "ignoring unknown field");
                }
            }
        }
        other => {
            return Err(Error::Thrift(format!("{}: expected struct, got {other}", def.name)));
        }
    }
    Ok(out)
}

/// Field-level encode. Returns `None` when the field must be omitted: null
/// values and, like linejs, structs whose IDL definition has no fields
/// (linejs' generated writers return `[]` only for those). A given struct
/// whose fields are all unset is still sent as an empty struct.
fn encode_field(schema: &Schema, ty: &TypeRef, v: &Value) -> Result<Option<TValue>> {
    if v.is_null() {
        return Ok(None);
    }
    let tv = encode_value(schema, ty, v)?;
    if let TValue::Struct(fields) = &tv
        && fields.is_empty()
        && let TypeRef::Named(name) = ty
        && schema.get_struct(name).is_some_and(|d| d.fields.is_empty())
    {
        return Ok(None);
    }
    Ok(Some(tv))
}

pub fn encode_value(schema: &Schema, ty: &TypeRef, v: &Value) -> Result<TValue> {
    match ty {
        TypeRef::Base(b) => encode_base(*b, v),
        TypeRef::List(inner) | TypeRef::Set(inner) => {
            let items = match v {
                Value::List(items) => items,
                other => return Err(Error::Thrift(format!("expected list, got {other}"))),
            };
            let et = wire_type(schema, inner, items.first());
            let enc = items
                .iter()
                .filter(|x| !x.is_null())
                .map(|x| encode_value(schema, inner, x))
                .collect::<Result<Vec<_>>>()?;
            Ok(if matches!(ty, TypeRef::List(_)) { TValue::List(et, enc) } else { TValue::Set(et, enc) })
        }
        TypeRef::Map(kt, vt) => {
            let mut entries = Vec::new();
            match v {
                Value::Struct(m) => {
                    for (k, val) in m {
                        if val.is_null() {
                            continue;
                        }
                        entries.push((encode_key(schema, kt, k)?, encode_value(schema, vt, val)?));
                    }
                }
                Value::Map(m) => {
                    for (k, val) in m {
                        if val.is_null() {
                            continue;
                        }
                        entries.push((encode_value(schema, kt, k)?, encode_value(schema, vt, val)?));
                    }
                }
                other => return Err(Error::Thrift(format!("expected map, got {other}"))),
            }
            let first = entries.first();
            let ktt = first.map(|(k, _)| k.ttype()).unwrap_or_else(|| wire_type(schema, kt, None));
            let vtt = first.map(|(_, v)| v.ttype()).unwrap_or_else(|| wire_type(schema, vt, None));
            Ok(TValue::Map(ktt, vtt, entries))
        }
        TypeRef::Named(name) => match schema.resolve(name) {
            Some(Named::Struct(def)) => Ok(TValue::Struct(encode_with_def(schema, def, v)?)),
            Some(Named::Enum(def)) => match v {
                Value::String(s) => def
                    .value_of(s)
                    .or_else(|| s.parse().ok())
                    .map(TValue::I32)
                    .ok_or_else(|| Error::Thrift(format!("unknown {} variant `{s}`", def.name))),
                other => other
                    .as_i64()
                    .map(|i| TValue::I32(i as i32))
                    .ok_or_else(|| Error::Thrift(format!("expected {} enum, got {other}", def.name))),
            },
            None => encode_base(BaseType::Any, v),
        },
    }
}

/// Map keys arrive as strings from struct-style maps; coerce to key type.
fn encode_key(schema: &Schema, kt: &TypeRef, k: &str) -> Result<TValue> {
    let v = match kt {
        TypeRef::Base(BaseType::I16 | BaseType::I32 | BaseType::I64 | BaseType::Byte) => {
            Value::Int(k.parse().map_err(|_| Error::Thrift(format!("map key `{k}` is not an integer")))?)
        }
        TypeRef::Base(BaseType::Double) => {
            Value::Double(k.parse().map_err(|_| Error::Thrift(format!("map key `{k}` is not a number")))?)
        }
        _ => Value::String(k.to_string()),
    };
    encode_value(schema, kt, &v)
}

fn encode_base(b: BaseType, v: &Value) -> Result<TValue> {
    let int = |v: &Value| v.as_i64().ok_or_else(|| Error::Thrift(format!("expected integer, got {v}")));
    Ok(match b {
        BaseType::Bool => TValue::Bool(v.as_bool().ok_or_else(|| Error::Thrift(format!("expected bool, got {v}")))?),
        BaseType::Byte => TValue::Byte(int(v)? as i8),
        BaseType::I16 => TValue::I16(int(v)? as i16),
        BaseType::I32 => TValue::I32(int(v)? as i32),
        BaseType::I64 => TValue::I64(int(v)?),
        BaseType::Double => TValue::Double(v.as_f64().ok_or_else(|| Error::Thrift(format!("expected double, got {v}")))?),
        BaseType::String | BaseType::Binary => match v {
            Value::String(s) => TValue::Binary(s.as_bytes().to_vec()),
            Value::Binary(b) => TValue::Binary(b.clone()),
            Value::Int(i) => TValue::Binary(i.to_string().into_bytes()),
            other => return Err(Error::Thrift(format!("expected string, got {other}"))),
        },
        BaseType::Any | BaseType::Void => infer(v)?,
    })
}

/// Best-effort encoding of a value with no schema information.
fn infer(v: &Value) -> Result<TValue> {
    Ok(match v {
        Value::Null => return Err(Error::Thrift("cannot infer type of null".into())),
        Value::Bool(b) => TValue::Bool(*b),
        Value::Int(i) => TValue::I64(*i),
        Value::Double(d) => TValue::Double(*d),
        Value::String(s) => TValue::Binary(s.as_bytes().to_vec()),
        Value::Binary(b) => TValue::Binary(b.clone()),
        Value::List(items) => {
            let enc = items.iter().filter(|x| !x.is_null()).map(infer).collect::<Result<Vec<_>>>()?;
            let et = enc.first().map(TValue::ttype).unwrap_or(TType::String);
            TValue::List(et, enc)
        }
        Value::Map(m) => {
            let enc = m.iter().map(|(k, v)| Ok((infer(k)?, infer(v)?))).collect::<Result<Vec<_>>>()?;
            let (kt, vt) = enc.first().map(|(k, v)| (k.ttype(), v.ttype())).unwrap_or((TType::String, TType::String));
            TValue::Map(kt, vt, enc)
        }
        Value::Struct(m) => {
            let mut fields = Vec::new();
            for (k, v) in m {
                let id: i16 = k
                    .parse()
                    .map_err(|_| Error::Thrift(format!("cannot infer field id for `{k}` without a schema")))?;
                if !v.is_null() {
                    fields.push((id, infer(v)?));
                }
            }
            TValue::Struct(fields)
        }
    })
}

/// Wire type for a schema type (used for container element headers).
fn wire_type(schema: &Schema, ty: &TypeRef, sample: Option<&Value>) -> TType {
    match ty {
        TypeRef::Base(b) => match b {
            BaseType::Bool => TType::Bool,
            BaseType::Byte => TType::Byte,
            BaseType::I16 => TType::I16,
            BaseType::I32 => TType::I32,
            BaseType::I64 => TType::I64,
            BaseType::Double => TType::Double,
            BaseType::String | BaseType::Binary => TType::String,
            BaseType::Any | BaseType::Void => {
                sample.and_then(|s| infer(s).ok()).map(|t| t.ttype()).unwrap_or(TType::String)
            }
        },
        TypeRef::List(_) => TType::List,
        TypeRef::Set(_) => TType::Set,
        TypeRef::Map(..) => TType::Map,
        TypeRef::Named(n) => match schema.resolve(n) {
            Some(Named::Struct(_)) => TType::Struct,
            Some(Named::Enum(_)) => TType::I32,
            None => TType::Struct,
        },
    }
}

// ---------------------------------------------------------------------------
// Decoding (TValue -> Value)
// ---------------------------------------------------------------------------

/// Decode a wire struct as `struct_name`. If the schema has no such struct the
/// result is keyed by field id (like linejs with `parse = false`).
pub fn decode_struct(schema: &Schema, struct_name: Option<&str>, tv: &TValue) -> Value {
    match struct_name.and_then(|n| schema.get_struct(n)) {
        Some(def) => decode_with_def(schema, def, tv),
        None => decode_raw(tv),
    }
}

fn decode_with_def(schema: &Schema, def: &StructDef, tv: &TValue) -> Value {
    let TValue::Struct(fields) = tv else { return decode_raw(tv) };
    let mut out = Map::new();
    for (fid, v) in fields {
        match def.field_by_id(*fid) {
            Some(f) => {
                out.insert(f.name.clone(), decode_value(schema, &f.ty, v));
            }
            None => {
                out.insert(fid.to_string(), decode_raw(v));
            }
        }
    }
    Value::Struct(out)
}

pub fn decode_value(schema: &Schema, ty: &TypeRef, tv: &TValue) -> Value {
    match (ty, tv) {
        (TypeRef::Named(name), _) => match schema.resolve(name) {
            Some(Named::Struct(def)) => decode_with_def(schema, def, tv),
            Some(Named::Enum(def)) => match tv.as_i64() {
                Some(i) => def.name_of(i as i32).map(|n| Value::String(n.to_string())).unwrap_or(Value::Int(i)),
                None => decode_raw(tv),
            },
            None => decode_raw(tv),
        },
        (TypeRef::List(inner) | TypeRef::Set(inner), TValue::List(_, items) | TValue::Set(_, items)) => {
            Value::List(items.iter().map(|x| decode_value(schema, inner, x)).collect())
        }
        (TypeRef::Map(kt, vt), TValue::Map(_, _, entries)) => Value::Map(
            entries.iter().map(|(k, v)| (decode_value(schema, kt, k), decode_value(schema, vt, v))).collect(),
        ),
        (TypeRef::Base(BaseType::Binary), TValue::Binary(b)) => Value::Binary(b.clone()),
        _ => decode_raw(tv),
    }
}

/// Schema-less decode: field ids become keys, strings are UTF-8 if possible.
pub fn decode_raw(tv: &TValue) -> Value {
    match tv {
        TValue::Bool(b) => Value::Bool(*b),
        TValue::Byte(v) => Value::Int(*v as i64),
        TValue::I16(v) => Value::Int(*v as i64),
        TValue::I32(v) => Value::Int(*v as i64),
        TValue::I64(v) => Value::Int(*v),
        TValue::Double(v) => Value::Double(*v),
        TValue::Binary(b) => match std::str::from_utf8(b) {
            Ok(s) => Value::String(s.to_string()),
            Err(_) => Value::Binary(b.clone()),
        },
        TValue::Struct(fields) => Value::Struct(fields.iter().map(|(id, v)| (id.to_string(), decode_raw(v))).collect()),
        TValue::List(_, items) | TValue::Set(_, items) => Value::List(items.iter().map(decode_raw).collect()),
        TValue::Map(_, _, entries) => Value::Map(entries.iter().map(|(k, v)| (decode_raw(k), decode_raw(v))).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thrift::wire::{read_struct, write_struct, Protocol};
    use crate::tv;

    #[test]
    fn send_message_args_roundtrip() {
        let schema = Schema::line();
        let args = tv!({
            "seq": 3,
            "message": {
                "to": "u0123456789abcdef0123456789abcdef",
                "text": "hello",
                "contentType": "NONE",
                "contentMetadata": {"e2eeVersion": "2"},
                "hasContent": false,
                "unknownField": 1
            }
        });
        let fields = encode_struct(schema, "sendMessage_args", &args).unwrap();
        assert_eq!(fields[0], (1, TValue::I32(3)));
        let msg = fields[1].1.clone();
        assert_eq!(msg.field(2).and_then(TValue::as_str), Some("u0123456789abcdef0123456789abcdef"));
        assert_eq!(msg.field(15), Some(&TValue::I32(0)));

        let bytes = write_struct(&fields, Protocol::Compact).unwrap();
        let back = read_struct(&bytes, Protocol::Compact).unwrap();
        let decoded = decode_struct(schema, Some("sendMessage_args"), &back);
        assert_eq!(decoded["message"]["text"].as_str(), Some("hello"));
        assert_eq!(decoded["message"]["contentType"].as_str(), Some("NONE"));
        assert_eq!(decoded["message"]["contentMetadata"]["e2eeVersion"].as_str(), Some("2"));
    }

    #[test]
    fn empty_struct_fields_are_sent_like_linejs() {
        let schema = Schema::line();
        let fields = encode_struct(schema, "sendMessage_args", &tv!({"message": {}})).unwrap();
        assert_eq!(fields, vec![(2, TValue::Struct(vec![]))]);
        let none = encode_struct(schema, "sendMessage_args", &tv!({})).unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn unknown_enum_values_stay_numeric() {
        let schema = Schema::line();
        let tv = TValue::Struct(vec![(15, TValue::I32(99999)), (99, TValue::I32(1))]);
        let v = decode_struct(schema, Some("Message"), &tv);
        assert_eq!(v["contentType"].as_i64(), Some(99999));
        assert_eq!(v["99"].as_i64(), Some(1));
    }
}
