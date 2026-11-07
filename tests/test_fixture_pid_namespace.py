# trace:BUG-2476 | ai:codex
import importlib.util
import json
import os
from pathlib import Path
from types import SimpleNamespace
import subprocess
import tempfile
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location("mcp_fixture", Path(__file__).with_name("test_mcp_stdio.py"))
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class ProcessBindingTests(unittest.TestCase):
    def binding(self, namespace):
        stat = "42 (fixture process) " + " ".join(["0"] * 19 + ["12345"])
        with patch.object(fixture.os, "getpid", return_value=42), \
                patch.object(fixture.Path, "read_text", return_value=stat), \
                patch.object(fixture.Path, "stat", return_value=SimpleNamespace(st_ino=namespace)):
            return fixture.own_process_binding()

    def test_host_binding_remains_unqualified(self):
        self.assertEqual(self.binding(0xEFFFFFFC), ["proc:42@linux-starttime:12345"])

    def test_container_binding_records_its_namespace(self):
        self.assertEqual(self.binding(123), ["proc:42@pidns-123/linux-starttime:12345"])
        self.assertNotEqual(self.binding(123), self.binding(456))

    def test_unreadable_namespace_refuses_to_mint_binding(self):
        with patch.object(fixture.Path, "stat", side_effect=PermissionError), \
                self.assertRaises(PermissionError):
            fixture.own_process_binding()


class ShellBindingTests(unittest.TestCase):
    def test_shell_grant_records_the_observed_namespace(self):
        root = Path(__file__).resolve().parents[1]
        with tempfile.TemporaryDirectory() as directory:
            project = Path(directory) / "project"
            home = Path(directory) / "home"
            result = subprocess.run([
                "bash", "-c", 'source "$1"; aida_seat_grant "$2" "$3" fixture advisor',
                "fixture", str(root / "tests/lib/seat_grant.sh"), str(project), str(home),
            ], check=True, capture_output=True, text=True)
            grant = json.loads((home / ".aida/session-grants" / f"{result.stdout.strip()}.json").read_text())
            namespace = os.stat("/proc/self/ns/pid").st_ino
            prefix = "" if namespace == 0xEFFFFFFC else f"pidns-{namespace}/"
            self.assertRegex(grant["binding"][0], rf"^proc:\d+@{prefix}linux-starttime:\d+$")


if __name__ == "__main__":
    unittest.main()
