#!/usr/bin/env python3
"""Mutation checks: each fault planted in the repository's own text must fail the feature-pages gate, and the
unmutated repository must pass it. A planted page (one with a real example) exercises the example branch, which no
page in the repository takes yet. --list: its cases."""
import importlib.util
import sys
import tempfile
from pathlib import Path
from unittest.mock import patch

# Verification policy rule 3: each case prints `@@case@@ <suite>::<case>` as it starts; --list names them all.
SUITE = 'scripts/test-check-feature-pages.py'
CASES = ['baseline', 'example-baseline',
         # (a) the sources and the registry
         'unmapped-concept', 'glossary-row-without-anchor', 'unmapped-cargo-feature', 'implicit-cargo-feature',
         'unmapped-crate', 'unmapped-companion-guarantee', 'non-literal-guarantee-id', 'aliased-descriptor',
         'early-test-module', 'brace-in-string', 'unit-key-without-heading', 'unit-heading-without-key',
         'unit-subtable-gone', 'stale-registry-id', 'stale-link', 'mapped-twice', 'exclusion-without-reason',
         'unknown-registry-key', 'feature-without-page',
         # (b) pages and the registry
         'page-not-in-registry', 'registry-page-missing', 'two-features-one-page', 'subdirectory',
         # (c) sections
         'missing-section', 'sections-out-of-order',
         # (d) links
         'dead-anchor', 'dead-glossary-anchor', 'dead-link', 'reference-style-link', 'titled-link', 'a-href',
         # (e) code items
         'absent-fn', 'absent-qualifier', 'in-code-without-item',
         # (f) configuration fields
         'missing-config-row', 'parser-guard-fields',
         # (g) tests
         'absent-test', 'short-test-absent', 'not-a-test', 'none-names-no-test', 'test-without-file',
         'excepted-test', 'in-code-without-test', 'test-not-executed', 'test-in-wrong-target', 'executed-baseline',
         # (h) examples
         'example-for-none-feature', 'example-absent-from-index', 'example-missing', 'example-row-without-status',
         'see-without-index-link', 'none-without-sentence', 'parser-guard-matrix',
         # (i) the page links its ids
         'concept-not-linked', 'guarantee-not-linked']
if sys.argv[1:] == ['--list']:
    for c in CASES:
        print(f'@@case-list@@ {SUITE}::{c}')
    sys.exit(0)


def case(name):
    assert name in CASES, name
    print(f'@@case@@ {SUITE}::{name}', flush=True)


spec = importlib.util.spec_from_file_location('feature_pages', Path(__file__).with_name('check-feature-pages.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
original_read = Path.read_text
original_entries = module.feature_entries
original_exists = module.page_exists
ROOT = module.ROOT
FLEET = 'docs/features/fleet-history-limits.md'
ROUTING = 'docs/features/inference-routing.md'
REG = 'docs/features/registry.toml'
PLANTED = 'planted-example.md'

# A page with a real example: hello_mesh, a row of the capability matrix.
EXAMPLE_PAGE = """# Planted example

## What it is

A page the mutation suite plants, so the gate's example branch runs.

## When to use it, and when not

- **Use it when** testing the gate.
- **Not when** reading the docs.

## Turn it on

- [`tls`](../reference/configuration.md#the-fields).

## See it work

[`hello_mesh`](../../examples/README.md#the-capability-matrix) in the example index.

## What it guarantees, and what it does not

Nothing.

## Go deeper

Nothing.
"""
EXAMPLE_ENTRY = f'[[feature]]\nname = "Planted example"\npage = "{PLANTED}"\nexample = "hello_mesh"\nids = []\n\n'


def run(edits=None, pages=None, dirs=()):
    """The gate's errors with `edits` ({path: [(old, new)]}) applied as it reads, `pages` ({name: text}) added to
    docs/features/, and `dirs` added as subdirectories there."""
    texts = {}
    for relative, replacements in (edits or {}).items():
        target = (ROOT / relative).resolve()
        text = original_read(target)
        for old, new in replacements:
            assert old in text, f'fixture no longer matches: {relative}: {old!r}'
            text = text.replace(old, new, 1)
        texts[target] = text
    for name, text in (pages or {}).items():
        texts[(ROOT / module.FEATURES / name).resolve()] = text

    def read(path, *args, **kwargs):
        resolved = Path(path).resolve()
        return texts[resolved] if resolved in texts else original_read(path, *args, **kwargs)

    def entries(root):
        extra = [(root / module.FEATURES / n, False) for n in (pages or {})]
        return original_entries(root) + extra + [(root / module.FEATURES / d, True) for d in dirs]

    def exists(root, page):
        return page in (pages or {}) or original_exists(root, page)
    with patch.object(Path, 'read_text', read), patch.object(module, 'feature_entries', entries), \
            patch.object(module, 'page_exists', exists):
        return module.check(ROOT)


def fails(expected, edits=None, pages=None, dirs=()):
    errors = run(edits, pages, dirs)
    assert any(expected in error for error in errors), (expected, errors)


def with_example(page_edits=(), reg_edits=(), other=None):
    """Edits that plant the example page (with its own replacements) and its registry entry."""
    page = EXAMPLE_PAGE
    for old, new in page_edits:
        assert old in page, old
        page = page.replace(old, new, 1)
    edits = {REG: [('# The feature registry', EXAMPLE_ENTRY + '# The feature registry'), *reg_edits]}
    edits.update(other or {})
    return edits, {PLANTED: page}


case('baseline')
assert module.check(ROOT) == [], module.check(ROOT)
case('example-baseline')
edits, pages = with_example()
assert run(edits, pages) == [], run(edits, pages)

# (a)
case('unmapped-concept')
fails('concept:planted-term (concept) is not mapped',
      {'docs/guide/00-concepts.md': [('<a id="lane"></a>', '<a id="planted-term"></a>')]})
case('glossary-row-without-anchor')
fails('the glossary row "Planted term" has no <a id>',
      {'docs/guide/00-concepts.md': [('| <a id="seam"></a>', '| Planted term | native | a row | x |\n| <a id="seam"></a>')]})
case('unmapped-cargo-feature')
fails('cargo:mycelium-wasm-host/planted (cargo) is not mapped',
      {'mycelium-wasm-host/Cargo.toml': [('\nstem = [', '\nplanted = []\nstem = [')]})
case('implicit-cargo-feature')
fails('cargo:mycelium-commitment/planted-dep (cargo) is not mapped',
      {'mycelium-commitment/Cargo.toml': [('[dependencies]\n', '[dependencies]\nplanted-dep = { version = "1", optional = true }\n')]})
case('unmapped-crate')
fails('crate:mycelium-commitment (crate) is not mapped',
      {REG: [('ids = ["concept:award", "crate:mycelium-commitment"]', 'ids = ["concept:award"]')]})
case('unmapped-companion-guarantee')
fails('companion-guarantee:stem.planted_guarantee (companion-guarantee) is not mapped',
      {'mycelium-wasm-host/src/activation_egress.rs': [('"stem.activation_egress",', '"stem.planted_guarantee",')]})
case('non-literal-guarantee-id')
fails('takes an id that is not a string literal',
      {'mycelium-wasm-host/src/rules.rs': [('\n];\n', '\n];\n\nfn planted() { let _ = GuaranteeDescriptor::new(PLANTED_ID, 1); }\n')]})
case('aliased-descriptor')
fails('imports GuaranteeDescriptor as Gd',
      {'mycelium-wasm-host/src/activation_egress.rs': [('use mycelium::{ConfineBackend, GuaranteeDescriptor,',
                                                        'use mycelium::{ConfineBackend, GuaranteeDescriptor as Gd, GuaranteeDescriptor,')]})
case('early-test-module')
fails('companion-guarantee:stem.planted_after_a_test_module (companion-guarantee) is not mapped',
      {'mycelium-wasm-host/src/activation_sandbox.rs': [
          ('\n#[cfg(test)]\n', '\n#[cfg(test)]\nmod early_helpers {}\n\n#[cfg(test)]\n'),
          ('"stem.activation_confined",', '"stem.planted_after_a_test_module",')]})
case('brace-in-string')
fails('companion-guarantee:stem.planted_after_a_brace (companion-guarantee) is not mapped',
      {'mycelium-wasm-host/src/rules.rs': [('\n];\n', '\n];\n\n#[cfg(test)]\nmod brace_helpers { const OPEN: &str = "{"; const C: char = \'{\'; }\n\n'
                                                        'fn planted() { let _ = GuaranteeDescriptor::new("stem.planted_after_a_brace", 1); }\n')]})
case('unit-key-without-heading')
fails('the unit-file key `[[planted]]` (src/capability_config.rs) has no section heading',
      {'src/capability_config.rs': [('    #[serde(default, rename = "serve")]\n',
                                     '    #[serde(default, rename = "planted")]\n    pub planteds: Vec<ServeDecl>,\n    #[serde(default, rename = "serve")]\n')]})
case('unit-heading-without-key')
fails('the section `[[planted]]` names no key of NodeCapabilityConfig',
      {'docs/reference/unit-file.md': [('\n## `[[serve]]`\n', '\n## `[[planted]]`\n\n## `[[serve]]`\n')]})
case('unit-subtable-gone')
fails('the unit-file table [activation.confine.egress] is not in the code',
      {'src/capability_config.rs': [('    pub egress: Option<EgressDecl>,', '    pub egresses: Option<EgressDecl>,')]})
case('stale-registry-id')
fails('rule:prov.shed is mapped but no source has it',
      {'docs/reference/rule-catalogue.json': [('"id": "prov.shed"', '"id": "prov.shedding"')]})
case('stale-link')
fails('links rule:prov.planted, which no source has',
      {REG: [('links = ["rule:ae.preflight"', 'links = ["rule:prov.planted", "rule:ae.preflight"')]})
case('mapped-twice')
fails('concept:mandate is mapped twice',
      {REG: [('ids = ["concept:evidence-journal"]', 'ids = ["concept:evidence-journal", "concept:mandate"]')]})
case('exclusion-without-reason')
fails('an exclusion has no reason', {REG: [('reason = "An external prerequisite', 'reason = ""\nids_ = 1\nwhy = "An external prerequisite')]})
case('unknown-registry-key')
fails('has an unknown key `link`',
      {REG: [('links = ["rule:ae.preflight"', 'link = ["rule:ae.preflight"')]})
case('feature-without-page')
fails('feature "Evidence journal" names no page',
      {REG: [('name = "Evidence journal"\npage = "todo"\n', 'name = "Evidence journal"\n')]})

# (b)
case('page-not-in-registry')
fails('docs/features/inference-routing.md is not in the registry',
      {REG: [('page = "inference-routing.md"\nexample = "none"', 'page = "todo"\nplanned = "inference-routing"')]})
case('registry-page-missing')
fails('page inference-routed.md does not exist', {REG: [('page = "inference-routing.md"', 'page = "inference-routed.md"')]})
case('two-features-one-page')
fails('the page inference-routing.md is named by two features',
      {REG: [('# The feature registry', '[[feature]]\nname = "Planted twin"\npage = "inference-routing.md"\nexample = "none"\nids = []\n\n# The feature registry')]})
case('subdirectory')
fails('docs/features/planted/ is a subdirectory', dirs=['planted'])

# (c)
case('missing-section')
fails("missing: ['See it work']", {FLEET: [('\n## See it work\n', '\n## Seeing it work\n')]})
case('sections-out-of-order')
fails('expected the six, in order', {FLEET: [('\n## Turn it on\n', '\n## @@\n'), ('\n## See it work\n', '\n## Turn it on\n'),
                                             ('\n## @@\n', '\n## See it work\n')]})

# (d)
case('dead-anchor')
fails('names an anchor', {FLEET: [('20-authorising-actions.md#checking-your-evaluator-against-the-contract',
                                   '20-authorising-actions.md#checking-your-evaluator')]})
case('dead-glossary-anchor')
fails('names an anchor docs/guide/00-concepts.md does not have',
      {FLEET: [('00-concepts.md#history-scope', '00-concepts.md#history-scopes')]})
case('dead-link')
fails('does not resolve', {FLEET: [('(../../src/agent/fleet_history.rs)', '(../../src/agent/fleet_histories.rs)')]})
case('reference-style-link')
fails('a reference-style link', {FLEET: [('\n## Go deeper\n', '\nSee also [the glossary][g].\n\n[g]: ../guide/00-concepts.md#nowhere\n\n## Go deeper\n')]})
case('titled-link')
fails('a titled link', {FLEET: [('\n## Go deeper\n', '\nSee also [the glossary](../guide/00-concepts.md#nowhere "glossary").\n\n## Go deeper\n')]})
case('a-href')
fails('an <a href>', {FLEET: [('\n## Go deeper\n', '\nSee also <a href="../guide/00-concepts.md#nowhere">the glossary</a>.\n\n## Go deeper\n')]})

# (e)
case('absent-fn')
fails('no fn, struct, enum, trait, const, type or static at_least', {FLEET: [('[`Rule::at_most`]', '[`Rule::at_least`]')]})
case('absent-qualifier')
fails('no struct, enum, trait or type NoSuchRule', {FLEET: [('[`Rule::scoped`]', '[`NoSuchRule::scoped`]')]})
case('in-code-without-item')
edits, pages = with_example([('## Turn it on\n\n', '## Turn it on\n\nThis feature is turned on in code.\n\n')])
fails('says "This feature is turned on in code" but links no code item', edits, pages)

# (f)
case('missing-config-row')
fails('names the configuration field `control_min_peers_seen`, which has no row',
      {FLEET: [('[`control_min_peers_heard`]', '[`control_min_peers_seen`]')]})
case('parser-guard-fields')
fails('no field rows found under "## The fields"', {'docs/reference/configuration.md': [('\n## The fields\n', '\n## Fields\n')]})

# (g)
case('absent-test')
fails('no fn the_tally_scope_converges_and_never_refuses is in a file',
      {FLEET: [('`the_tally_scope_converges_and_refuses_after_convergence`', '`the_tally_scope_converges_and_never_refuses`')]})
case('short-test-absent')
fails('no fn tally_converges is in a file', {FLEET: [('`the_tally_scope_converges_and_refuses_after_convergence`', '`tally_converges`')]})
case('none-names-no-test')
fails('names no test that runs the feature end to end',
      {ROUTING: [('`a_confidential_request_is_routed_to_an_in_domain_model_and_the_external_one_is_passed_over`', 'one test')]})
case('test-without-file')
fails('read as a test, but its paragraph links no .rs file',
      {ROUTING: [('\n## What it guarantees', '\n`planted_test_name` runs it too.\n\n## What it guarantees')]})
case('not-a-test')
fails('has no #[test] attribute',
      {'src/agent/fleet_history_tests.rs': [('#[tokio::test(flavor = "multi_thread", worker_threads = 8)]\nasync fn the_tally_scope',
                                             '// #[tokio::test] commented out\nasync fn the_tally_scope')]})
case('excepted-test')
fails('which scripts/test-coverage-exceptions.txt excepts',
      {'scripts/test-coverage-exceptions.txt': [('\n# Measurements and tools', '\nrust src/lib.rs::mycelium::agent::fleet_history::tests::the_tally_scope_converges_and_refuses_after_convergence — planted\n\n# Measurements and tools')]})
case('in-code-without-test')
fails('names no test that runs that code', {ROUTING: [('- `guide_20_routing_policy`, in', '- The test guide_20_routing_policy, in')]})

named = module.named_test_list(ROOT)
assert len(named) >= 5, named


def log_for(entries):
    """A CI log in libtest's form for (test, repository file, right target?) entries."""
    lines = []
    for test, file, right in entries:
        crate, within = module.crate_of(ROOT, file)
        if within.startswith('tests/'):
            stem = Path(within).stem
            binary = stem if right else 'some_other_binary'
            lines.append(f'     Running {within} (target/debug/deps/{binary}-0123abcd)')
        else:
            binary = crate if right else 'some_other_crate'
            lines.append(f'     Running unittests src/lib.rs (target/debug/deps/{binary}-0123abcd)')
        lines.append(f'test some::module::{test} ... ok')
    return '\n'.join(lines) + '\n'


with tempfile.TemporaryDirectory() as logs:
    log = Path(logs) / '1.log'
    case('test-not-executed')
    log.write_text(log_for([(t, files[0], True) for _, t, files in named[1:]]))
    errors = module.check_executed(ROOT, logs)
    assert any(f'`{named[0][1]}`' in e and 'which no job of this run executed' in e for e in errors), errors
    case('test-in-wrong-target')
    log.write_text(log_for([(t, files[0], i != 0) for i, (_, t, files) in enumerate(named)]))
    errors = module.check_executed(ROOT, logs)
    assert any(f'`{named[0][1]}`' in e for e in errors), errors
    case('executed-baseline')
    log.write_text(log_for([(t, files[0], True) for _, t, files in named]))
    assert module.check_executed(ROOT, logs) == [], module.check_executed(ROOT, logs)

# (h)
case('example-for-none-feature')
fails('has a row `fleet_history_limits` for "Fleet-wide history limits"',
      {'examples/README.md': [('\n| [`hello_mesh`](../docs/guide/01-gossip-kv.md#the-example) · [src](hello_mesh.rs) | ●',
                               '\n| [`fleet_history_limits`](../docs/features/fleet-history-limits.md) · [src](hello_mesh.rs) | ● | · | · | · | Intro | CLI | · | · | · | · | · |\n| [`hello_mesh`](../docs/guide/01-gossip-kv.md#the-example) · [src](hello_mesh.rs) | ●')]})
case('example-absent-from-index')
fails('records the example `routing_demo`, which is not a row',
      {REG: [('page = "inference-routing.md"\nexample = "none"', 'page = "inference-routing.md"\nexample = "routing_demo"')]})
case('example-missing')
fails('has a page but records no example', {REG: [('page = "inference-routing.md"\nexample = "none"\n', 'page = "inference-routing.md"\n')]})
case('example-row-without-status')
edits, pages = with_example(other={'examples/README.md': [
    ('[`hello_mesh`](../docs/guide/01-gossip-kv.md#the-example) · [src](hello_mesh.rs) | ● | · | · | · | Intro | CLI | · | · | · | · | · |',
     '[`hello_mesh`](../docs/guide/01-gossip-kv.md#the-example) · [src](hello_mesh.rs) | ● | · | · | · | Intro | CLI | · | · | · | ? | · |')]})
fails('the row `hello_mesh` has no CI status', edits, pages)
case('see-without-index-link')
edits, pages = with_example([('[`hello_mesh`](../../examples/README.md#the-capability-matrix) in the example index.', '`hello_mesh` runs it.')])
fails('See it work does not link the example index', edits, pages)
case('none-without-sentence')
fails('does not open with "No runnable example exists"', {ROUTING: [('No runnable example exists', 'There is no example')]})
case('parser-guard-matrix')
fails('no capability matrix rows found', {'examples/README.md': [('\n## The capability matrix\n', '\n## The matrix\n')]})

# (i)
case('concept-not-linked')
fails('does not link concept:history-scope', {FLEET: [('[history scope](../guide/00-concepts.md#history-scope)', 'history scope')]})
case('guarantee-not-linked')
fails('does not link guarantee:ae.authorised_at_seam',
      {ROUTING: [('[`ae.authorised_at_seam` and `ae.recorded_before_dispatch`]', '[`ae.recorded_before_dispatch`]')]})

print(f'test-check-feature-pages: {len(CASES) - 3} negative mutations rejected; three positive baselines passed')
