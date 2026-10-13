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

**Mixed fleet, corrected against the code:** the plan row said a node from before 2.3.0 is cut off; it is a node from
before **2.14.0** (no sealed record), and only from gossip-learned keys — a direct connection still anchors its
certificate key. `false` is the explicit opt-out; without `[tls]` the setting is inert and nothing warns.

**Pages touched:** [`security.md`](../security.md), [`operations.md`](../operations.md),
[`companions/commitment.md`](../companions/commitment.md); outside the wiki, the plan's row S and §3.5 block,
`deprecations.md` §41, `configuration.md`, guide 09/23/24, the threat model, `what-is-proven.md`, the runbooks.
