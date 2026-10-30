//! One `/PUSH` connection (`conn.ts`): a full-duplex HTTP/2 POST whose
//! request body is a channel of frames we keep writing to while the response
//! body streams frames back.

use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::{StreamExt, TryStreamExt};
use tokio::sync::{mpsc, Notify};

use super::frame::{
    request_packet, AckType, PingFrame, PushFrame, SignOnResponseFrame, FRAME_PING, FRAME_PUSH, FRAME_SIGN_ON_RESPONSE,
};
use crate::error::{Error, Result};

/// A decoded server → client packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Packet {
    /// Ping that must be acked (only `ACK_REQUIRED` pings are accepted).
    Ping(PingFrame),
    /// A complete sign-on response (continuation chunks already joined).
    SignOnResponse(SignOnResponseFrame),
    Push(PushFrame),
}

/// Splits the response byte stream into packets (`Conn.onDataReceived` /
/// `onPacketReceived`), buffering partial packets and non-final sign-on
/// response chunks.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
    not_fin: HashMap<u16, Vec<u8>>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk; returns every packet completed by it.
    pub fn feed(&mut self, data: &[u8]) -> Result<Vec<Packet>> {
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        let mut pos = 0;
        while self.buf.len() - pos >= 3 {
            let len = u16::from_be_bytes([self.buf[pos], self.buf[pos + 1]]) as usize;
            let ty = self.buf[pos + 2];
            if self.buf.len() - pos - 3 < len {
                break;
            }
            let payload = self.buf[pos + 3..pos + 3 + len].to_vec();
            pos += 3 + len;
            if let Some(p) = self.on_packet(ty, payload)? {
                out.push(p);
            }
        }
        self.buf.drain(..pos);
        Ok(out)
    }

    /// Bytes buffered while waiting for the rest of a packet.
    pub fn pending(&self) -> usize {
        self.buf.len()
    }

    fn on_packet(&mut self, ty: u8, dd: Vec<u8>) -> Result<Option<Packet>> {
        let short = || Error::internal("LegyPusherError", format!("PUSH frame type {ty} too short: {}", hex::encode(&dd)));
        match ty {
            FRAME_PING => {
                if dd.len() < 3 {
                    return Err(short());
                }
                let frame = PingFrame { ping_type: dd[0], ping_id: u16::from_be_bytes([dd[1], dd[2]]) };
                if AckType::from_u8(frame.ping_type) != Some(AckType::AckRequired) {
                    return Err(Error::internal(
                        "LegyPusherError",
                        format!("ping type not Implemented: {}", frame.ping_type),
                    ));
                }
                Ok(Some(Packet::Ping(frame)))
            }
            FRAME_SIGN_ON_RESPONSE => {
                if dd.len() < 2 {
                    return Err(short());
                }
                let req = u16::from_be_bytes([dd[0], dd[1]]);
                let request_id = req & 0x7fff;
                let is_fin = req & 0x8000 != 0;
                let chunk = &dd[2..];
                if !is_fin {
                    self.not_fin.entry(request_id).or_default().extend_from_slice(chunk);
                    return Ok(None);
                }
                let payload = match self.not_fin.remove(&request_id) {
                    Some(mut prev) => {
                        prev.extend_from_slice(chunk);
                        prev
                    }
                    None => chunk.to_vec(),
                };
                Ok(Some(Packet::SignOnResponse(SignOnResponseFrame { request_id, is_fin, payload })))
            }
            FRAME_PUSH => {
                if dd.len() < 6 {
                    return Err(short());
                }
                let frame = PushFrame {
                    push_type: dd[0],
                    service_type: dd[1],
                    push_id: i32::from_be_bytes([dd[2], dd[3], dd[4], dd[5]]),
                    payload: dd[6..].to_vec(),
                };
                match AckType::from_u8(frame.push_type) {
                    Some(AckType::None | AckType::AckRequired) => Ok(Some(Packet::Push(frame))),
                    _ => Err(Error::internal(
                        "LegyPusherError",
                        format!("push type not Implemented: {}", frame.push_type),
                    )),
                }
            }
            _ => Err(Error::internal(
                "LegyPusherError",
                format!(
                    "PUSH not Implemented: type:{ty}, payloads:{}, len:{}",
                    &hex::encode(&dd)[..(dd.len() * 2).min(30)],
                    dd.len()
                ),
            )),
        }
    }
}

/// Write half of a connection: frames are queued into the request body.
#[derive(Clone)]
pub struct ConnWriter {
    tx: mpsc::UnboundedSender<Bytes>,
    closed: Arc<Notify>,
}

impl ConnWriter {
    /// A writer plus the receiver that feeds the request body.
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<Bytes>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx, closed: Arc::new(Notify::new()) }, rx)
    }

    /// Queue raw bytes.
    pub fn write(&self, data: Vec<u8>) -> Result<()> {
        self.tx
            .send(Bytes::from(data))
            .map_err(|_| Error::internal("LegyPusherError", "push request stream is closed"))
    }

    /// Queue `payload` wrapped as a `frame_type` packet.
    pub fn write_request(&self, frame_type: u8, payload: &[u8]) -> Result<()> {
        self.write(request_packet(frame_type, payload))
    }

    /// Ask the reader to stop; the connection is torn down when it does.
    pub fn close(&self) {
        self.closed.notify_one();
    }

    pub(crate) async fn closed(&self) {
        self.closed.notified().await
    }
}

/// The response body: a stream of raw chunks.
pub type ByteStream = BoxStream<'static, Result<Bytes>>;

/// A push connection that has been requested but not necessarily answered.
pub struct Conn {
    pub writer: ConnWriter,
    /// Resolves to the response body once the server sends its headers.
    pub response: BoxFuture<'static, Result<ByteStream>>,
}

impl Conn {
    /// Assemble a connection from parts (used by tests and custom transports).
    pub fn from_parts(writer: ConnWriter, response: BoxFuture<'static, Result<ByteStream>>) -> Self {
        Self { writer, response }
    }

    /// Start `POST url` with a streaming body. Over TLS reqwest negotiates
    /// HTTP/2 through ALPN, which multiplexes the body and response streams.
    pub fn open(http: &reqwest::Client, url: &str, headers: Vec<(String, String)>) -> Self {
        let (writer, rx) = ConnWriter::channel();
        let body = futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|b| (Ok::<_, std::io::Error>(b), rx))
        });
        let mut req = http.post(url).body(reqwest::Body::wrap_stream(body));
        for (k, v) in headers {
            req = req.header(k, v);
        }
        let response = Box::pin(async move {
            let res = req.send().await?;
            let status = res.status();
            if !status.is_success() {
                let body = res.bytes().await.unwrap_or_default();
                return Err(Error::internal(
                    "LegyPusherError",
                    format!("PUSH responded with status {status}: {}", hex::encode(&body)),
                ));
            }
            Ok(res.bytes_stream().map_err(Error::from).boxed())
        });
        Self { writer, response }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ping(id: u16) -> Vec<u8> {
        let id = id.to_be_bytes();
        request_packet(FRAME_PING, &[2, id[0], id[1]])
    }

    #[test]
    fn decodes_ping() {
        let mut d = FrameDecoder::new();
        let p = d.feed(&ping(7)).unwrap();
        assert_eq!(p, vec![Packet::Ping(PingFrame { ping_type: 2, ping_id: 7 })]);
        assert_eq!(d.pending(), 0);
    }

    #[test]
    fn rejects_unsupported_ping_type() {
        let mut d = FrameDecoder::new();
        assert!(d.feed(&request_packet(FRAME_PING, &[0, 0, 1])).is_err());
    }

    #[test]
    fn buffers_partial_packets_and_splits_coalesced_ones() {
        let mut all = ping(1);
        all.extend(ping(2));
        all.extend(ping(3));
        let mut d = FrameDecoder::new();
        let first = d.feed(&all[..4]).unwrap();
        assert!(first.is_empty());
        assert_eq!(d.pending(), 4);
        let rest = d.feed(&all[4..]).unwrap();
        let ids: Vec<u16> = rest
            .iter()
            .map(|p| match p {
                Packet::Ping(f) => f.ping_id,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(ids, vec![1, 2, 3]);
        assert_eq!(d.pending(), 0);
    }

    #[test]
    fn header_split_across_chunks() {
        let p = ping(9);
        let mut d = FrameDecoder::new();
        assert!(d.feed(&p[..1]).unwrap().is_empty());
        assert!(d.feed(&p[1..2]).unwrap().is_empty());
        assert_eq!(d.feed(&p[2..]).unwrap().len(), 1);
    }

    #[test]
    fn joins_non_final_sign_on_chunks() {
        let mut d = FrameDecoder::new();
        let part = request_packet(FRAME_SIGN_ON_RESPONSE, &[0x00, 0x05, 1, 2]);
        let fin = request_packet(FRAME_SIGN_ON_RESPONSE, &[0x80, 0x05, 3]);
        assert!(d.feed(&part).unwrap().is_empty());
        let out = d.feed(&fin).unwrap();
        assert_eq!(
            out,
            vec![Packet::SignOnResponse(SignOnResponseFrame { request_id: 5, is_fin: true, payload: vec![1, 2, 3] })]
        );
        // The continuation buffer is consumed.
        let again = d.feed(&fin).unwrap();
        assert_eq!(
            again,
            vec![Packet::SignOnResponse(SignOnResponseFrame { request_id: 5, is_fin: true, payload: vec![3] })]
        );
    }

    #[test]
    fn decodes_push_frame() {
        let mut d = FrameDecoder::new();
        let raw = request_packet(FRAME_PUSH, &[2, 3, 0, 0, 1, 0, 0xde, 0xad]);
        let out = d.feed(&raw).unwrap();
        assert_eq!(
            out,
            vec![Packet::Push(PushFrame { push_type: 2, service_type: 3, push_id: 256, payload: vec![0xde, 0xad] })]
        );
    }

    #[test]
    fn rejects_unknown_types() {
        let mut d = FrameDecoder::new();
        assert!(d.feed(&request_packet(9, &[1, 2])).is_err());
        let mut d = FrameDecoder::new();
        assert!(d.feed(&request_packet(FRAME_PUSH, &[1, 3, 0, 0, 0, 1])).is_err());
    }

    #[tokio::test]
    async fn writer_queues_frames_and_reports_closed_body() {
        let (w, mut rx) = ConnWriter::channel();
        w.write_request(0, &[0, 0, 30]).unwrap();
        assert_eq!(rx.recv().await.unwrap().as_ref(), &[0, 3, 0, 0, 0, 30]);
        drop(rx);
        assert!(w.write(vec![1]).is_err());
    }

    #[tokio::test]
    async fn close_wakes_reader_even_if_called_first() {
        let (w, _rx) = ConnWriter::channel();
        w.close();
        tokio::time::timeout(std::time::Duration::from_millis(100), w.closed()).await.unwrap();
    }

    // `Conn.new reports a transport failure`: a failed connect surfaces as an
    // error from the response future rather than a panic.
    #[tokio::test]
    async fn open_reports_transport_failure() {
        let http = reqwest::Client::new();
        let conn = Conn::open(&http, "https://example.invalid/PUSH/1/subs?m=1", vec![]);
        conn.writer.write(vec![0, 3, 0, 0, 0, 30]).unwrap();
        assert!(conn.response.await.is_err());
    }
}
