#!/usr/bin/env python3
"""Regression tests for the trace/doc-header stranding gate (BUG-1550).

trace:BUG-1550 | ai:claude
"""

import importlib.util
import pathlib
import unittest

SCRIPT = pathlib.Path(__file__).parents[1] / "scripts/check-trace-headers.py"
SPEC = importlib.util.spec_from_file_location("check_trace_headers", SCRIPT)
module = importlib.util.module_from_spec(SPEC)
assert SPEC.loader
SPEC.loader.exec_module(module)


class StrandingTest(unittest.TestCase):
    def test_shape_A_stranded_trace_run_is_flagged_red(self):
        """Construct the tracker_cmd.rs shape with different symbols in a
        different (synthetic) file: a bare trace run for one item, then a
        doc + trace run for a DIFFERENT, unrelated item, both sitting
        directly above the second item's declaration. RED: must fire.
        """
        source = (
            "// trace:ARCH-widget-integration | ai:claude\n"
            "/// Handle widget integration commands.\n"
            "// trace:ARCH-gadget-integration | ai:claude\n"
            "pub(crate) fn handle_gadget_command(cmd: &GadgetCommand) -> Result<()> {\n"
            "    Ok(())\n"
            "}\n"
        )
        violations = module.find_strandings(source)
        self.assertEqual(len(violations), 1, violations)
        _, item_line, kind, name = violations[0]
        self.assertEqual((kind, name), ("fn", "handle_gadget_command"))
        self.assertEqual(item_line, 4)

    def test_shape_A_fixed_is_green(self):
        """Same fixture, with the stranded header moved back to its own item
        (the fix a human or agent would actually make). GREEN: silent."""
        source = (
            "/// Handle gadget integration commands.\n"
            "// trace:ARCH-gadget-integration | ai:claude\n"
            "pub(crate) fn handle_gadget_command(cmd: &GadgetCommand) -> Result<()> {\n"
            "    Ok(())\n"
            "}\n"
            "\n"
            "/// Handle widget integration commands.\n"
            "// trace:ARCH-widget-integration | ai:claude\n"
            "pub(crate) fn handle_widget_command(cmd: &WidgetCommand) -> Result<()> {\n"
            "    Ok(())\n"
            "}\n"
        )
        self.assertEqual(module.find_strandings(source), [])

    def test_repeated_spec_id_across_runs_is_not_a_stranding(self):
        """A trace run re-affirmed by a later revision of the SAME spec
        (e.g. ai:codex tags it, ai:claude later extends the doc and re-tags
        the same spec) is a legitimate authorship-revision pattern, not a
        splice -- the acceptance-criterion-5 distinction."""
        source = (
            "// trace:STORY-900 | ai:codex\n"
            "/// Compute the rolling average latency for the window.\n"
            "// trace:STORY-900 | ai:claude\n"
            "pub(crate) fn rolling_average_latency(samples: &[f64]) -> f64 {\n"
            "    0.0\n"
            "}\n"
        )
        self.assertEqual(module.find_strandings(source), [])

    def test_two_different_specs_each_with_their_own_doc_is_not_a_stranding(self):
        """The pr_ship.rs / human_cmd.rs shape: a single item legitimately
        re-documented and re-traced across two DIFFERENT specs over time,
        where each trace run has its own doc segment. Must NOT fire --
        this is the false-positive class the run-based (not line-based)
        signal exists to exclude."""
        source = (
            "/// Classify one invocation into the registration state.\n"
            "// trace:BUG-100 | ai:codex\n"
            "/// STORY-200: extended to cover the forge-neutral case too.\n"
            "// trace:STORY-200 | ai:claude\n"
            "pub(crate) fn classify_registration(raw: &str) -> bool {\n"
            "    true\n"
            "}\n"
        )
        self.assertEqual(module.find_strandings(source), [])

    def test_stacked_trace_tags_with_no_doc_between_them_is_not_a_stranding(self):
        """Multiple `// trace:` lines stacked back-to-back with nothing
        between them (co-authorship / a multi-spec fix tagged at once) is a
        normal, measured-common idiom (210 instances exist in this tree) --
        it must not be mistaken for two spliced headers."""
        source = (
            "/// Resolve the effective role for this session.\n"
            "// trace:TASK-1 | ai:claude\n"
            "// trace:TASK-2 | ai:claude\n"
            "pub(crate) fn resolve_effective_role(raw: &str) -> String {\n"
            "    raw.to_string()\n"
            "}\n"
        )
        self.assertEqual(module.find_strandings(source), [])

    def test_trace_comment_attached_to_a_block_not_an_item_is_not_flagged(self):
        """Acceptance criterion 5: a trace comment legitimately attached to
        a block/statement inside a function body (not an item declaration)
        must not be flagged -- the parser only recognises headers that
        directly precede an fn/struct/enum/trait/impl/const/static/type/mod
        declaration."""
        source = (
            "pub(crate) fn handle_things(x: i32) -> i32 {\n"
            "    // trace:TASK-5 | ai:claude\n"
            "    /// this is not a doc comment on an item, just commentary\n"
            "    // trace:TASK-6 | ai:claude\n"
            "    if x > 0 {\n"
            "        x\n"
            "    } else {\n"
            "        -x\n"
            "    }\n"
            "}\n"
        )
        self.assertEqual(module.find_strandings(source), [])

    def test_module_level_doc_comment_is_not_flagged(self):
        """A module-level `//!` doc comment documents the ENCLOSING module,
        not the next item -- it must never be treated as that item's
        header."""
        source = (
            "//! trace:EPIC-1\n"
            "//! Module-level documentation for this file.\n"
            "\n"
            "/// Do the thing.\n"
            "// trace:TASK-10 | ai:claude\n"
            "pub(crate) fn do_thing() {}\n"
        )
        self.assertEqual(module.find_strandings(source), [])

    def test_self_test_the_checkers_own_source_is_clean(self):
        """BUG-1550 acceptance criterion 3: run the check over its own
        source file as part of the test suite -- the instrument must be
        able to see itself, not just its fixtures. check-trace-headers.py
        is Python (no Rust `///`/`// trace:` syntax), so this also
        exercises the parser's behaviour on a file with none of the
        patterns it looks for: it must return cleanly, not raise."""
        text = SCRIPT.read_text(encoding="utf-8")
        self.assertEqual(module.find_strandings(text), [])

    def test_tree_wide_scan_is_clean_after_the_bug_1550_fixes(self):
        """Positive control: the real tree, after tracker_cmd.rs and mcp.rs
        were fixed, must be silent (allowlist entries excluded)."""
        findings = module.scan_tree(module.ROOT)
        self.assertEqual(findings, [], findings)


if __name__ == "__main__":
    unittest.main()
