#!/usr/bin/env python3
"""The release workflow verifies before it publishes (security-operations SO3).

WHY IT EXISTS
  `.github/workflows/release.yml` attests every asset and verifies each attestation before it attaches the asset to a
  Release or moves `latest`. A workflow change cannot be unit-tested, so this checks the shape instead, and catches a
  future asset added to the upload, or a step reordered, without a verification ahead of it.

THE RULES (each failure names the rule and the job or step)
  pinned       every `uses:` is pinned to a full 40-hex commit SHA (the workflow holds `id-token: write`).
  top-level    the workflow-level `permissions:` grants no write.
  signing      `id-token: write` and `attestations: write` appear only in a job with an `actions/attest` step, and
               every such job has both.
  attest-then-verify
               every `actions/attest` step with `subject-path` is followed in its job by
               `verify-release-attestation.sh files`; every one with `push-to-registry` by `… image`.
  upload       every `gh release upload` is preceded in its job by `verify-release-attestation.sh files` and by
               `… image` for every image attestation in the job, and uploads only the paths that check covers.
  latest       every `imagetools create` that names `:latest` is its own step (not the version tag's), preceded in
               its job by `verify-release-attestation.sh image`, and tags a digest (`@`), not a tag.

USAGE
  scripts/with-pyyaml.sh scripts/check-release-workflow.py              # check the workflow
  scripts/with-pyyaml.sh scripts/check-release-workflow.py --self-test  # and each planted fault fails it
"""
import copy
import re
import sys
from pathlib import Path

import yaml

WORKFLOW = Path(__file__).resolve().parent.parent / ".github/workflows/release.yml"
PINNED = re.compile(r"^[\w.-]+/[\w./-]+@[0-9a-f]{40}$")
# What `verify-release-attestation.sh files` checks: it refuses a directory holding anything else.
UPLOADABLE = {"bundle/mycelium-*.tar.gz", "bundle/SHA256SUMS", "bundle/*.sigstore.json"}
VERIFY_FILES = "verify-release-attestation.sh files"
VERIFY_IMAGE = "verify-release-attestation.sh image"


def load(text):
    # PyYAML reads the key `on` as True; nothing here needs it.
    return yaml.safe_load(text)


def writes(perms):
    if perms in (None, {}, "read-all"):
        return []
    if perms == "write-all":
        return ["write-all"]
    return [k for k, v in perms.items() if v == "write"]


def check(wf):
    errors = []
    if writes(wf.get("permissions")):
        errors.append(f"top-level: the workflow grants {writes(wf.get('permissions'))}; grant per job")
    for jname, job in wf["jobs"].items():
        steps = job.get("steps", [])
        runs = [s.get("run", "") or "" for s in steps]
        uses = [s.get("uses", "") or "" for s in steps]
        for i, u in enumerate(uses):
            if u and not PINNED.match(u.split()[0]):
                errors.append(f"pinned: {jname} step {i} uses {u!r}, not a full commit SHA")
        attest = [i for i, u in enumerate(uses) if u.startswith("actions/attest@")]
        perms = job.get("permissions") or {}
        grants = set(writes(perms))
        if attest and not {"id-token", "attestations"} <= grants:
            errors.append(f"signing: {jname} attests without id-token: write and attestations: write")
        if not attest and grants & {"id-token", "attestations"}:
            errors.append(f"signing: {jname} holds {sorted(grants & {'id-token', 'attestations'})} but attests nothing")
        verify_files = [i for i, r in enumerate(runs) if VERIFY_FILES in r]
        verify_image = [i for i, r in enumerate(runs) if VERIFY_IMAGE in r]
        image_attest = [i for i in attest if (steps[i].get("with") or {}).get("push-to-registry") in (True, "true")]
        for i in attest:
            w = steps[i].get("with") or {}
            if "subject-path" in w and not any(v > i for v in verify_files):
                errors.append(f"attest-then-verify: {jname} step {i} attests files and nothing after it runs {VERIFY_FILES}")
            if i in image_attest and not any(v > i for v in verify_image):
                errors.append(f"attest-then-verify: {jname} step {i} attests an image and nothing after it runs {VERIFY_IMAGE}")
        for i, r in enumerate(runs):
            for line in r.splitlines():
                if "gh release upload" in line:
                    if not any(v < i for v in verify_files):
                        errors.append(f"upload: {jname} step {i} uploads before any {VERIFY_FILES}")
                    for a in image_attest:
                        if not any(a < v < i for v in verify_image):
                            errors.append(f"upload: {jname} step {i} uploads before the image attested at step {a} is verified")
                    paths = {t for t in line.split() if t.startswith("bundle/")}
                    for p in sorted(paths - UPLOADABLE):
                        errors.append(f"upload: {jname} step {i} uploads {p}, which {VERIFY_FILES} does not cover")
                    if not paths:
                        errors.append(f"upload: {jname} step {i} uploads nothing under bundle/ (the verified directory)")
                if "imagetools create" in line and ":latest" in line:
                    if re.search(r'-t\s+"?\$IMAGE:\$VERSION', line) or line.count(" -t ") > 1:
                        errors.append(f"latest: {jname} step {i} tags latest together with another tag")
                    if not any(v < i for v in verify_image):
                        errors.append(f"latest: {jname} step {i} moves latest before any {VERIFY_IMAGE}")
                    for a in image_attest:
                        if not any(a < v < i for v in verify_image):
                            errors.append(f"latest: {jname} step {i} moves latest before the image attested at step {a} is verified")
                    if not re.search(r'"\$IMAGE@\$\w+"\s*$', line.strip()):
                        errors.append(f"latest: {jname} step {i} moves latest to something other than a digest")
    return errors


def job_with(wf, needle):
    for name, job in wf["jobs"].items():
        for i, s in enumerate(job.get("steps", [])):
            if any(needle in (s.get(k, "") or "") for k in ("name", "run", "uses")):
                return name, i
    raise SystemExit(f"self-test: no step holds {needle!r}; the planted faults no longer fit the workflow")


def self_test(text):
    base = load(text)
    if check(base):
        print("self-test: the workflow itself fails:", *check(base), sep="\n  ")
        return 1
    plants = {}

    def plant(name, fn):
        wf = copy.deepcopy(base)
        fn(wf)
        plants[name] = wf

    def drop(needle):
        def f(wf):
            j, i = job_with(wf, needle)
            del wf["jobs"][j]["steps"][i]
        return f

    def move_before(needle, anchor):
        def f(wf):
            j, i = job_with(wf, needle)
            steps = wf["jobs"][j]["steps"]
            step = steps.pop(i)
            _, a = job_with(wf, anchor)
            steps.insert(a, step)
        return f

    plant("pinned", lambda wf: wf["jobs"]["version"]["steps"][0].update(uses="actions/checkout@v4"))
    plant("top-level", lambda wf: wf.update(permissions={"contents": "write"}))
    plant("signing (id-token on a build job)", lambda wf: wf["jobs"]["linux"]["permissions"].update({"id-token": "write"}))
    plant("signing (attest without id-token)", lambda wf: wf["jobs"]["bundle"]["permissions"].pop("id-token"))
    plant("attest-then-verify (bundle's verify dropped)", drop('cp "$BUNDLE_PATH"'))
    plant("upload (publish's re-verify dropped)", drop("Verify each file's attestation again"))
    plant("upload (before the image is verified)", move_before("gh release upload", "Verify each image's attestation"))
    plant("upload (an asset nothing verifies)", lambda wf: _edit_run(wf, "gh release upload", "bundle/SHA256SUMS", "bundle/SHA256SUMS bundle/sbom.json"))
    plant("latest (before verification)", move_before("Move latest", "Verify each image's attestation"))
    plant("latest (with the version tag)", lambda wf: _edit_run(wf, "imagetools create -t \"$IMAGE:$VERSION\"", "-t \"$IMAGE:$VERSION\"", "-t \"$IMAGE:$VERSION\" -t \"$IMAGE:latest\""))
    plant("latest (to a tag, not a digest)", lambda wf: _edit_run(wf, "Move latest", "\"$IMAGE@$index\"\n", "\"$IMAGE:$VERSION\"\n"))
    plant("attest-then-verify (image verify dropped)", drop("Verify each image's attestation"))
    failed = 0
    for name, wf in plants.items():
        errs = check(wf)
        if errs:
            print(f"ok    {name}: {errs[0]}")
        else:
            print(f"MISS  {name}: the planted fault passed")
            failed += 1
    return 1 if failed else 0


def _edit_run(wf, needle, old, new):
    j, i = job_with(wf, needle)
    step = wf["jobs"][j]["steps"][i]
    if old not in step["run"]:
        raise SystemExit(f"self-test: {old!r} is not in the step holding {needle!r}")
    step["run"] = step["run"].replace(old, new, 1)


def main():
    text = WORKFLOW.read_text()
    if "--self-test" in sys.argv[1:]:
        return self_test(text)
    errs = check(load(text))
    for e in errs:
        print(f"check-release-workflow: {e}", file=sys.stderr)
    if not errs:
        print("check-release-workflow: every upload and the move of latest follow their verification; every action is pinned")
    return 1 if errs else 0


if __name__ == "__main__":
    sys.exit(main())
