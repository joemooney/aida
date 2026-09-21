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


if __name__ == "__main__":
    unittest.main()
