# 2026-10-30 — Y11: exposure at the take, claim and emit doors

Adopter-surface row Y11, decided as D29 (the maintainer, 2026-10-30): a tuple take (`take`, `take_by_key`), a
blackboard claim and the four emit doors (`signal.emit`, `mailbox.deliver`, `overlay.emit_reliable`, `shard.emit`) run
the gateway's decision only when an exposure rule selects their operation, with door-only lineage
(`src/agent/door.rs`; tests `tests/exposure_doors.rs`).

**Durable knowledge.**

- **Opt-in by the rule, not by the evaluator.** `door::decide` asks `selected` first — an attached policy, the profile
  (`legacy` is untouched) and the rules' patterns, no clock and no draw — so a node with no exposure rule does exactly
  what it did before, and a recording made before Y11 replays unchanged. Deciding these doors whenever an evaluator is
  attached would have changed every governed node's default and put a journal write on the busiest door: not built.
- **Door-only lineage.** The causal fields go on the door's own decision record and nowhere further. A signal frame and
  a tuple's WAL record have no field for them; carrying them onward would change the wire or the WAL. So X4's
  *a signal is not governed work* reads *not unless an exposure rule selects it* — `scripts/causal-doors.txt`'s new
  carries value `ruled`, and the registry gate now scans the companions' doors outside `src/` by symbol.
- **A Rust verb's actor is the node** (`node:{id}`, `ResolvedPrincipal::node`), recorded under `node:{door}`; a route's
  is the caller, under `gateway:{door}`. An exposure rule with `operation = "*"` selects these doors as well as the
  call doors (`deprecations.md` §73).
- **The collision refusal reaches the companions.** A door asking under an `EXEMPT_OPERATIONS` name is refused by name
  (`DoorRefusal::EXEMPT_NAME`), the companions' side of Y5's `protected_rpc_kinds` refusal.
