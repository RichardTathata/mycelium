#!/usr/bin/env python3
"""The front doors' routing structure (docs/positioning.md § The routes and the five-step path).

Structure and link targets, never prose: a door may reword a route; it may not drop one, reorder
them, point one somewhere else, or let a target stop resolving. Run by scripts/check-positioning.sh;
the mutation suite is scripts/test-check-front-doors.py.

  1. README.md carries the intent router (router markers) with four routes, in order, each one line,
     each holding its agreed targets, every link resolving (file and, for a Markdown file, anchor).
  2. README.md: router, then the capability compass (every row into docs/capabilities.md), then the
     five steps (the path markers; their text is compared by check-positioning.sh).
  3. The README's *See it work* examples are the examples page's chooser examples, exactly.
  4. examples/README.md: the chooser, then the *Learning path* heading and the five steps, then the
     capability matrix.
  5. docs/operations/README.md has its *Operator journey*: the heading and an ordered list that
     reaches the readiness checklist and observability (the *Run a fleet* route's own targets).
     The operations door is not required to carry the five steps.
"""
from pathlib import Path
import re
import sys
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parents[1]

# (label, the repository-relative targets the route must link; '#anchor' included where it matters)
ROUTES = [
    ('Build a fleet', ['docs/guide/README.md', 'docs/guide/tutorials/README.md',
                       'README.md#build-a-fleet--five-steps']),
    ('Run a fleet', ['docs/operations/README.md#operator-journey',
                     'docs/operations/production-readiness.md', 'docs/operations/observability.md']),
    ('See it work', ['examples/README.md#what-do-you-want-to-see']),
    ('Check the evidence', ['docs/operations/what-is-proven.md']),
]
SEE_COUNT = 4
JOURNEY_NEEDS = ['production-readiness.md', 'observability.md']

LINK = re.compile(r'\[((?:[^\[\]]|\[[^\]]*\])*)\]\(([^\s)]+)\)')
ITEM = re.compile(r'^- \*\*([^*]+)\*\* (?:→|—) ')


def block(text, name):
    """The lines between <!-- name:start --> and <!-- name:end -->, or None."""
    start, end = f'<!-- {name}:start -->', f'<!-- {name}:end -->'
    if start not in text or end not in text or text.index(start) > text.index(end):
        return None
    return text[text.index(start) + len(start):text.index(end)]


def slugs(markdown):
    """GitHub's heading anchors for a Markdown file (code fences skipped, duplicates numbered)."""
    seen, out, fenced = {}, set(), False
    for line in markdown.splitlines():
        if line.lstrip().startswith('```'):
            fenced = not fenced
            continue
        m = None if fenced else re.match(r'^#{1,6}\s+(.*?)\s*#*\s*$', line)
        if not m:
            continue
        s = re.sub(r'[^\w\- ]', '', m.group(1).strip().lower()).replace(' ', '-')
        n = seen.get(s, 0)
        seen[s] = n + 1
        out.add(s if n == 0 else f'{s}-{n}')
    return out


def resolve(root, source, url):
    """(repository-relative target 'path#anchor' or 'path', error or None) for a local link."""
    if re.match(r'^[a-z]+:', url):
        return url, None
    path, _, anchor = url.partition('#')
    base = (root / source).parent
    target = (base / unquote(path)).resolve() if path else (root / source).resolve()
    try:
        rel = target.relative_to(root.resolve()).as_posix()
    except ValueError:
        return url, f'{source}: link {url} leaves the repository'
    if not target.exists():
        return rel, f'{source}: link {url} does not resolve'
    if anchor and target.suffix == '.md' and anchor not in slugs(target.read_text()):
        return f'{rel}#{anchor}', f'{source}: link {url} names an anchor {rel} does not have'
    return (f'{rel}#{anchor}' if anchor else rel), None


def example_names(text):
    """Example names a block links: link text that is exactly one backticked identifier."""
    return [m.group(1)[1:-1] for m in LINK.finditer(text) if re.fullmatch(r'`[a-z0-9_]+`', m.group(1))]


def check(root):
    errors = []
    readme = (root / 'README.md').read_text()

    # 1. the router
    router = block(readme, 'router')
    see = []
    if router is None:
        errors.append('README.md: no intent router (<!-- router:start --> … <!-- router:end -->)')
    else:
        items = [line for line in router.splitlines() if line.startswith('- ')]
        labels = [ITEM.match(line).group(1) if ITEM.match(line) else line[:40] for line in items]
        if labels != [label for label, _ in ROUTES]:
            errors.append(f'README.md: the router routes are {labels}, expected {[l for l, _ in ROUTES]} in that order')
        for line, label in zip(items, labels):
            wanted = dict(ROUTES).get(label)
            if wanted is None:
                continue
            got = set()
            for m in LINK.finditer(line):
                target, error = resolve(root, 'README.md', m.group(2))
                if error:
                    errors.append(error)
                got.add(target)
            for target in wanted:
                if target not in got:
                    errors.append(f'README.md: route "{label}" does not link {target}')
            if label == 'See it work':
                see = example_names(line)

    # 2. order on the README, and the compass
    marks = [readme.find(f'<!-- {m} -->') for m in ('router:end', 'compass:start', 'compass:end', 'path:start')]
    if -1 in marks or marks != sorted(marks):
        errors.append('README.md: expected the router, then the capability compass, then the five steps')
    compass = block(readme, 'compass')
    if compass is not None:
        rows = [line for line in compass.splitlines() if line.startswith('|') and not set(line) <= set('|-: ')][1:]
        if not rows:
            errors.append('README.md: the capability compass has no rows')
        for row in rows:
            first = LINK.search(row)
            if not first or resolve(root, 'README.md', first.group(2))[0] != 'docs/capabilities.md':
                errors.append(f'README.md: a compass row does not point into docs/capabilities.md: {row[:60]}')
            for m in LINK.finditer(row):
                error = resolve(root, 'README.md', m.group(2))[1]
                if error:
                    errors.append(error)

    # 3 + 4. the examples page
    examples = (root / 'examples/README.md').read_text()
    chooser = block(examples, 'chooser')
    if chooser is None:
        errors.append('examples/README.md: no chooser (<!-- chooser:start --> … <!-- chooser:end -->)')
    else:
        items = [line for line in chooser.splitlines() if line.startswith('- ')]
        names = []
        for line in items:
            found = example_names(line)
            names += found[:1]
            for m in LINK.finditer(line):
                error = resolve(root, 'examples/README.md', m.group(2))[1]
                if error:
                    errors.append(error)
        if len(items) != SEE_COUNT or len(names) != SEE_COUNT:
            errors.append(f'examples/README.md: the chooser should name {SEE_COUNT} examples, one per line; it names {names}')
        if router is not None and sorted(see) != sorted(names):
            errors.append(f'README.md "See it work" names {see}; the examples chooser names {names}')
    order = [examples.find('<!-- chooser:start -->'), examples.find('\n## Learning path\n'),
             examples.find('<!-- path:start -->'), examples.find('\n## The capability matrix\n')]
    if -1 in order or order != sorted(order):
        errors.append('examples/README.md: expected the chooser, then "## Learning path" and the five steps, then "## The capability matrix"')

    # 5. the operations door's own journey
    ops = (root / 'docs/operations/README.md').read_text()
    m = re.search(r'^## Operator journey\n(.*?)(?=^## |\Z)', ops, re.M | re.S)
    if not m:
        errors.append('docs/operations/README.md: no "## Operator journey" section')
    else:
        steps = [line for line in m.group(1).splitlines() if re.match(r'^\d+\. ', line)]
        linked = {Path(link.group(2).partition('#')[0]).name for step in steps for link in LINK.finditer(step)}
        if len(steps) < 3:
            errors.append('docs/operations/README.md: the operator journey is not an ordered list of steps')
        for need in JOURNEY_NEEDS:
            if need not in linked:
                errors.append(f'docs/operations/README.md: the operator journey does not reach {need}')
    return errors


if __name__ == '__main__':
    errors = check(ROOT)
    for error in errors:
        print(f'front-doors: {error}', file=sys.stderr)
    print(f'check-front-doors: {len(errors)} failures')
    sys.exit(bool(errors))
