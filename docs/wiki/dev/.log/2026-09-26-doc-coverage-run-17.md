## [2026-09-26] fix | doc-coverage run 17, and the seven code gaps it surfaced

Up: [dev](../dev.md) · [operations](../operations.md) · follows
[2026-09-26-lint](2026-09-26-lint.md).

`/doc-coverage` run 17 (`docs/analysis/doc-coverage.md`, PR #428) re-audited the v3 axis and
Boundary H across the WHAT · WHY · HOW × Dev · Ops matrix. Three guide literals did not compile
(`TlsConfig { key_path, cert_path }`, `CapabilityGroupDef` without `topology_policy`, guide 04's
reference blocks calling handle methods on `agent.`), two readiness-checklist instructions failed
silently (`message/send`; a bare `identity_one_record` command), and twelve concepts gained the
HOW cells they lacked. Five calibration entries.

**The code gaps, closed the same day** (`fix/doc-coverage-code-gaps`), each with a test seen
failing first where a behaviour changed:

| Gap | Change | Test |
|---|---|---|
| the confidence bound was hardcoded in three governors | `GossipConfig.control_max_staleness_ms` / `control_min_peers_heard` (+ env), `ConfidenceBound::from_config`, `TuningGovernor::set_confidence_bound` | `the_confidence_bound_comes_from_the_operator_not_the_default`, `from_config_carries_the_operator_setting`, `a_zero_confidence_bound_is_refused` |
| `POST /gateway/kv` discarded its receipt | the route uses `kv_set_with_receipt` and answers `operation_id` + `local_durability` (+ `_error`) | the named-token gateway test's second half |
| no operator bundle-capture path | `src/main.rs` under `sim`: `GOSSIP_RECORD_BUNDLE_DIR` → current-thread runtime under the seams, bundle at shutdown; CI compiles `--bin mycelium --features cli,sim` | compile gate in CI; smoke-run by hand 2026-09-26 (a `cli,sim` build, six seconds, SIGTERM): `build.json`, `choices.trace` (974 B), `witness` absent by design, and `config.json` **empty** — `Bundle::new` takes only the trace, so the redacted config is attached by hand (stated in diagnostics.md) |
| `gateway_named_tokens` had no env var | `GOSSIP_GATEWAY_NAMED_TOKENS`, `name\|token\|scope,scope;…`, malformed refuses | `named_tokens_parse_from_the_environment_format`, `a_malformed_named_token_entry_refuses_the_whole_variable`, `the_control_bound_and_named_tokens_come_from_the_environment` |
| `scope_admits` is exact-or-`*`, so `llm:*` admitted nothing silently | **not widened** — `validate()` refuses a family wildcard by name (widening would have granted access at upgrade to tokens that never had it) | `a_family_wildcard_scope_is_refused_at_validate_not_silently_inert` |
| examples without `required-features` | only `coordination_viz` and `control_envelope_viz` lacked entries (two of the review's three already had them); added | — |
| two TTLs on one membership-intent key | pinned: floor 30 s, an order of magnitude below the governor's 5 min, cross-referenced doc comments | a `const _: () = assert!(…)` in `helpers.rs` — the build stops if they are unified upward |

**Invariants touched:** none new; no lock added (the tuning governor's bound is two atomics). The
`GossipConfig` upgrade note is in `CHANGELOG.md` `[Unreleased]`.
