## [2026-11-12] ingest | the third-party critique verified, and the plan that answers it proposed

A third-party critique of the whole project arrived on 2026-11-12 (a summary and the reviewer's reply; the full
per-layer document has not been received). It is vendored unchanged at
`docs/plans/external/2026-11-12-360-critique.md`, and every finding was read against `main` at `156ca329` — nothing
built or run — for [`docs/plans/external-critique-2026-11-12.md`](../../../plans/external-critique-2026-11-12.md)
(proposed, awaiting the maintainer's decisions D1–D12).

- **The root cause is the owner-signed rule** ([`owner-signed-records.md`](../owner-signed-records.md), #772). The
  plan groups that page's *Does not hold* and *Partly* rows into rows CR2–CR5 and CR7; the page now points at it.
- **Read as true, and not stated where an adopter looks:** an inbound entry's stamp is never refused — the drift
  bound clamps the receiver's clock only, at the anti-entropy arm *and* at both live KV frames
  (`mycelium-core/src/connection.rs`, its three KV `hlc.observe` sites), so a far-future stamp pins a key; the
  tombstone horizon is 50 minutes at the defaults (`src/agent/tasks.rs`, the sweep's cutoff); none of the four
  append logs has a checksum; a signal's nonce is burned before admission, in the set KV frames share, and nothing
  repairs a suppressed signal; an RPC reply is matched on a nonce and an emitter-written sender.
- **Already stated limits the critique reports as findings:** the default quorum from the live-peer view
  (`docs/threat-model.md` §7), the fence at the resource, the CA key on every node in the development posture,
  SWIM (stated for the federation profile only), the nonce consumed by a refused KV frame.
- **Not confirmed:** self-install as a default (a stem with `[hosts]` and a trusted publisher only); `TaskCtx` as
  the cause of every `too_many_arguments` allow (eleven are in `mycelium-core`).
- **For whoever builds the rows:** a WAL checksum is an on-disk format change against
  `tests/fixtures/persistence/fixint-v1/`, which has no magic or version byte — a new directory, never an edit;
  the catalogue ranks candidates on two fields outside the publisher's signature; the log scan and subscribe doors
  consult no owned-stream guard.
- **Touched:** `docs/operations/what-is-proven.md` (four *Not yet shown* rows: the WAL checksum, the lease's skew
  bound, the missing model, catalogue currency), `docs/plans/README.md`, `CLAUDE.md` (one line under *Open
  elsewhere*), `owner-signed-records.md` (the pointer).
