## [2026-09-13] ingest | AE-T — the thin authorisation/evidence slice, and the wedge

Up: [dev](../dev.md). Plan [rev 1.10 §6.8](../../../plans/v3-contracts-axis.md), D37–D38.

**Finding.** Under rev 1.9 the NovusLens loop (declared remit → exported policy → enforced action →
signed evidence → assessed page) sat at Phases C–E behind items 1, 5 and 7 — the axis's decisive
commercial demonstration was its furthest-from-runnable item. Read item by item, five of the eight
v3 items are correctness/security hardening of the shipped AGPL substrate; the commercial value is in
the composition (AE, RA, possibly federation) and in delivery, not in the substrate contracts.

**Decision.** AE-T: a strict subset of AE1–AE4 at the **gateway** as the sole, declared enforcement
point, as a Phase B exit. Anchors verified: `tools/call` dispatch `src/agent/http.rs:1307` →
`rpc_call_ctx` (`rpc.rs:131`) behind `gateway_auth` (`http.rs:431`) and `mcp:invoke`; `/a2a` at
`a2a.rs:462`; the audit chain's `AuditSink::export` (`audit.rs:260`) is the evidence source. Steps
T1–T4 + T-gate (scenario 2 locally against the stub consumer). D37 (provisional): Cedar in-process
via `cedar-policy`, no sidecar. D38: guarantee stated as a route-level preflight with
`coverage.complete: false`; AE1–AE4 and the four-run joint pack unchanged.

**The wedge is the gateway, not the fleet.** Enterprises won't rewrite agents onto Mycelium; they
will put a gateway in front of the MCP tools and A2A endpoints their agents already call, and Mycelium
already fronts both. "Enforce your remits where the agents actually act" is the entry sentence; the
fleet story comes after. Recorded in plan §1.5 and ROADMAP.

**Queue re-sequenced (§10 item 12):** item 7 first; item 1 PR 1 + AE0 ADR in parallel; AE-T T2–T4
directly after item 7; cooldown parameter and item 8 alongside.
