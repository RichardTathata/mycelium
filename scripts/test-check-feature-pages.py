#!/usr/bin/env python3
"""Mutation checks: each fault planted in the repository's own text must fail the feature-pages gate, and the
unmutated repository must pass it. --list: its cases."""
import importlib.util
import sys
import tempfile
from pathlib import Path
from unittest.mock import patch

# Verification policy rule 3: each case prints `@@case@@ <suite>::<case>` as it starts; --list names them all.
SUITE = 'scripts/test-check-feature-pages.py'
CASES = ['baseline',
         # (a) the sources and the registry
         'unmapped-concept', 'unmapped-cargo-feature', 'unmapped-companion-guarantee', 'unmapped-unit-section',
         'stale-registry-id', 'mapped-twice', 'exclusion-without-reason',
         # (b) pages and the registry
         'page-not-in-registry', 'registry-page-missing',
         # (c) sections
         'missing-section', 'sections-out-of-order',
         # (d) links
         'dead-anchor', 'dead-glossary-anchor', 'dead-link',
         # (e) code items
         'absent-fn',
         # (f) configuration fields
         'missing-config-row',
         # (g) tests
         'absent-test', 'not-a-test', 'excepted-test', 'in-code-without-test', 'test-not-executed',
         'executed-baseline',
         # (h) examples
         'example-for-none-feature', 'example-absent-from-index', 'none-without-sentence',
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
original = Path.read_text
FLEET = 'docs/features/fleet-history-limits.md'
ROUTING = 'docs/features/inference-routing.md'
REG = 'docs/features/registry.toml'


def mutated(relative, replacements, expected):
    """Apply each (old, new) to one file as the gate reads it; the gate must report `expected`."""
    target = (module.ROOT / relative).resolve()
    text = original(target)
    for old, new in replacements:
        assert old in text, f'fixture no longer matches: {relative}: {old!r}'
        text = text.replace(old, new, 1)

    def read(path, *args, **kwargs):
        return text if Path(path).resolve() == target else original(path, *args, **kwargs)
    with patch.object(Path, 'read_text', read):
        errors = module.check(module.ROOT)
    assert any(expected in error for error in errors), (expected, errors)


case('baseline')
assert module.check(module.ROOT) == [], module.check(module.ROOT)

case('unmapped-concept')
mutated('docs/guide/00-concepts.md', [('<a id="lane"></a>', '<a id="lane"></a><a id="planted-term"></a>')],
        'concept:planted-term (concept) is not mapped')
case('unmapped-cargo-feature')
mutated('mycelium-wasm-host/Cargo.toml', [('\nstem = [', '\nplanted = []\nstem = [')],
        'cargo:mycelium-wasm-host/planted (cargo) is not mapped')
case('unmapped-companion-guarantee')
mutated('mycelium-wasm-host/src/activation_egress.rs', [('"stem.activation_egress",', '"stem.planted_guarantee",')],
        'companion-guarantee:stem.planted_guarantee (companion-guarantee) is not mapped')
case('unmapped-unit-section')
mutated('docs/reference/unit-file.md', [('\n## `[[serve]]`\n', '\n## `[[planted]]`\n\n## `[[serve]]`\n')],
        'unit:[[planted]] (unit) is not mapped')
case('stale-registry-id')
mutated('docs/reference/rule-catalogue.json', [('"id": "prov.shed"', '"id": "prov.shedding"')],
        'rule:prov.shed is mapped but no source has it')
case('mapped-twice')
mutated(REG, [('ids = ["concept:evidence-journal"]', 'ids = ["concept:evidence-journal", "concept:mandate"]')],
        'concept:mandate is mapped twice')
case('exclusion-without-reason')
mutated(REG, [('reason = "An external prerequisite', 'reason = ""\nwhy = "An external prerequisite')],
        'an exclusion has no reason')

case('page-not-in-registry')
mutated(REG, [('page = "inference-routing.md"\nexample = "none"', 'page = "todo"\nplanned = "inference-routing"')],
        'docs/features/inference-routing.md is not in the registry')
case('registry-page-missing')
mutated(REG, [('page = "inference-routing.md"', 'page = "inference-routed.md"')],
        'page inference-routed.md does not exist')

case('missing-section')
mutated(FLEET, [('\n## See it work\n', '\n## Seeing it work\n')], "missing: ['See it work']")
case('sections-out-of-order')
mutated(FLEET, [('\n## Turn it on\n', '\n## @@\n'), ('\n## See it work\n', '\n## Turn it on\n'),
                ('\n## @@\n', '\n## See it work\n')], 'expected the six, in order')

case('dead-anchor')
mutated(FLEET, [('20-authorising-actions.md#checking-your-evaluator-against-the-contract',
                 '20-authorising-actions.md#checking-your-evaluator')], 'names an anchor')
case('dead-glossary-anchor')
mutated(FLEET, [('00-concepts.md#history-scope', '00-concepts.md#history-scopes')],
        'names an anchor docs/guide/00-concepts.md does not have')
case('dead-link')
mutated(FLEET, [('(../../src/agent/fleet_history.rs)', '(../../src/agent/fleet_histories.rs)')], 'does not resolve')

case('absent-fn')
mutated(FLEET, [('[`Rule::at_most`]', '[`Rule::at_least`]')], 'no fn, struct, enum, trait or const at_least')

case('missing-config-row')
mutated(FLEET, [('[`control_min_peers_heard`]', '[`control_min_peers_seen`]')],
        'names the configuration field `control_min_peers_seen`, which has no row')

case('absent-test')
mutated(FLEET, [('`the_tally_scope_converges_and_refuses_after_convergence`',
                 '`the_tally_scope_converges_and_never_refuses`')],
        'no fn the_tally_scope_converges_and_never_refuses is in a file')
case('not-a-test')
mutated('src/agent/fleet_history_tests.rs',
        [('#[tokio::test(flavor = "multi_thread", worker_threads = 8)]\nasync fn the_tally_scope',
          '// planted: not a test\nasync fn the_tally_scope')], 'has no #[test] attribute')
case('excepted-test')
mutated('scripts/test-coverage-exceptions.txt',
        [('\n# Measurements and tools', '\nrust src/lib.rs::mycelium::agent::fleet_history_tests::the_tally_scope_converges_and_refuses_after_convergence — planted\n\n# Measurements and tools')],
        'which scripts/test-coverage-exceptions.txt excepts')
case('in-code-without-test')
mutated(ROUTING, [('- `guide_20_routing_policy`, in', '- The test guide_20_routing_policy, in')],
        'names no test that runs that code')

named = module.named_test_list(module.ROOT)
assert len(named) >= 5, named


def executed_log(tests):
    lines = ['Running unittests src/lib.rs (target/debug/deps/mycelium-0123abcd)']
    lines += [f'test some::module::{t} ... ok' for t in tests]
    return '\n'.join(lines) + '\n'


with tempfile.TemporaryDirectory() as logs:
    case('test-not-executed')
    names = [t for _, t in named]
    (Path(logs) / '1.log').write_text(executed_log(names[1:]))
    errors = module.check_executed(module.ROOT, logs)
    assert any(f'`{names[0]}`, which no job of this run executed' in e for e in errors), errors
    case('executed-baseline')
    (Path(logs) / '1.log').write_text(executed_log(names))
    assert module.check_executed(module.ROOT, logs) == [], module.check_executed(module.ROOT, logs)

case('example-for-none-feature')
mutated('examples/README.md',
        [('\n| [`hello_mesh`](../docs/guide/01-gossip-kv.md#the-example) · [src](hello_mesh.rs) | ●', '\n| [`fleet_history_limits`](../docs/features/fleet-history-limits.md) · [src](hello_mesh.rs) | ● | · | · | · | Intro | CLI | · | · | · | · | · |\n| [`hello_mesh`](../docs/guide/01-gossip-kv.md#the-example) · [src](hello_mesh.rs) | ●')],
        'has a row `fleet_history_limits` for "Fleet-wide history limits"')
case('example-absent-from-index')
mutated(REG, [('page = "inference-routing.md"\nexample = "none"', 'page = "inference-routing.md"\nexample = "routing_demo"')],
        'records the example `routing_demo`, which is not a row')
case('none-without-sentence')
mutated(ROUTING, [('No runnable example exists', 'There is no example')], 'does not open with "No runnable example exists"')

case('concept-not-linked')
mutated(FLEET, [('[history scope](../guide/00-concepts.md#history-scope)', 'history scope')],
        'does not link concept:history-scope')
case('guarantee-not-linked')
mutated(ROUTING, [('[`ae.authorised_at_seam` and `ae.recorded_before_dispatch`]', '[`ae.recorded_before_dispatch`]')],
        'does not link guarantee:ae.authorised_at_seam')

print(f'test-check-feature-pages: {len(CASES) - 2} negative mutations rejected; positive baselines passed')
