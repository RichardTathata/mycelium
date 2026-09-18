# 2026-09-18 — item 2 PR 8: the federation transport, first arm

**What shipped.** `src/federation/edge.rs` (provider side: `PresentedCall`, `FederationEdge`, the catalogue
reply), `src/federation/client.rs` (consumer side: `FederationClient`), `src/agent/federation_http.rs` (the
auth-layer step, the catalogue route), `verify_federated_credential` in `call.rs`, `federation_principal`,
`GossipAgent::with_federation_edge`, the `/a2a` optional-auth layer's federation branch, the A2A handler's
authorisation step, and `lib_tests::federation_transport` (two tests). Docs: security, guide 17, the design
record §13 (rows 8–9 + where the gate stands), the plan's item 2 sequence, testing, lock-order rows 38–39,
changelog, history.

**The shape decision.** The header is read before the body, so the credential is verified twice on purpose:
identity at the auth layer (`verify_federated_credential`: signature, lifetime, expiry, skew), export-and-grant in
the handler (`verify_federated_call`) once the body has named the skill. The handler re-runs the identity checks
rather than trusting that a layer ran — they are cheap, and a handler reached by another route (a bare router in
a unit test) must not assume its caller was vetted.

**Two rules the HTTP layer had to own.** (1) *Present-and-refused is a refusal, never anonymous.* `/a2a` admits
anonymous callers (item 7's optional auth); without this rule a revoked partner's credential would fall through
to the anonymous path and the revocation would have done nothing visible. (2) *One identity per request* — a
bearer plus a credential is a 400; the gateway does not choose which caller it dispatches for.

**Plants, both directions.** Eight at the gateway, each asserted refused *and* counted as not reaching the
provider: wrong-export credential (`-32003`, "authorises"), ungranted (`-32003`, "policy does not grant"),
forged key (401), tampered field (401), malformed header (400), bearer + credential (400), streaming under a
credential (`-32003`), call credential presented for the catalogue (403) and no credential for it (401). Two on
the client, refused with no HTTP: call before `connect` (`Link(Down)`), export the catalogue never named
(`Resolve`). The positive controls: the federated call dispatches once with `federation:beta.example/svc/billing`;
the anonymous A2A path still dispatches as `anonymous`. The harness's non-vacuity test (a deliberately merged
pair fails `assert_never_merged`) still runs, over the lifted free function.

**Findings.**
- *The gateway's listener binds after `start()` returns.* The first draft probed a bare gateway immediately and
  failed on `ConnectionRefused`; the fix is a bounded readiness loop, recorded in testing.md so the next gateway
  test does not repeat it. The A2A caller test never hit this because its capability-key poll took long enough.
- *Reordering refusals would have changed a pin.* `verify_federated_call` checks the export binding before the
  lifetime; the identity-only verifier could not simply be the prefix of it. It is a separate function with the
  same refusals in the same relative order, minus the two it does not make.
- *The seams check caught three new sites* (`SystemTime::now` in the edge, `Instant::now` + the `Instant` import in
  the client). The edge now reads `sim_seam::wall_now_ms`. The client must *store* an `Instant`
  (`CatalogObservation::observed_at`, a public PR 3 field whose age the resolver already reads through
  `mono_elapsed`), and the seam had consumers of an `Instant` but no constructor — so `sim_seam::mono_instant()`
  was added: identical in both arms, because per the seam's own note the instant is never reproduced, only the
  interval read from it. The construction site is now the seam's; the invariant a reviewer checks is at the
  read sites.
- *The harness's assertion was a method on owned agents.* The provider task needs `request_principal` on the
  agent, so the gateway node has to be shared; lifting `assert_never_merged` to `fn(&[&GossipAgent],
  &[&GossipAgent])` let the PR 1 checks run unchanged over an `Arc`-held node.

**Not claimed.** The release gate. Its traces leg, its partition choreography (sever every link, keep working
locally, change permissions mid-partition, reconnect), a signed catalogue reply, TLS in the test, SDK verbs —
PR 9. The example still runs in one process and says so.
