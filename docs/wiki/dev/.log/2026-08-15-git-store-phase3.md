# ingest — council-substrate Phase 3: the write gate (2026-08-15)

**Shipped:** `GitStoreConfig::validate_cmd` — a deployment-supplied command run before every commit
with the candidate file in place (cwd = repo, argv + rel path); exit 0 admits, anything else
**refuses** with the output as findings, worktree restored either way (no commit, no residue).
Refusals travel as `WikiError::gate_refusal` — carried **inside `Io` as a typed payload**
(`GateRefusal`, detected by downcast via `as_gate_refusal`) rather than a new enum variant, so the
public error enum stays semver-additive. The curator treats a refusal as **drop-with-findings,
never retry** (a re-apply of the same content refuses again — retrying would wedge the queue):
proposal batch tombstoned, `Wiki::gate_refusals()` counts it, `warn!` carries the findings; clean
proposals keep applying.

**Design-record §9 question resolved:** per-apply gating with the deployment owning the command's
cost (FTT: the Node validator listed-only; their errors-block-warnings-pass policy is the command's
own exit-code contract — wrap exit-2-warnings to 0).

**Gates (green):** store-level (`the_write_gate_refuses_before_commit_and_leaves_no_residue` — a
stub validator refuses FORBIDDEN content: findings surfaced, `rev-list --count` unchanged, worktree
restored, refused create leaves no file, store keeps working) + curator e2e
(`gate_refused_proposals_are_dropped_and_the_curator_keeps_working` — refusal counted once, never
retried, subsequent clean proposal applies). Phases 4–5 (bulk ingest · work distribution) remain.
