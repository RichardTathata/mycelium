# Open: S12 leader election disagrees, intermittently, cause unknown — 2026-09-24

**Status: OPEN.** This entry exists because the failure briefly had a wrong owner, and a wrong owner
is worse than none — it stops people looking.

## The observation

`tests/overlay/scenarios/s12_leader_election.py` asserts that three nodes concurrently calling
`elect_leader("demo")` converge on one answer. On `8b588c6` it failed:

```
S12 leader election … AssertionError: Nodes disagree on leader:
{'overlay-a': '…0.3:57000', 'overlay-b': '…0.4:57000', 'overlay-c': '…0.4:57000'}
```

Two nodes said one thing, one said another. The federation two-mesh suite failed in the same run.
**Twelve consecutive greens** preceded it on earlier commits; **two greens** followed on later ones.

## What it is not

- **Not `require_identity_proofs`.** That commit flipped the default, and the failure was attributed
  to it for most of a day. The flag is only consulted from inside the
  `if let Some(ref tls_cfg) = self.config.tls` block in `src/agent/lifecycle.rs` — every identity
  writer and both readers (`prewarm_peer_keys`, `start_identity_watcher`) live there. The overlay
  nodes run `examples/three_node_demo.rs`, which contains **no TLS configuration at all**, and
  `tests/overlay/docker-compose.test.yml` sets no TLS env. With no TLS there are no `sys/identity/`
  entries, the validator never runs, and the flag is **inert**. Whatever `8b588c6` did to this test,
  it did not do through identity proofs — and `8b588c6` changed nothing else but docs and tests.
- **Not #369's rendezvous election rule.** That landed *after* the failing commit.

## Why the misattribution happened, since that is the reusable part

The reasoning was: *the flip was the only change in that commit, and the suite went red*. That is a
prior, not a mechanism. Nobody traced a path from the change to the assertion — which would have
taken one grep — before writing the conclusion into six documents. Recorded as rule 3 on the
[testing page](../testing/testing.md) §a green run is evidence about that run.

## Where to start

- The consensus path, not the identity path: `elect_leader` goes through gossip consensus, so the
  question is how three proposers converge and whether a node can conclude early on a minority view.
- The rate looks low (one in ~15 observed runs), so reproduction needs repetition:
  `make test-overlay` in a loop, capturing each node's view rather than only the assertion.
- Check whether the two-mesh federation failure in the same run shares a cause or was collateral —
  one red run, two suites, is itself a hint about the host rather than the code.
- **Do not assume it is a flake because it is rare.** "Nodes disagree on leader" is a
  correctness-class assertion; a low rate makes it harder to find, not less real.
