#!/usr/bin/env python3
"""Build the reader book and exclude copied contributor/archive assets.

mdBook before build.ignore support copies every non-Markdown source file,
including unlisted internal HTML reports. Only the generated book is pruned.
"""
# trace:STORY-1651 | ai:codex
from pathlib import Path
import functools
import http.server
import sys
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]
# Research reports are contributor evidence. trace:SPIKE-714 | ai:codex
EXCLUDED = ("internal", "archive", "plans", "spikes", "aida", "api")


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
    if "--serve" in sys.argv:
        # Serve exactly the publication output. Port 0 uses an available ephemeral port.
        handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(output))
        with http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler) as server:
            print(f"Reader book: http://127.0.0.1:{server.server_port}", flush=True)
            server.serve_forever()


if __name__ == "__main__":
    main()
