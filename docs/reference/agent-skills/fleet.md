# Mycelium: the fleet guide for a coding agent

Printed by `mycelium skills get fleet`, from the text the binary embeds, so it describes the binary you ran.
Mycelium is an embedded library for agent fleets with no coordinator. `mycelium` is its node and its tools. A
"skill" here is a guide for a coding agent, not a prompt skill a node serves.

## The walk: from the binary to a governed fleet

Six steps, run in order. They are the steps the `one-hour` job runs and times against each published release.
Run steps 3 to 6 inside the scaffold's directory.

1. The binary answers with its version.

```sh
mycelium --version
```

2. A scaffold, and the Python SDK from the same release.

```sh
mycelium init governed-call-py --template governed-call-py
pip install -q "https://github.com/RichardTathata/mycelium/archive/refs/tags/{{MYCELIUM_TAG}}.tar.gz#subdirectory=mycelium-py"
```

3. A declared unit, a permitted write, and a write the gateway refuses by name.

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
| `declare` | sends a project's units and skills to a gateway, once or under `--watch` | `mycelium.declare/1`, one line per event |
| `doctor` | says what `start()` would print and whether it would refuse, without a node | `mycelium.doctor/1` |
| `wire-check` | checks a directory of unit files offline | `mycelium.design/declaration/1` |
| `rules` | prints the rule catalogue the binary registers | `mycelium.rules/1` |
| `test` | runs fault classes over a declaration on an in-process fleet (`--faults`) | `mycelium.test-faults/1` |
| `explain` | renders a decision trace, one run's chain, or a record's closure | `mycelium.explain/1`, `mycelium.closure/1` |
| `tls` | mints a CA, issues a node or client certificate, prints a pin | `mycelium.tls/1` |
| `skills` | prints this guide (`get fleet`) or the list (`list`) | `mycelium.skills/1` |

## Reading the output

- With `--format json`, stdout is one JSON document named by its `schema` (`declare`: one per line). Logs go to
  stderr.
- A refusal is a message on stderr and a non-zero exit, with nothing on stdout. `declare` reports what a tick
  decided as an event on stdout, with exit 1.
- Within a schema version a document only grows. Ignore a field or an event you do not know.
- `tls` documents carry paths and fingerprints. Do not read, print or copy a key file. `doctor` shows a secret
  as `set` or `unset`.
- Fix what a refusal names and run the command again. Do not work around a refusal.
