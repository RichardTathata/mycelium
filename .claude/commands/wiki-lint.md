Run a lint pass over the LLM wiki (`docs/wiki/` — schema at `docs/wiki/AGENTS.md`) and fix
what it finds. Four checks, most-valuable first. Record the pass as a dated
`.log/YYYY-MM-DD-lint.md` entry in each section touched.

## 0. Review the calibration ledger (before the checks)

Read [`docs/wiki/dev/.log/lint-calibration.md`](../../docs/wiki/dev/.log/lint-calibration.md) — the
**miss-log**: every drift a *prior* lint declared clean, or a scope gap that let drift persist, with
the sharpening it produced. This is to `wiki-lint` what `ratings.md`'s calibration ledger is to
`mycelium-analysis` — it is what turns this from a checklist into an audit. Rules:

- A check with **repeated misses in one area** needs a *structural* fix, not another point patch —
  the ledger tells you which checks have been optimistic (the lock-order table and the reserved-prefix
  list each shipped drift twice before the check was widened).
- **Append to the ledger** whenever *this* pass — or anything since the last one (analysis,
  `doc-coverage`, a code review, a support question) — reveals drift a prior lint should have caught.
  A miss you fix silently is a miss the framework can't learn from. Every seeded sharpening below came
  from a real ledger entry; keep that loop closed.

## 1. Doc-vs-code verification (the load-bearing check)

For every wiki-page claim that cites code, confirm the code still says it. Minimum sweep:

- **Lock-order table** (`docs/wiki/dev/concurrency/lock-order.md`): grep the **whole
  workspace** — the table's scope is all crates since 2026-07-07 (rows 24–30 added the
  companions; the old `src/ mycelium-core/src/` sweep was the blind spot that hid them):
  `grep -rn "Mutex<\|RwLock<" src/ mycelium-core/src/ mycelium-*/src/ --include='*.rs'`
  outside test modules, **and** for lock-wrapping type aliases
  (`grep -rn "^type \|^pub type" … | grep -i "mutex\|rwlock"`) — an alias hides the lock
  from the field grep (the 2026-07-02 lint found `SenderLog` exactly this way). Diff both
  against the table's rows — grouped rows list every field name, so the diff is by name.
  Any undeclared site = a finding (this drift shipped once — analysis Run 28, calibration
  ledger 2026-07-02). **And read the acquisition sites of every row that makes a behavioural claim** —
  *held across X*, *released before Y*, *never nested with row N*, *single-flight* — and diff the table's
  prose (lock flavours, sanctioned `tokio::sync` exceptions) against its own Type column: row 17 claimed its
  write guard was held across the JWKS fetch for single-flight, and the code had fetched before locking since
  the row was written, through every name-only pass (ledger 2026-10-10).
- **Watch-channel RMW sweep** (the `borrow()`+`send()` lost-update family —
  [lock-free-and-atomics](docs/wiki/dev/concurrency/lock-free-and-atomics.md) rule: mutate watch
  state only inside `send_modify`/`send_if_modified`): grep the workspace for watch senders read
  then written in separate holds —
  `grep -rn -A8 '\.borrow()' src/ mycelium-core/src/ mycelium-*/src/ --include='*.rs'` filtered to
  hits whose following lines contain `.send(` on the same sender (exclude `#[cfg(test)]`,
  `borrow_and_update`, and receivers). Any hit = a finding: it is the exact unserialised
  read-modify-write that lost a concurrently-dialing peer in the fan-out activation
  (`peer_list_tx`, found live 2026-07-21; loom model `loom-spike/tests/bounded_append.rs` proves
  the schedule). The rule predated the bug — this sweep is what makes it mechanical.
- **Named regression gates — and every other identifier a page cites**: mechanically, not by
  eye. Extract every backticked `snake_case` identifier with two or more underscores from every
  wiki page **outside `.log/`** (`grep -rhoE '\`[a-z][a-z0-9]*(_[a-z0-9]+){2,}\`' docs/wiki
  --include='*.md'`, minus the `.log` trees) and `grep -rqF` each against the whole tree (`src/`,
  every crate's `src/`, `tests/` and `examples/`, `tests/`, `examples/`, `scripts/`, `.github/`, `Makefile`)
  — a companion's browser example (`mycelium-wiki/examples/wiki_council_viz.rs`) is cited by name, so
  leaving crate `examples/` out of the tree reports it missing (2026-10-02).
  An identifier absent from the tree = a finding: a renamed or deleted test, function, constant or
  flag. `.log/` entries are dated records and are exempt; a clippy lint name cited in prose is a
  known false positive, dismiss it by hand. This replaced "every test cited by name still exists"
  after `history.md` carried a test's pre-rename name for six days across a pass that declared
  §1 clean — the old check had no enumeration, so it checked whatever the linter remembered
  (ledger 2026-09-26). **Prove the sweep can say "present" before trusting a "missing"**: run it on one
  identifier known to exist first. A sweep reporting all 265 identifiers missing is broken, not the docs —
  under zsh an unquoted `$TREE` is one argument, so `grep` errors (exit 2) on every lookup (2026-10-09).
- **Cited constants/flags** (e.g. `MAX_KV_WRITE_BYTES`, `WIRE_VERSION`/`PREV_WIRE_VERSION`,
  `swim_failure_detector` default): read the cited file and confirm the stated value/default.
  **Scope includes guide *chapters*, not just the front-door docs** — grep every guide page that pins
  the wire version (`grep -rlnE "wire v[0-9]|WIRE_VERSION" docs/guide/`) and diff against
  `framing.rs`. `09-security.md` carried a stale `v10 "(current)"` / `v10↔v9` window through many
  passes because this check stopped at `building-on`/`faq` (ledger 2026-07-11).
- **KV-namespace table** (`src/lib.rs` §KV namespace ownership — code canon, but it drifts
  like a doc): **the sweep is `scripts/check-kv-namespaces.sh`'s job now** (a `make check` + CI gate since
  2026-10-10, after five lint misses — the 2026-07-07 lint found NINE missing, the 10-10 one four governor
  keys). It enumerates every slash-bearing constant, `format!` key head and KV-call literal in production code
  and fails on any with no row unless `scripts/kv-namespaces-nonkeys.txt` declares it a non-key. **The lint
  only reviews that allow-list's reasons**: each entry must really be a non-key (a seam stream, slot or ring
  name, schema tag, path) — an entry that is in fact a KV key is the finding; it needs a row instead.
- **Endpoint/feature lists** (`docs/wiki/dev/operations.md`): spot-check against
  `src/agent/http.rs` routes and `Cargo.toml` features. **And every URL a runbook or guide chapter tells an
  operator to call** — `grep -rnoE '(GET|POST|DELETE|PUT) /[A-Za-z0-9_/{}%.:-]+' docs/operations docs/guide
  docs/wiki` — exists as a literal in `http.rs` **and its segment shape matches**: a `{param}` pattern matches
  one path segment, so a slash-bearing value needs a `{*tail}` capture (the plan's *identifiers in paths* rule,
  §9 — `/consensus/{*slot}` since 2026-09-06; before that the runbooks' literal lock URL 404ed for two months,
  ledger 2026-09-06). When in doubt, probe it with a test — the router, not the doc, is canon. **A brace list of
  routes (`govern/{timing,tuning,…}`) is a set**: diff it against every router route with that prefix
  (`grep '.route("/govern/' src/agent/http.rs`), not member by member — checking each listed member exists let
  `/profile` go missing from 2026-09-18 and `/topology-override` from its first day (ledger 2026-10-07). A brace
  list under a **companion** prefix (`/gateway/reason/…`, `/gateway/wiki/…`) is diffed against that companion's
  router (`mycelium-<crate>/src/http.rs`) — the reason façade sat off its list for five weeks (ledger 2026-10-10).
- **CI-gate list** (`docs/wiki/dev/testing/testing.md`): diff the documented gate block against the
  *actual* `run:` steps in `.github/workflows/*.yml`. A page that lists the *clippy* of a crate's
  tests can imply coverage CI doesn't provide — `mycelium-core`'s whole suite was clippy-compiled but
  never *run* (no `-p mycelium-core` test job), and `testing.md` read as if it were covered (ledger
  2026-07-11). Confirm every gate the page names has a live `run:` line. **And for any step a page says
  runs a *reduced* feature set** (gateway-free, no default features, a minimal embed), confirm the
  feature set it actually builds: a step with `--tests`/`--all-targets` builds dev-dependencies, and
  `mycelium`'s dev-dependencies unify `gateway` and `tls` back on —
  `cargo tree -e features,normal,dev --no-default-features -i mycelium` shows it. The CI "Gateway-free
  build" job's clippy was described as the dead-code catcher for months while linting a gateway build
  (ledger 2026-10-06). **A sentence naming which tests a step runs** ("runs the node-free `auth.test.ts`")
  is a set too: diff it against the runner's own collection — jest's `testMatch`/`--listTests`, a
  `--test '*'` glob, the pytest directory — since the step's flags say nothing about which files exist
  (`testing.md` named one jest file of eight, ledger 2026-10-07).
- **External front-door docs that *restate* code facts** — `docs/guide/building-on-mycelium.md`
  (and lightly `docs/guide/faq.md`). These live outside `docs/wiki/` but duplicate code by
  design, so they drift like a wiki page and are higher-stakes (downstream integrators act on
  them). Verify: the reserved-KV-prefix list matches the `src/lib.rs` namespace-ownership
  table (top-level prefixes — grep `\| \`` rows, diff the sets); `WIRE_VERSION`; the eight
  sub-handle names; the `Cargo.toml` feature flags — **as a set**: derive the `[features]` names from the
  manifest and confirm each is named on the page or deliberately omitted (`default`, `fuzz-internals`'s
  warning); checking only that the *named* flags exist let `compliance` go unmentioned from 2.3.0 to
  2.25.0 (ledger 2026-10-06) — and the same set diff runs on **every** page that enumerates the features
  (`dev/operations.md` § Feature gates omitted `sim`, ledger 2026-10-10); **the install snippet's `tag = "…"` pins are the newest tag
  on each line** (`git tag -l 'v*' | sort -V | tail -1`; `git tag -l 'mycelium-<crate>-*'`). A mismatch = a finding (fix the doc) —
  and since 2026-09-26 the pin is also a **release-runbook anchor** (`RELEASING.md` §6), so a pin found stale here means
  a release skipped that step, which is a second finding to record. The
  *linking* front-doors (the FAQ's routing tables) need only the dead-link check in §3.

- **Closed-set claims are enumerations.** A sentence of the form *the one / the only / every / no other* on
  `runtime-invariants.md`, `lock-order.md` or `security.md` closes over a class of code sites: grep the code for
  every site of that class and diff — `runtime-invariants.md` called #162 *the one legitimate termination* for two
  weeks after C5 added the removed-member drop at four sites in `connection.rs` (ledger 2026-10-10). And where a page
  restates a code list that implements a property (*self-owned*, *protected*, *reserved*), diff the list against
  every declaration of that property in the code too, not only against the page: the tripwire's
  `SELF_OWNED_SYS_PREFIXES` omits `sys/consensus-accepted/`, which its own `kv_ns` doc calls strictly self-owned —
  a code gap to report, not a page edit.

Numbers the wiki deliberately does NOT pin (test counts, dep counts) are exempt — the
convention is "run the suite for the live total".

## 2. Staleness

Pages contradicted by work merged since the last lint: check each section's `.log/` dates
against `git log --oneline --since=<last lint>` — merged PRs with durable knowledge but no
ingest entry indicate a stale or missing page. ✅-shipped items still described as
pending/planned = a finding. **Every "unreleased" marker** on a live page (`grep -rn "unreleased" docs/wiki docs/guide
docs/operations docs/design docs/plans`, minus `.log/` and `history.md`'s dated section headings, which are records like a `.log` entry) is checked against the PR it names: `git tag --contains <merge sha>`
non-empty means it shipped, and the marker is a finding — two survived v2.26.0's release (ledger 2026-10-07). `docs/plans`
was missing from this grep until 2026-10-10, though a swept root since 2026-09-24 (ledger 2026-10-10).

## 3. Orphans & dead links

Every page reachable from its folder-note chain up to `wiki.md`; every relative link
resolves; every folder has its `<folder>/<folder>.md` folder-note. Also resolve every
relative link in the external front-door docs (`docs/guide/faq.md`,
`docs/guide/building-on-mycelium.md`) — they route into the guide/examples and break silently
when a chapter or example is renamed/moved. **Sweep inbound `#anchor` links too, not just
outbound** — when a heavily-linked front-door doc's *headings* change (e.g. an
`examples/README.md` section rename), links that point *into* the old heading break silently
across the repo. `grep -rnoE "examples/README\.md#[a-z-]+" docs/ README.md mycelium-*/` (and the
same for any front-door doc whose headings you changed) and confirm each `#anchor` still matches a
live heading. A README restructure shipped two such dead anchors (`#find-one-by-layer`, `#the-suites`)
before this sweep existed (ledger 2026-07-15).

## 4. Coverage

Durable knowledge with no wiki home: scan recent merged work (plans marked complete,
analysis findings, new invariants in code comments) and file what's missing under the right
section (routing test in `AGENTS.md`). **Not just "does the knowledge exist somewhere" — does the
*wiki cite* it?** The wiki's contract is "code is canon, the wiki cites it", so a new authoritative
`docs/design/` decision (or ADR) whose subject the wiki already covers must be *linked* from the
relevant wiki page/folder-note, not merely exist in `docs/design/`. A merged design doc that the
companion/architecture pages embody but never reference = a coverage finding (add the pointer). This
is a scope gap a prior pass hit: it counted `coordination-approaches.md` as "covered" because it
existed user-facing, without checking the wiki linked it (ledger 2026-07-13).

**A prose list of a manifest's set is an enumeration too — diff it against the manifest.** A sentence
that names *which* bins carry `required-features`, which crates a feature pulls in, or which profiles a
compose file has is a parallel copy of a `Cargo.toml`/compose fact, and it drifts like one: `testing.md`
said *"Four bins need it"* (wasm) from 2026-07-07 while the manifest grew to nine, through five passes
(ledger 2026-10-02). Derive the set from the manifest (`awk` over `[[bin]]` blocks) and diff the names.

**A page listing a suite's demos is diffed by name against the runner** — the `run_demo` lines in
`examples/coop/ci_smoke.sh`, the `[[bin]]` names in the crate's manifest — not counted: `examples.md` said fourteen
demos with twelve in CI while `procurement_authority` ran as the thirteenth for two weeks (ledger 2026-10-07).
**When a pass changes a count, grep the page — and its siblings — for every spelling of the old one**, digits and
words: the 10-07 fix moved "fourteen" to "fifteen" in one sentence and left "additional to the fourteen above" two
paragraphs later (ledger 2026-10-09).

**Enumeration pages: audit by *category*, not by pinned count.** For pages that list a set
(`dev/examples.md`, the sub-handle list, the feature list), a re-checked count is not coverage — a
whole *category* can be missing while the count it pins is still "right." `examples.md` enumerated the
coop demo count for many passes while the entire **visual-showcase** category (`conway`, `conway-gpu`,
the `*_viz` set) was absent (ledger 2026-07-14). So verify the enumeration covers every category that
exists in the tree. **Derive the category set from the tree — never a fixed list** (a hardcoded
category set is the same drift-bug one level up: it went stale the moment `mycelium-guardrails`,
`mycelium-reason`, and `mycelium-wiki` examples existed but sat outside the swept scope — ledger
2026-07-15). For examples that means `find . -path '*/examples/*.rs'` across **all** crates (not just
`examples/` + coop) plus the coop bins; group into categories; diff against the page. The durable fix
for recurrence: `dev/examples.md` should **cite `examples/README.md` § The capability matrix** (the front-door
index) as the canonical list, so the wiki synthesizes rather than maintaining a parallel enumeration
that silently falls behind.

**UI-example contract** ([`dev/ui-example-contract.md`](../../docs/wiki/dev/ui-example-contract.md)).
For every **browser** example (a `.rs` that `include_str!`s an `.html`, or serves inline HTML), verify
it: (1) advertises `ui/viz` + injects `__OPS_CONSOLE_LINK__`; (2) injects `__CONCEPTS__` (or, for the
documented inline-HTML/no-gateway/console exceptions `three_node_demo`/`conway-gpu`/`ops_console`,
carries the equivalent static panel or is exempt); (3) has a documented run command carrying
`gateway,metrics` (or `metrics` where gateway is default). A UI example missing the concepts box, the
Ops Console link, or the metrics feature is a finding. Enumerate them the tree-derived way: `grep -rl
'include_str!.*\.html' examples mycelium-*/examples` — and **classify every hit** as *compliant* or a
*documented exception*. An **unclassified** hit is itself a finding: the earlier check verified a known
list of 9 + named 2 exceptions but never reconciled against the full grep, so `ops_console` (a browser
example that is the *consumer* of `ui/viz`, not an advertiser) sat unclassified until its 2026-07-15
move re-ran the enumeration (ledger). **Diff the hit count against the count written on the
contract page's classification line** — the page pins it for exactly this; two compliant examples
(`control_envelope_viz`, `coordination_viz`) sat unclassified through the 2026-09-24 pass because the
enumeration was run and the list was not reconciled against it (ledger 2026-09-26).

## Output

Fix findings directly (they're doc edits), write the dated `.log/` lint entries naming what
was fixed, and report: findings by check, pages touched, anything needing a user decision
(e.g. a new top-level section — never add one unprompted). If a doc-vs-code finding reveals
the *code* is wrong rather than the page, stop and report it as a code bug instead of
"fixing" the wiki to match.

**Close the loop.** If this pass found drift a *prior* lint declared clean (or you learned of such
drift from analysis / `doc-coverage` / a review since the last pass), append a line to
[`.log/lint-calibration.md`](../../docs/wiki/dev/.log/lint-calibration.md): the check, the drift, how
it was found, and the **sharpening** — and, where the sharpening is a concrete check change, fold it
into the checks above (as the 2026-07-11 entries did). A miss recorded but not turned into a sharper
check is a lesson half-learned. A clean pass that surfaced no misses needs no ledger entry — don't
manufacture one.
