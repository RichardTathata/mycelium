//! The seams production code reaches the kernel through: clocks, named RNG streams, storage.
//!
//! # Why the two clocks are separate
//!
//! Wall time and monotonic time are different seams, not one with two names. Wall time **jumps** —
//! an operator corrects it, NTP steps it, a container resumes — and a lease that treats a jump as
//! elapsed time expires early. Monotonic time does not jump and cannot be compared across
//! processes. Code that reads one when it meant the other is a real bug class, and a harness that
//! recorded them as one `time` seam could not replay the jump that exposes it.
//!
//! # Why the RNG is named streams rather than one generator
//!
//! Two subsystems drawing from one generator are coupled: adding a draw in gossip shifts every
//! later value in consensus, and a replay of *changed code* diverges everywhere instead of at the
//! change. Named streams keep a draw local to the thing that drew it, so scenario replay attributes
//! a divergence to the code that actually moved.
//!
//! # The canonical request for a write
//!
//! [`fs_request`] is where PR 2's gate lives. A write's request carries the **content hash of the
//! bytes**, the target, the offset, and the flags that change the write's meaning — never the
//! length alone. Two WAL records of equal length are a different write; a trace recording `len=214`
//! would accept changed content as a faithful replay, which is a harness that re-seeds rather than
//! one that detects divergence.

use crate::kernel::{Divergence, Kernel};
use crate::trace::ChoiceKind;

/// Deterministic sources for a recorded run.
///
/// A recording still has to get its values from somewhere. In exploration these come from a seed,
/// so a run is reproducible before it has ever been recorded; PR 3's adapters will let production's
/// real clock and RNG flow through the same seams instead.
#[derive(Clone, Debug)]
pub struct Sources {
    wall_ms:  u64,
    mono_ns:  u64,
    seed:     u64,
    /// How far the wall clock advances per read. Zero is legitimate and worth testing: two reads in
    /// the same millisecond is the common case, and code that assumes time always moves is wrong.
    pub wall_step_ms: u64,
    /// How far the monotonic clock advances per read.
    pub mono_step_ns: u64,
}

impl Sources {
    /// Sources seeded for exploration. `wall_ms` is the run's starting wall time.
    pub fn seeded(seed: u64, wall_ms: u64) -> Self {
        Self { wall_ms, mono_ns: 0, seed, wall_step_ms: 1, mono_step_ns: 1_000_000 }
    }

    /// Advance and read the wall clock.
    fn wall(&mut self) -> u64 {
        let now = self.wall_ms;
        self.wall_ms = self.wall_ms.saturating_add(self.wall_step_ms);
        now
    }

    /// Advance and read the monotonic clock, in nanoseconds since the run began.
    fn mono(&mut self) -> u64 {
        let now = self.mono_ns;
        self.mono_ns = self.mono_ns.saturating_add(self.mono_step_ns);
        now
    }

    /// Step the wall clock without a read — a clock jump, forward or back.
    ///
    /// Back is the case that matters: a lease holder that treats a backward jump as elapsed time
    /// expires early, and that is only reachable in a test if the harness can do this.
    pub fn jump_wall_ms(&mut self, delta: i64) {
        self.wall_ms = self.wall_ms.saturating_add_signed(delta);
    }

    /// A draw from `stream`. Streams are independent: adding a draw in one does not move another.
    fn rng(&mut self, stream: &str, counter: u64) -> u64 {
        // splitmix64 over (seed, stream hash, counter) — no dependency, and a stream's values do
        // not depend on how many draws other streams made.
        let mut z = self
            .seed
            .wrapping_add(fnv1a(stream).wrapping_mul(0x9E37_79B9_7F4A_7C15))
            .wrapping_add(counter.wrapping_mul(0xBF58_476D_1CE4_E5B9));
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

/// What a storage effect returned to the production code.
///
/// `Short` exists because a partial completion is neither success nor failure, and the durability
/// argument turns on it: bytes were written, fewer than asked, and the caller has to decide what it
/// now believes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FsOutcome {
    /// All requested bytes were accepted.
    Ok(usize),
    /// Some bytes were accepted — the short write.
    Short(usize),
    /// The effect failed, with the error the caller saw.
    Err(String),
}

impl FsOutcome {
    fn encode(&self) -> String {
        match self {
            FsOutcome::Ok(n) => format!("Ok({n})"),
            FsOutcome::Short(n) => format!("Short wrote={n}"),
            FsOutcome::Err(e) => format!("Err({e})"),
        }
    }

    fn decode(s: &str) -> Option<Self> {
        if let Some(rest) = s.strip_prefix("Ok(").and_then(|r| r.strip_suffix(')')) {
            return rest.parse().ok().map(FsOutcome::Ok);
        }
        if let Some(rest) = s.strip_prefix("Short wrote=") {
            return rest.parse().ok().map(FsOutcome::Short);
        }
        if let Some(rest) = s.strip_prefix("Err(").and_then(|r| r.strip_suffix(')')) {
            return Some(FsOutcome::Err(rest.to_string()));
        }
        None
    }
}

/// The canonical request for a write — **content hash, target, offset, flags**, never length alone.
///
/// Public because PR 3's adapters must produce exactly this string; a second spelling of the same
/// request would replay as a divergence against a trace that is in fact identical.
pub fn fs_request(op: &str, bytes: &[u8], offset: u64, sync: bool) -> String {
    format!(
        "{op} d=sha256:{} len={} sync={sync} off={offset}",
        hex16(&sha256(bytes)),
        bytes.len()
    )
}

/// Production's view of the kernel, for one node.
pub struct Seams<'k> {
    kernel:   &'k mut Kernel,
    sources:  &'k mut Sources,
    node:     String,
    rng_seq:  std::collections::HashMap<String, u64>,
}

impl<'k> Seams<'k> {
    /// The seams for `node`.
    pub fn new(kernel: &'k mut Kernel, sources: &'k mut Sources, node: &str) -> Self {
        Self { kernel, sources, node: node.to_string(), rng_seq: Default::default() }
    }

    /// Read the wall clock, in epoch milliseconds.
    pub fn wall_now_ms(&mut self) -> Result<u64, Divergence> {
        let sources = &mut *self.sources;
        let out = self.kernel.decide(
            Some(&self.node),
            ChoiceKind::Wall,
            "-",
            "now_ms()",
            || sources.wall().to_string(),
        )?;
        Ok(out.parse().unwrap_or(0))
    }

    /// Read the monotonic clock, in nanoseconds since the run began.
    pub fn mono_now_ns(&mut self) -> Result<u64, Divergence> {
        let sources = &mut *self.sources;
        let out = self.kernel.decide(
            Some(&self.node),
            ChoiceKind::Mono,
            "-",
            "now()",
            || sources.mono().to_string(),
        )?;
        Ok(out.parse().unwrap_or(0))
    }

    /// Draw from a named RNG stream.
    pub fn rng_u64(&mut self, stream: &str) -> Result<u64, Divergence> {
        let counter = self.rng_seq.entry(stream.to_string()).or_insert(0);
        let n = *counter;
        *counter += 1;
        let sources = &mut *self.sources;
        let out = self.kernel.decide(
            Some(&self.node),
            ChoiceKind::Rng,
            stream,
            "draw(u64)",
            || format!("0x{:016x}", sources.rng(stream, n)),
        )?;
        Ok(u64::from_str_radix(out.trim_start_matches("0x"), 16).unwrap_or(0))
    }

    /// A storage effect.
    ///
    /// `observed` supplies what really happened in `Record`; in `Replay` it is not called and the
    /// recorded outcome is returned. Storage *adapters* — the fault injection, the page-cache and
    /// directory layers — are PR 3; this is the seam they will speak through.
    pub fn fs<F>(
        &mut self,
        file: &str,
        op: &str,
        bytes: &[u8],
        offset: u64,
        sync: bool,
        observed: F,
    ) -> Result<FsOutcome, Divergence>
    where
        F: FnOnce() -> FsOutcome,
    {
        let request = fs_request(op, bytes, offset, sync);
        let out = self.kernel.decide(
            Some(&self.node),
            ChoiceKind::Fs,
            file,
            &request,
            || observed().encode(),
        )?;
        Ok(FsOutcome::decode(&out).unwrap_or(FsOutcome::Err("unparseable recorded outcome".into())))
    }

    /// A bounded-channel send, and what the kernel decided about it.
    ///
    /// Capacity and fullness are *kernel state*: "the queue was full" drops a frame or skips a WAL
    /// append, so it is a schedulable fault, not an accident of timing to be reproduced by luck.
    pub fn chan(
        &mut self,
        stream: &str,
        request: &str,
        observed: impl FnOnce() -> String,
    ) -> Result<String, Divergence> {
        self.kernel.decide(Some(&self.node), ChoiceKind::Chan, stream, request, observed)
    }

    /// An external input — a token verification, an LLM reply, an MCP response.
    ///
    /// Inputs and faults are the *causal workload*: scenario replay keeps these and lets the kernel
    /// re-derive scheduling, timers and RNG under changed code.
    pub fn input<F>(&mut self, seam: &str, request: &str, observed: F) -> Result<String, Divergence>
    where
        F: FnOnce() -> String,
    {
        self.kernel.decide(Some(&self.node), ChoiceKind::Input, seam, request, observed)
    }
}

// ── A small SHA-256, so the harness has no dependency it does not need ───────────────────────

fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).into()
}

/// First 16 hex characters of a digest — enough to name a write in a trace a human reads, and the
/// full digest is recoverable from the bundle's inputs.
fn hex16(d: &[u8; 32]) -> String {
    use std::fmt::Write as _;
    d.iter().take(8).fold(String::with_capacity(16), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seams<'a>(k: &'a mut Kernel, s: &'a mut Sources) -> Seams<'a> {
        Seams::new(k, s, "n1")
    }

    #[test]
    fn the_two_clocks_are_separate_seams() {
        let mut k = Kernel::recording();
        let mut s = Sources::seeded(7, 1_789_000_000_000);
        let mut n = seams(&mut k, &mut s);
        let wall = n.wall_now_ms().unwrap();
        let mono = n.mono_now_ns().unwrap();
        assert_eq!(wall, 1_789_000_000_000);
        assert_eq!(mono, 0, "monotonic time starts at the run, not at the epoch");

        let kinds: Vec<_> = k.trace().entries().iter().map(|e| e.kind).collect();
        assert_eq!(kinds, vec![ChoiceKind::Wall, ChoiceKind::Mono]);
    }

    /// A backward jump is the case that matters: a lease holder treating it as elapsed time expires
    /// early, and that is only reachable if the harness can do this.
    #[test]
    fn the_wall_clock_can_jump_backwards_and_the_monotonic_one_cannot_be_asked_to() {
        let mut k = Kernel::recording();
        let mut s = Sources::seeded(7, 1_000_000);
        s.wall_step_ms = 0;
        let mut n = seams(&mut k, &mut s);
        assert_eq!(n.wall_now_ms().unwrap(), 1_000_000);
        drop(n);

        s.jump_wall_ms(-500);
        let mut n = seams(&mut k, &mut s);
        assert_eq!(n.wall_now_ms().unwrap(), 999_500);
    }

    /// Adding a draw in one stream must not move another, or scenario replay of changed code
    /// diverges everywhere instead of at the change.
    #[test]
    fn named_streams_are_independent() {
        let take = |extra: bool| {
            let mut k = Kernel::recording();
            let mut s = Sources::seeded(42, 0);
            let mut n = seams(&mut k, &mut s);
            if extra {
                n.rng_u64("gossip").unwrap();
            }
            n.rng_u64("consensus").unwrap()
        };
        assert_eq!(take(false), take(true), "a draw in gossip must not move consensus");
    }

    #[test]
    fn a_stream_advances_within_itself() {
        let mut k = Kernel::recording();
        let mut s = Sources::seeded(42, 0);
        let mut n = seams(&mut k, &mut s);
        assert_ne!(n.rng_u64("nonce").unwrap(), n.rng_u64("nonce").unwrap());
    }

    /// **The request carries content, not length.** This is the property PR 2's gate turns on.
    #[test]
    fn two_writes_of_equal_length_are_different_requests() {
        let a = fs_request("append", b"AAAA", 0, true);
        let b = fs_request("append", b"BBBB", 0, true);
        assert_ne!(a, b, "equal length, different bytes — a different write");
        assert!(a.contains("len=4") && a.contains("sync=true") && a.contains("off=0"));
    }

    #[test]
    fn a_flipped_sync_flag_or_a_moved_offset_is_a_different_request() {
        let base = fs_request("append", b"x", 0, true);
        assert_ne!(base, fs_request("append", b"x", 0, false), "sync changes the meaning");
        assert_ne!(base, fs_request("append", b"x", 8, true), "so does the offset");
    }

    #[test]
    fn a_short_write_survives_the_trace_as_a_short_write() {
        // Neither success nor failure: the durability argument turns on it.
        let mut k = Kernel::recording();
        let mut s = Sources::seeded(1, 0);
        let mut n = seams(&mut k, &mut s);
        let out = n.fs("wal.bin", "append", b"payload", 8192, true, || FsOutcome::Short(96)).unwrap();
        assert_eq!(out, FsOutcome::Short(96));

        let trace = k.trace().clone();
        let mut r = Kernel::replaying(trace);
        let mut s2 = Sources::seeded(1, 0);
        let mut n = Seams::new(&mut r, &mut s2, "n1");
        let replayed =
            n.fs("wal.bin", "append", b"payload", 8192, true, || unreachable!()).unwrap();
        assert_eq!(replayed, FsOutcome::Short(96), "the partial completion replays as itself");
    }

    #[test]
    fn every_outcome_round_trips() {
        for o in [FsOutcome::Ok(214), FsOutcome::Short(96), FsOutcome::Err("EIO".into())] {
            assert_eq!(FsOutcome::decode(&o.encode()), Some(o));
        }
    }
}
