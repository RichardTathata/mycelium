# 2026-09-19 — item 1's decisive demonstration: the receipt ladder

**What shipped.** `examples/receipt_ladder.rs`; CI steps that **run** the three decisive
demonstrations rather than only building them; gallery rows for all three; plan §12.1 row for item 1
marked delivered.

**Why this example and not a test.** Every rung already has tests. What no test gives is the
*contrast* — the same write, four ways, side by side, where the differences are visible as different
claims rather than as different assertions in different files. The ladder's whole point is negative:
`Buffered` is not a weaker `OnDisk`, it is a different fact. That reads as a table or as nine
printed steps; it does not read as a test suite.

**The two things it does by doing.** A peer is shut down and the next write's rung 3 reports it
**unknown**, not failed — a timeout is never a negative, and here that is a line of output rather
than a paragraph in an ADR. And the persisted node's data directory is reopened, so what replayed is
read back from the store rather than asserted.

**The overclaim caught on the way, worth recording.** The first draft called step 8 "a real process
crash" and said it showed `Buffered` surviving one. It does not: the example shuts the node down
**cleanly**, so the bytes had every chance to reach the file, and a `Buffered` record surviving that
shows the WAL replaying and nothing about durability under failure. Both failures `Buffered` declines
to survive — a kill, and power loss — are outside one cooperating process. The header and step 8 now
say so. The demonstration is weaker than the first draft claimed and is now accurate, which is the
trade this project makes every time.

**A detail the refusal earned.** `set_requiring_sync` on a node with no persistence returns
`DurabilityNotEstablished { persistence_configured, reason }`, not a generic rejection — and the
example prints the flag, because it distinguishes a misconfigured node from a node whose disk failed.
Those are different problems for whoever is holding the pager, and the receipt already knew.

**The build failure that CI caught and local work did not.** The first version used
`mycelium::test_util::alloc_port`, which lives behind the `test-util` feature. It built locally
because I built it *with* that feature — but CI's long-standing step is
`cargo build --examples --features tls,metrics,a2a,llm`, which builds **every** example without it,
so the whole step failed. The rule worth keeping: **an example is not a test, and must build under
the ordinary feature set**, because one example that needs a test-only feature breaks the build of
all of them. The port helper is now four lines of `std::net` in the example itself, with a comment
saying why it is not the shared one.

**CI runs them now.** `cargo build --examples` proves the API still compiles; it does not prove the
demonstration still demonstrates. The three decisive demonstrations (`receipt_ladder`,
`knowledge_layer`, `federated_domains`) are each a `cargo run` step, seconds each, exiting non-zero
on a panic.

**Still owed by §12.1:** item 4's control-envelope viz, item 5's curator handover, item 6's
replay-a-bundle CLI.
