//! A minimal protobuf wire-format codec, hand-written for the few messages
//! line-rs needs (LINE's gRPC services: ServiceChat, PlaceSearch, Calendar). This is deliberately not a
//! general protobuf library and there is no code generation: each message is
//! encoded and decoded by hand against the field numbers reverse-engineered
//! from the LINE app, matching the "embed the IDL, no codegen" approach used
//! for Thrift.
//!
//! Only proto3 scalars, length-delimited fields (strings, bytes, submessages)
//! and packed/unpacked repeated fields appear here. Groups (wire type 3/4) are
//! obsolete and unsupported.

use crate::error::{Error, Result};

/// Wire type in the low 3 bits of a field tag.
pub mod wire {
    pub const VARINT: u8 = 0;
    pub const I64: u8 = 1;
    pub const LEN: u8 = 2;
    pub const I32: u8 = 5;
}

const ERR: &str = "ProtobufError";

/// Builds a protobuf message. Proto3 semantics: scalar fields equal to their
/// default (0, `false`, empty) are skipped by the `*` helpers; use the
/// `*_always` helpers for `oneof` members or where explicit presence matters.
#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    fn put_varint(&mut self, mut v: u64) {
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.buf.push(byte);
                break;
            }
            self.buf.push(byte | 0x80);
        }
    }

    fn tag(&mut self, field: u32, wire: u8) {
        self.put_varint(((field as u64) << 3) | wire as u64);
    }

    pub fn int64_always(&mut self, field: u32, v: i64) {
        self.tag(field, wire::VARINT);
        self.put_varint(v as u64);
    }

    pub fn int64(&mut self, field: u32, v: i64) {
        if v != 0 {
            self.int64_always(field, v);
        }
    }

    pub fn int32(&mut self, field: u32, v: i32) {
        if v != 0 {
            // int32 is sign-extended to 64 bits on the wire.
            self.int64_always(field, v as i64);
        }
    }

    pub fn uint32(&mut self, field: u32, v: u32) {
        if v != 0 {
            self.tag(field, wire::VARINT);
            self.put_varint(v as u64);
        }
    }

    /// A proto enum, encoded like an int32.
    pub fn enumeration(&mut self, field: u32, v: i32) {
        self.int32(field, v);
    }

    /// `double` (fixed64, little-endian IEEE 754).
    pub fn double(&mut self, field: u32, v: f64) {
        if v != 0.0 {
            self.tag(field, wire::I64);
            self.buf.extend_from_slice(&v.to_bits().to_le_bytes());
        }
    }

    pub fn bool(&mut self, field: u32, v: bool) {
        if v {
            self.tag(field, wire::VARINT);
            self.put_varint(1);
        }
    }

    pub fn string_always(&mut self, field: u32, v: &str) {
        self.bytes_always(field, v.as_bytes());
    }

    pub fn string(&mut self, field: u32, v: &str) {
        if !v.is_empty() {
            self.string_always(field, v);
        }
    }

    pub fn bytes_always(&mut self, field: u32, v: &[u8]) {
        self.tag(field, wire::LEN);
        self.put_varint(v.len() as u64);
        self.buf.extend_from_slice(v);
    }

    pub fn bytes(&mut self, field: u32, v: &[u8]) {
        if !v.is_empty() {
            self.bytes_always(field, v);
        }
    }

    /// A submessage; `body` is its already-encoded bytes. Always emitted (an
    /// empty submessage is still a present field).
    pub fn message(&mut self, field: u32, body: &[u8]) {
        self.bytes_always(field, body);
    }
}

/// A single field's decoded value, borrowing from the source buffer.
#[derive(Debug, Clone, Copy)]
pub enum Value<'a> {
    Varint(u64),
    I64(u64),
    I32(u32),
    Len(&'a [u8]),
}

impl<'a> Value<'a> {
    pub fn as_u64(self) -> Result<u64> {
        match self {
            Value::Varint(v) => Ok(v),
            Value::I64(v) => Ok(v),
            Value::I32(v) => Ok(v as u64),
            Value::Len(_) => Err(Error::internal(ERR, "expected a numeric field")),
        }
    }

    pub fn as_i64(self) -> Result<i64> {
        Ok(self.as_u64()? as i64)
    }

    pub fn as_i32(self) -> Result<i32> {
        Ok(self.as_u64()? as i32)
    }

    /// A `double` field (wire type I64).
    pub fn as_f64(self) -> Result<f64> {
        match self {
            Value::I64(v) => Ok(f64::from_bits(v)),
            _ => Err(Error::internal(ERR, "expected a double field")),
        }
    }

    pub fn as_bool(self) -> Result<bool> {
        Ok(self.as_u64()? != 0)
    }

    pub fn as_bytes(self) -> Result<&'a [u8]> {
        match self {
            Value::Len(b) => Ok(b),
            _ => Err(Error::internal(ERR, "expected a length-delimited field")),
        }
    }

    pub fn as_str(self) -> Result<&'a str> {
        std::str::from_utf8(self.as_bytes()?)
            .map_err(|_| Error::internal(ERR, "field is not valid UTF-8"))
    }

    pub fn as_message(self) -> Result<Reader<'a>> {
        Ok(Reader::new(self.as_bytes()?))
    }
}

/// Iterates the fields of a protobuf message. Unknown field numbers are
/// returned too; callers match on the numbers they know and ignore the rest.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

/// One decoded field: its number and value.
pub struct Field<'a> {
    pub number: u32,
    pub value: Value<'a>,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn get_varint(&mut self) -> Result<u64> {
        let mut result: u64 = 0;
        let mut shift = 0;
        loop {
            let byte = *self
                .buf
                .get(self.pos)
                .ok_or_else(|| Error::internal(ERR, "truncated varint"))?;
            self.pos += 1;
            if shift >= 64 {
                return Err(Error::internal(ERR, "varint overflows 64 bits"));
            }
            result |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|end| *end <= self.buf.len())
            .ok_or_else(|| Error::internal(ERR, "length-delimited field runs past end"))?;
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    /// The next field, or `None` at the end of the message.
    pub fn next_field(&mut self) -> Result<Option<Field<'a>>> {
        if self.pos >= self.buf.len() {
            return Ok(None);
        }
        let tag = self.get_varint()?;
        let number = (tag >> 3) as u32;
        let wire = (tag & 0x7) as u8;
        if number == 0 {
            return Err(Error::internal(ERR, "field number 0 is invalid"));
        }
        let value = match wire {
            wire::VARINT => Value::Varint(self.get_varint()?),
            wire::I64 => {
                let b = self.take(8)?;
                Value::I64(u64::from_le_bytes(b.try_into().unwrap()))
            }
            wire::I32 => {
                let b = self.take(4)?;
                Value::I32(u32::from_le_bytes(b.try_into().unwrap()))
            }
            wire::LEN => {
                let len = self.get_varint()? as usize;
                Value::Len(self.take(len)?)
            }
            other => {
                return Err(Error::internal(
                    ERR,
                    format!("unsupported wire type {other}"),
                ));
            }
        };
        Ok(Some(Field { number, value }))
    }
}

/// Reads a packed repeated varint field (e.g. `repeated int32 reminder`).
pub fn packed_varints(buf: &[u8]) -> Result<Vec<u64>> {
    let mut r = Reader::new(buf);
    let mut out = Vec::new();
    while r.pos < r.buf.len() {
        out.push(r.get_varint()?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn double_round_trip() {
        let mut w = Writer::new();
        w.double(4, 35.6812);
        w.double(5, 0.0);
        let bytes = w.into_bytes();
        assert_eq!(bytes.len(), 9, "zero is skipped");
        let f = Reader::new(&bytes).next_field().unwrap().unwrap();
        assert_eq!(f.number, 4);
        assert_eq!(f.value.as_f64().unwrap(), 35.6812);
    }

    #[test]
    fn varint_roundtrip() {
        for v in [0u64, 1, 127, 128, 300, 16384, u32::MAX as u64, u64::MAX] {
            let mut w = Writer::new();
            w.int64_always(1, v as i64);
            let mut r = Reader::new(w.as_bytes());
            let f = r.next_field().unwrap().unwrap();
            assert_eq!(f.number, 1);
            assert_eq!(f.value.as_u64().unwrap(), v);
            assert!(r.next_field().unwrap().is_none());
        }
    }

    #[test]
    fn skips_proto3_defaults() {
        let mut w = Writer::new();
        w.int64(1, 0);
        w.bool(2, false);
        w.string(3, "");
        assert!(w.as_bytes().is_empty());
    }

    #[test]
    fn mixed_message() {
        let mut inner = Writer::new();
        inner.string(1, "hi");
        let mut w = Writer::new();
        w.string(1, "cal-id");
        w.int64(3, 1_700_000_000_000);
        w.bool(7, true);
        w.message(11, inner.as_bytes());

        let mut r = Reader::new(w.as_bytes());
        let f = r.next_field().unwrap().unwrap();
        assert_eq!(f.number, 1);
        assert_eq!(f.value.as_str().unwrap(), "cal-id");
        let f = r.next_field().unwrap().unwrap();
        assert_eq!(f.number, 3);
        assert_eq!(f.value.as_i64().unwrap(), 1_700_000_000_000);
        let f = r.next_field().unwrap().unwrap();
        assert_eq!(f.number, 7);
        assert!(f.value.as_bool().unwrap());
        let f = r.next_field().unwrap().unwrap();
        assert_eq!(f.number, 11);
        let mut sub = f.value.as_message().unwrap();
        let sf = sub.next_field().unwrap().unwrap();
        assert_eq!(sf.number, 1);
        assert_eq!(sf.value.as_str().unwrap(), "hi");
        assert!(r.next_field().unwrap().is_none());
    }

    #[test]
    fn truncated_varint_errors() {
        let mut r = Reader::new(&[0x08, 0x80]);
        assert!(r.next_field().is_err());
    }

    #[test]
    fn length_past_end_errors() {
        // field 1, LEN, length 10, but only 2 bytes follow
        let mut r = Reader::new(&[0x0a, 0x0a, 0x01, 0x02]);
        assert!(r.next_field().is_err());
    }

    #[test]
    fn packed_varint_list() {
        let mut body = Writer::new();
        // emulate a packed field body: varints 1,2,300
        body.put_varint(1);
        body.put_varint(2);
        body.put_varint(300);
        assert_eq!(packed_varints(body.as_bytes()).unwrap(), vec![1, 2, 300]);
    }
}
