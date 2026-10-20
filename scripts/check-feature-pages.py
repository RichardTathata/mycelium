#!/usr/bin/env python3
"""The feature-pages gate (docs/plans/feature-pages.md F-G, row F2).

Reads the checked-in registry, docs/features/registry.toml, and fails on:

  a. an id in any of the six sources that the registry does not map — to exactly one feature, or to an
     exclusion with a reason — and on a registry id that no source has (so the registry cannot rot);
  b. a registry entry naming a page that does not exist, and a page in docs/features/ the registry does not
     name;
  c. a page whose `##` headings are not the six sections, in order;
  d. a link in docs/features/ that does not resolve: the file, and for a Markdown target the anchor, from its
     headings (GitHub's slugs) or an `<a id>` (scripts/check-front-doors.py's resolver, shared; plan P5);
  e. P1: a code item in *Turn it on* — a link whose text is one backticked name and whose target is a `.rs`
     file — whose last path segment is not a `fn`, `struct`, `enum`, `trait` or `const` in that file;
  f. P9: a configuration field in a page — a backticked name linked to configuration.md — with no `| name |`
     row in configuration.md's field table;
  g. P1/P2: a test named in *Turn it on* or *See it work* that is not a `#[test]` `fn` in a `.rs` file the
     same paragraph or list item links, or that `scripts/test-coverage-exceptions.txt` excepts (pre-push);
     with `--executed <dir of CI job logs>`, also one that no job of the run executed (CI's test-coverage
     job, reading the logs `scripts/ci-test-coverage.py --fetch` saved);
  h. P2: an example-index row for a feature recorded `example = "none"`, an `example` not in the index, and a
     page whose *See it work* does not match what the registry records;
  i. a page that does not link every id its registry entry lists.

The six sources: the concepts glossary's `<a id>` anchors (docs/guide/00-concepts.md), the guarantee and rule
catalogues (docs/reference/*-catalogue.json), the unit-file sections (docs/reference/unit-file.md), every
workspace crate's Cargo `[features]`, and the guarantees a companion registers in code
(`GuaranteeDescriptor::new("<id>"` outside tests; plan P3, decided in F2). The mutation suite is
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
NOT_PAGES = {'README.md', '_template.md'}
SECTIONS = ['What it is', 'When to use it, and when not', 'Turn it on', 'See it work',
            'What it guarantees, and what it does not', 'Go deeper']
SOURCES = ('concept', 'guarantee', 'rule', 'unit', 'cargo', 'companion-guarantee')
IN_CODE = 'This feature is turned on in code'
NO_EXAMPLE = 'No runnable example exists'
EXCEPTIONS = 'scripts/test-coverage-exceptions.txt'


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


# ---------------------------------------------------------------------------------------------- the sources

def concept_ids(root):
    text = re.sub(r'```.*?```', '', read(root, 'docs/guide/00-concepts.md'), flags=re.S)
    return {f'concept:{a}' for a in re.findall(r'<a id="([^"]+)"', text)}


def catalogue_ids(root, rel, key, prefix):
    return {f'{prefix}:{entry["id"]}' for entry in json.loads(read(root, rel))[key]}


def unit_ids(root):
    """Each `##`/`###` heading of unit-file.md that is one backticked section name: the file's sections."""
    out, fenced = set(), False
    for line in read(root, 'docs/reference/unit-file.md').splitlines():
        if line.lstrip().startswith('```'):
            fenced = not fenced
            continue
        m = None if fenced else re.match(r'^#{2,3} `([^`]+)`\s*$', line)
        if m:
            out.add(f'unit:{m.group(1)}')
    return out


def workspace_crates(root):
    """(package name, manifest dir) for the root crate and every workspace member."""
    top = tomllib.loads(read(root, 'Cargo.toml'))
    out = []
    for member in ['.'] + top.get('workspace', {}).get('members', []):
        manifest = tomllib.loads(read(root, f'{member}/Cargo.toml' if member != '.' else 'Cargo.toml'))
        out.append((manifest['package']['name'], member, manifest))
    return out


def cargo_ids(root):
    return {f'cargo:{name}/{feature}' for name, _, manifest in workspace_crates(root)
            for feature in manifest.get('features', {})}


TEST_MODULE = re.compile(r'#\[cfg\((?:all\()?test\b[^\]]*\]\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{')
DESCRIPTOR = re.compile(r'GuaranteeDescriptor::new\(\s*"([^"]+)"')


def companion_guarantee_ids(root):
    """Every guarantee a crate registers in code, outside test modules and test files. The core's own are
    built by `g(…)` in src/agent/guarantee.rs and are the catalogue; this finds the rest, whatever crate."""
    out = set()
    for _, member, _ in workspace_crates(root):
        for path in sorted((root / member).glob('src/**/*.rs')):
            rel = path.relative_to(root).as_posix()
            if rel.endswith('_tests.rs') or '/tests/' in rel:
                continue
            text = path.read_text()
            cut = TEST_MODULE.search(text)
            text = text[:cut.start()] if cut else text
            out |= {f'companion-guarantee:{i}' for i in DESCRIPTOR.findall(text)}
    return out


def source_ids(root):
    ids = {
        'concept': concept_ids(root),
        'guarantee': catalogue_ids(root, 'docs/reference/guarantee-catalogue.json', 'guarantees', 'guarantee'),
        'rule': catalogue_ids(root, 'docs/reference/rule-catalogue.json', 'rules', 'rule'),
        'unit': unit_ids(root),
        'cargo': cargo_ids(root),
        'companion-guarantee': companion_guarantee_ids(root),
    }
    # A core guarantee is in the catalogue; one registered in code under a catalogue id is the core's own.
    core = {i.split(':', 1)[1] for i in ids['guarantee']}
    ids['companion-guarantee'] = {i for i in ids['companion-guarantee'] if i.split(':', 1)[1] not in core}
    return ids


# --------------------------------------------------------------------------------------------- the registry

def registry(root):
    return tomllib.loads(read(root, REGISTRY))


def check_registry(root, reg, ids, errors):
    """(a) and (b): every source id mapped exactly once, every mapped id real, every page real and named."""
    owner = {}
    for kind, entries in (('feature', reg.get('feature', [])), ('exclusion', reg.get('exclusion', []))):
        for entry in entries:
            label = entry.get('name') or entry.get('reason', '')[:50]
            if kind == 'exclusion' and not entry.get('reason', '').strip():
                errors.append(f'{REGISTRY}: an exclusion has no reason: {entry.get("ids")}')
            if kind == 'feature' and not entry.get('page'):
                errors.append(f'{REGISTRY}: feature "{label}" names no page (a page, or "todo")')
            for i in entry.get('ids', []):
                if i in owner:
                    errors.append(f'{REGISTRY}: {i} is mapped twice ({owner[i]}; {kind} "{label}")')
                owner[i] = f'{kind} "{label}"'
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
    pages = {f['page'] for f in reg.get('feature', []) if f.get('page') not in (None, 'todo')}
    for page in sorted(pages):
        if not (root / FEATURES / page).exists():
            errors.append(f'{REGISTRY}: page {page} does not exist in {FEATURES}/')
    on_disk = {p.name for p in (root / FEATURES).glob('*.md')} - NOT_PAGES
    for page in sorted(on_disk - pages):
        errors.append(f'{FEATURES}/{page} is not in the registry: add a [[feature]] naming it')
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


def defines(text, name, kinds=('fn', 'struct', 'enum', 'trait', 'const')):
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
        if TEST_ATTR.search(s):
            return True
        if not s.startswith(('#[', '//', ')]')) and not s.endswith((',', '(')):
            return False
    return False


TEST_NAME = re.compile(r'(?<![\[\w])`((?:[a-z_][a-z0-9_]*::)*[a-z][a-z0-9]*(?:_[a-z0-9]+){3,})`(?!\]\()')


def named_tests(body):
    """(test path, [.rs targets in the same block]) for each test a section names: a backticked snake_case name
    of four or more words, not itself a link's text, in a paragraph or list item that links its file."""
    out = []
    for block in blocks(body):
        files = [m.group(2) for m in LINK.finditer(block) if m.group(2).partition('#')[0].endswith('.rs')]
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
    return set(re.findall(r'(?m)^\| ([A-Za-z0-9_.\[\]]+) \|', m.group(1) if m else ''))


def example_rows(root):
    """{example name: row} for the example index's capability matrix (first cell's backticked link)."""
    text = read(root, 'examples/README.md')
    m = re.search(r'^## The capability matrix\n(.*?)(?=^## |\Z)', text, re.M | re.S)
    rows = {}
    for line in (m.group(1) if m else '').splitlines():
        if line.startswith('| ['):
            name = re.match(r'^\| \[`([^`]+)`\]', line)
            if name:
                rows[name.group(1)] = line
    return rows


def links_id(text, i, page):
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
        return f'`{name.split("/", 1)[1]}`' in text
    if source == 'companion-guarantee':
        return f'`{name}`' in text
    return False


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

    # (e) code items, (g) tests, in Turn it on
    turn = body.get('Turn it on', '')
    for m in LINK.finditer(turn):
        label, url = m.group(1), m.group(2)
        code = re.fullmatch(r'`([A-Za-z_][\w:]*)(?:\(\))?`', label)
        path = url.partition('#')[0]
        if not code or not path.endswith('.rs'):
            continue
        target = (root / rel).parent / path
        name = code.group(1).split('::')[-1]
        if target.exists() and not defines(target.read_text(), name):
            errors.append(f'{rel}: Turn it on names `{code.group(1)}`, but no fn, struct, enum, trait or const '
                          f'{name} is in {path}')
    if turn.lstrip().startswith(IN_CODE):
        if not any(m.group(2).partition('#')[0].endswith('.rs') for m in LINK.finditer(turn)):
            errors.append(f'{rel}: Turn it on says "{IN_CODE}" but links no code item')
        if not named_tests(turn):
            errors.append(f'{rel}: Turn it on says "{IN_CODE}" but names no test that runs that code')

    for section in ('Turn it on', 'See it work'):
        for test, files in named_tests(body.get(section, '')):
            name = test.split('::')[-1]
            found = [f for f in files if (root / rel).parent.joinpath(f.partition('#')[0]).exists()
                     and defines_test(((root / rel).parent / f.partition('#')[0]).read_text(), name) is not None]
            if not found:
                errors.append(f'{rel}: {section} names the test `{test}`, but no fn {name} is in a file the same '
                              f'paragraph links ({", ".join(files) or "none"})')
                continue
            if not any(defines_test(((root / rel).parent / f.partition('#')[0]).read_text(), name) for f in found):
                errors.append(f'{rel}: {section} names `{test}` as a test, but fn {name} has no #[test] attribute')
            hit = excepted(test, patterns)
            if hit:
                errors.append(f'{rel}: {section} names the test `{test}`, which {EXCEPTIONS} excepts ({hit[0]}): '
                              f'a test CI never runs cannot show the feature')
            tests_named.append((rel, test))

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
    else:
        if example not in rows:
            errors.append(f'{REGISTRY}: feature "{feature["name"]}" records the example `{example}`, which is not a '
                          f'row of the example index\'s capability matrix')
        if not any(m.group(2).partition('#')[0].endswith('examples/README.md') for m in LINK.finditer(see)):
            errors.append(f'{rel}: See it work does not link the example index')

    # (i) every id the registry gives this page
    for i in feature.get('ids', []) + feature.get('links', []):
        if not links_id(text, i, page):
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


def check_links(root, errors):
    """(d): every link in docs/features/ resolves, file and anchor."""
    for path in sorted((root / FEATURES).glob('*.md')):
        rel = path.relative_to(root).as_posix()
        text = re.sub(r'```.*?```', '', strip_comments(read(root, rel)), flags=re.S)
        for m in LINK.finditer(text):
            url = m.group(2)
            if re.match(r'^[a-z]+:', url):
                continue
            error = resolve(root, rel, url)[1]
            if error:
                errors.append(error)


def check(root):
    errors = []
    try:
        reg = registry(root)
    except (OSError, tomllib.TOMLDecodeError) as e:
        return [f'{REGISTRY}: cannot be read: {e}']
    ids = source_ids(root)
    check_registry(root, reg, ids, errors)
    check_links(root, errors)
    rows, fields, patterns = example_rows(root), config_fields(root), exception_patterns(root)
    if not rows:
        errors.append('examples/README.md: no capability matrix rows found (the parser or the index changed)')
    if not fields:
        errors.append('docs/reference/configuration.md: no field rows found under "## The fields"')
    tests = []
    for feature in reg.get('feature', []):
        if feature.get('page') not in (None, 'todo') and (root / FEATURES / feature['page']).exists():
            check_page(root, feature, rows, fields, patterns, errors, tests)
    check_examples(reg, rows, errors)
    return errors


def named_test_list(root):
    errors, tests = [], []
    reg = registry(root)
    rows, fields, patterns = example_rows(root), config_fields(root), exception_patterns(root)
    for feature in reg.get('feature', []):
        if feature.get('page') not in (None, 'todo') and (root / FEATURES / feature['page']).exists():
            check_page(root, feature, rows, fields, patterns, errors, tests)
    return tests


def check_executed(root, logs):
    """(g), CI half: every named test executed in this run, as scripts/ci-test-coverage.py reads the logs."""
    coverage = _load('ci_test_coverage', 'ci-test-coverage.py')
    executed, universe = set(), set()
    for f in sorted(glob.glob(str(Path(logs) / '*'))):
        coverage.scan(Path(f).read_text(encoding='utf-8', errors='replace'), executed, universe)
    if not executed:
        return [f'{logs}: no executed test in the logs (the fetch failed, or the log format changed)']
    errors = []
    for rel, test in named_test_list(root):
        if not any(key.startswith('rust ') and (key.endswith(f'::{test}') or key.endswith(f' {test}')) for key in executed):
            errors.append(f'{rel}: names the test `{test}`, which no job of this run executed')
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
