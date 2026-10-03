## [2026-10-03] ingest | a sink that would receive nothing is refused, not attached

**What:** `start()` refuses `with_audit_sink` on a node with no `[tls]` (records are sealed with the node
identity; without it nothing is sealed). `audit.chain` without `[tls]` remains a reported `not_configured`.

**Durable knowledge:** the I2 report fixed the *claim* (the row said `Set`); this fixes the *acceptance*.
The two are different defects with one root: a setting whose enforcing precondition is absent. The rule
from v2.18.1 generalises — an **attachment** is a setting too, and an attachment that cannot act is
refused at the lifecycle boundary, by name, not warned about.
