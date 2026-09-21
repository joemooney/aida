#!/usr/bin/env python3
"""Regression tests for the marker-driven classifier enumerator."""

import contextlib
import importlib.util
import io
import pathlib
import sys
import tempfile
import unittest

SCRIPT = pathlib.Path(__file__).parents[1] / "scripts/external-prose-classifiers.py"
SPEC = importlib.util.spec_from_file_location("classifier_inventory", SCRIPT)
module = importlib.util.module_from_spec(SPEC)
assert SPEC.loader
SPEC.loader.exec_module(module)


class EnumeratorTest(unittest.TestCase):
    def test_new_marker_is_enumerated(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            source = root / "aida-example/src/new.rs"
            source.parent.mkdir(parents=True)
            source.write_text(
                "fn classify(s: &str) -> bool {\n"
                "    // external-prose-classifier: fixture::new_classifier\n"
                '    s.contains("upstream prose")\n}\n',
                encoding="utf-8",
            )
            self.assertEqual(
                module.enumerate_sites(root),
                [("fixture::new_classifier", "aida-example/src/new.rs", 2)],
            )

    def test_line_drift_above_a_marker_does_not_change_the_inventory(self):
        """BUG-1526 criterion 4.

        The inventory identity is name + PATH. Inserting unrelated lines ABOVE a
        marked site shifts its line number and must NOT make the rendered doc
        stale — that drift is exactly what reddened eleven open PRs, each losing
        its whole test suite because the check ran ahead of `Run tests`.
        """
        def tree(root: pathlib.Path, preamble: str) -> str:
            source = root / "aida-example/src/new.rs"
            source.parent.mkdir(parents=True, exist_ok=True)
            source.write_text(
                preamble
                + "fn classify(s: &str) -> bool {\n"
                "    // external-prose-classifier: fixture::new_classifier\n"
                '    s.contains("upstream prose")\n}\n',
                encoding="utf-8",
            )
            return module.render(module.enumerate_sites(root))

        with tempfile.TemporaryDirectory() as a, tempfile.TemporaryDirectory() as b:
            before = tree(pathlib.Path(a), "")
            after = tree(pathlib.Path(b), "// padding\n" * 40)

        self.assertEqual(
            before, after,
            "inserting lines above a marked site must not change the rendered inventory",
        )
        self.assertNotIn(":2", before, "the rendered row must not carry a line number")

    def test_a_new_marker_still_changes_the_inventory(self):
        """The guard must still fire on a real SET change — otherwise criterion 4
        could be satisfied by making the check vacuous."""
        def tree(root: pathlib.Path, extra: str) -> str:
            source = root / "aida-example/src/new.rs"
            source.parent.mkdir(parents=True, exist_ok=True)
            source.write_text(
                "fn classify(s: &str) -> bool {\n"
                "    // external-prose-classifier: fixture::new_classifier\n"
                '    s.contains("upstream prose")\n}\n' + extra,
                encoding="utf-8",
            )
            return module.render(module.enumerate_sites(root))

        with tempfile.TemporaryDirectory() as a, tempfile.TemporaryDirectory() as b:
            before = tree(pathlib.Path(a), "")
            after = tree(
                pathlib.Path(b),
                "fn other(s: &str) -> bool {\n"
                "    // external-prose-classifier: fixture::second_classifier\n"
                '    s.contains("more prose")\n}\n',
            )

        self.assertNotEqual(
            before, after, "adding a NEW classifier marker must change the inventory"
        )

    def test_a_moved_classifier_changes_the_inventory(self):
        """BUG-1526 c1: identity is name + PATH, so a MOVE is a change.

        The reviewer broke the previous pair by attacking the unpinned axis. The
        other two tests vary LINE POSITION (name+path fixed) and the NAME SET
        (paths fixed); neither varies PATH with the name held constant, so the
        path half of the identity was unpinned. Two mutations passed all three:

            render() -> str(len(rows))            the doc becomes one digit
            render() -> name only, path dropped   silently reverts c1

        The second is the dangerous one — it is plausible as a refactor, and
        under it a classifier MOVING FILE stops registering as a change. This
        test closes both, because a move alters neither the row count nor the
        name set.
        """
        def tree(root: pathlib.Path, rel: str) -> str:
            src = root / rel
            src.parent.mkdir(parents=True, exist_ok=True)
            src.write_text(
                "fn classify(s: &str) -> bool {\n"
                "    // external-prose-classifier: fixture::new_classifier\n"
                '    s.contains("upstream prose")\n}\n',
                encoding="utf-8",
            )
            return module.render(module.enumerate_sites(root))

        with tempfile.TemporaryDirectory() as a, tempfile.TemporaryDirectory() as b:
            before = tree(pathlib.Path(a), "aida-example/src/old.rs")
            after = tree(pathlib.Path(b), "aida-example/src/moved.rs")

        self.assertNotEqual(
            before, after,
            "a classifier moving file must change the inventory — identity is name + PATH",
        )

    def _tree_with_one_marker(self, root: pathlib.Path) -> None:
        src = root / "aida-example/src/new.rs"
        src.parent.mkdir(parents=True, exist_ok=True)
        src.write_text(
            "fn classify(s: &str) -> bool {\n"
            "    // external-prose-classifier: fixture::new_classifier\n"
            '    s.contains("upstream prose")\n}\n',
            encoding="utf-8",
        )
        (root / "docs/architecture").mkdir(parents=True, exist_ok=True)

    def _run_check(self, root: pathlib.Path) -> int:
        argv = sys.argv
        sys.argv = ["external-prose-classifiers.py", "--check", "--root", str(root)]
        try:
            return module.main()
        finally:
            sys.argv = argv

    def test_check_fails_on_a_stale_doc_and_passes_on_a_fresh_one(self):
        """BUG-1526 criterion 3, and the gate itself.

        Every other test in this file calls only enumerate_sites() and render().
        Nothing exercised --check, so `if args.check: return 0` — one line —
        killed the gate with the whole suite green, and a known-stale doc exited
        0. Criterion 3's diagnostic lives entirely in main() and had no coverage
        at all, which is why a defect in it had to be found by reading.
        """
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            self._tree_with_one_marker(root)
            doc = root / "docs/architecture/external-tool-output-classifiers.md"

            doc.write_text("DELIBERATELY WRONG\n", encoding="utf-8")
            self.assertEqual(
                self._run_check(root), 1,
                "--check must FAIL on a stale doc; a gate that cannot fail is not a gate",
            )

            doc.write_text(
                module.render(module.enumerate_sites(root)), encoding="utf-8"
            )
            self.assertEqual(
                self._run_check(root), 0, "--check must PASS once the doc is regenerated"
            )

    def test_check_diagnostic_reports_a_move_as_a_change(self):
        """BUG-1526 criterion 3, PATH axis of the DIAGNOSTIC.

        The gate test above only proves --check returns 1 on a stale doc. It
        does not constrain what the message SAYS, and criterion 3 promises the
        message names what changed. Reading only the name cell made a MOVED
        classifier report "the classifier set is unchanged" — the same name-only
        blind spot that let a dropped path through the renderer tests. Fixing it
        without this test left the fix itself unpinned: reverting to cell [1]
        alone kept all five tests green.
        """
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            self._tree_with_one_marker(root)
            doc = root / "docs/architecture/external-tool-output-classifiers.md"

            # the doc describes the SAME classifier at a DIFFERENT path
            fresh = module.render(module.enumerate_sites(root))
            doc.write_text(
                fresh.replace("aida-example/src/new.rs", "aida-example/src/old.rs"),
                encoding="utf-8",
            )

            err = io.StringIO()
            with contextlib.redirect_stderr(err):
                rc = self._run_check(root)
            message = err.getvalue()

        self.assertEqual(rc, 1, "a moved classifier must fail --check")
        self.assertNotIn(
            "set is unchanged", message,
            "a move IS a change — reporting it as unchanged is the name-only defect",
        )
        self.assertIn("added:", message)
        self.assertIn("removed:", message)


if __name__ == "__main__":
    unittest.main()
