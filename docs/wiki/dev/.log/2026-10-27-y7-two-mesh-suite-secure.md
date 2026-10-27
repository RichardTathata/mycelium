## [2026-10-27] ingest | Y7: the two-mesh Docker suite under `secure-multi-domain`

Closes the one promise row Y7 stayed open on (`docs/plans/adopter-surface.md` §8), the first PR of the Y8 track.

- **What landed:** every mesh node of `make test-federation` starts under `secure-multi-domain` rev 1 — certificates
  issued off-node by a one-shot `pki` role per domain (the CA key in a volume no node mounts), HTTPS gateways closed by a
  token and pinned by the consumer, the authority, audit, persistence and electorate settings; the image built with
  `compliance`. New legs: each node's `GET /gateway/guarantees` (0), an exclusive outcome refused outside an electorate
  (2), a node keeping its CA key refused at start naming `id.ca_key_off_node` (10). Page updated: `dev/security.md`
  (the PR 10b paragraph).
- **Learned:** both doors name a skill at its provider (`skill:{ns}/{name}@{provider}`), so a gateway's evaluator rule
  names the member it routes to, never itself.
- **#691's review:** one certificate volume per node (a node could read every peer's key), the admin surface off the
  gateway listener (the partner could reach it), egress naming the partner's endpoint hosts, denial legs for the
  evaluator and the body binding, by-name refusals in the binary.
- **Carried to Y8, stated:** the consumer is a bare client, so the client-side `fed.*` facts are the in-process tests';
  no electorate is declared; `fed.partner_slots` is not exercised.
