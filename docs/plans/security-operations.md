# Security operations — SIEM export, SDK dependency audits, signed release assets

**Status:** agreed with the maintainer on 2026-11-22 (rev 0.1). Three rows, SO1–SO3, answering three items of a
CISO-style security assessment of the project (its row 9, the SIEM, and row 14, patching and provenance; the
assessment is not vendored here). Decisions D1–D12 (§ 1) were taken as recommended. **SO3** (signed release assets)
is built in this file's first PR; **SO2** (the SDKs' dependency audits) is in progress in a sibling PR; **SO1** (SIEM
export) is planned for the wave 2 MINOR. The scope every row was written from read `main` at `9644e9f8`.

**Where the rows ride** (D11, D12): SO2 and SO3 change CI and the release workflow only, with no library change, so
they need no MINOR and land as soon as they are reviewed — the next tag (wave 1b's MINOR) is the first attested
release. SO1 changes configuration, routes and a guarantee, so it rides in the wave 2 MINOR. Both are scheduled into
[`identity-and-lifecycle.md` § 4](identity-and-lifecycle.md#4--sequence-and-release-shape), the way the critique
plan's rows ride in wave 1b. The ids SO1–SO3 were unused in `docs/` when this plan was written.

## 1 · Decisions (taken 2026-11-22)

The decisions that change who can read what are marked **[R]**; the one that changes a default, **[D]**.

| # | Decision | Taken |
|---|---|---|
| D1 | The sink's format | **Native JSON lines (`mycelium.audit/1`) now; OCSF later**, as a second encoder over the same sink, built only when a consumer names its OCSF version. Each line carries the signed bytes, so an external verifier can run `verify_linked`; OCSF loses information (`detail` is free-form, `principal` has no clean actor field). |
| D2 | Where the sink writes | **A file only.** No syslog client in the library: a shipper (Vector, Fluent Bit, Filebeat, rsyslog `imfile`) tails the file. No network client or new dependency in `src/`, and no UDP syslog's silent loss. |
| D3 | A sink that cannot keep up | **Count every drop**, including an embedder's own sink's (`audit_sink_dropped`). The shipped sink **refills its gaps from the chain, verifying each record**; a hole it cannot refill is written as a `gap` line and counted. The secure profile gains no requirement in this wave. |
| D4 | Pruning against the sink | **`audit_prune_to_checkpoint` never prunes past a configured sink's durable cursor.** The node prunes less, never more. |
| D5 **[R]** | Who can read the export file | **Created mode `0600`**, with a `file_mode` override. A record holds principals, targets and detail, which only `audit:read` sees today. |
| D6 **[R]** | Refusals and decisions | The action evaluator's decisions already reach the chain (`outcome = Denied`) and are exported as they stand. Evidence-journal bodies are **deferred** (the assessment's export loop). **Authentication refusals (401, 403) are never sealed into the chain**: an unauthenticated caller would write replicated KV on every node. |
| D7 **[R]** | The cursor | **`GET /gateway/audit?node=<id>&since=<seq>&limit=<n>`**, `since` requiring `node`; default limit 500, cap 5000. Each record carries `prev_hash`, `sig` and `signed`; the page carries `head`, `next`, `more`, `behind` and `pruned_below`. Full verification only on `verify=full`. No `since` behaves as today. |
| D8 **[R][D]** | Credentials on `/metrics` and `/stats` | **Opt-in `gateway_telemetry_access = "public" \| "credential"`, default `"public"`.** In `"credential"` mode both paths need the new scope **`metrics:read`**; `/health` and `/ready` stay public. Refused at start on an open gateway and in a build without `compliance`. Reported as `gw.telemetry_closed`. **`secure-single-domain` requires it one MINOR later**, announced in this wave's deprecations. The global default never flips. |
| D9 | How strict the SDK audit is | **Fail on any advisory at any severity, development dependencies included**, as `cargo audit --deny warnings` does. Ignores per lockfile, each dated with a reason and a removal condition; a stale ignore warns. For Python, the frozen environment of each manifest and the declared floors of the three shipped packages. |
| D10 | Release signing | **GitHub artifact attestations** (Sigstore keyless, SLSA v1 provenance, the public Rekor log) over the three tarballs, `SHA256SUMS`, the image index and both per-architecture images; image attestations pushed to ghcr.io, bundles attached to the Release; verified in the workflow before upload and before `latest` moves. GPG rejected (a long-lived key); `cosign sign-blob` rejected (the same trust, less provenance). SLSA Build L3 (`slsa-github-generator`) can follow; it needs a reusable-workflow restructure. |
| D11 | Timing | SO2 and SO3 need no MINOR and land before wave 1b's tag; SO1 rides in the wave 2 MINOR. |
| D12 | Where the plan lives | This file, scheduled into `identity-and-lifecycle.md` § 4; indexed in [`README.md`](README.md); watch row W1 set to `plan`, then `adopted`, linking SO3. |

## 2 · The rows

<a id="so1"></a>

### SO1 · SIEM export: a sink attachable from configuration, a cursor on the audit route, credentials on telemetry *(assessment row 9)*

**State (2026-11-22):** *planned for the wave 2 MINOR*; D1–D8 decided. Land after wave 1b's release (I7-S blocks it,
and I2 and I7-S both touch `start()` and `config.rs`); SO1-3's profile revision follows any profile revision I2 makes.

**What today lacks.** The `AuditSink` trait (`src/agent/audit.rs`) is attachable in code only (`with_audit_sink`), is
fed through a 4096-slot channel whose overflow is one `warn!` and no counter, and has no shipped implementation, so
the binary cannot attach one and the confined-fleet profile's `audit_sink` requirement cannot be met from
configuration. `GET /gateway/audit` has no cursor (`limit` means "the most recent n"), returns neither the signature
nor `prev_hash`, and re-verifies every stream on every call. `/metrics` and `/stats` are public by design.

**What (design summary):**
1. **The cursor (SO1-1).** `GET /gateway/audit?node=&since=&limit=` pages one node's stream after a seq through a
   crate-internal reader that walks this node's own keys from `since + 1` to the chain head by direct `get` (O(records
   returned)). Each record carries `seq`, `hlc`, `time`, `principal`, `action`, `target`, `outcome`, `detail`,
   `prev_hash`, `content_hash`, `sig` and `signed`. The page reports `head`, `next`, `more`, **`behind`** (a cursor
   beyond the head — a truncation signal the chain itself cannot give) and `pruned_below`. The consumer verifies
   continuity with `verify_linked` from its own anchor.
2. **The shipped sink (SO1-2).** `[audit_export]` (`path`, `format = "mycelium.audit/1"`, `max_file_bytes`,
   `file_mode`; `GOSSIP_AUDIT_EXPORT_PATH`) attaches a JSON-lines file sink at `start()`, refused with no `[tls]`, with
   a code sink already attached, or without `compliance`. The writer runs on its own thread with a bounded queue,
   one line per sealed record, fsync per batch, never truncated; rotation renames and never deletes. A dropped record
   is refilled from the chain, **verified under the node's own keys and by hash link** (a slot any member can write
   is never trusted for where it sits); one that is absent, pruned or unverified becomes a `gap` line and is counted
   (`audit_export_unrecoverable`). After a restart the file's last line is the resume cursor. Pruning stops at the
   sink's cursor (D4). Counters on `/stats` and as `mycelium_audit_export_*`, and `audit_sink_dropped` for any sink.
   No lock of its own (a channel, thread-owned state, atomics), so no lock-order row; file access enters the
   sim-seam baseline in the open.
3. **Credentials on telemetry (SO1-3).** `gateway_telemetry_access` (`GOSSIP_GATEWAY_TELEMETRY_ACCESS`); in
   `credential` mode `/stats` and `/metrics` move into the gated router under `metrics:read` (the wildcard and the
   legacy token already admit them); `PUBLIC_PATHS` becomes a function of the configuration, and the public-surface
   test checks both modes. A Prometheus scrape presents a named token with `scopes = ["metrics:read"]`.
4. **OCSF (SO1-4, optional)**, only when D1's condition is met: one pinned OCSF version, class *API Activity* (6003),
   the Mycelium-only fields under `unmapped` so the record can still be verified.

**Exit:** a consumer pulls the chain page by page and verifies it with `verify_linked` from its own anchor; a tail
tombstoned by another member shows as `behind`; with a queue of one, the sink's file holds every sealed record and no
gap; a record planted at a slot by another member is exported as a gap, not as a record, and counted (the
forged-record test); a pruned unexported record is a gap line; a restart neither duplicates nor skips; in credential
mode `/stats` answers 401 without a bearer, 403 without `metrics:read` and 200 with it, and `/health` stays public.
Each test is seen failing first.

**Does not:** ship syslog, export evidence-journal bodies or spans (the assessment's export loop, later); seal
authentication refusals; give an anchor a fully rewritten fleet cannot defeat (CR3 and the operator's WORM store);
map to OCSF unless SO1-4 is taken; change the default of any setting.

**Format:** a new configuration table and field, a new scope, new response fields (additive). No wire change, and no
change to an existing on-disk file. **Size:** M (three PRs, plus one optional).

<a id="so2"></a>

### SO2 · The SDKs' dependencies audited in CI, as the Rust ones are *(assessment row 14)*

**State (2026-11-22):** *in progress* in a sibling PR; D9 decided. Not tied to a MINOR.

**What:** `npm audit --package-lock-only` over every tracked `package-lock.json`; `pip-audit` over the frozen
environment of every tracked `pyproject.toml` and `requirements*.txt`, and over the shipped packages' declared floors;
the CI-only inline pins moved into requirements files; it fails on any advisory not ignored, development
dependencies included, with each ignore per file, dated, with a reason and a removal condition, and a stale ignore
warning; it fails closed on finding nothing, or on not finding `mycelium-ts`.

**Exit:** a planted vulnerable fixture fails the script's test and an ignored one passes; the first run is green, or
each finding is fixed or ignored in the open; floors with a known advisory are raised.

**Does not:** audit what an SDK user's own resolver picks above the floor at install time; that is the user's
lockfile. **Size:** S.

<a id="so3"></a>

### SO3 · Release assets carry build provenance *(assessment row 14; watch [W1](rust-ecosystem-watch.md#w1-build-provenance-for-the-node-and-its-capabilities); ledger [`binary-provenance`](../operations/what-is-proven.md#binary-provenance))*

**State (2026-11-22):** *built* in this plan's first PR; first proven end to end by the next `v*` tag (wave 1b's
MINOR). Each promise below is quoted next to what delivers it, or written *not built*.

**What, and where it is:**
- *"GitHub artifact attestations (Sigstore keyless, SLSA v1 provenance) on each tarball, `SHA256SUMS`, the image index
  and the per-architecture images."* — `.github/workflows/release.yml`: the `bundle` job's `actions/attest` step over
  `bundle/mycelium-*.tar.gz` and `bundle/SHA256SUMS` (one attestation, four subjects); the `publish` job's three
  `actions/attest` steps over the amd64, arm64 and index digests. `actions/attest` with no predicate input generates
  SLSA v1 build provenance ([actions/attest, *Attestation modes*](https://github.com/actions/attest#attestation-modes)).
  It is the action `attest-build-provenance` v4 wraps; that action's README says new workflows should use it.
- *"The image attestations are pushed to GHCR, and the bundles are attached to the Release."* — `push-to-registry:
  true` on the three image steps; the Release upload attaches `bundle/*.sigstore.json`: `mycelium-NEW.sigstore.json`
  (the files) and `mycelium-NEW-image-{amd64,arm64,index}.sigstore.json`.
- *"Each is verified in the workflow before the upload and before `latest` moves."* —
  `scripts/verify-release-attestation.sh`, run after each attestation: `gh attestation verify` pinned to the
  repository, the exact signer identity `…/release.yml@<run's ref>` (`--cert-identity`; gh's `--signer-workflow` is a
  prefix match, #827's review F3), the run's ref and commit, and GitHub-hosted runners; once against GitHub's store
  and once against the bundle that will be attached (and, for an image, against the copy in the registry), plus two
  verifications that must fail — a ref that did not build it, and a prefix of the real identity. The `publish` job
  verifies the files again on the runner that uploads, and the upload step first checks the directory holds exactly
  the eight expected regular files (`assets`, F7). The image index is pushed under a staging tag, `sha-<commit>`;
  `:VERSION` and then `latest` are set after the image verification, each by digest to the verified index and checked
  to name it (F4).
- *"Permissions are per job."* — the workflow grants nothing at its top level; `id-token: write` and `attestations:
  write` are held by `bundle` and `publish` only; `packages: write` by `linux` (the per-arch push) and `publish`;
  `contents: write` by `publish` only.
- *"The dispatch dry run attests and verifies."* — so does the pull-request dry run, from a branch of this repository
  (a fork's pull request has no OIDC token, and the job says so and skips). See *What a dry run proves* below. **Only a
  push of a `v*` tag publishes:** a dispatch is a dry run on any ref, a tag included (#827's review F1 found that a
  dispatch on a tag ref published, whatever its input said), and `publish` refuses anything but a push by itself. A
  dispatch on a tag does not attest either, so an attestation whose identity names a tag comes only from its push.
- A tag is published only from a commit on `main` or a `release-*` branch, and only while it still points at the
  commit the run built (F6, the workflow's half).
- Every action in `release.yml` is pinned to a full commit SHA, and every base image (the Dockerfile's frontend,
  `rust:` and `debian:` stages, the smoke's container) by digest, since the workflow holds `id-token: write`; every
  checkout sets `persist-credentials: false` (F9). `cla.yml`'s third-party action, which runs on any user's comment
  with `contents: write`, is pinned by SHA too (F5); the other workflows' pins are a separate PR.
- `scripts/check-release-workflow.py` (in `make check` and CI's lint job) is an allow-list over the workflow's
  triggers, permissions, actions, step conditions and publishing commands, with continuation lines joined and
  comments dropped; it requires every upload and image tag after its verification, `latest` only in the last step,
  and publishing only from a tag push. Its `--self-test` plants 41 faults — the first version's 12, the 17 of #827's
  review (all of which the first version passed), and one or more per fix F1, F4, F6, F7, F9 — and catches each.
  Seen failing first: it reports main's workflow before this PR as failing. `verify-release-attestation.sh
  --self-test` plants ten directory faults (symlinks, directories in a bundle's name, dotfiles, extra or missing
  files, a mismatched sum).
- **Open (the maintainer's, a repository setting, F6):** a tag ruleset on `v*` restricting who may push one. The
  workflow refuses a tag off `main` and `release-*`, but cannot stop a writer pushing a tag on a commit of either.

**What a dry run proves, and what only a tag will.** A dry run attests and verifies the real tarballs and
`SHA256SUMS` exactly as a tag run does, in the same job, and uploads them as a workflow artifact (with the bundle).
It pushes no image and touches no Release, so **the image attestations, their registry copies, the staging and
version tags, the Release attachment and the move of `latest` run only on a tag**: the first tag after this PR is
their first run. A dry run's attestation names its own ref (`refs/pull/N/merge` or a branch; a dispatch on a tag
attests nothing), which every operator check — the exact identity `release.yml@refs/tags/vNEW` — refuses.

**Exit:** on the dry run and on the first tag, `gh attestation verify` passes for every asset and for
`oci://…@digest` of all three image digests, pinned to `release.yml` and the run's ref *(dry run: the files half, met —
#827's run 45024079934, all four files verified against the store and the bundle, the wrong ref refused; the tag:
both halves, pending)*; `latest` is not moved by a run whose verification failed *(by construction and by the
structural check; first exercised at the tag)*; `RELEASING.md` § 8 carries the operator's command *(done)*.

**Does not:** claim reproducible builds; ship an SBOM (a follow-up, `actions/attest` with `sbom-path`); reach SLSA
Build L3; sign capability artefacts (W1's second half, artifact-library design work); attest releases cut before it
(v2.40.1 and earlier carry sums only). **Size:** S.

## 3 · Order

```
now, any order (no MINOR):      SO2 · SO3 (before wave 1b's tag)
wave 2 MINOR, after wave 1b:     SO1-1 ──> SO1-2 ;  SO1-3 (independent; its profile revision after I2's)
the MINOR after wave 2:          secure-single-domain requires gw.telemetry_closed (announced in wave 2)
when a consumer names OCSF:      SO1-4
```

Each PR gets the independent adversarial review (verification policy rule 4). For SO1-2 the reviewer's enumeration
includes every caller of `seal_and_write` (the grep `seal_and_write(` in `src/`), the prune path and the start
refusals; for SO1-3, every router that mounts `/stats` or `/metrics` (`src/agent/http/mod.rs`) and
`CORS_READ_PATHS`; for SO3, every step of `release.yml` that uploads, tags or signs.
