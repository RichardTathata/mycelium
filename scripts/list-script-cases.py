#!/usr/bin/env python3
"""List a shell suite's cases from its own call sites, for its --list (verification policy rule 3).

Usage: list-script-cases.py <script> <suite> <function> <format>
  <format> builds the case from the call's arguments: {1}, {2}, … ({N:stem} = basename without extension).
  Prints one `@@case-list@@ <suite>::<case>` line per call of <function>, in order.

It reads the shell grammar rather than a line pattern: indentation, digits, a trailing `|| exit 1`, and a
`\\`-continued line are calls; a comment, a heredoc body, a function definition and a word that is not in command
position (`echo run_demo`) are not. A call site it cannot read fails the listing, loudly, rather than dropping a
case: the function under `&&`, `||`, `;`, `if` or `!`, an argument the format needs that is missing, or a case
that is not a literal name (`"$var"`). No call at all fails too.
"""
from __future__ import annotations

import os
import re
import shlex
import sys

CASE = re.compile(r"^[A-Za-z0-9_.-]+$")
FIELD = re.compile(r"\{(\d+)(:stem)?\}")
HEREDOC = re.compile(r"<<-?\s*(['\"]?)([A-Za-z_][A-Za-z0-9_]*)\1")
OPERATORS = {";", "&&", "||", "|", "&", "(", ")", "{", "}", "if", "then", "elif", "else", "do", "while", "until", "!",
             "time", ";;"}


def logical_lines(text: str):
    """(line number, text) with continuations joined and heredoc bodies skipped."""
    lines = text.split("\n")
    i = 0
    while i < len(lines):
        start, line = i + 1, lines[i]
        while line.endswith("\\") and i + 1 < len(lines):
            i += 1
            line = line[:-1] + " " + lines[i]
        i += 1
        m = HEREDOC.search(line)
        yield start, line
        if m:
            strip = "<<-" in line[m.start():m.start() + 3]
            while i < len(lines) and (lines[i].strip() if strip else lines[i]) != m.group(2):
                i += 1
            i += 1


def tokens(line: str) -> list[str]:
    lex = shlex.shlex(line, posix=True, punctuation_chars=";&|()")
    lex.whitespace_split = True
    lex.commenters = "#"
    return list(lex)


def main() -> int:
    script, suite, func, fmt = sys.argv[1:5]
    errors, cases = [], []
    for n, line in logical_lines(open(script, encoding="utf-8").read()):
        if func not in line:
            continue
        try:
            toks = tokens(line)
        except ValueError:
            if re.search(rf"(^|[\s;&|(]){re.escape(func)}(\s|$)", line.split("#", 1)[0]):
                errors.append(f"{script}:{n}: a call of {func} the listing cannot read: {line.strip()}")
            continue
        for i, t in enumerate(toks):
            if t != func:
                continue
            if i == 0:
                if toks[1:2] and toks[1].startswith("("):
                    break  # the definition
                args = [func]
                for a in toks[1:]:
                    if a in OPERATORS:
                        break
                    args.append(a)

                def field(m: re.Match) -> str:
                    k = int(m.group(1))
                    if k >= len(args):
                        raise IndexError(k)
                    v = args[k]
                    return os.path.splitext(os.path.basename(v))[0] if m.group(2) else v
                try:
                    case = FIELD.sub(field, fmt)
                except IndexError:
                    errors.append(f"{script}:{n}: {func} called with too few arguments for {fmt}: {line.strip()}")
                    break
                if not CASE.match(case):
                    errors.append(f"{script}:{n}: {func}'s case {case!r} is not a literal name: {line.strip()}")
                else:
                    cases.append(case)
            elif toks[i - 1] in OPERATORS:
                errors.append(f"{script}:{n}: {func} is called under {toks[i - 1]!r}, where the listing cannot "
                              f"follow it — call it at the start of a line: {line.strip()}")
    if not cases and not errors:
        errors.append(f"{script}: no call of {func} found")
    for e in errors:
        print(f"list-script-cases: {e}", file=sys.stderr)
    if errors:
        return 1
    for c in cases:
        print(f"@@case-list@@ {suite}::{c}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
