## [2026-09-25] ingest | closure plan C2: the mandate reaches the provider

Up: [dev](../dev.md) · page: [security](../security.md) · record `docs/plans/boundary-h-closure.md` C2, §8 ·
code `src/agent/gateway_caller.rs` (envelope field `m`, `frame_with_context_and_mandate`,
`gateway_rpc_call_with_mandate`, `GossipAgent::presented_mandate`), `src/agent/service_handle.rs`
(`rpc_call_with_mandate`), `src/agent/a2a.rs`, `src/agent/http.rs`.

**Why.** The gateway forwarded `{name, arguments}` and dropped `_meta`, so a provider could not check a mandate
even if it wanted to. C3's provider-side check needs the grant and the possession proof in hand.

**Design.** An optional envelope field, absent when there is no mandate (so the pre-C2 envelope is unchanged and
old providers ignore it). It is deliberately outside the gateway's signature: the grant and the proof
authenticate themselves, a strip only causes a refusal, and the provider must not take the gateway's word anyway,
because a gateway is a member and Boundary H's adversary is a colluding population of members.

**Found on the way.** `tasks/sendSubscribe` gave the preflight `Null` params, so a mandate on a stream was never
read. That is a #399 defect, fixed here, and the end-to-end test now drives the stream door too.
