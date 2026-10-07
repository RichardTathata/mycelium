## [2026-10-07] ingest | A presence ceiling sheds the surplus, not every host (#547, #545)

- `first_stem_fleet`'s CI snapshot caught three providers against a band of two. A 50 ms probe found the cause in
  the provisioner, not the example: above `max_providers` every host drew to withdraw at its bring-up
  probability, so at `self_elect_p = 1.0` the count went 3 → 0 → 3, round after round.
- The shed now ranks by `mycelium::election::rank` (rendezvous on the band) among providers that advertise
  `prov-shed/{ns}:{name}:{hash}` — marked from a band's first round, retracted only while providing without an
  install the node could withdraw; an unmarked peer is presumed to shed for `FIXED_AFTER` (a `cap/` entry and its
  mark arrive apart). `prov.shed` is catalogue rev 2; the trace records rank, keep, fixed and presumed counts.
- Six review rounds. Worth keeping: two intermediate designs were worse than the bug — a timing stall rule that
  cascaded a stale view to zero, and a per-install mark that lagged a herd into the original oscillation. The
  settled rule: absence of a mark means "will not act" only in the one case it can be known, and in doubt a
  provider is presumed to shed (the error is a band briefly above its ceiling, never below).
- Pages: `docs/reference/unit-file.md` (`max_providers`), `docs/reference/rule-catalogue.md` (`prov.shed`).
