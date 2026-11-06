#!/usr/bin/env python3
"""Check local file links throughout the documentation and audience entry points.

Historical pages are checked for file targets without changing their claims.
Anchor candidates remain the audit tool's responsibility.
"""
# trace:STORY-1651 | ai:codex
import importlib.util
import sys
from pathlib import Path
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("doc_audit", ROOT / "docs/internal/audit-documentation.py")
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)


def check():
    paths = [ROOT / name for name in ("README.md", "CONTRIBUTING.md")]
    paths += [p for p in (ROOT / "docs").rglob("*")
              if p.is_file() and p.suffix in (".md", ".html", ".svg")
              and "book" not in p.relative_to(ROOT / "docs").parts]
    errors = []
    checked = 0
    for path in sorted(set(paths)):
        for line, target in audit.links(str(path), path.read_text()):
            url = urlsplit(target)
            if url.scheme or url.netloc or not url.path or any(c in target for c in "<>{}"):
                continue
            checked += 1
            dest = path.parent / unquote(url.path)
            if not dest.exists():
                errors.append(f"{path.relative_to(ROOT)}:{line}: missing {target}")
    for error in errors:
        print(error)
    print(f"Documentation links: {checked} local paths, {len(errors)} missing")
    return int(bool(errors))


if __name__ == "__main__":
    sys.exit(check())
