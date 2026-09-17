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
pub(crate) fn wall_now_ms() -> u64 {
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
pub(crate) fn wall_now_ms() -> u64 {
    installed::with_seams(real_wall_now_ms, |s| s.wall_now_ms())
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
pub(crate) async fn fs_write_all(
    file: &mut tokio::fs::File,
    _name: &str,
    bytes: &[u8],
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt as _;
    file.write_all(bytes).await
}

/// `write_all`, through the kernel.
#[cfg(feature = "sim")]
pub(crate) async fn fs_write_all(
    file: &mut tokio::fs::File,
    name: &str,
    bytes: &[u8],
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt as _;
    // Checked before the `await`: in replay the write must not happen, or the replay mutates state
    // the recording already accounted for.
    if installed::is_replaying() {
        return installed::replay_fs(name, "write_all", bytes, false)
            .map_err(|e| std::io::Error::other(e));
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
pub(crate) async fn fs_write(
    path: &std::path::Path,
    _name: &str,
    bytes: &[u8],
) -> std::io::Result<()> {
    tokio::fs::write(path, bytes).await
}

/// A whole-file write, through the kernel.
#[cfg(feature = "sim")]
pub(crate) async fn fs_write(
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
pub(crate) async fn fs_rename(
    from: &std::path::Path,
    to: &std::path::Path,
    _name: &str,
) -> std::io::Result<()> {
    tokio::fs::rename(from, to).await
}

/// A rename, through the kernel. The request carries both paths, because renaming *somewhere else*
/// is a different effect and a trace that recorded only the source would accept it.
#[cfg(feature = "sim")]
pub(crate) async fn fs_rename(
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

/// `sync_data` on a file.
#[cfg(not(feature = "sim"))]
#[inline]
pub(crate) async fn fs_sync_data(file: &tokio::fs::File, _name: &str) -> std::io::Result<()> {
    file.sync_data().await
}

/// `sync_data`, through the kernel. The empty byte slice is deliberate: a sync has no content, and
/// what makes it a distinct effect is the file it names and the flag.
#[cfg(feature = "sim")]
pub(crate) async fn fs_sync_data(file: &tokio::fs::File, name: &str) -> std::io::Result<()> {
    if installed::is_replaying() {
        return installed::replay_fs(name, "sync_data", &[], true)
            .map_err(|e| std::io::Error::other(e));
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
pub(crate) async fn fs_sync_dir(dir: &tokio::fs::File, _name: &str) -> std::io::Result<()> {
    dir.sync_all().await
}

/// The directory sync, through the kernel. Recorded under its own op so the *ordering* property —
/// directory sync before WAL truncation — is visible in the trace and a divergence when removed.
#[cfg(feature = "sim")]
pub(crate) async fn fs_sync_dir(dir: &tokio::fs::File, name: &str) -> std::io::Result<()> {
    if installed::is_replaying() {
        return installed::replay_fs(name, "sync_dir", &[], true)
            .map_err(|e| std::io::Error::other(e));
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
