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

#[cfg(all(test, feature = "sim"))]
mod tests {
    use super::*;
    use mycelium_sim::{Kernel, Sources};

    fn install_recording() {
        installed::install(SimContext {
            kernel:  Kernel::recording(),
            sources: Sources::seeded(9, 1_789_000_000_000),
            node:    "n1".into(),
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
        });
        wall_now_ms(); // matches the one recorded read
        wall_now_ms(); // one more than was recorded — diverges
    }
}
