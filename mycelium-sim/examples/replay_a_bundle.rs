//! **Replay a bundle** — v3 item 6's decisive demonstration (`docs/plans/v3-contracts-axis.md`
//! §12.1).
//!
//! ```text
//! cargo run -p mycelium-sim --example replay_a_bundle
//! ```
//!
//! # The claim this exists to make checkable
//!
//! *A failure that depended on timing becomes a file you can re-run.* The kernel records every
//! decision a run asked it for — the clock, the randomness, the storage effects — and a **bundle**
//! is that recording plus what it takes to trust it: which build produced it, and which assertion
//! it was captured to reproduce.
//!
//! What a bundle buys, in one line: a bug stops being *"it failed on the third run yesterday"* and
//! becomes *"effect 7 differs, here is what changed"*.
//!
//! # The scenario
//!
//! A food-rescue depot sweeping its surplus-food offers: read the clock, draw a scheduling jitter,
//! and journal a verdict for each offer — still collectable, or past its collect-by time. Small on
//! purpose. The point is not the sweep; it is that the sweep's decisions are reproducible.
//!
//! The **bug** in step 4 is this example's own, not the library's: it drops the grace window a
//! depot allows for a late van. That matters for what the demonstration is allowed to claim — see
//! step 6, which is explicit that injecting a fault into the library itself is a test-only
//! affordance and stays that way.
//!
//! # What it does not demonstrate
//!
//! The corpus bundle for the real WAL/snapshot race (`mycelium-core/tests/replay-corpus/`) is
//! replayed by `the_checked_in_scenario_a_bundle_replays_here_and_matches_a_fresh_recording`, not
//! here: reproducing *that* failure means removing the WAL-tail merge, and the switch which does
//! that is `cfg(test)` on purpose. A binary that could disable a durability fix would be a
//! data-loss switch, and that is a worse thing to own than this demonstration is a good one.
//!
//! Nor does it show a multi-node schedule, or task interleaving (the scheduler seam's first arm
//! covers one node's waits — `docs/design/replay-nondeterminism-inventory.md` §3.1.1).

use mycelium_sim::bundle::{Build, Bundle};
use mycelium_sim::seams::{FsOutcome, Seams, Sources};
use mycelium_sim::{Kernel, Trace};

/// A surplus-food offer waiting for a van.
struct Offer {
    what: &'static str,
    collect_by_ms: u64,
}

/// The grace window a depot allows a late van. Dropping it is the bug injected in step 4.
const GRACE_MS: u64 = 30_000;

fn banner(s: &str) {
    println!("\n\x1b[1m{s}\x1b[0m");
}

fn note(s: impl AsRef<str>) {
    println!("   {}", s.as_ref());
}

/// The decision under study: for each offer, journal whether it is still collectable.
///
/// Every input it branches on comes from the kernel — the clock and the jitter — and every effect
/// it performs goes back through the kernel. That is the whole requirement for a replayable
/// decision: nothing enters or leaves except through the seam.
fn sweep(seams: &mut Seams<'_>, offers: &[Offer], grace_ms: u64) -> Result<Vec<String>, mycelium_sim::Divergence> {
    let now = seams.wall_now_ms()?;
    // A scheduling jitter, so the sweep does not synchronise across depots. Drawn whether or not it
    // is used, because a draw that happens only sometimes is a schedule that depends on data.
    let _jitter = seams.rng_u64("govern")? % 50;

    let mut verdicts = Vec::new();
    for offer in offers {
        let collectable = now <= offer.collect_by_ms + grace_ms;
        let line = format!("{} {}", offer.what, if collectable { "collectable" } else { "expired" });
        seams.fs("sweep.journal", "write_all", line.as_bytes(), 0, false, || FsOutcome::Ok(line.len()))?;
        verdicts.push(line);
    }
    Ok(verdicts)
}

fn offers() -> Vec<Offer> {
    vec![
        Offer { what: "bread-crates", collect_by_ms: 1_789_000_000_020 },
        Offer { what: "chilled-veg", collect_by_ms: 1_789_000_000_000 },
        Offer { what: "tinned-goods", collect_by_ms: 1_789_000_100_000 },
    ]
}

/// One run of the sweep against a given kernel, with a given grace window.
fn run(kernel: Kernel, grace_ms: u64) -> (Kernel, Result<Vec<String>, mycelium_sim::Divergence>) {
    let mut kernel = kernel;
    let mut sources = Sources::seeded(7, 1_789_000_000_010);
    let offers = offers();
    let result = {
        let mut seams = Seams::new(&mut kernel, &mut sources, "depot-north");
        sweep(&mut seams, &offers, grace_ms)
    };
    (kernel, result)
}

fn show_trace(trace: &Trace) {
    for entry in trace.entries() {
        note(format!("  {}", entry.to_line().replace('\t', "  ")));
    }
}

fn main() {
    let dir = std::env::temp_dir().join(format!("replay-bundle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    println!("\x1b[1mReplay a bundle — a timing-dependent failure, turned into a file\x1b[0m");

    // ── 1. Record ─────────────────────────────────────────────────────────────────────────────
    banner("1. Record the sweep — every decision it asked for, written down");
    let (kernel, verdicts) = run(Kernel::recording(), GRACE_MS);
    let verdicts = verdicts.expect("recording never diverges: there is nothing to diverge from");
    let trace = kernel.trace().clone();
    for v in &verdicts {
        note(format!("verdict: {v}"));
    }
    note("");
    note(format!("the kernel recorded {} decisions:", trace.len()));
    show_trace(&trace);
    note("");
    note("Read the columns: sequence · node · what kind of decision · which stream or file ·");
    note("what was asked · what came back. The clock reads and the jitter draw are inputs the");
    note("code branched on; the journal writes are effects it performed. Both are decisions.");

    // ── 2. Bundle ─────────────────────────────────────────────────────────────────────────────
    banner("2. Write a bundle — the trace, plus what it takes to trust it");
    let bundle = Bundle::new(trace.clone())
        .witnessed_by("replay_a_bundle::the_late_van_keeps_its_grace_window", None::<String>);
    bundle.write(&dir).expect("the bundle is written");
    let mut files: Vec<String> = std::fs::read_dir(&dir)
        .expect("read the bundle directory")
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string()))
        .collect();
    files.sort();
    note(format!("wrote {}", dir.display()));
    for f in &files {
        note(format!("  {f}"));
    }
    note("");
    note("`build.json` is the reason the rest can be believed: an exact replay is only meaningful");
    note("against a pinned build, and a trace replayed on a different commit is a *scenario*");
    note("replay — a weaker claim, which has to be visible rather than assumed.");
    let here = Build::current(&["sim"]);
    note(format!("this build: version {} · target {} · features {:?}", here.version, here.target, here.features));
    note("`witness.json` names the assertion the bundle was captured to reproduce. A bundle whose");
    note("witness is unknown is a bundle that cannot prove its own fix.");

    // ── 3. Replay ─────────────────────────────────────────────────────────────────────────────
    banner("3. Read it back and replay — the same schedule, a different moment");
    let read_back = Bundle::read(&dir).expect("the bundle round-trips");
    note(format!("witness: {}", read_back.witness.as_ref().map(|w| w.assertion.as_str()).unwrap_or("none")));
    let (kernel, replayed) = run(Kernel::replaying(read_back.trace.clone()), GRACE_MS);
    match replayed {
        Ok(v) if v == verdicts => {
            note(format!("replayed {} decisions, all matching — and the same verdicts came out.", kernel.position()));
        }
        Ok(_) => note("replayed without divergence, but the verdicts differ — which would be a bug in this example"),
        Err(d) => note(format!("unexpected divergence at seq {}", d.seq)),
    }
    note("No clock was waited on and no randomness was drawn: in replay the kernel *supplies* what");
    note("the recording observed, rather than re-observing it and hoping it agrees.");

    // ── 4. The failure, reproduced from the file ──────────────────────────────────────────────
    banner("4. Now break it — the bundle localises the bug to one effect");
    note("The bug: a depot drops the grace window it allows a late van (30s -> 0).");
    let (_kernel, broken) = run(Kernel::replaying(read_back.trace.clone()), 0);
    match broken {
        Err(d) => {
            note(format!("replay diverged at seq {}", d.seq));
            if let Some(expected) = &d.expected {
                note(format!("  recorded: {}", expected.to_line().replace('\t', "  ")));
            }
            note(format!("  replayed: {}", d.actual.to_line().replace('\t', "  ")));
            note("");
            note("That is the demonstration. Not *a test failed somewhere* — the exact effect that");
            note("changed, on the first run, with no timing luck and nothing to reproduce by hand.");
            note("");
            note("Read those two lines carefully, because they show what a trace is **not**. Same");
            note("file, same offset: what differs is the payload's digest and its length. The trace");
            note("records a *digest*, never the bytes — it is a log of decisions, not a copy of the");
            note("data, which is what keeps a bundle small and keeps payloads out of an artefact");
            note("that gets attached to bug reports. So the trace tells you **that** effect 4");
            note("changed and by how much; it does not tell you the offer's name.");
            let flipped: Vec<&String> = verdicts
                .iter()
                .filter(|v| {
                    let name = v.split(' ').next().unwrap_or("");
                    offers().iter().any(|o| o.what == name && 1_789_000_000_010 > o.collect_by_ms)
                })
                .collect();
            for v in flipped {
                note(format!("Which one was it? The example can say, because it holds the data: \"{v}\""));
                note("became \"expired\" without the window. The trace localises; the code names.");
            }
        }
        Ok(_) => note("unexpected: the broken sweep replayed clean, which would mean the bug never reached a seam"),
    }

    // ── 5. The fix, replayed green ────────────────────────────────────────────────────────────
    banner("5. Put the window back — the same bundle replays green");
    let (kernel, fixed) = run(Kernel::replaying(read_back.trace), GRACE_MS);
    match fixed {
        Ok(v) if v == verdicts => note(format!("{} decisions replayed, all matching. The fix is checked against the recording, not against a rerun.", kernel.position())),
        Ok(_) => note("replayed clean but the verdicts differ"),
        Err(d) => note(format!("unexpected divergence at seq {}", d.seq)),
    }

    // ── 6. The other shape of failure ─────────────────────────────────────────────────────────
    banner("6. The failure shape this scenario does not show");
    note("The bug above reached a seam, so the kernel caught it as a *divergence* and named the");
    note("effect. A bug whose effects never reach a seam — a wrong value computed and returned,");
    note("never written, never timed — replays perfectly and is caught by the bundle's **witness**");
    note("assertion instead. That is why a bundle names one, and why a bundle with an unknown");
    note("witness cannot prove its own fix.");
    note("");
    note("The real WAL/snapshot race is that second shape, and it is replayed by");
    note("`the_checked_in_scenario_a_bundle_replays_here_and_matches_a_fresh_recording` against the");
    note("corpus in `mycelium-core/tests/replay-corpus/`, not by this binary: reproducing it means");
    note("removing the WAL-tail merge, and that switch is `cfg(test)` deliberately. A binary that");
    note("could disable a durability fix would be a data-loss switch — worse to own than this");
    note("demonstration would be good.");

    // ── 7. What this did not show ─────────────────────────────────────────────────────────────
    banner("7. What this demonstration does not establish");
    note("· anything about more than one node, or about task interleaving beyond one node's waits");
    note("· that the recording captured *everything* — an unseamed source of nondeterminism is");
    note("  invisible here by construction, which is what the static seams check exists to stop");
    note("· a minimiser: this bundle is as long as the run was, and shrinking it is not built");
    println!();

    let _ = std::fs::remove_dir_all(&dir);
}
