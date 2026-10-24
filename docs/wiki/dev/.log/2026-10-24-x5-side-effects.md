# 2026-10-24 — X5: the external side-effect contract

Adopter-surface X5 (first PR). `mycelium-effects::external`: an `ExternalEffect` with a declared `EffectClass`
and `ProbeBudget`, a `Reconciler` over a SQLite ledger that prepares the record before the destination is
reached and refuses by name (counted) every move the class forbids; `CompensationPermit` under `envelope`; the
apply and probe behind `mycelium::sim_seam::input` — the `Input` class's first production use, a replay reaching
the destination zero times; `Execution::Unresolved`; `HttpDestination` and a lost-response mock. Pages:
[effects](../companions/effects.md), `docs/features/side-effects.md`, guide 18 § Beyond the fleet, the
inventory §2.4 row, `what-is-proven.md` (a real provider: not run), `deprecations.md` §60.
