#!/usr/bin/env python3
"""Mutation and integration fixtures for the BUG-1728 timestamp rule.

trace:BUG-1728 | ai:codex
"""

from __future__ import annotations

import importlib.util
import pathlib
import re
import shutil
import subprocess
import tempfile
import textwrap
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[1]
CHECK = ROOT / "scripts" / "check-portability.sh"
SCANNER = ROOT / "scripts" / "check_file_time_portability.py"
RULES = ROOT / "scripts" / "portability-rules.json"
ALLOWLIST = ROOT / "scripts" / "portability-allowlist.txt"
AIDA_BIN = ROOT / "aida-cli-lib" / "src" / "aida_bin.rs"
RECONSTITUTE = ROOT / "aida-cli-lib" / "src" / "reconstitute.rs"

SPEC = importlib.util.spec_from_file_location("file_time_portability", SCANNER)
assert SPEC and SPEC.loader
file_time_portability = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(file_time_portability)


def run_portability_check(sources: dict[str, str]) -> subprocess.CompletedProcess[str]:
    with tempfile.TemporaryDirectory() as temporary:
        root = pathlib.Path(temporary).resolve()
        scripts = root / "scripts"
        scripts.mkdir()
        shutil.copy(CHECK, scripts / "check-portability.sh")
        shutil.copy(SCANNER, scripts / SCANNER.name)
        shutil.copy(RULES, scripts / RULES.name)
        shutil.copy(ALLOWLIST, scripts / ALLOWLIST.name)
        for relative, body in sources.items():
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(textwrap.dedent(body).lstrip())

        subprocess.run(["git", "init", "-q"], cwd=root, check=True)
        subprocess.run(["git", "config", "user.email", "fixture@example.invalid"], cwd=root, check=True)
        subprocess.run(["git", "config", "user.name", "Fixture"], cwd=root, check=True)
        subprocess.run(["git", "add", "--", *sources], cwd=root, check=True)
        return subprocess.run(
            ["bash", str(scripts / "check-portability.sh"), "--print-findings"],
            cwd=root,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            env={
                "PATH": "/usr/bin:/bin",
                "GIT_CEILING_DIRECTORIES": str(root),
                "HOME": temporary,
            },
        )


class FileTimePortabilityRuleTest(unittest.TestCase):
    def test_mutating_the_four_fixed_file_opens_restores_all_findings(self):
        source = AIDA_BIN.read_text()
        mutated, replacements = re.subn(
            r"std::fs::OpenOptions::new\(\)\s*\.write\(true\)\s*\.open\(",
            "std::fs::File::open(",
            source,
        )
        self.assertEqual(replacements, 4, "the historical aida_bin sites changed unexpectedly")
        self.assertEqual(file_time_portability.scan_source(source), [])
        findings = file_time_portability.scan_source(mutated)
        self.assertEqual(len(findings), 4, findings)
        self.assertEqual(
            [method for _, _, method in findings],
            ["set_times", "set_times", "set_modified", "set_modified"],
        )

    def test_non_windows_directory_handle_is_not_a_windows_finding(self):
        self.assertEqual(file_time_portability.scan_source(RECONSTITUTE.read_text()), [])

    def test_check_portability_fails_for_a_file_open_handle_time_mutation(self):
        result = run_portability_check(
            {
                "src/timestamp.rs": """
                    #[cfg(test)]
                    mod tests {
                        fn unsafe_fixture() {
                            let file = std::fs::File::open("tool.exe").unwrap();
                            file.set_times(std::fs::FileTimes::new()).unwrap();
                        }
                    }
                """,
            }
        )
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("readonly-file-time-mutation", result.stdout)
        self.assertIn("file.set_times()", result.stderr)

    def test_check_portability_allows_a_writable_file_handle(self):
        result = run_portability_check(
            {
                "src/timestamp.rs": """
                    #[cfg(test)]
                    mod tests {
                        fn safe_fixture() {
                            let file = std::fs::OpenOptions::new()
                                .write(true)
                                .open("tool.exe")
                                .unwrap();
                            file.set_times(std::fs::FileTimes::new()).unwrap();
                        }
                    }
                """,
            }
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("readonly-file-time-mutation", result.stdout)


if __name__ == "__main__":
    unittest.main()
