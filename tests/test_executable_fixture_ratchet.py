"""Keep fake forge executables on the shared ETXTBSY-safe fixture helper."""

from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[1]


def function_body(source: str, name: str) -> str | None:
    match = re.search(rf"\bfn\s+{re.escape(name)}\s*\([^{{]*\)\s*(?:->[^{{]+)?\{{", source)
    if not match:
        return None
    start = match.end()
    depth = 1
    for index in range(start, len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                return source[start:index]
    raise AssertionError(f"unterminated Rust function {name}")


def local_fake_gh_violations(source: str) -> list[str]:
    violations = []
    for match in re.finditer(r"\bfn\s+(\w*fake_gh\w*)\s*\(", source):
        name = match.group(1)
        body = function_body(source, name)
        if body is None:
            continue
        if "std::fs::write" not in body and "set_mode(" not in body and "from_mode" not in body:
            continue
        if "test_exec::write_executable" not in body:
            violations.append(f"{name} does not use test_exec::write_executable")
        if "set_mode(" in body or "Permissions::from_mode" in body:
            violations.append(f"{name} sets executable permissions itself")
    return violations


def direct_executable_mode_changes(source: str) -> list[str]:
    violations = []
    for match in re.finditer(r"(?:set_mode|from_mode)\s*\(\s*(0o[0-7]+)", source):
        mode = int(match.group(1), 8)
        # Deliberately read-only directories are separate permission fixtures,
        # not executables used as process fixtures.
        if mode & 0o111 and mode != 0o555:
            line = source.count("\n", 0, match.start()) + 1
            violations.append(f"line {line}: executable mode must use test_exec::mark_executable")
    return violations


# --- BUG-1725: cfg(unix)-aware scan for unix-only APIs across every crate `src` ---
#
# The discriminator for "does this break the Windows build" is not the directory a
# file lives in -- every site that has actually broken the nightly lives outside
# the one directory the original check walked. It is whether the site is lexically
# inside a scope whose `#[cfg(..)]` predicate cannot be true on Windows.
#
# Two measurements on this tree shaped the implementation:
#   * Matching the literal `cfg(unix)` reports correct code. The tree also guards
#     with `#[cfg(all(test, unix))]`, `#[cfg(target_os = "linux")]` and
#     `#[cfg(not(windows))]`, so the predicate is evaluated rather than matched.
#   * Guard-to-violation distance reaches 90 lines, so no proximity window can
#     substitute for tracking braces and attributes.
# See BUG-1725.

_RAW_STRING_START = re.compile(r"b?r(?P<hashes>#*)\"")
_IDENT_CHAR = re.compile(r"[A-Za-z0-9_]")
_CFG_ATTRIBUTE = re.compile(r"\s*cfg\s*\((?P<predicate>.*)\)\s*$", re.DOTALL)
_UNIX_ONLY_PATH = re.compile(r"\bstd::os::unix\b")
_UNIX_ONLY_PERMISSIONS = re.compile(r"\b(?:set_mode|from_mode)\b")


def _split_top_level(text: str) -> list[str]:
    parts: list[str] = []
    current: list[str] = []
    depth = 0
    for char in text:
        if char == "(":
            depth += 1
        elif char == ")":
            depth -= 1
        if char == "," and depth == 0:
            parts.append("".join(current))
            current = []
        else:
            current.append(char)
    parts.append("".join(current))
    return [part.strip() for part in parts if part.strip()]


def evaluate_cfg_on_windows(predicate: str) -> bool | None:
    """Tri-state value of a `cfg` predicate when compiling for Windows.

    `None` means "depends on something we do not model" — `test`, a feature, an
    architecture. Only a predicate that is definitely `False` keeps unix-only
    code away from the Windows compiler, so only `False` counts as a guard.
    """
    predicate = predicate.strip()
    combinator = re.match(r"(all|any|not)\s*\((?P<body>.*)\)\s*$", predicate, re.DOTALL)
    if combinator:
        operands = [evaluate_cfg_on_windows(part) for part in _split_top_level(combinator.group("body"))]
        if combinator.group(1) == "not":
            inner = operands[0] if operands else None
            return None if inner is None else not inner
        if combinator.group(1) == "all":
            if any(operand is False for operand in operands):
                return False
            return True if all(operand is True for operand in operands) else None
        if any(operand is True for operand in operands):
            return True
        return False if all(operand is False for operand in operands) else None
    keyed = re.match(r'(\w+)\s*=\s*"([^"]*)"\s*$', predicate)
    if keyed:
        key, value = keyed.group(1), keyed.group(2)
        if key in {"target_os", "target_family"}:
            return value == "windows"
        return None
    if predicate == "unix":
        return False
    if predicate == "windows":
        return True
    return None


def mask_rust_noise(source: str) -> str:
    """Blank comments and literals, preserving offsets and newlines.

    Braces and `#[..]` inside a doc comment or a string literal would otherwise
    corrupt the scope tracking below.
    """
    out = list(source)
    length = len(source)

    def blank(start: int, end: int) -> None:
        for index in range(start, min(end, length)):
            if out[index] != "\n":
                out[index] = " "

    index = 0
    while index < length:
        char = source[index]
        if char == "/" and source.startswith("//", index):
            end = source.find("\n", index)
            end = length if end == -1 else end
            blank(index, end)
            index = end
            continue
        if char == "/" and source.startswith("/*", index):
            depth = 1
            cursor = index + 2
            while cursor < length and depth:
                if source.startswith("/*", cursor):
                    depth += 1
                    cursor += 2
                elif source.startswith("*/", cursor):
                    depth -= 1
                    cursor += 2
                else:
                    cursor += 1
            blank(index, cursor)
            index = cursor
            continue
        prefixed = index > 0 and bool(_IDENT_CHAR.match(source[index - 1]))
        if char in "rb" and not prefixed:
            raw = _RAW_STRING_START.match(source, index)
            if raw:
                terminator = '"' + raw.group("hashes")
                end = source.find(terminator, raw.end())
                end = length if end == -1 else end + len(terminator)
                blank(index, end)
                index = end
                continue
        if (char == '"' or (char == "b" and source.startswith('b"', index))) and not (
            char == "b" and prefixed
        ):
            cursor = index + (2 if char == "b" else 1)
            while cursor < length:
                if source[cursor] == "\\":
                    cursor += 2
                    continue
                if source[cursor] == '"':
                    cursor += 1
                    break
                cursor += 1
            blank(index, cursor)
            index = cursor
            continue
        if (char == "'" or (char == "b" and source.startswith("b'", index))) and not (
            char == "b" and prefixed
        ):
            body = index + (2 if char == "b" else 1)
            if body < length and source[body] == "\\":
                end = source.find("'", body + 1)
                if end != -1:
                    blank(index, end + 1)
                    index = end + 1
                    continue
            elif body + 1 < length and source[body + 1] == "'":
                blank(index, body + 2)
                index = body + 2
                continue
            # Anything else beginning with a quote is a lifetime, not a literal.
            index += 1
            continue
        index += 1
    return "".join(out)


def unix_guarded_regions(masked: str, source: str) -> list[tuple[int, int]]:
    """Offset spans that only compile on unix, per their `#[cfg(..)]` attribute.

    A scope guards its contents when its predicate cannot be true on Windows, so
    `#[cfg(unix)]`, `#[cfg(all(test, unix))]` and `#[cfg(target_os = "linux")]`
    are all honoured; matching the literal `cfg(unix)` reports correct code as
    broken, and this tree contains all three shapes.
    """
    length = len(masked)
    regions: list[tuple[int, int]] = []
    stack: list[int | None] = []
    pending_start: int | None = None
    paren_depth = 0
    index = 0
    while index < length:
        char = masked[index]
        if char == "#":
            cursor = index + 1
            inner = cursor < length and masked[cursor] == "!"
            if inner:
                cursor += 1
            if cursor < length and masked[cursor] == "[":
                depth = 1
                end = cursor + 1
                while end < length and depth:
                    if masked[end] == "[":
                        depth += 1
                    elif masked[end] == "]":
                        depth -= 1
                    end += 1
                # Bracket depth is counted on the masked text so a `]` inside a
                # string cannot close the attribute, but the predicate itself is
                # read from the original: masking blanks `"linux"` out of
                # `cfg(target_os = "linux")` and the guard would be missed.
                content = source[cursor + 1 : end - 1]
                cfg = _CFG_ATTRIBUTE.match(content.strip())
                if cfg and evaluate_cfg_on_windows(cfg.group("predicate")) is False:
                    if inner:
                        # `#![cfg(unix)]` guards everything from here to the end
                        # of the enclosing scope, which for a module file is the
                        # whole file.
                        regions.append((index, length))
                    elif pending_start is None:
                        pending_start = index
                index = end
                continue
        if char == "(":
            paren_depth += 1
        elif char == ")":
            paren_depth = max(0, paren_depth - 1)
        elif char == "{":
            stack.append(pending_start)
            pending_start = None
        elif char == "}":
            opened = stack.pop() if stack else None
            if opened is not None:
                regions.append((opened, index + 1))
            pending_start = None
        elif char == ";":
            if pending_start is not None:
                # An attribute on a brace-less item, e.g. a guarded `use`.
                regions.append((pending_start, index + 1))
            pending_start = None
        elif char == "," and paren_depth == 0:
            pending_start = None
        index += 1
    return regions


def unix_only_api_violations(source: str) -> list[str]:
    """Unix-only API uses that a Windows compile would reach.

    Covers both halves of the family that held the cross-platform nightly red:
    the executable-mode calls, and the bare `use std::os::unix::…` import, which
    carries no mode literal and is the `E0433` that fires first.
    """
    masked = mask_rust_noise(source)
    regions = unix_guarded_regions(masked, source)

    def guarded(offset: int) -> bool:
        return any(start <= offset < end for start, end in regions)

    violations = []
    for match in _UNIX_ONLY_PATH.finditer(masked):
        if not guarded(match.start()):
            line = masked.count("\n", 0, match.start()) + 1
            violations.append(f"line {line}: std::os::unix is reachable from a Windows compile; put it behind #[cfg(unix)]")
    # No mode filter here. `direct_executable_mode_changes` exempts non-executable
    # modes because they are not process fixtures, which is a question about the
    # `test_exec` policy. This check asks a different question -- whether the
    # Windows compiler can reach the call -- and `from_mode(0o644)` is exactly as
    # unix-only as `from_mode(0o755)`. `aida_bin.rs` proved it: its `0o644` site
    # was one of the eight errors in the nightly this bug was filed from.
    for match in _UNIX_ONLY_PERMISSIONS.finditer(masked):
        if not guarded(match.start()):
            line = masked.count("\n", 0, match.start()) + 1
            violations.append(
                f"line {line}: {match.group(0)} is reachable from a Windows compile; "
                "use crate::test_exec::mark_executable or a #[cfg(unix)] scope"
            )
    return sorted(violations, key=lambda entry: int(entry.split()[1].rstrip(":")))


def crate_source_roots() -> list[Path]:
    """Every crate `src` in the workspace.

    The original guard walked `aida-cli-lib/src/tests` alone, which is the one
    place the breakage never occurred: every site that has actually broken the
    Windows nightly lives in an inline `#[cfg(test)] mod` inside a production
    file.
    """
    roots = []
    for manifest in sorted(ROOT.glob("*/Cargo.toml")):
        source_root = manifest.parent / "src"
        if source_root.is_dir():
            roots.append(source_root)
    return roots


class ExecutableFixtureRatchetTests(unittest.TestCase):
    def test_fake_gh_helpers_use_shared_fixture_writer(self):
        source_root = ROOT / "aida-cli-lib/src"
        offenders = []
        for path in source_root.rglob("*.rs"):
            if path.name == "test_exec.rs":
                continue
            source = path.read_text(encoding="utf-8")
            for violation in local_fake_gh_violations(source):
                offenders.append(f"{path.relative_to(ROOT)}: {violation}")
        self.assertEqual([], offenders, "\n".join(offenders))

    def test_test_sources_do_not_set_executable_modes_directly(self):
        source_root = ROOT / "aida-cli-lib/src/tests"
        offenders = []
        for path in source_root.rglob("*.rs"):
            source = path.read_text(encoding="utf-8")
            for violation in direct_executable_mode_changes(source):
                offenders.append(f"{path.relative_to(ROOT)}: {violation}")
        self.assertEqual([], offenders, "\n".join(offenders))

    def test_fixture_executability_uses_the_shared_writer(self):
        source_root = ROOT / "aida-cli-lib/src/tests"
        offenders = []
        for path in source_root.rglob("*.rs"):
            source = path.read_text(encoding="utf-8")
            for match in re.finditer(r"crate::test_exec::mark_executable\(([^)]*)\)", source):
                # These calls restore temporary directory permissions after
                # read-only tests; actual process fixtures must use write_executable.
                if match.group(1).strip() not in {"&ro", "&locked_dir"}:
                    line = source.count("\n", 0, match.start()) + 1
                    offenders.append(
                        f"{path.relative_to(ROOT)}:{line}: fixture must use write_executable"
                    )
        self.assertEqual([], offenders, "\n".join(offenders))

    def test_unix_only_apis_are_guarded_across_every_crate_source(self):
        offenders = []
        for source_root in crate_source_roots():
            for path in sorted(source_root.rglob("*.rs")):
                source = path.read_text(encoding="utf-8")
                for violation in unix_only_api_violations(source):
                    offenders.append(f"{path.relative_to(ROOT)}: {violation}")
        self.assertEqual([], offenders, "\n".join(offenders))

    def test_crate_source_roots_reach_past_the_original_blind_spot(self):
        roots = {root.relative_to(ROOT).as_posix() for root in crate_source_roots()}
        self.assertIn("aida-cli-lib/src", roots)
        self.assertIn("aida-core/src", roots)

    def test_checker_rejects_an_unguarded_inline_test_module(self):
        # The exact shape of the family that held the nightly red: a production
        # file whose inline `#[cfg(test)] mod tests` reaches a unix-only API.
        bad = (
            "pub fn resolve() {}\n"
            "#[cfg(test)]\n"
            "mod tests {\n"
            "    use std::os::unix::fs::PermissionsExt;\n"
            "    #[test]\n"
            "    fn t() {\n"
            "        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();\n"
            "    }\n"
            "}\n"
        )
        self.assertEqual(
            [4, 7], [int(entry.split()[1].rstrip(":")) for entry in unix_only_api_violations(bad)]
        )

    def test_checker_accepts_a_guarded_inline_test_module(self):
        good = (
            "pub fn resolve() {}\n"
            "#[cfg(all(test, unix))]\n"
            "mod tests {\n"
            "    use std::os::unix::fs::PermissionsExt;\n"
            "    #[test]\n"
            "    fn t() {\n"
            "        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();\n"
            "    }\n"
            "}\n"
        )
        self.assertEqual([], unix_only_api_violations(good))

    def test_checker_accepts_every_guard_shape_in_this_tree(self):
        # One fixture per shape the workspace actually uses. A matcher for the
        # literal `cfg(unix)` passes the first and fails the rest, and would go
        # red in the gating ubuntu job on correct code.
        shapes = {
            "guarded block": "fn f() {\n#[cfg(unix)]\n{\nuse std::os::unix::fs::PermissionsExt;\np.set_mode(0o755);\n}\n}\n",
            "guarded item": "#[cfg(unix)]\nfn f() {\nuse std::os::unix::fs::PermissionsExt;\np.set_mode(0o755);\n}\n",
            "guarded mod": "#[cfg(unix)]\nmod m {\nuse std::os::unix::fs::PermissionsExt;\nfn f() { p.set_mode(0o755); }\n}\n",
            "compound predicate": "#[cfg(all(test, unix))]\nmod m {\nuse std::os::unix::fs::PermissionsExt;\n}\n",
            "unix-implying target_os": "#[cfg(target_os = \"linux\")]\nfn f() {\nuse std::os::unix::fs::OpenOptionsExt;\n}\n",
            "guarded brace-less item": "#[cfg(unix)]\nuse std::os::unix::fs::PermissionsExt;\n",
            "negated windows": "#[cfg(not(windows))]\nfn f() {\nuse std::os::unix::fs::PermissionsExt;\n}\n",
        }
        for shape, source in shapes.items():
            with self.subTest(shape=shape):
                self.assertEqual([], unix_only_api_violations(source))

    def test_checker_rejects_guards_that_still_reach_windows(self):
        # `any(unix, windows)` and a bare `cfg(test)` both compile on Windows.
        shapes = {
            "any including windows": "#[cfg(any(unix, windows))]\nfn f() {\nuse std::os::unix::fs::PermissionsExt;\n}\n",
            "test only": "#[cfg(test)]\nmod m {\nuse std::os::unix::fs::PermissionsExt;\n}\n",
            "runtime cfg macro": "fn f() {\nif cfg!(unix) {\nuse std::os::unix::fs::PermissionsExt;\n}\n}\n",
        }
        for shape, source in shapes.items():
            with self.subTest(shape=shape):
                self.assertTrue(unix_only_api_violations(source), shape)

    def test_checker_ignores_comments_and_string_literals(self):
        noise = (
            '// use std::os::unix::fs::PermissionsExt;\n'
            '/* p.set_mode(0o755); */\n'
            'fn f() { let s = "use std::os::unix::fs::PermissionsExt;"; }\n'
            'fn g() { let s = r#"{ from_mode(0o755) }"#; }\n'
        )
        self.assertEqual([], unix_only_api_violations(noise))

    def test_non_executable_modes_are_still_exempt_from_the_fixture_policy(self):
        # AC1 of BUG-1725: the `0o555`/non-executable exemption in the fixture
        # policy check is preserved untouched. The Windows-reachability check
        # deliberately does not inherit it.
        self.assertEqual([], direct_executable_mode_changes("permissions.set_mode(0o555);"))
        self.assertEqual([], direct_executable_mode_changes("permissions.set_mode(0o644);"))
        self.assertTrue(unix_only_api_violations("permissions.set_mode(0o644);"))

    def test_checker_rejects_a_future_local_fake_gh_copy(self):
        bad = """fn fake_gh(path: &Path, body: &str) {\n std::fs::write(path, body).unwrap();\n chmod(path, 0o755);\n }"""
        self.assertTrue(local_fake_gh_violations(bad))

    def test_checker_accepts_the_shared_helper(self):
        good = """fn fake_gh(path: &Path, body: &str) {\n crate::test_exec::write_executable(path, body);\n }"""
        self.assertEqual([], local_fake_gh_violations(good))

    def test_checker_rejects_direct_executable_mode(self):
        self.assertTrue(direct_executable_mode_changes("permissions.set_mode(0o755);"))


if __name__ == "__main__":
    unittest.main()
