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



class VerifyManualStaticFlagsTest(unittest.TestCase):
    """No-build hidden-flag guard run by the pre-push tier. trace:BUG-1937 | ai:claude"""

    CLAP_SOURCE = """
pub enum Commands {
    /// Cut a release.
    Release {
        /// Preview only.
        #[clap(long)]
        preview: bool,
        /// No-op kept for compatibility.
        // a plain comment between attribute and field
        #[clap(long, hide = true)]
        skip_xplat_check: bool,
        #[clap(
            long = "after-pr",
            value_name = "N",
            help = "a long help that says hide = true in prose"
        )]
        after_pr: Option<u64>,
        #[arg(long, alias = "old-name", visible_alias = "shown-name")]
        new_name: bool,
        #[clap(long)]
        #[clap(hide = true)]
        split_hidden: bool,
        #[clap(long)]
        r#type: Option<String>,
        #[clap(hide = true)]
        positional: Option<String>,
    },
    #[clap(hide = true)]
    Secret,
}
pub struct Other {
    #[clap(long, hide = true)]
    preview: bool,
}
"""

    def setUp(self):
        self.vm = load_verify_manual()

    def test_static_clap_parse_visible_vs_hidden(self):
        visible, hidden = self.vm.clap_long_flags(self.CLAP_SOURCE)
        self.assertEqual(
            visible,
            {"--preview", "--after-pr", "--new-name", "--shown-name", "--type"},
        )
        self.assertEqual(
            hidden,
            {"--skip-xplat-check", "--old-name", "--split-hidden", "--preview"},
        )

    def _cites(self, lines):
        with tempfile.TemporaryDirectory() as td:
            chapter = Path(td) / "manual.md"
            chapter.write_text("\n".join(lines) + "\n")
            return self.vm.static_flag_citations([str(chapter)])

    def test_static_hidden_flag_citation_fails(self):
        visible, hidden = self.vm.clap_long_flags(self.CLAP_SOURCE)
        cites = self._cites(
            [
                "### `aida release`",
                "Pass `--skip-xplat-check` to skip the gate.",
                "Run aida release --old-name in prose.",
            ]
        )
        misses = self.vm.static_hidden_flag_misses(cites, visible, hidden)
        self.assertEqual(
            [(m["flag"], m["parent"], m["line"]) for m in misses],
            [("--skip-xplat-check", "release", 2), ("--old-name", "release", 3)],
        )

    def test_static_flag_visible_elsewhere_passes(self):
        # `--preview` is hidden on one struct but visible on another command.
        visible, hidden = self.vm.clap_long_flags(self.CLAP_SOURCE)
        cites = self._cites(["### `aida release`", "`aida release --preview` previews."])
        self.assertEqual(self.vm.static_hidden_flag_misses(cites, visible, hidden), [])

    def test_static_scope_and_external_tool_spans_ignored(self):
        visible, hidden = self.vm.clap_long_flags(self.CLAP_SOURCE)
        cites = self._cites(
            [
                "Intro prose citing `--skip-xplat-check` outside any command.",
                "### `aida release`",
                "`git push --split-hidden` is git's flag, not ours.",
                "## Notes",
                "`--skip-xplat-check` after the section ended.",
            ]
        )
        self.assertEqual(self.vm.static_hidden_flag_misses(cites, visible, hidden), [])

    def test_static_mode_passes_on_current_manual(self):
        files = sorted(str(p) for p in (REPO / "docs" / "cli").glob("[0-9][0-9]-*.md"))
        self.assertTrue(files)
        self.assertEqual(self.vm.main_static(files), 0)


if __name__ == "__main__":
    unittest.main()
