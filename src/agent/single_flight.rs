//! **One run at a time, with tickets** — the identity fold's (`revocation::refresh`) and the client-certificate
//! fold's (`client_cert::refresh`) shared discipline.
//!
//! A call while a run is in progress makes that run go again, and returns. A caller that must see a run that
//! **started after** its call takes a [`SingleFlight::ticket`] and calls [`SingleFlight::run_for`] and then
//! [`SingleFlight::answered`] until it is `true`: any run that read the ticket at its start and ended whole answers it,
//! whichever thread ran it.
//!
//! **Who keeps going.** A plain caller ([`SingleFlight::run`]: the identity watcher, the maintenance tick, startup)
//! that holds the run keeps running it while anyone asks: it promises to fold. A ticketed caller
//! ([`SingleFlight::run_for`]: `revoke_key`, `change_roots`) promises only to ask and wait for its own answer: once a
//! pass has answered its ticket it **offers** the run, and the next caller that asks while it holds it **accepts** and
//! is handed the run whole (`running` is never released between the two, and the request bit is left for the taker), so
//! the ticketed caller returns after at most one more pass (the one it is in, or is about to start). If nobody asks,
//! the request bit empties and it leaves as any holder does.
//!
//! Three defects this replaces:
//! - **A livelock** (2026-11-21, #822). A waiter used to accept only a run it made itself. While another thread's run
//!   was in progress each retry asked for another pass, so that run never ended and the waiter never ran one; it hung
//!   CI's Test job for six hours, twice.
//! - **A lost request** (#822). A call used to try `running` first and set `again` after. Between the two, the holder
//!   could release `running`, find `again` clear and return; the request was then folded by nobody until the next
//!   trigger (60 s or more). A call now sets `again` first and then tries `running`; the holder releases `running` and
//!   then reads `again`. Both sides store one variable then read the other, so both are `SeqCst` (the wiki's ordering
//!   policy, a two-sided handshake): at least one of them sees the other, and that one runs.
//! - **A drafted waiter** (2026-11-28, the mirror of the first). A ticketed caller that found no run in progress
//!   became the holder and ran passes while `again` was set; with another thread asking every few µs and a pass taking
//!   milliseconds, `again` was always set when a pass ended, and the caller never returned to its own answered ticket
//!   (`tickets_are_answered_under_a_busy_fold_and_changing_roots`, about one run in eleven on CI). Releasing the run
//!   and re-taking it while `again` is set would not do: the release-to-retake window is nanoseconds wide, so the
//!   caller would win it almost every time. Hence the hand-over.
//!
//! The hand-over (`offer`: `NONE` → `OFFERED` by the leaving holder → `ACCEPTED` by one asker → `HANDED` by the holder
//! → `NONE` by the new holder). The holder checks for an acceptance **before** it takes the request bit, so it never
//! consumes a request it will not run; it withdraws an offer (`OFFERED` → `NONE`) before it releases `running`, so an
//! offer is accepted only while its holder holds the run, and an acceptance it can no longer withdraw it must hand on.
//! An asker that finds the run held and no offer returns, as before: the holder releases and then looks.
//!
//! A request made while a pass panics waits for the next call: unwinding skips the holder's re-check of `again`
//! (the guard still releases `running`, or hands it to an asker that accepted, so that next call runs).
//!
//! **A call on the thread that holds the run is a request, not a wait** (the hand-over's review, 2026-11-28). A pass that
//! called its own fold's `run` or `run_for` on the holding thread, after its holder had offered the run, would accept
//! its own thread's offer and wait for itself for ever. Each thread therefore records the runs it holds (`HELD`: pushed
//! when it takes `running` or is handed it, popped when it hands it on or releases it), and a call that finds `self`
//! there sets `again` and returns at once: the holder, outside the pass, sees the request and runs it. No pass does
//! this in the product; a test's plant does (`a_fold_that_asks_for_its_own_fold_on_the_holding_thread_returns`).
//!
//! Lock-free (atomics only): no lock-order row.
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

thread_local! {
    /// The runs this thread holds, by address: at most a few (one per fold, nested only through a pass).
    static HELD: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

const NONE: u8 = 0;
const OFFERED: u8 = 1;
const ACCEPTED: u8 = 2;
const HANDED: u8 = 3;

#[derive(Default)]
pub(crate) struct SingleFlight {
    running: AtomicBool,
    again: AtomicBool,
    /// The hand-over of a leaving ticketed holder's run to the next asker (`NONE`, `OFFERED`, `ACCEPTED`, `HANDED`).
    offer: AtomicU8,
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

/// Ends a holder's hold on every exit, a panic included: a panic in a fold must not leave every later call returning
/// at once and every ticket waiter spinning, nor an asker that accepted the run waiting for it.
struct Holding<'a> {
    sf:    &'a SingleFlight,
    armed: bool,
}

impl<'a> Holding<'a> {
    /// This thread holds `sf`'s run now (it took `running`, or was handed it): record it.
    fn new(sf: &'a SingleFlight) -> Self {
        let _ = HELD.try_with(|h| h.borrow_mut().push(sf.addr()));
        Holding { sf, armed: true }
    }

    /// End the hold; `true` when the run was handed to an asker that accepted it (then that asker holds it and does
    /// the release-then-look: this caller returns at once).
    fn end(mut self) -> bool {
        self.armed = false;
        self.sf.end_hold()
    }
}

impl Drop for Holding<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.sf.end_hold();
        }
    }
}

impl SingleFlight {
    /// Ask for a run and make it, or leave it to the run in progress. `pass` is one run; it returns whether it ran
    /// whole (`false`: cut short — it answers no ticket — and it asks for another pass itself if it wants one, with
    /// [`Self::ask_again`]). A caller that holds the run keeps running it while anyone asks; one that finds it held by
    /// a ticketed caller that has offered it takes it over (waiting for at most one of that caller's passes, which may start
    /// after the acceptance). A call on the thread that holds this run only asks.
    pub(crate) fn run(&self, pass: impl FnMut() -> bool) {
        self.run_inner(None, pass);
    }

    /// [`Self::run`] for a caller that waits for `ticket`: once a pass has answered it, the caller offers the run to the
    /// next asker and returns after at most one more pass once one accepts — or when no request is left, as any holder.
    /// Its own request is never left unrun (the taker runs it, or the caller already has), and nor is anyone else's.
    pub(crate) fn run_for(&self, ticket: u64, pass: impl FnMut() -> bool) {
        self.run_inner(Some(ticket), pass);
    }

    fn run_inner(&self, ticket: Option<u64>, mut pass: impl FnMut() -> bool) {
        self.again.store(true, Ordering::SeqCst);
        // Called from inside a pass of this run, on the thread that holds it: the request is recorded, and the holder runs
        // it after the pass (waiting here would be waiting for this thread).
        if HELD.try_with(|h| h.borrow().contains(&self.addr())).unwrap_or(false) {
            return;
        }
        #[cfg(test)]
        if let Some(plant) = BETWEEN_ASK_AND_TRY.with(|p| p.borrow_mut().take()) {
            plant();
        }
        loop {
            if self.running.swap(true, Ordering::SeqCst) {
                // A ticketed holder that is leaving offers the run: take it over (it hands it on after its pass).
                if self.offer.compare_exchange(OFFERED, ACCEPTED, Ordering::SeqCst, Ordering::SeqCst).is_err() {
                    // The holder reads `again` after it releases `running`: it runs for this call.
                    return;
                }
                while self.offer.load(Ordering::SeqCst) != HANDED {
                    std::hint::spin_loop();
                    std::thread::yield_now();
                }
                // This call holds the run now (`running` was never released); the request bit is as the holder left it.
                self.offer.store(NONE, Ordering::SeqCst);
            }
            let hold = Holding::new(self);
            let mut leaving = ticket.is_some_and(|t| self.answered(t));
            loop {
                if leaving {
                    if self.offer.load(Ordering::SeqCst) == ACCEPTED {
                        break;
                    }
                    // First time, or after a withdrawn offer and a re-take: offer the run.
                    let _ = self.offer.compare_exchange(NONE, OFFERED, Ordering::SeqCst, Ordering::SeqCst);
                }
                if !self.again.swap(false, Ordering::SeqCst) {
                    break;
                }
                // Acquire pairs with the ticket's AcqRel: a run that reads a ticket sees what its taker wrote before
                // taking it. Release on `done` pairs with `answered`'s Acquire: the waiter sees the run.
                let asked = self.asked.load(Ordering::Acquire);
                if pass() {
                    self.done.fetch_max(asked, Ordering::Release);
                }
                leaving = leaving || ticket.is_some_and(|t| self.answered(t));
            }
            #[cfg(test)]
            if let Some(plant) = AFTER_LAST_PASS.with(|p| p.borrow_mut().take()) {
                plant();
            }
            if hold.end() {
                return;
            }
            if !self.again.load(Ordering::SeqCst) {
                return;
            }
        }
    }

    /// Withdraw this holder's offer and release `running` — or, when an asker has accepted the offer, hand it the run
    /// (`running` stays set). `true` when handed.
    fn end_hold(&self) -> bool {
        let addr = self.addr();
        let _ = HELD.try_with(|h| {
            let mut h = h.borrow_mut();
            if let Some(i) = h.iter().rposition(|a| *a == addr) {
                h.remove(i);
            }
        });
        loop {
            match self.offer.load(Ordering::SeqCst) {
                ACCEPTED => {
                    self.offer.store(HANDED, Ordering::SeqCst);
                    return true;
                }
                OFFERED => {
                    if self.offer.compare_exchange(OFFERED, NONE, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                        break;
                    }
                }
                _ => break,
            }
        }
        self.running.store(false, Ordering::SeqCst);
        false
    }

    fn addr(&self) -> usize {
        self as *const Self as usize
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

    /// **A ticketed caller that holds the run hands it over once answered, while another thread asks without pause**
    /// (the drafted waiter, 2026-11-28 — the path `revocation::refresh_answers` and `client_cert::refresh_answers` share).
    /// The waiter takes a ticket and finds no run in progress, so it holds the run; its first pass starts a thread that
    /// calls [`SingleFlight::run`] in a loop and waits until that thread has asked. Each pass sleeps 2 ms, so a request
    /// is waiting whenever a pass ends. The waiter returns within 10 s, having run at most three passes, and the asking
    /// thread's requests made after it left are run (by that thread, which accepted the run). Seen failing first with
    /// `run_for` as `run` (the code at `01bb75b2`): the waiter ran a pass for every request and did not return.
    #[test]
    fn a_ticketed_holder_hands_the_run_to_an_asker_once_answered() {
        use std::sync::atomic::AtomicBool;
        let sf = Arc::new(SingleFlight::default());
        let (mine, theirs, asks) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
        let stop = Arc::new(AtomicBool::new(false));
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let (s, m, t, a, st) = (Arc::clone(&sf), Arc::clone(&mine), Arc::clone(&theirs), Arc::clone(&asks), Arc::clone(&stop));
        let waiter = std::thread::spawn(move || {
            let mut asker = None;
            let ticket = s.ticket();
            loop {
                s.run_for(ticket, || {
                    if m.fetch_add(1, Ordering::SeqCst) == 0 {
                        let (s2, t2, a2, st2) = (Arc::clone(&s), Arc::clone(&t), Arc::clone(&a), Arc::clone(&st));
                        asker = Some(std::thread::spawn(move || {
                            while !st2.load(Ordering::SeqCst) {
                                s2.run(|| {
                                    t2.fetch_add(1, Ordering::SeqCst);
                                    std::thread::sleep(Duration::from_millis(2));
                                    true
                                });
                                a2.fetch_add(1, Ordering::SeqCst);
                                std::thread::yield_now();
                            }
                        }));
                        let t0 = std::time::Instant::now();
                        while a.load(Ordering::SeqCst) == 0 && t0.elapsed() < Duration::from_secs(10) {
                            std::thread::yield_now();
                        }
                    }
                    std::thread::sleep(Duration::from_millis(2));
                    true
                });
                if s.answered(ticket) {
                    break;
                }
                std::thread::yield_now();
            }
            let _ = done_tx.send(());
            asker
        });
        let returned = done_rx.recv_timeout(Duration::from_secs(10));
        let (by_waiter, asked) = (mine.load(Ordering::SeqCst), asks.load(Ordering::SeqCst));
        let before = theirs.load(Ordering::SeqCst);
        if returned.is_ok() {
            let t0 = std::time::Instant::now();
            while theirs.load(Ordering::SeqCst) <= before && t0.elapsed() < Duration::from_secs(10) {
                std::thread::yield_now();
            }
        }
        let after = theirs.load(Ordering::SeqCst);
        stop.store(true, Ordering::SeqCst);
        if let Some(h) = waiter.join().unwrap() {
            h.join().unwrap();
        }
        assert!(returned.is_ok(), "the waiter did not return in 10 s: it ran a pass for every other request ({by_waiter} passes, {asked} asks)");
        assert!(asked > 0, "the asking thread asked while the waiter held the run");
        assert!(by_waiter <= 3, "the waiter left within two passes of the one that answered it ({by_waiter} passes)");
        assert!(after > before, "requests made after the waiter left were run ({before} → {after} passes by the asker)");
    }

    /// **An asker that accepts the run as its ticketed holder ends is handed it, and runs its request**: the holder's
    /// ticket is answered and its last look found no request; a call made then (planted after the last pass, before
    /// the hold ends) finds the run held and offered, and accepts. The holder hands the run over instead of releasing
    /// it, and the taker runs the pass its call asked for. A pin of the hand-over at the end of a hold (it cannot fail
    /// first: there was no offer before).
    #[test]
    fn an_asker_that_accepts_as_the_holder_ends_is_handed_the_run() {
        let sf = Arc::new(SingleFlight::default());
        let passes = Arc::new(AtomicU64::new(0));
        let asker: Arc<std::sync::Mutex<Option<std::thread::JoinHandle<()>>>> = Arc::default();
        let (s, p, h) = (Arc::clone(&sf), Arc::clone(&passes), Arc::clone(&asker));
        AFTER_LAST_PASS.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                let s2 = Arc::clone(&s);
                *h.lock().unwrap() = Some(std::thread::spawn(move || {
                    s2.run(|| {
                        p.fetch_add(1, Ordering::SeqCst);
                        true
                    })
                }));
                let t0 = std::time::Instant::now();
                while s.offer.load(Ordering::SeqCst) != ACCEPTED && t0.elapsed() < Duration::from_secs(10) {
                    std::thread::yield_now();
                }
                assert_eq!(s.offer.load(Ordering::SeqCst), ACCEPTED, "the late call accepted the offered run");
            }))
        });
        let ticket = sf.ticket();
        let p = Arc::clone(&passes);
        sf.run_for(ticket, || {
            p.fetch_add(1, Ordering::SeqCst);
            true
        });
        assert!(sf.answered(ticket));
        let handle = asker.lock().unwrap().take();
        handle.expect("the plant ran").join().unwrap();
        assert_eq!(passes.load(Ordering::SeqCst), 2, "the taker ran the pass its call asked for");
        assert!(!sf.running.load(Ordering::SeqCst), "the taker released the run");
        assert_eq!(sf.offer.load(Ordering::SeqCst), NONE);
    }
}
