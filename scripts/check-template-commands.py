#!/usr/bin/env python3
"""Template command lint: every `aida ...` line in a fenced code block of a
template must name a real command and only flags that command accepts.

Skills and commands under `aida-core/templates/` are instructions agents run
verbatim. An invented flag (e.g. `aida list --search`) makes every run of that
skill hit a parse error at that step, so this lint resolves each fenced
`aida ...` line against the live binary's `--help`:

  * the longest command path whose `--help` usage line matches is the command;
  * an unknown top-level command is a failure;
  * every `--flag` on the line must appear in that command's `--help`.

Lines may opt out with a trailing `# lint:allow-unknown` comment (for example
when a template deliberately shows a wrong spelling).

Usage:
  python3 scripts/check-template-commands.py [PATH ...]

PATH defaults to the skills, skills-portable and commands directories under
`aida-core/templates`. Directories are scanned for `*.md`.
The binary is `$AIDA_BIN` (default: `aida` on PATH).

# trace:BUG-1922 | ai:claude
"""
import functools
import os
import re
import shlex
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
AIDA_BIN = os.environ.get("AIDA_BIN", "aida")
ALLOW_MARKER = "lint:allow-unknown"
# Agent-executed instruction surfaces. `templates/docs/` is excluded: its
# fenced blocks are mostly captured terminal output, not commands to run.
DEFAULT_DIRS = ("skills", "skills-portable", "commands")
FENCE_RE = re.compile(r"^\s*(```|~~~)")
WORD_RE = re.compile(r"^[a-z][a-z0-9-]*$")
FLAG_RE = re.compile(r"^--([a-z][a-z0-9-]*)(=.*)?$")
# Shell operators that end the `aida` invocation on a line.
SHELL_STOP = {"|", "||", "&&", ";", ">", ">>", "2>&1", "2>/dev/null", "<", "&"}


def help_text(path):
    try:
        return subprocess.run(
            [AIDA_BIN, *path, "--help"],
            capture_output=True,
            text=True,
            timeout=30,
            env={**os.environ, "NO_COLOR": "1", "AIDA_OUTPUT_FORMAT": "human"},
        ).stdout
    except Exception:
        return ""


@functools.lru_cache(maxsize=None)
def command_help(path):
    """Return `--help` text when `path` is a real command path, else None."""
    text = help_text(path)
    if not text:
        return None
    # Compare word counts rather than spellings: a clap alias (`aida intake`)
    # prints its canonical name (`Usage: aida groom ...`), while a positional
    # swallowed as a word (`aida search keyword`) prints a shorter path.
    m = re.search(r"^Usage:\s+aida((?:\s+[a-z][a-z0-9-]*)*)(?:\s|$)", text, re.M)
    if m and len(m.group(1).split()) == len(path):
        return text
    return None


@functools.lru_cache(maxsize=None)
def builtin_type_words():
    """The requirement-type words `aida <type> list|add` rewrites, read from
    the Rust table so the lint cannot drift from the CLI."""
    src = os.path.join(ROOT, "aida-cli-lib", "src", "knowledge_list.rs")
    try:
        with open(src, encoding="utf-8") as fh:
            text = fh.read()
    except OSError:
        return frozenset()
    m = re.search(r"BUILTIN_TYPE_WORDS:\s*&\[&str\]\s*=\s*&\[(.*?)\];", text, re.S)
    if not m:
        return frozenset()
    # `doc` is a real subcommand and is never rewritten.
    return frozenset(re.findall(r'"([a-z-]+)"', m.group(1))) - {"doc"}


def rewrite_type_alias(args):
    """Mirror the CLI's `aida <type> list|add ...` -> `aida list|add --type <type> ...`."""
    if len(args) >= 2 and args[0] in builtin_type_words() and args[1] in ("list", "add"):
        return [args[1], "--type", args[0], *args[2:]]
    if len(args) == 1 and args[0] in builtin_type_words():
        return ["list", "--type", args[0]]
    return args


@functools.lru_cache(maxsize=None)
def command_flags(path):
    text = command_help(path) or ""
    return set(re.findall(r"--[a-z][a-z0-9-]*", text))


def fenced_aida_lines(text):
    """Yield (lineno, command) for `aida ...` lines inside fenced code blocks."""
    in_fence = False
    pending = None  # (lineno, accumulated text) for `\` continuations
    for lineno, raw in enumerate(text.splitlines(), 1):
        if FENCE_RE.match(raw):
            in_fence = not in_fence
            pending = None
            continue
        if not in_fence:
            continue
        line = raw.strip()
        if pending is not None:
            start, acc = pending
            acc = acc + " " + line.rstrip("\\").strip()
            if line.endswith("\\"):
                pending = (start, acc)
            else:
                pending = None
                yield start, acc
            continue
        if line.startswith("$ "):
            line = line[2:].lstrip()
        if not line.startswith("aida "):
            continue
        if line.endswith("\\"):
            pending = (lineno, line.rstrip("\\").strip())
            continue
        yield lineno, line


def tokenize(command):
    try:
        lexer = shlex.shlex(command, posix=True, punctuation_chars="|&;<>")
        lexer.whitespace_split = True
        lexer.commenters = "#"
        return list(lexer)
    except ValueError:
        return command.split("#", 1)[0].split()


def check_command(command):
    """Return a list of problem strings for one `aida ...` invocation."""
    if ALLOW_MARKER in command:
        return []
    tokens = tokenize(command)
    if not tokens or tokens[0] != "aida":
        return []
    args = []
    for tok in tokens[1:]:
        if tok in SHELL_STOP or set(tok) <= set("|&;<>"):
            break
        args.append(tok)
    args = rewrite_type_alias(args)
    words = []
    for tok in args:
        if not WORD_RE.match(tok):
            break
        words.append(tok)
    if not words:
        return []  # `aida --version`, `aida <cmd>` placeholders, etc.
    path = None
    for n in range(len(words), 0, -1):
        if command_help(tuple(words[:n])) is not None:
            path = tuple(words[:n])
            break
    if path is None:
        return [f"unknown command `aida {words[0]}`"]
    problems = []
    flags = command_flags(path)
    for tok in args:
        m = FLAG_RE.match(tok)
        if m and f"--{m.group(1)}" not in flags:
            problems.append(f"`aida {' '.join(path)}` has no flag `--{m.group(1)}`")
    return problems


def iter_files(paths):
    for p in paths:
        if os.path.isdir(p):
            for dirpath, _dirs, files in os.walk(p):
                for name in sorted(files):
                    if name.endswith(".md"):
                        yield os.path.join(dirpath, name)
        else:
            yield p


def main(argv):
    paths = argv or [
        os.path.join(ROOT, "aida-core", "templates", d) for d in DEFAULT_DIRS
    ]
    if command_help(("search",)) is None:
        print(
            f"check-template-commands: `{AIDA_BIN} search --help` failed; "
            "build the aida binary or set AIDA_BIN",
            file=sys.stderr,
        )
        return 2
    failures = []
    checked = 0
    for path in sorted(iter_files(paths)):
        with open(path, encoding="utf-8") as fh:
            text = fh.read()
        for lineno, command in fenced_aida_lines(text):
            checked += 1
            for problem in check_command(command):
                rel = os.path.relpath(path, ROOT)
                failures.append(f"{rel}:{lineno}: {problem}\n    {command}")
    if failures:
        print("Template commands that the aida CLI rejects:", file=sys.stderr)
        for f in failures:
            print(f"  {f}", file=sys.stderr)
        print(
            f"{len(failures)} problem(s) in {checked} fenced aida command(s). "
            f"Fix the template, or append `# {ALLOW_MARKER}` to a line that is "
            "deliberately invalid.",
            file=sys.stderr,
        )
        return 1
    print(f"OK template commands — {checked} fenced aida command(s) resolve")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
