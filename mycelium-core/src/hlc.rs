//! Hybrid Logical Clock (HLC) used to stamp every locally-originated update
//! and to observe remote stamps as they arrive.
//!
//! The clock is packed into a `u64`:
//!
//! - **High 48 bits**: physical Unix-millisecond time. Good through year 8901.
//! - **Low 16 bits**: logical counter. Up to 65 535 events per millisecond
//!   per node before saturation.
//!
//! This packing is deliberate: comparisons on the raw `u64` are also
//! comparisons on the (physical, logical) tuple in lexicographic order, so
//! the existing `>`-based LWW comparison in `apply_and_notify` continues to
//! work unchanged when the timestamp field is reinterpreted as HLC. Higher
//! physical always beats lower physical regardless of logical; with equal
//! physical, higher logical wins.
//!
//! ## Algorithm (Kulkarni et al. 2014)
//!
//! For a local event:
//!
//! ```text
//! next.phys    = max(prev.phys, wall_now_ms)
//! next.logical = if next.phys == prev.phys { prev.logical + 1 } else { 0 }
//! ```
//!
//! For an observed remote event with timestamp `r`:
//!
//! ```text
//! next.phys    = max(prev.phys, r.phys, wall_now_ms)
//! next.logical = match (next.phys ==) {
//!   prev.phys && r.phys => max(prev.logical, r.logical) + 1,
//!   prev.phys           => prev.logical + 1,
//!   r.phys              => r.logical + 1,
//!   neither             => 0,
//! }
//! ```
//!
//! This guarantees that any locally-originated update following an observed
//! remote update has a strictly greater HLC than the remote one — so causal
//! happens-before is preserved even under wall-clock skew.
//!
//! ## Documented limits
//!
//! - **Logical counter saturation.** The low 16 bits cap the logical at
//!   `65 535` events per ms per node. `tick()` saturates rather than
//!   wrapping, so on a node sustained at >65 k local events/ms the
//!   physical part takes over (every saturating-tick acts as if the wall
//!   clock advanced by 1 ms). Ordering stays correct; resolution degrades
//!   gracefully. Widening to a larger logical (e.g. 44+20) would require a
//!   wire-version bump and is deferred.
//!
//! - **Wall-clock forward jump.** Accepted unconditionally:
//!   `next.phys = max(prev.phys, wall_now_ms)` will jump the HLC ahead.
//!   Correctness for causal happens-before is unaffected. The
//!   `crate::seen` TTL eviction is keyed by `physical_ms`; a large
//!   forward jump can briefly age out the entire seen-set, allowing one
//!   round of duplicate admissions until the seen-set repopulates.
//!
//! - **Wall-clock backward jump.** `prev.phys` wins via `max`, so the HLC
//!   never goes backwards. Subsequent `wall_now_ms` reads return values
//!   smaller than `prev.phys`; the logical counter increments instead,
//!   maintaining strict monotonicity. The HLC is "self-correcting" against
//!   transient backward jumps but cannot recover lost time once the clock
//!   resyncs forward again.
//!
//! - **Remote drift bound.** `observe` clamps the *remote* physical time to
//!   `wall_now_ms + max_drift_ms` (default 5 minutes,
//!   [`DEFAULT_MAX_CLOCK_DRIFT_MS`]; configurable via
//!   `GossipConfig::max_clock_drift_ms`, `0` disables). Without the clamp a
//!   single peer with a far-future clock dragged every node's HLC forward
//!   irrecoverably (`max` never decays), and read-side evaporation — the
//!   substrate's failure detector — was silently suspended for the full
//!   drift duration (M2 Run-19 finding; the Kulkarni et al. 2014 algorithm
//!   mandates exactly this bound). Clamping is rate-limit `warn!`ed, and an
//!   inbound KV entry that was clamped is counted at the door it came through
//!   (`GET /stats` `kv_future_stamps`; `observe_reporting`).
//!
//!   **Documented trade-off:** for a stamp *beyond* the bound, the usual
//!   "local write after observe beats the remote" guarantee is deliberately
//!   waived — the poisoned entry would win LWW against local rewrites until
//!   the wall clock catches up. The capability layer independently
//!   quarantines such entries (its freshness check rejects stamps too far
//!   in the future), so failure detection no longer depends on this. Full
//!   inbound rejection of out-of-bound updates (store-level quarantine) is
//!   a candidate for the next wire-policy pass.

use std::sync::atomic::{AtomicU64, Ordering};
use tracing::warn;

/// Default bound on how far ahead of the local wall clock a *remote* HLC
/// stamp may pull this node's clock (5 minutes). See the module doc's
/// "Remote drift bound" section. Override via
/// `GossipConfig::max_clock_drift_ms`; `0` disables the bound.
pub const DEFAULT_MAX_CLOCK_DRIFT_MS: u64 = 300_000;

/// Minimum interval between drift-clamp warnings (per `Hlc` instance), so a
/// peer with a persistently skewed clock cannot flood the log.
const DRIFT_WARN_INTERVAL_MS: u64 = 10_000;

/// Number of bits reserved for the logical counter.
pub const LOGICAL_BITS: u32 = 16;
/// Mask covering the logical portion of a packed HLC value.
pub const LOGICAL_MASK: u64 = (1 << LOGICAL_BITS) - 1;
/// Maximum representable physical-ms value — the top `64 - LOGICAL_BITS` bits. Packing a larger
/// physical component would shift bits out of the `u64` and silently wrap (audit 2026-07-15 pass 4);
/// `pack` saturates to this and `tick`'s carry stops here. `2^48 - 1` ms ≈ year 8901, so wall time
/// never reaches it — only a poisoned `observe` (a `u64::MAX` remote stamp with `max_drift_ms = 0`) can.
pub const PHYS_MAX: u64 = (1 << (64 - LOGICAL_BITS)) - 1;

/// Extracts the physical-ms portion of a packed HLC timestamp.
#[inline]
pub fn physical_ms(packed: u64) -> u64 {
    packed >> LOGICAL_BITS
}

/// **The drift clamp's condition**: whether a packed stamp's physical time is strictly past `limit_ms`
/// (the receiver's wall clock plus its bound, saturating). Strict: a stamp *on* the limit is inside
/// the bound. The logical counter plays no part, and neither does what the entry is — a tombstone's
/// stamp is a stamp. `u64::MAX` reads as [`PHYS_MAX`], past any limit a wall clock can produce.
/// One shift and one comparison.
#[inline]
pub fn beyond_drift_bound(packed: u64, limit_ms: u64) -> bool {
    physical_ms(packed) > limit_ms
}

/// Packs `(phys_ms, logical)` into a single `u64` HLC value. `phys_ms` is saturated to [`PHYS_MAX`]
/// so the left-shift can never drop high bits and wrap to a small value (audit 2026-07-15 pass 4) —
/// a total function for any input.
#[inline]
pub fn pack(phys_ms: u64, logical: u64) -> u64 {
    (phys_ms.min(PHYS_MAX) << LOGICAL_BITS) | (logical & LOGICAL_MASK)
}

/// Returns the current wall-clock time in milliseconds since the Unix epoch.
/// Saturates to 0 if the clock is somehow before the epoch (a Windows-only
/// edge case after manual clock changes).
///
/// **The HLC's one wall-clock read** — the seam the replay inventory calls clean (§2.1), and the
/// read every write's LWW rank depends on. Routed through [`crate::sim_seam`] so a recorded run can
/// replay it; without the `sim` feature that is the same `SystemTime::now()` call it has always
/// been, so the substrate pays nothing for the harness's existence.
#[inline]
fn wall_now_ms() -> u64 {
    crate::sim_seam::wall_now_ms()
}

/// Hybrid Logical Clock. Internal state is a single `AtomicU64` storing the
/// packed `(phys_ms << 16) | logical` value, so both `tick` and `observe`
/// are lock-free CAS loops.
pub struct Hlc {
    state: AtomicU64,
    /// Remote stamps may pull the clock at most this far ahead of the local
    /// wall clock. `0` = unbounded (pre-bound behaviour).
    max_drift_ms: u64,
    /// Wall-ms of the last drift-clamp warning (rate limiting).
    last_drift_warn_ms: AtomicU64,
}

impl Hlc {
    /// Constructs a fresh HLC initialised to the current wall clock with
    /// logical zero and the default remote-drift bound.
    pub fn new() -> Self {
        Self::with_max_drift(DEFAULT_MAX_CLOCK_DRIFT_MS)
    }

    /// Like [`new`](Self::new) with an explicit remote-drift bound in
    /// milliseconds. `0` disables the bound.
    pub fn with_max_drift(max_drift_ms: u64) -> Self {
        Self {
            state:              AtomicU64::new(pack(wall_now_ms(), 0)),
            max_drift_ms,
            last_drift_warn_ms: AtomicU64::new(0),
        }
    }

    /// **Test support only** (closure plan C11): set the packed state directly, to reproduce a node
    /// whose clock was last ticked long ago. `Hlc::new()` seeds from the wall clock, so a test that
    /// does not force the state into the past cannot tell a live clock from a frozen one.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn force_state_for_tests(&self, packed: u64) {
        self.state.store(packed, Ordering::Release);
    }

    /// Returns the current packed HLC value without advancing it.
    ///
    /// **Not for decisions** (closure plan C11): this is the last value the clock was *ticked* to,
    /// and it stands still on a node with no traffic. A deadline, an expiry or a freshness check
    /// reads [`decision_now_ms`](Self::decision_now_ms). `scripts/check-hlc-current.sh` keeps new
    /// reads of this from appearing unclassified.
    pub fn current(&self) -> u64 {
        self.state.load(Ordering::Acquire)
    }

    /// **The current time, in the HLC's own frame, without advancing the clock.**
    ///
    /// Use this — not [`current`](Self::current) — wherever a *decision* depends on the time:
    /// whether a mandate has expired, whether a checkpoint is stale, when a credential was issued.
    ///
    /// `current()` is a bare load of the last value the clock was **ticked to**. It never reads the
    /// wall clock, so on a node whose gossip has gone quiet — which is exactly the partition case —
    /// it **freezes**. Anything comparing a deadline against it then fails *open*: an expired
    /// mandate never expires, a superseded policy never goes stale. That is the wrong direction for
    /// a authorization decision to fail in.
    ///
    /// This reads the wall clock and floors it at any peer time already observed, so both sides of
    /// a comparison sit in one frame — the same reasoning as the consensus lease read (audit
    /// 2026-07-15, BUG 8), which shrinks the disagreement window to true unsynchronised skew.
    ///
    /// It does **not** advance the clock. `tick()` would also read the wall clock, but it mutates
    /// shared state and consumes logical-counter space on what is a read path.
    pub fn decision_now_ms(&self) -> u64 {
        wall_now_ms().max(physical_ms(self.current()))
    }

    /// Advances the clock by one logical step **without reading the wall clock** and returns the
    /// new packed timestamp — strictly greater than every stamp this clock has handed out, and never
    /// ahead of what [`tick`](Self::tick) would return at the same instant (the same carry rule, from
    /// the same state, minus the `max(now)`).
    ///
    /// For a **failing exit that must consume no choice**: under replay every wall-clock read is a
    /// recorded decision, and a fault injected into a clean recording (the storage fault sweep) is
    /// followed by code the recording never ran. If that code reads the clock, the replay diverges at
    /// the read instead of measuring what the fault cost. `do_snapshot` lowers the node's opacity
    /// mark on its failing exit with this, so the lowering is a store write and nothing else.
    pub fn tick_logical(&self) -> u64 {
        loop {
            let prev = self.state.load(Ordering::Acquire);
            // `+1` on the packed value is `tick`'s own carry: the logical counter, overflowing into
            // the physical millisecond at `LOGICAL_MASK`; saturating at the ceiling as `tick` does.
            let next = prev.saturating_add(1);
            if self.state
                .compare_exchange(prev, next, Ordering::Release, Ordering::Acquire)
                .is_ok()
            {
                return next;
            }
        }
    }

    /// Advances the clock for a local event and returns the new packed
    /// timestamp.
    pub fn tick(&self) -> u64 {
        loop {
            let prev      = self.state.load(Ordering::Acquire);
            let prev_phys = physical_ms(prev);
            let prev_log  = prev & LOGICAL_MASK;
            let now_ms    = wall_now_ms();

            let mut next_phys = prev_phys.max(now_ms);
            let next_log = if next_phys == prev_phys {
                if prev_log >= LOGICAL_MASK {
                    if next_phys >= PHYS_MAX {
                        // At the physical ceiling — carrying (`next_phys += 1` → `pack(2^48 << 16)`)
                        // OVERFLOWS the u64 and WRAPS TO 0, corrupting every subsequent LWW comparison
                        // and fencing token (audit 2026-07-15 pass 4, reachable via a poisoned
                        // `observe` under `max_drift_ms = 0`). No strictly-larger stamp exists at the
                        // ceiling, so saturate at `u64::MAX` (pack(PHYS_MAX, LOGICAL_MASK)) instead.
                        LOGICAL_MASK
                    } else {
                        // Logical component saturated within this physical ms (>64k events).
                        // Carry into physical so `tick()` stays *strictly* monotonic — it must
                        // never return a value equal to the previous tick (causal happens-before).
                        next_phys += 1;
                        0
                    }
                } else {
                    prev_log + 1
                }
            } else {
                0
            };
            let new_ts = pack(next_phys, next_log);
            if self.state
                .compare_exchange(prev, new_ts, Ordering::Release, Ordering::Acquire)
                .is_ok()
            {
                return new_ts;
            }
        }
    }

    /// Absorbs a remote HLC stamp and advances the local clock to dominate
    /// the merged `(local, remote, wall_now)` triple. Returns the new packed
    /// timestamp.
    ///
    /// The remote physical component is clamped to
    /// `wall_now_ms + max_drift_ms` (see the module doc's "Remote drift
    /// bound") so a peer with a far-future clock cannot drag this node's
    /// clock — and with it, every subsequent local write stamp — into the
    /// future. Clamping is logged at `warn!`, rate-limited per instance.
    pub fn observe(&self, remote: u64) -> u64 {
        self.observe_reporting(remote).0
    }

    /// [`observe`](Self::observe), and **whether the stamp was beyond the drift bound** — the clamp's
    /// own condition ([`beyond_drift_bound`]), from the clamp's own wall-clock read, so a caller that
    /// counts such stamps (the three inbound KV doors, `connection.rs`; external-critique plan CR13,
    /// step 1) counts exactly the ones that were clamped and reads no clock of its own: no second
    /// read that could disagree with the first, and no extra recorded decision under replay. Always
    /// `false` when the bound is `0`.
    pub fn observe_reporting(&self, remote: u64) -> (u64, bool) {
        let mut remote_phys = physical_ms(remote);
        let remote_log      = remote & LOGICAL_MASK;
        let mut beyond      = false;
        if self.max_drift_ms > 0 {
            let limit = wall_now_ms().saturating_add(self.max_drift_ms);
            if beyond_drift_bound(remote, limit) {
                beyond = true;
                let now = wall_now_ms();
                let last = self.last_drift_warn_ms.load(Ordering::Relaxed);
                if now.saturating_sub(last) >= DRIFT_WARN_INTERVAL_MS
                    && self
                        .last_drift_warn_ms
                        .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                        .is_ok()
                {
                    warn!(
                        remote_phys_ms = remote_phys,
                        ahead_ms = remote_phys.saturating_sub(now), // wall may advance past remote between the two reads (pass 4)
                        max_drift_ms = self.max_drift_ms,
                        "remote HLC stamp exceeds the clock-drift bound; clamping — \
                         a peer's clock is far ahead (check NTP on the sending node). \
                         Entries carrying such stamps are quarantined by read-side \
                         freshness checks until the sender's clock is fixed.",
                    );
                }
                remote_phys = limit;
            }
        }
        loop {
            let prev      = self.state.load(Ordering::Acquire);
            let prev_phys = physical_ms(prev);
            let prev_log  = prev & LOGICAL_MASK;
            let now_ms    = wall_now_ms();

            let next_phys = prev_phys.max(remote_phys).max(now_ms);
            let next_log  = if next_phys == prev_phys && next_phys == remote_phys {
                prev_log.max(remote_log).saturating_add(1).min(LOGICAL_MASK)
            } else if next_phys == prev_phys {
                prev_log.saturating_add(1).min(LOGICAL_MASK)
            } else if next_phys == remote_phys {
                remote_log.saturating_add(1).min(LOGICAL_MASK)
            } else {
                0
            };
            let new_ts = pack(next_phys, next_log);
            if self.state
                .compare_exchange(prev, new_ts, Ordering::Release, Ordering::Acquire)
                .is_ok()
            {
                return (new_ts, beyond);
            }
        }
    }
}

impl Default for Hlc {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `tick_logical` is strictly monotonic from the clock's own state and never ahead of `tick`.
    #[test]
    fn tick_logical_advances_without_the_clock_and_stays_behind_tick() {
        let hlc = Hlc::new();
        let a = hlc.tick();
        let b = hlc.tick_logical();
        assert_eq!(b, a + 1, "one logical step from the last stamp");
        assert_eq!(physical_ms(b), physical_ms(a));
        let c = hlc.tick();
        assert!(c > b, "the next wall tick is still strictly later");
        // At a saturated logical counter the step carries into the millisecond, as `tick` does.
        hlc.force_state_for_tests(pack(1_000, LOGICAL_MASK));
        assert_eq!(hlc.tick_logical(), pack(1_001, 0));
    }

    /// **The clamp's condition is strict and reads the physical time only** (CR13, step 1): a stamp on
    /// the limit is inside the bound whatever its logical counter, one millisecond past it is beyond,
    /// and `u64::MAX` is beyond any limit a wall clock can produce.
    #[test]
    fn beyond_the_drift_bound_is_strictly_past_the_limit() {
        let limit = 1_000_000;
        assert!(!beyond_drift_bound(pack(limit - 1, 0), limit), "inside");
        assert!(!beyond_drift_bound(pack(limit, 0), limit), "on the limit");
        assert!(!beyond_drift_bound(pack(limit, LOGICAL_MASK), limit), "on the limit, logical counter full");
        assert!(beyond_drift_bound(pack(limit + 1, 0), limit), "one millisecond past");
        assert!(beyond_drift_bound(u64::MAX, limit), "u64::MAX");
        assert!(!beyond_drift_bound(0, 0) && !beyond_drift_bound(u64::MAX, u64::MAX), "the ends");
    }

    /// **`observe_reporting` reports exactly the stamps `observe` clamps** — with the wall clock held
    /// still, so the bound is a number: on it, not reported and the clock follows the stamp; one
    /// past it, reported and the clock stops at the limit; `u64::MAX` the same; with the bound at 0
    /// nothing is reported and nothing is clamped.
    #[cfg(not(feature = "sim"))]
    #[test]
    fn observe_reports_a_stamp_it_clamped_and_no_other() {
        const WALL: u64 = 1_800_000_000_000;
        const BOUND: u64 = 300_000;
        struct Unpin;
        impl Drop for Unpin { fn drop(&mut self) { crate::sim_seam::pin_wall_clock_for_test(None); } }
        crate::sim_seam::pin_wall_clock_for_test(Some(WALL));
        let _unpin = Unpin;

        let hlc = Hlc::with_max_drift(BOUND);
        let (t, beyond) = hlc.observe_reporting(pack(WALL + BOUND - 1, 7));
        assert!(!beyond && physical_ms(t) == WALL + BOUND - 1, "inside the bound");
        let (t, beyond) = hlc.observe_reporting(pack(WALL + BOUND, LOGICAL_MASK));
        assert!(!beyond && physical_ms(t) == WALL + BOUND, "on the bound");
        let (t, beyond) = hlc.observe_reporting(pack(WALL + BOUND + 1, 0));
        assert!(beyond, "one millisecond past the bound is reported");
        assert_eq!(physical_ms(t), WALL + BOUND, "and clamped to it");
        let (t, beyond) = hlc.observe_reporting(u64::MAX);
        assert!(beyond && physical_ms(t) == WALL + BOUND, "u64::MAX is reported and clamped");
        assert_eq!(hlc.observe(u64::MAX), hlc.current(), "observe is observe_reporting's first half");

        let unbounded = Hlc::with_max_drift(0);
        let (t, beyond) = unbounded.observe_reporting(pack(WALL + BOUND + 1, 0));
        assert!(!beyond && physical_ms(t) == WALL + BOUND + 1, "no bound: nothing reported, nothing clamped");
        assert!(!unbounded.observe_reporting(u64::MAX).1, "not even u64::MAX");
    }

    /// **What the clamp leaves behind, and why a count names no sender** (#782's review, F1). Three
    /// cases of existing clock behaviour, the wall clock held still; this test changes none of it.
    /// 1. A node observes `u64::MAX` (reported, clamped): its clock stands at the limit with the
    ///    logical counter full, so its **next local write** is one millisecond past the limit, and a
    ///    peer with the same wall clock reports that honest write.
    /// 2. A node observes a stamp **on** the limit with the logical counter full — *not reported* —
    ///    and its next local write is again one millisecond past, reported at a same-clock peer.
    /// 3. A peer four minutes ahead (inside the five-minute bound, not reported) pulls a node's clock
    ///    forward; that node's next write is reported by a receiver only 61 s behind, and not by one
    ///    60 s behind. The slow-clock case starts at the bound minus the largest forward skew in the
    ///    fleet, not at the bound.
    #[cfg(not(feature = "sim"))]
    #[test]
    fn an_honest_write_after_an_observed_future_stamp_is_reported_by_a_peer() {
        const WALL: u64 = 1_800_000_000_000;
        const BOUND: u64 = 300_000;
        struct Unpin;
        impl Drop for Unpin { fn drop(&mut self) { crate::sim_seam::pin_wall_clock_for_test(None); } }
        let _unpin = Unpin;
        let pin = |ms| crate::sim_seam::pin_wall_clock_for_test(Some(ms));
        pin(WALL);

        // 1. After a clamped `u64::MAX`.
        let (writer, peer) = (Hlc::with_max_drift(BOUND), Hlc::with_max_drift(BOUND));
        assert!(writer.observe_reporting(u64::MAX).1, "the poisoned stamp is reported");
        assert_eq!(writer.current(), pack(WALL + BOUND, LOGICAL_MASK), "and leaves the clock on the limit, counter full");
        let honest = writer.tick();
        assert_eq!(physical_ms(honest), WALL + BOUND + 1, "the next local write is one millisecond past the limit");
        assert!(peer.observe_reporting(honest).1, "a same-clock peer reports the honest write");

        // 2. After a stamp on the limit that was not itself reported.
        let (writer, peer) = (Hlc::with_max_drift(BOUND), Hlc::with_max_drift(BOUND));
        assert!(!writer.observe_reporting(pack(WALL + BOUND, LOGICAL_MASK)).1, "on the limit: not reported");
        let honest = writer.tick();
        assert_eq!(physical_ms(honest), WALL + BOUND + 1);
        assert!(peer.observe_reporting(honest).1, "yet the write that follows it is reported by a same-clock peer");

        // The same with a logical counter that is not full: the clock is left one step past it, the
        // next write is ON the limit, and a same-clock peer does not report it (the confirm round, N1).
        let (writer, peer) = (Hlc::with_max_drift(BOUND), Hlc::with_max_drift(BOUND));
        assert!(writer.observe_reporting(pack(WALL + BOUND + 1, 0)).1);
        assert_eq!(writer.current(), pack(WALL + BOUND, 1));
        assert!(!peer.observe_reporting(writer.tick()).1, "on the limit: a same-clock peer does not report it");

        // 3. A member four minutes ahead, and a receiver 61 s behind.
        let writer = Hlc::with_max_drift(BOUND);
        assert!(!writer.observe_reporting(pack(WALL + 240_000, 0)).1, "four minutes ahead is inside the bound");
        let honest = writer.tick();
        assert_eq!(physical_ms(honest), WALL + 240_000, "the writer's clock rides at the member's");
        pin(WALL - 60_000);
        assert!(!Hlc::with_max_drift(BOUND).observe_reporting(honest).1, "a receiver 60 s behind: on its limit, not reported");
        pin(WALL - 61_000);
        assert!(Hlc::with_max_drift(BOUND).observe_reporting(honest).1, "a receiver 61 s behind reports it");
    }

    /// Pins the HLC state directly; drift bound disabled so tests that pin
    /// far-future physical values keep their original semantics.
    fn pinned(state: u64) -> Hlc {
        Hlc {
            state:              AtomicU64::new(state),
            max_drift_ms:       0,
            last_drift_warn_ms: AtomicU64::new(0),
        }
    }

    /// Regression (audit 2026-07-15): `tick()` must be *strictly* monotonic even when the
    /// logical counter has saturated within a physical ms — the old code returned a value
    /// EQUAL to the previous tick (carry into physical was documented but never done).
    #[test]
    fn regression_tick_strictly_monotonic_at_saturation() {
        let far = wall_now_ms() + 60_000; // future physical so the loop stays on the saturation branch
        let hlc = pinned(pack(far, LOGICAL_MASK));
        let a = hlc.tick();
        let b = hlc.tick();
        assert!(b > a, "tick not strictly monotonic at logical saturation: a={a} b={b}");
        assert_eq!(physical_ms(a), far + 1, "the saturated tick must carry into physical");
    }

    #[test]
    fn pack_and_unpack_roundtrip() {
        let p = pack(1_700_000_000_000, 42);
        assert_eq!(physical_ms(p), 1_700_000_000_000);
        assert_eq!(p & LOGICAL_MASK, 42);
    }

    #[test]
    fn comparison_lex_order_phys_then_logical() {
        let a = pack(100, 0);
        let b = pack(100, 1);
        let c = pack(101, 0);
        assert!(a < b);
        assert!(b < c);
        // Physical-only difference dominates logical.
        assert!(pack(99, LOGICAL_MASK) < pack(100, 0));
    }

    #[test]
    fn tick_strictly_monotonic() {
        let hlc = Hlc::new();
        let mut prev = hlc.tick();
        for _ in 0..1_000 {
            let next = hlc.tick();
            assert!(next > prev, "tick must be strictly monotonic ({} <= {})", next, prev);
            prev = next;
        }
    }

    #[test]
    fn tick_bumps_logical_within_same_ms() {
        // Force the HLC to a fixed physical so wall_now_ms doesn't matter.
        let hlc = pinned(pack(u64::MAX >> LOGICAL_BITS, 0));
        let a = hlc.tick();
        let b = hlc.tick();
        assert_eq!(physical_ms(a), physical_ms(b));
        assert!((b & LOGICAL_MASK) > (a & LOGICAL_MASK));
    }

    #[test]
    fn observe_absorbs_strictly_greater_remote_phys() {
        let hlc = Hlc::new();
        // Build a remote stamp 10 seconds in the future.
        let future_phys = physical_ms(hlc.current()) + 10_000;
        let remote = pack(future_phys, 5);
        let next = hlc.observe(remote);
        assert!(next > remote, "observe must dominate the remote stamp");
        assert!(physical_ms(next) >= future_phys);
    }

    #[test]
    fn observe_then_tick_dominates_remote() {
        let hlc = Hlc::new();
        let future_phys = physical_ms(hlc.current()) + 1_000;
        let remote = pack(future_phys, 999);
        hlc.observe(remote);
        let local = hlc.tick();
        assert!(local > remote, "local tick after observe must beat remote");
    }

    #[test]
    fn observe_same_phys_bumps_logical() {
        // Pin the HLC and the remote to a physical value far in the future
        // so wall_now_ms can't dominate either side and reset the logical.
        let far_future = wall_now_ms() + 60_000;
        let hlc = pinned(pack(far_future, 3));
        let remote = pack(far_future, 7);
        let next = hlc.observe(remote);
        assert_eq!(physical_ms(next), far_future);
        assert!((next & LOGICAL_MASK) > 7);
    }

    /// Regression gate for the M2 Run-19 finding (Semantic Correctness).
    ///
    /// `observe` used to absorb remote physical time with an unbounded
    /// `max`: one peer with a far-ahead clock (NTP failure, or a hostile
    /// node in a non-TLS cluster) dragged this node's HLC — and every
    /// subsequent local write stamp — into the future, silently suspending
    /// read-side evaporation (the substrate's failure detector) for the
    /// full drift duration. Fixed by the drift clamp (module doc, "Remote
    /// drift bound"); companion impact gate in the capability layer:
    /// `future_stamped_entry_outlives_its_evaporation_window_by_the_drift`.
    #[test]
    fn observe_bounds_remote_clock_drift() {
        let hlc = Hlc::new(); // default bound: DEFAULT_MAX_CLOCK_DRIFT_MS
        let week_ahead = wall_now_ms() + 7 * 24 * 3600 * 1000;
        hlc.observe(pack(week_ahead, 0));
        let local = hlc.tick();
        assert!(
            physical_ms(local) <= wall_now_ms() + DEFAULT_MAX_CLOCK_DRIFT_MS + 1_000,
            "a remote stamp a week ahead must not drag local time past the \
             drift bound (local phys = {}, wall = {})",
            physical_ms(local), wall_now_ms(),
        );
    }

    /// The bound is configurable and `0` disables it (pre-bound behaviour).
    #[test]
    fn observe_drift_bound_zero_disables_clamp() {
        let hlc = Hlc::with_max_drift(0);
        let week_ahead_phys = wall_now_ms() + 7 * 24 * 3600 * 1000;
        let next = hlc.observe(pack(week_ahead_phys, 0));
        assert!(physical_ms(next) >= week_ahead_phys, "0 = unbounded");
    }

    /// Remote stamps within the bound are still absorbed normally — the
    /// clamp only engages beyond `wall_now + max_drift_ms`.
    #[test]
    fn observe_within_bound_still_dominates_remote() {
        let hlc = Hlc::new();
        let near_future = pack(wall_now_ms() + 10_000, 5); // 10 s ahead
        let next = hlc.observe(near_future);
        assert!(next > near_future, "in-bound remote stamps must be dominated");
    }

    #[test]
    fn logical_saturates_rather_than_wrapping() {
        let hlc = pinned(pack(0, LOGICAL_MASK));
        // Stuck at physical 0 forever (wall_now_ms is huge but the state
        // pretends it's still 1970). Real callers can't hit this — used here
        // to assert the saturation invariant.
        let next = hlc.tick();
        // Saturating: physical advances past 0, so logical resets to 0.
        assert!(physical_ms(next) > 0);
        assert_eq!(next & LOGICAL_MASK, 0);
    }

    #[test]
    fn regression_tick_at_ceiling_saturates_no_wrap_to_zero() {
        // Audit 2026-07-15 pass 4: a poisoned observe (a u64::MAX remote stamp with max_drift_ms=0)
        // drives the clock to the physical ceiling; the next tick's carry (`next_phys += 1`) then did
        // `pack(2^48 << 16)` → u64 overflow → WRAP TO 0, corrupting every subsequent LWW comparison
        // and fencing token. tick() must saturate at u64::MAX at the ceiling instead of wrapping.
        let hlc = pinned(pack(PHYS_MAX, LOGICAL_MASK)); // == u64::MAX
        let next = hlc.tick();
        assert_eq!(next, u64::MAX, "tick at the ceiling must saturate at u64::MAX, not wrap to 0");

        // The reachable path: observe a u64::MAX remote with the drift bound disabled, then tick.
        let hlc = Hlc::with_max_drift(0);
        let after_observe = hlc.observe(u64::MAX);
        let after_tick    = hlc.tick();
        assert!(after_tick >= after_observe, "tick after a poisoned observe must not wrap below");
        assert_ne!(after_tick, 0, "must not wrap to 0");
    }
}

#[cfg(test)]
mod prop_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Sequential tick() calls are strictly monotonically increasing.
        #[test]
        fn tick_always_advances(n_ticks in 2usize..=50usize) {
            let hlc = Hlc::new();
            let mut prev = hlc.tick();
            for _ in 1..n_ticks {
                let next = hlc.tick();
                prop_assert!(next > prev,
                    "tick must be strictly monotonic: {} <= {}", next, prev);
                prev = next;
            }
        }

        /// Input-fuzz gate (audit 2026-07-15 pass 4): observe() of ANY remote stamp (incl. u64::MAX)
        /// under ANY drift bound (incl. 0 = disabled) followed by tick() must never WRAP BELOW the
        /// observed value — the pack/carry overflow that wrapped the clock to 0. This is the HLC
        /// surface the Run-53 fuzz gate did not reach; it runs under overflow-checks, so the wrap fails
        /// the assertion. Unlike `tick_after_observe_beats_remote`, remote is unbounded so it reaches
        /// the physical ceiling where the clock must SATURATE (>=), not strictly advance.
        #[test]
        fn observe_then_tick_never_wraps(remote in any::<u64>(), drift in any::<u64>()) {
            let hlc = Hlc::with_max_drift(drift);
            let observed = hlc.observe(remote);   // must not panic
            let t1 = hlc.tick();                  // the pass-4 overflow site
            let t2 = hlc.tick();
            prop_assert!(t1 >= observed, "tick wrapped below observe: {t1:#x} < {observed:#x}");
            prop_assert!(t2 >= t1, "second tick regressed: {t2:#x} < {t1:#x}");
        }

        /// After observe(remote), a subsequent tick() must produce a value strictly
        /// greater than remote. Remote physical is bounded to ±60 s of now so that
        /// the logical counter never saturates (saturation at 64 K events/ms is
        /// unreachable in any real workload and is documented in the HLC module).
        #[test]
        fn tick_after_observe_beats_remote(
            phys_offset_ms in -60_000i64..=60_000i64,
            remote_log     in 0u64..0x8000u64,
        ) {
            let now = wall_now_ms();
            let remote_phys = (now as i64 + phys_offset_ms).max(1) as u64;
            let remote = pack(remote_phys, remote_log);
            let hlc = Hlc::new();
            hlc.observe(remote);
            let local = hlc.tick();
            prop_assert!(local > remote,
                "tick after observe must beat remote: local={} remote={}", local, remote);
        }

        /// observe(remote) always returns a value >= remote. Equal only at logical
        /// saturation (remote_log = LOGICAL_MASK, remote_phys > now_ms) which is
        /// out of scope for realistic workloads; this test stays well below that.
        #[test]
        fn observe_at_least_as_large_as_remote(
            phys_offset_ms in -60_000i64..=60_000i64,
            remote_log     in 0u64..0x8000u64,
        ) {
            let now = wall_now_ms();
            let remote_phys = (now as i64 + phys_offset_ms).max(1) as u64;
            let remote = pack(remote_phys, remote_log);
            let hlc = Hlc::new();
            let result = hlc.observe(remote);
            prop_assert!(result >= remote,
                "observe must return >= remote: result={} remote={}", result, remote);
        }
    }
}

#[cfg(test)]
mod decision_clock_tests {
    use super::*;

    /// **A decision clock must move even when nothing has ticked it.**
    ///
    /// `current()` is a bare load of the last value the clock was *ticked to*. On a node whose
    /// gossip has gone quiet — which is exactly the partition case — nothing ticks it, and any
    /// deadline compared against it fails **open**: a mandate never expires, a checkpoint never
    /// goes stale, a credential never ages out. That is the wrong direction for an authorization
    /// decision to fail in, and it was live in `ae_preflight`, the gateway caller envelope and the
    /// checkpoint offer until 2026-09-25 (found by review).
    #[test]
    fn a_quiet_clock_still_answers_with_the_time() {
        // A node that has been quiet for an hour: its HLC was last ticked then, and nothing since
        // has moved it. `Hlc::new()` seeds from the wall clock, so the clock must be pushed BACK
        // to reproduce the condition — the first version of this test used a fresh clock and
        // passed against the frozen implementation too, which made it worthless.
        let hlc = Hlc::new();
        let an_hour_ago = wall_now_ms() - 3_600_000;
        hlc.state.store(pack(an_hour_ago, 0), std::sync::atomic::Ordering::Release);

        let frozen = physical_ms(hlc.current());
        assert_eq!(frozen, an_hour_ago, "precondition: the clock is stale");

        let decided = hlc.decision_now_ms();
        assert!(
            decided >= frozen + 3_500_000,
            "decision_now_ms read a FROZEN clock ({decided} vs {frozen}): a deadline compared \
             against this never fires, so an expired mandate never expires and a stale checkpoint \
             never goes stale — it fails OPEN",
        );
    }

    /// It does not advance the clock — a read path must not consume logical-counter space or
    /// reorder anything.
    #[test]
    fn reading_the_decision_clock_does_not_tick_it() {
        let hlc = Hlc::new();
        let before = hlc.current();
        let _ = hlc.decision_now_ms();
        assert_eq!(hlc.current(), before, "a read must not mutate the clock");
    }

    /// And it stays in the HLC's frame: a peer time already observed pulls it **forward**, so two
    /// nodes comparing the same deadline are in one frame rather than two.
    #[test]
    fn an_observed_future_peer_time_pulls_the_decision_clock_forward() {
        let hlc = Hlc::new();
        let ahead = physical_ms(hlc.tick()) + 60_000;
        hlc.observe(pack(ahead, 0));
        assert!(
            hlc.decision_now_ms() >= ahead,
            "an observed peer time must floor the decision clock, not be ignored",
        );
    }
}
