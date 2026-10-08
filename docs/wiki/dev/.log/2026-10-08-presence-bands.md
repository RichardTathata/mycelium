# 2026-10-08 — presence bands: overlap named at design time, the rolling-upgrade dip counted (#560)

The two shed edge cases 2.26.0 documented on `docs/operations/capability-lifecycle.md`. **Overlap:** `mycelium
wire-check` warns `presence bands overlap` for two `[[presence]]` bands over one capability, at least one capped, not
provably disjoint — identity is the runtime's (`shed_band`: ranking and max age cleared), so one band spelled two ways is
one band, and a floor at or below a ceiling over one population does not fight. **Rolling upgrade:** a stem older than
2.26.0 advertises no `prov-shed` mark and draws; a new stem cannot stop it, so `DepartureWatch` makes the dip visible —
`mycelium_artifact_presence_unranked_departures_total` and a warning naming band and providers, per observer. The review
found the first version compared only capped pairs (a floor against a ceiling is the same fight) and erased its memory
at the ceiling (a dip reaching an observer in two rounds was missed); both fixed with tests seen failing first.
