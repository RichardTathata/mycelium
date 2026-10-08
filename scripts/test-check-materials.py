#!/usr/bin/env python3
"""Mutation checks: removing a route or introducing drift must fail the gate. --list: its cases."""
import importlib.util
import sys
from pathlib import Path
from unittest.mock import patch

# Verification policy rule 3: each case prints `@@case@@ <suite>::<case>` as it starts; --list names them all.
SUITE = 'scripts/test-check-materials.py'
CASES = ['baseline', 'missing-resource', 'missing-reader-route', 'missing-accessibility', 'stale-claim']
if sys.argv[1:] == ['--list']:
    for c in CASES:
        print(f'@@case-list@@ {SUITE}::{c}')
    sys.exit(0)


def case(name):
    assert name in CASES, name
    print(f'@@case@@ {SUITE}::{name}', flush=True)


spec = importlib.util.spec_from_file_location('materials', Path(__file__).with_name('check-materials.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
original = Path.read_text


def mutated(relative, old, new, expected):
    target = module.ROOT / relative
    assert old in original(target), f'fixture no longer matches: {relative}'
    def read(path, *args, **kwargs):
        text = original(path, *args, **kwargs)
        return text.replace(old, new) if path == target else text
    with patch.object(Path, 'read_text', read):
        errors, _, _ = module.check(module.ROOT)
    assert any(expected in error for error in errors), errors


case('baseline')
assert module.check(module.ROOT)[0] == []
case('missing-resource')
mutated('docs/README.md', '(capabilities.md)', '(removed.md)', 'missing resource removed.md')
case('missing-reader-route')
mutated('docs/capabilities.md', '(guide/21-mandates.md)', '(guide/20-authorising-actions.md)', 'missing reader route guide/21-mandates.md')
case('missing-accessibility')
mutated('docs/publications/customer-pitch.html', 'aria-label="Next slide"', 'title="Next slide"', 'missing accessibility support')
case('stale-claim')
mutated('README.md', '# Mycelium', '# Mycelium\n\nEvery action runs under explicit authority and leaves a record you can replay.', 'stale claim')
print('test-check-materials: four negative mutations rejected; positive baseline passed')
