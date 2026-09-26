## [2026-09-25] ingest | cardinality one proves nothing per-partner — two partners on one edge

*Written 2026-09-23 in a worktree and never committed; recovered 2026-09-25. Row 11 had meanwhile been
closed by #362's chain test, so this lands as the star case that complements it.*

Up: [dev](../dev.md) · pages touched: [testing](../testing/testing.md) (new section),
[security](../security.md) (federated-domains) · record `docs/design/federated-domains.md` (row 11) ·
code — **none**; the change is `src/federation/edge.rs` tests and `src/lib_tests.rs`.

The finding needed **no production change**, which is the finding rather than the absence of one.

## What one partner cannot tell you

The federated-domains record states a dozen guarantees about **a partner**: this partner sees only its
grants; this partner's credential authorises this export and no other; revoking this partner stops only
this partner. Each had a test. Each test configured **one** partner — and with one partner, *the key for
the claimed origin* and *the only key in the bundle* are the same key.

So the suite could not distinguish per-partner behaviour from global behaviour. The plant that shows it:
`TrustBundle::acceptable_keys` returns every non-revoked key in the bundle rather than the claimed
domain's — a one-line "helpfulness" bug of a kind that gets written.

- All **24** pre-existing federation tests stayed green.
- `a_trusted_partner_cannot_speak_for_another_trusted_partner` failed: beta's key authenticated a
  credential naming gamma, returning `Ok(FederatedCaller { origin_domain: gamma.example, … })`.
- `three_domains_one_edge_and_the_middle_domain_is_not_a_bridge` failed on the wire: the forged call
  answered **200**. Not refused — *executed*, and recorded as gamma.

That is a complete authentication bypass between partners of one edge, and the suite as it stood would
have shipped it green. Two more plants confirmed the neighbouring legs: a catalogue built from the union
rather than the asker's grants (beta is told `demo/secret` exists — enumerating what you may not call is
already being told something), and a `revoke` that revokes everyone.

**The rule, stated generally: a claim quantified over X needs two Xs to be a test.** One partner, one
domain, one tenant, one gateway — a suite at cardinality one is green, looks complete, and cannot see the
defect where X's authority reaches Y. It is the same family as *a gate that exists is not a gate that
runs*: the test executed, it just could not fail.

## And one claim needs three

**A common neighbour must not be a bridge.** Beta and gamma never exchange a byte; both federate with
alpha. The gate asserts non-merger for all three pairs — membership tables, the native `cap/`/`grp/`/`sys/`/
`consensus/` namespaces, and each node's connection table — and the beta/gamma pair is the one that did not
exist before. Alpha talking to both must not make them neighbours of each other.

## What is deliberately not covered

Capacity per partner. When this was written the edge had no per-partner accounting; #365 has since added
`max_in_flight_per_partner`, which has its own gate, so this test does not assert it again.

**Re-verified on recovery (2026-09-25, against `main` at `ecc1514`).** With the plant in place, exactly two tests
fail: `a_trusted_partner_cannot_speak_for_another_trusted_partner` and
`three_domains_one_edge_and_the_middle_domain_is_not_a_bridge`. The other 83 federation tests, including #362's
chain test, stay green. Without the plant, all 85 pass.
