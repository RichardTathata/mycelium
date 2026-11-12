## [2026-11-12] ingest | I2 written buildable: R1–R4 resolved (identity-and-lifecycle rev 0.18)

`docs/plans/identity-and-lifecycle.md` row I2 (CA succession by a pre-committed successor) is buildable. Its
round-3 open questions are decided by the maintainer (2026-11-12):

- **R1: after narrowing, no node is legacy.** `KeyStanding` gains `legacy_closed`, set from the persisted phase.
  Without it, a node whose every old-CA record lost standing would be trusted from `peer_keys` again.
- **R2: the rank is carried.** Ordering serials alone leaves a gap: an old key's successor record seen beside the
  current key's old record outranks it. So the original first issuance travels in a successor-signed `firstIssuedMs`
  extension, read into `CertIdentity.first_issued_ms`.
- **R3: a gateway inventory.** `GET /gateway/identity/inventory` (`identity:read`) returns raw records, and the issuer
  re-judges them offline with the fold's own rule. Revocations survive succession.
- **R4: anchors keep their certificates.** A digest cannot be re-judged against new roots. The certificates are
  journaled, and the caches are tagged with a roots generation, because a clear races a fold across `activate`.
- **I1:** client certificates lose standing at narrowing; `tls succeed` re-issues each principal's current key.
- **I7:** consent certificates follow the same rule. `tls succeed` re-issues each one over its public key alone,
  carrying the grade extension over.
- **I4 (§9 O8, closed):** every gateway must be anchored under the successor before any provider narrows. This is
  required, with an operator override (the maintainer's decision, 2026-11-12):
  - a provider waits, widened, naming each unanchored gateway;
  - `tls succeed --narrow --accept-unanchored <node>` publishes an audited, successor-signed acceptance;
  - an accepted gateway's cancels fail open for at most its `call:` window.

  A succession window must be longer than `GRANT_TTL_CAP_SECS`, so every `call:` window is shorter.

I2 waits on the sim-seams gate fix: `mycelium-core/src/tls.rs`'s grouped `use std::{fs, …}` hides its filesystem calls
from `scripts/check-sim-seams.sh`.

Pages touched: none in the wiki. Plan rev 0.18 and `docs/plans/README.md`.
