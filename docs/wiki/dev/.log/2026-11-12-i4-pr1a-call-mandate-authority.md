## [2026-11-12] ingest | I4's build opened: a `call:` mandate at the execution authority (PR 1a, with #775's review)

`docs/plans/identity-and-lifecycle.md` row I4 gains its build record (rev 0.20): the split (1a, 1b, 2, 3, 4); the
maintainer's decisions on Q1 (`StopRecord` unchanged; `NamedStop`), Q2 (the cancel receipt, none on the A2A or MCP
wire) and H1 (a KV copy of a call's checkpoint that fails verification is absent, not refused, with detectors; the
cancel-forward is the primary cancel path and the KV checkpoint the fallback); the build's engineering defaults; three
differences from the agreed row put to the maintainer and left open; one question left for PR 1b (`cm` outside the
envelope's signature).

PR 1a is the authority's half, in `src/agent/gateway_authority.rs` and `src/mandate/grant.rs`:
- `ExecutionAuthority::assess_call` takes lock-order row 43 once and, under that hold, verifies the mandate
  (`check_call_mandate`: scope `call:{term}`, a term of one short key segment, issuer = holder = the envelope's
  gateway, no entitlement table), inserts the scope's window, reads this node's own KV copy of the checkpoint, takes it
  and the carried one, and judges. The read under the hold is what closes N2's race; reading before the hold is the
  spike's shape and misses a revocation written between the read and the insert.
- A `call:` scope's checkpoints are held only where a call was admitted and only until the mandate's window ends.
  `offer_checkpoint` is the one place that enforces "only for a held window" (`CheckpointOffer::NotHeld`). The windows
  are capped per gateway and in all, and reclaimed in holds of 1 024 repeated until nothing expired is left: one hold
  per sweep was a throughput ceiling (#775's H2).
- The `call:` feeder takes the written keys from a bounded queue filled where the KV's prefix watch sees each write,
  and reads a key only if this node holds its window; on overflow it re-reads its windows, 256 per hold. The watch
  itself coalesces and carries no key. Reading every held window on every fleet-wide write was linear in the windows
  held (#775's M1).
- The one KV record it reads, `call/{gateway}/{term}/checkpoint`, is verified at read whatever its sequence (body =
  key, then the gateway's signature). A failing copy is absent and counted; an older copy over a newer one held, and
  a tombstone where a copy was read, are counted too. A first read after such a replay cannot be detected at the
  reader: that residual is on the inventory row and `what-is-proven.md`.
- No entitlement table entitles a `call:` scope (`EntitlementTable::is_entitled`), so no `GrantVerifier::check`
  caller finds one valid.
- `named_stop_records` names the mandate a stopped registration ran under.

Pages: [lock-order](../concurrency/lock-order.md) rows 43, 48 and 49; [owner-signed records](../owner-signed-records.md),
the checkpoint's inventory row (**Partly**). Also `scripts/check-kv-namespaces.sh`: a prefix absent from both
front-door lists ended the script silently under `pipefail`; fixed, and planted in its self-test. Not built yet: the
provider reading `cm` (1b), the gateway's mint (2), the cancel doors (3).
