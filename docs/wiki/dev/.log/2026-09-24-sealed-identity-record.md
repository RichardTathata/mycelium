# Phase 3b — the sealed identity record: closing a race by construction — 2026-09-24

**What shipped:** `sys/identity-signed/{node}` — `version(1) ‖ history ‖ proof(96)`, the key history
and the proof that authenticates it in **one** KV entry. Written by every TLS node at startup and on
rotation, alongside the legacy `sys/identity/` + `sys/identity-proof/` pair. Readers prefer it; with
`require_identity_proofs` set, readers accept **only** it.

**Why:** the same day's revert log
([`2026-09-24-identity-proof-default-revert.md`](2026-09-24-identity-proof-default-revert.md)) ends
with a precondition — *an atomic identity+proof record, or a bounded pending state*. This is the
first of those. Two entries are two gossip messages with no ordering between them, so a peer
requiring proofs could learn the identity first, reject it, and hold no key until the proof landed.
Transient for the key, permanent for a leader election decided inside the window.

**The shape of the fix is the part worth remembering: closed by construction, not by timing.** The
alternatives on the table were all timing arguments — write the proof first (reduces the odds),
defer instead of reject (the key still is not there), retry (shrinks the window). Every one of them
leaves a race and buys a smaller probability. One record has no ordering to lose. When a race can be
made structurally impossible, do not settle for making it rare.

## Three things that were nearly wrong

1. **Rotation.** Readers *prefer* the sealed record, so `rotate_identity` had to write it too —
   otherwise a rotation would update the pair while every proof-requiring peer went on reading the
   **pre-rotation** history and never learned the new key. A silent, total failure to rotate. *Any
   time a reader gains a preference, every writer of the old thing becomes a bug until it writes the
   new one.*
2. **Parsing.** The proof is fixed-width, so `parse_sealed_identity` splits from the end — but it
   also checks the version byte and that the history is a whole number of 32-byte keys. A malformed
   value is **no record**, never a partial one, so it falls back to the legacy path instead of
   authenticating something half-understood.
3. **The compatibility claim.** Appending the proof to the *existing* `sys/identity/` value was the
   obvious move and is a trap: the value is a concatenation of 32-byte keys, and 96 extra bytes is
   still a multiple of 32, so an older node would have parsed the proof as **three more verifying
   keys** and merged them into `peer_keys`. A new prefix is not fastidiousness — it is the only
   option that an old reader ignores instead of misreading.

## What is pinned

- `a_sealed_record_is_accepted_when_proofs_are_required` — the sealed record authenticates with no
  sibling entry to wait for. This is the mechanism.
- `the_legacy_pair_is_refused_when_proofs_are_required` — **the canary.** If this ever stops
  holding, the window is back and the default must not be flipped.
- `the_legacy_pair_still_works_when_proofs_are_not_required` — the compatibility claim, so the
  record ships with no upgrade note.
- `a_sealed_record_cannot_introduce_a_foreign_key_for_an_established_node` — sealing changes how a
  record travels, never what authorises it.
- `a_malformed_sealed_record_is_no_record` — version, truncation, misalignment.
- `test_identity_proofs_required_two_nodes_still_authenticate` — end to end, two proof-requiring
  nodes. Its doc comment states plainly that it **cannot** measure the race (in-process, loopback,
  milliseconds) and would likely pass on the old path too. It proves the sealed path is wired and
  sufficient; the race's gate is the Docker suite. Saying so in the test is the direct application
  of [the testing lore written the same day](../testing/testing.md) — *a green run is evidence about
  that run.*

## What is left

Not a defect: a **rollout**. A node accepts what its peers publish, and a peer on an older release
publishes only the pair. So `require_identity_proofs` is still an opt-in, safe once every node
writes a sealed record (`docs/operations/cert-rotation.md` has the one-line check), and the
*default* should flip a release later — deliberately, by editing
`config::tests::the_default_requires_identity_proofs`, which is written to explain itself.
