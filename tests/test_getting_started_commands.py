#!/usr/bin/env python3
"""Run the first-spec and first-review commands from docs/getting-started.md.

The bash blocks between the `doc-test:first-run` markers run, in order, in a
new git repository with an empty HOME, after the page's own `aida init` step.
`<ID>` is replaced by the ID `aida add` printed, and greet.py is edited before
the block that commits, as the page tells the reader to do. No network and no
agent CLI: `--run-review` is documented outside the markers.

Usage: python3 tests/test_getting_started_commands.py --aida target/debug/aida
trace:TASK-3960 | ai:claude
"""

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOC = ROOT / "docs" / "getting-started.md"
BEGIN = "<!-- doc-test:first-run begin -->"
END = "<!-- doc-test:first-run end -->"


def documented_section() -> str:
    text = DOC.read_text(encoding="utf-8")
    if text.count(BEGIN) != 1 or text.count(END) != 1:
        sys.exit(f"FAIL - {DOC.name} needs exactly one {BEGIN} / {END} pair")
    return text.split(BEGIN, 1)[1].split(END, 1)[0]


def bash_blocks(section: str) -> list:
    blocks = re.findall(r"```bash\n(.*?)```", section, re.S)
    if len(blocks) < 3:
        sys.exit(f"FAIL - expected the add, show and review blocks, found {len(blocks)}")
    return blocks


def report_labels(section: str) -> list:
    """Bold labels of the bullets that explain the review report."""
    return re.findall(r"^- \*\*(.+?)\*\*:", section, re.M)


def run(cmd: str, cwd: Path, env: dict) -> str:
    proc = subprocess.run(
        ["bash", "-euo", "pipefail", "-c", cmd],
        cwd=cwd, env=env, stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        text=True, timeout=120,
    )
    if proc.returncode != 0:
        sys.exit(f"FAIL - exit {proc.returncode} running:\n{cmd}\n--- output ---\n{proc.stdout}")
    return proc.stdout


def expect(desc: str, needle: str, haystack: str) -> None:
    if needle not in haystack:
        sys.exit(f"FAIL - {desc}: {needle!r} not in output\n--- output ---\n{haystack}")
    print(f"ok   - {desc}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--aida", required=True, help="path to the aida binary")
    args = parser.parse_args()
    aida = Path(args.aida).resolve()
    if not os.access(aida, os.X_OK):
        sys.exit(f"FAIL - {aida} is not an executable aida binary")

    section = documented_section()
    blocks = bash_blocks(section)
    labels = report_labels(section)

    tmp = Path(tempfile.mkdtemp(prefix="aida-getting-started-"))
    try:
        home, repo, bin_dir = tmp / "home", tmp / "project", tmp / "bin"
        for d in (home, repo, bin_dir):
            d.mkdir()
        # The page says `aida`; expose the binary under that name only.
        (bin_dir / "aida").symlink_to(aida)
        env = {
            "HOME": str(home),
            "PATH": f"{bin_dir}:/usr/local/bin:/usr/bin:/bin",
            "TMPDIR": os.environ.get("TMPDIR", "/tmp"),
            "LANG": "C.UTF-8",
        }
        greet = repo / "greet.py"
        greet.write_text('def greet(name):\n    return "Hello, " + name\n')
        run("git init -q -b main && git config user.name 'New User' "
            "&& git config user.email new@example.com "
            "&& git add -A && git commit -qm 'chore: initial'", repo, env)
        # Section 2 of the page: initialize, then keep the scaffold committed
        # so the review below sees only the reader's change.
        run("aida init", repo, env)
        run("git add -A && git commit -qm 'chore: scaffold AIDA' --no-verify", repo, env)
        print("ok   - aida init in a fresh repository")

        spec_id = None
        outputs = []
        for block in blocks:
            if "<ID>" in block:
                if spec_id is None:
                    sys.exit("FAIL - a block uses <ID> before `aida add` printed one")
                block = block.replace("<ID>", spec_id)
            if "git commit" in block:
                greet.write_text(
                    'def greet(name, greeting="Hello"):\n'
                    '    return greeting + ", " + name\n')
            out = run(block, repo, env)
            outputs.append(out)
            if spec_id is None:
                match = re.search(r"^Added: (\S+) - ", out, re.M)
                if match:
                    spec_id = match.group(1)

        add_out, show_out, review_out = outputs[0], outputs[1], outputs[-1]
        expect("aida add prints the documented Added line", "Added: ", add_out)
        expect("aida list shows the new spec", "Greet by name", add_out)
        expect("aida show prints the spec", "Greet by name", show_out)
        expect("aida show prints the acceptance", "optional greeting", show_out)
        expect("review prepare compares the branch", "**Mode**: `range`", review_out)
        expect("review prepare lists the changed file", "greet.py", review_out)
        expect("review prepare includes the spec's acceptance", "optional greeting", review_out)
        if not labels:
            sys.exit("FAIL - the page no longer explains the review report")
        for label in labels:
            expect(f"report has the documented '{label}' section", label, review_out)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    print("PASS - getting-started first spec and first review")
    return 0


if __name__ == "__main__":
    sys.exit(main())
