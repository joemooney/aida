#!/usr/bin/env python3
"""Regression tests for the marker-driven classifier enumerator."""

import importlib.util
import pathlib
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


if __name__ == "__main__":
    unittest.main()
