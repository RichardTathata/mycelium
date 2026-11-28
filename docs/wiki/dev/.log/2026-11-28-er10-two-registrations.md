## [2026-11-28] ingest | ER10: two registrations of one name do not withdraw each other

R6-06, R10-03, R21 "two editions", R22 "MCP replacement" (`docs/plans/external-engineering-review-2026-11-26.md`
ER10; tier 1 by D3). Every node-local registration of a name wrote one key, and each handle's task tombstoned it on
exit whoever else on the node still held it (`run_kv_persist_task`'s exit; `run_mcp_tool_task`'s and the bridge's
exits; the prompt backend's `Arc::ptr_eq` removal; the opacity watcher's `retain` by key).

**Design note.** The promise reading: a handle may withdraw only the promise it made, not a successor's. The guard is
node-local, above Layer I. A per-key table of live registrations, newest last
(`mycelium_core::kv_persist::Registrations<V>`, on `CoreCtx::soft_state_holders`; lock-order row 83), used through
`HeldKey`. The key carries the newest registration's value; an older one stands by (it computes its payload, which the
table keeps, and writes nothing). Withdrawing: under a newer registration, nothing; when it held the key and another
remains, the newest remaining one's value is written at once; the last tombstones. The update is stamped under the
table's lock, so the stamps' order is the decisions' order and LWW everywhere agrees; apply, sign and send run after.
Registration happens when `run_kv_persist_task` is called (it is now a plain `fn` returning the future), so order
follows handle creation; a `HeldKey` dropped without `withdraw` (an aborted task, a future never polled) withdraws in
`Drop` with a try-send. A successor's re-assert passes the soft-state gate, so nothing live is written after
shutdown's sweep. Rejected: compare-and-tombstone by the handle's own HLC stamp (the ER4 review's smallest fix) — it
fixes the replace-then-drop-old order but not R6-06's other order, where the last writer drops while an older holder
lives and the name vanishes until its refresh; a refcount alone — it cannot say which value the key should carry when
two registrations differ (R10-03's template, R22's schema, R21's filter); and any check in `apply_and_notify` (Layer I
must not learn a higher layer's law).

Covered by the same table: every `run_kv_persist_task` caller (`advertise_capability`, `declare_requirement`,
`define_capability_group`, `advertise_persistent`, the gateway client advertisement), MCP tools native and bridged
(only the newest registration's loop serves the name — two loops answered every call before), and prompt backends
(`TaskCtx::llm_skill_holders`, the same type with `V = Arc<dyn LlmBackend>`). The opacity watcher drops only cancelled
registrations of a key and re-adopts a live one. R21: a unit with two `[[requirement]]`s of one name and
different filters is refused at the doors that declare it on a node (`Stem::start`, `units/declare`; one `req/` record
per name carries one filter) — not at load, because `wire-check` reads such a file as shapes and an example
(`examples/units/semantic_coordination/agent-b.toml`, three editions of `compute/gpu`) and a `wire_check` test rely on
it; identical duplicates are one requirement.
ER5-F1's stem path (a late install registers `tools/x` after the new stem's and retires) now writes the live tool's
schema back; the late loop still serves for the moment it holds the name.

Out of scope, named: `gcap/` projections (ER9-F2, a definition rebuild inside one task family — ER9's), boundary
opacity per kind (one actuator per kind by contract, no exit tombstone), `grp/` membership (a set, not handles),
`sys/caller-context` (one writer). Not built: a periodic refresh of `tools/` keys (not needed for withdrawal), two
filters for one name in code (the newest's is published), a call at the instant of a replacement may be served by the
outgoing registration or by neither.

Pages: `architecture/runtime-invariants.md` (new section), `concurrency/lock-order.md` (row 83; row 21's note).
Outside the wiki: `docs/features/capability-discovery.md`, `mcp-tools.md`, `llm-prompt-skills.md`, `declare.md`, `stem.md`,
`docs/guide/deprecations.md` §TBD-ER10, `CHANGELOG.md`.

**Review round 1 (2026-11-28).** The opacity watcher now tracks each key's newest live registration (it kept the
oldest, so two filters of one name were judged by the one the record did not carry) and drops cancelled entries it
does not track; `register_written` registers nothing when its write is refused (an unwritable replacement had taken
the name from the tool whose schema the key carried); withdrawn and replaced values drop after the lock (row 83, also
taken under row 22); deprecations §TBD-ER10 names the API changes (`run_kv_persist_task` registers when called; new
public items) and the cross-node `cap-group/` limit; `unit-file.md`, `unit-files-and-wire-check.md`, `declare.md`,
`capability-discovery.md`, the `agent-b.toml` example and `mycelium-reason`'s `refresh_meta` doc updated.
