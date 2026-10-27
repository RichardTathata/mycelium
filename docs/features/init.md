# Init

## What it is

`mycelium init <dir> --template <name>` writes a [scaffold](../guide/00-concepts.md#init): a project of its own,
copied from a template the binary embeds, pinned to the release the binary came from. Three templates ship:
`governed-call`, in Rust — two in-process nodes, a member's direct call to a provider's tool permitted and refused
by a policy enforced at the provider and recorded in an evidence journal, and a prompt skill whose text is a
file — `governed-call-py`, in Python — the [published node](../guide/00-concepts.md#published-node) under a
named-token table, an SDK agent that declares a unit and writes a key, and a second agent refused by the gateway's
own scope rule — and `two-domains`, in Rust — two fleets of two nodes, each its own
[domain](../guide/00-concepts.md#domain) under `secure-multi-domain`, calling each other across one edge. The templates' source of truth is the `templates/` directory, which CI builds and runs as projects;
the binary embeds it, so the two cannot differ.

## When to use it, and when not

- **Use it when** you want a running fleet with one governed call in front of you before reading further: the
  Rust template if you embed the library, the Python one if you work from an SDK against the published node, and
  `two-domains` if your fleet calls, or is called by, another domain's — or for the secure posture of a single
  domain, which is that template with the second fleet deleted (D26).
- **Not when** you need a policy decided per call from Python: the published node attaches no evaluator, so the
  Python template's refusal is the gateway's scope rule, and its unit file's mandate and rule come back
  `not_enforced` — the node says so itself. The per-call policy is the Rust template's until a node can attach one
  from configuration.

## Turn it on

This feature is turned on in code.

- [`write`](../../src/init.rs) — writes a template into a new or empty directory, never over one, with
  `{{MYCELIUM_TAG}}` replaced by the binary's own release tag.
- [`TEMPLATES`](../../src/init.rs) — every template, every file embedded at compile time from `templates/`.
- `every_template_file_on_disk_is_embedded`, in [`src/init.rs`](../../src/init.rs): the directory and the embedded
  list hold the same files with the same bytes, so neither can fall behind the other.

## See it work

No runnable example exists: the [example index's capability matrix](../../examples/README.md#the-capability-matrix)
has no row for this feature; the templates are projects, not examples, and the CI job `Scaffold` writes and runs
every one on every change. The tests are in [`src/init.rs`](../../src/init.rs): `every_template_file_on_disk_is_embedded`,
`init_writes_the_binarys_own_tag_and_no_placeholder`, `init_refuses_a_directory_that_is_not_empty` and
`the_programs_stay_short`. The templates themselves are documented in [`templates/README.md`](../../templates/README.md).

## What it guarantees, and what it does not

- `init` adds no guarantee of its own. What a scaffolded fleet enforces is what its configuration and its program
  attach; `mycelium doctor node.toml` in either scaffold says which guarantees that is.
- The Rust template's refusal is a policy decision recorded as evidence (node-local, append-only, fsynced — not
  signed; only a reference to it enters the signed audit chain, which a node with no gateway does not run); the Python
  template's is the gateway's scope rule (`gateway.auth` in the [rule catalogue](../reference/rule-catalogue.md)), counted, not recorded.
- The two `governed-call` templates run the development posture: the Rust one shares one CA whose key is on both nodes
  (`id.ca_key_off_node` unmet), the Python one a gateway on loopback. Neither downloads a model: the skill answers
  through the echo backend. `two-domains` runs the secure one: each domain's CA minted and every certificate issued
  where no node is pointed (`mycelium tls ca`, `tls issue`), a pinned key (`tls pin`), named tokens, egress lists
  naming the partner; on one host every directory is readable by every process, which a real deployment does not share.
- `two-domains` pins a tag like the others, and **no published tag before the next MINOR release has `secure-multi-domain` (v2.38.2 is a PATCH without it)**,
  so until then the scaffold builds only against a checkout — CI's `Scaffold` job rewrites its dependency to a path,
  as it does for the Rust template.
- The plan row is [`adopter-surface.md`](../plans/adopter-surface.md) X2; its *not built* items are the TypeScript
  template (after X7) and a file watcher (X10 watches declarations, not code).

## Go deeper

- The templates: [`templates/README.md`](../../templates/README.md), [`governed-call`](../../templates/governed-call/README.md),
  [`governed-call-py`](../../templates/governed-call-py/README.md), [`two-domains`](../../templates/two-domains/README.md).
- Guide: [installation § The published node](../guide/installation.md#the-published-node) and
  [authorising actions](../guide/20-authorising-actions.md) for what the Rust template's policy is.
- No runbook and no design record; the plan row is the record.
- Code: [`src/init.rs`](../../src/init.rs), [`src/main.rs`](../../src/main.rs), [`templates/`](../../templates/README.md).
