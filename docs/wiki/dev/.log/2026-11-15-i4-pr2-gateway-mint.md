## [2026-11-15] ingest | I4, PR 2: the gateway's mint, its records, the revoke, the re-assertion, the collector

`docs/plans/identity-and-lifecycle.md` row I4 records PR 2 (*Delivered by PR 2*) and the maintainer's decision of
2026-11-15 on its first entry condition.

- `gateway_mint_call_mandates` (off by default): the one gateway dispatch path (`gateway_rpc_call_with_mandate`)
  frames through `gateway_caller::frame_minting`, which draws the mint once the envelope's id is known
  (`call_issuer::mint`, term `c{n}`); `call_issuer::record` writes the first checkpoint and the signed `window`
  record on the WAL's required-sync write before the envelope is sent, or the call is refused
  `call_mandate_not_durable` (-32025). The mint derives the call as the provider does
  (`provider_enforcement::derive_call`), so the possession proof and the provider's `derive` cannot disagree.
- The revoke (`call_issuer::revoke_call`) is built and called by no door (PR 3). It holds the new checkpoint as
  issued **before** writing it, so the re-assertion never writes the first back over the revoking one.
- The re-assertion (H1): a bounded queue on the KV's prefix watch over `call/{self}/…/checkpoint`; anything but the
  gateway's newest is written over, **past the replaced value's stamp** (`hlc.observe` first), or a far-ahead replay
  would win last-writer-wins against it. In memory only: nothing after a gateway restart.
- The collector (D-5), each minute at every node with an authority or a mint: `call/` past window + 2·skew +
  `max_drain_secs` (a forged or missing window ⇒ the cap-based floor; nothing verifies ⇒ never); `grant/` under a
  verified `issued` record past the later of window + 2·skew and issue + `grant_retention_secs`, CR2's markers and
  releases with it. Issuer `kv_delete`; others `kv_delete_held`. Not collected: unanswered/refused requests; CR2's
  `uses.journal` (not compacted, stated).
- Doors (rule 1, the maintainer's decision): `provider_enforcement::Door` at each `check`; a cancellable call at a
  door that cannot stop it is refused `door_cannot_stop` — bridged MCP, SDK serve, a hosted kind via `rpc_rx`, and
  the new `register_mcp_tool_unstoppable` (the wasm host's bridge). **`rpc_rx`, decided the same day in
  promise-theory terms (an agent promises only its own behaviour):** a receiver declares it stops
  (`RpcRequestRx::stops_when_authority_lapses`); an undeclared one is refused a cancellable call (`data.door`
  `rpc_rx`); a declared one's answer more than `LAPSE_ANSWER_GRACE_MS` (1 s) after the lapse is counted
  (`rpc_rx_answered_after_lapse`), never refused — assessment, not enforcement. Only two test loops declare;
  no shipped receiver races `authority_lapsed()` yet.
- `mycelium-reason`: `cancelled` (and `call_mandate_not_established`/`revoked`) is terminal (`RouteError::Cancelled`).
- Pages touched: `owner-signed-records.md` (the `window` row now on `main`; the checkpoint row narrowed).
