# Coordinator-comparison data

The per-decision CSVs (`gossip_n{10,20,40}.csv`, `broker_n{10,20,40}.csv`), their `summary.csv`, and the
figure `fig_decision_latency.{tex,svg}` that Paper 2a §8 and Table 2 are generated from.

**Sweep of 2026-10-03** (`RUN_ENV.txt` names the machine): `./examples/coordinator_comparison_runner.sh`
with the defaults — N ∈ {10, 20, 40}, one 20-second run per (mode, N) at 50 decisions/s, 6 s warm-up —
then `python3 examples/coordinator_comparison_plot.py` to regenerate the figure (its caption is computed
from `summary.csv`). The table in `main.tex` is transcribed from `summary.csv` by hand; a diff between the
two is a finding.

**History.** The table published in the first version of Paper 2a rested on a sweep whose CSVs were never
committed (15.7–27.9 µs gossip means, broker means to 10.9 ms, a timeout-dominated p99); the data
committed beside it from 2026-06-11 was a different run (46.9–39.8 µs; no broker N=40 row). Publication-
lint run 5 (2026-10-03, `docs/publications/overclaim-ledger.md`) found the mismatch; this sweep replaces
both, and every number in §8 now reads from this directory. No decision in it reached the 500 ms RPC
timeout. A header-only `gossip_n80.csv` from a sweep that never ran was removed.

`three_arm/` is the separate work-distribution pilot, documented in its own README.
