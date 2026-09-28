#!/usr/bin/env bash
# The positioning gate (docs/plans/proposition-alignment.md D1, D5, D6).
#
# 1. Every front door quotes the canonical sentence from docs/positioning.md verbatim.
# 2. Every funnel shows the five-step path with the same text (link targets stripped).
# 3. No live artifact links the repository's pre-move account.
set -u
cd "$(dirname "$0")/.."
fail=0
say() { printf '%s\n' "$*" >&2; }

sentence=$(grep -A1 '^<!-- sentence -->' docs/positioning.md | tail -1 | sed 's/^\*\*//; s/\*\*$//')
[ -n "$sentence" ] || { say "check-positioning: no sentence found in docs/positioning.md"; exit 2; }

# ── 1. the doors ────────────────────────────────────────────────────────────────
doors=(
  README.md
  docs/guide/faq.md
  docs/guide/README.md
  docs/guide/building-on-mycelium.md
  docs/operations/README.md
  src/lib.rs
  CLAUDE.md
  docs/wiki/wiki.md
  mycelium-py/README.md
  mycelium-ts/README.md
  docs/publications/customer-pitch.html
  docs/publications/presentation.html
)
for f in "${doors[@]}"; do
  if ! grep -qF -- "$sentence" "$f"; then
    say "positioning: $f does not quote the sentence verbatim"; fail=1
  fi
done

# ── 2. the path ─────────────────────────────────────────────────────────────────
strip_links() { sed -E 's/\[([^]]*)\]\([^)]*\)/\1/g; s/^[[:space:]]*//' ; }
canon=$(awk '/<!-- path:start -->/{f=1;next} /<!-- path:end -->/{f=0} f' docs/positioning.md | strip_links)
[ -n "$canon" ] || { say "check-positioning: no path block in docs/positioning.md"; exit 2; }
for f in README.md docs/guide/README.md examples/README.md docs/operations/README.md; do
  got=$(awk '/<!-- path:start -->/{f=1;next} /<!-- path:end -->/{f=0} f' "$f" | strip_links)
  if [ "$got" != "$canon" ]; then
    say "positioning: $f's five-step path differs from docs/positioning.md (or is missing its path markers)"; fail=1
  fi
done

# ── 3. the pre-move account ─────────────────────────────────────────────────────
# Historical records keep the old name on purpose: the wiki's history and lint logs, the analysis
# ledgers, the submitted paper sources (frozen at their DOI), and the CLA allowlist (old commits).
hits=$(grep -rln "RichardEko" \
  --exclude-dir=target --exclude-dir=.git --exclude-dir=node_modules --exclude-dir=.venv \
  --exclude-dir=paper1 --exclude-dir=paper2a --exclude-dir=.log \
  --exclude=history.md --exclude=lint-calibration.md --exclude=scale-tests.md \
  --exclude=doc-coverage.md --exclude=ratings.md --exclude=cla.yml --exclude=proposition-alignment.md \
  --exclude=check-positioning.sh . 2>/dev/null || true)
if [ -n "$hits" ]; then
  say "positioning: live artifacts still link the pre-move account (RichardEko):"; say "$hits"; fail=1
fi

[ "$fail" = 0 ] && say "check-positioning: ok"
exit $fail
