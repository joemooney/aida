"""Check the trust inventory for project-lowerable duration caps."""
# trace:TASK-3126 | ai:codex

from pathlib import Path
import tomllib
import unittest


class DurationCapInventoryTests(unittest.TestCase):
    def test_project_duration_caps_are_worktree_policy(self):
        inventory = Path(__file__).resolve().parents[1] / "scripts/config-trust.toml"
        with inventory.open("rb") as source:
            worktree = tomllib.load(source)["keys"]["worktree"]

        for section, key in (
            ("mandate", "max_ttl_hours"),
            ("seat", "inline_grant_max_hours"),
        ):
            with self.subTest(section=section, key=key):
                self.assertEqual(worktree[section][key], "policy")


if __name__ == "__main__":
    unittest.main()
