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
  4. SUBCOMMAND EXISTENCE (BUG-1949) — every cited `aida <cmd> <verb>` (and every
     slash/vs verb list inside a command's section) names a subcommand the CLI has.

Exit non-zero on a completeness omission, an unreflected surface-changing delta, a
flag mismatch, or a cited subcommand the CLI no longer has.

# trace:TASK-795
# trace:TASK-796

The SPEC-ID leak check (1b below) hard-fails on bare SPEC-IDs in the user-facing
manual, with ONE narrow carve-out (TASK-795): SPEC-IDs inside a recognized
`doc-intent` / `trace` HTML-comment backlink marker are ALLOWED, so a manual entry
can backlink to its shaping spec(s) without the breadcrumb leaking into visible text.

Usage: python3 docs/cli/verify-manual.py            # all chapters
       python3 docs/cli/verify-manual.py 04-git*.md # one chapter
       python3 docs/cli/verify-manual.py --static   # no-build hidden-flag check
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


@functools.lru_cache(maxsize=None)
def command_path_exists(cmd_path):
    text = help_text(cmd_path)
    if not text:
        return False
    return bool(
        re.search(
            r"^Usage:\s+aida\s+"
            + re.escape(cmd_path)
            + r"(?:\s|$|\[|<)",
            text,
            re.M,
        )
    )


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

    This is the live clap-derived tree available today, plus subcommands
    discovered recursively from each command's `--help`. Once STORY-1027's
    `aida commands --json --flags` lands, this can become one structured call.
    """
    out = subprocess.run(
        ["aida", "help", "commands"], capture_output=True, text=True
    ).stdout
    paths = set()
    for line in out.splitlines():
        m = re.match(r"^\s*aida\s+([a-z][a-z0-9-]*(?:\s+[a-z][a-z0-9-]*)*)\s{2,}", line)
        if m:
            paths.add(tuple(m.group(1).split()))
    paths.update((cmd,) for cmd in help_all_commands())

    queue = list(paths)
    seen = set()
    while queue:
        path = queue.pop(0)
        if path in seen:
            continue
        seen.add(path)
        text = help_text(" ".join(path))
        for line in text.splitlines():
            m = re.match(r"^\s{2}([a-z][a-z0-9-]*)\s{2,}", line)
            if not m:
                continue
            child = (*path, m.group(1))
            if child not in paths and command_path_exists(" ".join(child)):
                paths.add(child)
                queue.append(child)
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
        path = " ".join(cand)
        if cand in command_paths or command_path_exists(path):
            return path
    return None


def command_snippet_variants(snippet, current_command):
    snippet = snippet.strip()
    if snippet.startswith("--") or snippet.startswith("<"):
        return []
    variants = []
    if snippet.startswith("aida "):
        variants.append(snippet)
    else:
        first = snippet.split()[0] if snippet.split() else ""
        current_child = f"{current_command} {first}" if current_command and first else ""
        if current_child and command_path_exists(current_child):
            variants.append(f"aida {current_command} {snippet}")
            variants.append(f"aida {snippet}")
        else:
            variants.append(f"aida {snippet}")
            if current_command and not snippet.startswith(f"{current_command} "):
                variants.append(f"aida {current_command} {snippet}")
    return variants


def subtree_flags(top, command_paths):
    flags = set(help_flags(top))
    for path in command_paths:
        if path and path[0] == top:
            flags.update(help_flags(" ".join(path)))
    return flags


def flag_accuracy_misses(documented, command_paths):
    """Return citations whose flags do not resolve to their command subtree."""
    # trace:TASK-1251 | ai:codex
    misses = []
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
            misses.append(cite)
    return misses


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
                snippets = []
                for cm in AIDA_CMD_RE.finditer(line):
                    snippets.append((cm.start(), cm.group(0), None))
                for bm in re.finditer(r"`([^`]+)`", line):
                    if re.match(r"(?:git|cargo|python3?|bash|gh|glab)\b", bm.group(1)):
                        for fm in re.finditer(r"--[a-z][a-z0-9-]*", bm.group(1)):
                            consumed.add((bm.start(1) + fm.start(), fm.group(0)))
                        continue
                    for snippet in command_snippet_variants(bm.group(1), cur):
                        path = command_path_from_snippet(snippet, command_paths)
                        if path:
                            snippets.append((bm.start(1), snippet, path))
                            break
                for start, snippet, resolved_path in snippets:
                    path = resolved_path or command_path_from_snippet(snippet, command_paths)
                    if not path:
                        continue
                    offset = snippet.find("--")
                    if offset < 0:
                        continue
                    for fm in re.finditer(r"--[a-z][a-z0-9-]*", snippet[offset:]):
                        flag = fm.group(0)
                        documented[cur].append(
                            {
                                "flag": flag,
                                "path": path,
                                "parent": cur,
                                "file": f,
                                "line": line_no,
                            }
                        )
                        line_flag = line.find(flag, start)
                        if line_flag >= 0:
                            consumed.add((line_flag, flag))
                for fm in FLAG_RE.finditer(line):
                    flag = fm.group(1)
                    if (fm.start(1), flag) in consumed:
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


# --- 4. subcommand existence (BUG-1949) ------------------------------------
# trace:BUG-1949 | ai:claude
#
# The flag-accuracy check (2) resolves `--flag` tokens but never asks whether
# the subcommand a citation names still exists, so a verb cut from the CLI
# (`aida session forget`) survived in the manual. This check collects two
# citation shapes and asks the live binary about each:
#
#   * EXPLICIT — a backtick span (or a fenced code line) starting `aida a b c`.
#     Every leading lowercase word after a real top-level command is a
#     candidate subcommand. Unknown TOP-level words are left to the
#     completeness check: `aida <name>` may be a PATH extension (`aida-<name>`).
#   * VERB LIST — single-word backtick spans joined by ` / ` or ` vs ` inside a
#     `### `aida <cmd>`` section (`start` / `end` / `leases`): each word is read
#     as `aida <cmd> <word>` — or `aida <cmd> <sub> <word>` when the list sits in
#     parentheses right after a `<sub>` span (`scope` (`set` / `clear`)). A list
#     counts only when at least half its words ARE real subcommands, so a list
#     of roles, modes or graph reads is never mistaken for a list of verbs; and
#     a word that is itself a top-level command is a cross-reference, not a
#     miss.
#
# A path is a miss only when clap itself answers `unrecognized subcommand` —
# a command that takes a positional parses the word as an argument and never
# errors, so the check cannot false-fail on `aida show <ID>`-shaped prose.
#
# Deliberately NOT checked: whether a citation supplies a command's required
# positional arguments. The manual names commands by path in prose and verb
# lists (`usage unused`, `start` / `end`) without their arguments on purpose,
# so flagging a missing positional would fail nearly every verb list. The
# runnable-command guarantee belongs where a command is printed for the user
# to paste — CLI hints, which are tested at their source.
# trace:BUG-1973 | ai:claude
SUBCMD_WORD_RE = re.compile(r"^[a-z][a-z0-9-]*$")
VERB_LIST_RE = re.compile(
    r"`([a-z][a-z0-9-]*)`(?:\s*(?:/|vs\.?)\s*`[a-z][a-z0-9-]*`)+"
)
VERB_LIST_WORD_RE = re.compile(r"`([a-z][a-z0-9-]*)`")
FENCE_LINE_RE = re.compile(r"^\s*(?:\$\s+)?(aida\s.*)$")
NESTED_LIST_PARENT_RE = re.compile(r"`([a-z][a-z0-9-]*)`\s*\($")


def _leading_words(snippet):
    """Lowercase command words after `aida`, stopping at the first flag/arg."""
    toks = snippet.split()
    if not toks or toks[0] != "aida":
        return []
    words = []
    for tok in toks[1:]:
        if not SUBCMD_WORD_RE.match(tok):
            break
        words.append(tok)
    return words


def subcommand_citations(files):
    """Every subcommand path the manual cites, in the two shapes above."""
    # trace:BUG-1949 | ai:claude
    cites = []
    for f in files:
        with open(f) as fh:
            cur = None
            in_fence = False
            for line_no, line in enumerate(fh, 1):
                if line.lstrip().startswith("```"):
                    in_fence = not in_fence
                    continue
                if in_fence:
                    m = FENCE_LINE_RE.match(line)
                    words = _leading_words(m.group(1)) if m else []
                    if words:
                        cites.append(
                            {"kind": "explicit", "words": words, "file": f, "line": line_no}
                        )
                    continue
                m = ENTRY_RE.match(line)
                if m:
                    cur = m.group(1)
                    continue
                if line.startswith("## ") or line.startswith("---"):
                    cur = None
                for bm in re.finditer(r"`([^`]+)`", line):
                    words = _leading_words(bm.group(1))
                    if words:
                        cites.append(
                            {"kind": "explicit", "words": words, "file": f, "line": line_no}
                        )
                if not cur:
                    continue
                for lm in VERB_LIST_RE.finditer(line):
                    verbs = VERB_LIST_WORD_RE.findall(lm.group(0))
                    nested = NESTED_LIST_PARENT_RE.search(line[: lm.start()])
                    parent = f"{cur} {nested.group(1)}" if nested else cur
                    cites.append(
                        {
                            "kind": "list",
                            "parent": parent,
                            "words": verbs,
                            "file": f,
                            "line": line_no,
                        }
                    )
    return cites


@functools.lru_cache(maxsize=None)
def subcommand_unrecognized(cmd_path):
    """True when clap rejects the last word of cmd_path as an unknown subcommand."""
    # trace:BUG-1949 | ai:claude
    try:
        res = subprocess.run(
            ["aida", *cmd_path.split(), "--help"],
            capture_output=True,
            text=True,
            timeout=20,
        )
    except Exception:
        return False
    return "unrecognized subcommand" in (res.stderr or "")


def subcommand_misses(cites, unrecognized, known):
    """Citations naming a subcommand the CLI does not have.

    `unrecognized(path)` — clap rejects the path's last word as a subcommand.
    `known(path)` — the path is a real command (so its next word is a candidate
    subcommand, not a positional argument).
    """
    # trace:BUG-1949 | ai:claude
    misses = []
    seen = set()
    for cite in cites:
        if cite["kind"] == "explicit":
            words = cite["words"]
            if not known(words[0]):
                continue  # unknown top-level: completeness check / PATH extension
            for k in range(2, len(words) + 1):
                path = " ".join(words[:k])
                if unrecognized(path):
                    bad = path
                    break
                if not known(path):
                    bad = None
                    break
            else:
                bad = None
            if bad:
                key = (bad, cite["file"], cite["line"])
                if key not in seen:
                    seen.add(key)
                    misses.append({"path": bad, "file": cite["file"], "line": cite["line"]})
            continue
        parent = cite["parent"]
        paths = [f"{parent} {w}" for w in cite["words"]]
        if 2 * sum(1 for p in paths if known(p)) < len(paths):
            continue  # not a verb list (roles, modes, graph reads)
        for w, p in zip(cite["words"], paths):
            if known(w):
                continue  # a top-level command cited alongside, not a subcommand
            key = (p, cite["file"], cite["line"])
            if key not in seen and unrecognized(p):
                seen.add(key)
                misses.append({"path": p, "file": cite["file"], "line": cite["line"]})
    return misses


# --- static hidden-flag guard (BUG-1937) -----------------------------------
# trace:BUG-1937 | ai:claude
#
# Everything above shells out to a built `aida`, so the pre-push tier (which
# promises no cargo build) cannot run it. `--static` is the no-build slice of
# the flag-accuracy check: it reads the clap attributes straight from the Rust
# source and fails on a manual citation of a long flag that exists ONLY as a
# hidden flag or hidden alias. That's exactly how a hidden flag's doc citation
# reached CI. It's deliberately conservative: a flag that is visible on ANY command
# is never reported, so a wrong guess about which command a citation belongs
# to cannot produce a false failure. The full binary-backed guard still runs in
# CI and remains the authority on per-command accuracy.
REPO_ROOT = os.path.dirname(os.path.dirname(CLI_DIR))
CLAP_SOURCE_DIRS = ("aida-cli-lib/src", "aida-cli/src")
CLAP_ATTR_RE = re.compile(r"#\[(?:clap|arg)\(")
STRING_LIT_RE = re.compile(r'"(?:\\.|[^"\\])*"')
EXTERNAL_TOOL_RE = re.compile(r"(?:git|cargo|python3?|bash|gh|glab)\b")
FIELD_RE = re.compile(r"(?:pub(?:\([^)]*\))?\s+)?(?:r#)?([a-z_][a-z0-9_]*)\s*:(?!:)")


def _skip_string(text, i):
    """Index just past the string literal opening at text[i] == '"'."""
    i += 1
    while i < len(text):
        if text[i] == "\\":
            i += 2
            continue
        if text[i] == '"':
            return i + 1
        i += 1
    return i


def _balanced_end(text, i, open_ch, close_ch):
    """Index just past the delimiter that closes text[i] == open_ch."""
    depth = 0
    while i < len(text):
        ch = text[i]
        if ch == '"':
            i = _skip_string(text, i)
            continue
        if ch == open_ch:
            depth += 1
        elif ch == close_ch:
            depth -= 1
            if depth == 0:
                return i + 1
        i += 1
    return i


def _attached_field(text, i):
    """(field_start, field_name) of the item an attribute ending at i decorates.

    Skips whitespace, comments and further attributes. Returns (None, None)
    when the next item is not a named struct field (an enum variant, a struct).
    """
    while i < len(text):
        if text[i].isspace():
            i += 1
        elif text.startswith("//", i):
            nl = text.find("\n", i)
            i = len(text) if nl < 0 else nl + 1
        elif text.startswith("/*", i):
            end = text.find("*/", i + 2)
            i = len(text) if end < 0 else end + 2
        elif text.startswith("#[", i):
            i = _balanced_end(text, i + 1, "[", "]")
        else:
            m = FIELD_RE.match(text, i)
            if m:
                return i, m.group(1)
            return None, None
    return None, None


def clap_long_flags(text):
    """Return (visible, hidden) sets of `--long` flag names declared in text.

    A field's clap/arg attributes are merged before classifying, so
    `#[clap(long)] #[clap(hide = true)]` on one field reads as hidden. Clap
    `alias`/`aliases` are hidden by clap itself; `visible_alias(es)` are shown.
    """
    # trace:BUG-1937 | ai:claude
    per_field = {}
    for m in CLAP_ATTR_RE.finditer(text):
        paren = m.end() - 1
        end = _balanced_end(text, paren, "(", ")")
        body = text[paren + 1 : end - 1]
        start, name = _attached_field(text, end + 1)
        if start is None:
            continue
        entry = per_field.setdefault(start, [name, []])
        entry[1].append(body)

    visible, hidden = set(), set()
    for name, bodies in per_field.values():
        raw = " , ".join(bodies)
        bare = STRING_LIT_RE.sub('""', raw)
        is_hidden = bool(re.search(r"\bhide\s*=\s*true\b", bare))
        longs = set()
        explicit = re.findall(r'\blong\s*=\s*"([^"]+)"', raw)
        longs.update(explicit)
        if re.search(r"\blong\b(?!\s*=)", bare):
            longs.add(name.replace("_", "-"))
        aliases = set()
        for am in re.finditer(r'(?<![\w])alias\s*=\s*"([^"]+)"', raw):
            aliases.add(am.group(1))
        for am in re.finditer(r"(?<![\w])aliases\s*=\s*\[([^\]]*)\]", raw):
            aliases.update(re.findall(r'"([^"]+)"', am.group(1)))
        shown_aliases = set()
        for am in re.finditer(r'\bvisible_alias\s*=\s*"([^"]+)"', raw):
            shown_aliases.add(am.group(1))
        for am in re.finditer(r"\bvisible_aliases\s*=\s*\[([^\]]*)\]", raw):
            shown_aliases.update(re.findall(r'"([^"]+)"', am.group(1)))
        if not longs:
            continue  # positional / short-only / flatten: no long flag to cite
        (hidden if is_hidden else visible).update("--" + f for f in longs)
        hidden.update("--" + a for a in aliases)
        (hidden if is_hidden else visible).update("--" + a for a in shown_aliases)
    return visible, hidden


def clap_source_files(root=REPO_ROOT):
    """Non-test Rust sources that can declare `aida` CLI flags."""
    files = []
    for rel in CLAP_SOURCE_DIRS:
        base = os.path.join(root, rel)
        for dirpath, dirnames, fnames in os.walk(base):
            dirnames[:] = [d for d in dirnames if d != "tests"]
            for fn in fnames:
                if fn.endswith(".rs") and not fn.endswith("tests.rs"):
                    files.append(os.path.join(dirpath, fn))
    return sorted(files)


def static_flag_citations(files):
    """Every `--flag` cited inside a `### `aida <cmd>`` section.

    Same section scoping as parse_chapters: a `## ` heading or `---` rule ends a
    command's scope, and flags inside a git/cargo/python/bash/gh/glab span
    belong to that tool. Flags are taken from backtick spans and from bare
    `aida ...` prose, a superset of what the full guard checks.
    """
    # trace:BUG-1937 | ai:claude
    cites = []
    for f in files:
        with open(f) as fh:
            cur = None
            for line_no, line in enumerate(fh, 1):
                m = ENTRY_RE.match(line)
                if m:
                    cur = m.group(1)
                    continue
                if line.startswith("## ") or line.startswith("---"):
                    cur = None
                if not cur:
                    continue
                flags = []
                outside = line
                for bm in re.finditer(r"`([^`]+)`", line):
                    span = bm.group(1)
                    outside = outside.replace(bm.group(0), " ", 1)
                    if EXTERNAL_TOOL_RE.match(span):
                        continue
                    flags.extend(re.findall(r"--[a-z][a-z0-9-]*", span))
                for cm in AIDA_CMD_RE.finditer(outside):
                    flags.extend(re.findall(r"--[a-z][a-z0-9-]*", cm.group(0)))
                for flag in flags:
                    cites.append({"flag": flag, "parent": cur, "file": f, "line": line_no})
    return cites


def static_hidden_flag_misses(cites, visible, hidden):
    """Citations of a flag that exists only hidden (never visible anywhere)."""
    hidden_only = hidden - visible
    return [c for c in cites if c["flag"] in hidden_only]


def main_static(files):
    # trace:BUG-1937 | ai:claude
    visible, hidden = set(), set()
    sources = clap_source_files()
    for src in sources:
        with open(src, encoding="utf-8") as fh:
            v, h = clap_long_flags(fh.read())
        visible |= v
        hidden |= h
    if not visible:
        print(f"FAIL static flags — no clap long flags parsed from {', '.join(CLAP_SOURCE_DIRS)}")
        return 1
    misses = static_hidden_flag_misses(static_flag_citations(files), visible, hidden)
    if misses:
        print(
            f"FAIL static flags — {len(misses)} cited --flag token(s) are hidden from "
            "`--help` (hide = true or a hidden alias) and visible on no command:"
        )
        for cite in misses[:80]:
            loc = f"{os.path.basename(cite['file'])}:{cite['line']}"
            print(f"    {loc} `{cite['flag']}` cited under `aida {cite['parent']}`")
        print("    Fix: drop the citation, or unhide the flag if users should see it.")
        return 1
    print(
        f"OK static flags — no manual citation of a hidden-only flag "
        f"({len(visible)} visible / {len(hidden - visible)} hidden-only long flags parsed "
        f"from {len(sources)} source files)"
    )
    return 0


def main():
    # trace:BUG-1937 | ai:claude
    if "--static" in sys.argv[1:]:
        names = [a for a in sys.argv[1:] if a != "--static"]
        files = (
            [os.path.join(CLI_DIR, a) for a in names]
            if names
            else sorted(glob.glob(os.path.join(CLI_DIR, "[0-9][0-9]-*.md")))
        )
        sys.exit(main_static(files))
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

    # 4. subcommand existence — every cited `aida <cmd> <verb>` must be a verb
    # the CLI still has. trace:BUG-1949 | ai:claude
    sub_misses = subcommand_misses(
        subcommand_citations(files), subcommand_unrecognized, command_path_exists
    )
    if sub_misses:
        hard_fail = True
        print(
            f"FAIL subcommands — {len(sub_misses)} cited subcommand(s) the CLI "
            "rejects as unrecognized:"
        )
        for miss in sub_misses[:80]:
            loc = f"{os.path.basename(miss['file'])}:{miss['line']}"
            print(f"    {loc} `aida {miss['path']}`")
        print("    Fix: cite a subcommand listed in the parent's --help, or drop the citation.")
    else:
        print("OK subcommands — every cited subcommand exists in the CLI")

    # 2. flag accuracy
    # trace:TASK-1207 | ai:codex
    command_paths = help_command_paths()
    hard_flag_misses = flag_accuracy_misses(documented, command_paths)
    if hard_flag_misses:
        hard_fail = True
        print(
            f"FAIL flags — {len(hard_flag_misses)} cited --flag token(s) did not "
            "resolve to the cited command or its parent/child command subtree:"
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
