#!/usr/bin/env python3
"""Build the reader book and exclude copied contributor/archive assets.

mdBook before build.ignore support copies every non-Markdown source file,
including unlisted internal HTML reports. Only the generated book is pruned.
"""
# trace:STORY-1651 | ai:codex
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]
EXCLUDED = ("internal", "archive", "plans", "aida", "api")


def prune(output):
    if output.is_symlink():
        raise ValueError("Book output must be an ordinary directory")
    for name in EXCLUDED:
        path = output / name
        if path.is_symlink():
            path.unlink()
        elif path.is_dir():
            shutil.rmtree(path)
    # These are legacy standalone renders, not chapters in this book.
    for path in output.glob("user-guide*.html"):
        path.unlink()
    print("Reader book excludes contributor and archive assets")


def main():
    output = ROOT / "docs/book"
    if output.is_symlink():
        raise ValueError("Book output must be an ordinary directory")
    subprocess.run(["mdbook", "build", "docs"], cwd=ROOT, check=True)
    prune(output)


if __name__ == "__main__":
    main()
