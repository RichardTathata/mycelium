## [2026-10-16] ingest | a caller envelope is served once (post-360 row E's nonce cache)

**What changed:** a gateway's caller envelope carries an id (`n`, the gateway's HLC at framing) and a second
signature over the v1 fields plus the id (`sn`, `mycelium:gateway-caller:v2`). A provider admits each
`(gateway, issue time, id)` once at its receive boundaries (`gateway_caller::verify_received` → `ReplayWindows::admit`)
and drops a replay unanswered. Signed envelopes only; per-gateway window pruned at the freshness bound and capped at
65 536, with an eviction floor that fails closed. Lock-order row 60. `deprecations.md` §45; `rbac.md`'s caller table.

**The transferable parts:**
- *Key the cache on something unique, not on content.* Ed25519 is deterministic and the issue time is in
  milliseconds, so two legitimate identical calls in the same millisecond carried identical signatures: a cache keyed on
  the signature would have dropped the second. The RPC correlation nonce is random but outside the signature (and the
  envelope is framed before it is chosen), so the id had to go inside the envelope and under a signature.
- *A stateful check belongs at the boundary, once.* `verify` is called again by handlers (`request_principal`, provider
  enforcement) on the request they were handed; a replay window inside it would refuse the legitimate second read. The
  window lives in `verify_received`, called once per received frame.
- *Do not answer a replay.* RPC replies are matched by correlation nonce and sender; a refusal for a replay that reuses
  the original's nonce could reach the gateway before the real answer.

**Enumeration (rule 1):** `git grep -n "RpcRequest::from("` over `src mycelium-*/src` — every place a request is
built from a raw signal: `rpc_rx` (`rpc.rs`), the MCP tool loop and the MCP bridge (`mcp.rs`), the SDK serve stream
(`http/mesh.rs`), `llm.invoke` (`llm.rs`), the explain responder (`emergent.rs`). All six use `verify_received`. The
bridge had verified the context only inside provider enforcement (off by default) — now refused `-32022` as the tool
loop does. Later readers (`provider_enforcement::check`, `request_principal` in handlers) keep the stateless `verify`.

**The review (rule 4) moved the design.** Its first finding: every new envelope still carried a valid v1 signature,
so a replayer stripped the id and was served on every receipt — the window protected nothing this release. Fixed with
a separate self-owned marker, `sys/caller-context-id/{node}`: a new marker *value* would have changed what a 2.34
reader of `sys/caller-context/` sees (it compares the value to `"2"` exactly) and dropped its envelope promise
mid-upgrade. The same thinking found what the review had not: the signature did not name the **target**, so a window
per provider still let a capture be served once at every other provider of the same kind — `o` is now inside the v2
signature. Also from the review: three boundaries untested (now one test each, every one seen failing alone),
a public raw-signal door (`GossipAgent::receive_gateway_caller`), windows never removed (a sweep, with `admit`
re-checking its window under the lock), and the cap's arithmetic (600 s at the default drift bound, not 300 s).

**Tests:** `a_replayed_envelope_is_refused_at_the_receive_boundary`,
`an_envelope_id_and_target_are_signed_and_stripping_them_is_refused`, `rpc_rx_drops_a_replay_unanswered`,
`the_mcp_tool_loop_drops_a_replay_unanswered`, `the_replay_window_is_bounded_and_fails_closed`,
`an_empty_replay_window_is_swept` (`src/agent/gateway_caller.rs`); `the_mcp_bridge_refuses_a_replayed_or_failed_context`
(`src/agent/mcp.rs`); `every_receive_boundary_drops_a_replay_unanswered`,
`a_captured_envelope_is_refused_at_another_provider_and_stripped` (`src/lib_tests.rs`).

**Pages touched:** [lock-order](../concurrency/lock-order.md) (row 60); `docs/operations/rbac.md`,
`docs/guide/deprecations.md` §45, the post-360 plan (E's table entry, §3.5 block and §3.6); `src/lib.rs`'s namespace
table (`sys/caller-context-id/`).
