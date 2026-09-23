## [2026-09-23] ingest | the federation SDK verbs, and the open gateway the scope test found

Up: [dev](../dev.md) · pages touched: [security](../security.md) (federated-domains section; new
*a named token is a token model*) · record `docs/design/federated-domains.md` (row 11) · code
`src/agent/federation_http.rs`, `src/agent/http.rs`, `src/federation/client.rs`, `src/agent/mod.rs`
· SDKs `mycelium-py/src/mycelium/federation.py`, `mycelium-ts/src/federation.ts` · docs guide 17,
`docs/operations/federation.md`, `docs/operations/rbac.md`.

Two things landed together, and the second one was not planned: it fell out of writing a test for
the first.

## What "an SDK verb" had to mean here

A federated call could only be made from Rust. `FederationClient` was the only consumer, and
`examples/federation_node.rs` hand-rolled a `/connect` · `/call` · `/link` control API around one to
drive the Docker suite — which is a tell: the example had already designed the missing surface.

Three shapes were possible and only one survives this record's own rules:

| Shape | Cost |
|---|---|
| The SDK speaks the edge protocol | Credential minting, catalogue verification and TLS pinning in **three languages**, diverging on three schedules — and the domain's signing key in an SDK process. |
| Read-only verbs | Cheap, and leaves the gap: an SDK application still cannot call a partner. |
| **The node is the consumer; the SDK drives the node** | One implementation of the trust decisions, in the crate that already holds the key. The SDK holds a bearer, which it already had. |

So: `with_federation_clients` on the node, five routes behind two scopes, `agent.federation()` in
both SDKs. `federation:read` and `federation:invoke` are split because the powers differ — reading
which partners exist is operator information; `connect` and `call` **spend this domain's
credential** on a partner's gateway.

## The rule the shape forced

**The credential names the local caller, and the request body cannot say otherwise.** This is item
7's fix at one more boundary, and it is the third time the same deputy has had to be un-confused:
inside a domain (item 7), across a domain (PR 8's `FederatedCaller`), and now for a client that has
neither the key nor the trust. A gateway minting under its own configured principal would put a
service account in the partner's evidence for work it never asked for; reading the principal from
the body would let anyone holding `federation:invoke` have this domain vouch for an unauthenticated
identity. `FederationClient::call_as` is the whole mechanism and `http.rs` its only caller here.

On an open gateway the principal is `anonymous`. That is honest — *this domain vouches that the
caller was anonymous here* — and it is why the runbook says to configure tokens before connecting
two domains, rather than why the route should refuse.

## The refusal vocabulary is the receipt vocabulary

Every refusal body carries `sent` (did any byte cross) and `delivery` (`none` · `refused` ·
`completed` · `unknown`), and both SDKs raise a **distinct type** for `unknown`. An SDK cannot see
`ClientError`, so without this the hot invariant — *a timeout is `DeliveryUnknown`, never a
negative* — stops at the FFI boundary and every caller downstream is free to retry an effect that
may already have run. An unreadable refusal body (HTML from a proxy) fails closed to `unknown`.

Two small decisions worth keeping. The in-crate match on `ClientError` has **no `_` arm**:
`#[non_exhaustive]` binds other crates, not this one, so a new refusal stops the build at the place
that has to decide what it means for `sent` and `delivery` — the fail-closed rule lives in the
*values*, not in a default arm. And the outbound call is its own AE enforcement point
(`gateway:federation/call`), separate from `gateway:a2a`, because the two directions are different
and an evaluator guarding `/mcp` and `/a2a` but not this route is a remit with a third door open.

## The defect the test found

The scope test asserted 403 for a `federation:read` token on `/call` and got **504** — the call had
gone out. The scope layer had not run at all.

`gateway_auth`'s open-gateway predicate counted `gateway_auth_token` and `gateway_scoped_tokens`,
and not `gateway_named_tokens` — added in 2.10.0, honoured by `resolve_token` ever since. So a node
whose only credential model was named tokens required **no bearer at all**, and
`open_gateway_scopes` handed each request exactly the scope its route asked for. Affected
2.10.0–2.12.0. `GossipConfig`'s docs say to *prefer* named tokens, so the recommended configuration
was the affected one.

**Why it survived the Phase-C audit and the fuzz campaign: the tokens worked.** Presenting one was
admitted, so every positive assertion passed. The only way to see it was to present nothing — and
the one existing test that configures named tokens sets the positional table beside them, so it
could not. Same shape as 2.11.1's fuzz seeds that never reached their invariant: *a gate that looks
covered because its positive case passes.*

Two lessons, both general:

- **An auth test that never sends an unauthenticated request proves nothing about the gate.** It
  proves the credential is accepted, which is the half that was never in doubt.
- **A predicate that enumerates credential sources is a completeness claim**, of exactly the kind
  the lock-order table makes: adding a source means adding a term. It had no test saying so, and
  now it has `named_tokens_alone_still_close_the_gateway` — which fails when the fix is reverted.
