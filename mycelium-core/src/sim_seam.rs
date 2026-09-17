//! The replay seams, as production reaches them — item 6 PR 3.
//!
//! # The shape, and why it is this shape
//!
//! Every nondeterministic read the coverage map assigns to the kernel gets a function here.
//! Production calls **this** rather than `SystemTime::now` directly, and the `sim` feature decides
//! what that means:
//!
//! - **without `sim`** (every shipped build): the body is the call it replaced, and nothing else.
//!   No indirection, no branch, no kernel — the compiler sees the same code it saw before.
//! - **with `sim`**: the read is routed through a thread-local [`Kernel`], so it is recorded and,
//!   on replay, checked and supplied.
//!
//! The asymmetry is deliberate. A harness that made production pay for its existence would be
//! refused on those grounds alone, and rightly: this is a testing mechanism, and the substrate owes
//! it nothing at runtime.
//!
//! # Why a thread-local and not a parameter
//!
//! Threading a kernel handle through `Hlc::tick` would put it in the signature of every caller of
//! every clock read — hundreds of sites, most of which have no idea time is involved. The kernel is
//! per-run and single-threaded by construction, so a thread-local is the honest representation of
//! what it already is. It also means a call that *forgets* the seam is invisible rather than a
//! compile error, which is exactly why `scripts/check-sim-seams.sh` exists.
//!
//! # What a divergence does here
//!
//! It panics, with both sides printed. A clock read cannot return "the replay departed", and a
//! replay that silently continued past a divergence would produce a run that is neither the
//! recording nor an honest fresh run. The kernel's contract is to *stop*, and in a harness a panic
//! is how you stop.
//!
//! # Five-part statement
//!
//! *Guarantee:* with no `sim` feature, this module compiles to the calls it replaced; with `sim`
//! and an installed kernel, every read routed through it is recorded and replayed. *Assumptions:*
//! the kernel is installed on the thread that will read, and one thread per simulated node.
//! *Enforcing component:* [`wall_now_ms`] and the thread-local install. *Failure behaviour:* no
//! kernel installed under `sim` falls back to the real clock (so an un-instrumented test still
//! runs); a divergence panics. *Detecting tests:* this module's `tests`, under `--features sim`.
//! *Strength:* `SelfImposedPrevention` over this repository's own harness.

/// The real wall clock — the call this seam replaced, kept in one place.
#[cfg(not(feature = "sim"))]
#[inline]
pub fn wall_now_ms() -> u64 {
    real_wall_now_ms()
}

#[inline]
fn real_wall_now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(feature = "sim")]
mod installed {
    use mycelium_sim::{Kernel, Seams, Sources};
    use std::cell::RefCell;

    /// One simulated node's kernel, for this thread.
    pub struct SimContext {
        /// The kernel doing the recording or the checking.
        pub kernel:  Kernel,
        /// Where recorded values come from.
        pub sources: Sources,
        /// Which node these reads belong to.
        pub node:    String,
        /// Bytes written per file, so an effect's request can carry *where* as well as *what*.
        /// An append landing at a different position is a different effect even with the same bytes.
        pub offsets: std::collections::HashMap<String, u64>,
    }

    thread_local! {
        static CTX: RefCell<Option<SimContext>> = const { RefCell::new(None) };
    }

    /// Install a kernel for this thread. Replaces any previous one and returns it.
    pub fn install(ctx: SimContext) -> Option<SimContext> {
        CTX.with(|c| c.borrow_mut().replace(ctx))
    }

    /// Remove and return this thread's kernel — how a test reads the trace back.
    pub fn take() -> Option<SimContext> {
        CTX.with(|c| c.borrow_mut().take())
    }

    /// Is a replay in progress? Asked *before* an `await`, because in replay the effect must not
    /// happen at all — the recorded outcome is supplied instead, and performing the write anyway
    /// would make the replay mutate state the recording already accounted for.
    pub fn is_replaying() -> bool {
        CTX.with(|c| {
            c.borrow()
                .as_ref()
                .is_some_and(|ctx| ctx.kernel.mode() == mycelium_sim::Mode::Replay)
        })
    }

    /// Record a storage effect that really happened.
    ///
    /// The offset is tracked per file so the request carries *where* as well as *what* — an append
    /// that lands at a different position is a different effect even with identical bytes.
    pub fn record_fs(file: &str, op: &str, bytes: &[u8], sync: bool, ok: bool, err: &str) {
        CTX.with(|c| {
            let mut guard = c.borrow_mut();
            let Some(ctx) = guard.as_mut() else { return };
            let offset = *ctx.offsets.get(file).unwrap_or(&0);
            let outcome = if ok {
                mycelium_sim::FsOutcome::Ok(bytes.len())
            } else {
                mycelium_sim::FsOutcome::Err(err.to_string())
            };
            let mut seams = Seams::new(&mut ctx.kernel, &mut ctx.sources, &ctx.node);
            if let Err(d) = seams.fs(file, op, bytes, offset, sync, || outcome) {
                panic!("{d}");
            }
            if ok {
                *ctx.offsets.entry(file.to_string()).or_insert(0) += bytes.len() as u64;
            }
        });
    }

    /// Supply a recorded storage effect during replay, checking the request first.
    pub fn replay_fs(file: &str, op: &str, bytes: &[u8], sync: bool) -> Result<(), String> {
        CTX.with(|c| {
            let mut guard = c.borrow_mut();
            let Some(ctx) = guard.as_mut() else { return Ok(()) };
            let offset = *ctx.offsets.get(file).unwrap_or(&0);
            let mut seams = Seams::new(&mut ctx.kernel, &mut ctx.sources, &ctx.node);
            let outcome = match seams.fs(file, op, bytes, offset, sync, || unreachable!()) {
                Ok(o) => o,
                Err(d) => panic!("{d}"),
            };
            match outcome {
                mycelium_sim::FsOutcome::Ok(n) | mycelium_sim::FsOutcome::Short(n) => {
                    *ctx.offsets.entry(file.to_string()).or_insert(0) += n as u64;
                    Ok(())
                }
                mycelium_sim::FsOutcome::Err(e) => Err(e),
            }
        })
    }

    /// Which mode the installed kernel is in, or `None` when there is no kernel.
    pub fn chan_mode() -> Option<mycelium_sim::Mode> {
        CTX.with(|c| c.borrow().as_ref().map(|ctx| ctx.kernel.mode()))
    }

    /// Write down what a bounded send actually did.
    pub fn chan_record(stream: &str, verdict: &str) {
        CTX.with(|c| {
            let mut guard = c.borrow_mut();
            let Some(ctx) = guard.as_mut() else { return };
            let mut seams = Seams::new(&mut ctx.kernel, &mut ctx.sources, &ctx.node);
            if let Err(d) = seams.chan(stream, "try_send", || verdict.to_string()) {
                panic!("{d}");
            }
        });
    }

    /// The verdict the recording gave this send.
    pub fn chan_replay(stream: &str) -> String {
        CTX.with(|c| {
            let mut guard = c.borrow_mut();
            let Some(ctx) = guard.as_mut() else { return String::new() };
            let mut seams = Seams::new(&mut ctx.kernel, &mut ctx.sources, &ctx.node);
            match seams.chan(stream, "try_send", || unreachable!()) {
                Ok(v) => v,
                Err(d) => panic!("{d}"),
            }
        })
    }

    /// Route one read through the kernel, or fall back to the real source when none is installed.
    ///
    /// The fallback matters: most tests in this repository do not install a kernel, and they must
    /// keep working unchanged. An un-instrumented test under `--features sim` reads the real clock,
    /// exactly as it always did.
    pub fn with_seams<T>(
        fallback: impl FnOnce() -> T,
        f: impl FnOnce(&mut Seams<'_>) -> Result<T, mycelium_sim::Divergence>,
    ) -> T {
        CTX.with(|c| {
            let mut guard = c.borrow_mut();
            let Some(ctx) = guard.as_mut() else { return fallback() };
            let mut seams = Seams::new(&mut ctx.kernel, &mut ctx.sources, &ctx.node);
            match f(&mut seams) {
                Ok(v) => v,
                // A clock read cannot return "the replay departed", and continuing past a
                // divergence yields a run that is neither the recording nor an honest fresh one.
                Err(d) => panic!("{d}"),
            }
        })
    }
}

#[cfg(feature = "sim")]
pub use installed::{install, take, SimContext};

/// The wall clock, through the kernel.
#[cfg(feature = "sim")]
#[inline]
pub fn wall_now_ms() -> u64 {
    installed::with_seams(real_wall_now_ms, |s| s.wall_now_ms())
}

// ── Randomness ───────────────────────────────────────────────────────────────────────────────
//
// Five named streams, from the inventory §2.2: `nonce`, `shed`, `jitter`, `select`, `govern`.
// Named rather than one generator because two subsystems sharing a generator are coupled — adding
// a draw in gossip shifts every later value in consensus, and a scenario replay of changed code
// then diverges *everywhere* instead of at the change.
//
// Without `sim` these are the `fastrand` calls they replaced, so the distribution a shipped build
// draws from is untouched. Under `sim` the value comes from the kernel's stream, and the mapping
// from a `u64` draw is documented at each site — a seam may change *where* a number comes from, but
// it must not quietly change *what kind* of number it is.

/// A nonce-style draw: a `u64` at or above `lo`.
#[cfg(not(feature = "sim"))]
#[inline]
pub fn rng_u64_from(_stream: &str, lo: u64) -> u64 {
    fastrand::u64(lo..)
}

/// A nonce-style draw, from the kernel's named stream.
///
/// The mapping folds the draw into `lo..=u64::MAX`. It is very slightly biased when `lo > 0`, which
/// is irrelevant for a nonce — the property that matters is uniqueness, not uniformity — and is
/// stated rather than hidden, because a seam that silently changed a distribution would be a bug
/// that only ever showed up as a statistical one.
#[cfg(feature = "sim")]
#[inline]
pub fn rng_u64_from(stream: &str, lo: u64) -> u64 {
    let draw = installed::with_seams(|| fastrand::u64(lo..), |s| s.rng_u64(stream));
    if lo == 0 {
        draw
    } else {
        lo.saturating_add(draw % (u64::MAX - lo + 1))
    }
}

/// A roll in `[0, 1)`.
#[cfg(not(feature = "sim"))]
#[inline]
pub fn rng_f32(_stream: &str) -> f32 {
    fastrand::f32()
}

/// A roll in `[0, 1)`, from the kernel's named stream.
///
/// Twenty-four bits over `2^24`, so the result is **never** `1.0` — `fastrand::f32()` is in `[0,1)`
/// and a shedding test pins exactly that (`ops.rs`: a fill of `0.0` must always shed). A seam that
/// could return `1.0` would break a property the production code already relies on.
#[cfg(feature = "sim")]
#[inline]
pub fn rng_f32(stream: &str) -> f32 {
    let draw = installed::with_seams(|| u64::from(fastrand::u32(..)) << 32, |s| s.rng_u64(stream));
    ((draw >> 40) as f32) / ((1u64 << 24) as f32)
}

/// An index below `n` — the shape every "pick one" and every shuffle step uses.
#[cfg(not(feature = "sim"))]
#[inline]
pub fn rng_usize_below(_stream: &str, n: usize) -> usize {
    fastrand::usize(..n)
}

/// An index below `n`, from the kernel's named stream.
///
/// `n == 0` returns `0` rather than panicking as `fastrand::usize(..0)` would: a seam is not the
/// place to change whether a caller's bug is a panic, but it is also not the place to introduce a
/// new one. Callers here always pass a non-empty length.
#[cfg(feature = "sim")]
#[inline]
pub fn rng_usize_below(stream: &str, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let draw = installed::with_seams(|| fastrand::usize(..n) as u64, |s| s.rng_u64(stream));
    (draw % n as u64) as usize
}

/// A `u64` below `n`.
#[cfg(not(feature = "sim"))]
#[inline]
pub fn rng_u64_below(_stream: &str, n: u64) -> u64 {
    fastrand::u64(0..n)
}

/// A `u64` below `n`, from the kernel's named stream.
#[cfg(feature = "sim")]
#[inline]
pub fn rng_u64_below(stream: &str, n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    installed::with_seams(|| fastrand::u64(0..n), |s| s.rng_u64(stream)) % n
}

/// Shuffle in place.
#[cfg(not(feature = "sim"))]
#[inline]
pub fn rng_shuffle<T>(_stream: &str, slice: &mut [T]) {
    fastrand::shuffle(slice);
}

/// Shuffle in place, from the kernel's named stream.
///
/// Fisher–Yates over kernel draws rather than `fastrand::shuffle`, because a shuffle has to be
/// *one draw per step* for a replay to check it: a single opaque call would record nothing the
/// kernel could compare, and a changed shuffle would replay as identical.
#[cfg(feature = "sim")]
pub fn rng_shuffle<T>(stream: &str, slice: &mut [T]) {
    if slice.len() < 2 {
        return;
    }
    for i in (1..slice.len()).rev() {
        let j = rng_usize_below(stream, i + 1);
        slice.swap(i, j);
    }
}

// ── Channels ─────────────────────────────────────────────────────────────────────────────────
//
// "Was the queue full" is a decision, not an accident. A full gossip shard drops a frame; a full WAL
// channel skips an append; both change what the node then does. The inventory puts capacity and
// fullness in the kernel so fullness becomes a *schedulable fault* rather than something a test
// hopes to provoke by timing.
//
// The shape differs from storage, and it has to. A file write in replay can be skipped — the disk is
// restored from the bundle. A channel send **cannot**: its effect is in-process and the replay is
// reproducing that process. So the kernel decides the verdict, and the call site honours it — the
// send is performed when and only when the recording says it happened.

/// What the kernel decided about a bounded send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChanVerdict {
    /// The message went into the channel.
    Sent,
    /// The channel was full. The message did not go in, and the caller takes its drop path.
    Full,
    /// The receiver is gone.
    Closed,
}

// Only the `sim` path encodes a verdict into a trace; in a shipped build these would be dead, and
// the --no-default-features clippy is the gate that says so.
#[cfg(feature = "sim")]
impl ChanVerdict {
    fn encode(self) -> String {
        match self {
            ChanVerdict::Sent => "Sent".into(),
            ChanVerdict::Full => "Full".into(),
            ChanVerdict::Closed => "Closed".into(),
        }
    }

    fn decode(s: &str) -> Self {
        match s {
            "Sent" => ChanVerdict::Sent,
            "Closed" => ChanVerdict::Closed,
            // An unreadable verdict is treated as `Full`: the conservative reading, because a
            // dropped frame is recoverable and a phantom send is not.
            _ => ChanVerdict::Full,
        }
    }
}

/// Send on a bounded channel, without the kernel.
#[cfg(not(feature = "sim"))]
#[inline]
pub fn chan_try_send<T>(
    _stream: &str,
    tx: &tokio::sync::mpsc::Sender<T>,
    msg: T,
) -> ChanVerdict {
    verdict_of(tx.try_send(msg))
}

/// Send on a bounded channel, through the kernel.
///
/// In `Record` the real send decides and is written down. In `Replay` the recorded verdict decides:
/// `Sent` performs the send, anything else does not — and the message is dropped, exactly as the
/// recording dropped it. A replayed `Sent` that finds the channel full is a divergence: the replay's
/// channel state has departed from the recording's, and stopping is better than quietly losing a
/// frame the recording delivered.
#[cfg(feature = "sim")]
pub fn chan_try_send<T>(stream: &str, tx: &tokio::sync::mpsc::Sender<T>, msg: T) -> ChanVerdict {
    match installed::chan_mode() {
        // No kernel: the real send, unchanged.
        None => verdict_of(tx.try_send(msg)),
        Some(mycelium_sim::Mode::Record) => {
            let verdict = verdict_of(tx.try_send(msg));
            installed::chan_record(stream, &verdict.encode());
            verdict
        }
        Some(mycelium_sim::Mode::Replay) => {
            let recorded = ChanVerdict::decode(&installed::chan_replay(stream));
            if recorded == ChanVerdict::Sent {
                // The recording delivered it, so the replay must too — a channel's effect is
                // in-process, and the replay is reproducing that process.
                let actual = verdict_of(tx.try_send(msg));
                assert!(
                    actual == ChanVerdict::Sent,
                    "replay diverged at chan {stream}: the recording sent, the replay got \
                     {actual:?} — the replay's channel state has departed from the recording's"
                );
            }
            // Recorded `Full`/`Closed`: `msg` is dropped here, which is what the recording did.
            recorded
        }
    }
}

/// The verdict a `try_send` result carries.
#[inline]
fn verdict_of<T>(r: Result<(), tokio::sync::mpsc::error::TrySendError<T>>) -> ChanVerdict {
    use tokio::sync::mpsc::error::TrySendError;
    match r {
        Ok(())                       => ChanVerdict::Sent,
        Err(TrySendError::Full(_))   => ChanVerdict::Full,
        Err(TrySendError::Closed(_)) => ChanVerdict::Closed,
    }
}

// ── Storage ──────────────────────────────────────────────────────────────────────────────────
//
// These wrap the three effects the durability argument turns on: the write, the file sync, and the
// *directory* sync that makes a rename survive a power loss (v2.4.4). Their **order** is the
// property — "fsync the directory before truncating the WAL" is a sequence, and a trace catches its
// removal even before the storage model of PR 4 can simulate the loss it prevents.
//
// What this does *not* yet do: model storage state. A replay checks the sequence and content of
// effects; it does not reconstruct the disk, so it cannot yet answer "what would a reader see after
// a power loss". That needs the three-layer model (process memory · page cache · durable ·
// directory metadata) and the fault injection in PR 4. Said here rather than left to be assumed.

/// `write_all`, recorded.
#[cfg(not(feature = "sim"))]
#[inline]
pub async fn fs_write_all(
    file: &mut tokio::fs::File,
    _name: &str,
    bytes: &[u8],
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt as _;
    file.write_all(bytes).await
}

/// `write_all`, through the kernel.
#[cfg(feature = "sim")]
pub async fn fs_write_all(
    file: &mut tokio::fs::File,
    name: &str,
    bytes: &[u8],
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt as _;
    // Checked before the `await`: in replay the write must not happen, or the replay mutates state
    // the recording already accounted for.
    if installed::is_replaying() {
        return installed::replay_fs(name, "write_all", bytes, false)
            .map_err(std::io::Error::other);
    }
    let res = file.write_all(bytes).await;
    installed::record_fs(
        name,
        "write_all",
        bytes,
        false,
        res.is_ok(),
        &res.as_ref().err().map(|e| e.to_string()).unwrap_or_default(),
    );
    res
}

/// A whole-file write.
#[cfg(not(feature = "sim"))]
#[inline]
pub async fn fs_write(
    path: &std::path::Path,
    _name: &str,
    bytes: &[u8],
) -> std::io::Result<()> {
    tokio::fs::write(path, bytes).await
}

/// A whole-file write, through the kernel.
#[cfg(feature = "sim")]
pub async fn fs_write(
    path: &std::path::Path,
    name: &str,
    bytes: &[u8],
) -> std::io::Result<()> {
    if installed::is_replaying() {
        return installed::replay_fs(name, "write", bytes, false).map_err(std::io::Error::other);
    }
    let res = tokio::fs::write(path, bytes).await;
    installed::record_fs(
        name,
        "write",
        bytes,
        false,
        res.is_ok(),
        &res.as_ref().err().map(|e| e.to_string()).unwrap_or_default(),
    );
    res
}

/// A rename — the step that publishes a snapshot, and whose durability needs the *directory* sync.
#[cfg(not(feature = "sim"))]
#[inline]
pub async fn fs_rename(
    from: &std::path::Path,
    to: &std::path::Path,
    _name: &str,
) -> std::io::Result<()> {
    tokio::fs::rename(from, to).await
}

/// A rename, through the kernel. The request carries both paths, because renaming *somewhere else*
/// is a different effect and a trace that recorded only the source would accept it.
#[cfg(feature = "sim")]
pub async fn fs_rename(
    from: &std::path::Path,
    to: &std::path::Path,
    name: &str,
) -> std::io::Result<()> {
    let both = format!("{}->{}", from.display(), to.display());
    if installed::is_replaying() {
        return installed::replay_fs(name, "rename", both.as_bytes(), false)
            .map_err(std::io::Error::other);
    }
    let res = tokio::fs::rename(from, to).await;
    installed::record_fs(
        name,
        "rename",
        both.as_bytes(),
        false,
        res.is_ok(),
        &res.as_ref().err().map(|e| e.to_string()).unwrap_or_default(),
    );
    res
}

/// Read a whole file.
#[cfg(not(feature = "sim"))]
#[inline]
pub async fn fs_read(path: &std::path::Path, _name: &str) -> std::io::Result<Vec<u8>> {
    tokio::fs::read(path).await
}

/// Read a whole file, **checked** against the recording rather than supplied by it.
///
/// # Why a read is checked and a write is supplied
///
/// A write's bytes are its *request* — the kernel already knows them, so a replay can skip the
/// effect entirely. A read's bytes are its *result*, and the trace is a line per decision: putting
/// a snapshot's contents in it would make the trace the disk image. The bundle already carries disk
/// images (`initial/`), so in replay the read really happens, against the restored state, and the
/// seam compares what came back with what was recorded.
///
/// That makes this a **divergence check on recovery**: *the recovery read returned different bytes
/// than the recording did* is exactly the v2.4.3 class of failure — where a read error was mapped
/// to an empty tail and acknowledged records were truncated away. A harness that supplied the
/// recorded bytes instead would have replayed straight past it.
#[cfg(feature = "sim")]
pub async fn fs_read(path: &std::path::Path, name: &str) -> std::io::Result<Vec<u8>> {
    let res = tokio::fs::read(path).await;
    // The result, canonically: what came back, not how long it was. Two snapshots of equal length
    // are different snapshots.
    let digest: Vec<u8> = match &res {
        Ok(bytes) => bytes.clone(),
        Err(_) => Vec::new(),
    };
    installed::record_fs(
        name,
        "read",
        &digest,
        false,
        res.is_ok(),
        &res.as_ref().err().map(|e| e.to_string()).unwrap_or_default(),
    );
    res
}

/// `sync_data` on a file.
#[cfg(not(feature = "sim"))]
#[inline]
pub async fn fs_sync_data(file: &tokio::fs::File, _name: &str) -> std::io::Result<()> {
    file.sync_data().await
}

/// `sync_data`, through the kernel. The empty byte slice is deliberate: a sync has no content, and
/// what makes it a distinct effect is the file it names and the flag.
#[cfg(feature = "sim")]
pub async fn fs_sync_data(file: &tokio::fs::File, name: &str) -> std::io::Result<()> {
    if installed::is_replaying() {
        return installed::replay_fs(name, "sync_data", &[], true)
            .map_err(std::io::Error::other);
    }
    let res = file.sync_data().await;
    installed::record_fs(
        name,
        "sync_data",
        &[],
        true,
        res.is_ok(),
        &res.as_ref().err().map(|e| e.to_string()).unwrap_or_default(),
    );
    res
}

/// `sync_all` on a directory — what makes a preceding `rename` survive a power loss.
#[cfg(not(feature = "sim"))]
#[inline]
pub async fn fs_sync_dir(dir: &tokio::fs::File, _name: &str) -> std::io::Result<()> {
    dir.sync_all().await
}

/// The directory sync, through the kernel. Recorded under its own op so the *ordering* property —
/// directory sync before WAL truncation — is visible in the trace and a divergence when removed.
#[cfg(feature = "sim")]
pub async fn fs_sync_dir(dir: &tokio::fs::File, name: &str) -> std::io::Result<()> {
    if installed::is_replaying() {
        return installed::replay_fs(name, "sync_dir", &[], true)
            .map_err(std::io::Error::other);
    }
    let res = dir.sync_all().await;
    installed::record_fs(
        name,
        "sync_dir",
        &[],
        true,
        res.is_ok(),
        &res.as_ref().err().map(|e| e.to_string()).unwrap_or_default(),
    );
    res
}

#[cfg(all(test, feature = "sim"))]
mod tests {
    use super::*;
    use mycelium_sim::{Kernel, Sources};

    fn install_recording() {
        installed::install(SimContext {
            kernel:  Kernel::recording(),
            sources: Sources::seeded(9, 1_789_000_000_000),
            node:    "n1".into(),
            offsets: Default::default(),
        });
    }

    #[test]
    fn a_recorded_clock_read_comes_from_the_kernel_and_is_written_down() {
        install_recording();
        let a = wall_now_ms();
        let b = wall_now_ms();
        let ctx = installed::take().expect("installed");

        assert_eq!(a, 1_789_000_000_000, "the seeded source, not the machine's clock");
        assert_eq!(b, a + 1, "and it advances deterministically");
        assert_eq!(ctx.kernel.trace().len(), 2, "both reads are in the trace");
    }

    /// The replay returns the recorded values, whatever the machine's clock says.
    #[test]
    fn a_replayed_clock_read_returns_the_recorded_value() {
        install_recording();
        let recorded = (wall_now_ms(), wall_now_ms());
        let ctx = installed::take().expect("installed");

        installed::install(SimContext {
            kernel:  Kernel::replaying(ctx.kernel.trace().clone()),
            // A different seed: if the value came from here rather than the trace, it would differ.
            sources: Sources::seeded(12345, 0),
            node:    "n1".into(),
            offsets: Default::default(),
        });
        assert_eq!((wall_now_ms(), wall_now_ms()), recorded);
        installed::take();
    }

    /// Most tests in this repository never install a kernel, and they must keep working.
    #[test]
    fn with_no_kernel_installed_the_real_clock_is_read() {
        installed::take();
        assert!(wall_now_ms() > 1_700_000_000_000, "a real epoch-ms reading");
    }

    /// **The reason the streams are named.** Two subsystems on one generator are coupled: a new
    /// draw in one shifts every later value in the other, and a scenario replay of changed code
    /// then diverges *everywhere* rather than at the change. Checked through the seam, not the
    /// kernel's API, because the seam is what production calls.
    #[test]
    fn a_nonce_draw_does_not_move_a_shedding_roll() {
        let roll = |extra_nonces: usize| {
            installed::install(SimContext {
                kernel:  Kernel::recording(),
                sources: Sources::seeded(4242, 0),
                node:    "n1".into(),
                offsets: Default::default(),
            });
            for _ in 0..extra_nonces {
                rng_u64_from("nonce", 1);
            }
            let r = rng_f32("shed");
            installed::take();
            r
        };
        assert_eq!(roll(0), roll(5), "five extra nonces must not change the shedding roll");
    }

    /// `fastrand::f32()` is in `[0,1)` and the shedding code relies on it: a fill of `0.0` must
    /// *always* shed, which is only true if the roll can never reach `1.0`. A seam that could
    /// return `1.0` would break a property production already depends on.
    #[test]
    fn a_shedding_roll_is_never_one() {
        installed::install(SimContext {
            kernel:  Kernel::recording(),
            sources: Sources::seeded(7, 0),
            node:    "n1".into(),
            offsets: Default::default(),
        });
        for _ in 0..2_000 {
            let r = rng_f32("shed");
            assert!((0.0..1.0).contains(&r), "roll {r} outside [0,1)");
        }
        installed::take();
    }

    /// A nonce is drawn at or above its floor — the property the call sites rely on
    /// (`fastrand::u64(1..)`: never zero).
    #[test]
    fn a_nonce_respects_its_floor() {
        installed::install(SimContext {
            kernel:  Kernel::recording(),
            sources: Sources::seeded(11, 0),
            node:    "n1".into(),
            offsets: Default::default(),
        });
        for _ in 0..1_000 {
            assert!(rng_u64_from("nonce", 1) >= 1, "a nonce must never be zero");
        }
        installed::take();
    }

    /// And the draws replay: a recorded nonce comes back as itself, not as a fresh number.
    #[test]
    fn recorded_draws_replay_as_themselves() {
        installed::install(SimContext {
            kernel:  Kernel::recording(),
            sources: Sources::seeded(3, 0),
            node:    "n1".into(),
            offsets: Default::default(),
        });
        let recorded: Vec<u64> = (0..4).map(|_| rng_u64_from("nonce", 1)).collect();
        let ctx = installed::take().expect("installed");

        installed::install(SimContext {
            kernel:  Kernel::replaying(ctx.kernel.trace().clone()),
            // A different seed: a fresh draw would differ, a replayed one cannot.
            sources: Sources::seeded(999, 0),
            node:    "n1".into(),
            offsets: Default::default(),
        });
        let replayed: Vec<u64> = (0..4).map(|_| rng_u64_from("nonce", 1)).collect();
        installed::take();
        assert_eq!(recorded, replayed);
    }

    /// A shuffle is **one draw per step**, not one opaque call. A single call would record nothing
    /// the kernel could compare, so a changed shuffle would replay as identical — which is the
    /// failure the whole harness exists to make impossible.
    #[test]
    fn a_shuffle_is_one_recorded_draw_per_step_and_replays_identically() {
        installed::install(SimContext {
            kernel:  Kernel::recording(),
            sources: Sources::seeded(5, 0),
            node:    "n1".into(),
            offsets: Default::default(),
        });
        let mut v: Vec<u32> = (0..8).collect();
        rng_shuffle("select", &mut v);
        let ctx = installed::take().expect("installed");
        let recorded = v.clone();

        // Seven steps for eight elements — Fisher-Yates, one draw each.
        assert_eq!(ctx.kernel.trace().len(), 7, "one draw per step, visible to the kernel");

        installed::install(SimContext {
            kernel:  Kernel::replaying(ctx.kernel.trace().clone()),
            sources: Sources::seeded(4242, 0), // a different seed: a fresh shuffle would differ
            node:    "n1".into(),
            offsets: Default::default(),
        });
        let mut again: Vec<u32> = (0..8).collect();
        rng_shuffle("select", &mut again);
        installed::take();
        assert_eq!(again, recorded, "the replayed shuffle is the recorded one");
    }

    /// A one-element slice draws nothing — there is no decision to record.
    #[test]
    fn a_trivial_shuffle_records_no_decision() {
        installed::install(SimContext {
            kernel:  Kernel::recording(),
            sources: Sources::seeded(5, 0),
            node:    "n1".into(),
            offsets: Default::default(),
        });
        let mut one = [7u8];
        rng_shuffle("select", &mut one);
        let mut none: [u8; 0] = [];
        rng_shuffle("select", &mut none);
        let ctx = installed::take().expect("installed");
        assert_eq!(ctx.kernel.trace().len(), 0);
    }

    /// A channel with `cap` slots, already filled to the brim when `full` is set.
    fn chan(cap: usize) -> (tokio::sync::mpsc::Sender<u8>, tokio::sync::mpsc::Receiver<u8>) {
        tokio::sync::mpsc::channel(cap)
    }

    /// **A dropped frame replays as a dropped frame.** Channel saturation is the kind of thing a
    /// test normally has to provoke by timing and then hope for; recorded, it is just a fact of the
    /// run — the replay reproduces the drop without the channel needing to be full again.
    #[test]
    fn a_full_channel_replays_as_full_even_when_the_replays_channel_has_room() {
        let (tx, _rx) = chan(1);
        tx.try_send(1).expect("fill the one slot");

        installed::install(SimContext {
            kernel:  Kernel::recording(),
            sources: Sources::seeded(2, 0),
            node:    "n1".into(),
            offsets: Default::default(),
        });
        // Genuinely full: the recording captures a real drop.
        assert_eq!(chan_try_send("gossip/shard2", &tx, 2), ChanVerdict::Full);
        let ctx = installed::take().expect("installed");

        // The replay's channel has room — and must still drop, because the recording did.
        let (tx2, _rx2) = chan(8);
        installed::install(SimContext {
            kernel:  Kernel::replaying(ctx.kernel.trace().clone()),
            sources: Sources::seeded(2, 0),
            node:    "n1".into(),
            offsets: Default::default(),
        });
        assert_eq!(chan_try_send("gossip/shard2", &tx2, 2), ChanVerdict::Full);
        installed::take();
        assert_eq!(tx2.capacity(), 8, "the replayed drop did not consume a slot");
    }

    /// A recorded send really delivers on replay — a channel's effect is in-process, and the replay
    /// is reproducing that process.
    #[test]
    fn a_recorded_send_really_delivers_on_replay() {
        let (tx, mut rx) = chan(4);
        installed::install(SimContext {
            kernel:  Kernel::recording(),
            sources: Sources::seeded(2, 0),
            node:    "n1".into(),
            offsets: Default::default(),
        });
        assert_eq!(chan_try_send("gossip/shard0", &tx, 7), ChanVerdict::Sent);
        let ctx = installed::take().expect("installed");
        assert_eq!(rx.try_recv().ok(), Some(7));

        let (tx2, mut rx2) = chan(4);
        installed::install(SimContext {
            kernel:  Kernel::replaying(ctx.kernel.trace().clone()),
            sources: Sources::seeded(2, 0),
            node:    "n1".into(),
            offsets: Default::default(),
        });
        assert_eq!(chan_try_send("gossip/shard0", &tx2, 7), ChanVerdict::Sent);
        installed::take();
        assert_eq!(rx2.try_recv().ok(), Some(7), "the replayed send must actually arrive");
    }

    /// Per-shard streams: a drop on shard 2 and a drop on shard 5 are different events, and a trace
    /// that merged them could not tell a reader which key stopped propagating.
    #[test]
    fn shards_are_separate_streams() {
        let (tx, _rx) = chan(4);
        installed::install(SimContext {
            kernel:  Kernel::recording(),
            sources: Sources::seeded(2, 0),
            node:    "n1".into(),
            offsets: Default::default(),
        });
        chan_try_send("gossip/shard2", &tx, 1);
        chan_try_send("gossip/shard5", &tx, 2);
        let ctx = installed::take().expect("installed");
        let streams: Vec<String> =
            ctx.kernel.trace().entries().iter().map(|e| e.stream.clone()).collect();
        assert_eq!(streams, vec!["gossip/shard2", "gossip/shard5"]);
    }

    /// The recording delivered a frame the replay cannot: the replay's channel state has departed,
    /// and that is a divergence rather than a frame quietly lost.
    #[test]
    #[should_panic(expected = "replay diverged at chan")]
    fn a_replayed_send_that_finds_the_channel_full_diverges() {
        let (tx, _rx) = chan(4);
        installed::install(SimContext {
            kernel:  Kernel::recording(),
            sources: Sources::seeded(2, 0),
            node:    "n1".into(),
            offsets: Default::default(),
        });
        chan_try_send("gossip/shard0", &tx, 1);
        let ctx = installed::take().expect("installed");

        // A replay whose channel is full where the recording's was not.
        let (tx2, _rx2) = chan(1);
        tx2.try_send(99).expect("fill it");
        installed::install(SimContext {
            kernel:  Kernel::replaying(ctx.kernel.trace().clone()),
            sources: Sources::seeded(2, 0),
            node:    "n1".into(),
            offsets: Default::default(),
        });
        chan_try_send("gossip/shard0", &tx2, 1);
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let d = std::env::temp_dir().join(format!(
            "sim-seam-{name}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("mkdir");
        d.join("wal.bin")
    }

    async fn open(path: &std::path::Path) -> tokio::fs::File {
        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .expect("open")
    }

    /// A write and its sync are two effects, and the trace says so in order. That ordering *is* the
    /// durability property: a record is durable only once the sync returns.
    #[tokio::test]
    async fn a_write_and_its_sync_are_two_ordered_effects_in_the_trace() {
        install_recording();
        let path = tmp("ordered");
        let mut f = open(&path).await;
        fs_write_all(&mut f, "wal.bin", b"record-one").await.expect("write");
        fs_sync_data(&f, "wal.bin").await.expect("sync");
        let ctx = installed::take().expect("installed");

        let ops: Vec<String> =
            ctx.kernel.trace().entries().iter().map(|e| e.request.clone()).collect();
        assert_eq!(ops.len(), 2);
        assert!(ops[0].starts_with("write_all"), "{:?}", ops[0]);
        assert!(ops[1].starts_with("sync_data"), "{:?}", ops[1]);
        assert!(ops[0].contains("off=0"), "the first append is at the start of the file");
    }

    /// **The property the trace buys us today.** Losing the sync is a divergence — before the
    /// storage model of PR 4 can simulate the power loss it would expose, the *sequence* already
    /// catches its removal.
    #[tokio::test]
    #[should_panic(expected = "replay diverged")]
    async fn dropping_the_sync_diverges_from_the_recording() {
        install_recording();
        let path = tmp("nosync");
        let mut f = open(&path).await;
        fs_write_all(&mut f, "wal.bin", b"record-one").await.expect("write");
        fs_sync_data(&f, "wal.bin").await.expect("sync");
        fs_write_all(&mut f, "wal.bin", b"record-two").await.expect("write");
        let ctx = installed::take().expect("installed");

        installed::install(SimContext {
            kernel:  Kernel::replaying(ctx.kernel.trace().clone()),
            sources: Sources::seeded(9, 0),
            node:    "n1".into(),
            offsets: Default::default(),
        });
        let path2 = tmp("nosync-replay");
        let mut f2 = open(&path2).await;
        fs_write_all(&mut f2, "wal.bin", b"record-one").await.expect("matches");
        // The sync is gone — exactly the regression v2.4.4 guards against, one layer up.
        let _ = fs_write_all(&mut f2, "wal.bin", b"record-two").await;
    }

    /// Replay supplies the recorded outcome and **does not perform the write** — checked by
    /// replaying against a file that was never created: a real write would fail, a replayed one
    /// cannot.
    #[tokio::test]
    async fn a_replayed_write_does_not_touch_the_disk() {
        install_recording();
        let path = tmp("noio");
        let mut f = open(&path).await;
        fs_write_all(&mut f, "wal.bin", b"payload").await.expect("write");
        let ctx = installed::take().expect("installed");
        drop(f);

        installed::install(SimContext {
            kernel:  Kernel::replaying(ctx.kernel.trace().clone()),
            sources: Sources::seeded(9, 0),
            node:    "n1".into(),
            offsets: Default::default(),
        });
        // A file opened read-only: a genuine `write_all` would return EBADF.
        let mut ro = tokio::fs::File::open(&path).await.expect("reopen read-only");
        fs_write_all(&mut ro, "wal.bin", b"payload").await.expect("the replay does not write");
        installed::take();
    }

    /// Bytes that differ at the same length are a different write — the gate, reached through
    /// production's own path this time rather than the kernel's API.
    #[tokio::test]
    #[should_panic(expected = "replay diverged")]
    async fn changed_content_at_equal_length_diverges_through_the_production_path() {
        install_recording();
        let path = tmp("content");
        let mut f = open(&path).await;
        fs_write_all(&mut f, "wal.bin", b"AAAA").await.expect("write");
        let ctx = installed::take().expect("installed");

        installed::install(SimContext {
            kernel:  Kernel::replaying(ctx.kernel.trace().clone()),
            sources: Sources::seeded(9, 0),
            node:    "n1".into(),
            offsets: Default::default(),
        });
        let mut f2 = open(&tmp("content-replay")).await;
        let _ = fs_write_all(&mut f2, "wal.bin", b"BBBB").await;
    }

    /// **A recovery read that returns different bytes is a divergence.** This is the v2.4.3 class:
    /// a read whose result changed — there, an error mapped to an empty tail, so acknowledged
    /// records were truncated away. A harness that *supplied* the recorded bytes would have replayed
    /// straight past it; one that checks them stops.
    #[tokio::test]
    #[should_panic(expected = "replay diverged")]
    async fn a_recovery_read_returning_different_bytes_diverges() {
        let path = tmp("read");
        tokio::fs::write(&path, b"snapshot-AAAA").await.expect("write");

        install_recording();
        let got = fs_read(&path, "snapshot.bin").await.expect("read");
        assert_eq!(got, b"snapshot-AAAA");
        let ctx = installed::take().expect("installed");

        // Same length, different content — the disk changed under the replay.
        tokio::fs::write(&path, b"snapshot-BBBB").await.expect("rewrite");
        installed::install(SimContext {
            kernel:  Kernel::replaying(ctx.kernel.trace().clone()),
            sources: Sources::seeded(9, 0),
            node:    "n1".into(),
            offsets: Default::default(),
        });
        let _ = fs_read(&path, "snapshot.bin").await;
    }

    /// The same bytes replay cleanly — the check is on content, not on having read at all.
    #[tokio::test]
    async fn an_identical_recovery_read_replays_without_diverging() {
        let path = tmp("read-same");
        tokio::fs::write(&path, b"snapshot-AAAA").await.expect("write");

        install_recording();
        fs_read(&path, "snapshot.bin").await.expect("read");
        let ctx = installed::take().expect("installed");

        installed::install(SimContext {
            kernel:  Kernel::replaying(ctx.kernel.trace().clone()),
            sources: Sources::seeded(9, 0),
            node:    "n1".into(),
            offsets: Default::default(),
        });
        let again = fs_read(&path, "snapshot.bin").await.expect("read");
        installed::take();
        assert_eq!(again, b"snapshot-AAAA");
    }

    /// A replay asked for a read the recording never made: the run stops rather than inventing one.
    #[test]
    #[should_panic(expected = "replay diverged")]
    fn a_read_the_recording_never_made_stops_the_run() {
        install_recording();
        wall_now_ms();
        let ctx = installed::take().expect("installed");

        installed::install(SimContext {
            kernel:  Kernel::replaying(ctx.kernel.trace().clone()),
            sources: Sources::seeded(9, 0),
            node:    "n1".into(),
            offsets: Default::default(),
        });
        wall_now_ms(); // matches the one recorded read
        wall_now_ms(); // one more than was recorded — diverges
    }
}
