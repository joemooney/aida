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
