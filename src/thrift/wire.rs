//! Wire-level Thrift model and the two protocols LINE speaks:
//! `TBinaryProtocol` (protocol type 3) and `TCompactProtocol` (protocol type 4).
//!
//! [`TValue`] is the Rust counterpart of linejs' `NestedArray`: every value
//! carries its Thrift type so it can be encoded without a schema.

use crate::error::{Error, Result};

/// Thrift wire type ids (`thrift.Thrift.Type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TType {
    Stop = 0,
    Void = 1,
    Bool = 2,
    Byte = 3,
    Double = 4,
    I16 = 6,
    I32 = 8,
    I64 = 10,
    String = 11,
    Struct = 12,
    Map = 13,
    Set = 14,
    List = 15,
}

impl TType {
    pub fn from_u8(v: u8) -> Result<Self> {
        Ok(match v {
            0 => TType::Stop,
            1 => TType::Void,
            2 => TType::Bool,
            3 => TType::Byte,
            4 => TType::Double,
            6 => TType::I16,
            8 => TType::I32,
            10 => TType::I64,
            11 => TType::String,
            12 => TType::Struct,
            13 => TType::Map,
            14 => TType::Set,
            15 => TType::List,
            _ => return Err(Error::Thrift(format!("unknown thrift type {v}"))),
        })
    }
}

/// Protocol selector. The numeric values match linejs' `ProtocolKey`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Protocol {
    /// `TBinaryProtocol` (linejs protocol `3`).
    Binary = 3,
    /// `TCompactProtocol` (linejs protocol `4`).
    Compact = 4,
}

/// A typed Thrift value as it appears on the wire.
#[derive(Debug, Clone, PartialEq)]
pub enum TValue {
    Bool(bool),
    Byte(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    Double(f64),
    /// Thrift `string`/`binary` (type 11). Raw bytes; may or may not be UTF-8.
    Binary(Vec<u8>),
    /// Fields in write order, keyed by field id.
    Struct(Vec<(i16, TValue)>),
    List(TType, Vec<TValue>),
    Set(TType, Vec<TValue>),
    Map(TType, TType, Vec<(TValue, TValue)>),
}

impl TValue {
    pub fn ttype(&self) -> TType {
        match self {
            TValue::Bool(_) => TType::Bool,
            TValue::Byte(_) => TType::Byte,
            TValue::I16(_) => TType::I16,
            TValue::I32(_) => TType::I32,
            TValue::I64(_) => TType::I64,
            TValue::Double(_) => TType::Double,
            TValue::Binary(_) => TType::String,
            TValue::Struct(_) => TType::Struct,
            TValue::List(..) => TType::List,
            TValue::Set(..) => TType::Set,
            TValue::Map(..) => TType::Map,
        }
    }

    pub fn string(s: impl Into<String>) -> Self {
        TValue::Binary(s.into().into_bytes())
    }

    /// Look up a field of a struct by id.
    pub fn field(&self, fid: i16) -> Option<&TValue> {
        match self {
            TValue::Struct(fields) => fields.iter().find(|(id, _)| *id == fid).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            TValue::Binary(b) => std::str::from_utf8(b).ok(),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            TValue::Byte(v) => Some(*v as i64),
            TValue::I16(v) => Some(*v as i64),
            TValue::I32(v) => Some(*v as i64),
            TValue::I64(v) => Some(*v),
            _ => None,
        }
    }
}

/// A decoded Thrift message.
#[derive(Debug, Clone)]
pub struct TMessage {
    pub name: String,
    pub mtype: u8,
    pub seqid: i32,
    /// The message body (the `_args` / `_result` struct).
    pub body: TValue,
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// Encode a request message the way linejs' `writeThrift` does: a hand-made
/// header followed by the args struct and a trailing zero byte.
pub fn write_message(name: &str, args: &[(i16, TValue)], protocol: Protocol) -> Result<Vec<u8>> {
    let name_bytes = name.as_bytes();
    if name_bytes.len() > 0xff {
        return Err(Error::Thrift("method name too long".into()));
    }
    let mut out = Vec::with_capacity(64);
    match protocol {
        Protocol::Binary => {
            out.extend_from_slice(&[0x80, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, name_bytes.len() as u8]);
            out.extend_from_slice(name_bytes);
            out.extend_from_slice(&[0, 0, 0, 0]);
        }
        Protocol::Compact => {
            out.extend_from_slice(&[0x82, 0x21, 0x00]);
            write_varint(&mut out, name_bytes.len() as u64);
            out.extend_from_slice(name_bytes);
        }
    }
    // linejs skips the struct entirely when there are no fields and then
    // appends a single STOP byte; otherwise the struct carries its own STOP
    // and one more zero byte follows. Mirror that byte-for-byte.
    if !args.is_empty() {
        let mut w = Writer::new(protocol);
        w.write_struct(args)?;
        out.extend_from_slice(&w.buf);
    }
    out.push(0);
    Ok(out)
}

/// Encode a bare struct (no message header).
pub fn write_struct(fields: &[(i16, TValue)], protocol: Protocol) -> Result<Vec<u8>> {
    let mut w = Writer::new(protocol);
    w.write_struct(fields)?;
    Ok(w.buf)
}

fn write_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        if v & !0x7f == 0 {
            out.push(v as u8);
            return;
        }
        out.push(((v & 0x7f) | 0x80) as u8);
        v >>= 7;
    }
}

fn zigzag32(v: i32) -> u64 {
    ((v << 1) ^ (v >> 31)) as u32 as u64
}

fn zigzag64(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn compact_type(t: TType) -> u8 {
    match t {
        TType::Stop => 0,
        TType::Bool => 1,
        TType::Byte => 3,
        TType::I16 => 4,
        TType::I32 => 5,
        TType::I64 => 6,
        TType::Double => 7,
        TType::String => 8,
        TType::List => 9,
        TType::Set => 10,
        TType::Map => 11,
        TType::Struct => 12,
        TType::Void => 0,
    }
}

fn from_compact_type(t: u8) -> Result<TType> {
    Ok(match t {
        0 => TType::Stop,
        1 | 2 => TType::Bool,
        3 => TType::Byte,
        4 => TType::I16,
        5 => TType::I32,
        6 => TType::I64,
        7 => TType::Double,
        8 => TType::String,
        9 => TType::List,
        10 => TType::Set,
        11 => TType::Map,
        12 => TType::Struct,
        _ => return Err(Error::Thrift(format!("unknown compact type {t}"))),
    })
}

struct Writer {
    protocol: Protocol,
    buf: Vec<u8>,
    last_fid: Vec<i16>,
}

impl Writer {
    fn new(protocol: Protocol) -> Self {
        Self { protocol, buf: Vec::new(), last_fid: vec![0] }
    }

    fn write_struct(&mut self, fields: &[(i16, TValue)]) -> Result<()> {
        self.last_fid.push(0);
        for (fid, value) in fields {
            self.write_field(*fid, value)?;
        }
        self.buf.push(0); // STOP is 0 in both protocols
        self.last_fid.pop();
        Ok(())
    }

    fn write_field(&mut self, fid: i16, value: &TValue) -> Result<()> {
        let ttype = value.ttype();
        match self.protocol {
            Protocol::Binary => {
                self.buf.push(ttype as u8);
                self.buf.extend_from_slice(&fid.to_be_bytes());
                self.write_value(value)
            }
            Protocol::Compact => {
                let tid = match value {
                    // compact encodes bool fields inside the header nibble
                    TValue::Bool(b) => {
                        if *b {
                            1
                        } else {
                            2
                        }
                    }
                    _ => compact_type(ttype),
                };
                let last = *self.last_fid.last().unwrap();
                let delta = fid as i32 - last as i32;
                if delta > 0 && delta <= 15 {
                    self.buf.push(((delta as u8) << 4) | tid);
                } else {
                    self.buf.push(tid);
                    write_varint(&mut self.buf, zigzag32(fid as i32));
                }
                *self.last_fid.last_mut().unwrap() = fid;
                if matches!(value, TValue::Bool(_)) {
                    Ok(())
                } else {
                    self.write_value(value)
                }
            }
        }
    }

    fn write_value(&mut self, value: &TValue) -> Result<()> {
        let compact = self.protocol == Protocol::Compact;
        match value {
            TValue::Bool(b) => {
                if compact {
                    self.buf.push(if *b { 1 } else { 2 });
                } else {
                    self.buf.push(*b as u8);
                }
            }
            TValue::Byte(v) => self.buf.push(*v as u8),
            TValue::I16(v) => {
                if compact {
                    write_varint(&mut self.buf, zigzag32(*v as i32));
                } else {
                    self.buf.extend_from_slice(&v.to_be_bytes());
                }
            }
            TValue::I32(v) => {
                if compact {
                    write_varint(&mut self.buf, zigzag32(*v));
                } else {
                    self.buf.extend_from_slice(&v.to_be_bytes());
                }
            }
            TValue::I64(v) => {
                if compact {
                    write_varint(&mut self.buf, zigzag64(*v));
                } else {
                    self.buf.extend_from_slice(&v.to_be_bytes());
                }
            }
            TValue::Double(v) => {
                if compact {
                    self.buf.extend_from_slice(&v.to_le_bytes());
                } else {
                    self.buf.extend_from_slice(&v.to_be_bytes());
                }
            }
            TValue::Binary(b) => {
                if compact {
                    write_varint(&mut self.buf, b.len() as u64);
                } else {
                    self.buf.extend_from_slice(&(b.len() as i32).to_be_bytes());
                }
                self.buf.extend_from_slice(b);
            }
            TValue::Struct(fields) => self.write_struct(fields)?,
            TValue::List(et, items) | TValue::Set(et, items) => {
                if compact {
                    let ct = compact_type(*et);
                    if items.len() < 15 {
                        self.buf.push(((items.len() as u8) << 4) | ct);
                    } else {
                        self.buf.push(0xf0 | ct);
                        write_varint(&mut self.buf, items.len() as u64);
                    }
                } else {
                    self.buf.push(*et as u8);
                    self.buf.extend_from_slice(&(items.len() as i32).to_be_bytes());
                }
                for item in items {
                    self.write_value(item)?;
                }
            }
            TValue::Map(kt, vt, entries) => {
                if compact {
                    write_varint(&mut self.buf, entries.len() as u64);
                    if !entries.is_empty() {
                        self.buf.push((compact_type(*kt) << 4) | compact_type(*vt));
                    }
                } else {
                    self.buf.push(*kt as u8);
                    self.buf.push(*vt as u8);
                    self.buf.extend_from_slice(&(entries.len() as i32).to_be_bytes());
                }
                for (k, v) in entries {
                    self.write_value(k)?;
                    self.write_value(v)?;
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Decode a full message (header + body struct).
pub fn read_message(data: &[u8], protocol: Protocol) -> Result<TMessage> {
    let mut r = Reader::new(data, protocol);
    let (name, mtype, seqid) = r.read_message_begin()?;
    let body = r.read_struct()?;
    Ok(TMessage { name, mtype, seqid, body })
}

/// Decode a bare struct (no message header).
pub fn read_struct(data: &[u8], protocol: Protocol) -> Result<TValue> {
    Reader::new(data, protocol).read_struct()
}

const MAX_DEPTH: usize = 64;

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    protocol: Protocol,
    last_fid: Vec<i16>,
    pending_bool: Option<bool>,
    depth: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8], protocol: Protocol) -> Self {
        Self { data, pos: 0, protocol, last_fid: vec![0], pending_bool: None, depth: 0 }
    }

    fn eof() -> Error {
        Error::Thrift("unexpected end of thrift data".into())
    }

    fn byte(&mut self) -> Result<u8> {
        let b = *self.data.get(self.pos).ok_or_else(Self::eof)?;
        self.pos += 1;
        Ok(b)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or_else(Self::eof)?;
        let s = self.data.get(self.pos..end).ok_or_else(Self::eof)?;
        self.pos = end;
        Ok(s)
    }

    fn varint(&mut self) -> Result<u64> {
        let mut result = 0u64;
        let mut shift = 0;
        loop {
            let b = self.byte()?;
            result |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
            if shift >= 64 {
                return Err(Error::Thrift("varint too long".into()));
            }
        }
    }

    fn zz32(&mut self) -> Result<i32> {
        let v = self.varint()? as u32;
        Ok(((v >> 1) as i32) ^ -((v & 1) as i32))
    }

    fn zz64(&mut self) -> Result<i64> {
        let v = self.varint()?;
        Ok(((v >> 1) as i64) ^ -((v & 1) as i64))
    }

    fn be_i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn read_message_begin(&mut self) -> Result<(String, u8, i32)> {
        match self.protocol {
            Protocol::Compact => {
                let pid = self.byte()?;
                if pid != 0x82 {
                    return Err(Error::Thrift(format!("bad compact protocol id {pid:#x}")));
                }
                let vt = self.byte()?;
                let mtype = (vt >> 5) & 0x07;
                let seqid = self.varint()? as i32;
                let len = self.varint()? as usize;
                let name = String::from_utf8_lossy(self.take(len)?).into_owned();
                Ok((name, mtype, seqid))
            }
            Protocol::Binary => {
                let first = self.be_i32()?;
                if first < 0 {
                    let version = (first as u32) & 0xffff_0000;
                    if version != 0x8001_0000 {
                        return Err(Error::Thrift("bad binary protocol version".into()));
                    }
                    let mtype = (first & 0xff) as u8;
                    let len = self.be_i32()? as usize;
                    let name = String::from_utf8_lossy(self.take(len)?).into_owned();
                    let seqid = self.be_i32()?;
                    Ok((name, mtype, seqid))
                } else {
                    let name = String::from_utf8_lossy(self.take(first as usize)?).into_owned();
                    let mtype = self.byte()?;
                    let seqid = self.be_i32()?;
                    Ok((name, mtype, seqid))
                }
            }
        }
    }

    fn read_struct(&mut self) -> Result<TValue> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(Error::Thrift("thrift nesting too deep".into()));
        }
        self.last_fid.push(0);
        let mut fields = Vec::new();
        loop {
            // A truncated tail (no STOP) is tolerated like the JS reader does
            // for messages that end right after the last field.
            if self.pos >= self.data.len() {
                break;
            }
            let (ttype, fid) = match self.protocol {
                Protocol::Binary => {
                    let t = self.byte()?;
                    if t == 0 {
                        break;
                    }
                    let fid = i16::from_be_bytes(self.take(2)?.try_into().unwrap());
                    (TType::from_u8(t)?, fid)
                }
                Protocol::Compact => {
                    let b = self.byte()?;
                    if b == 0 {
                        break;
                    }
                    let ct = b & 0x0f;
                    let delta = b >> 4;
                    let fid = if delta == 0 {
                        self.zz32()? as i16
                    } else {
                        self.last_fid.last().unwrap().wrapping_add(delta as i16)
                    };
                    *self.last_fid.last_mut().unwrap() = fid;
                    if ct == 1 || ct == 2 {
                        self.pending_bool = Some(ct == 1);
                    }
                    (from_compact_type(ct)?, fid)
                }
            };
            let value = self.read_value(ttype)?;
            fields.push((fid, value));
        }
        self.last_fid.pop();
        self.depth -= 1;
        Ok(TValue::Struct(fields))
    }

    fn read_value(&mut self, ttype: TType) -> Result<TValue> {
        let compact = self.protocol == Protocol::Compact;
        Ok(match ttype {
            TType::Bool => {
                if let Some(b) = self.pending_bool.take() {
                    TValue::Bool(b)
                } else if compact {
                    TValue::Bool(self.byte()? == 1)
                } else {
                    TValue::Bool(self.byte()? != 0)
                }
            }
            TType::Byte => TValue::Byte(self.byte()? as i8),
            TType::I16 => {
                if compact {
                    TValue::I16(self.zz32()? as i16)
                } else {
                    TValue::I16(i16::from_be_bytes(self.take(2)?.try_into().unwrap()))
                }
            }
            TType::I32 => {
                if compact {
                    TValue::I32(self.zz32()?)
                } else {
                    TValue::I32(self.be_i32()?)
                }
            }
            TType::I64 => {
                if compact {
                    TValue::I64(self.zz64()?)
                } else {
                    TValue::I64(i64::from_be_bytes(self.take(8)?.try_into().unwrap()))
                }
            }
            TType::Double => {
                let b: [u8; 8] = self.take(8)?.try_into().unwrap();
                TValue::Double(if compact { f64::from_le_bytes(b) } else { f64::from_be_bytes(b) })
            }
            TType::String => {
                let len = if compact { self.varint()? as usize } else { self.be_i32()? as usize };
                TValue::Binary(self.take(len)?.to_vec())
            }
            TType::Struct => self.read_struct()?,
            TType::List | TType::Set => {
                let (et, size) = if compact {
                    let h = self.byte()?;
                    let et = from_compact_type(h & 0x0f)?;
                    let mut size = (h >> 4) as usize;
                    if size == 15 {
                        size = self.varint()? as usize;
                    }
                    (et, size)
                } else {
                    let et = TType::from_u8(self.byte()?)?;
                    (et, self.be_i32()?.max(0) as usize)
                };
                self.check_size(size)?;
                let mut items = Vec::with_capacity(size.min(4096));
                for _ in 0..size {
                    items.push(self.read_elem(et)?);
                }
                if ttype == TType::List { TValue::List(et, items) } else { TValue::Set(et, items) }
            }
            TType::Map => {
                let (kt, vt, size) = if compact {
                    let size = self.varint()? as usize;
                    if size == 0 {
                        (TType::Stop, TType::Stop, 0)
                    } else {
                        let kv = self.byte()?;
                        (from_compact_type(kv >> 4)?, from_compact_type(kv & 0x0f)?, size)
                    }
                } else {
                    let kt = TType::from_u8(self.byte()?)?;
                    let vt = TType::from_u8(self.byte()?)?;
                    (kt, vt, self.be_i32()?.max(0) as usize)
                };
                self.check_size(size)?;
                let mut entries = Vec::with_capacity(size.min(4096));
                for _ in 0..size {
                    let k = self.read_elem(kt)?;
                    let v = self.read_elem(vt)?;
                    entries.push((k, v));
                }
                TValue::Map(kt, vt, entries)
            }
            TType::Stop | TType::Void => {
                return Err(Error::Thrift(format!("cannot read value of type {ttype:?}")));
            }
        })
    }

    /// Container elements: compact bools are full bytes here.
    fn read_elem(&mut self, t: TType) -> Result<TValue> {
        if t == TType::Bool {
            let b = self.byte()?;
            return Ok(TValue::Bool(if self.protocol == Protocol::Compact { b == 1 } else { b != 0 }));
        }
        self.read_value(t)
    }

    fn check_size(&self, size: usize) -> Result<()> {
        if size > self.data.len().saturating_sub(self.pos) {
            return Err(Error::Thrift(format!("container size {size} exceeds remaining data")));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<(i16, TValue)> {
        vec![
            (1, TValue::I32(-5)),
            (2, TValue::string("hello")),
            (3, TValue::Bool(true)),
            (4, TValue::I64(618946633287860670)),
            (20, TValue::Double(1.5)),
            (
                21,
                TValue::Struct(vec![
                    (1, TValue::Bool(false)),
                    (2, TValue::List(TType::String, vec![TValue::string("a"), TValue::string("b")])),
                ]),
            ),
            (
                22,
                TValue::Map(TType::String, TType::I32, vec![(TValue::string("k"), TValue::I32(7))]),
            ),
            (23, TValue::List(TType::Bool, vec![TValue::Bool(true), TValue::Bool(false)])),
            (-1, TValue::I64(-1)),
        ]
    }

    #[test]
    fn roundtrip_both_protocols() {
        for p in [Protocol::Binary, Protocol::Compact] {
            let enc = write_struct(&sample(), p).unwrap();
            let dec = read_struct(&enc, p).unwrap();
            assert_eq!(dec, TValue::Struct(sample()), "{p:?}");
        }
    }

    #[test]
    fn message_header_matches_linejs() {
        let m = write_message("getProfile", &[], Protocol::Compact).unwrap();
        assert_eq!(&m[..4], &[0x82, 0x21, 0x00, 10]);
        assert_eq!(*m.last().unwrap(), 0);
        let m = write_message("sync", &[(1, TValue::I32(1))], Protocol::Binary).unwrap();
        assert_eq!(&m[..8], &[0x80, 1, 0, 1, 0, 0, 0, 4]);
        let msg = read_message(&m, Protocol::Binary).unwrap();
        assert_eq!(msg.name, "sync");
        assert_eq!(msg.body.field(1), Some(&TValue::I32(1)));
    }

    #[test]
    fn compact_message_roundtrip() {
        let m = write_message("x", &sample(), Protocol::Compact).unwrap();
        let msg = read_message(&m, Protocol::Compact).unwrap();
        assert_eq!(msg.body, TValue::Struct(sample()));
    }
}
