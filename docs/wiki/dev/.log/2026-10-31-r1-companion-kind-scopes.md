# 2026-10-31 — R1: a companion's kind at a gateway door needs the companion's family scope

External review 2026-10-31, finding F1; `docs/plans/external-review-2026-10-31.md` row R1, decision D1; PR #714.

**Durable knowledge.**

- **A route's scope is not the operation's scope.** The raw routes (`rpc/call`, `scatter`, `signal/emit`,
  `mailbox/deliver`, `shard/emit`, `overlay/emit_reliable`) carried a companion's RPC kinds to the companions' own
  handlers under `mesh:write` (or `consensus:write`) alone, so withholding `board:write` from a `mesh:write` holder
  kept it off `/gateway/bb/post` and not off the board. The 2.15.0 protected-kind list closed this for `mcp`,
  `skill` and `llm` kinds by name; companion kinds were never on it, and the scope consequence was unstated.
- **A handler cannot make this decision.** `ResolvedPrincipal.scopes` is the credential's set intersected with the
  route's requirement, so a handler sees `mesh:write` and nothing else; the gate lives in the auth middleware,
  which has the whole set, and reads the kind from the body field the handler reads (`method` on the call routes,
  `kind` on the emit routes) — never whichever the body carries.
- **The read-side twin and the serve side.** The signal streams (`/gateway/signal/sse/{kind}`, `/signals/{kind}`)
  tap the same handler table `rpc_rx` registers on, so observing a companion's kind is reading its contents
  (`:read`, whatever the verb); the serve stream answers the kind in the provider's place, so serving a companion's
  kind is writing (`:write` plus `mesh:serve`). Found by #714's review; the row's enumeration had stopped at "takes
  the kind from the request".
- **`cap:write` is not gated by this.** A `cap:write` client can advertise a companion's `{ns}.primary` role — that
  is the scope's meaning — and is then *sent* requests only through a serve stream the family scope gates.
- **Where the pin lives, and why.** CI runs `tests/` under `tls,a2a`, never `compliance`, so a scope test in
  `tests/exposure_doors.rs` would never execute. The pins are `src/agent/http/http_tests.rs` (every door, every
  family, both verbs) and the companions' `tests/raw_route_scope.rs` (end to end, the fact on the board), whose
  test builds now carry `compliance`.
- **Each companion diffs its own verbs against the core table** by reading its source for `rpc_kind(…, "op")`
  literals and `handler!("op", …)` registrations; the read/write split is the test's own statement.
