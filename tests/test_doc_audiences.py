"""Audience inventory regressions. No AIDA store or build required."""
# trace:STORY-1651 | ai:codex
import contextlib
import csv
import importlib.util
import io
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("doc_inventory", ROOT / "docs/archive/2026-10-cleanup/inventory.py")
inventory = importlib.util.module_from_spec(spec)
spec.loader.exec_module(inventory)


class AudienceTests(unittest.TestCase):
    def check_rows(self, paths, rows):
        with tempfile.TemporaryDirectory() as tmp:
            table = Path(tmp) / "inventory.tsv"
            with table.open("w") as output:
                writer = csv.writer(output, delimiter="\t")
                writer.writerow(inventory.HEADER)
                writer.writerows([[p, "maintained-doc", "kept", p, "-", "-", audience] for p, audience in rows])
            with patch.object(inventory, "TSV", str(table)), patch.object(inventory, "tracked_paths", return_value=paths), contextlib.redirect_stdout(io.StringIO()):
                return inventory.check()

    def test_new_doc_requires_inventory_entry(self):
        self.assertEqual(self.check_rows(["docs/getting-started.md"], []), 1)

    def test_duplicate_invalid_stale_and_wrong_audience_refuse(self):
        for rows in [
            [("docs/getting-started.md", "adopter")] * 2,
            [("docs/getting-started.md", "unknown")],
            [("docs/getting-started.md", "internal")],
            [("missing.md", "internal")],
        ]:
            with self.subTest(rows=rows):
                self.assertEqual(self.check_rows(["docs/getting-started.md"], rows), 1)

    def test_complete_valid_inventory(self):
        paths = ["docs/getting-started.md", "docs/advanced/mailbox.md", "docs/internal/monitor-contract.md"]
        self.assertEqual(self.check_rows(paths, list(zip(paths, ["adopter", "advanced", "internal"]))), 0)

    def test_unknown_top_level_topic_needs_audience_decision(self):
        self.assertIsNone(inventory.audience("docs/new-unclassified-topic.md"))

    def test_book_excludes_non_markdown_contributor_assets(self):
        import tomllib
        config = tomllib.loads((ROOT / "docs/book.toml").read_text())
        self.assertFalse(config["build"]["create-missing"])
        spec = importlib.util.spec_from_file_location("reader_book", ROOT / "scripts/build-reader-book.py")
        builder = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(builder)
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "book"
            for name in ("internal/report.html", "archive/retired.html", "plans/evidence.json", "aida/data.json", "advanced/admin-guide.html"):
                path = output / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("fixture")
            builder.prune(output)
            self.assertTrue((output / "advanced/admin-guide.html").exists())
            for directory in ("internal", "archive", "plans", "aida"):
                self.assertFalse((output / directory).exists())

    def test_book_excludes_contributor_chapter(self):
        import re
        for summary in [ROOT / "docs/SUMMARY.md", ROOT / "docs/cli/SUMMARY.md"]:
            for target in re.findall(r"\]\(([^)#]+)", summary.read_text()):
                path = (summary.parent / target).relative_to(ROOT).as_posix()
                self.assertIn(inventory.audience(path), {"adopter", "advanced", "generated"}, path)


if __name__ == "__main__":
    unittest.main()
