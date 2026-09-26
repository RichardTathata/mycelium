## [2026-09-26] fix | external review F7, F9, and the trust-slice doc drift

Up: [dev](../dev.md) · [testing](../testing/testing.md) · follows
[examples declare the features they use](2026-09-26-examples-required-features.md).

An independent customer-readiness review of `46ef0c9` (v2.15.0 + 1) reported ten findings; the
three public-repo code/doc items are fixed on `fix/f7-f9-doc-drift`. The private-repo items (pin,
lockfile, exporter and outbox recovery) are separate work.

- **F7** — `declared_electorate_min` read `mono_now_ns() / 1_000_000` and subtracted a Unix-ms
  `written_at_ms`. The monotonic origin is `MONO_ORIGIN_NS` (a year in ns ≈ 3.15e10 ms), three orders
  below any Unix timestamp, so the subtraction saturated to `0` and the electorate floor **never
  expired**. Shipped in #377 (v2.14.0). Now `sim_seam::wall_now_ms()`, the same clock the membership
  governor's `read_fresh_intent` uses. Regression test `electorate_intent_tests` in `helpers.rs`,
  seen failing before the fix.
- **F9** — `tests/gateway_mandate_external.rs` (added #399) panicked with *no reactor running* since
  #417 made `with_execution_authority` spawn the C10 sweeper; with a runtime it then failed on fixed
  timestamps that predate C8's `mark_started`. Now `#[tokio::test]` with stamps taken after attach,
  and **run in `ci.yml`** beside `ae_external_adapter` — it never had a run line.
- **Doc drift** — `declare_trust`'s comment (and the `consensus.rs` header, and guide chapter 4) said
  slices were stored for a future extension; the ballot loop has filtered the tally on the declared
  set under `use_trust_slices` all along. Restated as what it is: a fixed *eligible* voter set, quorum
  size unchanged, intersection not implemented — and the header now states the safety-sensitive
  profile (fixed `quorum_size`, slices on, `count_opaque_as_absent` off, no membership changes),
  which is the review's F8 answered by restriction rather than by a versioned electorate.
- **Guide wording (F10, public half)** — README's *nowhere else* (admission is scoped, forwarding is
  not) and *every higher-layer feature lives in the KV* (the evidence journal deliberately does not);
  the on-ramp pin `v2.13.0` → `v2.15.0`; the pilot page's *days old* dated to v2.4.0.

- **Retry policy (exit criterion 3)** — `scripts/ci-retest.sh` gains `CI_RETEST_STRICT=1`, under
  which a pass on isolated retry fails the job; applied to the `compliance,a2a` security gate in
  `ci.yml`. The F8 eligibility filter was already pinned by two `lib_tests` (a vote from outside
  the declared set is not counted); the `consensus.rs` header now cites them.

Pages touched: `dev/testing/testing.md` (the two rules, and the strict mode in the flake tier). The wiki's lesson is the one the review
drew: a documented guarantee was treated as covered without an executed test of its failure boundary.
