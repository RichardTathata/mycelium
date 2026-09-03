# ingest — nightly runner: the stale-checkout trap + the 08:00 schedule's first stretch read correctly (2026-09-03)

Surfaced while checking the 08:00 nightly results during the second 360° review pass.

- **09-01 green, in timing, confirms the TCC mechanism**: started 08:00:18, the scale suite passed at
  11:11 — blocked on the Allow prompt until the operator saw it, then six minutes for all three
  suites. 09-02's one-minute `DeadlineExceeded` triple is the prompt not being seen in time.
- **09-03 is the documented FORWARD-chain formation timeout, not the prompt**: build fine
  (~20 min), 100 containers up, seed+mgmt healthy, `? of 100 visible` (runner→mgmt curl failing).
  Network verified healthy host-side and VM-side the same afternoon. Re-run once before calling
  it a regression (the page's own rule).
- **The runner tested a stale checkout for 17 days.** `git pull skipped (offline / no auth)` on
  every run was false: the runner clone had a dirty tree (an in-place plist edit from 08-18) and
  `pull --ff-only` refused, silently under `2>/dev/null`. Clone sat at `b082802`. Scale-relevant
  code unchanged across the window, so the 09-01 evidence stands for current `src`. Fixed: the
  script names its failure mode (dirty / updated / failed) and prints the HEAD it runs; the clone
  fast-forwarded to HEAD and its origin repointed from the pre-move `RichardEko` URL.
- Correction to my own diagnosis mid-session: a "Docker→registry timeout" I reported came from
  invoking `timeout`, which macOS lacks — the probe never ran. Verified properly afterwards.

Lessons kept on the page: **a best-effort step that swallows stderr must still name its failure
mode**; and the ratings Addenda 4 → 5 record that the analysis loop repeated the log's false
"offline" claim without checking it.
