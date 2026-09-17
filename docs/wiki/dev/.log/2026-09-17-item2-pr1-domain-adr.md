## [2026-09-17] ingest | item 2 PR 1 — the federated-domains ADR, and a check the plan said existed

Up: [dev](../dev.md) · record `docs/design/federated-domains.md` · plan
`docs/plans/v3-contracts-axis.md` §5 (D5, D6, D7, D25) · code `scripts/check-kv-namespaces.sh`.

### Why this one, now

Phase A's exit gate requires *"item 8 published and cited by items 2/3/5's PR-1 ADRs"*. Item 8
(`docs/threat-model.md`) has been published since 2026-09-13; none of the three ADRs existed, so the gate was
blocked on documents rather than code. This is the first of the three.

### The anchors were re-verified, because they have been wrong before

The plan carries a note — *"anchor corrected 2026-09-13: there is no `federation_facts.rs`"* — so every claim
the ADR makes about the code was checked again rather than copied:

| Claim | Verified |
|---|---|
| SWIM control datagrams are unauthenticated | `swim.rs` signs nothing — the only `sign` hit is the word *signal* in a doc comment |
| intra-domain admission is a per-node CA root | `tls.rs:410`, `RootCertStore` |
| the A2A edge exists and is enforced | `a2a.rs` — `/.well-known/agent.json`, `/a2a` |
| OIDC owns the verifier and its allowlist | `oidc.rs:24,32` — `jsonwebtoken`, `ALLOWED_ALGS` |
| the egress rule already exists | `mycelium-core/src/config.rs:202` — `EgressPolicy::allow_hosts` |
| AgentFacts is the public descriptor | `mycelium-agentfacts/src/crdt.rs:24` — `FACTS_PREFIX = "facts/"` |
| no `federation/` KV prefix today | no such literal in `src/` or `mycelium-core/src/` |

### D5, decided

The plan left it explicitly open: *"Either the federation call **is** A2A with domain-bound origin credentials,
or the ADR states why `POST /federation/v1/call` must exist."*

**The federation call is A2A.** Three reasons, in order of weight: the edge already exists and is already
enforced (a second one starts at zero on auth, authorization, evidence and rate limiting); two invocation edges
with different auth models is exactly the shape v2.4.1 and v2.4.2 were spent removing, and this is the one place
where the caller is by construction not ours; and a capability invocation maps onto an A2A skill without losing
`origin`, which the federation-aware adapter preserves to the provider.

The record also states **what would reopen it** — a property A2A cannot express, such as streaming semantics
incompatible with `tasks/sendSubscribe` — so the decision has a stated way to be wrong rather than being
permanent by default.

### The finding: a check the plan said existed

§5 states, of the no-`federation/`-KV-prefix invariant: *"The wiki lint's namespace sweep checks it."*

**It did not.** `scripts/` held one check (`check-sim-seams.sh`); nothing swept namespaces, in CI or anywhere
else. The invariant was real, currently held, and entirely unenforced — so the first accidental violation would
have been found by a reviewer noticing, or not at all.

Rather than soften the ADR to match, the check was written: `scripts/check-kv-namespaces.sh`, in `make check`
and CI. It fails on a forbidden prefix literal in production code, naming the file, the line, and **the record
that forbids it** — so the failure tells you where the rule comes from, not just that you broke one. Test code
is exempt, for the same reason it is in the forbidden-call check.

**Verified in both directions**, which is the discipline this axis has settled on: a violation planted in
production code is caught with its citation; the same literal inside a `#[cfg(test)]` module is correctly
ignored. A check that has only ever passed is a check nobody has tested.

Its limits are stated in its own header rather than assumed away: a prefix assembled at runtime, or reached
through a constant defined elsewhere, is invisible to it. It catches the way the mistake is actually made —
someone writes the literal — and does not pretend to be a type system.

### The general shape, worth keeping

This is the third time on this axis that **a plan sentence asserting a gate turned out to describe one that did
not exist** (the others: `mycelium-core --features sim` tested but never linted; `seed_sender_log` with no test
in either representation). The pattern is not carelessness in the plan — it is that *"X is checked"* reads as a
statement of fact when it is usually a statement of intent, and nothing in a document distinguishes the two.
Running the check is what distinguishes them.
