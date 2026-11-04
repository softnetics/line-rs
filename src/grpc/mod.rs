//! A small gRPC-over-HTTP/2 client for LINE's LEGY gRPC services
//! (ServiceChat, PlaceSearch, Calendar).
//!
//! Unlike the Thrift talk path, these calls are plain gRPC over TLS straight
//! to the LEGY host — they do not go through the `gf.line.naver.jp/enc`
//! encrypted gateway. `reqwest` is HTTP/1.1-only here and cannot read the
//! HTTP/2 trailer that carries `grpc-status` on a successful call, so this
//! module drives `h2` directly.

use std::sync::Arc;

use bytes::{Buf, BufMut, Bytes, BytesMut};
use http::{HeaderMap, Request};
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

use crate::base::BaseClient;
use crate::error::{Error, Result};

const ERR: &str = "GrpcError";

/// A gRPC request: the full path (`/package.Service/Method`), the LINE
/// headers to send, and one request message (already protobuf-encoded).
pub struct GrpcRequest<'a> {
    pub host: &'a str,
    pub path: &'a str,
    /// Header name/value pairs (lowercase names).
    pub headers: &'a [(String, String)],
    pub body: &'a [u8],
}

fn tls_config() -> Result<Arc<ClientConfig>> {
    let mut roots = RootCertStore::empty();
    let certs = rustls_native_certs::load_native_certs();
    if certs.certs.is_empty() {
        if let Some(e) = certs.errors.into_iter().next() {
            return Err(Error::internal(ERR, format!("no system roots: {e}")));
        }
        return Err(Error::internal(ERR, "no system root certificates found"));
    }
    for cert in certs.certs {
        // Ignore individual malformed roots; the store only needs enough to
        // validate LINE's chain.
        let _ = roots.add(cert);
    }
    // Pick the ring provider explicitly: reqwest pulls in another rustls
    // CryptoProvider too, so the process-level default is ambiguous.
    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| Error::internal(ERR, format!("TLS config: {e}")))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec()];
    Ok(Arc::new(config))
}

/// Frames one gRPC message: a 1-byte "compressed" flag (0) and a big-endian
/// u32 length, then the payload.
fn frame(message: &[u8]) -> Bytes {
    let mut buf = BytesMut::with_capacity(5 + message.len());
    buf.put_u8(0);
    buf.put_u32(message.len() as u32);
    buf.put_slice(message);
    buf.freeze()
}

/// Splits a gRPC body (concatenated length-prefixed frames) into messages.
fn deframe(mut body: Bytes) -> Result<Vec<Bytes>> {
    let mut out = Vec::new();
    while !body.is_empty() {
        if body.len() < 5 {
            return Err(Error::internal(ERR, "truncated gRPC frame header"));
        }
        let compressed = body[0];
        let len = u32::from_be_bytes([body[1], body[2], body[3], body[4]]) as usize;
        if compressed != 0 {
            return Err(Error::internal(
                ERR,
                "compressed gRPC frames are not supported",
            ));
        }
        body.advance(5);
        if body.len() < len {
            return Err(Error::internal(ERR, "truncated gRPC frame body"));
        }
        out.push(body.split_to(len));
    }
    Ok(out)
}

fn grpc_status(headers: &HeaderMap, trailers: Option<&HeaderMap>) -> Result<()> {
    // The status rides on the trailers for a normal response, or on the
    // initial headers for a "trailers-only" error (no body).
    let pick = |name: &str| {
        trailers
            .and_then(|t| t.get(name))
            .or_else(|| headers.get(name))
            .and_then(|v| v.to_str().ok())
    };
    let status = pick("grpc-status").and_then(|s| s.parse::<i32>().ok());
    match status {
        Some(0) => Ok(()),
        Some(code) => {
            let message = pick("grpc-message").unwrap_or("").to_string();
            Err(Error::internal(
                ERR,
                format!(
                    "gRPC status {code}{}",
                    if message.is_empty() {
                        String::new()
                    } else {
                        format!(": {message}")
                    }
                ),
            ))
        }
        // No grpc-status anywhere: treat as a transport-level failure.
        None => Err(Error::internal(ERR, "response had no grpc-status")),
    }
}

/// Opens an HTTP/2 connection, sends one request and collects every response
/// message. Works for both unary (one message) and server-streaming (several)
/// calls; the connection is closed afterward.
pub async fn call(req: GrpcRequest<'_>) -> Result<Vec<Bytes>> {
    let config = tls_config()?;
    let connector = TlsConnector::from(config);
    let addr = (req.host, 443u16);
    let tcp = tokio::net::TcpStream::connect(addr)
        .await
        .map_err(|e| Error::internal(ERR, format!("connect to {}: {e}", req.host)))?;
    let server_name = ServerName::try_from(req.host.to_string())
        .map_err(|e| Error::internal(ERR, format!("invalid host {}: {e}", req.host)))?;
    let tls = connector
        .connect(server_name, tcp)
        .await
        .map_err(|e| Error::internal(ERR, format!("TLS handshake: {e}")))?;

    let (h2, connection) = h2::client::handshake(tls)
        .await
        .map_err(|e| Error::internal(ERR, format!("HTTP/2 handshake: {e}")))?;
    // The connection future drives I/O; it ends when the stream is done.
    let conn = tokio::spawn(async move {
        let _ = connection.await;
    });
    let mut h2 = h2
        .ready()
        .await
        .map_err(|e| Error::internal(ERR, format!("HTTP/2 not ready: {e}")))?;

    let uri = format!("https://{}{}", req.host, req.path);
    let mut builder = Request::builder().method("POST").uri(&uri);
    builder = builder.header("content-type", "application/grpc");
    builder = builder.header("te", "trailers");
    for (k, v) in req.headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    let request = builder
        .body(())
        .map_err(|e| Error::internal(ERR, format!("build request: {e}")))?;

    let (response, mut send) = h2
        .send_request(request, false)
        .map_err(|e| Error::internal(ERR, format!("send request: {e}")))?;
    send.send_data(frame(req.body), true)
        .map_err(|e| Error::internal(ERR, format!("send body: {e}")))?;

    let response = response
        .await
        .map_err(|e| Error::internal(ERR, format!("await response: {e}")))?;
    let headers = response.headers().clone();
    let mut body = response.into_body();

    let mut collected = BytesMut::new();
    while let Some(chunk) = body.data().await {
        let chunk = chunk.map_err(|e| Error::internal(ERR, format!("read body: {e}")))?;
        let len = chunk.len();
        collected.put(chunk);
        // Release flow-control window so the server keeps sending.
        let _ = body.flow_control().release_capacity(len);
    }
    let trailers = body
        .trailers()
        .await
        .map_err(|e| Error::internal(ERR, format!("read trailers: {e}")))?;

    grpc_status(&headers, trailers.as_ref())?;
    conn.abort();
    deframe(collected.freeze())
}

impl BaseClient {
    /// gRPC metadata for a LEGY call: the same identity headers as a Thrift
    /// call, split out the way the app's default gRPC interceptor sends them
    /// (not the tab-joined `x-line-application`), plus `x-line-access`.
    pub fn grpc_headers(&self) -> Vec<(String, String)> {
        let d = &self.device_details;
        let mut h = vec![
            ("user-agent".to_string(), d.user_agent()),
            (
                "x-line-application-type".to_string(),
                d.device.as_str().to_string(),
            ),
            (
                "x-line-application-version".to_string(),
                d.app_version.clone(),
            ),
            // The gRPC path sends the build phase as its own header (the
            // Thrift path folds it into x-line-application). Emulate RELEASE.
            (
                "x-line-application-phase".to_string(),
                "RELEASE".to_string(),
            ),
            ("x-line-system-name".to_string(), d.system_name.clone()),
            (
                "x-line-system-version".to_string(),
                d.system_version.clone(),
            ),
            ("x-lal".to_string(), "ja_JP".to_string()),
        ];
        if let Some(token) = self.auth_token() {
            h.push(("x-line-access".to_string(), token));
        }
        h
    }

    /// One unary gRPC call to `path` (`/package.Service/Method`) on the
    /// current LEGY host, with [`grpc_headers`](Self::grpc_headers). Returns
    /// the response message (empty for an empty response).
    pub async fn grpc_unary(&self, path: &str, body: &[u8]) -> Result<Bytes> {
        let headers = self.grpc_headers();
        let mut msgs = call(GrpcRequest {
            host: self.current_endpoint(),
            path,
            headers: &headers,
            body,
        })
        .await?;
        Ok(msgs.drain(..).next().unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_then_deframe() {
        let a = frame(b"hello");
        let b = frame(b"world!!");
        let mut joined = BytesMut::new();
        joined.put(a);
        joined.put(b);
        let msgs = deframe(joined.freeze()).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(&msgs[0][..], b"hello");
        assert_eq!(&msgs[1][..], b"world!!");
    }

    #[test]
    fn deframe_rejects_truncation() {
        assert!(deframe(Bytes::from_static(&[0, 0, 0, 0, 5, 1, 2])).is_err());
    }

    #[test]
    fn deframe_rejects_compressed() {
        assert!(deframe(Bytes::from_static(&[1, 0, 0, 0, 1, 42])).is_err());
    }

    #[test]
    fn status_ok_from_trailers() {
        let mut t = HeaderMap::new();
        t.insert("grpc-status", "0".parse().unwrap());
        assert!(grpc_status(&HeaderMap::new(), Some(&t)).is_ok());
    }

    #[test]
    fn status_error_from_headers() {
        let mut h = HeaderMap::new();
        h.insert("grpc-status", "3".parse().unwrap());
        h.insert("grpc-message", "Invalid Argument".parse().unwrap());
        let err = grpc_status(&h, None).unwrap_err();
        assert!(err.to_string().contains("Invalid Argument"));
    }

    #[test]
    fn missing_status_is_error() {
        assert!(grpc_status(&HeaderMap::new(), None).is_err());
    }
}
