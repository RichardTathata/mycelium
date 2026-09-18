# 2026-09-18 — item 2 PR 10a: the signed catalogue reply

**What shipped.** `CatalogReply { domain, for_partner, policy_revision, issued_at_ms, exports, signature }`
with `canonical_bytes` / `signed` / `verify` under `TAG_CATALOG`; `CatalogRefusal`;
`FederationEdge::with_signing_key` + `signs_catalogue`; `FederationClient::with_partner_key` +
`ClientError::Catalogue`. One edge unit test (bound to domain and asker; tamper, re-address, wrong key,
unsigned; tags distinct) and two plants in the transport tests.

**The one design choice.** The signature covers `for_partner`. Without it, a catalogue issued to a generously
granted partner could be replayed to a sparsely granted one as *its* catalogue, and the resolver would admit
calls the policy never granted — the refusal would come only at the provider's gateway, after the client had
already decided. Binding the asker into the signed bytes moves that refusal to discovery, where PR 3 put it.

**What the signature is not.** Not freshness: the resolver's observation window decides how long a list may be
relied on, and a signed stale list is still stale. Not a grant: a signed catalogue is what the domain *says* it
grants; the provider's gateway still authorises every call against policy at call time.

**Compatibility.** `signature` is `#[serde(default)]`; a PR 8 client parses a PR 10a reply. The unkeyed edge and
the keyless client keep PR 8's behaviour exactly.

**Remaining in row 10.** The Docker two-mesh suite (process isolation, real network severance) and SDK verbs.
