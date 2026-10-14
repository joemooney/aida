#!/usr/bin/env python3
import importlib.util
import contextlib
import io
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from types import SimpleNamespace


REPO = Path(__file__).resolve().parents[1]
SCRIPT = REPO / "docs" / "cli" / "verify-manual.py"


def load_verify_manual():
    spec = importlib.util.spec_from_file_location("verify_manual", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    sys.modules["verify_manual"] = module
    spec.loader.exec_module(module)
    return module


class VerifyManualCompletenessTest(unittest.TestCase):
    # trace:BUG-2121 | ai:codex
    HELP_ALL = """Requirements
  add            Add a new requirement
  import         Import from external sources
  help           Print help
  help-all       Full inventory grouped by topic

External commands
  gui                 external /home/joe/.cargo/bin/aida-gui
  host-bench          external /home/joe/.local/bin/aida-host-bench
"""

    def setUp(self):
        self.vm = load_verify_manual()

    def test_help_all_excludes_extensions_preserves_builtins(self):
        with patch.object(
            self.vm.subprocess, "run", return_value=SimpleNamespace(stdout=self.HELP_ALL)
        ) as run:
            self.assertEqual(self.vm.help_all_commands(), ["add", "import"])
        run.assert_called_once_with(["aida", "help-all"], capture_output=True, text=True)

    def test_completeness_ignores_extensions_but_rejects_missing_builtin(self):
        for undocumented, expected_exit in [(False, 0), (True, 1)]:
            with self.subTest(undocumented=undocumented), tempfile.TemporaryDirectory() as td:
                chapter = Path(td) / "01-fixture.md"
                chapter.write_text("### `aida add`\n### `aida import`\n")
                output = io.StringIO()
                help_all = self.HELP_ALL
                if undocumented:
                    help_all += "\nRequirements\n  missing        Undocumented built-in\n"
                with contextlib.ExitStack() as stack:
                    stack.enter_context(patch.object(sys, "argv", [str(SCRIPT)]))
                    stack.enter_context(patch.object(self.vm, "CLI_DIR", td))
                    stack.enter_context(patch.object(
                        self.vm.subprocess, "run", return_value=SimpleNamespace(stdout=help_all)
                    ))
                    stack.enter_context(patch.object(self.vm, "all_surface_changing_specs", return_value=({}, True)))
                    stack.enter_context(patch.object(self.vm, "subcommand_misses", return_value=[]))
                    stack.enter_context(patch.object(self.vm, "help_command_paths", return_value=set()))
                    stack.enter_context(patch.object(self.vm, "flag_accuracy_misses", return_value=[]))
                    stack.enter_context(contextlib.redirect_stdout(output))
                    with self.assertRaises(SystemExit) as exit_result:
                        self.vm.main()
                self.assertEqual(exit_result.exception.code, expected_exit)
                self.assertNotIn("aida gui", output.getvalue())
                self.assertNotIn("aida host-bench", output.getvalue())
                if undocumented:
                    self.assertIn("FAIL completeness — 1 command(s)", output.getvalue())
                    self.assertIn("aida missing", output.getvalue())
                else:
                    self.assertIn("OK completeness — all 2 help-all commands documented", output.getvalue())


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


class VerifyManualSubcommandTest(unittest.TestCase):
    """Cited subcommands must exist in the CLI. trace:BUG-1949 | ai:claude"""

    # A fake CLI: `session` and `role` (with a nested `scope`) have verbs;
    # `show` takes a positional, so it never rejects a word as a subcommand.
    KNOWN = {
        "session",
        "session conversations",
        "session resume",
        "session prune",
        "session gc",
        "role",
        "role scope",
        "role scope set",
        "role scope clear",
        "role show",
        "show",
        "pr",
    }
    WITH_VERBS = {"session", "role", "role scope"}

    def setUp(self):
        self.vm = load_verify_manual()

    def known(self, path):
        return path in self.KNOWN

    def unrecognized(self, path):
        parent = path.rsplit(" ", 1)[0] if " " in path else ""
        return parent in self.WITH_VERBS and path not in self.KNOWN

    def _misses(self, lines):
        with tempfile.TemporaryDirectory() as td:
            chapter = Path(td) / "manual.md"
            chapter.write_text("\n".join(lines) + "\n")
            cites = self.vm.subcommand_citations([str(chapter)])
        return [
            (m["path"], m["line"])
            for m in self.vm.subcommand_misses(cites, self.unrecognized, self.known)
        ]

    def test_subcommand_citations_flag_cut_session_verb(self):
        # The original drift: a cut verb in a slash list, a `vs` bullet, an
        # explicit backtick invocation and a fenced example all fail.
        misses = self._misses(
            [
                "### `aida session`",
                "Conversations (`conversations` / `resume` / `prune` / `forget`).",
                "- `prune` vs `forget` — bulk vs one-by-id.",
                "Run `aida session forget <id>` to drop one.",
                "```bash",
                "$ aida session forget abc",
                "```",
            ]
        )
        self.assertEqual(
            misses,
            [
                ("session forget", 2),
                ("session forget", 3),
                ("session forget", 4),
                ("session forget", 6),
            ],
        )

    def test_subcommand_citations_extract_verb_lists(self):
        # Real verbs, a nested `scope` list, positionals and PATH extensions pass.
        misses = self._misses(
            [
                "### `aida session`",
                "Conversations (`conversations` / `resume` / `prune`); cleanup is `gc`.",
                "`aida show forget` reads a spec; `aida deploy --dry-run` is an extension.",
                "### `aida role`",
                "- `scope` (`set` / `show` / `clear`) — standing filter.",
                "Roles (`advisor` / `implementer` / `reviewer`) are names, not verbs.",
            ]
        )
        self.assertEqual(misses, [])

    def test_subcommand_misses_reports_unrecognized_only(self):
        # A minority-known list is not a verb list; a top-level command cited
        # beside a subcommand is a cross-reference; a nested miss is reported.
        misses = self._misses(
            [
                "### `aida session`",
                "Reads (`list` / `search` / `resume`) are graph reads.",
                "The `pr` / `resume` / `prune` verbs.",
                "### `aida role`",
                "- `scope` (`set` / `clear` / `wipe`) — standing filter.",
                "## Notes",
                "`set` / `wipe` outside a command section is not a citation.",
            ]
        )
        self.assertEqual(misses, [("role scope wipe", 5)])

    def test_subcommand_unrecognized_reads_clap_error(self):
        class Result:
            def __init__(self, stderr):
                self.stderr = stderr
                self.stdout = ""

        def fake_run(args, capture_output, text, timeout=None):
            if args == ["aida", "session", "forget", "--help"]:
                return Result("error: unrecognized subcommand 'forget'\n")
            return Result("")

        old_run = self.vm.subprocess.run
        self.vm.subprocess.run = fake_run
        self.vm.subcommand_unrecognized.cache_clear()
        try:
            self.assertTrue(self.vm.subcommand_unrecognized("session forget"))
            self.assertFalse(self.vm.subcommand_unrecognized("session prune"))
        finally:
            self.vm.subprocess.run = old_run
            self.vm.subcommand_unrecognized.cache_clear()


if __name__ == "__main__":
    unittest.main()
