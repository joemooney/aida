#!/usr/bin/env python3
"""Fixtures for scripts/check-template-commands.py.

The lint exists because a skill template told agents to run
`aida list --search`, a flag clap rejects, so every run of that skill failed at
its dedup step. The parsing tests run without a binary; the end-to-end fixtures
need an `aida` binary (`$AIDA_BIN`, else `target/debug/aida`, else PATH).

trace:BUG-1922 | ai:claude
"""
import importlib.util
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
CHECK = ROOT / "scripts" / "check-template-commands.py"

_spec = importlib.util.spec_from_file_location("check_template_commands", CHECK)
lint = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(lint)


def find_aida():
    env = os.environ.get("AIDA_BIN")
    if env:
        return env
    built = ROOT / "target" / "debug" / "aida"
    if built.exists():
        return str(built)
    return shutil.which("aida")


AIDA = find_aida()


class ParsingTests(unittest.TestCase):
    def test_only_fenced_aida_lines_are_extracted(self):
        text = textwrap.dedent(
            """\
            Run aida list --search x in prose: not a command.

            ```bash
            aida search "kw"   # comment
            $ aida show BUG-1
            echo aida list --search x
            aida add --title "t" \\
              --description "d"
            ```
            """
        )
        got = list(lint.fenced_aida_lines(text))
        self.assertEqual(
            got,
            [
                (4, 'aida search "kw"   # comment'),
                (5, "aida show BUG-1"),
                (7, 'aida add --title "t" --description "d"'),
            ],
        )

    def test_tokenize_drops_comments_and_stops_at_pipes(self):
        toks = lint.tokenize('aida list --status approved | head  # trailing')
        self.assertEqual(toks[:3], ["aida", "list", "--status"])
        self.assertIn("|", toks)
        self.assertNotIn("trailing", toks)

    def test_type_alias_rewrite_mirrors_cli(self):
        self.assertIn("faq", lint.builtin_type_words())
        self.assertNotIn("doc", lint.builtin_type_words())
        self.assertEqual(
            lint.rewrite_type_alias(["faq", "list", "completed"]),
            ["list", "--type", "faq", "completed"],
        )
        self.assertEqual(lint.rewrite_type_alias(["doc", "list"]), ["doc", "list"])

    def test_allow_marker_skips_line(self):
        self.assertEqual(
            lint.check_command("aida list --search x  # lint:allow-unknown"), []
        )


@unittest.skipUnless(AIDA, "no aida binary (set AIDA_BIN or build target/debug/aida)")
class BinaryFixtureTests(unittest.TestCase):
    def run_lint(self, body):
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / "fixture.md"
            path.write_text(body, encoding="utf-8")
            return subprocess.run(
                [sys.executable, str(CHECK), str(path)],
                capture_output=True,
                text=True,
                env={**os.environ, "AIDA_BIN": AIDA},
                timeout=120,
            )

    def test_unknown_flag_fails(self):
        res = self.run_lint('```bash\naida list --search "x"\n```\n')
        self.assertEqual(res.returncode, 1, res.stdout + res.stderr)
        self.assertIn("--search", res.stderr)

    def test_unknown_command_fails(self):
        res = self.run_lint("```bash\naida definitely-not-a-command --x\n```\n")
        self.assertEqual(res.returncode, 1, res.stdout + res.stderr)
        self.assertIn("unknown command", res.stderr)

    def test_valid_commands_pass(self):
        body = textwrap.dedent(
            """\
            ```bash
            aida search "<keyword>" --limit 5
            aida faq list completed --tags topic:x
            aida show BUG-1 --full
            ```
            """
        )
        res = self.run_lint(body)
        self.assertEqual(res.returncode, 0, res.stdout + res.stderr)

    def test_shipped_templates_pass(self):
        res = subprocess.run(
            [sys.executable, str(CHECK)],
            capture_output=True,
            text=True,
            env={**os.environ, "AIDA_BIN": AIDA},
            timeout=600,
        )
        self.assertEqual(res.returncode, 0, res.stdout + res.stderr)


if __name__ == "__main__":
    unittest.main()
