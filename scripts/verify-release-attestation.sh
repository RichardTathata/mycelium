#!/usr/bin/env bash
# Verify a release's build provenance before anything is attached or a tag moves (security-operations SO3).
#
# WHY IT EXISTS
#   `.github/workflows/release.yml` attests every asset it publishes with GitHub artifact attestations (Sigstore
#   keyless, SLSA v1 build provenance, logged in Rekor). An attestation nobody checks proves nothing to the workflow
#   that made it, so the workflow runs this script after every attestation and before every upload or image tag, and
#   `scripts/check-release-workflow.py` (in `make check`) fails if an upload or a tag move is not preceded by a call to
#   it. Operators use the commands in RELEASING.md § 8; this script is the workflow's copy, with the same pins.
#
# USAGE
#   verify-release-attestation.sh files <dir> <version>
#     <dir> must hold exactly the three tarballs, SHA256SUMS and mycelium-<version>.sigstore.json, each a regular
#     file (not a symlink, not a directory); anything else fails, dotfiles included. Each tarball and SHA256SUMS is
#     verified twice: against the attestations GitHub stores for the repository, and against the bundle the Release
#     carries for offline checks.
#   verify-release-attestation.sh image <bundle-file> <image>@sha256:<digest>
#     The image, verified against GitHub's store, against the copy pushed to the registry (--bundle-from-oci) and
#     against <bundle-file>.
#   verify-release-attestation.sh assets <dir> <version>
#     Run in the step that uploads, before it uploads: <dir> must hold exactly the four files and the four bundles
#     (the files' and the amd64, arm64 and index images'), each a regular file, and SHA256SUMS must match. No network.
#
# THE IDENTITY each verification pins (from the workflow's own environment, so a dry run pins its own ref):
#   --repo           $GITHUB_REPOSITORY
#   --cert-identity  https://github.com/$GITHUB_REPOSITORY/.github/workflows/release.yml@$GITHUB_REF   (exact; gh's
#                    --signer-workflow is a prefix regex, so `…/workflows/rel` would pass it — #827's review, F3)
#   --source-ref     $GITHUB_REF          (refs/tags/vX.Y.Z on a release; refs/pull/N/merge or refs/heads/… on a dry run)
#   --source-digest  $GITHUB_SHA
#   --deny-self-hosted-runners
# and, once per call, two verifications that must FAIL — one pinned to a ref that did not build it, one to a signer
# identity that is a prefix of the real one — so a pin that silently stopped biting fails the run instead of
# passing everything.
set -euo pipefail

die() { echo "::error::$*" >&2; exit 1; }

# Exactly these names, each a regular file, nothing else (dotfiles included).
exact_set() { # <dir> <name>…
  local dir=$1; shift
  [ -d "$dir" ] && [ ! -L "$dir" ] || die "$dir is not a directory"
  local name path
  for name in "$@"; do
    path="$dir/$name"
    [ ! -L "$path" ] || die "$path is a symlink"
    [ -f "$path" ] || die "$path is missing or not a regular file"
    [ -s "$path" ] || die "$path is empty"
  done
  shopt -s dotglob nullglob
  for path in "$dir"/*; do
    name=${path##*/}
    case " $* " in *" $name "*) continue ;; esac
    die "$name is in $dir but is not an expected asset or bundle: it would be uploaded unverified"
  done
  shopt -u dotglob nullglob
}
assets_of() { # <version>: the three tarballs and SHA256SUMS
  echo "mycelium-$1-linux-x86_64.tar.gz mycelium-$1-linux-aarch64.tar.gz mycelium-$1-macos-aarch64.tar.gz SHA256SUMS"
}
sums_match() { (cd "$1" && sha256sum -c --strict SHA256SUMS) || die "SHA256SUMS does not match the tarballs"; }

mode=${1:-}
if [ "$mode" = --self-test ]; then
  # The local checks (no network): each planted directory must be refused, the clean ones accepted (#827's review, F7).
  tmp=$(mktemp -d); trap 'rm -rf -- "$tmp"' EXIT
  v=9.9.9; ok=0; bad=0
  mk() { # <name>: a clean upload set with matching sums
    local d="$tmp/$1"; mkdir -p "$d"
    local f; for f in linux-x86_64 linux-aarch64 macos-aarch64; do echo "$f" > "$d/mycelium-$v-$f.tar.gz"; done
    (cd "$d" && sha256sum mycelium-*.tar.gz > SHA256SUMS)
    for f in "" -image-amd64 -image-arm64 -image-index; do echo '{}' > "$d/mycelium-$v$f.sigstore.json"; done
    echo "$d"
  }
  expect() { # <want: pass|fail> <label> <dir>
    if ( "$0" assets "$3" "$v" ) >/dev/null 2>&1; then got=pass; else got=fail; fi
    if [ "$got" = "$1" ]; then echo "ok    $2 ($got)"; ok=$((ok+1)); else echo "MISS  $2: wanted $1, got $got"; bad=$((bad+1)); fi
  }
  expect pass "a clean upload set" "$(mk clean)"
  d=$(mk extra); echo x > "$d/evil.tar.gz"; expect fail "an extra file" "$d"
  d=$(mk dot); echo x > "$d/.hidden"; expect fail "a dotfile" "$d"
  d=$(mk dirbundle); mkdir "$d/extra.sigstore.json"; expect fail "a directory named like a bundle" "$d"
  d=$(mk linkbundle); rm "$d/mycelium-$v-image-index.sigstore.json"; ln -s /etc/hosts "$d/mycelium-$v-image-index.sigstore.json"; expect fail "a symlinked bundle" "$d"
  d=$(mk linkasset); mv "$d/SHA256SUMS" "$tmp/moved-sums"; ln -s "$tmp/moved-sums" "$d/SHA256SUMS"; expect fail "a symlinked SHA256SUMS" "$d"
  d=$(mk dirasset); rm "$d/mycelium-$v-macos-aarch64.tar.gz"; mkdir "$d/mycelium-$v-macos-aarch64.tar.gz"; expect fail "a directory in an asset's name" "$d"
  d=$(mk missing); rm "$d/mycelium-$v-image-arm64.sigstore.json"; expect fail "a missing bundle" "$d"
  d=$(mk empty); : > "$d/mycelium-$v.sigstore.json"; expect fail "an empty bundle" "$d"
  d=$(mk tampered); echo tampered >> "$d/mycelium-$v-linux-x86_64.tar.gz"; expect fail "a tarball that does not match SHA256SUMS" "$d"
  echo "self-test: $ok as expected, $bad missed"
  [ "$bad" -eq 0 ]
  exit
fi
if [ "$mode" = assets ]; then
  dir=${2:?dir}; version=${3:?version}
  # shellcheck disable=SC2046 # the names hold no spaces
  exact_set "$dir" $(assets_of "$version") "mycelium-$version.sigstore.json" \
    "mycelium-$version-image-amd64.sigstore.json" "mycelium-$version-image-arm64.sigstore.json" \
    "mycelium-$version-image-index.sigstore.json"
  sums_match "$dir"
  echo "the upload set is exactly the four files and the four bundles"
  exit 0
fi

: "${GITHUB_REPOSITORY:?set by Actions; run this from the release workflow}"
: "${GITHUB_REF:?set by Actions}"
: "${GITHUB_SHA:?set by Actions}"
command -v gh >/dev/null || die "gh is not on PATH"

IDENTITY="https://github.com/$GITHUB_REPOSITORY/.github/workflows/release.yml@$GITHUB_REF"
WRONG_REF="refs/tags/v0.0.0-so3-never-built-here"
PREFIX_IDENTITY="https://github.com/$GITHUB_REPOSITORY/.github/workflows/rel"

verify() { # <subject> [extra flags…]: must pass
  local subject=$1; shift
  gh attestation verify "$subject" --repo "$GITHUB_REPOSITORY" --cert-identity "$IDENTITY" \
    --source-ref "$GITHUB_REF" --source-digest "$GITHUB_SHA" --deny-self-hosted-runners "$@" >/dev/null \
    || die "attestation of $subject did not verify ($*)"
  local how="the attestation store"
  [ $# -eq 0 ] || how="$*"
  echo "verified: $subject ($how)"
}
must_refuse() { # <subject>: a wrong ref and a prefix identity must each fail
  local subject=$1
  if gh attestation verify "$subject" --repo "$GITHUB_REPOSITORY" --cert-identity "$IDENTITY" \
       --source-ref "$WRONG_REF" --deny-self-hosted-runners >/dev/null 2>&1; then
    die "attestation of $subject verified against $WRONG_REF: the source-ref pin does not bite"
  fi
  if gh attestation verify "$subject" --repo "$GITHUB_REPOSITORY" --cert-identity "$PREFIX_IDENTITY" \
       --deny-self-hosted-runners >/dev/null 2>&1; then
    die "attestation of $subject verified against the prefix identity $PREFIX_IDENTITY: the identity pin is not exact"
  fi
  echo "refused, as it must be: $subject against $WRONG_REF and against $PREFIX_IDENTITY"
}

case "$mode" in
  files)
    dir=${2:?dir}; version=${3:?version}
    bundle="$dir/mycelium-$version.sigstore.json"
    # shellcheck disable=SC2046 # the names hold no spaces
    exact_set "$dir" $(assets_of "$version") "mycelium-$version.sigstore.json"
    sums_match "$dir"
    for f in $(assets_of "$version"); do
      verify "$dir/$f"
      verify "$dir/$f" --bundle "$bundle"
    done
    must_refuse "$dir/SHA256SUMS"
    ;;
  image)
    bundle=${2:?bundle-file}; ref=${3:?image@digest}
    [[ "$ref" =~ @sha256:[0-9a-f]{64}$ ]] || die "$ref is not pinned by digest"
    [ -f "$bundle" ] && [ ! -L "$bundle" ] && [ -s "$bundle" ] || die "no bundle $bundle (a regular file)"
    verify "oci://$ref"
    verify "oci://$ref" --bundle-from-oci
    verify "oci://$ref" --bundle "$bundle"
    must_refuse "oci://$ref"
    ;;
  *)
    sed -n '2,/^set -euo/p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
