# Mycelium: the fleet guide for a coding agent

Printed by `mycelium skills get fleet`, from the text the binary embeds, so it describes the binary you ran. A
binary built from a branch may list a subcommand the latest release lacks.
Mycelium is an embedded library for agent fleets with no coordinator. `mycelium` is its node and its tools. A
"skill" here is a guide for a coding agent, not a prompt skill a node serves.

## The walk: from the binary to a governed fleet

Six steps, run in order. They are the steps the `one-hour` job runs and times against each published release.
Run steps 3 to 6 inside the scaffold's directory.

The walk needs the published binary, or a build with the release's features (`cli,tls,compliance,a2a,llm,metrics`).
A build without `compliance` fails from step 3 on: `start()` refuses the scaffold's token table, naming the
feature, and step 4 answers `not_in_build — compliance`.

1. The binary answers with its version.

```sh
mycelium --version
```

2. A scaffold, and the Python SDK from the same release. The first line below is the one command the job does
not run: it is for a Python your OS manages, which refuses a bare install. Keep the environment active to step 6.

```sh
python3 -m venv .venv && . .venv/bin/activate
mycelium init governed-call-py --template governed-call-py
pip install -q "https://github.com/RichardTathata/mycelium/archive/refs/tags/{{MYCELIUM_TAG}}.tar.gz#subdirectory=mycelium-py"
```

3. A declared unit, a permitted write, and a write the gateway refuses by name. The script starts a node on
ports 7950 (the mesh) and 8300 (the gateway), which must be free.

```sh
MYCELIUM_BIN=mycelium python3 main.py
```

Expect: `alice set demo/greeting: PERMITTED` and `reader set demo/greeting: REFUSED 403 — insufficient scope`.

4. A grant that is default-deny and holds across a restart. Write `grant.toml`: `node.toml` followed by

```toml
[persistence]
base_path = "data/grant-persist"

[tls]
auto_cert_dir = "data/grant-tls"
```

```sh
mycelium test --faults grant units --config grant.toml
```

Expect: `grant: passed`.

5. A lost acknowledgement recovered without a second effect. Write `recovery.toml`: `node.toml` followed by

```toml
[persistence]
base_path = "data/recovery-persist"
```

```sh
mycelium test --faults recovery units --config recovery.toml
```

Expect: `recovery: passed`.

6. A readiness report naming what the fleet enforces.

```sh
mycelium doctor node.toml
```

Expect: `start(): would start` and `guarantees: enforced`.

## The subcommands

Every subcommand of this binary. One not listed here is not in it. Each prints its usage with `--help`.

| Subcommand | What it does | `--format json` prints |
|---|---|---|
| `init` | writes a scaffold from a template the binary embeds | text only |
| `declare` | sends a project's units and prompt skills to a gateway, once or under `--watch` | `mycelium.declare/1`, one line per event |
| `doctor` | says what `start()` would print and whether it would refuse, without a node | `mycelium.doctor/1` |
| `wire-check` | checks a directory of unit files offline | `mycelium.design/declaration/1` |
| `rules` | prints the rule catalogue the binary registers | `mycelium.rules/1` |
| `test` | runs fault classes over a declaration on an in-process fleet (`--faults`) | `mycelium.test-faults/1` |
| `explain` | renders a decision trace, one run's chain, or a record's closure | `mycelium.explain/1`, `mycelium.closure/1` |
| `tls` | mints a CA, issues a node or client certificate, prints a pin | `mycelium.tls/1` |
| `skills` | prints an agent guide (`get fleet`, this one) or the list (`list`) | `mycelium.skills/1` |

## Reading the output

- With `--format json`, stdout is one JSON document named by its `schema` (`declare`: one per line). Logs go to
  stderr.
- A usage error, or a file that does not load, is a message on stderr and a non-zero exit, with nothing on stdout.
- A run that finds a problem still prints its document, and exits 1: `doctor` when `start()` would refuse,
  `test --faults` when a class fails, `declare` when a tick is refused. Read the document; it names what to fix.
- Within a schema version a document only grows. Ignore a field or an event you do not know.
- `tls` documents carry paths and fingerprints. Do not read, print or copy a key file. `doctor` shows a secret
  as `set` or `unset`.
- Fix what a refusal names and run the command again. Do not work around a refusal.
