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

/// The monotonic clock, in nanoseconds since this process first read it.
///
/// **Not the wall clock, and the difference is a correctness property rather than a preference.**
/// Every site this replaces measures an *interval* — how long since the rate window opened, how long
/// since the last failure. `Instant` is monotonic, so a backwards NTP step cannot make an interval
/// negative or enormous; `SystemTime` gives no such guarantee. Routing these through `wall_now_ms`
/// would have been one function fewer and a new class of bug: a rate window that never expires, a
/// backoff that fires instantly.
///
/// `Instant` has no epoch, so the seam supplies one — the first read — which is exactly what
/// `Seams::mono_now_ns` already means by "since the run began".
#[cfg(not(feature = "sim"))]
#[inline]
pub fn mono_now_ns() -> u64 {
    real_mono_now_ns()
}

/// Where the monotonic clock starts counting, so that **a point before process start is
/// representable**.
///
/// `Instant` has this property and a bare "nanoseconds since we started" does not.
///
/// **The caller that originally motivated this no longer needs it.** `SignalLog::seed` reconstructs
/// an entry that arrived `age_ms` ago and is called at startup; it briefly used this clock, and now
/// uses `Instant` again (the public types went back, and `Instant::checked_sub` handles it
/// natively). The offset is kept because the property is still real for any future caller that
/// subtracts a window from a fresh reading — and because a clock whose zero is reachable is a
/// clamping bug waiting for its first user. Recorded rather than silently retained: this rationale
/// used to name `seed` as the reason, and that is no longer true.
///
/// A year of headroom, which is more than any configured window (`signal_window_secs` defaults to
/// 600 s) and leaves ~583 years of runtime before a `u64` of nanoseconds runs out. It is deliberately
/// not an epoch: these values are only ever compared with each other.
pub const MONO_ORIGIN_NS: u64 = 365 * 24 * 60 * 60 * 1_000_000_000;

#[inline]
fn real_mono_now_ns() -> u64 {
    static ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    MONO_ORIGIN_NS + ORIGIN.get_or_init(std::time::Instant::now).elapsed().as_nanos() as u64
}

/// How long since a reading taken by [`mono_now_ns`] — the replacement for `Instant::elapsed`.
///
/// It exists so a call site reads the way it read before (`mono_since(t) < cooldown` against
/// `t.elapsed() < cooldown`); a conversion that made the surrounding code harder to follow would
/// have traded a real property for a testing one.
///
/// Saturating, because the readings are monotonic by construction and a replay supplies them from
/// the trace — "earlier is actually later" cannot happen, and saturating is how that is spelled.
/// A wrapping subtraction would express the same impossibility as *five centuries elapsed*, which a
/// cooldown would read as long expired.
#[inline]
pub fn mono_since(earlier_ns: u64) -> std::time::Duration {
    mono_between(earlier_ns, mono_now_ns())
}

/// How long since `at` — the kernel-owned replacement for `Instant::elapsed()`.
///
/// # Why this exists beside [`mono_since`]
///
/// `mono_since` needs its argument to be a seam reading, which means the *stored type* has to
/// change wherever it is used. For a public field like `CoreCtx::peers` that is an API break, and
/// the break buys nothing: **what a replay must reproduce is the decision, not the representation.**
/// Every staleness question is "how long since this", so recording *that* is sufficient — the
/// `Instant` itself never needs to be reproduced, only the interval derived from it.
///
/// So the map keeps real `Instant`s, and every read of their age goes through here. Under a kernel
/// the duration comes from the trace; without one it is `at.elapsed()` and nothing has changed.
#[cfg(not(feature = "sim"))]
#[inline]
pub fn mono_elapsed(at: &std::time::Instant) -> std::time::Duration {
    at.elapsed()
}

/// How long since `at`, through the kernel. See the no-`sim` arm for why the interval is recorded
/// rather than the instant.
#[cfg(feature = "sim")]
pub fn mono_elapsed(at: &std::time::Instant) -> std::time::Duration {
    std::time::Duration::from_nanos(installed::with_seams(
        || at.elapsed().as_nanos() as u64,
        |s| s.mono_now_ns(),
    ))
}

/// An `Instant` to *store* — the kernel-owned replacement for `Instant::now()` at a site that keeps
/// a real `Instant` in a public field (a `CatalogObservation::observed_at`, a peer's last-seen).
///
/// Deliberately identical in both arms: per [`mono_elapsed`], **what a replay must reproduce is
/// the decision, not the representation** — the instant itself is never compared or read except
/// through `mono_elapsed` / `mono_before` / `mono_span`, which are where the kernel supplies the
/// interval. This function exists so the construction site is the seam's, not the caller's: the
/// static check (`scripts/check-sim-seams.sh`) then sees no `Instant::now` outside this file, and
/// the invariant *every read of an instant's age goes through the seam* is the one a reviewer has
/// to check, at the read sites, rather than a count of constructions.
#[inline]
pub fn mono_instant() -> std::time::Instant {
    std::time::Instant::now()
}

/// Has `a` not yet reached `b`? — the kernel-owned replacement for `a < b` on two `Instant`s.
///
/// # Why a comparison needs a seam at all
///
/// [`mono_elapsed`] covers "how old is this", but three decisions compare two stamps the process
/// took at different moments — suppression expiry, the sender-log trim cutoff, and peer eviction.
/// Both sides are real `Instant`s, so a replay that re-took them would compare *its own* elapsed
/// wall time, not the recording's, and reach a different answer.
///
/// Recording the **verdict** fixes that without touching the stored type: the boolean is what the
/// code branches on, so the boolean is what the kernel owns. The same principle as everywhere else
/// in this module — reproduce the decision, not the representation.
#[cfg(not(feature = "sim"))]
#[inline]
pub fn mono_before(a: &std::time::Instant, b: &std::time::Instant) -> bool {
    a < b
}

/// Has `a` not yet reached `b`, through the kernel?
#[cfg(feature = "sim")]
pub fn mono_before(a: &std::time::Instant, b: &std::time::Instant) -> bool {
    installed::with_seams(|| u64::from(a < b), |s| s.mono_now_ns()) != 0
}

/// The interval between two `Instant`s the caller already holds — the kernel-owned replacement for
/// `Instant::duration_since`.
///
/// Same argument as [`mono_elapsed`]: the decision depends on the interval, so the interval is what
/// the kernel owns. Saturating, exactly as `duration_since` is.
#[cfg(not(feature = "sim"))]
#[inline]
pub fn mono_span(earlier: &std::time::Instant, later: &std::time::Instant) -> std::time::Duration {
    later.saturating_duration_since(*earlier)
}

/// The interval between two `Instant`s, through the kernel.
#[cfg(feature = "sim")]
pub fn mono_span(earlier: &std::time::Instant, later: &std::time::Instant) -> std::time::Duration {
    std::time::Duration::from_nanos(installed::with_seams(
        || later.saturating_duration_since(*earlier).as_nanos() as u64,
        |s| s.mono_now_ns(),
    ))
}

/// The interval between two readings from [`mono_now_ns`] — the replacement for
/// `Instant::duration_since`, for the callers that are handed both ends.
///
/// Saturating for the same reason [`mono_since`] is, and it matches what it replaced:
/// `Instant::duration_since` also saturates to zero rather than panicking when the arguments are
/// the wrong way round.
#[inline]
pub fn mono_between(earlier_ns: u64, later_ns: u64) -> std::time::Duration {
    std::time::Duration::from_nanos(later_ns.saturating_sub(earlier_ns))
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
        /// Set by [`super::pause_clock_for_replay`]: this thread's replay may honour each recorded
        /// wait on tokio's paused clock instead of yielding once. See that function for why the
        /// two facts are set together rather than separately.
        static CLOCK_PAUSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    /// Install a kernel for this thread. Replaces any previous one and returns it.
    pub fn install(ctx: SimContext) -> Option<SimContext> {
        CTX.with(|c| c.borrow_mut().replace(ctx))
    }

    /// Remove and return this thread's kernel — how a test reads the trace back. Also disarms the
    /// paused-clock replay: the kernel and the clock discipline arrive together and leave together,
    /// so a later test on this thread cannot inherit half of it.
    pub fn take() -> Option<SimContext> {
        set_clock_paused(false);
        CTX.with(|c| c.borrow_mut().take())
    }

    pub fn set_clock_paused(paused: bool) {
        CLOCK_PAUSED.with(|p| p.set(paused));
    }

    /// May a replayed wait advance tokio's paused clock on this thread?
    pub fn clock_paused() -> bool {
        CLOCK_PAUSED.with(|p| p.get())
    }

    /// Put a storage effect through the kernel, and return the outcome the kernel decides.
    ///
    /// **The effect really happens in both modes**, and the caller has already performed it when it
    /// calls this. In `Record` the kernel writes down what happened; in `Replay` it checks the
    /// request against the recorded one and hands back the *recorded* outcome, which is how an
    /// injected fault replays as a fault rather than as whatever the replay's disk did.
    ///
    /// # Why the effect is not suppressed in replay (item 6 PR 4)
    ///
    /// It used to be: a write's bytes are its request, so the kernel already knows them and the
    /// write looked redundant. That reasoning has a hole, and the WAL/snapshot scenario is standing
    /// in it — **a run that reads its own writes**. `do_snapshot` reads the WAL tail that
    /// `wal_append` wrote earlier in the same run; the bundle's `initial/` image restores the state
    /// *before* the run, so with the write suppressed the tail read comes back empty and the replay
    /// diverges against its own recording. Found by replaying the scenario, which is what PR 4 is
    /// for.
    ///
    /// Performing it is also the model this module already documents for reads — *"in replay the
    /// read really happens, against the restored state"*. The restored state simply has to include
    /// what this run wrote. A replay runs against its own directory, so there is nothing to
    /// double-apply.
    pub fn kernel_fs(file: &str, op: &str, bytes: &[u8], sync: bool, ok: bool, err: &str)
        -> Result<(), String>
    {
        CTX.with(|c| {
            let mut guard = c.borrow_mut();
            let Some(ctx) = guard.as_mut() else { return Ok(()) };
            let offset = *ctx.offsets.get(file).unwrap_or(&0);
            let observed = if ok {
                mycelium_sim::FsOutcome::Ok(bytes.len())
            } else {
                mycelium_sim::FsOutcome::Err(err.to_string())
            };
            let mut seams = Seams::new(&mut ctx.kernel, &mut ctx.sources, &ctx.node);
            let decided = match seams.fs(file, op, bytes, offset, sync, || observed) {
                Ok(o) => o,
                Err(d) => panic!("{d}"),
            };
            match decided {
                mycelium_sim::FsOutcome::Ok(n) | mycelium_sim::FsOutcome::Short(n) => {
                    *ctx.offsets.entry(file.to_string()).or_insert(0) += n as u64;
                    Ok(())
                }
                mycelium_sim::FsOutcome::Err(e) => Err(e),
            }
        })
    }

    /// Ask the kernel what should happen to this effect, **before it happens**.
    ///
    /// `None` means "no kernel, or we are recording" — the caller performs the effect and then
    /// reports it with [`kernel_fs`]. `Some(outcome)` means a replay has already decided, and the
    /// caller performs the effect **only if that outcome is `Ok`**.
    ///
    /// # Why the order matters (item 6 PR 4, the fault sweep)
    ///
    /// The two modes genuinely need opposite orders. Recording *must* act first, because the real
    /// outcome is the thing being written down. Replay *must* decide first, or an injected fault
    /// cannot prevent anything: a recorded `Err` applied after the fact would report a failed rename
    /// that had already renamed, and a sweep built on that would be testing a fiction.
    ///
    /// So a fault is not a lie told to the caller — it is an effect that does not happen.
    pub fn planned_fs(file: &str, op: &str, bytes: &[u8], sync: bool)
        -> Option<Result<(), String>>
    {
        CTX.with(|c| {
            let mut guard = c.borrow_mut();
            let ctx = guard.as_mut()?;
            if ctx.kernel.mode() != mycelium_sim::Mode::Replay {
                return None;
            }
            let offset = *ctx.offsets.get(file).unwrap_or(&0);
            let mut seams = Seams::new(&mut ctx.kernel, &mut ctx.sources, &ctx.node);
            let decided = match seams.fs(file, op, bytes, offset, sync, || unreachable!(
                "replay never produces an outcome; it reads the recorded one"
            )) {
                Ok(o) => o,
                Err(d) => panic!("{d}"),
            };
            Some(match decided {
                mycelium_sim::FsOutcome::Ok(n) | mycelium_sim::FsOutcome::Short(n) => {
                    *ctx.offsets.entry(file.to_string()).or_insert(0) += n as u64;
                    Ok(())
                }
                mycelium_sim::FsOutcome::Err(e) => Err(e),
            })
        })
    }

    /// Which mode the installed kernel is in, or `None` when there is no kernel.
    pub fn mode() -> Option<mycelium_sim::Mode> {
        CTX.with(|c| c.borrow().as_ref().map(|ctx| ctx.kernel.mode()))
    }

    /// The channel seam's name for [`mode`].
    pub fn chan_mode() -> Option<mycelium_sim::Mode> {
        mode()
    }

    /// Write down that a wait (`op` = `sleep` or `tick`) of `requested_ms` elapsed, and advance the
    /// simulated clocks by it.
    pub fn timer_record(op: &'static str, stream: &str, requested_ms: u64) {
        CTX.with(|c| {
            let mut guard = c.borrow_mut();
            let Some(ctx) = guard.as_mut() else { return };
            let mut seams = Seams::new(&mut ctx.kernel, &mut ctx.sources, &ctx.node);
            if let Err(d) = seams.timer(op, stream, requested_ms, || requested_ms) {
                panic!("{d}");
            }
        });
    }

    /// The effective duration the recording gave this wait, applied to the simulated clocks.
    pub fn timer_replay(op: &'static str, stream: &str, requested_ms: u64) -> u64 {
        CTX.with(|c| {
            let mut guard = c.borrow_mut();
            let Some(ctx) = guard.as_mut() else { return requested_ms };
            let mut seams = Seams::new(&mut ctx.kernel, &mut ctx.sources, &ctx.node);
            match seams.timer(op, stream, requested_ms, || unreachable!(
                "replay never produces a duration; it reads the recorded one"
            )) {
                Ok(v) => v,
                Err(d) => panic!("{d}"),
            }
        })
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

/// The monotonic clock, through the kernel. See the no-`sim` arm for why it is separate from
/// [`wall_now_ms`] — the two are different clocks, and a trace that merged them would be unreadable.
#[cfg(feature = "sim")]
#[inline]
pub fn mono_now_ns() -> u64 {
    installed::with_seams(real_mono_now_ns, |s| s.mono_now_ns())
}

// ── Timers ───────────────────────────────────────────────────────────────────────────────────
//
// Inventory §2.3: a fixed sleep inside protocol logic is *a schedule the kernel must explore* —
// 0, exact, and longer than the sleep. The one whose duration is a correctness assumption is the
// 1 s "let the winning commit converge" after `distributed_lock`'s optimistic commit; the D4 audit
// could only *model* that path, because this seam did not exist to replay it.
//
// What a replay reproduces is the decision — "this wait elapsed, and time is now later by D" —
// not the waiting. So a replayed sleep never wall-waits: the kernel supplies D, the simulated
// clocks advance by D, and the task yields once so the await point that was there is still there.
//
// What this seam does NOT yet give is the exploration itself. An authored D is honoured by the
// timer, but in exact replay the clock reads that follow are supplied from the trace as well, so
// they still say what the recording said. Re-deriving them is scenario replay — the inventory's
// third mode — and the kernel has two. The test that pins this boundary was first written as the
// claim, and failed; it now asserts both halves.

/// Sleep, without the kernel — the call this seam replaced.
#[cfg(not(feature = "sim"))]
#[inline]
pub async fn sleep_ms(_stream: &str, ms: u64) {
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
}

/// Arm the **scheduler seam's first arm** (item 6, 2026-09-19): pause tokio's clock and let every
/// replayed wait on this thread be taken *on that clock* instead of collapsing to one yield.
///
/// # Why this exists
///
/// A single task's effects replay without it; a whole node's do not. Two tasks that both wait —
/// the membership governor's jitter and a consensus round's defer — resume in whatever order the
/// runtime picks, because the kernel supplies *results* to requests the code makes and never
/// decides which task makes the next request. Yielding once discards the one fact that ordered
/// them: how long each was waiting. On a paused clock a wait is ordering information again, and
/// tokio advances to the nearest deadline when the runtime is idle, so the replay still never
/// spends wall time.
///
/// # Why one call and not two
///
/// The pause and the arming must agree. Pausing without arming leaves replays yielding while the
/// clock stands still — every wait collapses to the same instant, which is worse than not pausing.
/// Arming without pausing makes each replayed wait a *real* one, and a replay that sleeps for the
/// recorded durations is just a slow rerun. Neither half is useful alone, so neither is offered
/// alone.
///
/// # What it requires, and what it does not fix
///
/// A `current_thread` runtime (tokio's clock control panics on a multi-threaded one), and a
/// recording taken on one. Anything whose order came from *outside* the executor — real peers, real
/// sockets — is untouched: that is the network seam, and it is a record of its own
/// (`docs/design/replay-nondeterminism-inventory.md` §3.1).
#[cfg(feature = "sim")]
pub fn pause_clock_for_replay() {
    tokio::time::pause();
    installed::set_clock_paused(true);
}

/// Undo [`pause_clock_for_replay`] — resume tokio's clock and disarm the replay arms.
///
/// Paired with it, and for one situation: a test that replays the same trace **twice** on one
/// runtime — once armed, to show the recorded interleaving is reproduced, and once unarmed, to show
/// that it is the arm doing it. Without a resume the second run inherits a paused clock it did not
/// ask for, and the plant would be measuring something else.
///
/// Call it only after a [`pause_clock_for_replay`] on the same runtime: tokio's `resume` panics on
/// a clock that is already running. (Taking the kernel disarms the replay arms but cannot resume
/// the clock, which belongs to the runtime rather than to the thread's kernel — so this is
/// deliberately a separate call and not part of `take`.)
#[cfg(feature = "sim")]
pub fn resume_clock_after_replay() {
    installed::set_clock_paused(false);
    tokio::time::resume();
}

/// Sleep, through the kernel.
///
/// `Record` really sleeps — the production code runs for real, and unseamed timers elsewhere still
/// see real time pass — and writes down that the wait elapsed. `Replay` does not spend wall time:
/// the recorded effective duration advances the simulated clocks, and then either the task yields
/// once (the await point stays, the wall wait goes) or — with [`pause_clock_for_replay`] armed —
/// the wait is *taken on tokio's paused clock*, which keeps this task's wait ordered against every
/// other task's. A request that differs from the recording is a divergence.
///
/// One thread per simulated node, the module's standing assumption: the kernel is a thread-local,
/// and a sleep that resumed on another thread would find no kernel there.
#[cfg(feature = "sim")]
pub async fn sleep_ms(stream: &str, ms: u64) {
    match installed::mode() {
        None => tokio::time::sleep(std::time::Duration::from_millis(ms)).await,
        Some(mycelium_sim::Mode::Record) => {
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
            installed::timer_record("sleep", stream, ms);
        }
        Some(mycelium_sim::Mode::Replay) => replay_timer("sleep", stream, ms).await,
    }
}

/// How a replayed wait is taken.
///
/// **Unarmed** (every replay before the scheduler seam's first arm): check the request against the
/// trace, then yield once. The await point survives, the wall wait goes, and the order of requests
/// is the order tasks *entered* their waits.
///
/// **Armed** ([`pause_clock_for_replay`]): take the wait on the paused clock first, then check. The
/// order matters and the two arms must agree on what it is — and `Record` writes its entry *after*
/// the wait, so a recording's order is the order waits **completed**. Checking on entry would order
/// the replay by when each task started waiting instead, which is a different sequence whenever two
/// waits overlap: exactly the case this arm exists for. (Found by its own test: the 50 ms task,
/// spawned first, checked in first on replay while the recording had the 10 ms task first.)
///
/// The wait taken is the **nominal** duration the caller asked for, not the kernel's effective one,
/// because the effective value is only knowable by consuming the trace entry — which is the check
/// itself. In an exact replay the code requests what it requested when recording, so the ordering
/// is reproduced; a kernel that *rewrote* a duration is authoring a different schedule, and that is
/// scenario replay rather than exact replay.
#[cfg(feature = "sim")]
async fn replay_timer(op: &'static str, stream: &str, nominal_ms: u64) {
    if installed::clock_paused() {
        tokio::time::sleep(std::time::Duration::from_millis(nominal_ms)).await;
        let _effective_ms = installed::timer_replay(op, stream, nominal_ms);
    } else {
        let _effective_ms = installed::timer_replay(op, stream, nominal_ms);
        tokio::task::yield_now().await;
    }
}

// The second arm: periodic ticks (inventory §2.3, row 2 — "periodic loops → timer seam").
//
// What a replay reproduces is that *a tick fired*, and how much simulated time it stood for. The
// nominal schedule is the decision: the first tick is immediate (tokio's contract, and every loop
// here relies on it), every later one is one period. A delayed real tick under `Skip` is still one
// tick, so the recording says one period — the kernel's world reads the schedule the loop was
// written against, not the wall's jitter. The stream is owned, not `&'static`: a loop that runs per
// kind needs a name per kind (stream identity per destination), built once at construction.

/// A periodic tick, without the kernel — `tokio::time::interval` with the requested missed-tick
/// behaviour, and nothing else.
#[cfg(not(feature = "sim"))]
pub struct Ticker {
    inner: tokio::time::Interval,
}

/// A periodic tick, without the kernel.
#[cfg(not(feature = "sim"))]
#[inline]
pub fn interval_ms(
    _stream: impl Into<String>,
    period_ms: u64,
    missed: tokio::time::MissedTickBehavior,
) -> Ticker {
    // `interval` panics on a zero period; a caller that asked for one meant "as fast as possible".
    let mut inner = tokio::time::interval(std::time::Duration::from_millis(period_ms.max(1)));
    inner.set_missed_tick_behavior(missed);
    Ticker { inner }
}

#[cfg(not(feature = "sim"))]
impl Ticker {
    /// Wait for the next tick.
    #[inline]
    pub async fn tick(&mut self) {
        self.inner.tick().await;
    }

    /// Defer the next tick to `ms` from now; the period resumes after it.
    #[inline]
    pub fn reset_after_ms(&mut self, ms: u64) {
        self.inner.reset_after(std::time::Duration::from_millis(ms));
    }
}

/// A periodic tick, through the kernel.
#[cfg(feature = "sim")]
pub struct Ticker {
    inner: tokio::time::Interval,
    stream: String,
    period_ms: u64,
    first: bool,
    /// A deferral requested by `reset_after_ms`: the next tick's nominal wait, once.
    deferred_ms: Option<u64>,
}

/// A periodic tick, through the kernel. `stream` must be distinct per loop — two loops on one
/// stream would hand each other their ticks on replay.
#[cfg(feature = "sim")]
pub fn interval_ms(
    stream: impl Into<String>,
    period_ms: u64,
    missed: tokio::time::MissedTickBehavior,
) -> Ticker {
    let mut inner = tokio::time::interval(std::time::Duration::from_millis(period_ms.max(1)));
    inner.set_missed_tick_behavior(missed);
    Ticker { inner, stream: stream.into(), period_ms, first: true, deferred_ms: None }
}

#[cfg(feature = "sim")]
impl Ticker {
    /// Defer the next tick to `ms` from now; the period resumes after it. A deferral is a
    /// decision, so the next recorded tick carries `ms` as its nominal wait — a trace shows
    /// `tick(30000ms)` where the loop chose to wait, not a period that never elapsed.
    pub fn reset_after_ms(&mut self, ms: u64) {
        self.inner.reset_after(std::time::Duration::from_millis(ms));
        self.deferred_ms = Some(ms);
        self.first = false;
    }

    /// Wait for the next tick.
    ///
    /// `Record` really waits and writes down the nominal elapsed time (`0` for the first tick, one
    /// period after, or the deferral a `reset_after_ms` asked for). `Replay` does not wait: the
    /// recorded duration advances the simulated clocks and the task yields once. A tick whose
    /// period differs from the recording is a divergence.
    pub async fn tick(&mut self) {
        let nominal = match self.deferred_ms.take() {
            Some(ms) => ms,
            None if self.first => 0,
            None => self.period_ms,
        };
        self.first = false;
        match installed::mode() {
            None => {
                self.inner.tick().await;
            }
            Some(mycelium_sim::Mode::Record) => {
                self.inner.tick().await;
                installed::timer_record("tick", &self.stream, nominal);
            }
            Some(mycelium_sim::Mode::Replay) => replay_timer("tick", &self.stream, nominal).await,
        }
    }
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

/// `write_all` **followed by `flush`**, recorded as one effect.
///
/// The flush is load-bearing, not tidiness. `tokio::fs::File::write_all` copies into an in-process
/// buffer and queues the real syscall on a blocking thread, returning `Ok` **before it runs**. Worse,
/// `File::sync_data` completes that in-flight write and then *discards* its error (it is stashed in
/// the file's `last_write_err` and surfaces on the **next** write), so a `write_all` + `sync_data`
/// pair returns `Ok` for a record that failed with `ENOSPC` — and the durability receipt built on
/// that pair claimed `OnDisk` for bytes that never reached the disk.
///
/// Flushing here makes the error surface against **the record that caused it**, which is what a
/// receipt naming a rung requires. `do_snapshot` already flushed for the same reason before reading
/// the tail back; the receipt path did not.
#[cfg(not(feature = "sim"))]
#[inline]
pub async fn fs_write_all(
    file: &mut tokio::fs::File,
    _name: &str,
    bytes: &[u8],
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt as _;
    file.write_all(bytes).await?;
    file.flush().await
}

/// `write_all`, through the kernel.
#[cfg(feature = "sim")]
pub async fn fs_write_all(
    file: &mut tokio::fs::File,
    name: &str,
    bytes: &[u8],
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt as _;
    // The effect happens in both modes (see `installed::kernel_fs`); the kernel decides the outcome,
    // so a recorded failure replays as a failure.
    // Decide first, then act — see `installed::planned_fs`. In replay a recorded failure means the
    // effect does not happen at all, which is what makes a fault sweep honest.
    if let Some(decided) = installed::planned_fs(name, "write_all", bytes, false) {
        return match decided {
            Ok(()) => {
                let res = async {
                    file.write_all(bytes).await?;
                    file.flush().await
                }
                .await;
                assert!(
                    res.is_ok(),
                    "replay diverged at {name}/write_all: the recording succeeded, the replay got {res:?}"
                );
                Ok(())
            }
            Err(e) => Err(std::io::Error::other(e)),
        };
    }
    // One effect = a *completed* write. See the non-`sim` arm for why the flush is load-bearing.
    let res = async {
        file.write_all(bytes).await?;
        file.flush().await
    }
    .await;
    installed::kernel_fs(
        name,
        "write_all",
        bytes,
        false,
        res.is_ok(),
        &res.as_ref().err().map(|e| e.to_string()).unwrap_or_default(),
    )
    .map_err(std::io::Error::other)
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
    // Decide first, then act — see `installed::planned_fs`. In replay a recorded failure means the
    // effect does not happen at all, which is what makes a fault sweep honest.
    if let Some(decided) = installed::planned_fs(name, "write", bytes, false) {
        return match decided {
            Ok(()) => {
                let res = tokio::fs::write(path, bytes).await;
                assert!(
                    res.is_ok(),
                    "replay diverged at {name}/write: the recording succeeded, the replay got {res:?}"
                );
                Ok(())
            }
            Err(e) => Err(std::io::Error::other(e)),
        };
    }
    let res = tokio::fs::write(path, bytes).await;
    installed::kernel_fs(
        name,
        "write",
        bytes,
        false,
        res.is_ok(),
        &res.as_ref().err().map(|e| e.to_string()).unwrap_or_default(),
    )
    .map_err(std::io::Error::other)
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
    // **File names, not absolute paths.** Both ends are recorded because a rename that moved a
    // different file is a different effect — but an absolute path is run-specific (a temp directory
    // per run), and a request carrying one can only ever replay in the directory that produced it.
    // A bundle is meant to replay on another machine, so the request has to be portable.
    let name_of = |p: &std::path::Path| {
        p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    };
    let both = format!("{}->{}", name_of(from), name_of(to));
    // Decide first, then act — see `installed::planned_fs`. In replay a recorded failure means the
    // effect does not happen at all, which is what makes a fault sweep honest.
    if let Some(decided) = installed::planned_fs(name, "rename", both.as_bytes(), false) {
        return match decided {
            Ok(()) => {
                let res = tokio::fs::rename(from, to).await;
                assert!(
                    res.is_ok(),
                    "replay diverged at {name}/rename: the recording succeeded, the replay got {res:?}"
                );
                Ok(())
            }
            Err(e) => Err(std::io::Error::other(e)),
        };
    }
    let res = tokio::fs::rename(from, to).await;
    installed::kernel_fs(
        name,
        "rename",
        both.as_bytes(),
        false,
        res.is_ok(),
        &res.as_ref().err().map(|e| e.to_string()).unwrap_or_default(),
    )
    .map_err(std::io::Error::other)
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
    // The kernel decides the outcome the caller sees. A read is still *performed* — its bytes are
    // its result, not its request, so there is nothing to supply — but a recorded failure has to
    // come back as a failure, or a fault injected at a read is silently ignored. That matters here
    // more than anywhere: v2.4.3 exists because a failed WAL-tail read was treated as an empty tail
    // and the records it could not see were truncated away.
    match installed::kernel_fs(
        name,
        "read",
        &digest,
        false,
        res.is_ok(),
        &res.as_ref().err().map(|e| e.to_string()).unwrap_or_default(),
    ) {
        Ok(()) => res,
        Err(e) => Err(std::io::Error::other(e)),
    }
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
    // Decide first, then act — see `installed::planned_fs`. In replay a recorded failure means the
    // effect does not happen at all, which is what makes a fault sweep honest.
    if let Some(decided) = installed::planned_fs(name, "sync_data", &[], true) {
        return match decided {
            Ok(()) => {
                let res = file.sync_data().await;
                assert!(
                    res.is_ok(),
                    "replay diverged at {name}/sync_data: the recording succeeded, the replay got {res:?}"
                );
                Ok(())
            }
            Err(e) => Err(std::io::Error::other(e)),
        };
    }
    let res = file.sync_data().await;
    installed::kernel_fs(
        name,
        "sync_data",
        &[],
        true,
        res.is_ok(),
        &res.as_ref().err().map(|e| e.to_string()).unwrap_or_default(),
    )
    .map_err(std::io::Error::other)
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
    // Decide first, then act — see `installed::planned_fs`. In replay a recorded failure means the
    // effect does not happen at all, which is what makes a fault sweep honest.
    if let Some(decided) = installed::planned_fs(name, "sync_dir", &[], true) {
        return match decided {
            Ok(()) => {
                let res = dir.sync_all().await;
                assert!(
                    res.is_ok(),
                    "replay diverged at {name}/sync_dir: the recording succeeded, the replay got {res:?}"
                );
                Ok(())
            }
            Err(e) => Err(std::io::Error::other(e)),
        };
    }
    let res = dir.sync_all().await;
    installed::kernel_fs(
        name,
        "sync_dir",
        &[],
        true,
        res.is_ok(),
        &res.as_ref().err().map(|e| e.to_string()).unwrap_or_default(),
    )
    .map_err(std::io::Error::other)
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

    /// Two tasks whose waits differ, spawned longest-first so spawn order and completion order
    /// disagree. The trace records a wait when it *completes*, so the recording's order is the
    /// short wait then the long one.
    async fn two_waits() {
        let slow = tokio::spawn(async { sleep_ms("slow", 50).await });
        let fast = tokio::spawn(async { sleep_ms("fast", 10).await });
        for h in [slow, fast] {
            if let Err(e) = h.await {
                std::panic::resume_unwind(e.into_panic());
            }
        }
    }

    fn install_replaying_trace(trace: mycelium_sim::trace::Trace) {
        installed::install(SimContext {
            kernel:  Kernel::replaying(trace),
            sources: Sources::seeded(9, 1_789_000_000_000),
            node:    "n1".into(),
            offsets: Default::default(),
        });
    }

    /// **The scheduler seam's first arm** (inventory §3.1): with tokio's clock paused, a replayed
    /// wait is *ordering information* again. Two tasks spawned longest-first complete shortest-first
    /// in the recording; the replay reproduces that order instead of resuming them in spawn order.
    ///
    /// The plant is the sibling test below: the same trace, the same replay, without the pause —
    /// which diverges. Without that pair this test would pass on a replay that had simply kept
    /// spawn order and happened to agree.
    #[tokio::test]
    async fn a_paused_clock_replays_two_waits_in_the_recorded_order() {
        install_recording();
        two_waits().await;
        let trace = installed::take().expect("installed").kernel.trace().clone();
        assert!(trace.len() >= 2, "both waits were recorded: {}", trace.len());

        install_replaying_trace(trace);
        pause_clock_for_replay();
        two_waits().await;
        installed::take();
    }

    /// The plant for the test above: the identical replay with the clock left running collapses
    /// every wait to one yield, so the tasks resume in spawn order — long first — and the second
    /// request diverges from the recording. This is what every whole-node replay did before the
    /// first arm existed.
    #[tokio::test]
    #[should_panic(expected = "replay diverged")]
    async fn without_the_paused_clock_the_same_two_waits_diverge() {
        install_recording();
        two_waits().await;
        let trace = installed::take().expect("installed").kernel.trace().clone();

        install_replaying_trace(trace);
        two_waits().await;
    }

    fn install_replaying(text: &str) {
        let trace = mycelium_sim::trace::Trace::parse(text).expect("the trace round-trips");
        installed::install(SimContext {
            kernel:  Kernel::replaying(trace),
            sources: Sources::seeded(9, 1_789_000_000_000),
            node:    "n1".into(),
            offsets: Default::default(),
        });
    }

    /// **A recorded sleep advances simulated time by its duration** — in `Record` too, because
    /// under a kernel the clocks are `Sources`, and a wait that left them still would make the
    /// recording disagree with the real sleep that just happened.
    #[tokio::test(flavor = "current_thread")]
    async fn a_recorded_sleep_advances_simulated_time_by_its_duration() {
        install_recording();
        let w1 = wall_now_ms();
        let m1 = mono_now_ns();
        sleep_ms("t/converge", 20).await;
        let w2 = wall_now_ms();
        let m2 = mono_now_ns();
        let ctx = installed::take().expect("installed");

        // Each read steps its clock once; the sleep adds 20 ms between the reads.
        assert_eq!(w2 - w1, 20 + 1, "the wall clock elapsed the sleep, plus its own read step");
        assert_eq!(m2 - m1, 20_000_000 + 1_000_000, "and so did the monotonic clock");
        let timer = ctx
            .kernel
            .trace()
            .entries()
            .iter()
            .find(|c| c.kind == mycelium_sim::trace::ChoiceKind::Timer)
            .expect("a timer choice was written down");
        assert_eq!(timer.request, "sleep(20ms)");
        assert_eq!(timer.result, "20", "the recording's effective duration is the one asked for");
    }

    /// **A replayed sleep does not wall-wait.** The recording waited; the replay advances the
    /// simulated clocks by the recorded duration and yields once. This is what makes a 1 s
    /// converge sleep replayable in microseconds — and the property that would silently vanish if
    /// replay ever called the real timer.
    #[tokio::test(flavor = "current_thread")]
    async fn a_replayed_sleep_advances_the_clocks_without_waiting() {
        install_recording();
        let w1 = wall_now_ms();
        sleep_ms("t/converge", 150).await;
        let w2 = wall_now_ms();
        let text = installed::take().expect("installed").kernel.trace().to_text();
        assert_eq!(w2 - w1, 151);

        install_replaying(&text);
        let started = std::time::Instant::now();
        let r1 = wall_now_ms();
        sleep_ms("t/converge", 150).await;
        let r2 = wall_now_ms();
        let took = started.elapsed();
        installed::take();

        assert_eq!(r2 - r1, 151, "the replay's clocks elapsed exactly what the recording's did");
        assert!(
            took < std::time::Duration::from_millis(100),
            "but no wall time was spent waiting: {took:?}"
        );
    }

    /// **An authored timer result is honoured by the timer — and recorded clock reads still
    /// replay as recorded.** Both halves are pinned on purpose.
    ///
    /// The kernel checks a replay's *request* and supplies the recorded *result*, so editing a
    /// timer line to `0` or `5000` makes the replayed wait return that duration. What it does
    /// **not** do is change the clock reads recorded *after* the wait: in exact replay those are
    /// supplied from the trace too, so a read that followed a 20 ms sleep still says 20 ms later.
    ///
    /// That is the two-mode kernel's boundary, found by writing this test the other way first:
    /// **exact replay reproduces; it cannot explore.** Exploring 0 / exact / beyond means
    /// re-deriving the reads that follow an authored wait, which is *scenario replay* — the third
    /// mode the inventory names and the kernel does not yet have. When it lands, the second
    /// assertion here changes deliberately.
    #[tokio::test(flavor = "current_thread")]
    async fn an_authored_result_is_honoured_by_the_timer_but_recorded_reads_still_replay() {
        install_recording();
        let _ = wall_now_ms();
        sleep_ms("lock/converge", 20).await;
        let _ = wall_now_ms();
        let text = installed::take().expect("installed").kernel.trace().to_text();
        let recorded = mycelium_sim::trace::Trace::parse(&text).expect("the trace round-trips");

        for authored in [0u64, 20, 5000] {
            let mut schedule = mycelium_sim::trace::Trace::new();
            for c in recorded.entries() {
                let mut c = c.clone();
                if c.kind == mycelium_sim::trace::ChoiceKind::Timer {
                    c.result = authored.to_string();
                }
                schedule.push(c);
            }
            install_replaying(&schedule.to_text());
            let r1 = wall_now_ms();
            let effective = installed::timer_replay("sleep", "lock/converge", 20);
            let r2 = wall_now_ms();
            installed::take();

            assert_eq!(effective, authored, "the timer returns the authored duration");
            assert_eq!(
                r2 - r1,
                21,
                "but a recorded read is replayed, not re-derived: exact replay cannot explore"
            );
        }
    }

    /// A sleep that asks for a different duration than the recording is a **divergence**, printed
    /// with both sides — not a silently re-timed wait.
    #[test]
    fn a_sleep_that_differs_from_the_recording_is_a_divergence() {
        install_recording();
        installed::timer_record("sleep", "lock/converge", 20);
        let text = installed::take().expect("installed").kernel.trace().to_text();

        install_replaying(&text);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            installed::timer_replay("sleep", "lock/converge", 21)
        }));
        installed::take();

        let msg = match outcome {
            Err(payload) => payload.downcast_ref::<String>().cloned().unwrap_or_default(),
            Ok(v) => panic!("the replay accepted a different duration and returned {v}"),
        };
        assert!(
            msg.contains("sleep(20ms)") && msg.contains("sleep(21ms)"),
            "both sides are printed, so the reader sees what changed: {msg}"
        );
    }

    /// **A ticker records the nominal schedule**: an immediate first tick, then one period each —
    /// and the simulated clocks advance by exactly that, so a loop replays against the schedule it
    /// was written for.
    #[tokio::test(flavor = "current_thread")]
    async fn a_ticker_records_an_immediate_first_tick_then_one_period_each() {
        install_recording();
        let w1 = wall_now_ms();
        let mut t = interval_ms("t/tick", 30, tokio::time::MissedTickBehavior::Skip);
        t.tick().await;
        t.tick().await;
        t.tick().await;
        let w2 = wall_now_ms();
        let ctx = installed::take().expect("installed");

        // The first tick is immediate (contributes nothing), the next two one period each, plus the
        // read step.
        assert_eq!(w2 - w1, 30 + 30 + 1, "first tick immediate, then one period each, plus the read step");
        let ticks: Vec<&str> = ctx
            .kernel
            .trace()
            .entries()
            .iter()
            .filter(|c| c.kind == mycelium_sim::trace::ChoiceKind::Timer)
            .map(|c| c.request.as_str())
            .collect();
        assert_eq!(ticks, vec!["tick(0ms)", "tick(30ms)", "tick(30ms)"]);
    }

    /// **A replayed ticker does not wall-wait.** Two ticks of 150 ms took the recording ~150 ms;
    /// the replay advances the clocks the same and spends no wall time waiting.
    #[tokio::test(flavor = "current_thread")]
    async fn a_replayed_ticker_advances_the_clocks_without_waiting() {
        install_recording();
        let w1 = wall_now_ms();
        let mut t = interval_ms("t/tick", 150, tokio::time::MissedTickBehavior::Skip);
        t.tick().await;
        t.tick().await;
        let w2 = wall_now_ms();
        let text = installed::take().expect("installed").kernel.trace().to_text();
        assert_eq!(w2 - w1, 151);

        install_replaying(&text);
        let started = std::time::Instant::now();
        let r1 = wall_now_ms();
        let mut t = interval_ms("t/tick", 150, tokio::time::MissedTickBehavior::Skip);
        t.tick().await;
        t.tick().await;
        let r2 = wall_now_ms();
        let took = started.elapsed();
        installed::take();

        assert_eq!(r2 - r1, 151, "the replay's clocks elapsed what the recording's did");
        assert!(took < std::time::Duration::from_millis(100), "no wall wait: {took:?}");
    }

    /// **A sleep and a tick of the same length are different decisions.** A recorded sleep replayed
    /// as a tick diverges — the op is in the request — so a trace can never replay a periodic loop
    /// from a one-off wait, or the reverse.
    /// A deferral is a decision: `reset_after_ms` makes the next recorded tick carry the deferral
    /// as its nominal wait, so a trace shows where a loop chose to wait, and the period resumes
    /// after it. Without this the snapshot loop's 30 s "defer while opaque" would replay as one
    /// ordinary period.
    #[tokio::test]
    async fn a_deferred_tick_records_the_deferral_then_resumes_the_period() {
        install_recording();
        let mut t = interval_ms("t/snap", 20, tokio::time::MissedTickBehavior::Skip);
        t.tick().await; // immediate: tick(0ms)
        t.reset_after_ms(5);
        t.tick().await; // the deferral: tick(5ms)
        t.tick().await; // the period again: tick(20ms)
        let text = installed::take().expect("installed").kernel.trace().to_text();
        let pos = |s: &str| text.find(s).unwrap_or_else(|| panic!("{s} not in trace:\n{text}"));
        assert!(pos("tick(0ms)") < pos("tick(5ms)") && pos("tick(5ms)") < pos("tick(20ms)"), "{text}");
    }

    #[test]
    fn a_sleep_replayed_as_a_tick_is_a_divergence() {
        install_recording();
        installed::timer_record("sleep", "t/x", 20);
        let text = installed::take().expect("installed").kernel.trace().to_text();

        install_replaying(&text);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            installed::timer_replay("tick", "t/x", 20)
        }));
        installed::take();

        let msg = match outcome {
            Err(payload) => payload.downcast_ref::<String>().cloned().unwrap_or_default(),
            Ok(v) => panic!("a tick was accepted for a recorded sleep and returned {v}"),
        };
        assert!(msg.contains("sleep(20ms)") && msg.contains("tick(20ms)"), "{msg}");
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

    /// The monotonic clock is a **separate** stream from the wall clock, and interleaving the two
    /// must not make either read the other's value. They measure different things — an epoch and an
    /// interval — and a trace that merged them would be unreadable by anyone debugging from it.
    #[test]
    fn the_monotonic_clock_is_not_the_wall_clock() {
        install_recording();
        let w1 = wall_now_ms();
        let m1 = mono_now_ns();
        let w2 = wall_now_ms();
        let m2 = mono_now_ns();
        let ctx = installed::take().expect("installed");

        assert_eq!(w1, 1_789_000_000_000, "the seeded wall source");
        assert_eq!(w2, w1 + 1, "the wall clock advanced by its own step");
        assert_eq!(m2 - m1, 1_000_000, "the monotonic clock advanced by its own step, not the wall's");
        assert_eq!(m1, MONO_ORIGIN_NS, "the run's origin, and the kernel agrees with production");
        assert!(m1 < 1_700_000_000_000_000_000, "nothing like an epoch reading");
        assert_eq!(ctx.kernel.trace().len(), 4, "all four reads are in the trace");
    }

    /// **A point before the run began is representable.** `Instant` has this property — on both
    /// platforms it is internally offset, so `Instant::now() - 600s` is fine however young the
    /// process is — and a bare "nanoseconds since we started" would not. Kept as a property of this
    /// clock for any caller that subtracts a window from a fresh reading; the caller that first
    /// needed it (`SignalLog::seed`) is back on `Instant` and no longer does.
    #[test]
    fn the_monotonic_origin_leaves_room_to_point_before_the_run_began() {
        installed::take();
        let now = mono_now_ns();
        let a_year = 365u64 * 24 * 60 * 60 * 1_000_000_000;
        assert!(
            now >= a_year,
            "a window subtracted from a fresh reading must not clamp: {now} < {a_year}"
        );
        // The case that motivated it: ten minutes before a process that is milliseconds old.
        let ten_minutes = 600u64 * 1_000_000_000;
        assert_eq!(
            mono_between(now - ten_minutes, now),
            std::time::Duration::from_secs(600),
            "ten minutes before now is exactly ten minutes ago, not clamped to zero"
        );
    }

    /// Without a kernel the seam is the real monotonic clock — an interval that only goes forward.
    #[test]
    fn the_monotonic_seam_falls_back_to_a_real_monotonic_reading() {
        installed::take();
        let a = mono_now_ns();
        let b = mono_now_ns();
        assert!(b >= a, "a monotonic clock never goes backwards");
        // An hour past the origin at most: this counts from process start (plus the origin offset),
        // not from the epoch.
        assert!(
            a - MONO_ORIGIN_NS < 60 * 60 * 1_000_000_000,
            "since this process started, not since the epoch"
        );
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

/// The production arm's own gate. Deliberately **not** under `sim`: this pins what a shipped build
/// does, and the `sim` arm's recorder returns `Ok` when no kernel is installed, which would hide it.
#[cfg(all(test, not(feature = "sim")))]
mod production_arm_tests {
    use super::*;

    /// **A queued write that fails is reported by the write that caused it.**
    ///
    /// `tokio::fs::File::write_all` returns `Ok` before the syscall runs, and `File::sync_data`
    /// completes the in-flight write and then **discards its error** (stashing it to surface on the
    /// *next* write). So `write_all` + `sync_data` both answered `Ok` for a record that failed, and
    /// the durability receipt built on that pair claimed `OnDisk` for bytes that never reached disk
    /// — with the error then blaming the following record. Found by the Phase-C adversarial audit.
    ///
    /// A read-only handle makes the queued write fail at the syscall, the same shape as `ENOSPC` on
    /// a full disk and deterministic. Before the fix this returned `Ok`.
    #[tokio::test]
    async fn a_queued_write_that_fails_is_reported_by_the_write_that_caused_it() {
        let dir = std::env::temp_dir().join(format!("sim-seam-queued-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("wal.bin");
        std::fs::write(&path, b"").expect("create");

        let mut read_only = tokio::fs::OpenOptions::new()
            .read(true)
            .open(&path)
            .await
            .expect("open read-only");

        let res = fs_write_all(&mut read_only, "wal.bin", b"a record that cannot be written").await;
        assert!(
            res.is_err(),
            "a write that cannot reach the file must not answer Ok — a durability receipt is built \
             on exactly this answer, and reported OnDisk when it lied"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
