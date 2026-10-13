## [2026-10-13] ingest | identity proofs on by default (post-360 row S)

**What changed:** `require_identity_proofs` defaults to **`true`** (unreleased, after 2.33.0;
`docs/guide/deprecations.md` §41). A TLS node merges a peer's keys only from its sealed record
(`sys/identity-signed/`); a bare `sys/identity/{n}` key list without a proof is refused and counted.

**The 2.14.0 attempt, re-examined first.** `8b588c68` (#368) flipped it; `7cb3b872` (#372) reverted it for the
Docker suite's `S12 leader election … Nodes disagree on leader`. The flag could not have caused that — it is inert
without TLS and the overlay suite runs none (#373's correction) — and S12 was an election with a quorum of one,
closed by the electorate refusal (#377; [the S12 log](2026-09-24-s12-leader-election-open.md)). The ordering window
the revert named was real and the sealed record (2.14.0) closes it by construction.

**What it closes:** the identity-poisoning residual rows F and Q left — a member garbling another node's sealed record
and proof and writing a bare key list naming its own key, a valid frame under its own signature, reached the
verify-only history set (forged audit history) and, on a legacy certificate, the live trusted set (impersonation).
Both seen failing on 4251d50a by the new tests in `src/lib_tests.rs`.

**Two defects the flip exposed — the transferable part:**
- A field-level `#[serde(default)]` on a `bool` is `bool::default()` = `false`, **whatever the struct's `Default`
  says** (the container-level `#[serde(default)]` is overridden by the field's). Flipping `Default` alone would have
  left every TOML-configured node off. The field now has its own `default = "…"`. A default flip touches both.
- The environment override read any value but `1`/`true`/`TRUE`/`yes` as `false` — harmless while `false` was the
  default, a silent fail-open once it is not. Strict now, like `GOSSIP_GATEWAY_ALLOW_UNAUTHENTICATED`.

**Mixed fleet, corrected twice.** The plan row said a node from before 2.3.0 is cut off; the PR first said
before 2.14.0 (no sealed record); the review (finding 2) showed the real boundary is 2.32.0 and was already
drawn by 2.33.0: since row F only a tagged proof is accepted, in either posture, so 2.3.0–2.31.x peers are not
learned from gossip with or without the flag. The default newly refuses only a node from before 2.3.0 (no proof
at all) — the one node `false` re-admits. Lesson: a compatibility boundary is read from every rule a record
passes, not from the one being changed.

**Rotation laundered a planted key (review finding 1).** `rotate_identity` built the new history from the raw
`sys/identity/{self}` — any member can write it — and signed it with the trusted prior key into the sealed
record, so with proofs on a planted key still reached the history set at every peer. Row Q had fixed the start
path (`own_history_at_start`) and not this second writer of the same record; the door grep had searched
`kv_ns::IDENTITY` and missed `format!("sys/identity/{}")`. Rotation now uses `revocation::own_history`.

**Pages touched:** [`security.md`](../security.md), [`operations.md`](../operations.md),
[`companions/commitment.md`](../companions/commitment.md); outside the wiki, the plan's row S and §3.5 block,
`deprecations.md` §41, `configuration.md`, guide 09/23/24, the threat model, `what-is-proven.md`, the runbooks.
