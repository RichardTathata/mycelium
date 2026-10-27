#!/usr/bin/env python3
"""Adopter-surface Y3: check a browser's rendering of the Ops Console against its README.

  check-ops-console-render.py --tabs <README>
      print the README table's tabs, one per line (the cases scripts/ops-console-render.sh runs)
  check-ops-console-render.py --viz <dumped DOM> <href|none>
      the page's "↗ label" link (`a#vizlink`) carries exactly <href>, or — `none` — no href and is hidden
  check-ops-console-render.py --check <README> <Tab> <state> <node,node,…> <dumped DOM>
      the DOM rendered <Tab>, and has one section per node, each in <state>, each naming exactly the routes
      the README's row names for <Tab>, in order

The README's table is `| Tab | Route(s) | Shows |`; the routes are the backticked spans in the second
column. The page marks each node's section `data-panel`, `data-node`, `data-state`, `data-routes` and the
view `data-rendered`; nothing else of the page's markup is relied on.
"""
from __future__ import annotations

import re
import sys
from html.parser import HTMLParser

ROW = re.compile(r"^\|\s*([A-Za-z]+)\s*\|\s*(`[^|]*`)\s*\|")


def table(readme: str) -> dict[str, list[str]]:
    rows: dict[str, list[str]] = {}
    with open(readme, encoding="utf-8") as f:
        for line in f:
            m = ROW.match(line)
            if m and m.group(1) not in ("Tab",):
                rows[m.group(1)] = re.findall(r"`([^`]+)`", m.group(2))
    if not rows:
        sys.exit(f"{readme}: no `| Tab | Route(s) | Shows |` rows found")
    return rows


class Sections(HTMLParser):
    def __init__(self) -> None:
        super().__init__()
        self.sections: list[dict[str, str]] = []
        self.rendered: str | None = None
        self.vizlink: dict[str, str] | None = None

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if tag == "section" and "data-panel" in a:
            self.sections.append(a)
        if tag == "main" and "data-rendered" in a:
            self.rendered = a["data-rendered"]
        if tag == "a" and a.get("id") == "vizlink":
            self.vizlink = a


def check(readme: str, tab: str, state: str, nodes: list[str], dom_path: str) -> list[str]:
    rows = table(readme)
    if tab not in rows:
        return [f"{tab}: no row in {readme}"]
    p = Sections()
    with open(dom_path, encoding="utf-8") as f:
        p.feed(f.read())
    errs = []
    if p.rendered != tab:
        errs.append(f"{tab}: the page rendered {p.rendered!r}")
    seen = {s.get("data-node"): s for s in p.sections if s.get("data-panel") == tab}
    for n in nodes:
        s = seen.get(n)
        if s is None:
            errs.append(f"{tab}: no section for {n}")
            continue
        if s.get("data-state") != state:
            errs.append(f"{tab}: {n} is {s.get('data-state')!r}, expected {state!r}")
        routes = (s.get("data-routes") or "").split()
        if routes != rows[tab]:
            errs.append(f"{tab}: {n} read {routes}, the README names {rows[tab]}")
    extra = set(seen) - set(nodes)
    if extra:
        errs.append(f"{tab}: sections for nodes not asked for: {sorted(extra)}")
    return errs


def check_viz(dom_path: str, want: str) -> list[str]:
    p = Sections()
    with open(dom_path, encoding="utf-8") as f:
        p.feed(f.read())
    if p.vizlink is None:
        return ["the page has no a#vizlink"]
    href = p.vizlink.get("href")
    shown = "display: none" not in (p.vizlink.get("style") or "").replace("display:none", "display: none")
    if want == "none":
        return [] if href is None and not shown else [f"a link was made: href={href!r}, shown={shown}"]
    return [] if href == want and shown else [f"expected {want!r} shown, got href={href!r}, shown={shown}"]


def main(argv: list[str]) -> int:
    if len(argv) == 3 and argv[1] == "--tabs":
        print("\n".join(table(argv[2])))
        return 0
    if len(argv) == 4 and argv[1] == "--viz":
        errs = check_viz(argv[2], argv[3])
        for e in errs:
            print(f"check-ops-console-render: viz link: {e}", file=sys.stderr)
        print(f"check-ops-console-render: viz link {'FAILED' if errs else 'ok'} ({argv[3]})")
        return 1 if errs else 0
    if len(argv) == 7 and argv[1] == "--check":
        _, _, readme, tab, state, nodes, dom = argv
        errs = check(readme, tab, state, [n for n in nodes.split(",") if n], dom)
        for e in errs:
            print(f"check-ops-console-render: {e}", file=sys.stderr)
        print(f"check-ops-console-render: {tab} {'FAILED' if errs else 'ok'} ({state})")
        return 1 if errs else 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
