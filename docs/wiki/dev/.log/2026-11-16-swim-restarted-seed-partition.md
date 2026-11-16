## [2026-11-16] ingest | #798: a restarted SWIM seed was partitioned for good once suspected

The Eve fleet job's intermittent `a write at node 1 reaching node 3 (probe/2) within 120000 ms` after the gateway's
restart. Root-caused with SWIM transition logs on all three nodes: a probe from node 2 landed in the gateway's
downtime (`Suspect`), the suspicion gossiped to node 3, and because the prober chose among `Alive` members only, no
one probed the restarted gateway again. It has `bootstrap_peers = []`, so it contacted no one either; it never heard
the rumour, never refuted it, and both peers promoted it to `Dead` four seconds later. Node 3 logged nothing because
`eve_fleet_node` installs no tracing subscriber — not because it saw nothing.

Fix in `mycelium-core/src/swim.rs` / `swim_membership.rs`: suspects stay probe candidates, a probe carries the
prober's rumour about its target, and a `Dead` or unheard-of bootstrap peer is re-probed once a suspicion window.
Page: [runtime-invariants](../architecture/runtime-invariants.md) § Fan-out activation is event-driven.

Not explained by it: O2 / #746 (a reply written into the TCP connection to a restarted peer's previous process); the
`emit_signal` lead in `call_mint_tests.rs` (no restart there).
