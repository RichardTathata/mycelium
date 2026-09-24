# The identity-proof default: flipped, and reverted a day later — 2026-09-24

**What changed:** `require_identity_proofs` is back to **`false`**. It was flipped to `true` on
2026-09-23 (`8b588c6`, unreleased) and reverted here. No released version carried the flip, so no
deployment is affected.

> **Correction, appended the same day.** The mechanism below was offered as the leading account of
> the S12 failure. **It cannot be**: every identity writer and both readers live inside the
> `config.tls` block in `src/agent/lifecycle.rs`, and the overlay suite's nodes are the demo binary,
> which configures no TLS. With no TLS there are no `sys/identity/` entries and the validator never
> runs — **the flag is inert in that suite**. S12's intermittency is unexplained and still open; it
> is not the flip, and not #369's election-rule change either (that landed after the failing
> commit). The revert stands on the plainer ground that a default flip nothing had demonstrated safe
> does not belong on main. The *window* described below is real for a TLS fleet and worth closing —
> which Phase 3b does — but it is a hazard, not the cause of anything observed.
>
> The error is the one this repo's testing page warns about, run backwards: a red run taken as
> evidence about a **cause**, on the strength of "it was the only change in that commit", without
> checking whether the mechanism could reach the test.

**Why the revert.** The Docker suite failed `S12 leader election … Nodes disagree on leader`
intermittently — three nodes, two answers — with the federation two-mesh suite failing in the same
run, after **twelve consecutive greens** on the preceding commits and two greens after. That
signature is a race, and the flip was the only change between the greens and it — which is where the
reasoning went wrong, above.

**Mechanism (offered at the time; see the correction above).** A node's identity and its proof are two separate
`kv_set` calls in `src/agent/lifecycle.rs` — two gossip messages, no ordering between them. With
proofs required, a peer that learns the identity first rejects it and holds no key for that node.
The *key* recovers by itself: `start_identity_watcher` subscribes to the broader `sys/identity`
prefix (not `sys/identity/`) precisely so a late proof re-validates its entry. **The window is
transient; a decision taken inside it is not.** A leader election is one-shot — a node that could
not verify a peer's consensus signature during the window does not re-run the election when the key
arrives.

**The transferable lesson, and the reason this is a `.log` entry rather than a one-line revert:**
flipping the default **broke no unit test**. Every test that exercises the behaviour sets the flag
explicitly, and the in-process suites have no cross-process ordering window to lose a race in. *A
config default whose only failure mode is a race between processes is not testable by the suite that
gates the PR.* The gate that could see it is the Docker suite, which runs after the merge. A green
`make check` on a default flip is not evidence; it is the absence of a gate.

**What a future flip has to clear.** Not "every node writes a proof anyway" — that is true, it was
the argument for the flip, and it is not the failing condition. The precondition is an **atomic**
identity+proof record, or a bounded *pending its proof* state that defers rather than rejects. That
is a design change.

**Kept from the attempt** (all of it still correct):

- `config::tests::the_default_requires_identity_proofs` — the value pinned **with its reason**, so
  flipping it back means editing a test that explains why it is there.
- `lib_tests::identity_proof_default::an_opted_in_configuration_rejects_an_unsigned_identity_entry`
  — renamed from `a_default_configuration_…`; the end-to-end join from config field to rejected
  entry, which was never what failed.
- `requiring_proofs_does_not_close_trust_on_first_use` — the TOFU boundary, unchanged, still written
  to fail if that window is ever closed.
- The runbook rewrite: `docs/operations/cert-rotation.md` now says *when to opt in* instead of
  prescribing a rollout finished ten releases ago.

**Pages touched:** [`security.md`](../security.md) (section rewritten — the flip's section became
the revert's), [`operations.md`](../operations.md) (default + one-line why),
`docs/operations/cert-rotation.md`, `CHANGELOG.md`, `mycelium-core/src/config.rs`,
`src/lib_tests.rs`, `src/agent/http.rs` (comments).

---

## Found on the way out: a test that stopped failing and started flipping a coin

The revert's own CI went red on something unrelated —
`mycelium-tuple-space::failover::auto_election_is_deterministic`, *"lowest candidate id did not win
the election"*. It is not a regression from the revert. The election rule became **rendezvous** on
2026-09-20 (`mycelium::election`, PR #369) and that test still asserted the rule it replaced.

**It did not start failing. It started being a coin flip.** The ports are kernel-assigned, so
whether `hash(ring, node)` favours the lower id is chance. It passed twice on `main` — which is why
#369 and #370 both merged green — and failed on the next branch that ran it. The branch that caught
it had nothing to do with elections.

That is the same shape as the bug this log is about, one layer up: **a green run is evidence only
about the run**. There, a default flip passed every gate that runs before merge because those gates
cannot host the race. Here, a stale assertion passed because the coin came up heads twice. Neither
was ever *checked*.

**Fixed by asserting the property that is actually deterministic**, not by pinning the other answer:
exactly one primary (which never depended on the rule), and the winner is **the node
`mycelium::election::winner` names**, computed in the test from the same ring name and candidate ids
the node uses. A future rule change now fails this test honestly, or passes it honestly, instead of
re-rolling. Verified eight consecutive runs.

Two stale comments went with it — `mycelium-wiki/src/agent.rs`'s sentinel comment said *"lowest id
wins"* fifteen lines above a `mycelium::election::elect` call, and the wiki failover test's module
note said the same. Both were only comments: the wiki's assertions are rule-agnostic (`is_curator()
^ is_curator()`), which is why they survived the rule change. **That is the lesson worth copying** —
the assertion that does not name the rule is the one that stayed true when the rule changed.
