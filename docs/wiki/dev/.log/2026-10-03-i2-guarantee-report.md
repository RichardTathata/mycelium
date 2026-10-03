## [2026-10-03] ingest | I2: the guarantee registry and the startup report

**What:** `src/agent/guarantee.rs` — descriptor, registry (I1's guarantee half), the five-state resolution,
the report (`guarantee_report()`, `GET /gateway/guarantees`, a log block at `start()`), 23 core guarantees
from the plan's §8 audit table, `register_guarantee` for companions, `late_attachments` for the G13
boundary. `ConfinementReport` became a view (resolving each of its six settings ignoring the node's role).

**Durable knowledge:**
- **The role predicate is what makes "not met" honest.** Without it a node with no gateway fails every
  gateway guarantee, and the only way to pass is to waive — which is the failure G10 names. `NotApplicable`
  carries the role fact so a reader can dispute it.
- **Resolve outside the registry lock.** A resolver reads config and `OnceLock`s; the registry is cloned
  under its lock and resolved after — one lock per function, lock-order row 52.
- **The confined view sharpened one reading:** `require_identity_proofs` without `[tls]` was `Set`; it is
  inert, and reads `Unset` now. The test that assumed otherwise was changed, and says why.
- **Features unify in the lib tests** (see the testing page): the `not(compliance)` arms run; the
  `not(tls)` arms do not. The report's `not_in_build` for `tls` is exercised only by the binary.
