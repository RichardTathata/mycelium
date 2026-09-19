# 2026-09-19 — item 5's decisive demonstration, and the defect it found

**What shipped.** `mycelium-wiki/examples/curator_handover.rs` (feature `git-store`, run in CI);
`WikiError::mandate_revoked` / `as_mandate_revoked` carrying `MandateRevoked { refname, expected,
found }`; `GitStore` distinguishing a fence refusal from a CAS conflict; two integration tests; the
gallery row; plan §12.1 row for item 5 marked delivered.

**The defect, which is the reason §12 is a gate and not decoration.** The mandate fence puts the
appointment check inside the write's own git transaction — verify the mandate ref, update the
content ref, both or neither. That part worked. But a failed transaction returned
`CommitOutcome::RefMoved`, which the caller reports as `WikiError::Conflict`: *"compare-and-swap
version conflict (re-read and retry)"*. So **a curator who had been replaced was told to re-read and
retry**, and retrying refuses forever.

A conflict and a revocation have opposite remedies. Item 5's own record says a refusal that names
something else invites a fix that does not help; `ResourceAuthority::check` is ordered epoch-first
for exactly this reason. The store was doing the check correctly and then describing it wrongly,
which is the kind of thing only a consumer notices.

**The fix.** On a failed transaction with a fence configured, ask the fence's ref what it holds now;
if it is not what this writer expected, return `mandate_revoked` instead. Carried inside
`WikiError::Io` exactly as `GateRefusal` already is — additive, no downstream match breaks. A
genuine lost CAS race still returns `Conflict`, which still *is* a retry signal, and the plant test
pins that so this is not "every failure now says revoked".

**Why nobody had seen it.** The fence's tests check the **transaction text** — that
`update_ref_stdin` builds the right `verify`/`update` lines. Nothing had ever run the fence against
real git. Building the demonstration was its first end-to-end exercise, and it took about four
minutes to surface. A mechanism tested only at the string level is a mechanism whose *reported*
behaviour is untested.

**Two API-shape lessons the example paid for.** The store is **manifest-last** by design, so a
section written without a manifest entry is invisible to `read` — that is the torn-write guarantee
working, and it was this example's first bug. And `write_section` is a compare-and-swap: passing
`None` when the section exists is a conflict, so the example had to read the version token first,
which matters here because otherwise a CAS conflict would masquerade as the fence refusal the
demonstration is about.

**What the demonstration does not establish:** two curators writing concurrently (one process, one
thing at a time — the race the transaction removes is argued structurally); the consensus-side
handover (`LockService` under replay scenario B, item 5's D4 audit); the remote half, where `push`
carries the same fence as `--force-with-lease` and showing it honestly needs a second repository.

**Still owed by §12.1:** item 4's control-envelope viz — the last one.
