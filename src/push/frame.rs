//! LEGY push frames (`connData.ts`).
//!
//! Every packet on the `/PUSH` stream is `u16 length | u8 type | payload`
//! (big-endian). Client → server types: `0` status, `2` sign-on request.
//! Server → client types: `1` ping, `3` sign-on response, `4` push.

/// Status frame (client → server).
pub const FRAME_STATUS: u8 = 0;
/// Ping frame (server → client, acked by the client).
pub const FRAME_PING: u8 = 1;
/// Sign-on request (client → server).
pub const FRAME_SIGN_ON_REQUEST: u8 = 2;
/// Sign-on response (server → client).
pub const FRAME_SIGN_ON_RESPONSE: u8 = 3;
/// Push notification (server → client).
pub const FRAME_PUSH: u8 = 4;

/// `LegyH2PingFrameType` / `LegyH2PushFrameType` (identical values).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AckType {
    None = 0,
    Ack = 1,
    AckRequired = 2,
}

impl AckType {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => AckType::None,
            1 => AckType::Ack,
            2 => AckType::AckRequired,
            _ => return None,
        })
    }
}

/// Wrap `payload` in a packet header (`LegyH2Frame.requestPacket` /
/// `ConnManager.buildRequest`). The length is truncated to 16 bits like linejs.
pub fn request_packet(frame_type: u8, payload: &[u8]) -> Vec<u8> {
    let len = payload.len();
    let mut out = Vec::with_capacity(3 + len);
    out.push((len >> 8) as u8);
    out.push(len as u8);
    out.push(frame_type);
    out.extend_from_slice(payload);
    out
}

/// The status frame sent right after connecting.
pub fn status_packet(flag: u8, ping_interval: u8) -> Vec<u8> {
    request_packet(FRAME_STATUS, &[0, flag, ping_interval])
}

/// A full sign-on request packet: `u16 id | u8 service | 0 | u16 len | req`.
pub fn sign_on_request_packet(request_id: u16, service_type: u8, request: &[u8]) -> Vec<u8> {
    let len = request.len();
    let mut header = Vec::with_capacity(6 + len);
    header.extend_from_slice(&request_id.to_be_bytes());
    header.push(service_type);
    header.push(0);
    header.push((len >> 8) as u8);
    header.push(len as u8);
    header.extend_from_slice(request);
    request_packet(FRAME_SIGN_ON_REQUEST, &header)
}

/// Bitmask of push services for the `m` query parameter (`gen_m`).
pub fn services_mask(services: &[u8]) -> u32 {
    services.iter().fold(0, |m, &s| m | 1u32.checked_shl(u32::from(s).wrapping_sub(1)).unwrap_or(0))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PingFrame {
    pub ping_type: u8,
    pub ping_id: u16,
}

impl PingFrame {
    pub fn ack_packet(&self) -> Vec<u8> {
        let id = self.ping_id.to_be_bytes();
        request_packet(FRAME_PING, &[AckType::Ack as u8, id[0], id[1]])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignOnResponseFrame {
    pub request_id: u16,
    pub is_fin: bool,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushFrame {
    pub push_type: u8,
    pub service_type: u8,
    pub push_id: i32,
    pub payload: Vec<u8>,
}

impl PushFrame {
    pub fn ack_packet(&self) -> Vec<u8> {
        let mut payload = vec![AckType::Ack as u8, self.service_type];
        payload.extend_from_slice(&self.push_id.to_be_bytes());
        request_packet(FRAME_PUSH, &payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_packet_layout() {
        assert_eq!(request_packet(2, &[9, 8, 7]), vec![0, 3, 2, 9, 8, 7]);
        let big = vec![0u8; 0x1234];
        let p = request_packet(5, &big);
        assert_eq!(&p[..3], &[0x12, 0x34, 5]);
        assert_eq!(p.len(), 3 + 0x1234);
    }

    #[test]
    fn status_packet_layout() {
        assert_eq!(status_packet(0, 30), vec![0, 3, 0, 0, 0, 30]);
    }

    #[test]
    fn sign_on_request_layout() {
        let p = sign_on_request_packet(1, 3, &[0xaa, 0xbb]);
        assert_eq!(p, vec![0, 8, 2, 0, 1, 3, 0, 0, 2, 0xaa, 0xbb]);
    }

    #[test]
    fn ping_ack() {
        let f = PingFrame { ping_type: 2, ping_id: 0x0102 };
        assert_eq!(f.ack_packet(), vec![0, 3, 1, 1, 1, 2]);
    }

    #[test]
    fn push_ack() {
        let f = PushFrame { push_type: 2, service_type: 3, push_id: -2, payload: vec![] };
        assert_eq!(f.ack_packet(), vec![0, 6, 4, 1, 3, 0xff, 0xff, 0xff, 0xfe]);
    }

    #[test]
    fn services_mask_matches_gen_m() {
        assert_eq!(services_mask(&[1, 3, 5, 6, 8, 9, 10]), 0b11_1011_0101);
        assert_eq!(services_mask(&[3, 8]), 132);
        assert_eq!(services_mask(&[3, 6, 8, 9, 10]), 0b11_1010_0100);
    }
}
