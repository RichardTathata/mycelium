#!/usr/bin/env python3
"""Check live reader routes and resource targets; no network or Rust build needed.

Historical papers/plans and audit ledgers are intentionally outside the claim scan.
This is a structural gate, not a substitute for editorial or accessibility review.
"""
from pathlib import Path
import re
import subprocess
import sys
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parents[1]
ROUTES = {
    'README.md': ['docs/README.md', 'docs/capabilities.md', 'docs/publications/research-guide.md'],
    'docs/README.md': ['capabilities.md', 'publications/customer-pitch.html', 'guide/installation.md', 'operations/README.md', 'publications/research-guide.md'],
    'docs/operations/README.md': ['deployment.md', 'production-readiness.md', 'observability.md', 'diagnostics.md', 'engagement-kit.md'],
    'docs/publications/README.md': ['research-guide.md', 'overclaim-ledger.md'],
}
for deck in ('customer-pitch', 'presentation'):
    ROUTES[f'docs/publications/{deck}.html'] = ['../capabilities.md', '../../examples/README.md', '../operations/what-is-proven.md', '../operations/customer-pilot.md', 'research-guide.md']
CHAPTERS = ['01-gossip-kv', '02-capabilities', '03-signals', '04-consensus', '05-skills', '06-tool-discovery', '07-pipelines', '08-a2a-interop', '11-semantic-coordination', '15-reasoning-and-langgraph', '16-guardrails', '17-federation', '18-contracts-and-receipts', '19-replay-and-simulation', '20-authorising-actions', '21-mandates', '22-stability-and-control', '23-knowledge', '24-commitments']
ROUTES['docs/capabilities.md'] = [f'guide/{chapter}.md' for chapter in CHAPTERS]
ROUTES['mycelium-wasm-host/README.md'] = ['../docs/guide/tutorials/01-first-stem-fleet.md']
STALE = ['Every action runs under explicit authority and leaves a record you can replay.',
         'Every coordination system before Mycelium', 'always has an up-to-date view, on every node',
         'two published crates', 'loopback (~1 ms overhead)', 'Zero — no processes']
LINK = re.compile(r'\]\(([^\s)]+)(?:\s+"[^"]*")?\)|(?:href|src)=["\']([^"\']+)["\']|^\s*\[[^\]]+\]:\s*(\S+)', re.M)


def links(text):
    text = re.sub(r'```.*?```', '', text, flags=re.S)
    return [next(value for value in match.groups() if value).strip('<>') for match in LINK.finditer(text)]


def scoped(path):
    return (path in ('README.md', 'docs/README.md', 'docs/capabilities.md', 'docs/positioning.md')
            or path.startswith(('docs/guide/', 'docs/operations/', 'examples/'))
            or (path.startswith('mycelium') and path.endswith('/README.md'))
            or path in ('docs/publications/README.md', 'docs/publications/research-guide.md',
                        'docs/publications/customer-pitch.html', 'docs/publications/presentation.html'))


def check(root):
    errors = []
    names = subprocess.check_output(['git', 'ls-files', '--cached', '--others', '--exclude-standard'], cwd=root, text=True).splitlines()
    files = sorted({p for p in names if scoped(p) and Path(p).suffix in ('.md', '.html')})
    count = 0
    for name in files:
        text = (root / name).read_text()
        urls = links(text)
        for url in urls:
            parsed = urlsplit(url)
            if parsed.scheme or parsed.netloc or not parsed.path or '${' in url:
                continue
            if parsed.path.startswith('/'):
                continue  # HTTP routes in example pages are not repository files.
            count += 1
            target = (root / name).parent / unquote(parsed.path)
            if not target.exists():
                errors.append(f'{name}: missing resource {url}')
        for phrase in STALE:
            if phrase in text:
                errors.append(f'{name}: stale claim: {phrase}')
    for name, required in ROUTES.items():
        path = root / name
        if not path.exists():
            errors.append(f'missing reader entry: {name}')
            continue
        urls = links(path.read_text())
        for target in required:
            if target not in urls:
                errors.append(f'{name}: missing reader route {target}')
    for deck in ('customer-pitch', 'presentation'):
        name = f'docs/publications/{deck}.html'
        text = (root / name).read_text()
        for marker in ('aria-label="Previous slide"', 'aria-label="Next slide"', 'aria-live="polite"', 'slide.inert', 'prefers-reduced-motion', '@media print', 'body.reading', "setAttribute('role', 'heading')"):
            if marker not in text:
                errors.append(f'{name}: missing accessibility support: {marker}')
    return errors, len(files), count


if __name__ == '__main__':
    errors, files, count = check(ROOT)
    for error in errors:
        print(error, file=sys.stderr)
    print(f'check-materials: {files} live files, {count} local resources, {len(errors)} failures')
    sys.exit(bool(errors))
