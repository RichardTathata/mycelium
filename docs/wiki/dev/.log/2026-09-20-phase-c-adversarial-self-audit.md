# [2026-09-20] ingest | the Phase-C adversarial self-audit, and v2.9.1

Up: [dev](../dev.md) · ledger [history](../history.md) · plan `docs/plans/v3-contracts-axis.md` §12.6 ·
tag `v2.9.1` · PR #323.

**What it was.** §12.6 asks for an adversarial self-audit at Phase C exit over **items 1 + 2 + 7
together**, on the v2.2.0 five-pass pattern, *because those three compose into the first composed
guarantee this plan makes* — a durable, attributed, cross-domain effect — and posture rule 6 says a
composed guarantee is established by argument **and** gate, never by naming.

The audit was the deliverable. Four defects in shipped code were the result, plus a fifth that only
surfaced once the first was fixed.

## Method — worth repeating

One adversarial pass per item, run independently, each told to **attack the claims rather than
summarise them** and told plainly that a clean pass is a valid result and better than an invented
finding. The composition — the actual subject — was taken separately, because no per-item pass would
have looked at it.

**Every finding was re-verified against the code before it was acted on.** That mattered: the passes
reported eighteen findings between them and not all survived. The tokio behaviour underneath the
worst one was checked against 1.53.1's own source rather than inferred.

## The composition finding — the audit's actual subject

**The composed guarantee has no gate, and is not reconstructible after the fact.** Checked several
ways: the receipt tests and the federation tests have **zero overlap** in either direction; the
Docker federation suite mentions durability, persistence and receipts **zero** times; across the
whole repository the receipt-returning write appears in only three files, none a federation test.

Underneath the missing test is a structural gap. `WriteReceipt` carries the operation and attempt
identity but **no principal**. `AeEvidence` carries the principal but **not the durability rung**.
And a receipt is **never persisted** by the substrate — it is returned to the caller and that is the
end of it. So after the fact you can prove *who asked* and that a decision was recorded, but not what
durability the resulting write established. The join key would be `operation_id`, and the join is
only possible if the caller wrote the receipt down itself.

That is precisely what posture rule 6 exists to catch, about the plan's own centrepiece. **Not fixed
in v2.9.1** — it is a design gap, not a defect, and needs its own decision.

## The four defects (fixed, v2.9.1)

1. **A durability receipt reported `on_disk` for a write that had failed.** `write_all` queues the
   syscall and returns `Ok` before it runs; `sync_data` completes that write, **discards its error**,
   and syncs a healthy descriptor. `set_requiring_sync` was worst hit — its contract is
   *durable or nothing*, and it returned `OnDisk`, then applied **and** gossiped. The fix makes a
   *completed* write the unit of the write seam. **The codebase already knew**: `do_snapshot` flushes
   for exactly this reason before reading the tail back; the receipt path did not.
2. **A federated partner could read or cancel any caller's tasks** — only `tasks/send` authorised
   against the export; `tasks/get` and `tasks/cancel` took no caller and no export, over
   caller-supplied and therefore enumerable ids.
3. **A client could supply its own caller-context frame** through the raw-emission routes, where the
   gateway is the sender so `via` matches by construction. The existing guard caught only *bare* bytes.
4. **A remote panic in item 7's own refusal path** — an under-length payload, reachable *because* the
   refusal branch answers by echoing a nonce that was never received.

## The fifth, and why it is the most instructive

`a_replayed_write_does_not_touch_the_disk` asserted that a replayed write performs no I/O and opened
a **read-only file** to prove it. The replay path *does* re-perform a recorded success. So the write
was failing with `EBADF` on every run, the swallowed error hid it, and **the test measured nothing
and passed**. Fixing the swallow made it fail — the test working correctly for the first time.

> A test that passes for the wrong reason is worth more attention than one that fails.

## Two process notes

**A fix that broke a legitimate case, caught by the suite.** Requiring `CallerAttestation::Signed`
for the node-principal mapping also closes defect 3 — and breaks a node's own *unsigned* self
envelope on a mesh with no `tls` identity, which `a_promising_node_is_never_the_bare_sender` asserts
works. Reverted, with the reason commented in place so nobody re-tries it. The route is closed where
it actually opens: at injection.

**`make check` lints the `sim` feature but does not run its tests.** Defect 5 was found by CI, not
locally, because `cargo test -p mycelium-core --features sim` is in `check-full` and not `check`.

## A correction to the release notes themselves

v2.9.1's *known and not fixed* section listed `Buffered`'s "survives a process crash" as an open
over-claim. **It is not open** — the same flush closes it: `poll_flush` awaits the in-flight
operation and returns the write's real result, so once `fs_write_all` returns `Ok` the syscall has
run and the bytes are in the OS page cache, which is exactly what `Buffered` claims. The notes
under-sold their own fix. `CHANGELOG.md` carries a dated correction; the tag message is left as the
historical record.

## Still open from the audit

None is a privilege boundary. **`persisted_by`'s "this exact content"** is the sharpest remaining:
`answer()` reads the store then syncs, but on the inbound path apply precedes the WAL append and in
`Async` that append is a `try_send` that **drops silently on a full channel and returns `Ok`** — so a
peer under backpressure holds the value with no WAL record and still answers `Persisted`. Also open:
signed catalogue replies carry no expiry or nonce (replays into a stale *view*; the provider
re-authorises at call time, so it ends in a refusal); key **rotation** is unreachable on the call
path, so a partner mid-rotation is refused as `BadSignature`; and the per-partner budget is enforced
consumer-side only.
