# Research guide

Mycelium is both an implementation and a vehicle for testing hypotheses about decentralised
coordination. Read the [paper sequence](README.md) for the arguments, the
[capability map](../capabilities.md) for current mechanisms, and [what is proven](../operations/what-is-proven.md)
for the distinction between CI coverage, bounded demonstrations and outstanding acceptance.

## Questions, evidence and assumptions

| Question | Research resource | Implementation / evidence | Interpretation limit |
|---|---|---|---|
| How does local pull compare with mediated assignment under heterogeneous, changing capacity? | [HLKS sources](paper2a/), [experiment design](../plans/three_arm_workdist.md) | [Three-arm runner](../../examples/three_arm_runner.sh), [recorded dataset](paper2a/data/three_arm/README.md) | The recorded 108 runs used 20 workers on one Apple Silicon host, 2026-06-12. Exclusions and reruns are documented. This is not a multi-host production result. |
| What structural role does a coordinator play? | [Coordinator Trap](paper1/), [related-work snapshot](paper1/related-work.md) | [Comparison runner](../../examples/coordinator_comparison_runner.sh), [examples](../../examples/README.md) | A selected baseline and workload cannot establish superiority over all architectures. The related-work snapshot is dated and needs refreshing for new comparative claims. |
| Does coordinator freedom prevent capture? | [Capture Problem](paper2b/), [Monetary Ecology](monetary-ecology/) | [Authority](../guide/20-authorising-actions.md), [mandates](../guide/21-mandates.md), [control](../guide/22-stability-and-control.md) | These mechanisms address specific boundaries; they do not establish a general social or technical impossibility of capture. |
| Can adaptive behaviour remain inspectable and reproducible? | [Replay guide](../guide/19-replay-and-simulation.md), [rule-catalogue plan](../plans/guarantees-and-rule-catalogue.md) | [Simulation crate](../../mycelium-sim/), [knowledge example](../../examples/knowledge_layer.rs), [stem/co-op examples](../../examples/coop/README.md) | Captured choices, provenance and configured rules support investigation; they do not prove arbitrary external effects or the quality of learned behaviour. |

## Reproduce a result

1. Choose whether you are reproducing a published dataset or testing current code. For a paper,
   record its DOI/version and recover the exact source revision and environment used; the dataset
   notes do not themselves pin an immutable code revision. Do not substitute the current release
   and label the output an exact reproduction.
2. For a **new run**, use a separate checkout of a selected tag (see [installation](../guide/installation.md)).
   Record `git rev-parse HEAD`, `rustc --version`, platform, features and runner parameters.
3. Start with a reduced three-arm sweep from the repository root:

```sh
OUT_DIR=work/three-arm-retest HETS="0 0.5" DRIFTS="0 0.2" SEEDS="1" N=8 DURATION_SECS=15 \
  bash examples/three_arm_runner.sh
```

This command writes fresh results outside the published dataset. It needs the Rust toolchain and
local TCP ports. The [runner](../../examples/three_arm_runner.sh) lists full-sweep defaults;
[plotting code](../../examples/three_arm_plot.py) defines the analysis. A reduced sweep is a smoke
run, not a replacement for the published experiment. Keep failures, raw outputs, exclusions and
rerun criteria with the result. Compare distributions and uncertainty, not just a favourable mean.

## Open questions

- Repeat coordination comparisons across hosts, partitions and realistic hidden work; distinguish
  model assumptions from behaviour observed in a particular runner.
- Test composition: does demand-driven installation preserve workload objectives while authority,
  capacity and network views change together? Stem demos establish scenarios, not that general result.
- Evaluate rule interactions and trace completeness using the [guarantees and rule-catalogue plan](../plans/guarantees-and-rule-catalogue.md).
  The registry, startup report and profiles (v2.19.0) and the rule catalogue with its decision trace (v2.20.0) are delivered; a decision-level replay of a whole agent is not yet shown (`what-is-proven.md`). Test the profiles against their named checks rather than treating them as universal guarantees.
- Compare discovery, propagation, admission, response and authority separately in prior-art work.
  Similar terminology is not architectural equivalence; claims of uniqueness require a source-based comparison.

For a deployment study, use the [customer pilot protocol](../operations/customer-pilot.md) and agree
acceptance criteria before observing results. For commercial context see the [buyer deck](customer-pitch.html);
for architecture context see the [engineering deck](presentation.html).
