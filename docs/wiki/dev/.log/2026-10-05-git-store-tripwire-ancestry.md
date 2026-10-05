## [2026-10-05] ingest | the git store's push tripwire checks ancestry, not equality

**What:** `mycelium-wiki/src/git_store.rs` (`diverged_after_push`, factored out of `publish`; the rule
changed from equality to ancestry), an in-crate witness, the changelog, the realignment plan's R6a row,
and this log.

**Durable knowledge:**

- **A post-push check that compares heads for equality is wrong whenever more than one writer pushes
  to the remote.** Between our push and our `ls-remote` another publisher can land on top; the
  remote is then a descendant of our head, which is the healthy case. The question a tripwire should
  ask is *does the remote still contain what we published* — `merge-base --is-ancestor`. The wiki's
  `GitMirror` keeps its equality check: a mirror is the single writer to its remote, so there a
  different head *is* a divergence.
- **How it surfaced:** the release runbook's step 2b. `main`'s CI failed once on the ten-council
  contention run while the PR that preceded it was green — the race is narrow and needs ten
  concurrent pushers. It had shipped since the tripwire was added; nothing in the release content
  caused it.
- **Making a narrow race deterministic:** factor the check into a method with its *old* semantics
  first, write the witness against the method (A publishes, B publishes on top, then A asks), see it
  fail, and only then change the rule. The fail-first evidence is then about the rule, not about
  timing.
