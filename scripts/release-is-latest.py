#!/usr/bin/env python3
"""Whether a release is the highest `vX.Y.Z` tag, so `latest` may move to it (security-operations SO3).

WHY IT EXISTS
  `latest` — the GitHub Release's label and the image's `:latest` tag — names the newest line, not the newest push. A
  patch cut from an older line after a newer MINOR (2.40.2 after 2.41.0) must not move it back (the maintainer's
  decision, 2026-11-22). The release workflow's `publish` job asks this script once and passes its answer to both
  `gh release create --latest=…` and the step that moves `:latest`.

THE RULE
  The tags considered are the repository's `v<MAJOR>.<MINOR>.<PATCH>` tags, each a number without leading zeros
  (`v02.99.0` is not one), compared as three integers (so 2.10.0 is above 2.9.0). A pre-release (`v2.42.0-rc.1`, any suffix) is not a release this repository cuts (RELEASING.md tags
  plain `vX.Y.Z` only) and is ignored, as are the companions' `<name>-vX.Y.Z` tags and anything else. The answer is
  `true` when VERSION is a plain `X.Y.Z` at or above every such tag, `false` otherwise — a pre-release VERSION is
  always `false`. VERSION's own tag must be in the list: a listing that does not hold the tag being published (an
  empty answer, a failed fetch) is refused rather than read as "nothing is higher".

USAGE
  git ls-remote --tags origin 'refs/tags/v*' | scripts/release-is-latest.py 2.41.0   # prints true or false
  git tag -l 'v*' | scripts/release-is-latest.py 2.41.0                              # tag names work too
  scripts/release-is-latest.py --self-test
  Exit 0 with the answer on stdout and the reason on stderr; exit 1 when the listing lacks vVERSION; 2 on misuse.
"""
import re
import sys

N = r"(0|[1-9]\d*)"  # a number without leading zeros (semver 2.0.0, item 2)
RELEASE = re.compile(rf"^v{N}\.{N}\.{N}$")
PLAIN = re.compile(rf"^{N}\.{N}\.{N}$")


def tag_names(text):
    """Tag names from `git ls-remote --tags` lines (`<sha>\\trefs/tags/<name>`, `^{}` peeled lines skipped) or bare names."""
    out = []
    for line in text.splitlines():
        f = line.split()
        if not f:
            continue
        name = f[-1]
        if name.endswith("^{}"):
            continue
        out.append(name[len("refs/tags/"):] if name.startswith("refs/tags/") else name)
    return out


def decide(version, names):
    """(answer, reason); raises ValueError when the listing does not hold vVERSION."""
    if f"v{version}" not in names:
        raise ValueError(f"the tag listing does not hold v{version} ({len(names)} tags); refusing to decide latest from it")
    m = PLAIN.match(version)
    if not m:
        return False, f"{version} is a pre-release or not a plain X.Y.Z; latest stays where it is"
    mine = tuple(int(x) for x in m.groups())
    higher = sorted((tuple(int(x) for x in r.groups()), n) for n in names if (r := RELEASE.match(n)))
    top = higher[-1] if higher else (mine, f"v{version}")
    if top[0] > mine:
        return False, f"{top[1]} is above v{version}; latest stays on {top[1]}"
    return True, f"v{version} is the highest release tag"


CASES = [
    # (version, tags, expected) — expected None means refused.
    ("2.40.2", ["v2.40.0", "v2.40.1", "v2.41.0", "v2.40.2"], False),          # a patch to an older line
    ("2.41.0", ["v2.40.0", "v2.40.1", "v2.40.2", "v2.41.0"], True),           # the newest MINOR, after an older patch
    ("2.41.1", ["v2.41.0", "v2.40.2", "v2.41.1"], True),                      # a patch to the newest line
    ("2.10.0", ["v2.8.0", "v2.9.0", "v2.9.1", "v2.10.0"], True),             # numeric, not lexical: 10 > 9
    ("2.9.2", ["v2.9.0", "v2.10.0", "v2.9.2"], False),                       # …and 2.9.2 stays below 2.10.0
    ("2.42.0", ["v2.41.3", "v2.40.9", "v2.42.0"], True),                      # the first release of a new line
    ("3.0.0", ["v2.99.9", "v3.0.0"], True),                                   # the first release of a MAJOR
    ("1.0.0", ["v1.0.0"], True),                                              # the repository's first release
    ("2.41.0", ["v2.41.0", "v2.42.0"], False),                                # a re-run of an older tag's push
    ("2.41.0", ["v2.41.0", "v2.42.0-rc.1", "v3.0.0-beta"], True),             # pre-release tags are ignored
    ("2.42.0-rc.1", ["v2.41.0", "v2.42.0-rc.1"], False),                      # a pre-release never moves latest
    ("2.41.0", ["v2.41.0", "mycelium-py-v9.9.9", "langgraph-checkpoint-mycelium-v0.3.4"], True),  # companions ignored
    ("2.41.0", ["v2.41.0", "v9", "v2.50", "vnext", "v2.50.0.1"], True),       # malformed tags ignored
    ("2.41.0", ["v2.41.0", "v02.99.0", "v2.041.0"], True),                   # leading zeros are not a version
    ("02.41.0", ["v02.41.0"], False),                                         # …nor is a VERSION with one
    ("2.41.0", [], None),                                                     # an empty listing is refused
    ("2.41.0", ["v2.40.0", "v2.42.0"], None),                                 # a listing without the tag is refused
]

LS_REMOTE = ("a" * 40 + "\trefs/tags/v2.40.2\n" + "b" * 40 + "\trefs/tags/v2.40.2^{}\n"
             + "c" * 40 + "\trefs/tags/v2.41.0\n" + "d" * 40 + "\trefs/tags/v2.41.0^{}\n")


def self_test():
    bad = 0
    for version, tags, want in CASES:
        try:
            got, why = decide(version, tags)
        except ValueError as e:
            got, why = None, str(e)
        ok = got == want
        bad += not ok
        print(f"{'ok  ' if ok else 'FAIL'}  {version} among {tags}: {got} ({why})")
    names = tag_names(LS_REMOTE)
    ok = names == ["v2.40.2", "v2.41.0"] and decide("2.40.2", names)[0] is False and decide("2.41.0", names)[0] is True
    bad += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  ls-remote lines parsed, peeled lines skipped: {names}")
    total = len(CASES) + 1
    print(f"release-is-latest self-test: {total - bad} of {total} as expected")
    return 1 if bad else 0


def main():
    args = sys.argv[1:]
    if args == ["--self-test"]:
        return self_test()
    if len(args) != 1 or not args[0]:
        print(__doc__, file=sys.stderr)
        return 2
    try:
        answer, why = decide(args[0], tag_names(sys.stdin.read()))
    except ValueError as e:
        print(f"release-is-latest: {e}", file=sys.stderr)
        return 1
    print(f"release-is-latest: {why}", file=sys.stderr)
    print("true" if answer else "false")
    return 0


if __name__ == "__main__":
    sys.exit(main())
