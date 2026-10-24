#!/usr/bin/env python3
"""The feature-pages gate (docs/plans/feature-pages.md F-G, row F2).

Reads the checked-in registry, docs/features/registry.toml, and fails on:

  a. an id in any of the seven sources that the registry does not map — to exactly one feature, or to an
     exclusion with a reason — a registry id that no source has (so the registry cannot rot), a registry key
     outside the known set (a typo would drop a check), and a source that cannot say what it names: a glossary
     quick-reference row with no `<a id>`, a unit-file key with no heading (or a heading with no key), a
     companion guarantee whose id is not a string literal or whose descriptor is named or built another way (an
     import alias, a type alias, a qualified path, a struct literal);
  b. a registry entry naming a page that does not exist, two entries naming one page, a page in docs/features/
     the registry does not name, and a subdirectory of docs/features/;
  c. a page whose `##` headings are not the six sections, in order;
  d. a link in docs/features/ that does not resolve — the file, and for a Markdown target the anchor, from its
     headings (GitHub's slugs) or an `<a id>` (scripts/check-front-doors.py's resolver, shared; plan P5) — and a
     link the resolver cannot read: reference-style, titled, or `<a href>`;
  e. P1: a code item anywhere on a page — a link whose text is one backticked name and whose target is a `.rs`
     file — whose last path segment is not a `fn`, `struct`, `enum`, `trait`, `const`, `type` or `static` in that
     file, or whose CamelCase qualifier is not a type there;
  f. P9: a configuration field in a page — a backticked name linked to configuration.md — with no `| name |`
     row in configuration.md's field table;
  g. P1/P2: a test — a backticked snake_case name in *Turn it on*'s last item or anywhere in *See it work* — that
     links no `.rs` file in its paragraph, is not a `#[test]` `fn` in one, or that
     `scripts/test-coverage-exceptions.txt` excepts; a page with no example that names no test; and, with
     `--executed <dir of CI job logs>`, a named test no job of the run executed in the target its file belongs to
     (an integration file's own binary; a `src/` file's crate library) — CI's test-coverage job, reading the logs
     `scripts/ci-test-coverage.py --fetch` saved;
  h. P2: an example-index row for a feature recorded `example = "none"`, an `example` not in the index or with no
     CI status, and a page whose *See it work* does not match what the registry records;
  i. a page that does not link every id its registry entry lists;
  j. the adapter contract (adopter-surface X7): docs/reference/adapter-contract.md has a row for every adapter
     `src/adapter_contract.rs` declares, every row has one cell per property reading carried / not carried / not
     applicable, every *carried* cell names a test that exists in the file it links (a Rust `fn`, a pytest `def`, a
     jest title), and in `--executed` mode every such test executed in the run — a Rust test in its file's target, a
     pytest test in its file, a jest title in its file.

The seven sources: the concepts glossary's quick-reference rows, by their `<a id>` (docs/guide/00-concepts.md);
the guarantee and rule catalogues (docs/reference/*-catalogue.json); the unit file's keys, read from
`NodeCapabilityConfig` in src/capability_config.rs and checked against docs/reference/unit-file.md's headings;
every workspace crate's Cargo features, implicit ones included; the guarantees a companion registers in code
(`GuaranteeDescriptor::new("<id>"` outside tests; plan P3, decided in F2); and the workspace crates themselves.
What a page's prose claims is not checked here: that is the F-S `/doc-coverage` pass. The mutation suite is
scripts/test-check-feature-pages.py.
"""
from __future__ import annotations

import fnmatch
import glob
import importlib.util
import json
import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FEATURES = 'docs/features'
REGISTRY = f'{FEATURES}/registry.toml'
GLOSSARY = 'docs/guide/00-concepts.md'
UNIT_FILE = 'docs/reference/unit-file.md'
UNIT_CODE = 'src/capability_config.rs'
NOT_PAGES = {'README.md', '_template.md', 'registry.toml'}
SECTIONS = ['What it is', 'When to use it, and when not', 'Turn it on', 'See it work',
            'What it guarantees, and what it does not', 'Go deeper']
SOURCES = ('concept', 'guarantee', 'rule', 'unit', 'cargo', 'companion-guarantee', 'crate')
FEATURE_KEYS = {'name', 'page', 'planned', 'example', 'example_watch', 'ids', 'links'}
EXCLUSION_KEYS = {'reason', 'ids'}
IN_CODE = 'This feature is turned on in code'
NO_EXAMPLE = 'No runnable example exists'
EXCEPTIONS = 'scripts/test-coverage-exceptions.txt'
# The unit file's nested tables, as a path of field names from a top-level key; each step is checked in the code.
UNIT_SUBTABLES = [('activation', 'confine'), ('activation', 'confine', 'egress')]
ADAPTER_CODE = 'src/adapter_contract.rs'
ADAPTER_PAGE = 'docs/reference/adapter-contract.md'
ADAPTER_PROPERTIES = ['session persistence', 'cancellation', 'tracing', 'identity propagation', 'grants',
                      'provider-side enforcement', 'receipts']
ADAPTER_ROW = re.compile(r'^\| \*\*(.+?)\*\* \|')
CARRIED_CELL = re.compile(r'^carried — (?:`([^`]+)`|"([^"]+)") \(\[[^\]]+\]\(\.\./\.\./([^)]+)\)\)')


def _load(name, file):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(file))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


front_doors = _load('front_doors', 'check-front-doors.py')
LINK, slugs, resolve = front_doors.LINK, front_doors.slugs, front_doors.resolve


def read(root, rel):
    return (root / rel).read_text()


def strip_comments(text):
    return re.sub(r'<!--.*?-->', '', text, flags=re.S)


def feature_entries(root):
    """[(path, is_dir)] for what docs/features/ holds. A seam, so the mutation suite can add a page."""
    return [(p, p.is_dir()) for p in sorted((root / FEATURES).iterdir())]


def page_exists(root, page):
    return (root / FEATURES / page).is_file()


# ---------------------------------------------------------------------------------------------- the sources

def is_group_header(cells):
    """A group's header row, and nothing else: four cells, a bold label, the kind and demo cells empty, and the
    one-line cell empty or wholly italic *and carrying a `(#…)` link* (a pointer to the longer form, not a
    definition). A bold term with any text of its own — a definition in italics, a demo, a kind — or with the
    wrong number of cells is a term, and needs an `<a id>`."""
    return (len(cells) == 4 and re.fullmatch(r'\*\*[^*]+\*\*', cells[0]) is not None and not cells[1]
            and not cells[3] and (not cells[2] or re.fullmatch(r'\*[^*]*\(#[^)]+\)[^*]*\*', cells[2]) is not None))


def table_cells(line):
    """The cells of a Markdown table row, or None for a line that is not one: the separator, prose, a blank. A row
    is any line whose stripped text starts with `|` — `|Term|` and an indented row render as rows too, so the
    parser must read them, or a term written that way is invisible."""
    s = line.strip()
    if not s.startswith('|') or re.fullmatch(r'\|[-:| ]+\|', s):
        return None
    return [c.strip() for c in s.strip('|').split('|')]


def concept_ids(root, errors):
    """Each quick-reference row's `<a id>`; a row without one is an error (a term no registry entry can name)."""
    text = read(root, GLOSSARY)
    m = re.search(r'^## Glossary quick-reference\n(.*?)(?=^## |\Z)', text, re.M | re.S)
    if not m:
        errors.append(f'{GLOSSARY}: no "## Glossary quick-reference" table')
        return set()
    out = set()
    for line in m.group(1).splitlines():
        cells = table_cells(line)
        if cells is None:
            continue
        if cells[0] == 'Term':
            continue  # the table's header
        if is_group_header(cells):
            continue
        a = re.match(r'<a id="([^"]+)"></a>', cells[0])
        if a:
            out.add(f'concept:{a.group(1)}')
        else:
            errors.append(f'{GLOSSARY}: the glossary row "{cells[0][:50]}" has no <a id>: anchor it, then map it in {REGISTRY}')
    return out


def catalogue_ids(root, rel, key, prefix):
    return {f'{prefix}:{entry["id"]}' for entry in json.loads(read(root, rel))[key]}


def struct_fields(code, name):
    """[(serialized key, type)] for `pub struct <name>`'s fields — public, `pub(…)` and private alike, since serde
    reads them all — a serde `rename` honoured."""
    m = re.search(rf'\bpub struct {name}\s*\{{', code)
    if not m:
        return None
    body = code[m.end():code.index('\n}', m.end())]
    out, rename = [], None
    for line in body.splitlines():
        s = line.strip()
        r = re.search(r'#\[serde\([^\]]*\brename\s*=\s*"([^"]+)"', s)
        if r:
            rename = r.group(1)
        f = re.match(r'(?:pub(?:\([^)]*\))?\s+)?(\w+)\s*:\s*(?!:)(.+?),?\s*(?://.*)?$', s)
        if f:
            out.append((rename or f.group(1), f.group(2).rstrip(',')))
            rename = None
    return out


def unit_ids(root, errors):
    """The unit file's keys from the code — `NodeCapabilityConfig`'s fields (a `Vec` is `[[key]]`, an `Option` of a
    table `[key]`, a scalar `key`) and the nested tables in UNIT_SUBTABLES — each checked against a heading of
    unit-file.md, and each such heading against the code."""
    code = read(root, UNIT_CODE)
    top = struct_fields(code, 'NodeCapabilityConfig')
    if not top:
        errors.append(f'{UNIT_CODE}: no `pub struct NodeCapabilityConfig` with fields (the parser or the code changed)')
        return set()
    keys = set()
    types = {}
    for key, ty in top:
        inner = re.match(r'(?:Vec|Option)<(\w+)>', ty)
        if ty.startswith('Vec<'):
            keys.add(f'[[{key}]]')
        elif inner and inner.group(1) not in ('String', 'bool', 'u64', 'u32', 'usize'):
            keys.add(f'[{key}]')
        else:
            keys.add(key)
        types[key] = inner.group(1) if inner else ty
    for path in UNIT_SUBTABLES:
        ty = types.get(path[0])
        for step in path[1:]:
            fields = dict(struct_fields(code, ty) or [])
            inner = re.match(r'(?:Vec|Option)<(\w+)>', fields.get(step, ''))
            if not inner:
                errors.append(f'{UNIT_CODE}: the unit-file table [{".".join(path)}] is not in the code (no `{step}` in {ty})')
                ty = None
                break
            ty = inner.group(1)
        if ty:
            keys.add(f'[{".".join(path)}]')
    headings, fenced = set(), False
    for line in read(root, UNIT_FILE).splitlines():
        if line.lstrip().startswith('```'):
            fenced = not fenced
            continue
        m = None if fenced else re.match(r'^#{2,3} `([^`]+)`\s*$', line)
        if m:
            headings.add(m.group(1))
    for key in sorted(keys - headings):
        errors.append(f'{UNIT_FILE}: the unit-file key `{key}` ({UNIT_CODE}) has no section heading')
    for key in sorted(headings - keys):
        errors.append(f'{UNIT_FILE}: the section `{key}` names no key of NodeCapabilityConfig ({UNIT_CODE})')
    return {f'unit:{k}' for k in keys}


def workspace_crates(root):
    """[(package name, member dir, manifest)] for the root crate and every workspace member."""
    top = tomllib.loads(read(root, 'Cargo.toml'))
    out = []
    members = ['.']
    for member in top.get('workspace', {}).get('members', []):
        # cargo expands a glob member; so does the gate, or a `crates/*` entry is a traceback instead of a source
        members += sorted(p.parent.relative_to(root).as_posix() for p in root.glob(f'{member}/Cargo.toml')) \
            if any(ch in member for ch in '*?[') else [member]
    for member in members:
        manifest = tomllib.loads(read(root, f'{member}/Cargo.toml' if member != '.' else 'Cargo.toml'))
        out.append((manifest['package']['name'], member, manifest))
    return out


def optional_deps(manifest):
    tables = [manifest.get(k, {}) for k in ('dependencies', 'build-dependencies')]
    tables += [t.get(k, {}) for t in manifest.get('target', {}).values() for k in ('dependencies', 'build-dependencies')]
    return {name for table in tables for name, spec in table.items() if isinstance(spec, dict) and spec.get('optional')}


def cargo_ids(root):
    """Each crate's declared features, and the implicit feature cargo makes of an optional dependency that no
    feature names as `dep:<name>`."""
    out = set()
    for name, _, manifest in workspace_crates(root):
        features = manifest.get('features', {})
        named = {v[4:] for values in features.values() for v in values if v.startswith('dep:')}
        out |= {f'cargo:{name}/{f}' for f in features}
        out |= {f'cargo:{name}/{d}' for d in optional_deps(manifest) - named}
    return out


def crate_ids(root):
    return {f'crate:{name}' for name, _, _ in workspace_crates(root)}


RUST_TOKEN = re.compile(r'//[^\n]*|/\*.*?\*/|r(#*)".*?"\1|b?"(?:\\.|[^"\\])*"|b?\'(?:\\.|[^\'\\])\'|[{}]', re.S)
TEST_MODULE = re.compile(r'#\[cfg\((?:all\()?test\b[^\]]*\]\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{')


COMMENT = re.compile(r'//[^\n]*|/\*.*?\*/|r(#*)".*?"\1|b?"(?:\\.|[^"\\])*"|b?\'(?:\\.|[^\'\\])\'', re.S)


def without_comments(text):
    """The text with every comment blanked (newlines kept, so line numbers hold) and string literals left alone,
    so a doc comment that mentions `GuaranteeDescriptor::new("…"` is neither a registration nor a refusal. Char
    literals are tokens too, or a `'"'` opens a pseudo-string and a later `/*` inside a string blanks real code."""
    return COMMENT.sub(lambda m: re.sub(r'[^\n]', ' ', m.group(0)) if m.group(0).startswith('/') else m.group(0), text)


def without_test_modules(text):
    """The text with every `#[cfg(test)]` module's body removed, by brace matching that skips comments and string
    and char literals (so a `"{"` cannot swallow the rest of a file)."""
    out, pos = [], 0
    for m in TEST_MODULE.finditer(text):
        if m.start() < pos:
            continue
        out.append(text[pos:m.start()])
        depth, end = 1, len(text)
        for t in RUST_TOKEN.finditer(text, m.end()):
            if t.group(0) in '{}':
                depth += 1 if t.group(0) == '{' else -1
                if not depth:
                    end = t.end()
                    break
        pos = end
    out.append(text[pos:])
    return ''.join(out)


DESCRIPTOR_CALL = re.compile(r'\bGuaranteeDescriptor\s*::\s*new\s*\(\s*')
DESCRIPTOR_ALIAS = re.compile(r'\bGuaranteeDescriptor\s+as\s+(\w+)')
# Every other way to name or build a descriptor, which the literal scan above would not read: a type alias, a
# qualified path (`<GuaranteeDescriptor>::new`, `<… as …>::`), a struct literal (only possible inside `mycelium`,
# where `applies` and `resolve` are `pub(crate)`; refused anyway), an `impl … for GuaranteeDescriptor` block (a
# local trait on the foreign type, inside which `Self::new("<id>"` names no descriptor), an inherent `impl` (only
# `mycelium` can write one: a core guarantee registered outside `core_guarantees()`), and `new` named as a
# function item without its call (`let f = GuaranteeDescriptor::new; f("<id>", …)`). The defining file is exempt
# from the struct-literal and impl checks: its literals are `new` itself and the core catalogue's `g(…)`, and its
# `impl Debug for` is the type's own.
DESCRIPTOR_ESCAPES = [
    (re.compile(r'\btype\s+(\w+)[^=;]*=[^;]*\bGuaranteeDescriptor\b'), 'a type alias for GuaranteeDescriptor'),
    (re.compile(r'(?<![\w>])<\s*(?:\w+::)*GuaranteeDescriptor\b[^<>;{}()]*>\s*::'),
     'a qualified path (<GuaranteeDescriptor>::…)'),
    (re.compile(r'(?<![\w:])(?:\w+::)*GuaranteeDescriptor\s*\{'), 'a GuaranteeDescriptor struct literal'),
    (re.compile(r'\bimpl\b[^{;]*?\bfor\s+(?:\w+::)*GuaranteeDescriptor\b'), 'an impl block on GuaranteeDescriptor'),
    (re.compile(r'\bimpl(?:\s*<[^>]*>)?\s+(?:\w+::)*GuaranteeDescriptor\b[^{;]*\{'), 'an inherent impl on GuaranteeDescriptor'),
    (re.compile(r'\bGuaranteeDescriptor\s*::\s*new\b(?!\s*\()'), 'GuaranteeDescriptor::new named without a call'),
]
HOME_EXEMPT = ('literal', 'impl')  # the type's own `impl` and `impl Debug for`
NOT_A_LITERAL = re.compile(r'(?:->|\bstruct|\bimpl(?:<[^>]*>)?|\bfor)\s*$')  # a return type, the definition, an impl
DESCRIPTOR_HOME = 'src/agent/guarantee.rs'


def companion_guarantee_ids(root, errors):
    """Every guarantee a crate registers in code under `src/`, outside comments, test modules and test files (an
    `examples/`, `benches/` or `build.rs` descriptor is not a shipped surface; plan §4.2). The core's own are built
    by `g(…)` in src/agent/guarantee.rs and are the catalogue; this finds the rest, whatever crate. A registration
    the scan cannot read — an id that is not a string literal, a descriptor imported under another name, a type
    alias, a qualified-path call, a struct literal, an `impl … for GuaranteeDescriptor` block or `new` named without
    its call — is refused, so a guarantee cannot be invisible to the registry."""
    out = set()
    for _, member, _ in workspace_crates(root):
        for path in sorted((root / member).glob('src/**/*.rs')):
            rel = path.relative_to(root).as_posix()
            if rel.endswith('_tests.rs') or '/tests/' in rel:
                continue
            text = without_test_modules(without_comments(path.read_text()))
            for alias in DESCRIPTOR_ALIAS.findall(text):
                errors.append(f'{rel}: imports GuaranteeDescriptor as {alias}: the registry reads '
                              f'`GuaranteeDescriptor::new("<id>"`; use the name')
            for pattern, what in DESCRIPTOR_ESCAPES:
                if rel == DESCRIPTOR_HOME and any(w in what for w in HOME_EXEMPT):
                    continue
                for m in pattern.finditer(text):
                    if 'literal' in what and NOT_A_LITERAL.search(text[max(0, m.start() - 80):m.start()]):
                        continue
                    line = text.count('\n', 0, m.start()) + 1
                    errors.append(f'{rel}: {what} near line {line} of the scanned text: the registry reads '
                                  f'`GuaranteeDescriptor::new("<id>"`; build the descriptor that way')
            for m in DESCRIPTOR_CALL.finditer(text):
                lit = re.match(r'"([^"\\]+)"\s*,', text[m.end():])
                if lit:
                    out.add(f'companion-guarantee:{lit.group(1)}')
                else:
                    line = text.count('\n', 0, m.start()) + 1
                    errors.append(f'{rel}: a GuaranteeDescriptor::new near line {line} of the scanned text takes an '
                                  f'id that is not a string literal: the registry cannot read it')
    return out


def source_ids(root, errors):
    ids = {
        'concept': concept_ids(root, errors),
        'guarantee': catalogue_ids(root, 'docs/reference/guarantee-catalogue.json', 'guarantees', 'guarantee'),
        'rule': catalogue_ids(root, 'docs/reference/rule-catalogue.json', 'rules', 'rule'),
        'unit': unit_ids(root, errors),
        'cargo': cargo_ids(root),
        'companion-guarantee': companion_guarantee_ids(root, errors),
        'crate': crate_ids(root),
    }
    # A core guarantee is in the catalogue; one registered in code under a catalogue id is the core's own.
    core = {i.split(':', 1)[1] for i in ids['guarantee']}
    ids['companion-guarantee'] = {i for i in ids['companion-guarantee'] if i.split(':', 1)[1] not in core}
    return ids


# --------------------------------------------------------------------------------------------- the registry

def registry(root):
    return tomllib.loads(read(root, REGISTRY))


def check_shape(reg, errors):
    """Refuse a key outside the known set, and a value of the wrong type: a typo must not drop a check."""
    for key in sorted(set(reg) - {'feature', 'exclusion'}):
        errors.append(f'{REGISTRY}: unknown top-level key `{key}`')
    for kind, allowed in (('feature', FEATURE_KEYS), ('exclusion', EXCLUSION_KEYS)):
        for entry in reg.get(kind, []):
            label = entry.get('name') or str(entry.get('reason', ''))[:50]
            for key in sorted(set(entry) - allowed):
                errors.append(f'{REGISTRY}: {kind} "{label}" has an unknown key `{key}` (known: {", ".join(sorted(allowed))})')
            for key in ('ids', 'links', 'example_watch'):
                value = entry.get(key, [])
                if not isinstance(value, list) or not all(isinstance(v, str) for v in value):
                    errors.append(f'{REGISTRY}: {kind} "{label}": `{key}` must be a list of strings')
            for key in ('name', 'page', 'planned', 'example', 'reason'):
                if key in entry and not isinstance(entry[key], str):
                    errors.append(f'{REGISTRY}: {kind} "{label}": `{key}` must be a string')


def real_pages(reg):
    return [f for f in reg.get('feature', []) if f.get('page') not in (None, 'todo')]


def check_registry(root, reg, ids, errors):
    """(a) and (b): every source id mapped exactly once, every mapped id real, every page real and named once."""
    owner = {}
    for kind, entries in (('feature', reg.get('feature', [])), ('exclusion', reg.get('exclusion', []))):
        for entry in entries:
            label = entry.get('name') or entry.get('reason', '')[:50]
            if kind == 'exclusion' and not entry.get('reason', '').strip():
                errors.append(f'{REGISTRY}: an exclusion has no reason: {entry.get("ids")}')
            if kind == 'feature' and not entry.get('page'):
                errors.append(f'{REGISTRY}: feature "{label}" names no page (a page, or "todo")')
            if kind == 'feature' and entry.get('page') == 'todo' and not entry.get('planned'):
                errors.append(f'{REGISTRY}: feature "{label}" is "todo" but names no `planned` page')
            for i in entry.get('ids', []):
                if i in owner:
                    errors.append(f'{REGISTRY}: {i} is mapped twice ({owner[i]}; {kind} "{label}")')
                owner[i] = f'{kind} "{label}"'
    taken = {Path(f['page']).stem: f['name'] for f in real_pages(reg)}
    for f in reg.get('feature', []):
        planned = f.get('planned', '')
        if f.get('page') == 'todo' and Path(planned).stem in taken:
            errors.append(f'{REGISTRY}: feature "{f.get("name")}" names `planned` page "{planned}", which is already '
                          f'the page of "{taken[Path(planned).stem]}": one feature per page')
    known = set().union(*ids.values())
    for source, found in ids.items():
        for i in sorted(found - owner.keys()):
            errors.append(f'{REGISTRY}: {i} ({source}) is not mapped: map it to a feature page, "todo", or an exclusion with a reason')
    for i in sorted(owner.keys() - known):
        errors.append(f'{REGISTRY}: {i} is mapped but no source has it (renamed or removed?)')
    for f in reg.get('feature', []):
        for i in f.get('links', []):
            if i not in known:
                errors.append(f'{REGISTRY}: feature "{f.get("name")}" links {i}, which no source has')
    named = {}
    for f in real_pages(reg):
        if f['page'] in named:
            errors.append(f'{REGISTRY}: the page {f["page"]} is named by two features ("{named[f["page"]]}" and '
                          f'"{f.get("name")}"): one page, one feature')
        named[f['page']] = f.get('name')
        if not page_exists(root, f['page']):
            errors.append(f'{REGISTRY}: page {f["page"]} does not exist in {FEATURES}/')
    for path, is_dir in feature_entries(root):
        if is_dir:
            errors.append(f'{FEATURES}/{path.name}/ is a subdirectory: feature pages live in {FEATURES}/ itself')
        elif path.suffix == '.md' and path.name not in NOT_PAGES and path.name not in named:
            errors.append(f'{FEATURES}/{path.name} is not in the registry: add a [[feature]] naming it')
    return known


# ------------------------------------------------------------------------------------------------ the pages

def sections(text):
    """[(heading, body)] for each `## ` heading outside code fences."""
    out, fenced, current = [], False, None
    for line in text.splitlines():
        if line.lstrip().startswith('```'):
            fenced = not fenced
        m = None if fenced else re.match(r'^## (.+?)\s*$', line)
        if m:
            current = [m.group(1), []]
            out.append(current)
        elif current is not None:
            current[1].append(line)
    return [(h, '\n'.join(b)) for h, b in out]


def blocks(body):
    """Paragraphs and list items: what one link's neighbourhood is."""
    out, cur = [], []
    for line in body.splitlines():
        if not line.strip() or re.match(r'^\s*[-*] |^\s*\d+\. ', line):
            if cur:
                out.append('\n'.join(cur))
            cur = [line] if line.strip() else []
        else:
            cur.append(line)
    if cur:
        out.append('\n'.join(cur))
    return out


def defines(text, name, kinds=('fn', 'struct', 'enum', 'trait', 'const', 'type', 'static')):
    return re.search(r'(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:(?:async|unsafe|const|extern(?:\s+"[^"]*")?)\s+)*'
                     rf'(?:{"|".join(kinds)})\s+{re.escape(name)}\b', text) is not None


TEST_ATTR = re.compile(r'#\[(?:[\w:]+::)?test\b|#\[tokio::test|#\[rstest')


def defines_test(text, name):
    m = re.search(rf'(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+{re.escape(name)}\s*[(<]', text)
    if not m:
        return None
    # The attributes and doc comments directly above the fn, and nothing further up.
    for line in reversed(text[:m.start()].splitlines()):
        s = line.strip()
        if s.startswith('#[') and TEST_ATTR.search(s):
            return True
        if not s.startswith(('#[', '//', ')]')) and not s.endswith((',', '(')):
            return False
    return False


TEST_NAME = re.compile(r'(?<![\[\w])`((?:[a-z_][a-z0-9_]*::)*[a-z][a-z0-9]*(?:_[a-z0-9]+)+)`(?!\]\()')


def named_tests(body, last_only=False):
    """(test path, [.rs targets in the same block]) for each test a section names: a backticked snake_case name
    (module path allowed), not itself a link's text. In *Turn it on* only the last paragraph or item counts, and
    only one that links a `.rs` file (P1: the section ends with the test); in *See it work* every such name is a
    test, and one in a paragraph that links no `.rs` file is reported with no file."""
    out = []
    candidates = blocks(body)
    for block in candidates[-1:] if last_only else candidates:
        files = [m.group(2) for m in LINK.finditer(block) if m.group(2).partition('#')[0].endswith('.rs')]
        if last_only and not files:
            continue
        for m in TEST_NAME.finditer(LINK.sub(lambda x: ' ' * len(x.group(0)), block)):
            out.append((m.group(1), files))
    return out


def exception_patterns(root):
    out = []
    for line in read(root, EXCEPTIONS).splitlines():
        line = line.strip()
        if line and not line.startswith('#'):
            out.append(line.partition(' — ')[0].strip())
    return out


def excepted(name, patterns):
    """True when an exception's last key segment matches the test's name."""
    last = name.split('::')[-1]
    return [p for p in patterns if fnmatch.fnmatchcase(last, p.split('::')[-1].split(' - ')[0].strip())]


def config_fields(root):
    table = read(root, 'docs/reference/configuration.md')
    m = re.search(r'^## The fields\n(.*?)(?=^## |\Z)', table, re.M | re.S)
    return set(re.findall(r'(?m)^\| ([A-Za-z0-9_.\[\]]+) \|', m.group(1) if m else '')) - {'field'}


def example_rows(root):
    """{example name: row} for the example index's capability matrix (first cell's backticked link)."""
    text = read(root, 'examples/README.md')
    m = re.search(r'^## The capability matrix\n(.*?)(?=^## |\Z)', text, re.M | re.S)
    rows = {}
    for line in (m.group(1) if m else '').splitlines():
        cells = table_cells(line)
        if cells:
            name = re.match(r'^\[`([^`]+)`\]', cells[0])
            if name:
                rows[name.group(1)] = line
    return rows


def links_id(root, text, i):
    """(i): does the page link this id, in the form its source takes?"""
    source, _, name = i.partition(':')
    targets = [(m.group(1), m.group(2)) for m in LINK.finditer(text)]
    if source == 'concept':
        return any(u.partition('#')[0].endswith('guide/00-concepts.md') and u.partition('#')[2] == name for _, u in targets)
    if source in ('guarantee', 'rule'):
        cat = f'reference/{source}-catalogue.md'
        return any(u.partition('#')[0].endswith(cat) and f'`{name}`' in t for t, u in targets)
    if source == 'unit':
        want = re.sub(r'[^\w\- ]', '', name.lower()).replace(' ', '-')
        return any(u.partition('#')[0].endswith('reference/unit-file.md') and u.partition('#')[2] == want for _, u in targets)
    if source == 'cargo':
        crate, feature = name.split('/', 1)
        return f'`{feature}`' in text and (crate == 'mycelium' or crate in text)
    if source == 'companion-guarantee':
        return f'`{name}`' in text
    if source == 'crate':
        member = next((m for n, m, _ in workspace_crates(root) if n == name), name)
        return f'`{name}`' in text or any(f'{member}/' in u for _, u in targets)
    return False


def file_of(root, rel, url):
    """The repository-relative path a page's link names, or None outside the repository."""
    try:
        return ((root / rel).parent / url.partition('#')[0]).resolve().relative_to(root.resolve()).as_posix()
    except ValueError:
        return None


def check_page(root, feature, rows, fields, patterns, errors, tests_named):
    page = feature['page']
    rel = f'{FEATURES}/{page}'
    text = strip_comments(read(root, rel))
    secs = sections(text)
    heads = [h for h, _ in secs]
    if heads != SECTIONS:
        missing = [s for s in SECTIONS if s not in heads]
        errors.append(f'{rel}: the sections are {heads}; expected the six, in order'
                      + (f' (missing: {missing})' if missing else ''))
    body = dict(secs)

    # (e) code items, anywhere on the page: the last segment, and a CamelCase qualifier, defined in the linked file
    turn = body.get('Turn it on', '')
    for m in LINK.finditer(text):
        label, url = m.group(1), m.group(2)
        code = re.fullmatch(r'`([A-Za-z_][\w:]*)(?:\(\))?`', label)
        path = url.partition('#')[0]
        if not code or not path.endswith('.rs'):
            continue
        target = (root / rel).parent / path
        if not target.exists():
            continue
        source = target.read_text()
        segments = code.group(1).split('::')
        name = segments[-1]
        if not defines(source, name):
            errors.append(f'{rel}: names `{code.group(1)}`, but no fn, struct, enum, trait, const, type or static '
                          f'{name} is in {path}')
        for qualifier in segments[:-1]:
            if qualifier[:1].isupper() and not defines(source, qualifier, ('struct', 'enum', 'trait', 'type')):
                errors.append(f'{rel}: names `{code.group(1)}`, but no struct, enum, trait or type {qualifier} is in {path}')
    if turn.lstrip().startswith(IN_CODE):
        if not any(m.group(2).partition('#')[0].endswith('.rs') for m in LINK.finditer(turn)):
            errors.append(f'{rel}: Turn it on says "{IN_CODE}" but links no code item')
        if not named_tests(turn, last_only=True):
            errors.append(f'{rel}: Turn it on says "{IN_CODE}" but names no test that runs that code')

    # (g) tests
    for section in ('Turn it on', 'See it work'):
        for test, files in named_tests(body.get(section, ''), last_only=section == 'Turn it on'):
            name = test.split('::')[-1]
            if not files:
                errors.append(f'{rel}: {section} names `{test}`, read as a test, but its paragraph links no .rs file: '
                              f'link the file that defines it (or, if it is not a test, move it out of this section)')
                continue
            found = [f for f in files if (root / rel).parent.joinpath(f.partition('#')[0]).exists()
                     and defines_test(((root / rel).parent / f.partition('#')[0]).read_text(), name) is not None]
            if not found:
                errors.append(f'{rel}: {section} names the test `{test}`, but no fn {name} is in a file the same '
                              f'paragraph links ({", ".join(files)})')
                continue
            tests = [f for f in found if defines_test(((root / rel).parent / f.partition('#')[0]).read_text(), name)]
            if not tests:
                errors.append(f'{rel}: {section} names `{test}` as a test, but fn {name} has no #[test] attribute')
            hit = excepted(test, patterns)
            if hit:
                errors.append(f'{rel}: {section} names the test `{test}`, which {EXCEPTIONS} excepts ({hit[0]}): '
                              f'a test CI never runs cannot show the feature')
            tests_named.append((rel, test, [file_of(root, rel, f) for f in tests or found]))

    # (f) configuration fields, anywhere on the page
    for m in LINK.finditer(text):
        label, url = m.group(1), m.group(2)
        field = re.fullmatch(r'`([A-Za-z0-9_.]+)`', label)
        if field and url.partition('#')[0].endswith('reference/configuration.md') and field.group(1) not in fields:
            errors.append(f'{rel}: names the configuration field `{field.group(1)}`, which has no row in '
                          f'configuration.md § The fields')

    # (h) the example
    see = body.get('See it work', '').lstrip()
    example = feature.get('example')
    if example is None:
        errors.append(f'{REGISTRY}: feature "{feature["name"]}" has a page but records no example (a name in the '
                      f'example index, or "none")')
    elif example == 'none':
        if not see.startswith(NO_EXAMPLE):
            errors.append(f'{rel}: the registry records example = "none", but See it work does not open with '
                          f'"{NO_EXAMPLE}"')
        if not named_tests(see):
            errors.append(f'{rel}: See it work has no example and names no test that runs the feature end to end')
    else:
        if example not in rows:
            errors.append(f'{REGISTRY}: feature "{feature["name"]}" records the example `{example}`, which is not a '
                          f'row of the example index\'s capability matrix')
        elif rows[example].rstrip(' |').split('|')[-2].strip() not in ('✓', '✓ᵖ', '·'):
            errors.append(f'examples/README.md: the row `{example}` has no CI status (✓, ✓ᵖ or ·)')
        if not any(m.group(2).partition('#')[0].endswith('examples/README.md') for m in LINK.finditer(see)):
            errors.append(f'{rel}: See it work does not link the example index')

    # (i) every id the registry gives this page
    for i in feature.get('ids', []) + feature.get('links', []):
        if not links_id(root, text, i):
            errors.append(f'{rel}: does not link {i}, which the registry gives it')


def watched(feature):
    """The example names that would be a row for this feature: the page's name in snake case, and any
    `example_watch` the registry lists."""
    stem = Path(feature['page']).stem.replace('-', '_') if feature.get('page') not in (None, 'todo') else feature.get('planned', '').replace('-', '_')
    return [w for w in [stem] + feature.get('example_watch', []) if w]


def check_examples(reg, rows, errors):
    """(h): the example index gains no row for a feature recorded `example = "none"`."""
    for feature in reg.get('feature', []):
        if feature.get('example') != 'none':
            continue
        page = feature.get('page')
        for name, row in rows.items():
            hit = [w for w in watched(feature) if w in name]
            if hit or (page not in (None, 'todo') and f'features/{page}' in row):
                errors.append(f'examples/README.md: the capability matrix has a row `{name}` for '
                              f'"{feature["name"]}", which the registry records as example = "none": '
                              f'record the example and link it from the page')


UNREADABLE_LINKS = [
    (re.compile(r'\]\([^)\s]+\s+"[^"]*"\)'), 'a titled link'),
    (re.compile(r'(?<!\])\[[^\]]+\]\[[^\]]*\]'), 'a reference-style link'),
    (re.compile(r'(?m)^\s*\[[^\]]+\]:\s*\S+'), 'a reference definition'),
    (re.compile(r'<a\s[^>]*\bhref=', re.I), 'an <a href>'),
]


def check_links(root, errors):
    """(d): every link in docs/features/ resolves, file and anchor; a link the resolver cannot read is refused."""
    for path, is_dir in feature_entries(root):
        if is_dir or path.suffix != '.md':
            continue
        rel = path.relative_to(root).as_posix()
        text = re.sub(r'`[^`\n]*`', '``', re.sub(r'```.*?```', '', strip_comments(read(root, rel)), flags=re.S))
        for pattern, what in UNREADABLE_LINKS:
            for m in pattern.finditer(text):
                errors.append(f'{rel}: {what} ({m.group(0).strip()[:60]}): write an inline link, [text](target)')
        for m in LINK.finditer(text):  # a link written inside a code span is not a link
            url = m.group(2)
            if re.match(r'^[a-z]+:', url):
                continue
            error = resolve(root, rel, url)[1]
            if error:
                errors.append(error)


def pages_checked(root, reg, errors):
    rows, fields, patterns = example_rows(root), config_fields(root), exception_patterns(root)
    if not rows:
        errors.append('examples/README.md: no capability matrix rows found (the parser or the index changed)')
    if not fields:
        errors.append('docs/reference/configuration.md: no field rows found under "## The fields"')
    tests = []
    for feature in real_pages(reg):
        if page_exists(root, feature['page']):
            check_page(root, feature, rows, fields, patterns, errors, tests)
    check_adapter_contract(root, errors, tests)
    return rows, tests


def check(root):
    errors = []
    try:
        reg = registry(root)
    except (OSError, tomllib.TOMLDecodeError) as e:
        return [f'{REGISTRY}: cannot be read: {e}']
    check_shape(reg, errors)
    ids = source_ids(root, errors)
    check_registry(root, reg, ids, errors)
    check_links(root, errors)
    rows, _ = pages_checked(root, reg, errors)
    check_examples(reg, rows, errors)
    return errors


def adapter_names(root):
    """The adapters `src/adapter_contract.rs` declares, by `name:` inside `pub const ADAPTERS`."""
    code = read(root, ADAPTER_CODE)
    m = re.search(r'pub const ADAPTERS: &\[Adapter\] = &\[(.*?)\n\];', code, re.S)
    return re.findall(r'\bname: "([^"]+)"', m.group(1)) if m else []


def adapter_rows(root):
    """{adapter: [cell text per property]} from the contract page's table."""
    rows = {}
    for line in read(root, ADAPTER_PAGE).splitlines():
        m = ADAPTER_ROW.match(line)
        if not m:
            continue
        cells = [c.strip() for c in line.strip().strip('|').split(' | ')]
        rows[m.group(1)] = cells[2:]
    return rows


def test_is_in(root, test, file):
    text = read(root, file)
    if file.endswith('.rs'):
        return f'fn {test}(' in text
    if file.endswith('.py'):
        return f'def {test}(' in text
    return f'"{test}"' in text


def check_adapter_contract(root, errors, tests):
    """(j): the contract page against the code; the carried cells' tests go to `tests` for the executed mode."""
    names = adapter_names(root)
    if not names:
        errors.append(f'{ADAPTER_CODE}: no `pub const ADAPTERS` with `name:` fields (the parser or the code changed)')
        return
    try:
        rows = adapter_rows(root)
    except FileNotFoundError:
        errors.append(f'{ADAPTER_PAGE}: missing (regenerate with UPDATE_ADAPTER_CONTRACT=1 cargo test --lib adapter_contract)')
        return
    for name in names:
        if name not in rows:
            errors.append(f'{ADAPTER_PAGE}: no row for adapter `{name}` (declared in {ADAPTER_CODE})')
    for name, cells in rows.items():
        if name not in names:
            errors.append(f'{ADAPTER_PAGE}: row `{name}` names no adapter {ADAPTER_CODE} declares')
        if len(cells) != len(ADAPTER_PROPERTIES):
            errors.append(f'{ADAPTER_PAGE}: row `{name}` has {len(cells)} property cells, not {len(ADAPTER_PROPERTIES)}')
        for prop, cell in zip(ADAPTER_PROPERTIES, cells):
            if cell.startswith('carried'):
                m = CARRIED_CELL.match(cell)
                if not m:
                    errors.append(f'{ADAPTER_PAGE}: `{name}` / {prop}: a carried cell must name its test and link its file')
                    continue
                test, file = m.group(1) or m.group(2), m.group(3)
                if not (root / file).is_file():
                    errors.append(f'{ADAPTER_PAGE}: `{name}` / {prop}: names the test `{test}` in {file}, which does not exist')
                elif not test_is_in(root, test, file):
                    errors.append(f'{ADAPTER_PAGE}: `{name}` / {prop}: names the test `{test}`, which is not in {file}')
                else:
                    tests.append((ADAPTER_PAGE, test, [file]))
            elif not cell.startswith(('not carried', 'not applicable')):
                errors.append(f'{ADAPTER_PAGE}: `{name}` / {prop}: a cell reads carried, not carried or not applicable, not {cell[:30]!r}')


def named_test_list(root):
    return pages_checked(root, registry(root), [])[1]


def crate_of(root, rel):
    """(crate's Rust name, path within the crate) for a repository file: the nearest directory with a Cargo.toml."""
    path = Path(rel)
    for parent in path.parents:
        manifest = root / parent / 'Cargo.toml'
        if manifest.is_file():
            name = tomllib.loads(manifest.read_text()).get('package', {}).get('name')
            if name:
                return name.replace('-', '_'), path.relative_to(parent).as_posix()
    return None, rel


def executed_in(key, test, crate, within):
    """Did this executed key run `test` in the target its file belongs to? An integration file `tests/<stem>.rs` is
    its own binary `<stem>`; a `src/` file is the crate's library or binaries, under the crate's name. A module
    path is not derived from the file: `#[path]` attributes make it differ (fleet_history_tests.rs runs as
    `fleet_history::tests`)."""
    if key.startswith('python '):
        # `python <file>::<test>` or `::Class::test`, with a `[param]` suffix; the file is repository-relative once
        # canonicalised, and `within` is the full path for a file under no Cargo.toml of its own.
        path, _, rest = key[7:].partition('::')
        leaf = rest.split('::')[-1].split('[', 1)[0]
        return path == within and leaf == test
    if key.startswith('typescript '):
        file, _, title = key[11:].partition('::')
        return title.strip() == test and within.endswith(file)
    if not key.startswith('rust '):
        return False
    parts = key[5:].split('::', 2)
    if len(parts) < 3:
        return False
    target, binary, rest = parts
    if not (rest == test or rest.endswith(f'::{test}')):
        return False
    if within.startswith('tests/'):
        stem = Path(within).stem
        return target == f'tests/{stem}.rs' and binary == stem
    if within.startswith('src/'):
        return target.startswith('src/') and binary == crate
    return True


def check_executed(root, logs):
    """(g), CI half: every named test executed in this run, in its file's target."""
    coverage = _load('ci_test_coverage', 'ci-test-coverage.py')
    executed, universe = set(), set()
    for f in sorted(glob.glob(str(Path(logs) / '*'))):
        coverage.scan(Path(f).read_text(encoding='utf-8', errors='replace'), executed, universe)
    if not executed:
        return [f'{logs}: no executed test in the logs (the fetch failed, or the log format changed)']
    errors = []
    for rel, test, files in named_test_list(root):
        places = [crate_of(root, f) for f in files if f]
        if not any(executed_in(key, test, crate, within) for key in executed for crate, within in places):
            errors.append(f'{rel}: names the test `{test}` (in {", ".join(f for f in files if f)}), which no job of this '
                          f'run executed in that file\'s target')
    return errors


def counts(root):
    reg = registry(root)
    out = {s: {'page': 0, 'todo': 0, 'excluded': 0} for s in SOURCES}
    for f in reg.get('feature', []):
        for i in f.get('ids', []):
            out[i.split(':', 1)[0]]['todo' if f.get('page') == 'todo' else 'page'] += 1
    for e in reg.get('exclusion', []):
        for i in e.get('ids', []):
            out[i.split(':', 1)[0]]['excluded'] += 1
    return out


if __name__ == '__main__':
    args = sys.argv[1:]
    if args[:1] == ['--executed'] and len(args) == 2:
        errors = check_executed(ROOT, args[1])
    elif not args:
        errors = check(ROOT)
    else:
        sys.exit('usage: check-feature-pages.py [--executed <dir of CI job logs>]')
    for error in errors:
        print(f'feature-pages: {error}', file=sys.stderr)
    if not args and not errors:
        c = counts(ROOT)
        todo = sum(v['todo'] for v in c.values())
        print('check-feature-pages: ' + '; '.join(f'{s} {v["page"]} paged, {v["todo"]} todo, {v["excluded"]} excluded'
                                                  for s, v in c.items()) + f' ({todo} ids await a page: row F3)')
    print(f'check-feature-pages: {len(errors)} failures')
    sys.exit(bool(errors))
