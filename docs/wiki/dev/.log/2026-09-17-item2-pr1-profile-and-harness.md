## [2026-09-17] ingest | item 2 PR 1 — the enforced profile and the two-mesh harness

Up: [dev](../dev.md) · record `docs/design/federated-domains.md` §1, §2, §9 · plan
`docs/plans/v3-contracts-axis.md` §5 · code `mycelium-core/src/config.rs`, `src/lib_tests.rs`.

Item 2 PR 1 is *"domain ADR + enforced profile + two-mesh harness"*. The ADR landed earlier today
(#242); this is the other two thirds.

### The profile is a refusal, not advice

`DomainProfile::{Open, Enforced}`, default `Open`, checked at `validate()` — **before a socket is
opened**, because by the time a node is listening a partner has something to talk to.

`Enforced` requires exactly two things, and each has a specific reason rather than a precautionary
one:

- **SWIM off.** Its control datagrams are unauthenticated UDP — `swim.rs` signs nothing (verified
  when the ADR was written: the only `sign` hit in that module is the word *signal*, in a comment).
  A membership protocol any host on the path can forge is not one to run at a federation boundary.
- **TLS required.** Admission *is* the domain (§1). Without a per-node CA root, "independently
  admitted" has no mechanism behind it and the profile would be a label.

**The refusal is loud on purpose.** `swim_failure_detector` defaults to *on*, so enabling the
profile without also turning SWIM off is rejected rather than silently satisfied by disabling SWIM
underneath the operator. Changing a liveness mechanism is not something a profile flag should do
behind your back.

That default is also what made the second test fail on its first run: the no-TLS case hit the SWIM
check first and asserted on the wrong field's message. The fix was to turn SWIM off in that test so
it exercises the check it names — a small thing, but it is the reason the test suite has a test per
*reason* rather than one test per profile.

### The harness asserts from the tables, and proves it is not vacuous

§13's release gate says non-merger must be proved *"from membership tables, consensus state and
traces"* — **not from a narrative**. So `two_meshes(n)` builds two meshes that share no bootstrap
peer, and `assert_never_merged()` checks:

1. **membership** — no foreign node in any peer table (a foreign node there is one the failure
   detector, the fan-out and quorum sizing would all count);
2. **the native namespaces** — no `cap/` · `grp/` · `sys/` key naming a foreign node, because
   foreign state is indistinguishable from local state once it is in the medium.

The half that makes it worth having: the same assertions are run against a **deliberately merged**
pair and are required to **fail**. Without that, a harness that checked nothing would pass the test
just as happily — and "two meshes did not merge" is exactly the kind of claim that is vacuously true
when the setup silently did not work. The structural poll guards the same failure from the other
side: each mesh must actually form first, or non-merger is true because nothing connected.

### What it does not yet prove

Today both meshes are trivially separate — there is no federation edge for them to leak across. The
harness is **scaffolding**: PRs 2–7 add the edge and re-run these assertions with it present, at
which point they stop being trivially true and start being the thing under test. Recorded here so
nobody later reads a green test as evidence of a property it could not yet have tested.

### Gates

`make check` clean · core **191** · mycelium **461** (`tls,metrics,a2a,llm`) / **523**
(`compliance,a2a`).
