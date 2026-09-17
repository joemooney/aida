#!/usr/bin/env python3
import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path


REPO = Path(__file__).resolve().parents[1]
SCRIPT = REPO / "docs" / "cli" / "verify-manual.py"


def load_verify_manual():
    spec = importlib.util.spec_from_file_location("verify_manual", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    sys.modules["verify_manual"] = module
    spec.loader.exec_module(module)
    return module


class VerifyManualFlagsTest(unittest.TestCase):
    def setUp(self):
        self.vm = load_verify_manual()
        self.old_help_flags = self.vm.help_flags
        self.old_help_command_paths = self.vm.help_command_paths
        self.vm.help_flags.cache_clear()

    def tearDown(self):
        self.vm.help_flags = self.old_help_flags
        self.vm.help_command_paths = self.old_help_command_paths

    def test_command_paths_accept_unindented_help_commands_rows(self):
        def help_commands(_args, capture_output, text, timeout=None):
            self.assertTrue(capture_output)
            self.assertTrue(text)

            class Result:
                stdout = ""

            if _args == ["aida", "help", "commands"]:
                Result.stdout = """aida add                              Add a new requirement
aida advisor register                 Record the current session
  aida queue done                      Mark queue item done
"""
            elif _args == ["aida", "help-all"]:
                Result.stdout = """  add  Add a new requirement
  advisor  The advisor seat
  queue  Personal work queue commands
"""
            elif _args[-1:] == ["--help"]:
                command = " ".join(_args[1:-1])
                Result.stdout = f"Usage: aida {command} [OPTIONS]\n"

            return Result()

        old_run = self.vm.subprocess.run
        self.vm.subprocess.run = help_commands
        try:
            self.assertEqual(
                self.vm.help_command_paths(),
                {
                    ("add",),
                    ("advisor",),
                    ("advisor", "register"),
                    ("queue",),
                    ("queue", "done"),
                },
            )
        finally:
            self.vm.subprocess.run = old_run

    def test_false_positive_flags_resolve_through_command_subtree(self):
        paths = {("add",), ("advisor",), ("advisor", "register")}
        flag_map = {
            "add": {"--type", "--title", "--format"},
            "advisor": {"--format"},
            "advisor register": {"--uuid", "--format"},
        }

        def fake_help_flags(cmd_path):
            return flag_map.get(cmd_path, set())

        self.vm.help_flags = fake_help_flags
        self.vm.help_command_paths = lambda: paths

        with tempfile.TemporaryDirectory() as td:
            chapter = Path(td) / "manual.md"
            chapter.write_text(
                "\n".join(
                    [
                        "### `aida add`",
                        "`--type` creates the typed spec.",
                        "`--title` overrides the positional title.",
                        "Global `--format` works under the add command.",
                        "### `aida advisor`",
                        "`aida advisor register --uuid abc --format json` records a session.",
                    ]
                )
            )
            documented = self.vm.parse_chapters([str(chapter)])

        self.assertEqual(self.vm.flag_accuracy_misses(documented, paths), [])


if __name__ == "__main__":
    unittest.main()
