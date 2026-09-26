## [2026-09-26] ingest | closure plan C5: removing a member

Up: [dev](../dev.md) · page: [security](../security.md) · ADR `docs/design/member-removal.md` · record
`docs/plans/boundary-h-closure.md` C5 · test `src/lib_tests.rs` → `member_removal`.

Until now an operator could not cut a member off. Key revocation is self-revocation, so a colluding node never signs
its own, and at the transport any certificate from the fleet CA was a member. A removal is now an operator act: a
`SignedMemberRemoval`, signed by a configured membership authority (P1's external-issuer path) and naming the node
and every key it is known to hold.

A node that accepts one:
- drops the removed node's pings, signals and gossip writes at the connection loop;
- refuses its certificate at the TLS handshake, in both directions, so reconnecting does not bring it back;
- refuses its RPCs before any serve path (`CallerError::Removed`);
- counts its keys as revoked, so a mandate it holds no longer proves possession at A1.

The removal spreads by gossip under `sys/membership/removed/`, and every node verifies it before applying it; an
operator can also offer it to each node directly. It is monotonic: a removed identity comes back only as a new one.

Stale membership is split by layer, as the ADR's review adopted. Presence fails open, so an authority outage does
not partition the fleet; protected work already fails closed through A1 and C3.

Removal only means something if the removed node cannot mint itself a new identity. The confinement report
therefore gains `ca_key_off_node`: a node that holds the fleet CA's private key reports it unmet.
