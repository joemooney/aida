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
                "fn new_classifier(s: &str) -> bool {\n"
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
                + "fn new_classifier(s: &str) -> bool {\n"
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
                "fn new_classifier(s: &str) -> bool {\n"
                "    // external-prose-classifier: fixture::new_classifier\n"
                '    s.contains("upstream prose")\n}\n' + extra,
                encoding="utf-8",
            )
            return module.render(module.enumerate_sites(root))

        with tempfile.TemporaryDirectory() as a, tempfile.TemporaryDirectory() as b:
            before = tree(pathlib.Path(a), "")
            after = tree(
                pathlib.Path(b),
                "fn second_classifier(s: &str) -> bool {\n"
                "    // external-prose-classifier: fixture::second_classifier\n"
                '    s.contains("more prose")\n}\n',
            )

        self.assertNotEqual(
            before, after, "adding a NEW classifier marker must change the inventory"
        )

    def test_a_source_file_move_does_not_change_the_inventory(self):
        """TASK-1309: file location is diagnostic, not classifier identity."""
        def tree(root: pathlib.Path, rel: str) -> str:
            src = root / rel
            src.parent.mkdir(parents=True, exist_ok=True)
            src.write_text(
                "fn new_classifier(s: &str) -> bool {\n"
                "    // external-prose-classifier: fixture::new_classifier\n"
                '    s.contains("upstream prose")\n}\n',
                encoding="utf-8",
            )
            return module.render(module.enumerate_sites(root))

        with tempfile.TemporaryDirectory() as a, tempfile.TemporaryDirectory() as b:
            before = tree(pathlib.Path(a), "aida-example/src/old.rs")
            after = tree(pathlib.Path(b), "aida-example/src/moved.rs")

        self.assertEqual(
            before, after,
            "moving an unchanged module-qualified classifier must not change the inventory",
        )

    def test_marker_moved_to_a_different_function_is_rejected(self):
        """A stale semantic anchor must not hide a changed classifier."""
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            src = root / "aida-example/src/new.rs"
            src.parent.mkdir(parents=True)
            src.write_text(
                "fn changed_classifier(s: &str) -> bool {\n"
                "    // external-prose-classifier: fixture::old_classifier\n"
                '    s.contains("prose")\n}\n',
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "anchored to function changed_classifier"):
                module.enumerate_sites(root)

    def test_marker_after_a_closed_function_has_no_anchor(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            src = root / "aida-example/src/new.rs"
            src.parent.mkdir(parents=True)
            src.write_text(
                "fn old_classifier() {}\n"
                "// external-prose-classifier: fixture::old_classifier\n",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "has no function anchor"):
                module.enumerate_sites(root)

    def test_closed_nested_function_restores_the_outer_scope(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            src = root / "aida-example/src/new.rs"
            src.parent.mkdir(parents=True)
            src.write_text(
                "fn outer_classifier() {\n"
                "    fn inner_classifier() {}\n"
                "    // external-prose-classifier: fixture::outer_classifier\n"
                "}\n",
                encoding="utf-8",
            )
            self.assertEqual(
                module.enumerate_sites(root),
                [("fixture::outer_classifier", "aida-example/src/new.rs", 3)],
            )

    def test_marker_inside_nested_function_uses_nested_scope(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            src = root / "aida-example/src/new.rs"
            src.parent.mkdir(parents=True)
            src.write_text(
                "fn outer() {\n"
                "    fn inner_classifier() {\n"
                "        // external-prose-classifier: fixture::inner_classifier\n"
                "    }\n"
                "}\n",
                encoding="utf-8",
            )
            self.assertEqual(
                module.enumerate_sites(root),
                [("fixture::inner_classifier", "aida-example/src/new.rs", 3)],
            )

    def test_removed_marker_changes_the_inventory(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            self._tree_with_one_marker(root)
            before = module.render(module.enumerate_sites(root))
            (root / "aida-example/src/new.rs").write_text(
                "fn new_classifier(s: &str) -> bool { s.contains(\"prose\") }\n",
                encoding="utf-8",
            )
            after = module.render(module.enumerate_sites(root))
        self.assertNotEqual(before, after)

    def test_duplicate_semantic_key_is_rejected_as_ambiguous(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            for filename in ("one.rs", "two.rs"):
                src = root / f"aida-example/src/{filename}"
                src.parent.mkdir(parents=True, exist_ok=True)
                src.write_text(
                    "fn same_classifier() {\n"
                    "    // external-prose-classifier: fixture::same_classifier\n}\n",
                    encoding="utf-8",
                )
            with self.assertRaisesRegex(ValueError, "duplicate classifier marker"):
                module.enumerate_sites(root)

    def _tree_with_one_marker(self, root: pathlib.Path) -> None:
        src = root / "aida-example/src/new.rs"
        src.parent.mkdir(parents=True, exist_ok=True)
        src.write_text(
            "fn new_classifier(s: &str) -> bool {\n"
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

            # the doc describes a DIFFERENT stable classifier key
            fresh = module.render(module.enumerate_sites(root))
            doc.write_text(
                fresh.replace("fixture::new_classifier", "fixture::old_classifier"),
                encoding="utf-8",
            )

            err = io.StringIO()
            with contextlib.redirect_stderr(err):
                rc = self._run_check(root)
            message = err.getvalue()

        self.assertEqual(rc, 1, "a renamed classifier must fail --check")
        self.assertNotIn(
            "set is unchanged", message,
            "a rename IS a set change — reporting it as unchanged hides inventory drift",
        )
        self.assertIn("added:", message)
        self.assertIn("removed:", message)


if __name__ == "__main__":
    unittest.main()
