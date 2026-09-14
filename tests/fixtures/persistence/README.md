# Golden on-disk persistence fixtures (V2 gate, contracts axis)

One directory per **released on-disk format**: `wal.bin` (u32le-length-prefixed `SyncEntry` records)
and `snapshot.bin` (`KvSnapshot { snapshot_hlc, entries }`), both in the `serde_fixint` encoding, plus
`expected.json` — the live values and tombstoned keys a replay must produce.

`golden_fixture_replays_every_released_on_disk_format` (`mycelium-core/src/persistence.rs`) replays every
directory here through the production `replay` + LWW apply path in CI.

- `fixint-v1/` — the format unchanged since persistence shipped in v1.0.0 (the bincode → `serde_fixint`
  swap was byte-identical). Regenerate only with that writer:
  `cargo test -p mycelium-core regenerate_golden_fixture_fixint_v1 -- --ignored`.

**A format change adds a directory; it never edits one.** Every released file must keep replaying.
Record: `docs/design/contracts-receipts.md` §9.
