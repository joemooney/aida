"""Ratchet: test fixtures run write-class git without detached auto-maintenance.

Since git 2.54 every commit/merge/fetch/... runs `git maintenance run --auto
--detach` with the geometric strategy. A background `repack -d` then prunes
loose objects and rmdirs emptied `.git/objects/<xx>` dirs while the fixture's
next git call writes an object ("unable to create temporary file"). Fixtures
avoid it with `-c maintenance.auto=false -c gc.auto=0`, via the per-crate
`test_git` helper (or inline in a shell script). The count of unguarded sites
per file may only go down; `scripts/git-maintenance-baseline.txt` records it.

The scan covers Rust test code (including split statements such as
`let mut c = Command::new("git"); c.arg("commit")` and argv passed through a
variable) and the shell/Python fixtures under `tests/`: a shell script exports
SCRIPT_GUARD_LINE, a Python fixture passes the -c pair in its argv list.

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

# Subcommands that start auto-maintenance in the local repository (commit,
# merge, am, rebase, cherry-pick, revert, pull, fetch), commit-tree (writes
# objects a running repack can race) and push: the remote's receive-pack runs
# auto-maintenance under the remote's own config, which the client's -c pair
# never reaches. test_git covers that through init.templateDir: every repo it
# creates, bare remotes included, has maintenance off in its config.
WRITE_CLASS = ("commit", "commit-tree", "merge", "am", "rebase", "cherry-pick", "revert", "pull", "fetch", "push")
GUARD_TEXT = ("maintenance.auto=false", "NO_AUTO_MAINTENANCE", "test_git::")
# Cheap raw-text filter: a file can only hold a site if it names a write-class
# subcommand as a literal or after `git`.
CANDIDATE = re.compile(
    r'"(?:' + "|".join(map(re.escape, WRITE_CLASS)) + r')"|\bgit\b[^\n]*?\b(?:' + "|".join(map(re.escape, WRITE_CLASS)) + r")\b"
)
SHELL_SITE = re.compile(
    r"\bgit((?:\s+-[cC]\s+\S+|\s+--?[A-Za-z][\w-]*(?:=\S+)?)*)\s+(" + "|".join(map(re.escape, WRITE_CLASS)) + r")(?![\w-])"
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


GITISH = re.compile(r'"(?:[^"\n]*/)?git"|\w*git\w*\s*[(!]|git_|_git\b')


def gitish(source: str, s: int, e: int) -> bool:
    return bool(GITISH.search(source[s:e]))


def statement_guarded(source: str, masked: str, definitions, s: int, e: int) -> bool:
    return any(token in source[s:e] for token in GUARD_TEXT) or definitions.guarded_call_in(masked, s, e)


def git_bindings(source: str, masked: str, definitions, lo: int, before: int):
    """`let [mut] NAME = <git command>` statements in [lo, before): name ->
    (statement start, guarded) for the nearest such binding of each name. A
    later `NAME.args(NO_AUTO_MAINTENANCE)`-style statement also guards it."""
    # trace:TASK-1955 | ai:claude (TASK-2010)
    found = {}
    for m in re.finditer(r"\blet\s+(?:mut\s+)?(\w+)\s*(?::[^=;]*)?=(?!=)", masked[:before]):
        if m.start() < lo:
            continue
        s, e = statement_span(masked, m.end(), lo, before)
        rhs = source[m.end():e]
        command = re.match(r'\s*(?:[\w:]*::)?Command::new\(\s*"(?:[^"\n]*/)?git"', rhs)
        helper = re.match(r"[\s&]*(?:mut\s+)?[\w:.()&\s]*?\b(\w*git\w*)\s*\(", masked[m.end():e])
        if not (command or helper) or e > before:
            continue
        name = m.group(1)
        guarded = statement_guarded(source, masked, definitions, m.end(), e) or bool(
            re.search(r"\b" + re.escape(name) + r"\b[^;]*(?:" + "|".join(map(re.escape, GUARD_TEXT)) + ")", source[e:before])
        )
        found[name] = (m.start(), guarded)
    return found


def git_receiver(source: str, masked: str, definitions, s: int, e: int, lo: int):
    """None when the statement calls methods on no git-command binding; else
    whether that binding is guarded."""
    # trace:TASK-1955 | ai:claude (TASK-2010)
    # A literal reaches a split command only through `.arg(` / `.args(`; a
    # method on the captured output (`held.trim()` of `git_ok(..)`) is not a run.
    if not re.search(r"\.\s*args?\s*\(", masked[s:e]):
        return None
    names = set(re.findall(r"\b(\w+)\s*\.\s*\w+", masked[s:e]))
    if not names:
        return None
    bindings = git_bindings(source, masked, definitions, lo, s)
    hits = [bindings[n][1] for n in names if n in bindings]
    if not hits:
        return None
    return all(hits)


def unguarded_argv_use(source: str, masked: str, definitions, s: int, e: int, lo: int, hi: int) -> bool:
    """The literal sits in `let NAME = [..]`/`vec![..]`; some later statement
    passes NAME to an unguarded git spawn."""
    m = re.match(r"\s*let\s+(?:mut\s+)?(\w+)\b", masked[s:e])
    if not m:
        return False
    name = m.group(1)
    for use in re.finditer(r"\b" + re.escape(name) + r"\b", masked[e:hi]):
        at = e + use.start()
        us, ue = statement_span(masked, at, lo, hi)
        if re.match(r"\s*let\s+(?:mut\s+)?" + re.escape(name) + r"\b", masked[us:ue]):
            break  # rebound: later uses see another value
        if gitish(source, us, ue):
            if not statement_guarded(source, masked, definitions, us, ue):
                return True
            continue
        if git_receiver(source, masked, definitions, us, ue, lo) is False:
            return True
    return False


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
                definitions = definitions or Definitions(source, masked)
                if gitish(source, s, e):
                    if not statement_guarded(source, masked, definitions, s, e):
                        sites.append(f"{line}: git {literal}")
                    continue
                # trace:TASK-1955 | ai:claude (TASK-2010)
                # Split statement: `let mut c = Command::new("git"); c.arg("commit")`.
                binding = git_receiver(source, masked, definitions, s, e, lo)
                if binding is not None:
                    if not binding:
                        sites.append(f"{line}: git {literal}")
                    continue
                # Variable argv: `let a = ["commit", ..]; git(dir, &a)`.
                if unguarded_argv_use(source, masked, definitions, s, e, lo, hi):
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


# trace:TASK-1955 | ai:claude (TASK-2010)
# Shell and Python fixtures under tests/. A shell script turns maintenance off
# for every git it runs with SCRIPT_GUARD_LINE near its top (the environment
# form of the -c pair); a site behind `env -i` loses that environment and
# needs the inline pair. Python fixtures pass the pair in the argv list.
SCRIPT_GUARD_LINE = (
    "export GIT_CONFIG_COUNT=2 GIT_CONFIG_KEY_0=maintenance.auto GIT_CONFIG_VALUE_0=false"
    " GIT_CONFIG_KEY_1=gc.auto GIT_CONFIG_VALUE_1=0"
)
SCRIPT_GUARD = re.compile(r"^[^#\n]*\bGIT_CONFIG_KEY_(\d+)=maintenance\.auto\b", re.M)


def script_guard(source: str):
    """The first SCRIPT_GUARD line that really applies: its GIT_CONFIG_COUNT
    covers the key's index and the matching value turns maintenance off."""
    # trace:TASK-1955 | ai:claude (TASK-2010)
    for m in SCRIPT_GUARD.finditer(source):
        end = source.find("\n", m.end())
        line = source[m.start():end if end >= 0 else len(source)]
        index = int(m.group(1))
        count = re.search(r"\bGIT_CONFIG_COUNT=(\d+)\b", line)
        value = re.search(rf"\bGIT_CONFIG_VALUE_{index}=false\b", line)
        if count and int(count.group(1)) > index and value:
            return m
    return None
PY_SITE = re.compile(
    r"""["']git["']\s*,((?:\s*(?:["'][^"'\n]*["']|[\w.]+)\s*,)*?)\s*["'](""" + "|".join(map(re.escape, WRITE_CLASS)) + r""")["']"""
)


def script_sites(source: str, python: bool = False) -> list[str]:
    sites = []
    lines = source.split("\n")
    if python:
        for number, text in enumerate(lines, 1):
            if text.lstrip().startswith("#"):
                continue
            for m in PY_SITE.finditer(text):
                if "maintenance.auto=false" not in m.group(1):
                    sites.append(f"{number}: py git {m.group(2)}")
        return sites
    guard = script_guard(source)
    guard_line = source.count("\n", 0, guard.start()) + 1 if guard else None
    number = 0
    while number < len(lines):
        first = number + 1
        text = lines[number]
        while text.endswith("\\") and number + 1 < len(lines):
            number += 1
            text = text[:-1] + " " + lines[number]
        number += 1
        if text.lstrip().startswith("#"):
            continue
        for m in SHELL_SITE.finditer(text):
            if "maintenance.auto=false" in m.group(1):
                continue
            prefix = text[:m.start()]
            # Quoted text (an assertion message, a grep pattern, JSON) is not
            # a git run; `$(` starts a command again inside double quotes.
            inner = prefix.rsplit("$(", 1)[-1]
            if inner.count("'") % 2 or inner.count('"') % 2:
                continue
            command = re.split(r";|&&|\|\||\||\(", prefix)[-1]
            if re.match(r"\s*(?:echo|printf)\b", command):
                continue
            if guard_line is not None and guard_line < first and not re.search(r"\benv\s+-i\b", prefix):
                continue
            sites.append(f"{first}: sh git {m.group(2)}")
    return sites


def script_paths() -> list[Path]:
    paths = sorted((ROOT / "tests").glob("*.sh")) + sorted((ROOT / "tests/lib").glob("*.sh"))
    paths += sorted(p for p in (ROOT / "tests").glob("*.py") if p.resolve() != Path(__file__).resolve())
    return paths


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
    for path in script_paths():
        sites = script_sites(path.read_text(encoding="utf-8"), python=path.suffix == ".py")
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
    # trace:BUG-2191.ac7c1671 | ai:claude
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

    # trace:BUG-2191.ac7c1671 | ai:claude
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

    # trace:BUG-2191.ac7c1671 | ai:claude
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

    # trace:BUG-2191.ac7c1671 | ai:claude
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

    # trace:BUG-2191.ac7c1671 | ai:claude
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

    # trace:BUG-2191.ac7c1671 | ai:claude
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

    # trace:TASK-1955 | ai:claude (TASK-2010)
    def test_detects_split_statements_and_variable_argv(self):
        source = '''
fn t() {
    let mut c = Command::new("git");
    c.current_dir(dir);
    c.args(["commit", "-m", "x"]);
    let args = vec!["merge", "--no-ff", "side"];
    git(dir, &args);
    let fetch = ["fetch", "origin"];
    std::process::Command::new("/usr/bin/git").args(fetch).status();
    let kind = "commit";
    assert_eq!(kind, label);
}
'''
        self.assertEqual(["5: git commit", "6: git merge", "8: git fetch"], self.sites(source))
        guarded = (
            source.replace('Command::new("git")', "crate::test_git::git_command()")
            .replace("git(dir, &args)", "crate::test_git::git_command().current_dir(dir).args(&args)")
            .replace(".args(fetch)", ".args(NO_AUTO_MAINTENANCE).args(fetch)")
        )
        self.assertEqual([], self.sites(guarded))
        # A binding guarded by a later statement on it is guarded too.
        later = '''
fn t() {
    let mut c = Command::new("git");
    c.args(crate::git_ops::NO_AUTO_MAINTENANCE);
    c.arg("commit");
}
'''
        self.assertEqual([], self.sites(later))
        # Output captured from a git helper is a string, not a command.
        captured = '''
fn t() {
    let held = git_ok(dir, &["cat-file", "-t", sha]);
    assert_eq!(held.trim(), "commit");
}
'''
        self.assertEqual([], self.sites(captured))

    # trace:TASK-1955 | ai:claude (TASK-2010)
    def test_detects_unguarded_shell_and_python_fixture_sites(self):
        script = '''#!/bin/bash
set -euo pipefail
git commit -qm seed
git -C "$R" merge-base --is-ancestor a b
info "Test 4: release exports AIDA_RELEASE=1 before git commit"
grep -n '^git push origin' "$SCRIPT"
out="$(git fetch origin 2>&1)"
env -i PATH="$PATH" \\
    git -c user.name=t commit -qm hook
# git commit in a comment
'''
        self.assertEqual(
            ["3: sh git commit", "7: sh git fetch", "8: sh git commit"], script_sites(script)
        )
        guarded = script.replace("set -euo pipefail\n", "set -euo pipefail\n" + SCRIPT_GUARD_LINE + "\n")
        # The exported pair covers every site but the one behind `env -i`.
        self.assertEqual(["9: sh git commit"], script_sites(guarded))
        # A pair the count does not reach, or a value that leaves maintenance
        # on, guards nothing.
        for broken in (
            SCRIPT_GUARD_LINE.replace("GIT_CONFIG_COUNT=2", "GIT_CONFIG_COUNT=0"),
            SCRIPT_GUARD_LINE.replace("GIT_CONFIG_VALUE_0=false", "GIT_CONFIG_VALUE_0=true"),
        ):
            unguarded = script.replace("set -euo pipefail\n", "set -euo pipefail\n" + broken + "\n")
            self.assertEqual(["4: sh git commit", "8: sh git fetch", "9: sh git commit"], script_sites(unguarded))
        inline = guarded.replace("git -c user.name=t", "git -c maintenance.auto=false -c gc.auto=0 -c user.name=t")
        self.assertEqual([], script_sites(inline))
        python = '''
subprocess.run(["git", "commit", "-qm", "x"], cwd=root, check=True)
subprocess.run(["git", "-c", "commit.gpgsign=false", "commit", "-qm", "x"])
subprocess.run(["git", "-c", "maintenance.auto=false", "-c", "gc.auto=0", "commit", "-qm", "x"])
subprocess.run(["git", "merge-base", "a", "b"])
'''
        self.assertEqual(["2: py git commit", "3: py git commit"], script_sites(python, python=True))


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
