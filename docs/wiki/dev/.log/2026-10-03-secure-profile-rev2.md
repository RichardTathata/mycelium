## [2026-10-03] ingest | secure-single-domain rev 2

**What:** `SECURE_SINGLE_DOMAIN` revision 2 requires `id.ca_key_off_node` and `persist.unreadable_refused`
(17 ids, pinned). The acceptance test's node now gets a certificate issued off-node by a throwaway issuer
node (`issue_node_cert`), copies only `ca-cert.pem`, and asserts no CA key was minted on the node.

**Durable knowledge:** G12 worked as written — the requirement shipped one release after the mechanism that
makes it meetable (v2.20.0: the pre-issued certificate path) with the announcement in that release's notes.
A profile revision is a behaviour change for every deployment under it; the number is the contract.
