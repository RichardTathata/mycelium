#!/usr/bin/env python3
"""Mutation checks: removing a route or introducing drift must fail the gate."""
import importlib.util
from pathlib import Path
from unittest.mock import patch

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


assert module.check(module.ROOT)[0] == []
mutated('docs/README.md', '(capabilities.md)', '(removed.md)', 'missing resource removed.md')
mutated('docs/capabilities.md', '(guide/21-mandates.md)', '(guide/20-authorising-actions.md)', 'missing reader route guide/21-mandates.md')
mutated('docs/publications/customer-pitch.html', 'aria-label="Next slide"', 'title="Next slide"', 'missing accessibility support')
mutated('README.md', '# Mycelium', '# Mycelium\n\nEvery action runs under explicit authority and leaves a record you can replay.', 'stale claim')
print('test-check-materials: four negative mutations rejected; positive baseline passed')
