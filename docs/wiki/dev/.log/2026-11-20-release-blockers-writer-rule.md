## [2026-11-20] ingest | The release-blocker script's writer rule, narrowed

An independent review of #812 planted four shapes on copies of `mycelium-core/src/tls.rs` that
`scripts/check-release-blockers.sh` passed while `CA_GENERATION_OID_IS_PROVISIONAL` is `true`: a wrapper that calls
its delegate, discards the answer and mints ungated; an ungated `commit_successor_observed` naming `commit_successor()`
in a log string; the two writers handing to each other; an ungated `commit_successor` naming the delegate in a string.
The cause: any line naming any listed writer before the first write counted as the gate.

The rule now: only `commit_successor` delegates, only to `commit_successor_observed`, only when its own body has no
mint or write line, and only when every definition of the delegate holds the `if !…ca_generation_oid_open()` gate on
its own. Comments (line and nested block) and the inside of string and char literals are dropped before matching;
every definition of a listed name is checked, whatever its qualifiers or generics, each bounded by brace depth. Found
on the way and gated: a gate inside a string or a block comment, a second ungated definition under another `cfg`, a
wrapper over a delegate the file does not define, an `async` or generic writer the old pattern did not see.
The PR's own review (#818) added four: a file without `commit_successor` while the constant is `true` now fails,
naming it (a rename or a move must update the list); a definition with its `#[…]` attribute on the same line, and
one with a space before its `(`, are seen; a C string (`c"…"`, `cr#"…"#`) is read as one. A gated function nested
inside an ungated wrapper still lends it its gate; the header says so.

Still a textual tripwire, and the header says what it cannot see (no Rust parse; helpers and macros it does not name;
a listed writer missing from the file). The guards remain `mint_ca_at`'s own refusal, the readers' refusal and CI's
run of the shipped binary. Self-test 14 → 30 cases; passes under BWK awk, mawk and gawk.

Pages: none of the wiki's states the writer rule; the plan's I2 PR 2b-1 row
(`docs/plans/identity-and-lifecycle.md`) is corrected.
