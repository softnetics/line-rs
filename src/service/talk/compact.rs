//! Compact message protocol (`/CA5` plain, `/ECA5` E2EE): a terse binary
//! encoding LINE clients use to send text messages without a full thrift
//! `sendMessage` envelope. Port of linejs `service/talk/compact.ts`.

use std::fmt;

/// Endpoint for plain compact messages.
pub const COMPACT_PLAIN_MESSAGE_ENDPOINT: &str = "/CA5";
/// Endpoint for E2EE compact messages.
pub const COMPACT_E2EE_MESSAGE_ENDPOINT: &str = "/ECA5";

/// Compact message type byte for plain text.
pub const MSG_TYPE_PLAIN: u8 = 2;
/// Compact message type byte for E2EE (1:1 / group).
pub const MSG_TYPE_E2EE: u8 = 5;
/// Alternative E2EE message type byte.
pub const MSG_TYPE_E2EE_ALT: u8 = 6;

/// Decoded `/CA5` / `/ECA5` success body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactMessageResponse {
    pub sequence_id: i32,
    pub message_id: i64,
    /// Seconds since the epoch.
    pub created_time: i64,
}

/// Compact protocol failure; `code` is the server error code when the server
/// answered with a failure body (e.g. 82/99 = E2EE required, 119 = refresh).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactMessageProtocolError {
    pub message: String,
    pub code: Option<i32>,
}

impl CompactMessageProtocolError {
    fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), code: None }
    }
}

impl fmt::Display for CompactMessageProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CompactMessageProtocolError {}

impl From<CompactMessageProtocolError> for crate::Error {
    fn from(e: CompactMessageProtocolError) -> Self {
        crate::Error::internal("CompactMessageProtocolError", e.message)
    }
}

type Result<T> = std::result::Result<T, CompactMessageProtocolError>;

/// Message payload for [`pack_compact_message`].
#[derive(Debug, Clone, Copy)]
pub enum CompactArgs<'a> {
    Text(&'a str),
    /// The five E2EE chunks: salt, ciphertext, nonce, sender key id, receiver key id.
    Chunks(&'a [Vec<u8>]),
}

/// Inputs of [`pack_compact_message`].
#[derive(Debug, Clone, Copy)]
pub struct CompactMessageProtocolOptions<'a> {
    pub msg_type: u8,
    pub seq_id: i32,
    pub to: &'a str,
    pub args: CompactArgs<'a>,
    /// Trailing byte after plain text (default 0).
    pub plain_suffix: u8,
}

/// Pack a compact message body.
pub fn pack_compact_message(options: &CompactMessageProtocolOptions<'_>) -> Result<Vec<u8>> {
    let mut out = vec![options.msg_type, mid_type(options.to)?];
    write_compact_i32(&mut out, options.seq_id);
    out.extend_from_slice(&mid_to_bytes(options.to)?);
    match options.args {
        CompactArgs::Text(text) => {
            write_compact_binary(&mut out, &encode_compact_text(text));
            out.push(options.plain_suffix);
        }
        CompactArgs::Chunks(chunks) => {
            if chunks.len() < 5 {
                return Err(CompactMessageProtocolError::new("compact E2EE message requires 5 chunks"));
            }
            out.push(2);
            for chunk in &chunks[..3] {
                write_compact_binary(&mut out, chunk);
            }
            write_compact_i32(&mut out, read_signed_i32(&chunks[3])?);
            write_compact_i32(&mut out, read_signed_i32(&chunks[4])?);
        }
    }
    Ok(out)
}

/// Pack a plain text compact message (`/CA5`).
pub fn pack_compact_plain_message(seq_id: i32, to: &str, text: &str) -> Result<Vec<u8>> {
    pack_compact_message(&CompactMessageProtocolOptions {
        msg_type: MSG_TYPE_PLAIN,
        seq_id,
        to,
        args: CompactArgs::Text(text),
        plain_suffix: 0,
    })
}

/// Pack an E2EE compact message (`/ECA5`); `msg_type` is 5 (default) or 6.
pub fn pack_compact_e2ee_message(seq_id: i32, to: &str, chunks: &[Vec<u8>], msg_type: u8) -> Result<Vec<u8>> {
    pack_compact_message(&CompactMessageProtocolOptions {
        msg_type,
        seq_id,
        to,
        args: CompactArgs::Chunks(chunks),
        plain_suffix: 0,
    })
}

/// Decode a compact message response; a failure body becomes an error
/// carrying the server's code.
pub fn decode_compact_message_response(data: &[u8]) -> Result<CompactMessageResponse> {
    let mut reader = CompactReader { data, offset: 0 };
    if !reader.read_bool()? {
        let code = reader.read_i32()?;
        return Err(CompactMessageProtocolError {
            message: format!("compact message failed: error code {code}"),
            code: Some(code),
        });
    }
    let sequence_id = reader.read_i32()?;
    let message_id = reader.read_i64()?;
    let created_ms = reader.read_i64()?;
    reader.assert_done()?;
    Ok(CompactMessageResponse { sequence_id, message_id, created_time: created_ms / 1000 })
}

/// Text as BOM-prefixed UTF-8, or BOM-prefixed UTF-16LE when that is shorter.
pub fn encode_compact_text(text: &str) -> Vec<u8> {
    let mut utf8 = vec![0xef, 0xbb, 0xbf];
    utf8.extend_from_slice(text.as_bytes());
    let mut utf16 = vec![0xff, 0xfe];
    for unit in text.encode_utf16() {
        utf16.extend_from_slice(&unit.to_le_bytes());
    }
    if utf8.len() > utf16.len() { utf16 } else { utf8 }
}

/// Zigzag varint i32.
pub fn write_compact_i32(out: &mut Vec<u8>, value: i32) {
    write_varint(out, ((value << 1) ^ (value >> 31)) as u32 as u64);
}

/// Zigzag varint i64.
pub fn write_compact_i64(out: &mut Vec<u8>, value: i64) {
    write_varint(out, ((value << 1) ^ (value >> 63)) as u64);
}

fn write_compact_binary(out: &mut Vec<u8>, data: &[u8]) {
    write_varint(out, data.len() as u64);
    out.extend_from_slice(data);
}

fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        if value & !0x7f == 0 {
            out.push(value as u8);
            return;
        }
        out.push((value & 0x7f) as u8 | 0x80);
        value >>= 7;
    }
}

fn mid_type(mid: &str) -> Result<u8> {
    match mid.as_bytes().first() {
        Some(b'u') => Ok(0),
        Some(b'r') => Ok(1),
        Some(b'c') => Ok(2),
        _ => Err(CompactMessageProtocolError::new("unsupported compact message MID type")),
    }
}

fn mid_to_bytes(mid: &str) -> Result<[u8; 16]> {
    let invalid = || CompactMessageProtocolError::new("invalid compact message MID format");
    let hex_part = mid.get(1..).filter(|h| h.len() == 32).ok_or_else(invalid)?;
    let mut out = [0u8; 16];
    hex::decode_to_slice(hex_part, &mut out).map_err(|_| invalid())?;
    Ok(out)
}

fn read_signed_i32(bytes: &[u8]) -> Result<i32> {
    let arr: [u8; 4] =
        bytes.try_into().map_err(|_| CompactMessageProtocolError::new("compact E2EE key id chunk must be 4 bytes"))?;
    Ok(i32::from_be_bytes(arr))
}

struct CompactReader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl CompactReader<'_> {
    fn read_byte(&mut self) -> Result<u8> {
        let b = *self
            .data
            .get(self.offset)
            .ok_or_else(|| CompactMessageProtocolError::new("unexpected end of compact data"))?;
        self.offset += 1;
        Ok(b)
    }

    fn read_bool(&mut self) -> Result<bool> {
        match self.read_byte()? {
            1 => Ok(true),
            2 => Ok(false),
            v => Err(CompactMessageProtocolError::new(format!("invalid compact bool value {v}"))),
        }
    }

    fn read_varint(&mut self) -> Result<u128> {
        let mut shift = 0u32;
        let mut result = 0u128;
        loop {
            let byte = self.read_byte()?;
            result |= u128::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
            if shift > 70 {
                return Err(CompactMessageProtocolError::new("compact varint is too long"));
            }
        }
    }

    fn read_zigzag(&mut self) -> Result<i128> {
        let n = self.read_varint()? as i128;
        Ok((n >> 1) ^ -(n & 1))
    }

    fn read_i32(&mut self) -> Result<i32> {
        i32::try_from(self.read_zigzag()?).map_err(|_| CompactMessageProtocolError::new("compact i32 out of range"))
    }

    fn read_i64(&mut self) -> Result<i64> {
        Ok(self.read_zigzag()? as i64)
    }

    fn assert_done(&self) -> Result<()> {
        if self.offset != self.data.len() {
            return Err(CompactMessageProtocolError::new("unexpected trailing compact message response bytes"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_prefers_utf8_for_ascii() {
        assert_eq!(hex::encode(encode_compact_text("hello")), "efbbbf68656c6c6f");
    }

    #[test]
    fn text_prefers_utf16_when_shorter() {
        assert_eq!(hex::encode(encode_compact_text("あ")), "fffe4230");
    }

    #[test]
    fn plain_message_layout() {
        let mid = format!("u{}", "00".repeat(16));
        let packed = pack_compact_plain_message(0, &mid, "hi").unwrap();
        assert_eq!(hex::encode(packed), format!("0200{}{}05efbbbf686900", "00", "00".repeat(16)));
    }

    #[test]
    fn e2ee_message_layout() {
        let mid = format!("c{}", "11".repeat(16));
        let salt: Vec<u8> = (0..16).collect();
        let chunks = vec![salt.clone(), vec![0xaa, 0xbb, 0xcc], vec![0x22; 12], vec![0, 0, 0, 0x96], vec![0xff; 4]];
        let packed = pack_compact_e2ee_message(1, &mid, &chunks, MSG_TYPE_E2EE).unwrap();
        let expected =
            format!("050202{}0210{}03aabbcc0c{}ac0201", "11".repeat(16), hex::encode(&salt), "22".repeat(12));
        assert_eq!(hex::encode(packed), expected);
    }

    #[test]
    fn decodes_success_body() {
        let mut body = vec![1];
        write_compact_i32(&mut body, 7);
        write_compact_i64(&mut body, 1_234_567_890_123);
        write_compact_i64(&mut body, 1_710_000_000_000);
        assert_eq!(
            decode_compact_message_response(&body).unwrap(),
            CompactMessageResponse { sequence_id: 7, message_id: 1_234_567_890_123, created_time: 1_710_000_000 }
        );
    }

    #[test]
    fn decodes_error_code() {
        let mut body = vec![2];
        write_compact_i32(&mut body, 82);
        let err = decode_compact_message_response(&body).unwrap_err();
        assert_eq!(err.code, Some(82));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(pack_compact_plain_message(0, "s0", "x").is_err());
        assert!(pack_compact_plain_message(0, "uzz", "x").is_err());
        let mid = format!("u{}", "00".repeat(16));
        assert!(pack_compact_e2ee_message(0, &mid, &vec![Vec::new(); 4], MSG_TYPE_E2EE).is_err());
        assert!(decode_compact_message_response(&[1, 0, 0, 0, 9]).is_err());
        assert!(decode_compact_message_response(&[3]).is_err());
    }
}
