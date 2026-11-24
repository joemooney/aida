"""Pin the advisor's keep-open comment in the repository and scaffold."""

from pathlib import Path
import unittest


class CloseoutAuditWordingTest(unittest.TestCase):
    # trace:BUG-2731 | ai:codex
    def test_keep_open_verdict(self):
        root = Path(__file__).resolve().parents[1]
        guide = Path(".aida/discipline/advisor-role.md")
        for base in (root, root / "aida-core/templates"):
            with self.subTest(base=base):
                self.assertIn(
                    "> AUDIT VERDICT (not a lane instruction): keep open; implementation still wanted",
                    (base / guide).read_text().splitlines(),
                )
