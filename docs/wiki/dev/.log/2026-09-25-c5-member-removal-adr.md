## [2026-09-25] ingest | closure plan C5: the member-removal ADR, proposed

Up: [dev](../dev.md) · page: [security](../security.md) · record `docs/design/member-removal.md` (proposed).

C5 is the one closure item with a real design question, so it gets an ADR and review before code. Two things decide
it. First, what "unknown" means for membership: failing closed, as A1 does for protected work, would let one
authority outage partition the fleet, so the ADR recommends splitting by layer (presence fails open, protected work
already fails closed). Second, removal is only real if a removed member cannot mint a new identity, which rules out
the development default of every node holding the CA key; the strict profile would require it off-node, and the
confinement report would say whether it is.
