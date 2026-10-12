#!/usr/bin/env python3
"""Whole-stack protocol checker for the STORY-1488 lib.rs split slices.

usage: verify_stack.py <base> <head> [--build]

Checks every commit in base..head (oldest first) against the pipelined slice
protocol (TASK-1538 comments, 2026-10-10 plus the rule-7 amendment):
  1 class      subject is a MOVE (`refactor(cli-lib): extract <m>.rs`, or the
               append form `refactor(cli-lib): extend <m>.rs`), a
               `retarget:` / `tooling:` commit, or the final slice (detected
               from the diff: it deletes every remaining lib_part*.rs)
  2 spec-id    no spec ID in any subject except the final slice
  3 purity     MOVE/final: removed lines == added lines, apart from wiring that
               sits in its own file; only move-set files change; each new module
               is wired exactly once in lib.rs (plus amendment-1n `pub use
               <m>::{..};` shims naming only bare-`pub` items this MOVE removes
               from lib*.rs); lib*.rs only LOSES lines (bar
               lib.rs wiring); each new module receives moved lines; every
               removed -U0 block lands contiguous and in order in one module
               (blocks may reorder among themselves; a final-slice block may
               split only at a module seam). An extend appends to an
               existing <m>.rs: the parent's file is a byte prefix of the new
               one, it adds no module, no lib.rs mod/glob line (1n shims only)
               and no header, and the appended chunk has no `mod ` declaration
               or `#[path` attribute that it does not remove verbatim from
               lib*.rs in the same commit
  4 retarget   retarget:/tooling: commits change no code line in lib*.rs or in
               a module moved earlier in the range; a retarget may widen a
               source-scan corpus (signed amendment 1e) by adding
               `include_str!("<moved>.rs")` and its `"{}\\n…{}"` placeholder
               string (reported as NOTE 4)
  5 pairing    every retarget: commit directly follows a MOVE
  6 tags       every slice-NN-rR tag in the range is annotated and carries
               numeric proofs: `warnings <n>[/<n>]`, `tests <n>/<n>`, and
               `<m>.rs=<lines>` for each module its segment adds (the real line
               count at the tagged commit); every MOVE is covered by a tag
  7 size       every new module is <= 6000 lines
  8 linearity  head descends from base; no merge commits in the range
  9 build      with --build: `cargo check -p aida-cli-lib` at each MOVE/final
               commit in a temporary detached worktree with its own target dir

Read-only: never edits, tags, pushes or merges. Works from refs only.
Exit status is 0 only if every check passes.

See also: verify_pr.sh (per-slice build/test/golden gate).

# trace:TASK-1722 | ai:antigravity
# trace:TASK-1722 | ai:claude
"""

import collections
import os
import re
import shutil
import subprocess
import sys
import tempfile

SRC = "aida-cli-lib/src/"
LIB_RS = SRC + "lib.rs"
MANIFEST = "docs/plans/2026-10-04-story-1488-incremental-manifest.toml"
SIZE_CAP = 6000

MOVE_SUBJECT = re.compile(r"^refactor\(cli-lib\): (extract|extend) ([a-z0-9_]+)\.rs\b")
# trace:TASK-1538 | ai:claude
# An extend's appended text must not pull in another file as a submodule.
EXTEND_FORBIDDEN = re.compile(r"^\s*(#\[path\b|(pub(\([^)]*\))? )?mod \w+)")
# Same set as the pre-commit hook's SPEC_ID_RE (aida-pre-commit.sh).
SPEC_ID = re.compile(r"(^|[^A-Za-z0-9_])(STORY|TASK|BUG|EPIC|SPIKE|FR|CR|SPEC|ADR|PRIN|DOC)-[0-9]+")
LIB_PART = re.compile(r"^aida-cli-lib/src/lib_part[0-9]+\.rs$")
LIB_ANY = re.compile(r"^aida-cli-lib/src/lib(_part[0-9]+)?\.rs$")
NEW_MODULE = re.compile(r"^aida-cli-lib/src/([a-z0-9_]+)\.rs$")
SLICE_TAG = re.compile(r"^slice-[0-9]+-r[0-9]+$")
V3 = re.compile(r"\bwarnings\s+\d+(/\d+)?(?![/\w])", re.I)
V4 = re.compile(r"\btests\s+\d+/\d+(?![/\w])", re.I)
V7 = re.compile(r"\b([a-z0-9_]+)\.rs=(\d+)(?![/\w])")
HEADER_WIRING = re.compile(r"^(//!.*|// trace:.*|use crate::\*;)$")

CHECKS = [(1, "class"), (2, "spec-id"), (3, "purity"), (4, "retarget"), (5, "pairing"),
          (6, "tags"), (7, "size"), (8, "linearity"), (9, "build")]


class GitError(RuntimeError):
    pass


def git(*args, check=True):
    res = subprocess.run(["git", *args], capture_output=True, text=True)
    if check and res.returncode != 0:
        raise GitError(f"git {' '.join(args)} failed: {res.stderr.strip()}")
    return res.stdout


def line_count(rev, path):
    try:
        return git("show", f"{rev}:{path}").count("\n")
    except GitError:
        return None


def lib_parts(rev):
    return [p for p in git("ls-tree", "-r", "--name-only", rev, SRC).split() if LIB_PART.match(p)]


def name_status(sha):
    """[(status, path)] for a non-merge commit, renames split into D+A."""
    out = git("diff-tree", "--no-commit-id", "-r", "--no-renames", "--name-status", sha)
    return [tuple(l.split("\t", 1)) for l in out.splitlines() if l]


def diff_lines(sha):
    """Yield (file, '+'|'-', text) for every changed line of the commit."""
    cur = ""
    for line in git("show", "--format=", "-U0", "--no-color", "--no-renames", sha).splitlines():
        if line.startswith("diff --git "):
            cur = line.split(" b/", 1)[-1]
        elif line.startswith(("+++", "---", "@@", "\\")):
            continue
        elif line[:1] in "+-":
            yield cur, line[0], line[1:]


def diff_hunks(sha):
    """Yield (file, sign, [lines]) for every contiguous same-sign run of the -U0 diff."""
    cur, run = "", None
    for line in git("show", "--format=", "-U0", "--no-color", "--no-renames", sha).splitlines():
        if line.startswith("diff --git "):
            cur = line.split(" b/", 1)[-1]
        if line[:1] in "+-" and not line.startswith(("+++", "---")):
            if run and run[0] == cur and run[1] == line[0]:
                run[2].append(line[1:])
                continue
            if run:
                yield run
            run = (cur, line[0], [line[1:]])
        elif line.startswith(("diff --git ", "@@")) and run:
            yield run
            run = None
    if run:
        yield run


def split_block(block, bodies):
    """Cover `block` greedily with maximal runs found contiguously in `bodies`.

    Returns [(module, length)], or None if some line lands nowhere. A block
    lands intact when every piece goes to a different module than the piece
    before it: a split is only legitimate at a module seam (the final slice
    deletes whole lib_part files whose items fan out to several modules).
    """
    index = {m: collections.defaultdict(list) for m in bodies}
    for m, body in bodies.items():
        for pos, l in enumerate(body):
            index[m][l].append(pos)
    pieces, i = [], 0
    while i < len(block):
        best = (None, 0)
        for m, body in bodies.items():
            for pos in index[m].get(block[i], ()):
                k = 0
                while i + k < len(block) and pos + k < len(body) and body[pos + k] == block[i + k]:
                    k += 1
                if k > best[1]:
                    best = (m, k)
        if not best[1]:
            return None
        pieces.append(best)
        i += best[1]
    return pieces


PUB_DECL = re.compile(r"pub (?:(?:async|const|unsafe|extern \"C\") )*"
                      r"(?:fn|struct|enum|trait|type|const|static|union|mod) (\w+)")


# trace:TASK-1538 | ai:claude
def moved_pub_items(sha):
    """Names declared bare `pub` (not pub(crate)) at the root of lines the
    commit removes from lib*.rs: the only names an amendment-1n shim may name."""
    return frozenset(m.group(1) for path, sign, text in diff_lines(sha)
                     if sign == "-" and LIB_ANY.match(path)
                     for m in [PUB_DECL.match(text)] if m)


def wiring_ok(path, text, modules, deleted_parts, shim_names=frozenset(), extend=False):
    """Is this added/removed line allowed wiring for the file it sits in?

    An extend's module is already wired and headed: only 1n shims are allowed.
    """
    s = text.strip()
    if path == LIB_RS:
        for m in modules:
            if not extend and re.fullmatch(r"(pub\(crate\) )?(mod {0};|use {0}::\*;)".format(m), s):
                return True
            # Amendment 1n: a moved bare-`pub` item keeps its crate-root path
            # through a `pub use` shim naming that module; it must not widen
            # the API by re-exporting a pub(crate) or unmoved item.
            shim = re.fullmatch(r"pub use {0}::(\w+|\{{(\w+(?:, \w+)*)\}});".format(m), s)
            if shim:
                names = (shim.group(2) or shim.group(1)).split(", ")
                return all(n in shim_names for n in names)
        return any(s == f'include!("{os.path.basename(p)}");' for p in deleted_parts)
    m = NEW_MODULE.match(path)
    return bool(not extend and m and m.group(1) in modules and HEADER_WIRING.match(s))


# trace:TASK-1538 | ai:claude
def check_extend(sha, module):
    """Append-only rules for `refactor(cli-lib): extend <m>.rs`."""
    path = f"{SRC}{module}.rs"
    status = dict((p, s) for s, p in name_status(sha))
    if status.get(path) != "M":
        return [f"FAIL 3: extend must modify an existing {path} (status {status.get(path, 'unchanged')})"]
    old = git("show", f"{sha}^:{path}")
    new = git("show", f"{sha}:{path}")
    if not new.startswith(old):
        return [f"FAIL 3: extend edits already-moved text in {path} (parent is not a prefix)"]
    # A declaration the commit removes verbatim from lib*.rs moves with its
    # owner (e.g. a `#[path = "tests/..."]` test module); any other is new.
    carried = collections.Counter(text.strip() for p, sign, text in diff_lines(sha)
                                  if sign == "-" and LIB_ANY.match(p) and EXTEND_FORBIDDEN.match(text))
    issues = []
    for l in new[len(old):].splitlines():
        if EXTEND_FORBIDDEN.match(l):
            if carried[l.strip()]:
                carried[l.strip()] -= 1
                continue
            issues.append(f"FAIL 3: extend appends a submodule declaration: {l.strip()[:80]}")
            break
    return issues


def check_purity(sha, modules, deleted_parts, allowed_files, extend=False):
    issues = []
    files = [p for _, p in name_status(sha)]
    odd = [f for f in files if f not in allowed_files and not LIB_ANY.match(f)]
    if odd:
        issues.append(f"FAIL 3: file outside move set: {', '.join(odd)}")

    shims = moved_pub_items(sha)
    wiring = lambda path, text: wiring_ok(path, text, modules, deleted_parts, shims, extend)
    added, removed = collections.Counter(), collections.Counter()
    ok_add, ok_rem = collections.Counter(), collections.Counter()
    lib_adds = collections.Counter()
    for path, sign, text in diff_lines(sha):
        if path == MANIFEST:
            continue  # manifest entries are declarations, not moved code
        if sign == "+":
            added[text] += 1
            ok_add[text] += wiring(path, text)
            if path == LIB_RS:
                lib_adds[text.strip()] += 1
        else:
            removed[text] += 1
            ok_rem[text] += wiring(path, text)

    def residue(mine, other, excused):
        out = collections.Counter()
        for l, n in (mine - other).items():
            if not l.strip():
                continue  # blank lines are layout, not code
            left = max(0, n - excused[l])
            if left:
                out[l] = left
        return out

    extra_add = residue(added, removed, ok_add)
    extra_rem = residue(removed, added, ok_rem)
    if extra_add or extra_rem:
        norm = lambda c: collections.Counter({re.sub(r"\s+", " ", l.strip()): n for l, n in c.items()})
        if norm(extra_add) == norm(extra_rem):
            issues.append("FAIL 3: residue is whitespace-only (reindent)")
        else:
            issues.append("FAIL 3: added/removed lines do not match a pure move")
        for l, n in list(extra_add.items())[:10]:
            issues.append(f"        + x{n} {l}")
        for l, n in list(extra_rem.items())[:10]:
            issues.append(f"        - x{n} {l}")

    # A move only takes lines OUT of lib*.rs: a non-wiring line added there is
    # code relocated between root files, not a move.
    for path, sign, text in diff_lines(sha):
        if sign == "+" and LIB_ANY.match(path) and text.strip() \
                and not wiring(path, text):
            issues.append(f"FAIL 3: non-wiring line added to {path}: {text.strip()[:80]}")
            break

    # Each removed block must land intact: its lines contiguous and in the same
    # order inside one new module (blocks may be reordered relative to each other).
    landed = collections.defaultdict(list)
    removed_blocks = []
    for path, sign, lines in diff_hunks(sha):
        if sign == "+":
            m = NEW_MODULE.match(path)
            if m and m.group(1) in modules:
                # keep header-shaped lines: moved code may carry its own `// trace:`
                landed[m.group(1)] += [l for l in lines if l.strip()]
        elif LIB_ANY.match(path):
            block = [l for l in lines if l.strip() and not wiring(path, l)]
            if block:
                removed_blocks.append((path, block))
    for m in sorted(modules):
        if all(wiring(f"{SRC}{m}.rs", l) for l in landed[m]):
            issues.append(f"FAIL 3: {m}.rs receives no moved lines (empty extraction)")
    for path, block in removed_blocks:
        pieces = split_block(block, landed)
        if pieces is None or any(a[0] == b[0] for a, b in zip(pieces, pieces[1:])):
            issues.append(f"FAIL 3: block removed from {path} does not land intact and in order "
                          f"(starts: {block[0].strip()[:60]})")

    for m in sorted(modules):
        mods = sum(n for l, n in lib_adds.items() if re.fullmatch(rf"(pub\(crate\) )?mod {m};", l))
        uses = sum(n for l, n in lib_adds.items() if re.fullmatch(rf"(pub\(crate\) )?use {m}::\*;", l))
        if extend and (mods or uses):
            issues.append(f"FAIL 3: extend re-wires {m} in lib.rs (mod x{mods}, use x{uses})")
        elif not extend and (mods != 1 or uses != 1):
            issues.append(f"FAIL 3: module {m} not wired exactly once in lib.rs (mod x{mods}, use x{uses})")
    return issues


SCAN_FORMAT = re.compile(r'^"(\{\}\\n)*\{\}",?$')
SCAN_INCLUDE = re.compile(r'^include_str!\("([^"]+)"\),?$')


def code_text(sha, protected, moved=()):
    """Whitespace-free concatenation of added/removed non-comment code lines.

    A retarget that widens a source-scan corpus (signed amendment 1e: a guard
    whose `format!` concatenates `include_str!` files) may add
    `include_str!("<moved module>.rs")`, the comma that precedes it, and the
    matching `"{}\\n…{}"` placeholder string; those lines are excused and
    counted in the third return value.
    """
    add, rem, excused = [], [], 0
    for path, sign, text in diff_lines(sha):
        if not (LIB_ANY.match(path) or path in protected):
            continue
        s = text.strip()
        if not s or s.startswith("//"):
            continue
        inc = SCAN_INCLUDE.match(s)
        if sign == "+" and inc and inc.group(1) in {f"{m}.rs" for m in moved}:
            excused += 1
            continue
        if moved and SCAN_FORMAT.match(s):
            excused += 1
            continue
        if inc:
            s = s.rstrip(",")
        (add if sign == "+" else rem).append(re.sub(r"\s+", "", s))
    return "".join(add), "".join(rem), excused


def check_tag(tag, tag_sha, segment_modules):
    issues = []
    if git("cat-file", "-t", f"refs/tags/{tag}").strip() != "tag":
        return [f"FAIL 6: tag {tag} is not annotated"]
    body = git("tag", "-l", "--format=%(contents)", tag)
    if not V3.search(body):
        issues.append(f"FAIL 6: tag {tag} has no numeric V3 record (`warnings <n>/<n>`)")
    if not V4.search(body):
        issues.append(f"FAIL 6: tag {tag} has no numeric V4 record (`tests <n>/<n>`)")
    sizes = {m: int(n) for m, n in V7.findall(body)}
    for m in segment_modules:
        actual = line_count(tag_sha, f"{SRC}{m}.rs")
        if m not in sizes:
            issues.append(f"FAIL 6: tag {tag} has no V7 record `{m}.rs=<lines>`")
        elif sizes[m] != actual:
            issues.append(f"FAIL 6: tag {tag} says {m}.rs={sizes[m]}, actual {actual}")
        elif sizes[m] > SIZE_CAP:
            issues.append(f"FAIL 6: tag {tag} records {m}.rs={sizes[m]} > {SIZE_CAP}")
    return issues


def build(sha, target):
    wt = tempfile.mkdtemp(prefix="verify-stack-wt-")
    try:
        git("worktree", "add", "--detach", wt, sha)
        env = dict(os.environ, CARGO_TARGET_DIR=target)
        res = subprocess.run(["cargo", "check", "-p", "aida-cli-lib"], cwd=wt, env=env,
                             capture_output=True, text=True)
        tail = res.stderr.strip().splitlines()[-3:]
        return res.returncode == 0, tail
    finally:
        git("worktree", "remove", "--force", wt, check=False)
        shutil.rmtree(wt, ignore_errors=True)


def main():
    args = sys.argv[1:]
    do_build = "--build" in args
    args = [a for a in args if a != "--build"]
    unknown = [a for a in args if a.startswith("-")]
    if unknown or len(args) != 2:
        if unknown:
            print(f"error: unknown option(s): {' '.join(unknown)}", file=sys.stderr)
        print(__doc__.split("\n\n")[1], file=sys.stderr)
        sys.exit(2)
    base, head = args

    fails = collections.Counter()
    notes = collections.defaultdict(list)

    lin_issue = None
    try:
        if subprocess.run(["git", "merge-base", "--is-ancestor", base, head]).returncode != 0:
            lin_issue = "head does not descend from base"
        elif git("rev-list", "--merges", f"{base}..{head}").strip():
            lin_issue = "range contains merge commits"
    except GitError as e:
        lin_issue = str(e)
    if lin_issue:
        print(f"FAIL 8: {lin_issue}")
        fails[8] += 1
        shas = []
    else:
        shas = git("rev-list", "--reverse", f"{base}..{head}").split()

    classes = collections.Counter()
    moved = []           # modules moved so far, in order
    segment = []         # modules added since the last tag
    untagged_moves = []  # MOVE commits not yet covered by a tag
    tagged_any = False
    prev_class = None
    build_target = tempfile.mkdtemp(prefix="verify-stack-target-") if do_build else None
    try:
        for sha in shas:
            subj = git("log", "-1", "--format=%s", sha).strip()
            issues = []
            status = name_status(sha)
            parts_before = lib_parts(f"{sha}^")
            parts_deleted = [p for s, p in status if s == "D" and LIB_PART.match(p)]
            is_final = bool(parts_before) and not lib_parts(sha)
            mmove = MOVE_SUBJECT.match(subj)
            if is_final:
                cls = "FINAL"
            elif mmove:
                cls = "MOVE"
            elif subj.startswith("retarget:"):
                cls = "RETARGET"
            elif subj.startswith("tooling:"):
                cls = "TOOLING"
            else:
                cls = "OTHER"
                issues.append("FAIL 1: subject is not a MOVE, retarget: or tooling: commit")
            classes[cls] += 1

            if SPEC_ID.search(subj) and cls != "FINAL":
                issues.append("FAIL 2: spec ID in subject")

            modules = []
            if cls in ("MOVE", "FINAL"):
                added_mods = [NEW_MODULE.match(p).group(1) for s, p in status
                              if s == "A" and NEW_MODULE.match(p) and not LIB_PART.match(p)]
                extend = cls == "MOVE" and mmove.group(1) == "extend"
                if cls == "MOVE":
                    modules = [mmove.group(2)]
                    if extend:
                        issues += check_extend(sha, modules[0])
                    elif modules[0] not in added_mods:
                        issues.append(f"FAIL 3: commit does not create {SRC}{modules[0]}.rs")
                    stray = [m for m in added_mods if m not in modules]
                    if stray:
                        issues.append(f"FAIL 3: creates modules beyond its subject: {', '.join(stray)}")
                else:
                    modules = added_mods
                allowed = {f"{SRC}{m}.rs" for m in modules} | {MANIFEST}
                issues += check_purity(sha, set(modules), parts_deleted, allowed, extend)
                for m in modules:
                    n = line_count(sha, f"{SRC}{m}.rs")
                    if n is not None and n > SIZE_CAP:
                        issues.append(f"FAIL 7: {m}.rs is {n} lines > {SIZE_CAP}")
                if prev_class == "MOVE":
                    notes[sha].append("NOTE 5: previous MOVE has no retarget: commit")
                moved += modules
                segment += modules
                untagged_moves.append(sha)
                if do_build:
                    ok, tail = build(sha, build_target)
                    if ok:
                        notes[sha].append("NOTE 9: cargo check -p aida-cli-lib ok")
                    else:
                        issues.append("FAIL 9: cargo check -p aida-cli-lib failed")
                        issues += [f"        {l}" for l in tail]
            else:
                protected = {f"{SRC}{m}.rs" for m in moved}
                a, r, excused = code_text(sha, protected, moved if cls == "RETARGET" else ())
                if excused and a == r:
                    notes[sha].append(f"NOTE 4: {excused} scan-corpus line(s) widened to a moved module")
                if a != r:
                    bad = [p for _, p in status if LIB_ANY.match(p) or p in protected]
                    issues.append(f"FAIL 4: {cls.lower()} commit changes protected code: {', '.join(bad)}")
                if cls == "RETARGET" and prev_class != "MOVE":
                    issues.append("FAIL 5: retarget: commit does not directly follow a MOVE")

            tags = sorted(t for t in git("tag", "--points-at", sha).split() if SLICE_TAG.match(t))
            for t in tags:
                issues += check_tag(t, sha, segment)
            if tags:
                tagged_any = True
                notes[sha].append(f"NOTE 6: tagged {', '.join(tags)}")
                segment, untagged_moves = [], []

            for i in issues:
                if i.startswith("FAIL "):
                    fails[int(i.split()[1].rstrip(":"))] += 1
            bad = any(i.startswith("FAIL") for i in issues)
            ins = dels = 0
            for line in git("show", "--format=", "--numstat", sha).splitlines():
                a, d, _ = line.split("\t", 2)
                ins += int(a) if a.isdigit() else 0
                dels += int(d) if d.isdigit() else 0
            print(f"{'FAIL' if bad else 'PASS'}  {sha[:10]} {cls:<8} +{ins} -{dels}  {subj}")
            if cls in ("RETARGET", "TOOLING"):
                print(f"      files: {', '.join(p for _, p in status)}")
            for i in issues + notes[sha]:
                print(f"      {i}")
            prev_class = cls
    finally:
        if build_target:
            shutil.rmtree(build_target, ignore_errors=True)

    for sha in untagged_moves:
        print(f"FAIL 6: {sha[:10]} is not covered by a slice-NN-rR tag")
        fails[6] += 1

    n = len(shas)
    print(f"\nverify_stack {base}..{head}: {n} commit(s): "
          + ", ".join(f"{classes[c]} {c}" for c in ("MOVE", "RETARGET", "TOOLING", "FINAL", "OTHER") if classes[c]))
    for num, name in CHECKS:
        if num == 9 and not do_build:
            verdict = "SKIP (no --build)"
        elif num == 6 and not tagged_any and not fails[6]:
            verdict = "SKIP (no slice tags in range)"
        elif n == 0 and num != 8:
            verdict = "SKIP (empty range)"
        elif fails[num]:
            verdict = f"FAIL ({fails[num]} finding(s))"
        else:
            verdict = "PASS"
        print(f"  {num} {name:<10} {verdict}")
    ok = not fails and n > 0
    print(f"RESULT: {'PASS' if ok else 'FAIL'}")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
