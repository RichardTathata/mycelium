//! An HTTP destination of the reference shape (adopter-surface X5): `POST {prefix}/effects` with
//! `Idempotency-Key` and `Content-Hash` headers sends an effect; `GET {prefix}/effects/{key}` is the
//! reconciliation probe. One connection per call, a bounded wait, **no redirect followed** — a redirect is
//! a destination the allow-list did not gate (realignment repairs R3), so this client treats one as a
//! failed answer rather than a place to go.
//!
//! This is the *mock-grade* reference, the shape a real provider adapter takes, written over the
//! standard library's TCP so the crate pulls no HTTP client in; its test harness loses responses on
//! demand, which is how the unknown path is exercised. It is not a production HTTP client:
//! `docs/operations/what-is-proven.md` lists what stands where a provider would.
//!
//! The wire: a `200` whose body is `fresh` or `replayed` is a commit; `409` with body
//! `conflict <committed-hash-hex>` is [`EffectRefusal::Conflict`]; any other status is
//! [`EffectRefusal::Failed`]; a connection that times out or closes before a status line is
//! [`EffectRefusal::DeliveryUnknown`]. A probe's `200` body is `done` or `not_done`; anything else is
//! [`ProbeAnswer::Unknown`].

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::external::{ExternalDestination, IdempotencyKey, ProbeAnswer};
use crate::{DedupOutcome, DestinationCommit, Effect, EffectRefusal};

/// The HTTP destination.
pub struct HttpDestination {
    identity: String,
    host: String,
    port: u16,
    prefix: String,
    timeout: Duration,
}

impl HttpDestination {
    /// A destination at `host:port`, its routes under `prefix` (may be empty), every call bounded by
    /// `timeout`. `identity` is what every receipt names, and `host` is what the egress gate reads.
    pub fn new(identity: impl Into<String>, host: impl Into<String>, port: u16, prefix: impl Into<String>, timeout: Duration) -> Self {
        Self { identity: identity.into(), host: host.into(), port, prefix: prefix.into(), timeout }
    }

    fn request(&self, head: &str, body: &[u8]) -> Result<(u16, Vec<u8>), EffectRefusal> {
        let addr = (self.host.as_str(), self.port)
            .to_socket_addrs()
            .map_err(|e| EffectRefusal::Failed(format!("{}:{} does not resolve: {e}", self.host, self.port)))?
            .next()
            .ok_or_else(|| EffectRefusal::Failed(format!("{}:{} resolves to nothing", self.host, self.port)))?;
        let mut stream = TcpStream::connect_timeout(&addr, self.timeout).map_err(|e| EffectRefusal::Failed(format!("connect: {e}")))?;
        stream.set_read_timeout(Some(self.timeout)).map_err(|e| EffectRefusal::Failed(e.to_string()))?;
        stream.set_write_timeout(Some(self.timeout)).map_err(|e| EffectRefusal::Failed(e.to_string()))?;
        // A write that fails leaves the destination without a complete request, so nothing acts on it: *failed*,
        // not unknown. (A late `EPIPE` after a fully written small request is the residual: the kernel reported
        // the write done, the peer closed after reading it; the next read answers *unknown*, as it should.)
        if let Err(e) = stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(body)) {
            return Err(EffectRefusal::Failed(format!("write: {e}")));
        }
        // Written: from here the destination may act, so a failure to read an answer is *unknown*.
        let mut raw = Vec::new();
        if stream.read_to_end(&mut raw).is_err() && raw.is_empty() {
            return Err(EffectRefusal::DeliveryUnknown);
        }
        let text = String::from_utf8_lossy(&raw).into_owned();
        let Some((head, body)) = text.split_once("\r\n\r\n") else {
            return Err(EffectRefusal::DeliveryUnknown);
        };
        let status: u16 = head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or(EffectRefusal::DeliveryUnknown)?;
        Ok((status, body.trim().as_bytes().to_vec()))
    }
}

/// A key goes into a header and a request line: a control character or a space in it would be header
/// injection or a broken request, so it is refused before anything is written.
fn key_is_wire_safe(key: &IdempotencyKey) -> Result<(), EffectRefusal> {
    let text = key.to_string();
    if text.chars().any(|c| c.is_control() || c == ' ') {
        return Err(EffectRefusal::Failed(format!("the idempotency key {text:?} carries a control character or a space; refused before the request")));
    }
    Ok(())
}

impl ExternalDestination for HttpDestination {
    fn identity(&self) -> &str {
        &self.identity
    }
    fn dedup_scope(&self) -> &str {
        &self.identity
    }
    fn host(&self) -> Option<&str> {
        Some(&self.host)
    }

    fn send(&self, key: &IdempotencyKey, effect: &Effect) -> Result<DestinationCommit, EffectRefusal> {
        key_is_wire_safe(key)?;
        let head = format!(
            "POST {}/effects HTTP/1.1\r\nHost: {}\r\nIdempotency-Key: {key}\r\nContent-Hash: {:016x}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.prefix,
            self.host,
            effect.content_hash,
            effect.payload.len()
        );
        let (status, body) = self.request(&head, &effect.payload)?;
        let body = String::from_utf8_lossy(&body).into_owned();
        match status {
            200 => Ok(DestinationCommit::new(&self.identity, if body == "replayed" { DedupOutcome::Replayed } else { DedupOutcome::Fresh })),
            409 => Err(EffectRefusal::Conflict {
                operation_id: effect.operation_id.clone(),
                committed_hash: body.strip_prefix("conflict ").and_then(|h| u64::from_str_radix(h.trim(), 16).ok()).unwrap_or(0),
                presented_hash: effect.content_hash,
            }),
            // A redirect is not followed: the allow-list gated this host, not the next one.
            300..=399 => Err(EffectRefusal::Failed(format!("the destination redirected ({status}); a redirect is not followed"))),
            _ => Err(EffectRefusal::Failed(format!("the destination answered {status}: {body}"))),
        }
    }

    fn observe(&self, key: &IdempotencyKey) -> ProbeAnswer {
        if key_is_wire_safe(key).is_err() {
            return ProbeAnswer::Unknown;
        }
        let head = format!("GET {}/effects/{key} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n", self.prefix, self.host);
        match self.request(&head, b"") {
            Ok((200, body)) => match body.as_slice() {
                b"done" => ProbeAnswer::Done,
                b"not_done" => ProbeAnswer::NotDone,
                _ => ProbeAnswer::Unknown,
            },
            _ => ProbeAnswer::Unknown,
        }
    }
}
