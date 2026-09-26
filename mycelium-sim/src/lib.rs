//! `mycelium-sim` — the deterministic replay harness (v3 contracts axis, **item 6**).
//!
//! Plan: `docs/plans/v3-contracts-axis.md` §4. Schema of record:
//! `docs/design/replay-nondeterminism-inventory.md` §5, fixed by PR 1 — this crate implements it
//! and does not get to redesign it.
//!
//! # What this is for
//!
//! *A seed is not a durable reproduction artefact.* A run reproduced by re-seeding is reproducible
//! only while the code is unchanged, which is exactly when nobody needs it. This harness records
//! **what the production code asked for and what it received**, and a replay checks each request
//! against the recording — so a changed build tells you *where* it first departed, which is the
//! question a reviewer actually has.
//!
//! # The pieces
//!
//! | Module | What it is |
//! |---|---|
//! | [`trace`] | the choices trace — one line per decision, kind · stream · canonical request · result |
//! | [`kernel`] | one decision at a time: record it, or check it against the recording |
//! | [`seams`] | what production reaches the kernel through — two clocks, named RNG streams, storage, inputs |
//! | [`bundle`] | what a failing run leaves behind and a reviewer replays |
//!
//! # Scope of PR 2
//!
//! The kernel, the clock and RNG interfaces, record/replay, and a bundle sufficient for **exact**
//! reproduction. The storage and channel **adapters** (PR 3), the timer seam every periodic loop
//! ticks through, and the scheduler seam (v2.9.0) have since landed in `mycelium_core::sim_seam`,
//! and the static forbidden-call check is `scripts/check-sim-seams.sh`, run by `make check` against
//! a baseline of permitted call sites. (This paragraph said "both PR 3 … until those land" long
//! after they had; doc-coverage run 17.)
//!
//! # The gate
//!
//! `tests/divergence.rs`: record a run, replay it with one WAL record's bytes changed **at equal
//! length**, and require a divergence. A harness that passes that test unchanged is not detecting
//! divergence, only re-seeding.

pub mod bundle;
pub mod kernel;
pub mod seams;
pub mod trace;

pub use bundle::{Bundle, BundleError, Witness};
pub use kernel::{Divergence, Kernel, Mode};
pub use seams::{fs_request, FsOutcome, Seams, Sources};
pub use trace::{Choice, ChoiceKind, Trace, TraceError};
