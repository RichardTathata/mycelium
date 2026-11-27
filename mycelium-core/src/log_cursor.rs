//! A consumer group's position in a log stream, in the log's own order (post-360 hardening row E).
//!
//! A log entry's key is `log/{stream}/{hlc:016x}/{node}`: two nodes appending in one millisecond share
//! an HLC, and the node salt keeps both. [`KvHandle::scan_log`](crate::KvHandle::scan_log) orders a
//! stream by `(hlc, key)`, so a consumer's position must be that pair — a position that is an HLC alone
//! cannot say which of two same-HLC entries it has passed, and the consumers that kept one delivered one
//! of them and skipped the other for ever.
//!
//! **On the wire, two keys.** `clog/{stream}/{group}/offset` keeps its old format — a 16-hex HLC, read as
//! *every entry through this HLC is done* — but is written **one below** the last entry's HLC, so a node
//! that predates the cursor, or one whose cursor write has not arrived yet, re-delivers rather than skips
//! (#613's review; [`record`]). Beside it, `clog/{stream}/{group}/cursor` holds the last
//! entry's key suffix after `log/{stream}/` (`{hlc:016x}/{node}`). A reader takes the cursor when it is
//! at least as far as the offset, and otherwise the offset, read as *every entry at or before this HLC*
//! — which is what an older writer meant by it.
//!
//! **A live subscription is not a group** (engineering review ER8). [`LiveCursor`] keeps no position: it
//! holds the key suffixes it has handed on, so an entry that arrives after a later one was delivered is
//! still delivered. The group's single position cannot, and a late entry that sorts before it is not
//! delivered to the group (`docs/operations/what-is-proven.md#log-group-skips-a-late-row`).

use crate::context::CoreCtx;
use crate::store::{scan_kv_prefix, KvState};
use bytes::Bytes;
use std::{collections::HashSet, sync::Arc};

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

/// A **live** subscription's place in a stream (engineering review ER8, R8-01): the floor it was opened at and
/// the entries it has handed on, by key suffix — not a position.
///
/// **The contract.** Every live entry at or after `since` is handed on **once per subscription**. The backlog
/// and each later batch are in `(hlc, key)` order; an entry that arrives late — a same-stamp sibling from a peer,
/// or an entry stamped *below* the last one handed on, from a lagging writer or a healed partition — is handed on
/// when it arrives, so across batches the order is **arrival order** and a subscriber may see a stamp lower than
/// the previous one. `since` is a floor, not a resume token: a resumed subscription is opened at the last stamp
/// it saw, inclusive, and gets that stamp's entries again.
///
/// A [`LogPosition`] cannot do this: a single position skips whatever sorts before it, which is right for a
/// group's persisted offset and wrong for a live stream. **Memory** is one key suffix per live entry of the
/// stream at or after `since`; [`compact_log`](crate::KvHandle::compact_log) shrinks it, as each
/// [`pending`](Self::pending) forgets a suffix that is no longer live. A key deleted and written again is a new
/// entry and is handed on again; an entry's value rewritten in place under the same key is not. Comparisons
/// only, so a crafted `ffff…` stamp neither overflows nor moves the floor.
#[derive(Debug)]
pub struct LiveCursor {
    since: u64,
    delivered: HashSet<Arc<str>>,
}

impl LiveCursor {
    /// A cursor that hands on every entry at or after `since`.
    pub fn new(since: u64) -> Self {
        Self { since, delivered: HashSet::new() }
    }

    /// Live rows at or after `since` not yet handed on, in `(hlc, key)` order. Forgets delivered suffixes
    /// that are no longer live.
    pub fn pending(&mut self, kv: &KvState, stream: &str) -> Vec<LogRow> {
        let rows: Vec<LogRow> = rows_after(kv, stream, &LogPosition::Start)
            .into_iter()
            .filter(|r| r.hlc >= self.since)
            .collect();
        if !self.delivered.is_empty() {
            let live: HashSet<&str> = rows.iter().map(|r| &*r.suffix).collect();
            self.delivered.retain(|s| live.contains(&**s));
        }
        rows.into_iter().filter(|r| !self.delivered.contains(&r.suffix)).collect()
    }

    /// Record that `row` was handed on — after its send succeeded.
    pub fn mark(&mut self, row: &LogRow) {
        self.delivered.insert(Arc::clone(&row.suffix));
    }
}

/// Record that `row` was handed on: the cursor, then the legacy offset. Through the ordinary KV write
/// path (applied, logged to the WAL when one is configured, gossiped).
///
/// **The two are separate writes, and a peer may receive them in either order** (#613's review): only on
/// the writing node is the cursor always at least as far as the offset. So the offset is written **one
/// below** the entry's HLC: a reader that has the new offset and not yet the new cursor — or a node that
/// predates the cursor and reads only the offset — resumes at the entry's HLC and **re-delivers** that
/// entry and any same-HLC sibling, never skips one. An entry at HLC 0 is the one case the offset cannot
/// express (it saturates at 0); HLCs are wall-clock based, so none is.
pub fn record(ctx: &CoreCtx, stream: &str, group: &str, row: &LogRow) {
    let _ = crate::ops::kv_set(ctx, Arc::from(cursor_key(stream, group).as_str()), Bytes::from(row.suffix.as_bytes().to_vec()));
    let _ = crate::ops::kv_set(ctx, Arc::from(offset_key(stream, group).as_str()), Bytes::from(format!("{:016x}", row.hlc.saturating_sub(1)).into_bytes()));
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

    /// ER8: a live cursor hands on a late sibling and a late lower stamp, never repeats an entry, and its
    /// delivered set shrinks when entries are compacted away — memory is bounded by the live stream.
    #[test]
    fn the_delivered_set_forgets_compacted_entries() {
        let kv = KvState::new(0);
        let hlc = crate::hlc::Hlc::new();
        let id = crate::node_id::NodeId::new("127.0.0.1", 1).unwrap();
        let write = |h: u64, node: &str, tomb: bool| {
            let key = format!("log/s/{h:016x}/{node}");
            crate::store::apply_and_notify(&kv, &crate::framing::make_gossip_update(&id, 60, Arc::from(key.as_str()), Bytes::from_static(b"v"), tomb, &hlc));
        };
        let suffixes = |rows: &[LogRow]| rows.iter().map(|r| r.suffix.to_string()).collect::<Vec<_>>();
        for h in [0x100, 0x200, 0x300] { write(h, "a", false); }
        let mut cursor = LiveCursor::new(0x200);
        let first = cursor.pending(&kv, "s");
        assert_eq!(suffixes(&first), [format!("{:016x}/a", 0x200), format!("{:016x}/a", 0x300)], "the floor is inclusive");
        for r in &first { cursor.mark(r); }
        assert!(cursor.pending(&kv, "s").is_empty(), "nothing is handed on twice");
        // Late arrivals: a same-stamp sibling and a lower stamp above the floor; one below it is never pending.
        write(0x300, "b", false);
        write(0x250, "c", false);
        write(0x1ff, "d", false);
        let late = cursor.pending(&kv, "s");
        assert_eq!(suffixes(&late), [format!("{:016x}/c", 0x250), format!("{:016x}/b", 0x300)]);
        for r in &late { cursor.mark(r); }
        assert_eq!(cursor.delivered.len(), 4);
        // Compaction tombstones the older entries; the set forgets them on the next pass.
        write(0x200, "a", true);
        write(0x250, "c", true);
        assert!(cursor.pending(&kv, "s").is_empty());
        assert_eq!(cursor.delivered.len(), 2, "only the live entries' suffixes are kept");
        // A crafted maximum stamp is an entry like any other and moves nothing.
        write(u64::MAX, "x", false);
        write(0x400, "a", false);
        let next = cursor.pending(&kv, "s");
        assert_eq!(suffixes(&next), [format!("{:016x}/a", 0x400), format!("{:016x}/x", u64::MAX)]);
    }
}
