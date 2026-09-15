## [2026-09-15] ingest | the reason-node CI job gated on `/health`, then dispatched through the gateway

Up: [dev](../dev.md) · page [testing](../testing/testing.md) §*Structural polling, not fixed sleeps* ·
code `.github/workflows/ci.yml`, `src/agent/lifecycle.rs` (the marker publication).

**Signal.** Two `TestCallTyped` cases failed with HTTP 412 `provider_without_caller_context` on
PR 219, the first CI failure since 2026-09-14.

**Not a product bug.** Under the default secure caller-context profile (item 7) a gateway refuses to
route to a provider whose `sys/caller-context/{node}` marker it cannot see, rather than silently
running the call under the node's own identity. The 412 is that refusal working exactly as designed —
the confused-deputy fix doing its job.

**The defect is the readiness gate.** The job polled `/health`, which proves the HTTP server is
listening, and then dispatched through the gateway immediately. The marker is published **lazily** —
once a peer connects, or after a 5 s grace — precisely so the write cannot land in an outbound-writer
reconnect-backoff window (the 2026-09-13 lock-race fix), and it must then gossip to the other node.
Between "healthy" and "both markers visible" every dispatch is a 412. The gate now polls
`/gateway/kv/keys?prefix=sys/caller-context/` on the dispatching node until it sees both, and reports
the count it did see when it gives up.

**Third instance of one shape in one day.** Scenario 13's readiness poll trusted a gossiped role
record that survives a restart; its assertions used client deadlines shorter than the server budgets
they measured; this gated on a weaker property than the one the test depends on. All three are a
check that proves less than the thing it stands in for — the same family as the contracts axis'
receipts, in the harness rather than the product. The rule now on the testing page: *if this predicate
is true and the next line still fails, what did I fail to prove?*
