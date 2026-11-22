#!/usr/bin/env python3
"""The release workflow publishes only what it has verified, and only from a tag push (security-operations SO3).

WHY IT EXISTS
  `.github/workflows/release.yml` attests every asset and verifies each attestation before it attaches the asset to a
  Release or moves an image tag. A workflow change cannot be unit-tested, so this checks the file's shape instead. It
  is deliberately an allow-list: a trigger, a permission, an action, a step condition or a publishing command it does
  not know fails it, so a change to the publishing path is a change to this file too, made in the open. Its first
  version checked a narrower shape and missed all seventeen of #827's review plants; each is now a self-test case.

THE RULES (each failure names its rule)
  triggers     `on:` is only `push` (of `v*` tags, nothing else), `pull_request` and `workflow_dispatch` — no
               `pull_request_target`, `workflow_run` or other privileged trigger.
  permissions  the top level is `permissions: {}`; every job states a mapping (never `write-all`/`read-all`);
               `id-token`/`attestations: write` only in a job that attests, and both there; `contents: write` only in
               `publish`; `packages: write` only in `linux` and `publish`.
  jobs         no job-level `uses:` (reusable workflows); no `continue-on-error` anywhere.
  conditions   a job- or step-level `if:` is one of the few this workflow uses, where it uses it (`publish` runs only
               on `dry_run == 'false'`; its steps have no condition at all).
  pinned       every step `uses:` is a known action at a full 40-hex SHA; every checkout sets
               `persist-credentials: false`.
  dry-run      `version` sets `dry_run=false` once, inside `if [[ "${GITHUB_EVENT_NAME}" == "push" &&
               "${GITHUB_REF_TYPE}" == "tag" ]]`, after the tag's ancestry check; `publish` refuses again by itself
               anything but a push of the tag, and a tag that no longer points at GITHUB_SHA.
  shell        no `|| true`, `|| :`, `set +e`, `curl`, `wget`, `gh api`, `uploads.github.com`, `docker push` or
               registry tool (`crane`, `oras`, `skopeo`, `regctl`) in any step; lines joined across `\\` and
               comments dropped before any matching, so a verification in a comment is no verification.
  verify       a verification is a line that is exactly `scripts/verify-release-attestation.sh <mode> …`, nothing
               chained to it; every `actions/attest` is followed in its job by the matching verification.
  upload       `gh release create` names no asset; `gh release upload` names exactly the three verified globs, after
               `… assets` in the same step and after the files' and every image's verification; nothing else
               publishes (gh release and imagetools only in `publish`, a registry push only by digest in `linux`).
  tags         `publish` tags the image exactly three times, each with one `-t`: `sha-$GITHUB_SHA` from the two
               per-arch digests before any attestation; `$VERSION` from the verified digest after the images verify;
               `latest` from the verified digest in the job's last step, after the upload. `:latest` appears nowhere else.

USAGE
  scripts/with-pyyaml.sh scripts/check-release-workflow.py              # check the workflow
  scripts/with-pyyaml.sh scripts/check-release-workflow.py --self-test  # and each planted fault fails it
"""
import copy
import re
import shlex
import sys
from pathlib import Path

import yaml

WORKFLOW = Path(__file__).resolve().parent.parent / ".github/workflows/release.yml"
SHA = re.compile(r"^([\w.-]+/[\w.-]+)@[0-9a-f]{40}$")
ACTIONS = {"actions/checkout", "actions/upload-artifact", "actions/download-artifact", "actions/attest",
           "docker/login-action", "docker/setup-buildx-action", "dtolnay/rust-toolchain", "Swatinem/rust-cache"}
DRY = "needs.version.outputs.dry_run == 'false'"
# Where each condition may stand: (job, level) -> allowed `if:` values. A job or step not listed has none.
CONDITIONS = {("publish", "job"): {DRY}, ("linux", "step"): {DRY},
              ("bundle", "step"): {"env.ATTEST == 'true'", "env.ATTEST != 'true'"}}
UPLOADABLE = {"bundle/mycelium-*.tar.gz", "bundle/SHA256SUMS", "bundle/*.sigstore.json"}
VERIFY = re.compile(r'^scripts/verify-release-attestation\.sh (files|image|assets) [^|;&]+$')
FORBIDDEN = [(r"\|\|\s*(true|:)(\s|$)", "a failure swallowed by `|| true`"), (r"\bset\s+\+e\b", "`set +e`"),
             (r"\bcurl\b", "curl"), (r"\bwget\b", "wget"), (r"\bgh\s+api\b", "`gh api`"),
             (r"uploads\.github\.com", "a direct upload"), (r"\bdocker\s+push\b", "`docker push`"),
             (r"\b(crane|oras|skopeo|regctl)\b", "a registry tool")]
SIGNING = {"id-token", "attestations"}
ATTEST_EXPR = ("${{ ((github.event_name == 'push' && github.ref_type == 'tag') || (github.event_name == 'workflow_dispatch'"
               " && github.ref_type != 'tag') || (github.event_name == 'pull_request' && github.event.pull_request.head"
               ".repo.full_name == github.repository && github.actor != 'dependabot[bot]')) && 'true' || 'false' }}")
TRIGGER_GUARD = '[[ "${GITHUB_EVENT_NAME}" == "push" && "${GITHUB_REF_TYPE}" == "tag" ]]'


def lines(run):
    """Logical lines: continuations joined, full-line comments and blanks dropped."""
    out, buf = [], ""
    for raw in (run or "").splitlines():
        s = raw.rstrip()
        if s.endswith("\\"):
            buf += s[:-1] + " "
            continue
        s = (buf + s).strip()
        buf = ""
        if s and not s.startswith("#"):
            out.append(s)
    if buf.strip():
        out.append(buf.strip())
    return out


def words(line):
    try:
        return shlex.split(line, comments=True)
    except ValueError:
        return line.split()


def writes(perms):
    return {k for k, v in perms.items() if v == "write"} if isinstance(perms, dict) else set()


def check(wf):
    E = []
    jobs = wf.get("jobs") or {}
    on = wf.get("on", wf.get(True)) or {}
    if isinstance(on, (str, list)):
        on = {k: None for k in ([on] if isinstance(on, str) else on)}
    for t in on:
        if t not in ("push", "pull_request", "workflow_dispatch"):
            E.append(f"triggers: `{t}` is not a trigger this workflow may have")
    push = on.get("push")
    if push != {"tags": ["v*"]}:
        E.append(f"triggers: push must be exactly tags ['v*'], not {push!r}")
    if "permissions" not in wf or wf["permissions"] != {}:
        E.append(f"permissions: the top level must be `permissions: {{}}`, not {wf.get('permissions', '(absent)')!r}")
    for jn, job in jobs.items():
        if "uses" in job:
            E.append(f"jobs: {jn} calls a reusable workflow ({job['uses']})")
        if "continue-on-error" in job:
            E.append(f"jobs: {jn} has continue-on-error")
        if "if" in job and str(job["if"]).strip() not in CONDITIONS.get((jn, "job"), set()):
            E.append(f"conditions: job {jn} has `if: {job['if']}`")
        perms = job.get("permissions")
        if not isinstance(perms, dict):
            E.append(f"permissions: job {jn} states {perms!r}, not a mapping of scopes")
            perms = {}
        for k, v in perms.items():
            if v not in ("read", "write", "none"):
                E.append(f"permissions: job {jn} grants {k}: {v!r}")
        steps = job.get("steps") or []
        attests = [i for i, s in enumerate(steps) if str(s.get("uses", "")).startswith("actions/attest@")]
        g = writes(perms)
        if attests and not SIGNING <= g:
            E.append(f"permissions: {jn} attests without id-token: write and attestations: write")
        if not attests and g & SIGNING:
            E.append(f"permissions: {jn} holds {sorted(g & SIGNING)} but attests nothing")
        if "contents" in g and jn != "publish":
            E.append(f"permissions: {jn} holds contents: write")
        if "packages" in g and jn not in ("linux", "publish"):
            E.append(f"permissions: {jn} holds packages: write")
        for i, s in enumerate(steps):
            where = f"{jn} step {i} ({s.get('name') or s.get('uses') or s.get('id') or 'run'})"
            if "continue-on-error" in s:
                E.append(f"jobs: {where} has continue-on-error")
            if "if" in s and str(s["if"]).strip() not in CONDITIONS.get((jn, "step"), set()):
                E.append(f"conditions: {where} has `if: {s['if']}`")
            u = s.get("uses")
            if u:
                m = SHA.match(str(u).split()[0])
                if not m:
                    E.append(f"pinned: {where} uses {u!r}, not a full commit SHA")
                elif m.group(1) not in ACTIONS:
                    E.append(f"pinned: {where} uses {m.group(1)}, which is not an action this workflow knows")
                if str(u).startswith("actions/checkout@") and (s.get("with") or {}).get("persist-credentials") is not False:
                    E.append(f"pinned: {where} checks out without persist-credentials: false")
                if str(u).startswith("docker/login-action@") and jn not in ("linux", "publish"):
                    E.append(f"upload: {where} logs in to a registry outside linux and publish")
            for ln in lines(s.get("run")):
                for pat, what in FORBIDDEN:
                    if re.search(pat, ln):
                        E.append(f"shell: {where} has {what}: {ln[:80]}")
                for sub in re.findall(r"\bgh\s+release\s+(\S+)", ln):
                    if sub not in ("view", "create", "upload"):
                        E.append(f"upload: {where} runs `gh release {sub}`, which this workflow does not use")
                if (re.search(r"\bgh\s+release\s+(create|upload)\b", ln) or "imagetools create" in ln) and jn != "publish":
                    E.append(f"upload: {where} publishes outside the publish job: {ln[:80]}")
                if "push=true" in ln and not (jn == "linux" and "push-by-digest=true" in ln and "name=$IMAGE," in ln):
                    E.append(f"upload: {where} pushes an image other than by digest from linux: {ln[:80]}")
                if ":latest" in ln and not (jn == "publish" and s.get("name") == "Move latest to the verified image"):
                    E.append(f"tags: {where} names :latest")
    if str(((jobs.get("bundle") or {}).get("env") or {}).get("ATTEST", "")).strip() != ATTEST_EXPR:
        E.append("dry-run: bundle's ATTEST is not the known expression (a dispatch on a tag must not sign as the tag)")
    E += check_version(jobs.get("version") or {})
    E += check_job_order(jobs)
    return E


def verifies(step, mode):
    return [ln for ln in lines(step.get("run")) if VERIFY.match(ln) and ln.split()[1] == mode]


def check_version(job):
    E = []
    steps = job.get("steps") or []
    v = next((s for s in steps if s.get("id") == "v"), None)
    if v is None:
        return ["dry-run: the version job has no step `v`"]
    ls = lines(v.get("run"))
    falses = [i for i, ln in enumerate(ls) if "dry_run=false" in ln and "GITHUB_OUTPUT" in ln]
    guard = [i for i, ln in enumerate(ls) if ln.startswith("if ") and TRIGGER_GUARD in ln]
    if len(falses) != 1:
        E.append(f"dry-run: version sets dry_run=false {len(falses)} times (must be once)")
    if len(guard) != 1:
        E.append("dry-run: version does not guard publishing on a push of a tag (GITHUB_EVENT_NAME == push and a tag ref)")
    if falses and guard:
        f, g = falses[0], guard[0]
        els, depth = len(ls), 0
        for i in range(g + 1, len(ls)):
            ln = ls[i]
            if ln.startswith("if ") and not re.search(r";\s*fi$", ln):
                depth += 1
            elif ln == "fi" or ln == "else" or ln.startswith("elif "):
                if depth == 0:
                    els = i
                    break
                if ln == "fi":
                    depth -= 1
        if not g < f < els:
            E.append("dry-run: dry_run=false is set outside the push-of-a-tag branch")
        if not any("merge-base --is-ancestor" in ls[i] for i in range(g + 1, f)):
            E.append("dry-run: the tag's commit is not checked to be on main or a release-* branch before publishing")
    if any("dry_run" in ln and "GITHUB_OUTPUT" in ln for s in steps if s is not v for ln in lines(s.get("run"))):
        E.append("dry-run: another step writes dry_run=false")
    return E


def check_job_order(jobs):
    E = []
    for jn, job in jobs.items():
        steps = job.get("steps") or []
        files = [i for i, s in enumerate(steps) if verifies(s, "files")]
        image = [i for i, s in enumerate(steps) if verifies(s, "image")]
        for i, s in enumerate(steps):
            if not str(s.get("uses", "")).startswith("actions/attest@"):
                continue
            w = s.get("with") or {}
            if "subject-path" in w and not any(v > i for v in files):
                E.append(f"verify: {jn} step {i} attests files and nothing after it verifies them")
            if w.get("push-to-registry") in (True, "true") and not any(v > i for v in image):
                E.append(f"verify: {jn} step {i} attests an image and nothing after it verifies it")
    pub = jobs.get("publish")
    if not pub:
        return E + ["upload: there is no publish job"]
    if str(pub.get("if", "")).strip() != DRY:
        E.append(f"conditions: publish must run only on `{DRY}`")
    if not {"version", "bundle"} <= set(pub.get("needs") or []):
        E.append("upload: publish must need version and bundle")
    steps = pub.get("steps") or []
    runs = [lines(s.get("run")) for s in steps]
    first = next((r for r in runs if r), [])
    if not any('"$GITHUB_EVENT_NAME" = "push"' in ln for ln in first):
        E.append("dry-run: publish's first step does not refuse, by itself, anything but a push")
    if not any('rev-parse "refs/tags/$tag^{commit}"' in ln for ln in first) or not any('= "$GITHUB_SHA"' in ln for ln in first):
        E.append("dry-run: publish's first step does not refuse a tag that no longer points at GITHUB_SHA")
    attest_img = [i for i, s in enumerate(steps) if str(s.get("uses", "")).startswith("actions/attest@")
                  and (s.get("with") or {}).get("push-to-registry") in (True, "true")]
    files = [i for i, s in enumerate(steps) if verifies(s, "files")]
    image = [i for i, s in enumerate(steps) if len(verifies(s, "image")) >= 3]
    image_done = min((v for v in image if attest_img and v > max(attest_img)), default=None)
    if len(attest_img) != 3:
        E.append(f"verify: publish attests {len(attest_img)} images, not the two per-arch images and the index")
    if image_done is None:
        E.append("verify: publish has no step that verifies all three images after they are attested")
    upload_at = None
    tags = {}
    for i, r in enumerate(runs):
        for j, ln in enumerate(r):
            w = words(ln)
            if w[:3] == ["gh", "release", "create"]:
                pos, k = [], 3
                while k < len(w):
                    if w[k] in ("--title", "--notes-file", "-t", "-F"):
                        k += 2
                    elif w[k].startswith("-"):
                        k += 1
                    else:
                        pos.append(w[k]); k += 1
                if pos != ["$tag"]:
                    E.append(f"upload: gh release create names assets or another tag: {pos}")
            if w[:3] == ["gh", "release", "upload"]:
                upload_at = i
                pos = [t for t in w[3:] if not t.startswith("-")]
                flags = [t for t in w[3:] if t.startswith("-")]
                if pos[:1] != ["$tag"] or set(pos[1:]) != UPLOADABLE or len(pos[1:]) != len(UPLOADABLE):
                    E.append(f"upload: gh release upload must name $tag and exactly {sorted(UPLOADABLE)}, not {pos}")
                if set(flags) - {"--clobber"}:
                    E.append(f"upload: gh release upload passes {sorted(set(flags) - {'--clobber'})}")
                if not any(VERIFY.match(x) and x.split()[1] == "assets" for x in r[:j]):
                    E.append("upload: the upload step does not run `verify-release-attestation.sh assets` before it uploads")
                if not any(v < i for v in files):
                    E.append("upload: the upload comes before the files are verified on this runner")
                if image_done is None or image_done > i:
                    E.append("upload: the upload comes before the images are verified")
            if "imagetools create" in ln:
                tv = [w[k + 1] for k in range(len(w) - 1) if w[k] in ("-t", "--tag")]
                src = [t for t in w[w.index("create") + 1:] if not t.startswith("-") and t not in tv] if "create" in w else []
                if len(tv) != 1:
                    E.append(f"tags: an imagetools create in publish step {i} has {len(tv)} tags (one each)")
                    continue
                t = tv[0]
                kind = {"$IMAGE:sha-$GITHUB_SHA": "staging", "$IMAGE:$VERSION": "version", "$IMAGE:latest": "latest"}.get(t)
                if kind is None:
                    E.append(f"tags: publish step {i} tags {t}, which is not staging, version or latest")
                    continue
                if kind in tags:
                    E.append(f"tags: publish tags {kind} twice")
                tags[kind] = (i, src)
    if upload_at is None:
        E.append("upload: publish uploads nothing")
    for kind in ("staging", "version", "latest"):
        if kind not in tags:
            E.append(f"tags: publish never sets the {kind} tag")
    if "staging" in tags:
        i, src = tags["staging"]
        if src != ["$IMAGE@$amd64", "$IMAGE@$arm64"]:
            E.append(f"tags: the staging tag is made from {src}, not the two per-arch digests")
        if attest_img and i > min(attest_img):
            E.append("tags: the staging index is made after an image attestation")
    for kind in ("version", "latest"):
        if kind in tags:
            i, src = tags[kind]
            if src != ["$IMAGE@$DIGEST_INDEX"]:
                E.append(f"tags: {kind} is set from {src}, not the verified index digest")
            if image_done is None or i <= image_done:
                E.append(f"tags: {kind} moves before the images are verified")
    if "latest" in tags:
        i, _ = tags["latest"]
        if i != len(steps) - 1:
            E.append("tags: latest is not set in publish's last step")
        if upload_at is not None and i < upload_at:
            E.append("tags: latest moves before the Release is attached")
        if "version" in tags and i < tags["version"][0]:
            E.append("tags: latest moves before the version tag")
    return E


# ---- self-test ---------------------------------------------------------------------------------------------------

def find(wf, job, needle):
    for s in wf["jobs"][job]["steps"]:
        if needle in (s.get("name", "") or "") + (s.get("run", "") or "") + (s.get("uses", "") or "") + str(s.get("id", "")):
            return s
    raise SystemExit(f"self-test: no step in {job} holds {needle!r}; the planted faults no longer fit the workflow")


def edit(wf, job, needle, old, new):
    s = find(wf, job, needle)
    if old not in s["run"]:
        raise SystemExit(f"self-test: {old!r} is not in {job}'s step holding {needle!r}")
    s["run"] = s["run"].replace(old, new, 1)


def move(wf, job, needle, before):
    steps = wf["jobs"][job]["steps"]
    s = find(wf, job, needle)
    steps.remove(s)
    steps.insert(steps.index(find(wf, job, before)), s)


def plants():
    P = {}
    V = "Verify each file's attestation again"
    UP = "gh release upload"
    # The first version's twelve.
    P["unpinned action"] = lambda w: w["jobs"]["version"]["steps"][0].update(uses="actions/checkout@v4")
    P["top-level contents: write"] = lambda w: w.update(permissions={"contents": "write"})
    P["id-token on a build job"] = lambda w: w["jobs"]["linux"]["permissions"].update({"id-token": "write"})
    P["attest without id-token"] = lambda w: w["jobs"]["bundle"]["permissions"].pop("id-token")
    P["bundle's verify dropped"] = lambda w: w["jobs"]["bundle"]["steps"].remove(find(w, "bundle", "Verify each file's attestation"))
    P["publish's re-verify dropped"] = lambda w: w["jobs"]["publish"]["steps"].remove(find(w, "publish", V))
    P["upload before the images verify"] = lambda w: move(w, "publish", UP, "Verify each image's attestation")
    P["an asset nothing verifies"] = lambda w: edit(w, "publish", UP, "bundle/SHA256SUMS", "bundle/SHA256SUMS bundle/sbom.json")
    P["latest before verification"] = lambda w: move(w, "publish", "Move latest", "Verify each image's attestation")
    P["latest with another tag"] = lambda w: edit(w, "publish", "Move latest", '-t "$IMAGE:latest"', '-t "$IMAGE:$VERSION" -t "$IMAGE:latest"')
    P["latest to a tag, not a digest"] = lambda w: edit(w, "publish", "Move latest", '"$IMAGE:latest" "$IMAGE@$DIGEST_INDEX"', '"$IMAGE:latest" "$IMAGE:$VERSION"')
    P["image verify dropped"] = lambda w: w["jobs"]["publish"]["steps"].remove(find(w, "publish", "Verify each image's attestation"))
    # #827's review: seventeen plants the first version passed.
    P["continue-on-error on publish verify"] = lambda w: find(w, "publish", V).update({"continue-on-error": True})
    P["if: false on publish verify"] = lambda w: find(w, "publish", V).update({"if": "false"})
    P["if: always() on upload"] = lambda w: find(w, "publish", UP).update({"if": "always()"})
    P["|| true on verify"] = lambda w: find(w, "publish", V).update({"run": 'scripts/verify-release-attestation.sh files bundle "$VERSION" || true'})
    P["verify only in a comment"] = lambda w: find(w, "publish", V).update({"run": "# scripts/verify-release-attestation.sh files bundle\ntrue"})
    P["upload a non-bundle path"] = lambda w: edit(w, "publish", UP, "--clobber", "dist/evil --clobber")
    P["assets on gh release create"] = lambda w: edit(w, "publish", UP, "--latest", "--latest dist/evil.tar.gz")
    P["latest via a line continuation"] = lambda w: find(w, "publish", "Move latest").update({"run": 'docker buildx imagetools create \\\n  -t "$IMAGE:latest" "$IMAGE:$VERSION"'})
    P["latest pushed from linux"] = lambda w: edit(w, "linux", "Push the", "push-by-digest=true,name-canonical=true,", "")
    P["latest named in linux"] = lambda w: edit(w, "linux", "Push the", "name=$IMAGE,", "name=$IMAGE:latest,")
    P["job-level write-all"] = lambda w: w["jobs"]["linux"].update({"permissions": "write-all"})
    P["top-level permissions removed"] = lambda w: w.pop("permissions")
    P["a reusable workflow, unpinned"] = lambda w: w["jobs"].update({"extra": {"uses": "someone/repo/.github/workflows/x.yml@main", "secrets": "inherit"}})
    P["pull_request_target trigger"] = lambda w: w[True].update({"pull_request_target": {}})
    P["publish's if removed"] = lambda w: w["jobs"]["publish"].pop("if")
    P["dry_run forced false"] = lambda w: edit(w, "version", "GITHUB_REF_TYPE", 'echo "dry_run=true"', 'echo "dry_run=false"')
    P["a third-party upload action"] = lambda w: w["jobs"]["publish"]["steps"].insert(3, {"uses": "softprops/action-gh-release@" + "a" * 40, "with": {"files": "dist/*"}})
    P["curl to uploads.github.com"] = lambda w: w["jobs"]["publish"]["steps"].insert(3, {"run": 'curl -X POST --data-binary @dist/x https://uploads.github.com/repos/o/r/releases/1/assets?name=x'})
    # The fixes of #827's review.
    P["a dispatch on a tag publishes (F1)"] = lambda w: edit(w, "version", "GITHUB_REF_TYPE", '"${GITHUB_EVENT_NAME}" == "push" && ', "")
    P["publish trusts dry_run alone (F1)"] = lambda w: edit(w, "publish", "The tag object", '[ "$GITHUB_EVENT_NAME" = "push" ]', "true")
    P["the version tag before verification (F4)"] = lambda w: move(w, "publish", "Tag the verified image by version", "Verify each image's attestation")
    P["the index tagged by version first (F4)"] = lambda w: edit(w, "publish", "under a staging tag", '-t "$IMAGE:sha-$GITHUB_SHA"', '-t "$IMAGE:$VERSION"')
    P["the ancestry check removed (F6)"] = lambda w: edit(w, "version", "GITHUB_REF_TYPE", "git merge-base --is-ancestor", "true")
    P["a re-pointed tag published (F6)"] = lambda w: edit(w, "publish", "The tag object", '= "$GITHUB_SHA" ]', '= "$now" ]')
    P["no assets check before upload (F7)"] = lambda w: edit(w, "publish", UP, 'scripts/verify-release-attestation.sh assets bundle "$VERSION"\n', "")
    P["checkout keeps credentials (F9)"] = lambda w: find(w, "bundle", "actions/checkout@")["with"].pop("persist-credentials")
    P["a dispatch on a tag attests as the tag"] = lambda w: w["jobs"]["bundle"]["env"].update(ATTEST="${{ 'true' }}")
    P["set +e before verify"] = lambda w: edit(w, "publish", "Verify each image's attestation", "set -euo pipefail", "set +e")
    P["gh release edit"] = lambda w: edit(w, "publish", UP, "gh release view \"$tag\" --json", "gh release edit \"$tag\" --draft=false; gh release view \"$tag\" --json")
    return P


def self_test(text):
    base = yaml.safe_load(text)
    errs = check(base)
    if errs:
        print("self-test: the workflow itself fails:", *errs, sep="\n  ")
        return 1
    missed = 0
    for name, f in plants().items():
        wf = copy.deepcopy(base)
        f(wf)
        e = check(wf)
        if e:
            print(f"ok    {name}: {e[0]}")
        else:
            print(f"MISS  {name}: the planted fault passed")
            missed += 1
    print(f"self-test: {len(plants()) - missed} of {len(plants())} planted faults caught")
    return 1 if missed else 0


def main():
    text = WORKFLOW.read_text()
    if "--self-test" in sys.argv[1:]:
        return self_test(text)
    errs = check(yaml.safe_load(text))
    for e in errs:
        print(f"check-release-workflow: {e}", file=sys.stderr)
    if not errs:
        print("check-release-workflow: publishing only from a tag push, every upload and tag after its verification; every action pinned")
    return 1 if errs else 0


if __name__ == "__main__":
    sys.exit(main())
