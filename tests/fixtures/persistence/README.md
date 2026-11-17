# Golden on-disk persistence fixtures (V2 gate, contracts axis)

One directory per **released KV on-disk format** (every directory but the `identity-journal-*` goldens, excluded by name): `wal.bin` (u32le-length-prefixed `SyncEntry` records)
and `snapshot.bin` (`KvSnapshot { snapshot_hlc, entries }`), both in the `serde_fixint` encoding, plus
`expected.json` — the live values and tombstoned keys a replay must produce.

`golden_fixture_replays_every_released_on_disk_format` (`mycelium-core/src/persistence.rs`) replays every
directory here through the production `replay` + LWW apply path in CI.

- `fixint-v1/` — the format unchanged since persistence shipped in v1.0.0 (the bincode → `serde_fixint`
  swap was byte-identical). Regenerate only with that writer:
  `cargo test -p mycelium-core regenerate_golden_fixture_fixint_v1 -- --ignored`.

**A format change adds a directory; it never edits one.** Every released file must keep replaying.
Record: `docs/design/contracts-receipts.md` §9.

## The identity journals (identity-and-lifecycle I2, PR 2a; #805's review F6)

`identity-journal-v1/` pins one entry of every variant of the two node-local identity journals, as hex, one
`<variant> <hex>` line each: `revocations.hex` (`identity/revocations.journal`: `Counted`, `Saturated`, `Anchor`) and
`client-certs.hex` (`identity/client-certs.journal`: `Raised` — the pre-I2 entry — `Saturated`, `Published`,
`RaisedWithCert`). `the_identity_journal_entries_are_pinned` (`src/agent/revocation.rs`) and
`the_client_cert_journal_entries_are_pinned` (`src/agent/client_cert.rs`) encode each entry and compare the bytes, decode
each line back, and show that a reader with the variants before this release refuses (and so skips) the new ones while
reading the old ones — a variant reordered or inserted in the middle fails them. **Add a line for a new variant; never edit
one.**
