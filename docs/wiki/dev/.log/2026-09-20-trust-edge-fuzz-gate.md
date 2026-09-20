# [2026-09-20] ingest | §12.6's trust-edge fuzz gate, and the two defects it found

Up: [dev](../dev.md) · pages [testing](../testing/testing.md) · plan `docs/plans/v3-contracts-axis.md` §12.6 ·
PR #329, on top of the Phase-C audit work (#325–#328).

**What it was.** §12.6's remaining requirement: *fuzz targets for every new parser on a trust edge
(trust bundles, the edge protocol frames, replay bundle decoding)*. Eight shipped, and the work of
writing down what each parser is relied on for found two defects.

## What shipped

Eight targets, covering the three surfaces §12.6 names — trust bundles, the edge protocol frames,
replay bundle decoding:

| Target | Parser | Crate |
|---|---|---|
| `caller_frame` | `split_frame` — the RPC frame classification | `mycelium` |
| `caller_envelope` | the envelope JSON, `via`, the two base64 fields | `mycelium` |
| `presented_call` | `PresentedCall::from_header_value` | `mycelium` |
| `catalog_reply` | `CatalogReply` deserialization | `mycelium` |
| `federation_objects` | `DomainDescriptor` / `DomainPolicy` | `mycelium` |
| `trust_bundle` | `TrustBundle` | `mycelium` |
| `replay_trace` | `Trace::parse` | `mycelium-sim` |
| `replay_bundle` | `FsOutcome::decode`, the flat-object reader | `mycelium-sim` |

Nightly `cargo-fuzz` at 120s each, plus mutation passes in the PR-time in-suite mini-fuzz
(24,889 inputs). The mini-fuzz CI line gained `tls` — without it two of the eight are not compiled
and the pass silently covers six.

## The governing idea

**Assert the invariant the parser is relied on for, not that it survived.** A crash is the easy
case. What ships is a *wrong-but-well-formed* parse. So: byte conservation for the frame, and
round-trip / signing-field stability for every object whose signature covers bytes rebuilt from
the parsed fields.

## Two defects, both found by writing the invariants down

**1. A derived `Deserialize` on a validating newtype validates nothing.** `DomainId::new` enforces
`[a-z0-9.-]` and a 253-byte cap, and the type documents why: *two ids differing only in case are one
domain to a human and two to a `HashMap`, and the place that difference surfaces is a trust
decision.* The derive wrote the inner field directly, so the rule held only for **constructed** ids
— while most `DomainId`s are **parsed**, from partner bytes, before verification. Measured against
the unfixed code, `UPPER`, `sl/ash`, a newline, `""` and a 10 KB id all parsed cleanly.

Bounded and worth not overstating: it **could not forge authority** — an id no trust bundle holds a
key for is refused whatever its spelling. What it admitted was `Depot`/`depot` as two entries read
as one, a `/` making `federation:{domain}/{principal}` ambiguous about where the domain ends, a
newline reaching a pre-authentication log line, and unbounded length. `PrincipalId` and `TermId`
had the same gap. Fixed with a manual `Deserialize` through the constructor.

**Generalisation worth carrying:** every newtype whose constructor validates needs a manual
`Deserialize`, or the invariant is false on exactly the path that matters.

**2. `mycelium-sim`'s bundle codec lost fields.** `unquote` used `trim_matches('"')`, stripping
*every* trailing quote rather than the one delimiter, so a value ending in an escaped quote came
back mangled. And `quote` never escaped newlines although `parse_object` iterates `text.lines()`, so
a newline-bearing value was **truncated and the rest dropped silently**. `witness.assertion` is free
text, so a replay would check a weaker assertion than the one recorded and report success — a silent
divergence in the crate built to make divergence loud. Bundles already on disk read back unchanged.

## Three method lessons

- **`parse → write → parse` is the weak invariant.** It passed everything, because a stably-lossy
  reader reproduces its own mangling. The defects only appear starting from the **value**:
  `write → read` fidelity. A green target asserting the weaker property looks exactly like a green
  target asserting the right one.
- **A seed that does not reach the layer tests nothing.** The first `caller_envelope` seed was merely
  frame-shaped, so `serde_json` refused it and every envelope assertion was unreachable. The case
  count gave it away — identical before and after adding a whole layer. The mini-fuzz now asserts its
  own seed's reachability before mutating it.
- **A plant caught by a panic has not verified the assertion.** The first frame plant was an
  off-by-one that tripped a slice panic; that proves the harness runs, not that the invariant is
  checked. Replaced with a silent byte-drop, which the conservation assertion caught by name.

## Plants, all observed to fail

| Plant | Caught by | Message |
|---|---|---|
| `DomainId` validation removed | mini-fuzz **and** the targeted test | *a policy parsed an id no constructor would make* |
| `split_frame` drops the app bytes | mini-fuzz | *framing lost or invented bytes* |
| `unquote` back to `trim_matches` | fidelity test | *field "quote" did not survive…* |
| `quote` not escaping newlines | fidelity test | *field "newline" did not survive…* |

The first is the one that matters: the mini-fuzz rediscovered it **by mutation alone**, independently
of the test written for it.

## Known and not fixed — four further trust-edge parsers

Named rather than quietly omitted; each is a different subsystem, not the same parser one layer down:

- `src/agent/journal.rs:332` — a `u32` length straight from the file into `vec![0u8; want]`, with
  `max_bytes` checked *after* the read and by design never for the first record. Under both the
  evidence journal and the rights ledger.
- `src/control/ledger.rs:333` — `PublishedRightsHead::decode`, peer-writable gossip bytes through the
  hand-rolled `serde_fixint`, before `verify_published_head`.
- `src/control/ledger.rs:362` — `RightsLedger::open`, every on-disk `LedgerEvent`, unsigned.
- `mycelium-commitment/src/lib.rs:482` — offers and awards decoded from the gossip log; `award()`
  picks a winner from them and neither type is signed.

## Process note

The PR first showed CI green on a single check. It had not run: #328 was squash-merged to main while
this branch carried the unsquashed commit, leaving the PR `CONFLICTING`, and **GitHub silently skips
`pull_request` checks on an unmergeable PR**. Only the CLA gate (`pull_request_target`) had run.
Check `gh pr view --json mergeable` before reading a short check list as success.
