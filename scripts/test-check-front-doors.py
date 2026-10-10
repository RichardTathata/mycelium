#!/usr/bin/env python3
"""Mutation checks: breaking a front door's routing structure must fail the gate. --list: its cases."""
import importlib.util
import sys
from pathlib import Path
from unittest.mock import patch

# Verification policy rule 3: each case prints `@@case@@ <suite>::<case>` as it starts; --list names them all.
SUITE = 'scripts/test-check-front-doors.py'
CASES = ['baseline', 'broken-route-target', 'retargeted-route', 'missing-route', 'routes-reordered',
         'broken-anchor', 'see-differs-from-chooser', 'chooser-after-path', 'compass-off-map',
         'ops-journey-missing', 'ops-journey-unreached']
if sys.argv[1:] == ['--list']:
    for c in CASES:
        print(f'@@case-list@@ {SUITE}::{c}')
    sys.exit(0)


def case(name):
    assert name in CASES, name
    print(f'@@case@@ {SUITE}::{name}', flush=True)


spec = importlib.util.spec_from_file_location('front_doors', Path(__file__).with_name('check-front-doors.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
original = Path.read_text


def mutated(relative, old, new, expected):
    target = module.ROOT / relative
    assert old in original(target), f'fixture no longer matches: {relative}: {old!r}'
    def read(path, *args, **kwargs):
        text = original(path, *args, **kwargs)
        return text.replace(old, new, 1) if Path(path) == target else text
    with patch.object(Path, 'read_text', read):
        errors = module.check(module.ROOT)
    assert any(expected in error for error in errors), (expected, errors)


case('baseline')
assert module.check(module.ROOT) == [], module.check(module.ROOT)
case('broken-route-target')
mutated('README.md', '(docs/operations/what-is-proven.md):', '(docs/operations/what-was-proven.md):',
        'does not resolve')
case('retargeted-route')
mutated('README.md', '[production-readiness checklist](docs/operations/production-readiness.md)',
        '[production-readiness checklist](docs/operations/deployment.md)',
        'route "Run a fleet" does not link docs/operations/production-readiness.md')
case('missing-route')
mutated('README.md', '- **Check the evidence** →', 'Check the evidence →', 'the router routes are')
case('routes-reordered')
mutated('README.md', '- **Build a fleet** →', '- **Run a fleet** →', 'the router routes are')
case('broken-anchor')
mutated('examples/README.md', '## What do you want to see?', '## What would you like to see?',
        'names an anchor examples/README.md does not have')
case('see-differs-from-chooser')
mutated('examples/README.md', '[`diagnostics`](coop/README.md#12--diagnostics)',
        '[`rotation`](coop/README.md#06--rotation)', 'the examples chooser names')
case('chooser-after-path')
mutated('examples/README.md', '\n## Learning path\n', '\n## Learning steps\n', 'expected the chooser, then')
case('compass-off-map')
mutated('README.md', '| [Federation](docs/capabilities.md) |', '| [Federation](docs/guide/17-federation.md) |',
        'compass row does not point into docs/capabilities.md')
case('ops-journey-missing')
mutated('docs/operations/README.md', '## Operator journey', '## Operator route', 'no "## Operator journey" section')
case('ops-journey-unreached')
mutated('docs/operations/README.md', '[Observe the fleet](observability.md)', 'Observe the fleet',
        'the operator journey does not reach observability.md')
print(f'test-check-front-doors: {len(CASES) - 1} negative mutations rejected; positive baseline passed')
