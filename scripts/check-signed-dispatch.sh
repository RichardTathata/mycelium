#!/usr/bin/env bash
# Every locally-originated KV write leaves the node through `framing::make_kv_wire_msg` — post-360
# hardening plan row F.
#
# WHY IT EXISTS
#   Under `[tls]` a node signs its KV writes (`WireMessage::SignedData`), and a TLS node can refuse an
#   unsigned `Data` frame (`kv_require_signed_frames`; unconditional at plan row F2). Row F's
#   enumeration found twenty-three agent write sites that built a bare `Data` frame themselves and never
#   signed; #611's review found a twenty-fourth — the connection handler's quorum-evidence write — in a
#   file the first version of this gate exempted whole. Every one goes through
#   `framing::local_kv_wire_msg`. This keeps a new one from arriving quietly.
#
# WHAT IT CHECKS
#   Every construction of a `Data` frame in production code under `src/`, `mycelium-core/src/` and
#   `mycelium-*/src/`: `WireMessage::Data(` in any spacing, and a bare `Data(` in a file that imports the
#   variant (`use …WireMessage::{…Data…}`, `use …WireMessage::Data`, `use …WireMessage::*`). Exempt:
#   - the codec (`mycelium-core/src/codec.rs`), which is the encoder and decoder;
#   - test code: `*_tests.rs` / `lib_tests.rs` files, and an inline `#[cfg(test)] mod x { … }` block (to its
#     closing brace; an out-of-line `mod x;` skips nothing);
#   - a line carrying the marker `signed-dispatch: allow (<reason>)`, or directly below one — used for
#     the receive-and-forward path, match-arm patterns and `make_kv_wire_msg`'s own plaintext arm.
#   `--self-test` copies the tree, plants a bare dispatch in `mycelium-core/src/connection.rs`, one in
#   a new agent file, one spelled through a `use`, one after an out-of-line `#[cfg(test)] mod x;`, and
#   one after an inline test module holding `"{"` and `'{'`, and requires the gate to name all five.
#   Brace counting blanks string and char literals one line at a time: a string spanning lines, or a raw
#   string containing `"`, can still miscount (stated, not handled).
set -euo pipefail
cd "$(dirname "$0")/.."

check_tree() {
    python3 -I - "$1" <<'PY'
import os, re, sys
root = sys.argv[1]
full = re.compile(r'WireMessage\s*::\s*Data\s*\(')
bare = re.compile(r'(?<![\w:])Data\s*\(')
imports = re.compile(r'WireMessage\s*::\s*(\*|Data\b|\{[^}]*\bData\b[^}]*\})')
marker = 'signed-dispatch: allow'
test_mod = re.compile(r'^\s*#\[cfg\((all\()?test\b')
mod_inline = re.compile(r'^\s*(pub(\([^)]*\))?\s+)?mod\s+\w+\s*\{')
# String and char literals are blanked before braces are counted (#611's third review, L1: a `"{"` in an
# inline test module hid the next item). One line at a time — a string literal spanning lines, or a raw
# string containing `"`, can still miscount; stated, not handled.
string_lit = re.compile(r'"(?:\\.|[^"\\])*"')
char_lit = re.compile(r"'(?:\\.|[^'\\])'")
def strip_literals(line):
    return char_lit.sub("' '", string_lit.sub('""', line)).split('//', 1)[0]

hits = []
dirs = ['src', 'mycelium-core/src'] + sorted(
    d + '/src' for d in os.listdir(root) if d.startswith('mycelium-') and d != 'mycelium-core' and os.path.isdir(os.path.join(root, d, 'src')))
for d in dirs:
    for dp, _, fs in os.walk(os.path.join(root, d)):
        for f in fs:
            if not f.endswith('.rs'):
                continue
            path = os.path.join(dp, f)
            rel = os.path.relpath(path, root)
            if rel == 'mycelium-core/src/codec.rs' or f == 'lib_tests.rs' or f.endswith('_tests.rs'):
                continue
            lines = open(path, encoding='utf-8', errors='replace').read().split('\n')
            imported = any(imports.search(l) and l.lstrip().startswith(('use ', 'pub use ')) for l in lines)
            skip_until = -1
            for i, l in enumerate(lines):
                if i <= skip_until:
                    continue
                if test_mod.match(l):
                    # Skip an INLINE `#[cfg(test)] mod x { … }` to its closing brace, and nothing else:
                    # an out-of-line `mod x;` (as at src/agent/mod.rs:28) ends nothing (#611's second
                    # review, N2 — the first cut stopped scanning the file there).
                    j = next((k for k in range(i + 1, len(lines)) if not lines[k].startswith('#[')), None)
                    if j is not None and mod_inline.match(lines[j]):
                        depth = 0
                        opened = False
                        for k in range(j, len(lines)):
                            code_k = strip_literals(lines[k])
                            opened = opened or '{' in code_k
                            depth += code_k.count('{') - code_k.count('}')
                            if depth <= 0 and opened:
                                skip_until = k
                                break
                        else:
                            skip_until = len(lines)
                        continue
                code = l.split('//', 1)[0]
                if not (full.search(code) or (imported and bare.search(code))):
                    continue
                if marker in l or (i > 0 and marker in lines[i - 1]):
                    continue
                hits.append(f'{rel}:{i + 1}:{l.strip()}')
if hits:
    print('check-signed-dispatch: a KV write dispatched as a bare Data frame (route it through framing::local_kv_wire_msg, '
          'or mark a receive path `// signed-dispatch: allow (<reason>)`):', file=sys.stderr)
    print('\n'.join(hits), file=sys.stderr)
    sys.exit(1)
PY
}

if [ "${1:-}" = "--self-test" ]; then
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    mkdir -p "$tmp/mycelium-core"
    cp -R src "$tmp/src"
    cp -R mycelium-core/src "$tmp/mycelium-core/src"
    check_tree "$tmp" || { echo "check-signed-dispatch --self-test: the unplanted tree is not clean" >&2; exit 1; }
    { echo 'fn __planted_conn(u: GossipUpdate) -> WireMessage { WireMessage::Data(u) }'; cat "$tmp/mycelium-core/src/connection.rs"; } > "$tmp/c.rs"
    mv "$tmp/c.rs" "$tmp/mycelium-core/src/connection.rs"
    printf 'fn __planted_agent(u: GossipUpdate) -> WireMessage { WireMessage :: Data (u) }\n' > "$tmp/src/agent/__planted_agent.rs"
    printf 'use crate::framing::WireMessage::{self, Data};\nfn __planted_use(u: GossipUpdate) -> WireMessage { Data(u) }\n' > "$tmp/src/agent/__planted_use.rs"
    printf '#[cfg(test)]\nmod early_tests;\n\nfn __planted_after_mod(u: GossipUpdate) -> WireMessage { WireMessage::Data(u) }\n' > "$tmp/src/agent/__planted_after_mod.rs"
    printf '#[cfg(test)]\nmod inline_tests {\n    const OPEN: &str = "{";\n    const C: char = %s;\n}\n\nfn __planted_after_brace(u: GossipUpdate) -> WireMessage { WireMessage::Data(u) }\n' "'{'" > "$tmp/src/agent/__planted_after_brace.rs"
    out=$(check_tree "$tmp" 2>&1) && { echo "check-signed-dispatch --self-test: the planted dispatches were NOT caught" >&2; exit 1; }
    for want in "connection.rs:1:" "__planted_agent.rs:1:" "__planted_use.rs:2:" "__planted_after_mod.rs:4:" "__planted_after_brace.rs:7:"; do
        grep -q "$want" <<<"$out" || { echo "check-signed-dispatch --self-test: $want not named" >&2; echo "$out" >&2; exit 1; }
    done
    echo "check-signed-dispatch --self-test: all five planted bare dispatches were caught, by name"
    exit 0
fi

check_tree . && echo "check-signed-dispatch: clean"
