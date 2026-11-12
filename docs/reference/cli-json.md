# The CLI's JSON documents

Every `mycelium` subcommand an agent or a script reads takes `--format json` and prints one JSON document on stdout
(`declare`: one per line), named by a `schema` id. Its logs go to stderr, so stdout parses as a whole. A refusal — a
usage error, a file that does not load, a key that would be replaced without `--force` — is a message on stderr and a
non-zero exit, with nothing on stdout.

| Command | Schema | Golden(s) |
|---|---|---|
| `mycelium doctor --format json` | `mycelium.doctor/1` | none; its fields are [`Doctor`](../../src/doctor.rs)'s |
| `mycelium wire-check --format json` | `mycelium.design/declaration/1` | [`declaration.schema.json`](declaration.schema.json) |
| `mycelium rules --format json` | `mycelium.rules/1` | [`rule-catalogue.json`](rule-catalogue.json) |
| `mycelium test --faults --format json` | `mycelium.test-faults/1` | [`tests/fixtures/pilot-kit/faults.json`](../../tests/fixtures/pilot-kit/faults.json) |
| `mycelium explain <trace> --format json` | `mycelium.explain/1` | [`explain.json`](../../tests/fixtures/cli/explain.json), [`explain-target.json`](../../tests/fixtures/cli/explain-target.json) |
| `mycelium explain --correlation <id> … --format json` | `mycelium.closure/1`, `view: "chain"` | [`explain-correlation.json`](../../tests/fixtures/cli/explain-correlation.json) |
| `mycelium explain --closure <hash> … --format json` | `mycelium.closure/1`, `view: "closure"` | [`explain-closure.json`](../../tests/fixtures/cli/explain-closure.json), [`explain-closure-missing.json`](../../tests/fixtures/cli/explain-closure-missing.json) |
| `mycelium declare [--watch] --format json` | `mycelium.declare/1`, one line per event | [`declare.json`](../../tests/fixtures/cli/declare.json), [`declare-refused.json`](../../tests/fixtures/cli/declare-refused.json), [`declare-watch-*.json`](../../tests/fixtures/cli) |
| `mycelium tls ca\|issue\|pin --format json` | `mycelium.tls/1` | [`tls-ca.json`](../../tests/fixtures/cli/tls-ca.json), [`tls-issue-node.json`](../../tests/fixtures/cli/tls-issue-node.json), [`tls-issue-client.json`](../../tests/fixtures/cli/tls-issue-client.json), [`tls-pin.json`](../../tests/fixtures/cli/tls-pin.json) |

The last four rows are identity-and-lifecycle N3. Their goldens are checked by
[`tests/cli_json.rs`](../../tests/cli_json.rs), which runs the built binary: every object's keys, every array's length
and every value must equal the golden's, except where the golden holds a placeholder for a value that differs between
runs — `"<sha256>"`, `"<path:SUFFIX>"`, `"<gateway>"`, `"<string>"`. A field added, dropped or renamed fails there.
`--format text` (the default) prints what each command printed before.

## `mycelium explain <decisions.jsonl> --format json` — `mycelium.explain/1`

The text form's explanation ([`explain_json`](../../mycelium-core/src/decision.rs)).

- `file` — the trace, as given.
- `targets` — in first-seen order, each `{target, steps}`; a step is the record's `seq`, `node`, `rule`,
  `rule_revision`, `trigger`, `outcome`, `reason`, `effect` (`null` where the record has none — the text's `-`),
  `reads` (each input's `source`, `value`, `provenance`, `truncated`), `unattributable` (it read a gossiped or sampled
  input) and `inputs_truncated`. A step is the record's fields, not the record: the record is in the trace.
- `rules` — every rule used, `{id, summary}`; `summary` is the catalogue's, or `null` without one (`--catalogue`).
- `counts` — `records`, `targets`, `unattributable`, `inputs_truncated`, `unparsed` (lines that did not parse).
- `dropped` — a sentence: what the sink dropped is in its counters, not in this document.

## `mycelium explain --correlation | --closure … --format json` — `mycelium.closure/1`

The schema id the text form already prints at its head is kept; `view` is `"chain"` or `"closure"`
([`chain_json`, `closure_json`](../../src/closure.rs)).

- `files` — each file read: `path` as given, `kind` (`journal` or `trace`), `records`, `unparsed`, `duplicates` (an
  earlier file's records, byte for byte), `dropped` (the sink's counters from `decisions.stats.json`, `null` when not
  given — which is not zero).
- `records` — each with its `mark` (`captured`, `reconstructed`; in a closure the root without a parent is
  `captured, a root`), `source`, `file`, `index`, the full `hash`, `kind`, `at_ms`, `node`, `enforcement_point`,
  `correlation`, `operation_id`, `attempt_id`, `parent` (the full hash) and `parent_held` (whether a file holds it;
  `null` without a parent), `summary`; in a closure also `depth` and `boundary` (a federation edge: the leaf, not the
  end of the chain).
- A chain's `correlation` and `counts`: `records`, `captured`, `reconstructed`, `unattributable_in_files` (records
  with no id at all), `other_correlation`.
- A closure's `root`, `root_found` (`false`: the root is in none of the files, and `records` is empty), `root_cited_by`
  (when not found), `root_parent_missing` (the root's own parent when no file holds it), `fleet` — `journals`, `fleet`
  (`--fleet`, or `null`), `nodes_named`, and `complete`: `false` where the text says INCOMPLETE, `true` when the stated
  fleet's journals are all given, `null` when the reader cannot tell — `holes` (`{hash, cited_by}`), `hole_reason`
  and `counts` (`records`, `files`, `boundaries`, `holes`).
- `order` — how records are ordered, and how far each source is trusted.

## `mycelium declare --format json` — `mycelium.declare/1`

One JSON object per line ([`event_json`](../../src/declare.rs)): once, a single line; under `--watch`, a line per
event, so the stream is JSON Lines. An unchanged tick prints nothing, as in the text form. `event` is one of:

- `declared` — `gateway`, `units`, `skills`, `not_enforced` (what the node said it does not enforce), `warnings`.
- `refused` — `wire-check` found errors: `errors`, `report` (wire-check's text report), `previous_stands` (whether
  this watch holds an earlier declaration). Exit 1 on the first tick.
- `rejected` — the gateway refused a unit the check passed: `unit`, `error`; its previous declaration was put back.
- `error` — the watch itself was refused (no credential, no scope, no gateway): `error`.
- `heartbeat_lost` — a handle the node no longer knows; declared again at the next tick: `error`.
- `retracted` — Ctrl-C under `--watch`: `declarations`, the count retracted.

## `mycelium tls … --format json` — `mycelium.tls/1`

**Paths and fingerprints only.** A document names the files written and the SHA-256 of each certificate's
`SubjectPublicKeyInfo`, read back from the file on disk; it never carries a key or a certificate body —
`tests/cli_json.rs` asserts no PEM block and no line of any file written appears in the output. `command` is `ca`,
`issue` or `pin`; `warnings` holds what `tls_writable_by_others` says about a directory another local user could
write (on stderr in the text form).

- `ca` — `cert` (`ca-cert.pem`), `key` (the path of `ca-key.pem`, which stays on this host), `spki_sha256` (the CA
  certificate's; not a pin, since no gateway presents the CA's key).
- `issue`, `kind: "node"` — `node`, `cert`, `key`, `spki_sha256` (the value `tls pin` prints for this certificate,
  the one a partner pins), `ca_cert`, `ca_spki_sha256`.
- `issue`, `kind: "client"` — `principal` (`token:{issuer}/{name}`), `issuer`, `name`, `cert`, `key`, `spki_sha256`.
- `pin` — `cert`, `spki_sha256`. A CA certificate or a file with a key is refused as in the text form.
