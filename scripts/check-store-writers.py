#!/usr/bin/env python3
"""Sink-site source-review ratchet for STORY-2561; never authorizes a writer.

Regenerate after merging main, then review the sink and operation mapping diff.
See docs/internal/security/store-mutation-writers.md for audit scope and limits.
"""
# trace:TASK-3222 | ai:codex
from __future__ import annotations

import argparse
import ast
import bisect
import json
from pathlib import Path
import re
import shlex
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
INVENTORY = "scripts/store-writers.toml"
# Include adjacent REST/TUI consumers, not just the two primary crates.
RUST_ROOTS = ("aida-core/src", "aida-cli-lib/src", "aida-cli/src", "aida-server/src", "aida-tui/src", "aida-desktop/src", "aida-crate/src", "aida-generate-types/src")
SCRIPT_ROOTS = ("scripts", "aida-core/templates", "hooks", ".githooks")
# Deliberately over-inclusive: a filesystem destination need not contain 'store'.
APIS = (
    r"(?:fs::|File::|OpenOptions::|tokio::fs::)(?:write|rename|remove_file|remove_dir_all|create_dir_all|create|copy|hard_link|symlink|new)\b\s*(?=\()",
    r"\b(?:write_atomic|atomic_write|write_object(?:_if_changed)?|delete_object|with_store_write_lock|with_sync_lock|with_dispense_lock)\b\s*(?=\()",
    r"\b(?:save(?:_reporting(?:_with_origin)?|_imported|_unstamped|_with_origin|_with_commit_subject|_with_conflict_detection)?|update_atomically(?:_tracked)?|update_spec_atomically(?:_with_subject)?|bulk_update(?:_atomically)?|add_requirement(?:_atomic|_with_id)?|update_requirement(?:_versioned)?|delete_requirement|queue_(?:add|remove\w*|reorder|clear)|lock_store|lock_queue_write)\s*(?=\()",
    r"\b(?:execute|execute_batch|transaction|transaction_with_behavior|unchecked_transaction|write_all|write_fmt|set_len|persist|persist_noclobber|truncate|create_new)\s*(?=\()",
    r"\b(?:write_cas\w*|write_batch_cas\w*|write_local|write_batch_local|delete_ledger|set_role_cas|set_allowed_seats_cas|allow_seat_cas|disallow_seat_cas|update_seats_cas|unset_role_cas|link_cas|register_node\w*|set_node_identity_field|hijack_node|unregister_node|acquire_claim|release_claim|release_stale_foreign_claim|acquire_lock_claim|heartbeat_lock_claim|release_lock_claim|claim_first_block|auto_allocate_block_inner|rollback_unpushed_claim|digest_local_to_canonical|digest_snapshot_to_canonical|write_snapshot_locked|publish_to_store|commit_record|commit_child_store_writes|commit_store_writes_since)\s*(?=\()",
    r"\b(?:pull_rebase\w*|merge_union_auto|fast_forward_store|merge_gate(?:_locked)?|sync_objects|commit_pending_store_changes_on_host|rollback_commit_if_head_matches|squash_orphan_to_snapshot|opportunistic_store_gc|gc_aggressive|push_exact(?:_outcome)?)\s*(?=\()",
    r"\bgit_ops::(?:add|add_all|commit|commit_paths|push|init|configure_user)\s*(?=\()",
)
SEED = re.compile("|".join(APIS))
# Sink identity excludes line numbers, argument values and unrelated source text.
DISPOSITIONS = {"s2_typed", "host_only", "lane_private", "read_only", "blocked"}
# trace:TASK-3222 | ai:codex
# Reads can refresh persistent caches, so only trivial command families are
# exempt. Unknown/new subcommands are writer candidates without a vocabulary edit.
AIDA_READ_ONLY = {"help", "version"}
GIT_WRITERS = {"add", "commit", "push", "update-ref", "commit-tree", "merge", "rebase"}
AIDA_VALUE_FLAGS = {"--file", "--project", "-p", "--server", "-s", "--format",
                    "--cast-out", "--cast-title"}
GIT_VALUE_FLAGS = {"-C", "-c", "--git-dir", "--work-tree", "--namespace",
                   "--config-env", "--exec-path"}


def executable_kind(token):
    token = token.strip("\"'")
    variable = re.fullmatch(r"\$(?:\{((?:AIDA|GIT)(?:_[A-Z0-9_]+)?)\}|((?:AIDA|GIT)(?:_[A-Z0-9_]+)?))", token)
    if variable:
        return (variable.group(1) or variable.group(2)).split("_")[0].lower()
    return Path(token).name


def argv_writer(argv):
    if len(argv) < 2:
        return None
    executable = executable_kind(argv[0])
    if executable not in {"aida", "git"}:
        return None
    value_flags = AIDA_VALUE_FLAGS if executable == "aida" else GIT_VALUE_FLAGS
    index = 1
    while index < len(argv) and argv[index].startswith("-"):
        flag = argv[index]
        if flag == "--":
            index += 1
            break
        index += 2 if flag in value_flags else 1
    if index >= len(argv):
        return None
    command = argv[index].strip("\"'")
    if executable == "aida" and command not in AIDA_READ_ONLY:
        return "aida " + command
    if executable == "git" and (command in GIT_WRITERS or
                                command == "hash-object" and "-w" in argv[index+1:]):
        return "git " + command
    return None


def blank(text: str) -> str:
    return re.sub(r"[^\n]", " ", text)


def mask_rust(source: str) -> str:
    """Offset-preserving comment/string mask, including nested block comments."""
    token = re.compile(r'//|/\*|(?<!\w)(?:b|c)?r(#*)"|"|\'(?:\\.|[^\'\\\n])\'')
    out, pos = [], 0
    while match := token.search(source, pos):
        out.append(source[pos:match.start()])
        start, kind = match.start(), match.group()
        if kind == "//":
            end = source.find("\n", match.end())
            end = len(source) if end < 0 else end
        elif kind == "/*":
            end, depth = match.end(), 1
            while depth:
                sub = re.search(r"/\*|\*/", source[end:])
                if not sub:
                    raise ValueError("unterminated Rust comment")
                depth += 1 if sub.group() == "/*" else -1
                end += sub.end()
        elif match.group(1) is not None:
            close = '"' + match.group(1)
            end = source.find(close, match.end())
            if end < 0:
                raise ValueError("unterminated Rust raw string")
            end += len(close)
        elif kind == '"':
            end = match.end()
            while end < len(source) and source[end] != '"':
                end += 2 if source[end] == "\\" else 1
            end += 1
        else:
            end = match.end()
        out.append(blank(source[start:end]))
        pos = end
    return "".join(out) + source[pos:]


def block_end(masked: str, start: int) -> int:
    depth = 0
    for at in range(start, len(masked)):
        if masked[at] == "{":
            depth += 1
        elif masked[at] == "}":
            depth -= 1
            if depth == 0:
                return at + 1
    raise ValueError("unclosed Rust item; audit syntax before inventorying")


def production(path: str, source: str) -> tuple[str, str]:
    """Remove only test items, never the rest of a mixed production/test file."""
    if path.endswith(".rs"):
        masked = mask_rust(source)
        spans = []
        for match in re.finditer(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]", masked):
            tail = re.match(r"(?:\s*#\[[^\]]*\])*\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:mod|fn)\b", masked[match.end():])
            if not tail:
                continue  # cfg(test) fields/arms retained conservatively
            start = masked.find("{", match.end())
            semi = masked.find(";", match.end())
            end = semi + 1 if 0 <= semi < start or start < 0 else block_end(masked, start)
            spans.append((match.start(), end))
        for start, end in reversed(spans):
            source = source[:start] + blank(source[start:end]) + source[end:]
            masked = masked[:start] + blank(masked[start:end]) + masked[end:]
        return source, masked
    return source, source


def sources(root: Path) -> dict[str, tuple[str, str]]:
    paths = set()
    # Discover new workspace-style crate roots too; a new crate cannot silently
    # escape the ratchet just because it was not in the initial audit.
    rust_roots = set(RUST_ROOTS) | {p.relative_to(root).as_posix() for p in root.glob("aida-*/src")}
    for directory in sorted(rust_roots):
        for suffix in ("*.rs", "*.py", "*.sh"):
            paths.update((root / directory).rglob(suffix))
    for directory in SCRIPT_ROOTS:
        for suffix in ("*.py", "*.sh"):
            paths.update((root / directory).rglob(suffix))
    result = {}
    for path in sorted(paths):
        rel = path.relative_to(root).as_posix()
        if any(p in {"tests", "test", "fixtures", "__pycache__"} for p in path.relative_to(root).parts) or path.name == "tests.rs" or path.name.startswith("test_") or path.name.endswith("_tests.rs"):
            continue
        if rel == "scripts/check-store-writers.py":
            continue  # read-only guard; covered by its independent unit fixtures
        if path.is_symlink():
            raise ValueError(f"source symlink needs explicit audit: {rel}")
        result[rel] = production(rel, path.read_text())
    return result


def module_path(path: str) -> str:
    crate, relative = path.split("/src/", 1)
    parts = list(Path(relative).with_suffix("").parts)
    if parts[-1] in {"lib", "main", "mod"}:
        parts.pop()
    return "::".join([crate.replace("-", "_"), *parts])


def functions(masked: str) -> list[tuple[int, int, str]]:
    """Lexical enclosing mod/impl/fn paths, with nested items kept distinct."""
    scopes = []
    pattern = re.compile(r"\b(fn|mod|impl|trait)(?=\s|<)\s*([^{};]+)")
    for match in pattern.finditer(masked):
        kind, header = match.groups()
        start = match.end()
        if start >= len(masked) or masked[start] != "{":
            continue
        if kind == "fn":
            name = re.match(r"(\w+)\s*(?:<.*>)?\s*\(", header)
            if not name:
                continue
            label = name.group(1)
        elif kind == "mod":
            label = header.strip()
        elif kind == "trait":
            label = "trait " + header.split(":", 1)[0].strip()
        else:
            label = "impl " + re.sub(r"\s+", " ", header.strip())
        end = block_end(masked, start)
        scopes.append((start, end, label, kind))
    return [(start + 1, end - 1,
             "::".join([label for begin, finish, label, _ in scopes
                        if begin < start < finish] + [label]))
            for start, end, label, kind in scopes if kind == "fn"]


def enclosing(items, at):
    index = bisect.bisect_right(items, (at, float("inf"), "")) - 1
    while index >= 0:
        start, end, name = items[index]
        if start <= at < end:
            return name
        index -= 1
    return "<module>"


CALL = re.compile(r"(?<![\w:])(?:[A-Za-z_]\w*::)+[A-Za-z_]\w*\s*(?=\()")


def function_identity(path, item):
    def impl_type(match):
        header = match.group(1).split(" for ")[-1]
        return re.sub(r"<[^<>]*>", "", header).strip()
    item = re.sub(r"impl (.+?)(?=::[^:]+$)", impl_type, item)
    return module_path(path) + "::" + item


def qualified(call, path, item=None):
    """Only explicit paths are propagated. Receiver/bare-name inference is unsafe."""
    call = re.sub(r"\s+", "", call)
    if "::" not in call:
        return None
    module = module_path(path)
    if call.startswith("Self::") and item and "impl " in item:
        return function_identity(path, item).rsplit("::", 1)[0] + call[4:]
    crate = module.split("::")[0]
    if call.startswith("crate::"):
        return crate + call[5:]
    if call.startswith("self::"):
        return module + call[4:]
    while call.startswith("super::"):
        module = module.rsplit("::", 1)[0]
        call = call[7:]
    if call.split("::")[0].startswith("aida_"):
        return call
    return module + "::" + call


def script_sites(path, source):
    if path.endswith(".py"):
        tree = ast.parse(source, filename=path)
        parents = {}
        for parent in ast.walk(tree):
            for child in ast.iter_child_nodes(parent):
                parents[child] = parent
        writers = {"write_text", "write_bytes", "write", "rename", "replace", "unlink",
                   "mkdir", "makedirs", "rmtree", "remove", "touch", "move", "copy", "copyfile", "copy2", "execute",
                   "executemany", "commit", "open", "dump", "safe_dump"}
        for node in ast.walk(tree):
            if not isinstance(node, ast.Call):
                continue
            name = ast.unparse(node.func)
            kind = name.rsplit(".", 1)[-1]
            subprocess_sink = None
            if name in {"subprocess.run", "subprocess.call", "subprocess.check_call",
                        "subprocess.check_output", "subprocess.Popen"}:
                argv = next((k.value for k in node.keywords if k.arg == "args"),
                            node.args[0] if node.args else None)
                if isinstance(argv, (ast.List, ast.Tuple)):
                    values = [v.value if isinstance(v, ast.Constant) and isinstance(v.value, str)
                              else "<dynamic>" for v in argv.elts]
                    subprocess_sink = argv_writer(values)
                elif (isinstance(argv, ast.Constant) and isinstance(argv.value, str)
                      and any(k.arg == "shell" and isinstance(k.value, ast.Constant)
                              and k.value.value is True for k in node.keywords)):
                    subprocess_sink = argv_writer(shlex.split(argv.value))
            elif name == "os.system" and node.args:
                command = node.args[0]
                if isinstance(command, ast.Constant) and isinstance(command.value, str):
                    subprocess_sink = argv_writer(shlex.split(command.value))
            if kind not in writers and not subprocess_sink:
                continue
            # open(..., 'r') and open(path) are reads, not write sites.
            if name.rsplit(".", 1)[-1] == "open":
                # Path.open(mode) differs from builtins/io.open(path, mode).
                mode_index = 1 if name in {"open", "io.open", "builtins.open", "os.open"} else 0
                mode = next((k.value for k in node.keywords if k.arg == "mode"),
                            node.args[mode_index] if len(node.args) > mode_index else ast.Constant("r"))
                if isinstance(mode, ast.Constant) and isinstance(mode.value, str) and not set(mode.value) & set("wax+"):
                    continue
            chain, parent = [], parents.get(node)
            while parent:
                if isinstance(parent, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                    chain.append(parent.name)
                parent = parents.get(parent)
            yield "::".join(reversed(chain)) or "<module>", "python:" + ("subprocess:" + subprocess_sink if subprocess_sink else kind)
    else:
        # Shell lexical tokens keep quotes/comments opaque without expanding
        # variables or treating embedded heredoc payloads as executable Python.
        token_pattern = r"\"(?:\\.|[^\"\\])*\"|'[^']*'|#[^\n]*|\$\{[^}]*\}|[;&|<>]+|[(){}]|[^\s;&|<>(){}]+"
        matches = [m for m in re.finditer(token_pattern, source)
                  if not m.group().startswith("#")]
        tokens = [m.group() for m in matches]
        scopes, stack = [], []
        for i, token in enumerate(tokens):
            if token == "{":
                label = None
                if i >= 3 and tokens[i-2:i] == ["(", ")"]:
                    label = tokens[i-3]
                elif i >= 2 and tokens[i-2] == "function":
                    label = tokens[i-1]
                stack.append((matches[i].end(), label))
            elif token == "}" and stack:
                start, label = stack.pop()
                if label and re.fullmatch(r"[\w-]+", label):
                    scopes.append((start, matches[i].start(), label))
        scopes.sort()
        for i, token in enumerate(tokens):
            item = "::".join(name for start, end, name in scopes
                             if start <= matches[i].start() < end) or "<script>"
            if executable_kind(token) in {"aida", "git"} and i + 1 < len(tokens):
                end = i + 1
                while end < len(tokens) and not re.fullmatch(r"[;&|<>]+|[(){}]", tokens[end]) and "\n" not in source[matches[end-1].end():matches[end].start()]:
                    end += 1
                sink = argv_writer(tokens[i:end])
                if sink:
                    yield item, sink
            # Audit literal shell payloads used by the multi-clone harness.
            # Ordinary quoted prose remains opaque; no expansion or execution.
            elif token in {"sh", "bash"} and tokens[i+1:i+2] == ["-c"] and i + 2 < len(tokens):
                payload = tokens[i+2]
                if payload.startswith(("'", '"')):
                    for _, sink in script_sites(path, shlex.split(payload)[0]):
                        yield item, sink
            # Command substitutions inside double quotes are executable shell,
            # unlike ordinary quoted prose. Nested/dynamic forms remain lexical.
            if not token.startswith("'"):
                for substitution in re.finditer(r"\$\((.*)\)", token, re.DOTALL):
                    for _, sink in script_sites(path, substitution.group(1)):
                        yield item, sink
            if token in {"mv", "cp", "rm", "mkdir", "tee", "install", ">", ">>"}:
                if token in {">", ">>"} and i + 1 < len(tokens) and tokens[i+1].strip("\"'") == "/dev/null":
                    continue
                yield item, token


def site_id(path, item, sink):
    return f"{path}|{item}|{sink}"


def scan(root):
    files = sources(root)
    parsed = {p: functions(m) for p, (_, m) in files.items() if p.endswith(".rs")}
    direct, edges = {}, {}
    result = {}
    for path, (source, masked) in files.items():
        if not path.endswith(".rs"):
            for item, sink in script_sites(path, source):
                result[site_id(path, item, sink)] = dict(path=path, item=item, sink=sink)
            continue
        items = parsed[path]
        for hit in SEED.finditer(masked):
            # The seed vocabulary contains facade methods as well as raw I/O.
            # A declaration is not a call, including nested function headers.
            if re.search(r"\bfn\s+$", masked[max(0, hit.start() - 80):hit.start()]):
                continue
            item = enclosing(items, hit.start())
            sink = re.sub(r"\s+", "", hit.group()).rstrip("(")
            result[site_id(path, item, sink)] = dict(path=path, item=item, sink=sink)
            direct.setdefault(function_identity(path, item), set()).add(site_id(path, item, sink))
        for start, end, item in items:
            identity = function_identity(path, item)
            calls = set()
            for hit in CALL.finditer(masked[start:end]):
                if enclosing(items, start + hit.start()) != item:
                    continue
                target = qualified(hit.group(), path, item)
                if target:
                    calls.add(target)
            edges[identity] = calls
    wrappers = set(direct)
    while True:
        expanded = wrappers | {name for name, calls in edges.items() if calls & wrappers}
        if expanded == wrappers:
            break
        wrappers = expanded
    for path, items in parsed.items():
        masked = files[path][1]
        for hit in CALL.finditer(masked):
            item = enclosing(items, hit.start())
            target = qualified(hit.group(), path, item)
            if target in wrappers:
                sink = "wrapper:" + target
                result[site_id(path, item, sink)] = dict(path=path, item=item, sink=sink)
    return result, files, parsed, edges


def discover(root: Path) -> dict[str, dict]:
    return scan(root)[0]


def operations(root, scanned=None):
    """Account for every registered CLI command and dispatched MCP tool.

    Unknown trait/receiver/legacy dispatch is a blocker, never silently read-only.
    """
    sites, files, parsed, edges = scanned or scan(root)
    result = {}
    by_function = {}
    for key, site in sites.items():
        if site["path"].endswith(".rs"):
            by_function.setdefault(function_identity(site["path"], site["item"]), set()).add(key)

    def resolve(surface, selector, entrypoints, note=None):
        visited, pending, found = set(), list(entrypoints), set()
        while pending:
            name = pending.pop()
            if name in visited:
                continue
            visited.add(name)
            found.update(by_function.get(name, ()))
            pending.extend(edges.get(name, set()) - visited)
        result[surface + ":" + selector] = dict(surface=surface, selector=selector,
            entrypoints=entrypoints, sites=sorted(found),
            resolution_note=note or ("Qualified lexical sink routes; receiver/trait and child edges still require review."
                if found else f"No qualified sink route from {', '.join(entrypoints)}; reconcile this operation's receiver/import/child calls before cutover."))

    for path, (source, masked) in files.items():
        if not path.endswith(".rs"):
            continue
        module = module_path(path)
        # Registration bodies are structural selectors, not string write hits.
        for match in re.finditer(r"\bCommandReg\s*\{", masked):
            start = source.find("{", match.start())
            body = source[start:block_end(masked, start)]
            name = re.search(r'\bname:\s*"([^"\n]+)"', body)
            run = re.search(r"\bHandler::\w+\(\s*([\w:]+)\s*\)", body)
            if name:
                handler = run.group(1) if run else "<unresolved-handler>"
                entry = qualified(handler, path) if "::" in handler else module + "::" + handler
                resolve("cli", name.group(1), [entry])
        if path == "aida-cli-lib/src/mcp.rs":
            for match in re.finditer(r'"([a-z_]+)"\s*=>\s*self\s*\.\s*(tool_\w+)', source):
                item = next((item for _, _, item in parsed[path] if item.split("::")[-1] == match.group(2)), match.group(2))
                resolve("mcp", match.group(1), [function_identity(path, item)])
    # Enumerate every immediate operation of the core writing command groups,
    # including unresolved routes. Never treat missing lexical edges as reads.
    groups = {"QueueCommand": "queue", "CommentCommand": "comment",
              "FindingsCommand": "findings", "MailboxCommand": "mailbox",
              "SessionCommand": "session"}
    for path, (source, masked) in files.items():
        for match in re.finditer(r"\benum\s+(\w+)\s*\{", masked):
            if match.group(1) not in groups:
                continue
            start = masked.find("{", match.start()) + 1
            end = block_end(masked, start - 1) - 1
            depth, expecting, attribute_start = 0, True, start
            for token in re.finditer(r"[A-Za-z_]\w*|[{}()\[\],]", masked[start:end]):
                value = token.group()
                if value in {"{", "(", "["}:
                    depth += 1
                elif value in {"}", ")", "]"}:
                    depth -= 1
                elif depth == 0 and value == ",":
                    expecting, attribute_start = True, start + token.end()
                elif depth == 0 and expecting and value[0].isupper():
                    attrs = source[attribute_start:start + token.start()]
                    override = re.search(r'(?:clap|command)\s*\(\s*name\s*=\s*"([^"\n]+)"', attrs)
                    name = override.group(1) if override else re.sub(r"(?<!^)(?=[A-Z])", "-", value).lower()
                    selector = groups[match.group(1)] + " " + name
                    resolve("cli", selector, [f"<unresolved:{path}::{match.group(1)}::{value}>"])
                    expecting = False
    # Split important compound CLI writers at the concrete operation seam.
    explicit = {
        "claim": ("claim", "handle_claim"),
        "comment edit": ("comment_cmd", "edit_comment_interactive"),
        "comment delete": ("comment_cmd", "delete_comment"),
        "comment add": ("comment_cmd", "add_comment_cli_relayed"),
        "findings add": ("findings_gate", "handle_findings_add"),
        "session handoff": ("seat_rotation", "write_handoff_by"),
        "session handoff standing-orders add": ("seat_rotation", "add_standing_order"),
        "mailbox send": ("mailbox_store", "write_message_with_deliveries"),
        "mailbox digest": ("mailbox_store", "digest_snapshot_to_canonical"),
    }
    for selector, (module, function) in explicit.items():
        if (root / f"aida-cli-lib/src/{module}.rs").exists():
            resolve("cli", selector, [f"aida_cli_lib::{module}::{function}"])
    # trace:TASK-3222 | ai:codex
    # Reviewed bare-name edge: defer_resurface::run calls sweep. Do not infer
    # other bare calls globally, where unrelated names can collide.
    if "aida-cli-lib/src/defer_resurface.rs" in files:
        resolve("cli", "defer resurface",
                [f"aida_cli_lib::defer_resurface::{name}" for name in ("run", "sweep")],
                "Reviewed run_defer -> defer_resurface::run -> sweep route, including the bare sweep call; atomic spec updates and human-queue writes remain blocked for TASK-3223 authority and receiver review.")
    # Explicit reviewed receiver/import edges, never global bare-name propagation.
    mcp_path = "aida-cli-lib/src/mcp.rs"
    if mcp_path in files:
        for selector in ("queue_add", "queue_rework"):
            if "mcp:" + selector not in result:
                continue
            names = ["tool_" + selector, "tool_" + selector + "_inner"]
            entries = [function_identity(mcp_path, item) for _, _, item in parsed[mcp_path]
                       if item.split("::")[-1] in names]
            resolve("mcp", selector, entries,
                    "Reviewed self.tool_*_inner receiver edge; outer authority checks remain required; backend receiver dispatch is a cutover blocker.")
        for selector in ("resolve_punt", "escalate_punt"):
            key = "mcp:" + selector
            if key in result:
                resolve("mcp", selector, result[key]["entrypoints"] +
                        [f"aida_cli_lib::punt::{function}" for function in
                         (selector + "_core", "write_punt_response", "close_open_records")],
                        "Reviewed punt module import and core calls to write_punt_response/close_open_records; response/ledger writers need operation-specific host authority.")
        for selector in ("queue_work", "db_sync", "pull", "session_start", "session_end", "role_enter", "role_end"):
            key = "mcp:" + selector
            if key in result:
                result[key]["resolution_note"] = (f"tool_{selector} is PEEK-only: returns a CLI invocation without performing that mutation. "
                    "Keep blocked until the corresponding CLI operation and any incidental reads/cache effects are reconciled.")
    queue_path = "aida-cli-lib/src/queue_cmd.rs"
    if queue_path in files:
        source, masked = files[queue_path]
        arm = re.search(r"QueueCommand::Done\s*\{[^}]*\}\s*=>\s*\{", masked)
        if arm:
            start, end = arm.end(), block_end(masked, arm.end()-1)-1
            item = enclosing(parsed[queue_path], start)
            entry = function_identity(queue_path, item) + "::arm QueueCommand::Done"
            kinds = {re.sub(r"\s+", "", h.group()) for h in SEED.finditer(masked[start:end])}
            targets = {qualified(h.group(), queue_path, item) for h in CALL.finditer(masked[start:end])}
            by_function[entry] = {key for key, site in sites.items() if site["path"] == queue_path
                                 and site["item"] == item and (site["sink"] in kinds or
                                 site["sink"].removeprefix("wrapper:") in targets)}
            # Do not reuse the whole dispatcher's outgoing edges.
            edges = dict(edges)
            edges[entry] = targets - {None}
            resolve("cli", "queue done", [entry],
                    "Concrete QueueCommand::Done arm only; sink IDs retain their enclosing function identity. Receiver writes and shared pre-match effects require cutover review.")
    return result


def check(root: Path, inventory: Path | None = None, scanned=None) -> list[str]:
    data = tomllib.loads((inventory or root / INVENTORY).read_text())
    if data.get("version") != 2:
        raise ValueError("inventory needs sink-site format version=2; regenerate after merging main")
    rows = data.get("writer", [])
    paths, ids, errors = {}, set(), []
    required = ("id", "path", "item", "sink", "lock", "gate", "disposition", "proposed_operation", "evidence_tests")
    for row in rows:
        if not isinstance(row, dict):
            raise ValueError("writer record must be a table")
        if any(not row.get(key) for key in required):
            errors.append(f"incomplete writer row: {row.get('id', '<missing>')}")
            continue
        key = site_id(row["path"], row["item"], row["sink"])
        if row["id"] != key:
            errors.append(f"writer id must match site: {key}")
        if key in paths or row["id"] in ids:
            errors.append(f"duplicate writer id/site: {key}")
        paths[key] = row
        ids.add(row["id"])
        if Path(row["path"]).is_absolute() or ".." in Path(row["path"]).parts:
            errors.append(f"unsafe path: {key}")
        if row["disposition"] not in DISPOSITIONS:
            errors.append(f"invalid classification: {key}")
        if row["disposition"] == "blocked" and not row.get("blocker"):
            errors.append(f"blocked row needs a cutover blocker: {key}")
    scanned = scanned or scan(root)
    current = scanned[0]
    for key in sorted(current.keys() - paths.keys()):
        errors.append(f"unclassified writer/caller sink site (addition): {key}")
    for key in sorted(paths.keys() - current.keys()):
        errors.append(f"stale writer row (delete row): {key}")
    expected = operations(root, scanned)
    registered = {}
    for row in data.get("operation", []):
        key = row["surface"] + ":" + row["selector"]
        if key in registered:
            errors.append(f"duplicate operation: {key}")
        registered[key] = row
        if any(not row.get(f) for f in ("lock", "gate", "proposed_operation", "evidence_tests", "blocker")) or row.get("disposition") != "blocked":
            errors.append(f"operation needs explicit cutover blocker: {key}")
    for key in sorted(expected.keys() | registered.keys()):
        if key not in expected or key not in registered or any(registered[key].get(f) != expected[key][f] for f in ("entrypoints", "sites", "resolution_note")):
            errors.append(f"operation mapping changed: {key}")
    return errors


def regenerate(root):
    """Keep reviewed site policy; new sites start blocked, never authorized."""
    target = root / INVENTORY
    old = tomllib.loads(target.read_text()) if target.exists() else {}
    prior = {site_id(r["path"], r["item"], r["sink"]): r for r in old.get("writer", []) if "item" in r}
    scanned = scan(root)
    lines = ["# Source-review evidence only; no runtime authority. trace:TASK-3222 | ai:codex", "version = 2", "", "writer = ["]
    for key, site in sorted(scanned[0].items()):
        row = prior.get(key, dict(id=key, **site, lock="caller-owned; inspect enclosing item before cutover",
            gate="caller policy; no service authority in S1a", disposition="blocked",
            blocker="TASK-3223: resolve destination and dynamic caller/trait policy before routing",
            proposed_operation="none until typed destination and authority review",
            evidence_tests=["tests/test_store_writer_inventory.py"]))
        lines.append("  { " + ", ".join(f"{k} = {json.dumps(v)}" for k, v in row.items()) + " },")
    lines += ["]", "", "operation = ["]
    prior_ops = {r["id"]: r for r in old.get("operation", [])}
    for key, evidence in sorted(operations(root, scanned).items()):
        row = dict(id=key, **evidence, disposition="blocked",
                   lock="unresolved dynamic/receiver route; cutover blocker",
                   gate="operation authority must be revalidated at host write boundary",
                   proposed_operation="unresolved; no S1 mutation verb",
                   evidence_tests=["tests/test_store_writer_inventory.py"],
                   blocker="TASK-3223: confirm receiver/trait and deferred-child edges and operation-specific authority; lexical mapping grants no routing")
        for field in ("lock", "gate", "proposed_operation", "evidence_tests", "blocker"):
            if field in prior_ops.get(key, {}):
                row[field] = prior_ops[key][field]
        lines.append("  { " + ", ".join(f"{k} = {json.dumps(v)}" for k, v in row.items()) + " },")
    lines += ["]", ""]
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text("\n".join(lines))
    return scanned


# trace:TASK-3223 | ai:codex
def mediated_blockers(data):
    """Report unresolved review evidence, never infer routing from a sink map.

    The v2 operation schema deliberately requires blocked dispositions. A
    reconciled inventory therefore cannot certify mediated-profile readiness;
    routing needs reviewed operation policy and evidence before this can pass.
    Only host-only and lane-private writer sites are outside routing in v2.
    Neither s2_typed nor read_only carries verified routing evidence in this
    schema. All operation mappings remain unresolved, regardless of labels.
    """
    return {
        kind: sorted(row["id"] for row in data.get(kind, [])
                     if kind == "operation" or row.get("disposition") not in
                     {"host_only", "lane_private"})
        for kind in ("writer", "operation")
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--candidates", action="store_true", help="print sink-site discovery evidence")
    parser.add_argument("--regenerate", action="store_true", help="reconcile sites after merging main; new sites remain blocked; review diff")
    parser.add_argument("--profile", choices=("compatibility", "mediated"),
                        default="compatibility",
                        help="mediated additionally refuses unresolved routing evidence; never grants launch authority")
    args = parser.parse_args()
    try:
        if args.candidates:
            print(json.dumps(discover(args.root), indent=2, sort_keys=True))
            return 0
        scanned = regenerate(args.root) if args.regenerate else None
        errors = check(args.root, scanned=scanned)
        if errors:
            print("store-writer inventory refused:\n" + "\n".join(errors) +
                  "\nAfter merging current main, run python3 scripts/check-store-writers.py --regenerate; review additions and delete stale rows in the inventory diff.", file=sys.stderr)
            return 1
        data = tomllib.loads((args.root / INVENTORY).read_text())
        if args.profile == "mediated":
            blockers = mediated_blockers(data)
            print(json.dumps(dict(profile="mediated", unresolved=blockers,
                                  counts={kind: len(rows) for kind, rows in blockers.items()}),
                             sort_keys=True))
            if any(blockers.values()):
                print("mediated profile refused: reconciled inventory still has unresolved routing evidence; "
                      "keep production admission gated", file=sys.stderr)
                return 1
        print(f"store-writers: {len(data['writer'])} sink sites reconciled; {len(data.get('operation', []))} operation mappings with explicit S2 cutover blockers; no runtime authority granted")
        return 0
    except (OSError, ValueError, TypeError, KeyError, SyntaxError) as error:
        print(f"store-writer inventory refused: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
