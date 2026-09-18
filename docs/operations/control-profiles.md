# Control profiles — the shadow-mode rollout runbook

*v3 item 4 · `docs/design/adaptive-stability.md` §7 (profiles — shadow before enforcement), §2 (the decisive
rule), §8 (detection, not prevention — except the ledger). The Phase E exit item "shadow-mode rollout
documented".*

A node runs its governors under **one control profile**, set at runtime and taking effect on each
governor's next pass — no restart, no config-struct change:

```rust
use mycelium::control::Profile;
agent.set_control_profile(Profile::Observe);   // default: Profile::Legacy
let p = agent.control_profile();
```

There is no gateway route for it yet; a gateway-side setter is an Ops Console item, not a promise here.

## The ladder

| Profile | What changes | What to watch |
|---|---|---|
| **`Legacy`** (default) | nothing. Today's governors, untouched: the membership governor's cooldown and hysteresis as before, the tuning gate without spacing or settling, the opacity boundary without release spacing, the provisioner's install rights admitted whatever the ledger says. The control contract is **not consulted** | the ordinary tripwires (`membership_flaps`, `opacity_oscillations`) |
| **`Observe`** | the contract is **evaluated and counted, and nothing is held**. The confidence predicate records what it *would* have held; the tuning gate and the opacity boundary count what spacing and settling *would* have held; the provisioner counts installs the ledger *would* have refused and still records the rejection in the ledger. Behaviour is `Legacy`'s | `control_would_hold_count()` · `tuning_governor().{held_by_spacing, held_by_settling}` (would-holds under this profile) · `opacity_releases_spaced()` · `Provisioner::rights_would_refuse()` · the ledger's `rejections()` |
| **`EnforceLocal`** | **Tier A**: budgets on this node's own actuators. The predicate holds speculative scale-up and routine scale-down on an uncertain view (never shedding, never rescue); the tuning gate holds a change inside its spacing or while its last change is unseen at the knob; the opacity boundary holds a release inside its spacing (never a shed). Rights-backed bounds are still *counted only* | the same counters, now meaning *held*; `settled_unknown` for knobs that never read back |
| **`EnforceAllocated`** | **Tier C**: everything in `EnforceLocal`, and ceilings backed by **allocated rights** — the provisioner refuses an install the ledger says the node holds no unit for, recording `admission.rejected` beside it | `Provisioner::rights_refusals()` · `rights/head/{holder}` in the medium (the node's signed claim) |

Each step up adds enforcement; nothing below it changes meaning. The counters keep their names across
the ladder — under `Observe` they count what would have been held, under an enforcing profile what was —
so a graph does not jump when the profile does. `GovernorSnapshot.profile` and `control_profile()` say
which reading you are looking at.

## The rollout

1. **Stay in `Legacy` until you have the counters in a dashboard.** A profile change with nobody watching the
   tripwires is a change nobody can judge.
2. **Move to `Observe` and live there.** The record's words: *this is where a deployment lives until its own
   traces show the rules are right*. Watch, for at least the longest cycle your fleet has (a membership
   cooldown, a tuner interval, the slowest install):
   - `control_would_hold_count` rising steadily → the predicate would be holding routine actions on a view
     it considers stale: either the fleet really is that partitioned, or `ConfidenceBound` is too strict for
     your health-check interval. Loosen the bound *on evidence*, not the rule.
   - `held_by_spacing` rising fast → the advisor recommends faster than the spacing admits; that is the
     spacing doing its job. `held_by_settling` rising → knobs are slow to read back; check the applier.
   - `opacity_releases_spaced` rising → a boundary that would flap; the release spacing is worth having.
   - `rights_would_refuse` rising → the node runs more installs than it holds units for. Under
     `EnforceAllocated` those installs will be refused: allocate first, or accept the refusals.
3. **Move to `EnforceLocal`.** Only the node's own actuators change. Watch for the *same* counters — they now
   mean held — and for the effects: a group that stops leaving on a partition, a knob that changes at most
   every other tick, a boundary that releases no sooner than its spacing.
4. **Move to `EnforceAllocated` only where rights have been allocated** (`RightsLedger::allocate`, the
   provisioner's `with_install_rights`). An unallocated node under this profile is refused every install —
   visibly, which is the point, and not what you want by surprise.

## Rollback

Set a lower profile. **Rollback disables new enforcement admissions and deletes nothing**: a right already
held is honoured until it is released, expires or is revoked; the ledger's journal is never truncated by a
profile change; the counters are never reset (compare two readings). A held action becomes admitted on the
governor's next pass.

## What the profile does not touch

The membership governor's cooldown and the opacity boundary's hysteresis are *today's* breakers and stay on
under every profile — they are what `Legacy` means. The companions' admission bounds
(`docs/operations/admission-control.md`) are the companions' own, Tier B, and not gated by the node's
profile. The mandate fence (`docs/design/scoped-mandates.md`) is authority, not control, and is never a
profile question.
