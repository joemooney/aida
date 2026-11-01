#!/usr/bin/env python3
"""Assemble the public product website from one git revision.

Both hosts publish the output of this script: GitHub Pages (the `gh-pages`
branch) and GitLab Pages (the `pages` job in `.gitlab-ci.yml`). Generating
both from the same revision with the same options gives byte-identical file
sets; `--compare` verifies an existing published tree against a revision.

The published set is the static showcase (pages, styles, scripts, assets;
not templates, the builder, the manifest or this README), plus the standalone
overview `docs/what-is-aida.html` as `overview.html`, plus `.nojekyll`.
Relative links that resolve to a published file are rewritten to its
published name; every other in-repository link points at the public
repository, because the site does not carry the rest of the repository.

Standard library plus the git executable only; no network IO.
"""
# trace:TASK-2604 | ai:claude

from __future__ import annotations

import argparse
import posixpath
import re
import subprocess
import sys
from pathlib import Path

SHOWCASE = "docs/showcase"
OVERVIEW_SOURCE = "docs/what-is-aida.html"
DEFAULT_REPO_URL = "https://github.com/joemooney/aida"
DEFAULT_BLOB_PREFIX = DEFAULT_REPO_URL + "/blob/main/"
PUBLISHED_SUFFIXES = (".html", ".css", ".js", ".svg", ".png", ".jpg", ".webp", ".ico")
EXCLUDED_NAMES = {"build.py", "publish.py", "publication.json", "README.md"}
LINK_RE = re.compile(r'(\b(?:href|src)=")([^"]*)(")')
SCHEME_RE = re.compile(r"^[a-zA-Z][a-zA-Z0-9+.-]*:")


def git(*args: str) -> bytes:
    return subprocess.run(["git", *args], check=True, capture_output=True).stdout


def tracked_files(rev: str) -> list[str]:
    out = git("ls-tree", "-r", "--name-only", rev, "--", SHOWCASE, OVERVIEW_SOURCE)
    return [line for line in out.decode().splitlines() if line]


def select(paths: list[str]) -> dict[str, str]:
    """Map repository path -> published path."""
    published: dict[str, str] = {}
    for path in paths:
        if path == OVERVIEW_SOURCE:
            published[path] = "overview.html"
            continue
        rel = posixpath.relpath(path, SHOWCASE)
        name = posixpath.basename(rel)
        if rel.startswith("..") or name in EXCLUDED_NAMES or name.startswith("."):
            continue
        if name.endswith("-template.html") or not name.endswith(PUBLISHED_SUFFIXES):
            continue
        published[path] = rel
    return published


def rewrite_links(html: str, source: str, target: str, published: dict[str, str], blob_prefix: str, repo_url: str) -> str:
    source_dir = posixpath.dirname(source)
    target_dir = posixpath.dirname(target) or "."

    def replace(match: re.Match[str]) -> str:
        value = match.group(2)
        if not value or value.startswith(("#", "/", "?")) or SCHEME_RE.match(value):
            return match.group(0)
        path, sep, fragment = value.partition("#")
        suffix = sep + fragment
        resolved = posixpath.normpath(posixpath.join(source_dir, path))
        if resolved.startswith(".."):
            return match.group(0)
        if resolved in published:
            new = posixpath.relpath(published[resolved], target_dir) + suffix
        elif resolved == ".":
            new = repo_url + suffix
        else:
            new = blob_prefix + resolved + suffix
        return match.group(1) + new + match.group(3)

    return LINK_RE.sub(replace, html)


def assemble(rev: str, repo_url: str, blob_prefix: str) -> dict[str, bytes]:
    published = select(tracked_files(rev))
    if OVERVIEW_SOURCE not in published or f"{SHOWCASE}/index.html" not in published:
        raise SystemExit(f"publish.py: {rev} lacks {OVERVIEW_SOURCE} or {SHOWCASE}/index.html")
    tree: dict[str, bytes] = {".nojekyll": b""}
    for source, target in published.items():
        data = git("show", f"{rev}:{source}")
        if target.endswith(".html"):
            data = rewrite_links(data.decode("utf-8"), source, target, published, blob_prefix, repo_url).encode("utf-8")
        tree[target] = data
    return tree


def write_tree(tree: dict[str, bytes], out: Path) -> None:
    if out.exists() and any(out.iterdir()):
        raise SystemExit(f"publish.py: {out} is not empty; refusing to mix revisions")
    for name, data in sorted(tree.items()):
        path = out / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)


def read_tree(root: Path) -> dict[str, bytes]:
    return {
        p.relative_to(root).as_posix(): p.read_bytes()
        for p in root.rglob("*")
        if p.is_file() and ".git" not in p.relative_to(root).parts
    }


def compare(expected: dict[str, bytes], actual: dict[str, bytes]) -> list[str]:
    problems = []
    for name in sorted(expected.keys() - actual.keys()):
        problems.append(f"missing: {name}")
    for name in sorted(actual.keys() - expected.keys()):
        problems.append(f"extra:   {name}")
    for name in sorted(expected.keys() & actual.keys()):
        if expected[name] != actual[name]:
            problems.append(f"differs: {name}")
    return problems


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--out", type=Path, help="write the site into this (empty or new) directory")
    mode.add_argument("--compare", type=Path, metavar="DIR", help="check a published tree against the revision")
    parser.add_argument("--rev", default="HEAD", help="git revision to publish (default: HEAD)")
    parser.add_argument("--repo-url", default=DEFAULT_REPO_URL, help="public repository URL for repository-root links")
    parser.add_argument("--blob-prefix", default=DEFAULT_BLOB_PREFIX, help="URL prefix for other repository files")
    args = parser.parse_args(argv)

    tree = assemble(args.rev, args.repo_url, args.blob_prefix)
    if args.out:
        write_tree(tree, args.out)
        print(f"publish.py: wrote {len(tree)} files from {args.rev} to {args.out}")
        return 0
    problems = compare(tree, read_tree(args.compare))
    for line in problems:
        print(line)
    if problems:
        print(f"publish.py: {args.compare} does not match {args.rev} ({len(problems)} differences)")
        return 1
    print(f"publish.py: {args.compare} matches {args.rev} ({len(tree)} files)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
