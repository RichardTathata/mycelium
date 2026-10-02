## [2026-10-02] ingest | plan: guarantees and the rule catalogue

**What:** a proposed plan, `docs/plans/guarantees-and-rule-catalogue.md`, combining the 360 review's top
recommendation (a supported secure profile validated at start) with an external design note on an
implementation-linked rule catalogue and bounded decision tracing.

**Durable knowledge:** the two share one inventory — a guarantee's enforcement points are the catalogue's
*authority* rules — but have opposite behaviour contracts: the profile refuses to start, the trace must change
nothing. `src/agent/confinement.rs` (`ConfinementReport`: `Set` / `Unset` / `NotInBuild`, and `Unverified` for
what a node cannot see) is the prototype the profile generalises. Order: inventory → startup report → profile
(one MINOR); trace core → provisioning pilot → replay in parallel once the schema is fixed.
