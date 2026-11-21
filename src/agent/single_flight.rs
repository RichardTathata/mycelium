//! **One run at a time, with tickets** — the identity fold's (`revocation::refresh`) and the client-certificate
//! fold's (`client_cert::refresh`) shared discipline.
//!
//! A call while a run is in progress makes that run go again, and returns. A caller that must see a run that
//! **started after** its call takes a [`SingleFlight::ticket`] and calls [`SingleFlight::answered`] after each
//! `refresh` until it is `true`: any run that read the ticket at its start and ended whole answers it, whichever
//! thread ran it.
//!
//! Two defects this replaces (2026-11-21, #822):
//! - **A livelock.** A waiter used to accept only a run it made itself. While another thread's run was in progress
//!   each retry asked for another pass, so that run never ended and the waiter never ran one; it hung CI's Test job
//!   for six hours, twice.
//! - **A lost request.** A call used to try `running` first and set `again` after. Between the two, the holder could
//!   release `running`, find `again` clear and return; the request was then folded by nobody until the next trigger
//!   (60 s or more). A call now sets `again` first and then tries `running`; the holder releases `running` and then
//!   reads `again`. Both sides store one variable then read the other, so both are `SeqCst` (the wiki's ordering
//!   policy, a two-sided handshake): at least one of them sees the other, and that one runs.
//!
//! A request made while a pass panics waits for the next call: unwinding skips the holder's re-check of `again`
//! (the guard still releases `running`, so that next call runs).
//!
//! Lock-free (atomics only): no lock-order row.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Default)]
pub(crate) struct SingleFlight {
    running: AtomicBool,
    again: AtomicBool,
    /// Tickets handed out ([`Self::ticket`]).
    asked: AtomicU64,
    /// The newest ticket a whole run has answered.
    done: AtomicU64,
}

#[cfg(test)]
thread_local! {
    /// A plant run once by the next [`SingleFlight::run`] on this thread, between asking (`again`) and trying
    /// `running` — the window the lost request lived in.
    pub(crate) static BETWEEN_ASK_AND_TRY: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    /// A plant run once by the next holder on this thread after its last `again.swap(false)` found no request and
    /// before it releases `running` — the window the holder's re-check of `again` covers.
    pub(crate) static AFTER_LAST_PASS: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

/// Clears `running` on every exit of a run, a panic included: a panic in a fold must not leave every later call
/// returning at once and every ticket waiter spinning.
struct Running<'a>(&'a AtomicBool);

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl SingleFlight {
    /// Ask for a run and make it, or leave it to the run in progress. `pass` is one run; it returns whether it ran
    /// whole (`false`: cut short — it answers no ticket — and it asks for another pass itself if it wants one, with
    /// [`Self::ask_again`]).
    pub(crate) fn run(&self, mut pass: impl FnMut() -> bool) {
        self.again.store(true, Ordering::SeqCst);
        #[cfg(test)]
        if let Some(plant) = BETWEEN_ASK_AND_TRY.with(|p| p.borrow_mut().take()) {
            plant();
        }
        loop {
            if self.running.swap(true, Ordering::SeqCst) {
                // The holder reads `again` after it releases `running`: it runs for this call.
                return;
            }
            {
                let _running = Running(&self.running);
                while self.again.swap(false, Ordering::SeqCst) {
                    // Acquire pairs with the ticket's AcqRel: a run that reads a ticket sees what its taker wrote before
                    // taking it. Release on `done` pairs with `answered`'s Acquire: the waiter sees the run.
                    let asked = self.asked.load(Ordering::Acquire);
                    if pass() {
                        self.done.fetch_max(asked, Ordering::Release);
                    }
                }
                #[cfg(test)]
                if let Some(plant) = AFTER_LAST_PASS.with(|p| p.borrow_mut().take()) {
                    plant();
                }
            }
            if !self.again.load(Ordering::SeqCst) {
                return;
            }
        }
    }

    /// A run in progress asks for another pass (the identity fold, when the roots changed under it).
    pub(crate) fn ask_again(&self) {
        self.again.store(true, Ordering::SeqCst);
    }

    /// A ticket for a run that starts after this call.
    pub(crate) fn ticket(&self) -> u64 {
        self.asked.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Whether a whole run that read `ticket` at its start has ended.
    pub(crate) fn answered(&self, ticket: u64) -> bool {
        self.done.load(Ordering::Acquire) >= ticket
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::time::Duration;

    /// **A request made while a run ends is run** (#822's review, B: a lost request). A holder is in its run when a
    /// second call asks; the plant between the call's two steps lets the holder finish and return before the call
    /// goes on. The call's request is run — by the holder, or by the call itself — before the call returns. Seen
    /// failing first with the old order (try `running`, then set `again`): the holder released `running`, found
    /// `again` clear and returned, the call then set `again` and returned, and one run had been made for two asks.
    #[test]
    fn a_request_made_while_a_run_ends_is_run() {
        let sf = Arc::new(SingleFlight::default());
        let runs = Arc::new(AtomicU64::new(0));
        let (in_run_tx, in_run_rx) = mpsc::channel::<()>();
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let (s, r) = (Arc::clone(&sf), Arc::clone(&runs));
        let holder = std::thread::spawn(move || {
            let (in_run, go) = (in_run_tx, go_rx);
            s.run(|| {
                if r.fetch_add(1, Ordering::SeqCst) == 0 {
                    let _ = in_run.send(());
                    let _ = go.recv_timeout(Duration::from_secs(10));
                }
                true
            });
            let _ = done_tx.send(());
        });
        in_run_rx.recv_timeout(Duration::from_secs(10)).expect("the holder is in its run");
        BETWEEN_ASK_AND_TRY.with(|p| {
            *p.borrow_mut() = Some(Box::new(move || {
                let _ = go_tx.send(());
                // Let the holder end its run and return (or run again for this call, which also returns).
                let _ = done_rx.recv_timeout(Duration::from_secs(10));
            }))
        });
        let r = Arc::clone(&runs);
        sf.run(|| {
            r.fetch_add(1, Ordering::SeqCst);
            true
        });
        holder.join().unwrap();
        assert!(runs.load(Ordering::SeqCst) >= 2, "one run for two asks: the second call's request was lost");
    }

    /// **A request that finds the run held just before the holder releases it is run** (#822's confirm round: the
    /// holder's half of the handshake). The holder has made its last pass and found no request; before it releases
    /// `running`, a second call asks, finds `running` held and returns. The holder's re-check of `again` after the
    /// release sees the request and makes a second pass. Seen failing first with the re-check removed (the holder
    /// returning after the release): one pass for two asks.
    #[test]
    fn a_request_that_finds_the_run_held_as_it_ends_is_run_by_the_holder() {
        let sf = Arc::new(SingleFlight::default());
        let passes = Arc::new(AtomicU64::new(0));
        let (s, p) = (Arc::clone(&sf), Arc::clone(&passes));
        AFTER_LAST_PASS.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                let asker = std::thread::spawn(move || {
                    s.run(|| {
                        p.fetch_add(1, Ordering::SeqCst);
                        true
                    })
                });
                asker.join().unwrap();
            }))
        });
        let p = Arc::clone(&passes);
        sf.run(|| {
            p.fetch_add(1, Ordering::SeqCst);
            true
        });
        assert_eq!(passes.load(Ordering::SeqCst), 2, "the holder ran a second pass for the request made as it ended");
    }

    /// **A panic in a run leaves no run claimed**: the next call runs (#822's review, the guard).
    #[test]
    fn a_panicking_run_releases_running() {
        let sf = SingleFlight::default();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sf.run(|| panic!("the test's run panics"))));
        assert!(r.is_err());
        let ticket = sf.ticket();
        sf.run(|| true);
        assert!(sf.answered(ticket), "a call after the panic ran and answered");
    }
}
