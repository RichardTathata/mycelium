# [2026-09-20] ingest | measuring the trust-edge risk list, and two tests that proved less than their names

Up: [dev](../dev.md) · pages [testing](../testing/testing.md) · plan `docs/plans/v3-contracts-axis.md` §12.6 ·
PRs #330, #331, following [`2026-09-20-trust-edge-fuzz-gate.md`](2026-09-20-trust-edge-fuzz-gate.md).

**What it was.** #329 shipped eight trust-edge fuzz targets and named four parsers it had *not*
covered. This closes that list — one fixed, three measured — and records two occasions where a test
claimed more than it could detect.

## The list, measured

An unqualified list of "unfuzzed parsers" reads as a list of vulnerabilities. Mine did, and it
overstated the risk. Each was then measured rather than left as a worry:

| Parser | Outcome |
|---|---|
| `agent/journal.rs` framing | **Real, fixed (#330).** Two defects. |
| `control/ledger.rs:333` | Reserved surface — returns `Option`, cannot panic, every caller is a `#[test]`. |
| `control/ledger.rs:362` + `serde_fixint` | Live, and **survives** 20k mutations: 3,808 decodes, 0 panics. Fuzz-targeted anyway (#331). |
| `mycelium-commitment/src/lib.rs:482` | `serde_json`, memory-safe. The gap is **provenance, not a decoder bound** — still open. |

**The lesson to carry:** publish the measurement with the list. "Unfuzzed" and "vulnerable" are
different claims, and only one of them was true here.

## The journal's two defects (#330)

**A torn tail was counted as a record.** `count_records` decided a record was complete by seeking
past it and checking the seek returned `Ok` — but **seeking past the end of a file is legal and
succeeds**. On a file with one complete record followed by a prefix claiming 256 MiB,
`read_journal_from` said 1 and `count_records` said 2. Not cosmetic: `count_records` drives the next
append's sequence number while `read_journal_from` is what an exporter reads, so after a crash
mid-append the next record took a seq no reader would hand out — a hole in the sequence, on the
evidence-journal path that exists for compliance export.

**A record was allocated for before the file was known to hold it.** A `u32` length prefix went
straight into `vec![0u8; want]`; `max_bytes` is checked *after* the read and by design never for the
first record. One flipped bit asks for up to 4 GiB. The journal is node-local, so this is corruption
rather than an attacker — and a corrupt journal should be reported by the reader that finds it, not
resolved by the allocator.

## Two tests that proved less than their names

**1. The allocation bound has no failing test, and saying so is the point.** Deleting the bound left
every assertion passing — tried, not assumed. `vec![0u8; want]` goes through `alloc_zeroed`, which
the OS satisfies with **lazy zero pages**: a 4 GiB request succeeds instantly, the next `read_exact`
fails, and the function returns exactly what it returns with the bound in place. The observable
outcome is identical by construction.

Resolution: extract the rule into a `record_fits` predicate that *is* falsifiable (planted an
off-by-one; it failed by name), rename the behavioural test to what it shows, and write the limit
into its doc comment. Gating the allocation itself would need a counting `#[global_allocator]`
across the whole test binary — one per binary, all 548 tests, TLS-recursion hazards in the allocator
path. Rejected as disproportionate; revisit if this family recurs.

**2. Noise tests the entry check; only mutation tests the decoder.** A first `serde_fixint` sweep of
**40,000 random inputs produced zero successful decodes** — noise dies on the first length prefix, so
the interior was never entered and "0 panics" measured nothing. Mutating a valid encoding instead
gave 3,808 decodes and a real answer. This is the same trap as #329's frame-shaped envelope seed,
reached from the opposite direction, which is why **every mini-fuzz seed now asserts its own
reachability** before being mutated.

The shared shape: *a green run is evidence only about the inputs that actually arrived*. Both times
the tell was a number that did not move — an unchanged case count, a zero decode count.

## What `serde_fixint` got anyway

A fuzz target, despite the sweep finding nothing, because a 20k mutation sweep is far weaker than
coverage-guided fuzzing and a hand-rolled binary decoder reading bytes off disk is precisely the
shape behind the unbounded-allocation decode DoS that sat uncaught through M2 Run-20. It covers the
two types that reach the codec from outside the process — an unsigned on-disk `LedgerEvent` and a
peer-writable `PublishedRightsHead` — asserting round-trip stability, not byte equality, because
`from_slice` deliberately tolerates trailing bytes.

Mini-fuzz: 25,870 inputs, up from 24,889.

## Still open

`mycelium-commitment`'s unsigned `Offer`/`Award`. A peer publishing an offer is what an open
contract net is *for*, so the question is whether an offer should carry provenance — a design
decision, not a parser bound, and named rather than quietly closed.
