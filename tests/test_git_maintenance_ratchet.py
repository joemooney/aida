"""Ratchet: test fixtures run write-class git without detached auto-maintenance.

Since git 2.54 every commit/merge/fetch/... runs `git maintenance run --auto
--detach` with the geometric strategy. A background `repack -d` then prunes
loose objects and rmdirs emptied `.git/objects/<xx>` dirs while the fixture's
next git call writes an object ("unable to create temporary file"). Fixtures
avoid it with `-c maintenance.auto=false -c gc.auto=0`, via the per-crate
`test_git` helper (or inline in a shell script). The count of unguarded sites
per file may only go down; `scripts/git-maintenance-baseline.txt` records it.

Run `python3 tests/test_git_maintenance_ratchet.py --print` to list the sites,
`--write-baseline` after migrating fixtures.
"""
# trace:BUG-2191 | ai:claude

from __future__ import annotations

from pathlib import Path
import bisect
import re
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]
BASELINE = ROOT / "scripts/git-maintenance-baseline.txt"
CRATES = ["aida-core", "aida-cli-lib", "aida-cli", "aida-server", "aida-tui"]

# Subcommands that start auto-maintenance (git's run_auto_maintenance callers)
# plus commit-tree, which writes objects a running repack can race.
WRITE_CLASS = ("commit", "commit-tree", "merge", "am", "rebase", "cherry-pick", "revert", "pull", "fetch")
GUARD_TEXT = ("maintenance.auto=false", "NO_AUTO_MAINTENANCE", "test_git::")
# Cheap raw-text filter: a file can only hold a site if it names a write-class
# subcommand as a literal or after `git`.
CANDIDATE = re.compile(
    r'"(?:' + "|".join(map(re.escape, WRITE_CLASS)) + r')"|\bgit\b[^\n]*?\b(?:' + "|".join(map(re.escape, WRITE_CLASS)) + r")\b"
)
SHELL_SITE = re.compile(
    r"\bgit((?:\s+-[cC]\s+\S+|\s+--?[A-Za-z][\w-]*(?:=\S+)?)*)\s+(" + "|".join(map(re.escape, WRITE_CLASS)) + r")\b"
)


RAW_OPEN = re.compile(r'r(#*)"')
CHAR_LIT = re.compile(r"'(?:\\.|\\u\{[0-9a-fA-F]+\}|[^\\'])'")
ESCAPES = re.compile(r"\\[nrt]")


TOKEN = re.compile(r"""//|/\*|(?<![\w])r#*"|"|'""")


NOT_NEWLINE = re.compile(r"[^\n]")


def mask(source: str) -> tuple[str, list[tuple[int, int]]]:
    """Blank comments and literal contents (keeping offsets); return the masked
    text and the (start, end) span of every string literal's contents."""
    out = []
    strings = []
    i, n = 0, len(source)

    def blank(a: int, b: int) -> str:
        return NOT_NEWLINE.sub(" ", source[a:b])

    while i < n:
        m = TOKEN.search(source, i)
        if not m:
            out.append(source[i:])
            break
        out.append(source[i:m.start()])
        tok, i = m.group(), m.start()
        if tok == "//":
            j = source.find("\n", i)
            j = n if j < 0 else j
            out.append(blank(i, j))
        elif tok == "/*":
            j = source.find("*/", i + 2)
            j = n if j < 0 else j + 2
            out.append(blank(i, j))
        elif tok.startswith("r"):
            hashes = tok[1:-1]
            body = m.end()
            close = source.find('"' + hashes, body)
            close = n if close < 0 else close
            strings.append((body, close))
            j = min(n, close + 1 + len(hashes))
            out.append(source[i:body] + blank(body, close) + source[close:j])
        elif tok == '"':
            j = i + 1
            while j < n and source[j] != '"':
                j += 2 if source[j] == "\\" else 1
            strings.append((i + 1, j))
            out.append('"' + blank(i + 1, j) + source[j:j + 1])
            j += 1
        else:
            c = CHAR_LIT.match(source, i)
            if c:
                j = c.end()
                out.append("'" + blank(i + 1, j - 1) + "'")
            else:
                j = i + 1
                out.append("'")
        i = j
    return "".join(out), strings


def block_end(masked: str, open_brace: int) -> int:
    depth = 0
    for k in range(open_brace, len(masked)):
        if masked[k] == "{":
            depth += 1
        elif masked[k] == "}":
            depth -= 1
            if depth == 0:
                return k + 1
    return len(masked)


def test_regions(path: Path, source: str, masked: str) -> list[tuple[int, int]]:
    """Whole file for test-only files; otherwise each `#[cfg(test)]` item."""
    rel = path.relative_to(ROOT).as_posix()
    if "/tests/" in rel or path.name.startswith("test_") or path.name.endswith("_tests.rs"):
        return [(0, len(source))]
    regions = []
    for m in re.finditer(r"#\[cfg\((?:test|all\(test[^\]]*|any\(test[^\]]*)\)\]", masked):
        rest = masked[m.end():]
        item = re.match(r"(?:\s*#\[[^\]]*\])*\s*[^;{]*", rest)
        stop = m.end() + item.end()
        if stop < len(masked) and masked[stop] == "{":
            regions.append((m.start(), block_end(masked, stop)))
    return regions


def statement_span(masked: str, at: int, lo: int, hi: int) -> tuple[int, int]:
    """The enclosing statement: back to the previous `;`/`{`/`}` boundary that
    is not inside brackets opened after it, forward to the next `;`."""
    depth, k = 0, at
    while k > lo:
        ch = masked[k - 1]
        if ch in ")]":
            depth += 1
        elif ch in "([":
            if depth == 0:
                pass
            else:
                depth -= 1
        elif ch in ";{}" and depth == 0:
            break
        k -= 1
    start = k
    depth, k = 0, at
    while k < hi:
        ch = masked[k]
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            if depth == 0:
                break
            depth -= 1
        elif ch == ";" and depth == 0:
            break
        k += 1
    return start, k


CALL = re.compile(r"\b(\w+)\s*\(")


class Definitions:
    """This file's fns and `let name = |..|` closures. A call resolves to the
    nearest definition of its name before it (else the nearest after: helper
    fns often sit at the bottom), which approximates Rust's lexical scoping.
    A definition is guarded when its body applies the guard or calls a
    guarded definition."""

    def __init__(self, source: str, masked: str):
        defs = []  # (name, offset, body_start, body_end)
        for m in re.finditer(r"\bfn\s+(\w+)\s*(?:<[^>]*>)?\s*\(", masked):
            brace = masked.find("{", m.end())
            semi = masked.find(";", m.end())
            if brace < 0 or (0 <= semi < brace):
                continue
            defs.append((m.group(1), m.start(), brace, block_end(masked, brace)))
        for m in re.finditer(r"\blet\s+(?:mut\s+)?(\w+)\s*=\s*(?:move\s*)?\|", masked):
            _, end = statement_span(masked, m.end(), 0, len(masked))
            defs.append((m.group(1), m.start(), m.end(), end))
        self.by_name = {}
        for d in sorted(defs, key=lambda d: d[1]):
            self.by_name.setdefault(d[0], []).append(d)
        self.guarded = {d for d in defs if any(t in source[d[2]:d[3]] for t in GUARD_TEXT)}
        self.calls = {d: [(c.group(1), d[2] + c.start()) for c in CALL.finditer(masked, d[2], d[3])] for d in defs}
        while True:
            grown = {
                d for d in defs
                if d not in self.guarded
                and any(self.resolve(name, at) in self.guarded for name, at in self.calls[d])
            }
            if not grown:
                break
            self.guarded |= grown

    def resolve(self, name: str, at: int):
        candidates = self.by_name.get(name)
        if not candidates:
            return None
        before = [d for d in candidates if d[1] < at]
        return before[-1] if before else candidates[0]

    def guarded_call_in(self, masked: str, start: int, end: int) -> bool:
        return any(
            self.resolve(c.group(1), start + c.start()) in self.guarded
            for c in CALL.finditer(masked, start, end)
        )


def looks_like_script(literal: str, source: str, masked: str, start: int, lo: int, hi: int) -> bool:
    """A string that is run, not one asserted on: a script body, or an
    argument next to `sh`/`bash`/`-c`."""
    joined = re.sub(r"\\\n\s*", "", literal)  # Rust `\<newline>` continuations
    if "\n" in joined or "\\n" in joined or joined.startswith("#!"):
        return True
    s, e = statement_span(masked, start, lo, hi)
    return bool(re.search(r'"(?:/bin/)?(?:sh|bash)"|"-c"', source[s:e]))


def file_sites(path: Path, source: str, masked: str, strings) -> list[str]:
    definitions = None
    newlines = [k for k, ch in enumerate(source) if ch == "\n"]
    line_of = lambda offset: bisect.bisect_left(newlines, offset) + 1
    sites = []
    for lo, hi in test_regions(path, source, masked):
        for start, end in strings:
            if not (lo <= start and end <= hi):
                continue
            literal = source[start:end]
            line = line_of(start)
            if literal in WRITE_CLASS:
                s, e = statement_span(masked, start, lo, hi)
                stmt = source[s:e]
                if not re.search(r'"git"|\w*git\w*\s*[(!]|git_|_git\b', stmt):
                    continue
                if any(token in stmt for token in GUARD_TEXT):
                    continue
                definitions = definitions or Definitions(source, masked)
                if definitions.guarded_call_in(masked, s, e):
                    continue
                sites.append(f"{line}: git {literal}")
            elif looks_like_script(literal, source, masked, start, lo, hi):
                # A `\n` escape ends a shell line; "\n " keeps the length (offsets).
                script = ESCAPES.sub("\n ", literal)
                for m in SHELL_SITE.finditer(script):
                    if "maintenance.auto=false" in m.group(1):
                        continue
                    # Single-quoted text, or what `echo`/`printf` prints, is
                    # not a git run.
                    line_prefix = script[:m.start()].split("\n")[-1]
                    if line_prefix.count("'") % 2:
                        continue
                    command = re.split(r";|&&|\|\||\||\(", line_prefix)[-1]
                    if re.match(r"\s*(?:echo|printf)\b", command):
                        continue
                    sites.append(f"{line_of(start + m.start())}: sh git {m.group(2)}")
    return sites


def scan() -> dict[str, list[str]]:
    sources = {}
    for crate in CRATES:
        for path in sorted((ROOT / crate).rglob("*.rs")):
            if "target" in path.parts:
                continue
            source = path.read_text(encoding="utf-8")
            if CANDIDATE.search(source):
                sources[path] = (source, *mask(source))
    result = {}
    for path, (source, masked, strings) in sources.items():
        sites = file_sites(path, source, masked, strings)
        if sites:
            result[path.relative_to(ROOT).as_posix()] = sites
    return result


def read_baseline() -> dict[str, int]:
    counts = {}
    for line in BASELINE.read_text(encoding="utf-8").splitlines():
        if line.strip() and not line.startswith("#"):
            count, path = line.split(maxsplit=1)
            counts[path] = int(count)
    return counts


def write_baseline(found: dict[str, list[str]]) -> None:
    lines = [
        "# Unguarded write-class git spawns in test sources, per file (BUG-2191).",
        "# Counts may only go down: migrate fixtures to test_git and rerun",
        "# `python3 tests/test_git_maintenance_ratchet.py --write-baseline`.",
    ]
    lines += [f"{len(sites)} {path}" for path, sites in sorted(found.items())]
    BASELINE.write_text("\n".join(lines) + "\n", encoding="utf-8")


class GitMaintenanceRatchetTests(unittest.TestCase):
    def test_unguarded_write_class_git_sites_only_go_down(self):
        baseline = read_baseline()
        found = scan()
        grown, stale = [], []
        for path in sorted(set(found) | set(baseline)):
            now, was = len(found.get(path, [])), baseline.get(path, 0)
            if now > was:
                grown.append(f"{path}: {now} > baseline {was}\n    " + "\n    ".join(found[path]))
            elif now < was:
                stale.append(f"{path}: {now} < baseline {was}")
        self.assertEqual(
            [], grown,
            "New test git commit/merge/fetch/... without `-c maintenance.auto=false -c gc.auto=0`."
            " Spawn it through the crate's test_git helper (or add the -c pair to the shell"
            " script):\n" + "\n".join(grown),
        )
        self.assertEqual(
            [], stale,
            "Fewer unguarded sites than the baseline records; lower it with"
            " `python3 tests/test_git_maintenance_ratchet.py --write-baseline`:\n" + "\n".join(stale),
        )

    def sites(self, source: str, name: str = "aida-cli-lib/src/tests/x_tests.rs") -> list[str]:
        masked, strings = mask(source)
        return file_sites(Path(ROOT / name), source, masked, strings)

    def test_detects_direct_wrapped_and_shell_sites(self):
        source = '''
fn git(dir: &Path, args: &[&str]) { Command::new("git").current_dir(dir).args(args).status().unwrap(); }
fn t() {
    Command::new("git").args(["commit", "-m", "x"]).status().unwrap();
    git(
        dir,
        &["merge", "--no-ff", "side"],
    );
    let script = "#!/bin/sh\\ngit -C repo commit -qm msg\\n";
    git(dir, &["status"]);
    let kind = "commit";
    assert!(recipe.contains("git fetch origin"));
    let fake = "#!/bin/sh\\necho 'recover with git fetch origin'\\n";
    let msg = "should advise `git rebase --abort` \\
               when a rebase is in progress";
}
'''
        self.assertEqual(["4: git commit", "7: git merge", "9: sh git commit"], self.sites(source))

    def test_guard_helper_and_inline_guard_are_accepted(self):
        source = '''
fn git(dir: &Path, args: &[&str]) { crate::test_git::git_command().current_dir(dir).args(args).status().unwrap(); }
fn t() {
    git(dir, &["commit", "-m", "x"]);
    crate::test_git::git_command().args(["merge", "side"]).status().unwrap();
    Command::new("git").args(["-c", "maintenance.auto=false", "-c", "gc.auto=0", "fetch", "o"]);
    let script = "git -c maintenance.auto=false -c gc.auto=0 commit -qm msg";
}
'''
        self.assertEqual([], self.sites(source))

    def test_guard_reaches_callers_transitively_but_not_namesakes(self):
        source = '''
const NO_AUTO_MAINTENANCE: [&str; 4] = ["-c", "maintenance.auto=false", "-c", "gc.auto=0"];
fn fixture_git(dir: &Path) -> Command { let mut c = Command::new("git"); c.args(NO_AUTO_MAINTENANCE); c }
fn git_in(dir: &Path, args: &[&str]) -> String { run(fixture_git(dir).args(args)) }
impl Fx { fn git(&self, args: &[&str]) -> String { git_in(&self.dir, args) } }
fn t(fx: &Fx) { fx.git(&["commit", "-m", "x"]); }
'''
        self.assertEqual([], self.sites(source))
        # An unguarded namesake elsewhere in the file stays unguarded.
        other = '''
fn git(dir: &Path, args: &[&str]) { Command::new("git").args(args).status(); }
fn t() { git(dir, &["commit", "-m", "x"]); }
'''
        self.assertEqual(["3: git commit"], self.sites(other))

    def test_closures_follow_the_same_guard_rule(self):
        source = '''
fn a() {
    let git = |args: &[&str]| crate::test_git::git_command().args(args).status();
    git(&["commit", "-m", "x"]);
}
fn b() {
    let git = |args: &[&str]| Command::new("git").args(args).status();
    git(&["commit", "-m", "x"]);
}
'''
        self.assertEqual(["4: git commit", "8: git commit"], self.sites(source))
        guarded = source.replace('Command::new("git")', "crate::test_git::git_command()")
        self.assertEqual([], self.sites(guarded))

    def test_only_cfg_test_items_count_in_production_files(self):
        source = '''
fn prod() { Command::new("git").args(["commit"]).status(); }
#[cfg(test)]
mod tests {
    fn t() { Command::new("git").args(["commit"]).status(); }
}
fn later_prod() { Command::new("git").args(["merge"]).status(); }
'''
        self.assertEqual(["5: git commit"], self.sites(source, "aida-core/src/x.rs"))


if __name__ == "__main__":
    if "--print" in sys.argv or "--write-baseline" in sys.argv:
        found = scan()
        if "--write-baseline" in sys.argv:
            write_baseline(found)
        for path, sites in sorted(found.items(), key=lambda kv: -len(kv[1])):
            print(f"{len(sites):4} {path}")
        print(f"total {sum(map(len, found.values()))} in {len(found)} files")
    else:
        unittest.main()
