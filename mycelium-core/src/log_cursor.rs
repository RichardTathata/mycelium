//! A consumer group's position in a log stream, in the log's own order (post-360 hardening row E).
//!
//! A log entry's key is `log/{stream}/{hlc:016x}/{node}`: two nodes appending in one millisecond share
//! an HLC, and the node salt keeps both. [`KvHandle::scan_log`](crate::KvHandle::scan_log) orders a
//! stream by `(hlc, key)`, so a consumer's position must be that pair — a position that is an HLC alone
//! cannot say which of two same-HLC entries it has passed, and the consumers that kept one delivered one
//! of them and skipped the other for ever.
//!
//! **On the wire, two keys.** `clog/{stream}/{group}/offset` keeps its old value — the 16-hex HLC of the
//! last entry handed on — so a node that predates this reads a position it understands (it skips a
//! later same-HLC sibling, as it always did). Beside it, `clog/{stream}/{group}/cursor` holds the last
//! entry's key suffix after `log/{stream}/` (`{hlc:016x}/{node}`). A reader takes the cursor when it is
//! at least as far as the offset, and otherwise the offset, read as *every entry at or before this HLC*
//! — which is what an older writer meant by it.

use crate::context::CoreCtx;
use crate::store::{scan_kv_prefix, KvState};
use bytes::Bytes;
use std::sync::Arc;

/// Where a consumer group stands in a stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LogPosition {
    /// Nothing handed on yet.
    Start,
    /// Every entry at or before this HLC has been handed on — the legacy offset's meaning.
    ThroughHlc(u64),
    /// The entry with this HLC and key suffix (after `log/{stream}/`) was the last handed on.
    After {
        /// The entry's HLC.
        hlc: u64,
        /// The entry's key after `log/{stream}/`: `{hlc:016x}` or `{hlc:016x}/{node}`.
        suffix: Arc<str>,
    },
}

/// One log entry with the key it is ordered by.
#[derive(Clone, Debug)]
pub struct LogRow {
    /// The entry's HLC (the key's first segment).
    pub hlc: u64,
    /// The entry's key after `log/{stream}/`.
    pub suffix: Arc<str>,
    /// The entry's value.
    pub value: Bytes,
}

impl LogPosition {
    /// Is `row` after this position, in `(hlc, key)` order?
    pub fn precedes(&self, row: &LogRow) -> bool {
        match self {
            LogPosition::Start => true,
            LogPosition::ThroughHlc(h) => row.hlc > *h,
            LogPosition::After { hlc, suffix } => (row.hlc, &*row.suffix) > (*hlc, &**suffix),
        }
    }

    /// The position just past `row`.
    pub fn past(row: &LogRow) -> Self {
        LogPosition::After { hlc: row.hlc, suffix: Arc::clone(&row.suffix) }
    }
}

/// `clog/{stream}/{group}/offset` — the legacy HLC-only position, still written.
pub fn offset_key(stream: &str, group: &str) -> String {
    format!("clog/{stream}/{group}/offset")
}

/// `clog/{stream}/{group}/cursor` — the `(hlc, key)` position.
pub fn cursor_key(stream: &str, group: &str) -> String {
    format!("clog/{stream}/{group}/cursor")
}

fn live(kv: &KvState, key: &str) -> Option<Bytes> {
    kv.store.pin().get(key).and_then(|e| e.data.clone())
}

fn parse_hlc(s: &str) -> Option<u64> {
    u64::from_str_radix(s.split('/').next()?, 16).ok()
}

/// The group's position, read from the store (see the module doc for the two keys).
pub fn read_position(kv: &KvState, stream: &str, group: &str) -> LogPosition {
    let text = |key: String| live(kv, &key).and_then(|b| String::from_utf8(b.to_vec()).ok());
    let offset = text(offset_key(stream, group)).and_then(|s| parse_hlc(&s));
    let cursor = text(cursor_key(stream, group))
        .and_then(|s| parse_hlc(&s).map(|hlc| (hlc, Arc::<str>::from(s.as_str()))));
    match (cursor, offset) {
        (Some((hlc, suffix)), off) if off.is_none_or(|o| hlc >= o) => LogPosition::After { hlc, suffix },
        (_, Some(o)) => LogPosition::ThroughHlc(o),
        // `(Some(cursor), None)` is taken by the first arm.
        _ => LogPosition::Start,
    }
}

/// Every entry of `stream` after `pos`, in `(hlc, key)` order.
pub fn rows_after(kv: &KvState, stream: &str, pos: &LogPosition) -> Vec<LogRow> {
    let prefix = format!("log/{stream}/");
    let mut rows: Vec<LogRow> = scan_kv_prefix(kv, &prefix)
        .into_iter()
        .filter_map(|(k, value)| {
            let suffix = k.strip_prefix(&prefix)?;
            let hlc = parse_hlc(suffix)?;
            Some(LogRow { hlc, suffix: Arc::from(suffix), value })
        })
        .filter(|row| pos.precedes(row))
        .collect();
    rows.sort_by(|a, b| (a.hlc, &a.suffix).cmp(&(b.hlc, &b.suffix)));
    rows
}

/// Record that `row` was handed on: the cursor, then the legacy offset (so a reader that sees the new
/// offset also sees a cursor at least as far). Through the ordinary KV write path (applied, logged to
/// the WAL when one is configured, gossiped).
pub fn record(ctx: &CoreCtx, stream: &str, group: &str, row: &LogRow) {
    let _ = crate::ops::kv_set(ctx, Arc::from(cursor_key(stream, group).as_str()), Bytes::from(row.suffix.as_bytes().to_vec()));
    let _ = crate::ops::kv_set(ctx, Arc::from(offset_key(stream, group).as_str()), Bytes::from(format!("{:016x}", row.hlc).into_bytes()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(hlc: u64, node: &str) -> LogRow {
        LogRow { hlc, suffix: Arc::from(format!("{hlc:016x}/{node}").as_str()), value: Bytes::new() }
    }

    #[test]
    fn a_position_orders_by_hlc_then_key() {
        let a = row(0x100, "10.0.0.1:1");
        let b = row(0x100, "10.0.0.2:1");
        let c = row(0x200, "10.0.0.1:1");
        assert!(LogPosition::Start.precedes(&a));
        let after_a = LogPosition::past(&a);
        assert!(!after_a.precedes(&a) && after_a.precedes(&b) && after_a.precedes(&c), "the same-HLC sibling is still ahead");
        // The legacy offset means every entry at that HLC is done.
        assert!(!LogPosition::ThroughHlc(0x100).precedes(&b) && LogPosition::ThroughHlc(0x100).precedes(&c));
        // An unsalted (pre-salt) key sorts before every salted key at its HLC.
        let bare = LogRow { hlc: 0x100, suffix: Arc::from(format!("{:016x}", 0x100).as_str()), value: Bytes::new() };
        assert!(LogPosition::past(&bare).precedes(&a));
    }

    #[test]
    fn the_cursor_is_read_only_when_it_is_at_least_as_far_as_the_offset() {
        let kv = KvState::new(0);
        let hlc = crate::hlc::Hlc::new();
        let id = crate::node_id::NodeId::new("127.0.0.1", 1).unwrap();
        let put = |k: String, v: &str| {
            crate::store::apply_and_notify(&kv, &crate::framing::make_gossip_update(&id, 60, Arc::from(k.as_str()), Bytes::from(v.to_string()), false, &hlc));
        };
        assert_eq!(read_position(&kv, "s", "g"), LogPosition::Start);
        put(offset_key("s", "g"), &format!("{:016x}", 0x100));
        assert_eq!(read_position(&kv, "s", "g"), LogPosition::ThroughHlc(0x100), "an older writer's offset");
        put(cursor_key("s", "g"), &format!("{:016x}/n1", 0x100));
        assert!(matches!(read_position(&kv, "s", "g"), LogPosition::After { hlc: 0x100, .. }), "the cursor at the offset");
        // An older node advanced the offset past the cursor: the offset wins.
        put(offset_key("s", "g"), &format!("{:016x}", 0x300));
        assert_eq!(read_position(&kv, "s", "g"), LogPosition::ThroughHlc(0x300));
    }
}
