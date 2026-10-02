## [2026-10-02] ingest | v2.18.1 carried another session's uncommitted work

**What:** PR #474 was committed with `git add -A` from `/Volumes/Scratch/Mycelium` while another interactive
session was editing that checkout; 36 of its files (tutorials, an example, a CI step, a script, a positioning
sentence on every door) went into the commit and the release under a message that did not describe them.
The gate did not catch it because the sentence had been changed consistently. CHANGELOG [2.18.1] now says so.

**Durable knowledge:** the main checkout is shared and may hold another session's uncommitted edits at any
time. Branch work goes in a worktree; staging is by path, never `-A`; a `git status` showing files the task
did not touch is a stop, not a commit. Recorded in the assistant's memory as well.
