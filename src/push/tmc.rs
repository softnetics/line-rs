//! `TMoreCompactProtocol` reader (`thrift/readwrite/tmc.ts`, from CHRLINE).
//!
//! LINE answers talk `sync` sign-on requests on the push stream in this
//! format: a Huffman-coded sequence of field types, a table of mids and a
//! body whose structs carry field-id bitmaps instead of field headers. The
//! output is a `_result` struct as a [`TValue`], ready for schema decoding.

use crate::error::{Error, Result};
use crate::thrift::{TType, TValue};

fn err(msg: impl Into<String>) -> Error {
    Error::Thrift(format!("TMoreCompact: {}", msg.into()))
}

/// Huffman codes of the type ids (bit strings, MSB first).
const HUFFMAN: [(&str, u8); 13] = [
    ("1011", 2),
    ("10101001", 3),
    ("10101000", 4),
    ("1010111", 6),
    ("01", 8),
    ("00", 10),
    ("10100", 11),
    ("1101", 12),
    ("1010110", 13),
    ("1010101", 14),
    ("1100", 15),
    ("111", 16),
    ("100", 17),
];

fn huffman_tree() -> [u8; 512] {
    let mut tree = [0u8; 512];
    for (pattern, ty) in HUFFMAN {
        let mut idx = 0usize;
        for b in pattern.bytes() {
            idx = (idx << 1) + if b == b'0' { 1 } else { 2 };
        }
        tree[idx] = ty;
    }
    tree
}

/// Decode a TMoreCompact message into its result struct (field `0` success
/// or `1` exception).
pub fn read_result(data: &[u8]) -> Result<TValue> {
    let mut r = Reader { data, pos: 3, types: Vec::new(), next_type: 0, strings: Vec::new(), last_string_id: 0 };
    r.parse()
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    types: Vec<u8>,
    next_type: usize,
    strings: Vec<String>,
    last_string_id: i64,
}

impl Reader<'_> {
    fn parse(&mut self) -> Result<TValue> {
        if self.data.len() == 4 {
            return Err(err(format!("Invalid data: {} (code: 20)", hex::encode(self.data))));
        }
        self.read_type_sequence()?;
        let table = self.varint()? as usize;
        for _ in 0..table {
            let raw = self.take(17)?;
            let mid = format!("{}{}", raw[0] as char, hex::encode(&raw[1..]));
            self.strings.push(mid);
        }
        self.read_body()
    }

    fn read_type_sequence(&mut self) -> Result<()> {
        let len = self.varint()? as usize;
        let header = self.take(len)?.to_vec();
        let tree = huffman_tree();
        let mut node = 0usize;
        for byte in header {
            for bit in (0..8).rev() {
                let idx = (node << 1) + if byte & (1 << bit) == 0 { 1 } else { 2 };
                match tree.get(idx) {
                    Some(&0) => node = idx,
                    Some(&ty) => {
                        self.types.push(ty);
                        node = 0;
                    }
                    None => return Err(err("bad type header")),
                }
            }
        }
        Ok(())
    }

    fn read_body(&mut self) -> Result<TValue> {
        let bits = self.varint()?;
        match bits {
            0 => Ok(TValue::Struct(Vec::new())),
            1 | 2 => {
                let fid = field_ids(bits)[0];
                let ty = self.next_type()?;
                let v = self.read_value(ty)?;
                Ok(TValue::Struct(vec![(fid, v)]))
            }
            other => Err(err(format!("recv fid `{other}`, expected `1`"))),
        }
    }

    fn next_type(&mut self) -> Result<u8> {
        let t = self.types.get(self.next_type).copied().ok_or_else(|| err("type sequence exhausted"))?;
        self.next_type += 1;
        Ok(t)
    }

    fn take(&mut self, n: usize) -> Result<&[u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.data.len()).ok_or_else(|| err("unexpected end"))?;
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn varint(&mut self) -> Result<u64> {
        let mut out = 0u64;
        let mut shift = 0u32;
        loop {
            let b = self.byte()?;
            if shift < 64 {
                out |= u64::from(b & 0x7f) << shift;
            }
            if b & 0x80 == 0 {
                return Ok(out);
            }
            shift += 7;
        }
    }

    fn zigzag(&mut self) -> Result<i64> {
        let n = self.varint()?;
        Ok((n >> 1) as i64 ^ -((n & 1) as i64))
    }

    fn read_value(&mut self, ty: u8) -> Result<TValue> {
        Ok(match ty {
            2 => TValue::Bool(self.varint()? != 0),
            3 => TValue::Byte(self.byte()? as i8),
            4 => TValue::Double(f64::from_le_bytes(self.take(8)?.try_into().unwrap())),
            8 => TValue::I32(self.zigzag()? as i32),
            10 => TValue::I64(self.zigzag()?),
            11 => {
                let n = self.varint()? as usize;
                TValue::Binary(self.take(n)?.to_vec())
            }
            12 => {
                let bits = self.varint()?;
                let mut fields = Vec::new();
                for fid in field_ids(bits) {
                    let t = self.next_type()?;
                    fields.push((fid, self.read_value(t)?));
                }
                TValue::Struct(fields)
            }
            13 => {
                let size = self.varint()?;
                if size == 0 {
                    return Ok(TValue::Map(TType::Stop, TType::Stop, Vec::new()));
                }
                let types = self.byte()?;
                let (kt, vt) = (compact_to_ttype(types >> 4)?, compact_to_ttype(types & 0x0f)?);
                let mut entries = Vec::new();
                for _ in 0..size {
                    let k = self.read_value(kt as u8)?;
                    let v = self.read_value(vt as u8)?;
                    entries.push((k, v));
                }
                TValue::Map(kt, vt, entries)
            }
            14 | 15 => {
                let st = self.byte()?;
                let mut count = u64::from(st >> 4);
                let et = compact_to_ttype(st & 0x0f)?;
                if count == 15 {
                    count = self.varint()?;
                }
                let mut items = Vec::new();
                for _ in 0..count {
                    items.push(self.read_value(et as u8)?);
                }
                if ty == 14 { TValue::Set(et, items) } else { TValue::List(et, items) }
            }
            // String given as a zigzag delta of the previous numeric string id.
            16 => {
                let delta = self.zigzag()?;
                self.last_string_id = self.last_string_id.wrapping_add(delta);
                TValue::string(self.last_string_id.to_string())
            }
            // String from the mid table.
            17 => {
                let idx = self.varint()? as usize;
                let s = self.strings.get(idx).ok_or_else(|| err(format!("mid not found: {idx}")))?;
                TValue::string(s.clone())
            }
            other => return Err(err(format!("can't read type: {other}"))),
        })
    }
}

/// Field ids set in a bitmap (bit `n` → field `n`).
fn field_ids(bits: u64) -> Vec<i16> {
    (0..64).filter(|i| bits & (1u64 << i) != 0).map(|i| i as i16).collect()
}

fn compact_to_ttype(c: u8) -> Result<TType> {
    Ok(match c {
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
        _ => return Err(err(format!("Invalid type: {c}"))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thrift::{decode_struct, Schema};

    #[test]
    fn decodes_full_sync_response() {
        // types: struct(1101) struct(1101) i64(00) + padding
        let data = [0x82, 0x21, 0x01, 0x02, 0xdd, 0x00, 0x00, 0x01, 0x04, 0x04, 0x0a];
        let tv = read_result(&data).unwrap();
        assert_eq!(
            tv,
            TValue::Struct(vec![(0, TValue::Struct(vec![(2, TValue::Struct(vec![(2, TValue::I64(5))]))]))])
        );
        let v = decode_struct(Schema::line(), Some("sync_result"), &tv);
        assert_eq!(v["success"]["fullSyncResponse"]["nextRevision"].as_i64(), Some(5));
    }

    #[test]
    fn decodes_string_table_and_id_deltas() {
        // types: struct(1101) mid(100) id(111) id(111) i32(01) + padding(0)
        // bits: 1101 100 111 111 01 0 -> 11011001 11111010
        let mut data = vec![0x82, 0x21, 0x01, 0x02, 0xd9, 0xfa, 0x01, b'u'];
        data.extend([0xab; 16]);
        // body: field 0 → struct with fields 1,2,3,4 (bitmap 0b11110)
        data.extend([0x01, 0x1e, 0x00, 0x14, 0x03, 0x07]);
        let tv = read_result(&data).unwrap();
        let s = tv.field(0).unwrap();
        assert_eq!(s.field(1).and_then(TValue::as_str), Some(format!("u{}", "ab".repeat(16)).as_str()));
        assert_eq!(s.field(2).and_then(TValue::as_str), Some("10"));
        assert_eq!(s.field(3).and_then(TValue::as_str), Some("8"));
        assert_eq!(s.field(4), Some(&TValue::I32(-4)));
    }

    #[test]
    fn decodes_lists_and_maps() {
        // types: struct(1101) list(1100) map(1010110) + padding(0)
        // bits: 1101 1100 1010110 0 -> 11011100 10101100
        let data = [
            0x82, 0x21, 0x01, 0x02, 0xdc, 0xac, 0x00, 0x01, 0x06, // struct, fields 1,2
            0x25, 0x02, 0x04, // list<i32> [1, 2]
            0x01, 0x68, 0x02, 0x01, b'x', // map<i64,string> {1: "x"}
        ];
        let tv = read_result(&data).unwrap();
        let s = tv.field(0).unwrap();
        assert_eq!(s.field(1), Some(&TValue::List(TType::I32, vec![TValue::I32(1), TValue::I32(2)])));
        assert_eq!(
            s.field(2),
            Some(&TValue::Map(TType::I64, TType::String, vec![(TValue::I64(1), TValue::string("x"))]))
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(read_result(&[0, 0, 0, 0]).is_err());
        assert!(read_result(&[0x82, 0x21, 0x01, 0x05]).is_err());
        assert!(read_result(&[0x82, 0x21, 0x01, 0x00, 0x00, 0x08]).is_err());
    }
}
