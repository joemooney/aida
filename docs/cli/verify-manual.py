#!/usr/bin/env python3
"""CLI-manual drift-guard (STORY-597 core).

Checks against the live binary + the spec graph:
  1. COMPLETENESS — every command in `aida help-all` has a `### `aida <cmd>`` entry
     in some docs/cli/*.md chapter (a top-level command may legitimately be covered
     under a parent header, so we match on the leading command token).
  2. FLAG ACCURACY — every `--flag` cited inside a command's section appears in that
     command's `--help` (catches invented/renamed flags — the recurring doc-drift bug).
  3. INTERFACE-CHANGE REFLECTION (TASK-796) — every SURFACE-CHANGING spec (one that
     declares a non-empty `interface_changes` block, STORY-542) has its delta REFLECTED
     in the manual: the command(s) and flag(s) the change names must appear. This is the
     other half of the doc-intent loop — completeness asks "is every command documented?";
     reflection asks "is every spec's documented change actually IN the docs?". Drift here
     is a spec that moved a surface without the manual following.

Exit non-zero on a completeness omission OR an unreflected surface-changing delta (hard);
print WARN for flag mismatches (soft, since some `--flag` mentions are cross-references to
other commands).

# trace:TASK-795
# trace:TASK-796

The SPEC-ID leak check (1b below) hard-fails on bare SPEC-IDs in the user-facing
manual, with ONE narrow carve-out (TASK-795): SPEC-IDs inside a recognized
`doc-intent` / `trace` HTML-comment backlink marker are ALLOWED, so a manual entry
can backlink to its shaping spec(s) without the breadcrumb leaking into visible text.

Usage: python3 docs/cli/verify-manual.py            # all chapters
       python3 docs/cli/verify-manual.py 04-git*.md # one chapter
"""
import functools
import glob
import importlib.util
import os
import re
import shlex
import subprocess
import sys

CLI_DIR = os.path.dirname(os.path.abspath(__file__))
ENTRY_RE = re.compile(r"^### `aida ([a-z][a-z0-9-]*)")  # ### `aida <cmd> ...`
FLAG_RE = re.compile(r"`(--[a-z][a-z0-9-]*)`")
AIDA_CMD_RE = re.compile(r"\baida\b[^\n`]*")

# Reuse the part-1 (STORY-603) store resolver + interface_changes block parsing
# VERBATIM — the drift-guard's reflection check (3) resolves each spec's
# interface_changes from the same `aida-store` source the part-1 gate enforces,
# so the two halves of the doc-intent loop can never disagree on what a spec
# declared. Do NOT re-derive store loading or block scoping here.
# trace:TASK-796
_VIC_SPEC = importlib.util.spec_from_file_location(
    "verify_interface_changes",
    os.path.join(CLI_DIR, "verify-interface-changes.py"),
)
_vic = importlib.util.module_from_spec(_VIC_SPEC)
_VIC_SPEC.loader.exec_module(_vic)
_load_spec_yaml = _vic.load_spec_yaml  # (spec_id) -> raw YAML | None
_block_body = _vic._block_body  # (yaml, header_re) -> [lines]
_sh = _vic.sh
STORE_BRANCH = _vic.STORE_BRANCH


def help_text(cmd):
    args = cmd.split() if isinstance(cmd, str) else list(cmd)
    try:
        return subprocess.run(
            ["aida", *args, "--help"], capture_output=True, text=True, timeout=20
        ).stdout
    except Exception:
        return ""


@functools.lru_cache(maxsize=None)
def help_flags(cmd_path):
    return set(re.findall(r"--[a-z][a-z0-9-]*", help_text(cmd_path)))


def help_all_commands():
    out = subprocess.run(["aida", "help-all"], capture_output=True, text=True).stdout
    cmds = []
    for line in out.splitlines():
        m = re.match(r"^  ([a-z][a-z0-9-]+)\s", line)
        if m:
            cmds.append(m.group(1))
    # meta commands the manual needn't document as their own entry
    skip = {"help", "help-all"}
    return sorted(set(cmds) - skip)


def help_command_paths():
    """Return full command paths from `aida help commands`.

    This is the live clap-derived tree available today. Once STORY-1027's
    `aida commands --json --flags` lands, this can become one structured call.
    """
    out = subprocess.run(
        ["aida", "help", "commands"], capture_output=True, text=True
    ).stdout
    paths = set()
    for line in out.splitlines():
        m = re.match(r"^\s+aida\s+([a-z][a-z0-9-]*(?:\s+[a-z][a-z0-9-]*)*)\s{2,}", line)
        if m:
            paths.add(tuple(m.group(1).split()))
    return paths


def command_path_from_snippet(snippet, command_paths):
    try:
        tokens = shlex.split(snippet)
    except ValueError:
        tokens = snippet.split()
    if "aida" not in tokens:
        return None
    tokens = tokens[tokens.index("aida") + 1 :]
    words = []
    for tok in tokens:
        if tok.startswith("-") or tok.startswith("<") or tok.startswith("["):
            break
        clean = tok.strip(",.;:()")
        if not re.match(r"^[a-z][a-z0-9-]*$", clean):
            break
        words.append(clean)
    for n in range(len(words), 0, -1):
        cand = tuple(words[:n])
        if cand in command_paths:
            return " ".join(cand)
    return None


def subtree_flags(top, command_paths):
    flags = set()
    for path in command_paths:
        if path and path[0] == top:
            flags.update(help_flags(" ".join(path)))
    return flags


def parse_chapters(files):
    """Return {cmd: [flag citations in its section]} keyed by top-level command.

    Each citation records the flag and, when the prose includes a concrete
    `aida a b c --flag` command, the full command path it names.
    """
    documented = {}
    command_paths = help_command_paths()
    for f in files:
        with open(f) as fh:
            cur = None
            for line_no, line in enumerate(fh, 1):
                m = ENTRY_RE.match(line)
                if m:
                    cur = m.group(1)
                    documented.setdefault(cur, [])
                    continue
                # a non-command section (## ...) ends the current command's scope,
                # so cross-cutting prose isn't misattributed to the command above it
                if line.startswith("## ") or line.startswith("---"):
                    cur = None
                if not cur:
                    continue
                consumed = set()
                for cm in AIDA_CMD_RE.finditer(line):
                    snippet = cm.group(0)
                    path = command_path_from_snippet(snippet, command_paths)
                    if not path:
                        continue
                    for fm in re.finditer(r"--[a-z][a-z0-9-]*", snippet):
                        documented[cur].append(
                            {
                                "flag": fm.group(0),
                                "path": path,
                                "parent": cur,
                                "file": f,
                                "line": line_no,
                            }
                        )
                        consumed.add((cm.start() + fm.start(), fm.group(0)))
                for fm in FLAG_RE.finditer(line):
                    flag = fm.group(1)
                    if (fm.start(), flag) in consumed:
                        continue
                    documented[cur].append(
                        {
                            "flag": flag,
                            "path": None,
                            "parent": cur,
                            "file": f,
                            "line": line_no,
                        }
                    )
    return documented


# --- 3. interface-change reflection (TASK-796) ----------------------------
# trace:TASK-796
#
# A spec is "surface-changing" when it declares a non-empty `interface_changes`
# block (the same metadata the part-1 gate forces onto surface-touching diffs).
# This check asserts the manual REFLECTS each such delta. Matching is deliberately
# token-based and substrate-grounded — no LLM judgement:
#
#   * COMMAND tokens — `aida <cmd>` named in an interface_changes line. We only
#     REQUIRE reflection of a command that is a real TOP-LEVEL command (present in
#     `aida help-all`), to stay consistent with the completeness check's universe:
#     a subcommand / alias / hidden verb (e.g. `aida zen finish`) is legitimately
#     covered under a parent — or not documented as its own surface at all — so a
#     spec naming one is advisory, not a hard miss.
#   * FLAG tokens — every `--flag` named in an interface_changes line MUST appear
#     somewhere in the manual. Flags are specific and the completeness check ignores
#     them, so this is the coverage the reflection check genuinely adds.
#
# "Reflected" = the token appears anywhere in the assembled manual text. When a
# manual entry backlinks to the spec (part-3 `<!-- doc-intent: shaped by <ID> -->`),
# that entry is the natural home for the delta, but we match manual-wide so a delta
# documented in a sibling/cross-ref entry still counts (the goal is "the change is in
# the docs," not "in one exact entry"). A surface-changing spec whose command/flag is
# absent everywhere is the drift this guard catches.
SPEC_ID_FILE_RE = re.compile(r"/([A-Z]+-[0-9]+)\.yaml$")
IC_CMD_RE = re.compile(r"`?\baida ([a-z][a-z0-9-]+)\b")
IC_FLAG_RE = re.compile(r"(--[a-z][a-z0-9-]+)")


def all_surface_changing_specs():
    """Every spec_id on the store branch (or local worktree) that declares a
    non-empty `interface_changes` block, mapped to its delta lines."""
    # trace:TASK-796
    ids = set()
    ls = _sh(["git", "ls-tree", "-r", STORE_BRANCH, "--name-only"])
    if ls.returncode == 0:
        for path in ls.stdout.splitlines():
            m = SPEC_ID_FILE_RE.search(path)
            if m:
                ids.add(m.group(1))
    if not ids:
        store = os.path.join(_vic.REPO, ".aida-store", "objects")
        for _root, _dirs, fnames in os.walk(store):
            for fn in fnames:
                m = re.match(r"([A-Z]+-[0-9]+)\.yaml$", fn)
                if m:
                    ids.add(m.group(1))
    out = {}
    for sid in sorted(ids):
        yaml_text = _load_spec_yaml(sid)
        if yaml_text is None:
            continue
        deltas = [
            m.group(1).strip()
            for m in (
                re.match(r"^\s+-\s+(.*)$", raw)
                for raw in _block_body(yaml_text, r"^interface_changes:\s*$")
            )
            if m
        ]
        if deltas:
            out[sid] = deltas
    return out, bool(ids)


def reflection_misses(spec_deltas, manual_text, top_level_cmds):
    """For each surface-changing spec, the (command|flag) tokens its
    interface_changes name that are NOT reflected anywhere in the manual.
    Command tokens are only required when they're real top-level commands."""
    # trace:TASK-796
    misses = {}
    for sid, lines in spec_deltas.items():
        cmds, flags = set(), set()
        for ln in lines:
            for m in IC_CMD_RE.finditer(ln):
                cmds.add(m.group(1))
            for m in IC_FLAG_RE.finditer(ln):
                flags.add(m.group(1))
        unreflected = []
        for c in sorted(cmds):
            if c not in top_level_cmds:
                continue  # subcommand/alias/hidden — advisory, not required
            if not re.search(r"\baida " + re.escape(c) + r"\b", manual_text):
                unreflected.append(f"command `aida {c}`")
        for fl in sorted(flags):
            if not re.search(r"`?" + re.escape(fl) + r"`?", manual_text):
                unreflected.append(f"flag `{fl}`")
        if unreflected:
            misses[sid] = unreflected
    return misses


def main():
    files = (
        [os.path.join(CLI_DIR, a) for a in sys.argv[1:]]
        if len(sys.argv) > 1
        else sorted(glob.glob(os.path.join(CLI_DIR, "[0-9][0-9]-*.md")))
    )
    documented = parse_chapters(files)
    all_cmds = help_all_commands()

    # 1. completeness (only meaningful when scanning the whole manual)
    hard_fail = False
    if len(sys.argv) == 1:
        missing = [c for c in all_cmds if c not in documented]
        if missing:
            hard_fail = True
            print(f"FAIL completeness — {len(missing)} command(s) with no manual entry:")
            for c in missing:
                print(f"    aida {c}")
        else:
            print(f"OK completeness — all {len(all_cmds)} help-all commands documented")

    # 1b. SPEC-ID leakage — the user-facing manual must carry no STORY-x/TASK-x
    # noise (same convention that keeps SPEC-IDs out of --help). Hard fail.
    #
    # EXCEPTION (TASK-795): a manual entry backlinks to its shaping spec(s) via a
    # MACHINE-READABLE, NON-LEAKING marker so a reader/agent is one hop from intent
    # without the user-facing prose carrying the breadcrumb. The convention is an
    # HTML-comment `doc-intent` marker (emitted by generate-entry.py):
    #
    #     <!-- doc-intent: shaped by TASK-795, STORY-603 -->
    #     <!-- trace:TASK-795 -->
    #
    # A human reader never sees it; the leak check ALLOWS SPEC-IDs that appear ONLY
    # inside this marker. We blank the marker spans BEFORE scanning so the IDs in
    # them are exempt — but a bare SPEC-ID anywhere in visible text (or inside any
    # OTHER comment form) still hard-fails. This is the deliberately-narrow carve-out
    # that reconciles the part-3 backlink convention with the part-1 leak gate.
    backlink = re.compile(
        r"<!--\s*(?:doc-intent:[^>]*?|trace:[^>]*?)-->", re.I
    )
    specid = re.compile(r"\b(STORY|TASK|BUG|EPIC|SPIKE|ADR|FR|PRIN|VIS|CON|TERM|CR)-[0-9]+")
    leaks = []
    for f in files:
        for i, line in enumerate(open(f), 1):
            # blank out recognized backlink markers; their SPEC-IDs are exempt
            scan = backlink.sub("", line)
            for m in specid.finditer(scan):
                leaks.append(f"{os.path.basename(f)}:{i} {m.group(0)}")
    if leaks:
        hard_fail = True
        print(f"FAIL spec-id leak — {len(leaks)} SPEC-ID(s) in the user-facing manual (use <spec-id>/<epic-id> placeholders):")
        for l in leaks[:20]:
            print(f"    {l}")
    else:
        print("OK spec-ids — no SPEC-ID noise in the manual")

    # 3. interface-change reflection (TASK-796) — only meaningful on a full scan.
    # Every surface-changing spec's declared delta must be REFLECTED in the manual.
    # trace:TASK-796
    if len(sys.argv) == 1:
        manual_text = ""
        for f in files:
            manual_text += open(f).read() + "\n"
        spec_deltas, store_ok = all_surface_changing_specs()
        if not store_ok:
            print(
                f"SKIP reflection — could not resolve the '{STORE_BRANCH}' store; "
                "cannot check interface_changes reflection.\n"
                f"    (Fetch the store: git fetch origin {STORE_BRANCH}:{STORE_BRANCH})"
            )
        else:
            misses = reflection_misses(spec_deltas, manual_text, set(all_cmds))
            if misses:
                hard_fail = True
                total = sum(len(v) for v in misses.values())
                print(
                    f"FAIL reflection — {total} surface delta(s) across "
                    f"{len(misses)} spec(s) not reflected in the manual:"
                )
                for sid in sorted(misses):
                    for item in misses[sid]:
                        print(f"    {sid}: {item} declared in interface_changes but absent from the manual")
                print(
                    "    Fix: document the new/changed surface in the relevant chapter "
                    "(generate a draft with docs/cli/generate-entry.py <command>)."
                )
            else:
                n = len(spec_deltas)
                print(f"OK reflection — all {n} surface-changing spec(s)' interface_changes reflected in the manual")

    # 2. flag accuracy
    # trace:TASK-1207 | ai:codex
    hard_flag_misses = []
    command_paths = help_command_paths()
    subtree_cache = {}
    for cmd, citations in sorted(documented.items()):
        if not citations:
            continue
        subtree_cache.setdefault(cmd, subtree_flags(cmd, command_paths))
        seen = set()
        for cite in citations:
            fl = cite["flag"]
            path = cite["path"] or cmd
            key = (fl, path, cite["file"], cite["line"])
            if key in seen:
                continue
            seen.add(key)
            if cite["path"] and fl in help_flags(path):
                continue
            if fl in subtree_cache[cmd]:
                continue
            hard_flag_misses.append(cite)
    if hard_flag_misses:
        hard_fail = True
        print(
            f"FAIL flags — {len(hard_flag_misses)} cited --flag token(s) do not resolve "
            "to the cited command or its parent command subtree:"
        )
        for cite in hard_flag_misses[:80]:
            loc = f"{os.path.basename(cite['file'])}:{cite['line']}"
            where = f"aida {cite['path']}" if cite["path"] else f"aida {cite['parent']}"
            print(f"    {loc} `{cite['flag']}` cited under `{where}`")
        if len(hard_flag_misses) > 80:
            print(f"    ... {len(hard_flag_misses) - 80} more")
    else:
        print("OK flags — every cited --flag found in its command's --help")

    sys.exit(1 if hard_fail else 0)


if __name__ == "__main__":
    main()
