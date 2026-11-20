## [2026-11-20] ingest | A grant's `…/issued` is read only as the answer to the request its key names

Found by reading, confirmed by a test on two nodes: the revoke route verified the issued record's signature (any key
this node has held) and its holder, never its term or scope against the path. A member copying another genuine grant
of the same issuer and holder under a request's `…/issued` made a signed revoke of that request revoke the other
grant's term, leave the path's grant live, and write `…/revoked` and the status route's `revoked` for it. Released in
2.38.0, 2.39.0 (no signature or holder check at all there) and 2.40.0.

Rule 1, enumerated with `git grep -n '"/issued"\|/issued"\|{key}/issued'` and every `SignedMandateGrant` parse: the
revoke route, the agent's wait (`state_machine.rs`) and the status route took the grant from the key without
comparing it — each now calls `gateway_authority::grant_answers_key` (scope a grant's, term the request, scope
`grant:{term}`, holder segment the key's) and reads a mismatch as absent, counted on `/stats` `grant_issued_refused`.
I4's collector already compared term and holder segment (`call_issuer::verified_grant`). The pending lists, the A2A
answered set and the decide route's `already_answered` read the key's presence only; the grants stream reads the body
for display (the issuer's name from `purpose`) and decides nothing. The doors and the use marker take the term from the
presented grant's signed body, not from a key; `protected.rs` reads grants from capability attributes.

Pages: [owner-signed-records](../owner-signed-records.md) (the `grant/…/issued` row); `docs/features/grants.md`.
