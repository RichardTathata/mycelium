# Scale tests, the Docker-bridge ceiling, and the SWIM saga

↑ [testing](testing.md)

## The iptables FORWARD-chain ceiling (environmental, not a Mycelium bug)

At 100 nodes, peer exchange creates ~5 000 TCP connections in a Docker bridge network; the
Linux bridge iptables FORWARD chain grows O(N²) and new runner→node connections start timing
out (errno 110). Everything below follows from this.

- **`make test-scale` (100 nodes)** passes by verifying the KV write on seed *immediately*
  (before saturation) and reading mgmt via conntrack entries established during earlier
  polling. Formation-within-240s variance (8–94/100 on identical code) is the documented
  ceiling, not a regression — identical code converges at 20/30/50 nodes.
- **`make test-scale-resilience` defaults to 20 workers** because its Phase-3 late-joiner
  probe needs a *fresh* TCP connection mid-test; at 50 workers the chain is already
  saturated. (With SWIM on, 50 workers passed 11/11 — see below.) If Phase 3 fails, suspect
  the chain first; mitigations: macvlan, nftables, keep workers ≤ 20.
- **`make test-scale-entries` (30 nodes, 5 000 × `ENTRY_BYTES` keys)** covers the
  entry-volume axis: live-gossip fraction, anti-entropy sweep tail, stability, sampled
  payload integrity, backpressure (`dropped_frames`; raise `GOSSIP_WRITER_CHANNEL_DEPTH`).
  It stays at 30 nodes because its polling keeps opening connections all test long.
- **Consecutive-run VM fatigue:** repeated 100-node rounds degrade formation monotonically
  in one Docker Desktop session (conntrack/iptables state accumulates in the VM across
  recreated networks). Before calling a formation timeout a regression, `docker desktop
  restart` and re-run once.

The v1 mitigation is `GOSSIP_MAX_ACTIVE_CONNECTIONS` (O(N×K)); the v2 structural fix was
SWIM (below). Anti-entropy has been `O(divergence)` since wire v12 (Merkle buckets) and
frame-chunked since 2026-07-02 ([runtime-invariants](../architecture/runtime-invariants.md)).

**Beyond the ceiling: go multi-host.** Every mitigation above is a *single-host* tweak — the
ceiling itself is that all N containers share one host's bridge/conntrack/iptables. The
structural escape is to spread nodes across **multiple hosts**, where each host carries only
its share of the connections and its own iptables state, so the O(N²) chain never forms on
any one host. The [`deploy/kubernetes/`](../../../../deploy/kubernetes/) reference cluster is
the path: `kubectl scale statefulset mycelium-worker --replicas=N` across a multi-node cluster
(confirm the spread with `kubectl get pods -o wide`). The in-repo Docker harness stays the
single-host *baseline* (fast, CI-adjacent, no cluster needed); multi-host k8s is the
next-level axis for node counts past ~100. Note the k8s manifests are validated offline only
(rendered, not applied in CI) — see their README.

**CI status (2026-07-10):** the scale suites run **nightly on a self-hosted runner**
(`.github/workflows/scale-nightly.yml`, 06:00 UTC, runner label `mycelium-scale` — queued
until the runner is registered). They stay off hosted runners and off the PR path on purpose:
a 2-core hosted runner hits the iptables ceiling above ~50 nodes, and each suite is
dozens-to-100 containers. The small correctness suites are the PR-path gate —
[cluster-suites](cluster-suites.md).

**The local nightly's macOS Local-Network trap (root-caused 2026-08-16).** The launchd runner
(`scripts/launchd/`, 02:00 local, Colima/vz) went dark for a month — every row from 2026-07-15
on was exit-2 at Docker image build ("load metadata … DeadlineExceeded" / registry unreachable).
Root cause: the script's deliberate `colima stop -f && colima start` VM-restart raises the macOS
**Local Network** privacy prompt, and TCC attributes it *per context* — interactive shells ride
the terminal app's existing grant, but the **launchd session prompts on its own identity**, and
an unanswered 02:00 prompt leaves that VM session unable to reach even its NAT gateway for DNS.
Diagnosis rule: reproduce with `launchctl kickstart gui/$UID/com.mycelium.scale-nightly`, **not**
an interactive `colima restart` — the interactive form silently succeeds and proves nothing.
Fix: one supervised kickstart, click **Allow** — the grant is recorded per-binary and persists
across VM restarts (verified same-day: entries PASS + resilience PASS post-grant, through the
real launchd path). Standing caution: a `brew upgrade` of colima/lima is a new binary → the
prompt returns once — after upgrading, run one supervised kickstart. (The runner executes the
**boot-volume clone** `~/Mycelium` because TCC blocks background agents from `/Volumes/Scratch`;
it **auto-fast-forwards at each run** — "runner checkout updated to …" in `launchd.out.log` — so
between nightlies it lags main by at most a day, not indefinitely; an earlier version of this
note claimed otherwise.)

**2026-08-17 — the trap was real but not the whole outage.** The first unattended 02:00 run
after the grant failed again (all three suites, same ~60 s registry-metadata `DeadlineExceeded`),
*including* a round on a VM session that had working network the previous afternoon — so TCC
cannot explain it, the machine never sleeps (`sleep 0`), and the Wi-Fi link stayed up all night.
Suspects were an upstream/WAN window vs. a recurring prompt; a host-side probe (no VM/TCC in the
path) discriminated them overnight.

**2026-08-18 — CLOSED: the grant is session-scoped, so the schedule moved to a user-present
hour.** The probe's verdict: host gateway/DNS/registry all **green at 01:58, 02:02, and 04:30**
while the 02:00 suites died between the first two — WAN refuted. Simultaneously the operator
found a **fresh permission prompt on screen again** — the Local Network grant for bare
(non-bundled) CLI tools under launchd **does not survive to the next day**; it held for the rest
of the grant-day (every supervised run green) and was gone by the next night. Unattended runs at
a sleeping hour therefore re-prompt and die *by construction*. Fix: `StartCalendarInterval`
moved **02:00 → 08:00** (all plist copies + the installed agent; 13:00 briefly, then 08:00 at
the operator's preference) — the operator is present, a
prompt (if any) gets one click, and that day's grant covers the whole run. The 01:58/02:02/04:30
`com.mycelium.netprobe` agent stays until the first green 08:00 run confirms, then can be
`launchctl bootout`-ed. Lesson for any macOS launchd job whose tooling starts a VM: **schedule it
when a human is at the screen, or the TCC prompt will silently kill it** — do not burn time on
"the network must be down."

**2026-09-03 — the 08:00 schedule's first stretch, read correctly (two more traps).** Results:
**09-01 fully green** (scale · resilience · entries — the first clean full nightly on record),
09-02 all exit-2, 09-03 all exit-2. Three things the logs settle:

- **The green run confirms the mechanism, in timing:** it started 08:00:18 and the *scale* suite
  passed at **11:11** — a three-hour "run" that was really the Docker step blocked on the Allow
  prompt until the operator saw it; once clicked, all three suites finished in **six minutes**.
  So a present operator makes the grant *grantable*; it still has to be seen. 09-02 (one-minute
  registry `DeadlineExceeded` on all three) is that prompt not being seen in time.
- **09-03 is a different signature — the documented ceiling, not the prompt.** Docker was
  unreachable at 08:00 (the runner force-recovered Colima), the image build then succeeded
  (~20 min, so the registry path was fine), all 100 containers started, seed + mgmt went healthy,
  and formation timed out: `only ? of 100 nodes visible to mgmt` — the `?` is `curl` to mgmt
  failing, i.e. **runner→node connections timing out**, the FORWARD-chain symptom described at
  the top of this page. Per the rule above: restart and re-run once before calling it a
  regression. It was also confirmed *the same afternoon, operator present*: host→registry 401 in
  0.3 s, VM→registry 401 in 0.3 s, `docker pull` fine — the network was never the problem on 09-03.
- **Trap 3 — the runner tested a stale checkout for 17 days.** Every run's log said
  `git pull skipped (offline / no auth)`, including the green one. It was neither: the runner
  clone (`~/Mycelium`) had an **uncommitted local edit** (the plist hour, made in place on 08-18),
  and `pull --ff-only` refuses a dirty tree — silently, under the script's `2>/dev/null`. The
  clone sat at `b082802` (2026-08-17) through 09-03. Scale-relevant code (`src/`, core,
  `tests/integration`) is unchanged across that window, so the 09-01 evidence still stands for
  current `src`; but the message lied. Fixed: the script now distinguishes *dirty checkout* /
  *updated* / *pull failed* and prints the HEAD it runs; the clone was fast-forwarded (its edit
  was already upstream as `c0577a1`) and its origin repointed from the pre-move `RichardEko`
  URL. Lesson: **a best-effort step that swallows stderr must still name its failure mode** — a
  wrong reason in a log costs more than no reason, because it gets believed.

The netprobe retirement now waits for a second clean morning *with the operator watching for
the prompt at 08:00* — and a 09-03-style formation timeout should first be re-run once.

## The WSB-M5 SWIM divergence saga — lessons that outlive the bug

Stage-4 SWIM cutover showed a long in-process/Docker divergence (in-process flat, Docker
linear seed-connection growth). Resolution, in order of lesson value:

1. **The true root cause was config, not networking:** the Docker demo built its config from
   `GossipConfig::default()` and never called `apply_env_overrides()` — so
   `GOSSIP_SWIM_FAILURE_DETECTOR` (and every `GOSSIP_*` knob) was silently ignored; SWIM was
   OFF in every Docker run. **When a Docker test and an in-process test diverge, first
   confirm the binary actually applies the config the test thinks it set.**
2. With SWIM actually on, three mechanism fixes flattened the curve over the real bridge:
   `gossip_sample` newest+random tail (membership heals under UDP loss), faster SWIM gossip
   defaults, de-pin threshold `k+k/3` with the bootstrap excluded from the reconcile *pool*.
   Docker `seed_established`: N=50→24, N=100→22 (from 121). G3: 50-worker resilience 11/11.
3. The in-process oracle (`src/swim_oracle_tests.rs`,
   `SWIM_ORACLE_N=100 cargo test --lib swim_scale_oracle -- --ignored --nocapture`) is the
   fast reproduction harness; the full history is in
   `docs/plans/v2-wsb-scale-transport.md`.

`swim_failure_detector` now defaults **true**. Rolling-upgrade caveat: don't mix SWIM-on/off
nodes — flip a cluster together.
