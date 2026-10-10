#!/usr/bin/env bash
# The positioning gate (docs/plans/proposition-alignment.md D1, D5, D6).
#
# 1. Every front door quotes the canonical hero and sentence from docs/positioning.md verbatim.
# 2. The five-step developer path is identical on the README (the *Build a fleet* route), the developer
#    guide and the examples page (its learning path); the routing structure — the README's intent
#    router and its targets, the examples chooser, the operations door's own journey — is
#    scripts/check-front-doors.py (structure and link targets, not prose; docs/positioning.md
#    § The routes and the five-step path).
# 3. No live artifact links the repository's pre-move account.
set -u
cd "$(dirname "$0")/.."
fail=0
say() { printf '%s\n' "$*" >&2; }

sentence=$(grep -A1 '^<!-- sentence -->' docs/positioning.md | tail -1 | sed 's/^\*\*//; s/\*\*$//')
[ -n "$sentence" ] || { say "check-positioning: no sentence found in docs/positioning.md"; exit 2; }
hero=$(grep -A1 '^<!-- hero -->' docs/positioning.md | tail -1 | sed 's/^\*\*//; s/\*\*$//')
[ -n "$hero" ] || { say "check-positioning: no hero found in docs/positioning.md"; exit 2; }

# ── 1. the doors ────────────────────────────────────────────────────────────────
doors=(
  README.md
  docs/guide/faq.md
  docs/guide/README.md
  docs/guide/building-on-mycelium.md
  docs/operations/README.md
  docs/operations/customer-pilot.md
  docs/operations/engagement-kit.md
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
  if ! grep -qF -- "$hero" "$f"; then
    say "positioning: $f does not quote the hero verbatim"; fail=1
  fi
done

# ── 2. the path ─────────────────────────────────────────────────────────────────
strip_links() { sed -E 's/\[([^]]*)\]\([^)]*\)/\1/g; s/^[[:space:]]*//' ; }
canon=$(awk '/<!-- path:start -->/{f=1;next} /<!-- path:end -->/{f=0} f' docs/positioning.md | strip_links)
[ -n "$canon" ] || { say "check-positioning: no path block in docs/positioning.md"; exit 2; }
for f in README.md docs/guide/README.md examples/README.md; do
  got=$(awk '/<!-- path:start -->/{f=1;next} /<!-- path:end -->/{f=0} f' "$f" | strip_links)
  if [ "$got" != "$canon" ]; then
    say "positioning: $f's five-step path differs from docs/positioning.md (or is missing its path markers)"; fail=1
  fi
done
python3 scripts/check-front-doors.py || fail=1

# ── 3. the pre-move account ─────────────────────────────────────────────────────
# Historical records keep the old name on purpose: the wiki's history and lint logs, the analysis
# ledgers, the submitted paper sources (frozen at their DOI), and the CLA allowlist (old commits).
hits=$(git grep -l -I --untracked "RichardEko" -- . \
  ':!**/paper1/**' ':!**/paper2a/**' ':!**/.log/**' \
  ':!**/history.md' ':!**/lint-calibration.md' ':!**/scale-tests.md' \
  ':!**/doc-coverage.md' ':!**/ratings.md' ':!**/cla.yml' \
  ':!**/proposition-alignment.md' ':!scripts/check-positioning.sh' 2>/dev/null || true)
if [ -n "$hits" ]; then
  say "positioning: live artifacts still link the pre-move account (RichardEko):"; say "$hits"; fail=1
fi

python3 scripts/check-materials.py || fail=1

[ "$fail" = 0 ] && say "check-positioning: ok"
exit $fail
