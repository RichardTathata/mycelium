# 2026-10-31 — external review R2 and R3: the restart gate, and grant records on disk before the step they admit

External review 2026-10-31, finding F2 (plan `docs/plans/external-review-2026-10-31.md`, rows R2 and R3, PR #713).

**Durable knowledge.**

- **A graceful restart proves the exit snapshot, not the WAL.** `shutdown()` stops the WAL writer last, and the writer's
  exit snapshot is taken from the **store** — so every record the store held crosses the restart whether or not the WAL
  took it. Every restart gate (`a_restarted_agent_re_waits…`, `test --faults restart`, the acknowledged-replica gate)
  was of that shape; with the WAL append removed from `write_owned_kv` all of them passed. The WAL-only form
  (`test_util::sabotage_exit_snapshot`: the install target made a directory so the rename refuses and the WAL is not
  truncated; the start snapshot put back) is the shape that proves a record reached the WAL. A plant on one inbound
  append site in `connection.rs` did not fail it — a KV frame arrives through the `SignedData` arm — so a plant must
  cover every inbound append site.
- **Invariant 1 has a second set of persist-first users.** The grant records a step must not proceed without (the
  agent's `consumed`, a door's `used/{door}`, a revocation's checkpoint and `revoked`) go persist → apply → gossip through
  `kv_set_requiring_sync`, under `contracts-receipts.md` §2.2's exception; the request stays apply → `append_try`
  (`runtime-invariants.md` § Persistence, write-site list updated).
- **A durable refusal spends nothing.** A use that could not be recorded ran nothing, so its in-memory claim is released
  (`ExecutionAuthority::release_grant_use`): a retry after repair runs once, which is also what a restart answers.
  Keeping the claim spent would have made a refusal spend a use until the next restart — an asymmetry the review found.
- **`consumed` did not fail safe.** `await_grant` re-honours an un-consumed request (re-reads `…/issued`, verifies,
  admits); the single-use truth at a provider with no door is `consumed`, so it is durable before the commit.
