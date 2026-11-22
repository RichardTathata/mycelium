#!/usr/bin/env bash
# Verify a release's build provenance before anything is attached or `latest` moves (security-operations SO3).
#
# WHY IT EXISTS
#   `.github/workflows/release.yml` attests every asset it publishes with GitHub artifact attestations (Sigstore
#   keyless, SLSA v1 build provenance, logged in Rekor). An attestation nobody checks proves nothing to the workflow
#   that made it, so the workflow runs this script after every attestation and before every upload, and
#   `scripts/check-release-workflow.py` (in `make check`) fails if an upload or the move of `latest` is not preceded
#   by a call to it. Operators use the commands in RELEASING.md § 8 instead; this script is the workflow's copy.
#
# USAGE
#   verify-release-attestation.sh files <dir> <version>
#     <dir> holds exactly what will be uploaded: the three tarballs, SHA256SUMS and *.sigstore.json bundles. Every
#     other file fails the check (an unattested file would otherwise be attached). Each tarball and SHA256SUMS is
#     verified twice: against the attestations GitHub stores for the repository, and against
#     <dir>/mycelium-<version>.sigstore.json, the bundle the Release carries for offline checks.
#   verify-release-attestation.sh image <bundle-file> <image>@sha256:<digest>
#     The image, verified against GitHub's store, against the copy pushed to the registry (--bundle-from-oci) and
#     against <bundle-file>.
#
# THE IDENTITY each verification pins (all from the workflow's own environment, so a dry run pins its own ref):
#   --repo            $GITHUB_REPOSITORY
#   --signer-workflow $GITHUB_REPOSITORY/.github/workflows/release.yml
#   --source-ref      $GITHUB_REF          (refs/tags/vX.Y.Z on a release; refs/pull/N/merge or refs/heads/main on a dry run)
#   --source-digest   $GITHUB_SHA
#   --deny-self-hosted-runners
# and, once per call, a verification pinned to a ref that did not build it must FAIL — so a pin that silently stopped
# biting (a renamed flag, an identity check skipped) fails the run instead of passing everything.
set -euo pipefail

die() { echo "::error::$*" >&2; exit 1; }
: "${GITHUB_REPOSITORY:?set by Actions; run this from the release workflow}"
: "${GITHUB_REF:?set by Actions}"
: "${GITHUB_SHA:?set by Actions}"
command -v gh >/dev/null || die "gh is not on PATH"

PIN=(--repo "$GITHUB_REPOSITORY"
     --signer-workflow "$GITHUB_REPOSITORY/.github/workflows/release.yml"
     --deny-self-hosted-runners
     --source-digest "$GITHUB_SHA")
WRONG_REF="refs/tags/v0.0.0-so3-never-built-here"

verify() { # <subject> [extra flags…]: must pass
  local subject=$1; shift
  gh attestation verify "$subject" "${PIN[@]}" --source-ref "$GITHUB_REF" "$@" >/dev/null \
    || die "attestation of $subject did not verify ($*)"
  local how="the attestation store"
  [ $# -eq 0 ] || how="$*"
  echo "verified: $subject ($how)"
}
must_refuse() { # <subject> [extra flags…]: pinned to a ref that did not build it, must fail
  local subject=$1; shift
  if gh attestation verify "$subject" "${PIN[@]}" --source-ref "$WRONG_REF" "$@" >/dev/null 2>&1; then
    die "attestation of $subject verified against $WRONG_REF: the source-ref pin does not bite"
  fi
  echo "refused, as it must be: $subject against $WRONG_REF"
}

case "${1:-}" in
  files)
    dir=${2:?dir}; version=${3:?version}
    [ -d "$dir" ] || die "no directory $dir"
    bundle="$dir/mycelium-$version.sigstore.json"
    [ -s "$bundle" ] || die "no bundle $bundle"
    expected=("mycelium-$version-linux-x86_64.tar.gz" "mycelium-$version-linux-aarch64.tar.gz"
              "mycelium-$version-macos-aarch64.tar.gz" "SHA256SUMS")
    for f in "${expected[@]}"; do [ -s "$dir/$f" ] || die "missing $dir/$f"; done
    # Everything in the directory is uploaded: each file is a verified subject or a bundle, nothing else.
    for path in "$dir"/*; do
      name=${path##*/}
      case " ${expected[*]} " in *" $name "*) continue ;; esac
      case "$name" in *.sigstore.json) continue ;; esac
      die "$name is in $dir but is not an attested subject: it would be uploaded unverified"
    done
    (cd "$dir" && sha256sum -c --strict SHA256SUMS) || die "SHA256SUMS does not match the tarballs"
    for f in "${expected[@]}"; do
      verify "$dir/$f"
      verify "$dir/$f" --bundle "$bundle"
    done
    must_refuse "$dir/SHA256SUMS"
    ;;
  image)
    bundle=${2:?bundle-file}; ref=${3:?image@digest}
    [[ "$ref" =~ @sha256:[0-9a-f]{64}$ ]] || die "$ref is not pinned by digest"
    [ -s "$bundle" ] || die "no bundle $bundle"
    verify "oci://$ref"
    verify "oci://$ref" --bundle-from-oci
    verify "oci://$ref" --bundle "$bundle"
    must_refuse "oci://$ref"
    ;;
  *)
    sed -n '2,/^set -euo/p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
