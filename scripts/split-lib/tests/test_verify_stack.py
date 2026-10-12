# trace:TASK-1722 | ai:antigravity
# trace:TASK-1722 | ai:claude
"""Fixture tests for verify_stack.py: each protocol check failing on purpose."""
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

CHECKER = os.path.join(os.path.dirname(os.path.abspath(__file__)), os.pardir, "verify_stack.py")
SRC = "aida-cli-lib/src"
MANIFEST = "docs/plans/2026-10-04-story-1488-incremental-manifest.toml"
GOOD_PROOF = "V3 warnings 7/7, V4 tests 12/12, V7 {sizes}"


def git(repo, *args):
    res = subprocess.run(["git", "-C", repo, *args], capture_output=True, text=True)
    if res.returncode != 0:
        raise RuntimeError(f"git {args}: {res.stderr}")
    return res.stdout


class TestVerifyStack(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        git(self.tmp, "init", "-q", "-b", "main")
        for k, v in (("user.email", "t@t"), ("user.name", "t"), ("commit.gpgsign", "false"),
                     ("tag.gpgsign", "false"), ("core.hooksPath", "/dev/null")):
            git(self.tmp, "config", k, v)
        self.write(f"{SRC}/lib.rs", "// lib\nfn root() {}\ninclude!(\"lib_part1.rs\");\n")
        self.write(f"{SRC}/lib_part1.rs", "fn a() {}\nfn b() {}\nfn c() {}\n")
        self.write(MANIFEST, "# manifest\n")
        self.commit("init")
        self.base = self.head()

    # --- fixture helpers -------------------------------------------------
    def path(self, rel):
        return os.path.join(self.tmp, rel)

    def write(self, rel, text):
        os.makedirs(os.path.dirname(self.path(rel)), exist_ok=True)
        with open(self.path(rel), "w") as f:
            f.write(text)

    def read(self, rel):
        with open(self.path(rel)) as f:
            return f.read()

    def commit(self, msg):
        git(self.tmp, "add", "-A")
        git(self.tmp, "commit", "-q", "-m", msg)

    def head(self):
        return git(self.tmp, "rev-parse", "HEAD").strip()

    def tag(self, name, msg=None):
        git(self.tmp, "tag", *(["-a", name, "-m", msg] if msg else [name]))

    def move(self, mod, fn, extra="", wiring_twice=False, subject=None, shim="", vis=""):
        """Pure move of `<vis>fn <fn>() {}` from lib_part1.rs into <mod>.rs."""
        line = f"{vis}fn {fn}() {{}}\n"
        self.write(f"{SRC}/lib_part1.rs", self.read(f"{SRC}/lib_part1.rs").replace(line, ""))
        self.write(f"{SRC}/{mod}.rs", f"//! {mod}\n// trace:STORY-1488 | ai:claude\n\nuse crate::*;\n\n{line}{extra}")
        wiring = f"mod {mod};\nuse {mod}::*;\n" + (f"mod {mod};\n" if wiring_twice else "") + shim
        self.write(f"{SRC}/lib.rs", wiring + self.read(f"{SRC}/lib.rs"))
        self.write(MANIFEST, self.read(MANIFEST) + f"[{mod}]\n")
        self.commit(subject or f"refactor(cli-lib): extract {mod}.rs")

    def retarget(self, subject="retarget: inventory", edit_lib=False):
        if edit_lib:
            self.write(f"{SRC}/lib_part1.rs", self.read(f"{SRC}/lib_part1.rs") + "fn evil() {}\n")
        else:
            self.write("scripts/config-trust.toml", f"# {self.head()}\n")
        self.commit(subject)

    def proof(self, *mods):
        sizes = ", ".join(f"{m}.rs={self.read(f'{SRC}/{m}.rs').count(chr(10))}" for m in mods)
        return GOOD_PROOF.format(sizes=sizes)

    def run_checker(self, *extra):
        return subprocess.run([sys.executable, CHECKER, self.base, self.head(), *extra],
                              cwd=self.tmp, capture_output=True, text=True)

    def assertFails(self, needle, *extra):
        res = self.run_checker(*extra)
        self.assertNotEqual(res.returncode, 0, res.stdout)
        self.assertIn(needle, res.stdout + res.stderr)
        return res

    # --- the passing stack ------------------------------------------------
    def test_valid_stack_passes_with_real_tally(self):
        self.move("m1", "a")
        self.retarget()
        self.tag("slice-01-r1", self.proof("m1"))
        self.move("m2", "b")
        self.tag("slice-02-r1", self.proof("m2"))
        res = self.run_checker()
        self.assertEqual(res.returncode, 0, res.stdout)
        self.assertIn("3 commit(s): 2 MOVE, 1 RETARGET", res.stdout)
        self.assertIn("9 build      SKIP (no --build)", res.stdout)
        self.assertIn("RESULT: PASS", res.stdout)

    def test_summary_counts_real_commits(self):
        # Advisor finding 4: a 1-commit range must not print a canned 9/9.
        self.move("m1", "a")
        self.tag("slice-01-r1", self.proof("m1"))
        res = self.run_checker()
        self.assertEqual(res.returncode, 0, res.stdout)
        self.assertIn("1 commit(s): 1 MOVE", res.stdout)
        self.assertNotIn("9/9", res.stdout)

    # --- check 1-3 ----------------------------------------------------------
    def test_move_edits_one_line(self):
        self.move("m1", "a", extra="fn new() {}\n")
        self.tag("slice-01-r1", self.proof("m1"))
        self.assertFails("FAIL 3: added/removed lines do not match a pure move")

    def test_spec_id_in_subject(self):
        self.move("m1", "a", subject="refactor(cli-lib): extract m1.rs (TASK-123)")
        self.tag("slice-01-r1", self.proof("m1"))
        self.assertFails("FAIL 2: spec ID in subject")

    def test_spec_id_full_hook_set(self):
        # Advisor finding 7: SPIKE/ADR/DOC/... are spec IDs too.
        self.move("m1", "a")
        self.retarget(subject="retarget: per ADR-72 and SPIKE-9")
        self.tag("slice-01-r1", self.proof("m1"))
        self.assertFails("FAIL 2: spec ID in subject")

    def test_module_wired_twice(self):
        self.move("m1", "a", wiring_twice=True)
        self.tag("slice-01-r1", self.proof("m1"))
        self.assertFails("FAIL 3: module m1 not wired exactly once in lib.rs")

    def visible_base(self):
        """Rebase the fixture on `pub fn a`, `pub(crate) fn b`, private `fn c`."""
        self.write(f"{SRC}/lib_part1.rs", "pub fn a() {}\npub(crate) fn b() {}\nfn c() {}\n")
        self.commit("visibility")
        self.base = self.head()

    def test_pub_use_shim_for_a_moved_pub_item_passes(self):
        # Amendment 1n: a moved bare-`pub` item keeps its crate-root path.
        self.visible_base()
        self.move("m1", "a", vis="pub ", shim="pub use m1::{a};\n")
        self.tag("slice-01-r1", self.proof("m1"))
        res = self.run_checker()
        self.assertEqual(res.returncode, 0, res.stdout)

    def test_pub_use_shim_for_a_moved_pub_crate_item_fails(self):
        # The shim would widen a pub(crate) item to the public API.
        self.visible_base()
        self.move("m1", "b", vis="pub(crate) ", shim="pub use m1::b;\n")
        self.tag("slice-01-r1", self.proof("m1"))
        self.assertFails("FAIL 3: non-wiring line added to aida-cli-lib/src/lib.rs: pub use m1::b;")

    def test_pub_use_shim_naming_an_unmoved_item_fails(self):
        self.visible_base()
        self.move("m1", "a", vis="pub ", shim="pub use m1::{a, c};\n")
        self.tag("slice-01-r1", self.proof("m1"))
        self.assertFails("FAIL 3: non-wiring line added to aida-cli-lib/src/lib.rs: pub use m1::{a, c};")

    def test_pub_use_shim_for_another_module_fails(self):
        self.visible_base()
        self.move("m1", "a", vis="pub ", shim="pub use m2::a;\n")
        self.tag("slice-01-r1", self.proof("m1"))
        self.assertFails("FAIL 3: non-wiring line added to aida-cli-lib/src/lib.rs: pub use m2::a;")

    def test_wiring_outside_its_file_is_residue(self):
        # Advisor finding 6: `use crate::*;` belongs in the new module only.
        self.move("m1", "a")
        git(self.tmp, "reset", "-q", "--soft", "HEAD~1")
        self.write(f"{SRC}/lib_part1.rs", "use crate::*;\n" + self.read(f"{SRC}/lib_part1.rs"))
        self.commit("refactor(cli-lib): extract m1.rs")
        self.tag("slice-01-r1", self.proof("m1"))
        self.assertFails("FAIL 3: added/removed lines do not match a pure move")

    def test_manifest_elsewhere_is_outside_move_set(self):
        # Advisor finding 8: only the incremental manifest path is excused.
        self.move("m1", "a")
        git(self.tmp, "reset", "-q", "--soft", "HEAD~1")
        self.write("docs/plans/other-manifest.toml", "fn evil() {}\n")
        self.commit("refactor(cli-lib): extract m1.rs")
        self.tag("slice-01-r1", self.proof("m1"))
        self.assertFails("FAIL 3: file outside move set: docs/plans/other-manifest.toml")

    # --- check 2/3: the final slice is detected from the diff -------------
    def test_final_slice_subject_bypass_fails(self):
        # Advisor finding 2: a subject that merely *says* final is not exempt.
        self.write(f"{SRC}/lib.rs", self.read(f"{SRC}/lib.rs") + "fn a() { evil(); }\n")
        self.commit("chore: delete lib_part leftovers (TASK-9)")
        res = self.assertFails("FAIL 1: subject is not a MOVE")
        self.assertIn("FAIL 2: spec ID in subject", res.stdout)

    def test_final_slice_with_code_edit_fails_purity(self):
        self.move("m1", "a")
        self.move("m2", "b")
        self.write(f"{SRC}/lib.rs", self.read(f"{SRC}/lib.rs").replace('include!("lib_part1.rs");\n', "")
                   + "fn a() { evil(); }\n")
        self.write(f"{SRC}/m3.rs", "//! m3\nuse crate::*;\nfn c() {}\n")
        self.write(f"{SRC}/lib.rs", "mod m3;\nuse m3::*;\n" + self.read(f"{SRC}/lib.rs"))
        os.remove(self.path(f"{SRC}/lib_part1.rs"))
        self.commit("refactor(cli-lib): extract m3.rs (TASK-9)")
        self.tag("slice-01-r1", self.proof("m1", "m2", "m3"))
        self.assertFails("FAIL 3: added/removed lines do not match a pure move")

    def test_final_slice_pure_passes_with_spec_id(self):
        self.move("m1", "a")
        self.move("m2", "b")
        self.write(f"{SRC}/lib.rs", self.read(f"{SRC}/lib.rs").replace('include!("lib_part1.rs");\n', ""))
        self.write(f"{SRC}/m3.rs", "//! m3\nuse crate::*;\nfn c() {}\n")
        self.write(f"{SRC}/lib.rs", "mod m3;\nuse m3::*;\n" + self.read(f"{SRC}/lib.rs"))
        os.remove(self.path(f"{SRC}/lib_part1.rs"))
        self.commit("refactor(cli-lib): extract m3.rs (TASK-9)")
        self.tag("slice-01-r1", self.proof("m1", "m2", "m3"))
        res = self.run_checker()
        self.assertEqual(res.returncode, 0, res.stdout)
        self.assertIn("1 FINAL", res.stdout)

    # --- round 2: blocks land intact; lib*.rs only loses lines ---------------
    HEADER = "//! {m}\n// trace:STORY-1488 | ai:claude\n\nuse crate::*;\n\n"

    def two_parts(self):
        self.write(f"{SRC}/lib.rs", self.read(f"{SRC}/lib.rs") + 'include!("lib_part2.rs");\n')
        self.write(f"{SRC}/lib_part1.rs", "fn a() {\n    one();\n    two();\n}\nfn keep1() {}\n")
        self.write(f"{SRC}/lib_part2.rs", "fn z() {\n    three();\n}\nfn keep2() {}\n")
        self.commit("init parts")
        self.base = self.head()

    def wire(self, mod, body):
        self.write(f"{SRC}/{mod}.rs", self.HEADER.format(m=mod) + body)
        self.write(f"{SRC}/lib.rs", f"mod {mod};\nuse {mod}::*;\n" + self.read(f"{SRC}/lib.rs"))
        self.write(MANIFEST, self.read(MANIFEST) + f"[{mod}]\n")

    def test_fixture_a_reorder_inside_a_block_fails(self):
        # Same multiset of lines, but the body of `a` is reordered: a Counter
        # match cannot see it, the intact-block check must.
        self.two_parts()
        self.write(f"{SRC}/lib_part1.rs", "fn keep1() {}\n")
        self.wire("m1", "fn a() {\n    two();\n    one();\n}\n")
        self.commit("refactor(cli-lib): extract m1.rs")
        self.tag("slice-01-r1", self.proof("m1"))
        self.assertFails("does not land intact and in order")

    def test_fixture_b_relocation_between_parts_and_empty_extraction_fails(self):
        # Code shuffled from lib_part1 into lib_part2 while the "extracted"
        # module gets nothing but its header.
        self.two_parts()
        self.write(f"{SRC}/lib_part1.rs", "fn keep1() {}\n")
        self.write(f"{SRC}/lib_part2.rs", self.read(f"{SRC}/lib_part2.rs")
                   + "fn a() {\n    one();\n    two();\n}\n")
        self.wire("m1", "")
        self.commit("refactor(cli-lib): extract m1.rs")
        self.tag("slice-01-r1", self.proof("m1"))
        res = self.assertFails("FAIL 3: non-wiring line added to aida-cli-lib/src/lib_part2.rs")
        self.assertIn("FAIL 3: m1.rs receives no moved lines (empty extraction)", res.stdout)

    def test_moved_code_carrying_header_shaped_lines_passes(self):
        # A moved block may contain its own `// trace:` comment; it must still
        # match contiguously against the new module's body.
        self.two_parts()
        self.write(f"{SRC}/lib_part1.rs", "// trace:TASK-9 | ai:x\nfn a() {\n    one();\n}\nfn keep1() {}\n")
        self.commit("init traced")
        self.base = self.head()
        self.write(f"{SRC}/lib_part1.rs", "fn keep1() {}\n")
        self.wire("m1", "// trace:TASK-9 | ai:x\nfn a() {\n    one();\n}\n")
        self.commit("refactor(cli-lib): extract m1.rs")
        self.tag("slice-01-r1", self.proof("m1"))
        res = self.run_checker()
        self.assertEqual(res.returncode, 0, res.stdout)

    def test_two_parts_in_opposite_block_order_passes(self):
        # Positive control: blocks from two lib_part files may land in either
        # order relative to each other, as long as each stays intact.
        self.two_parts()
        self.write(f"{SRC}/lib_part1.rs", "fn keep1() {}\n")
        self.write(f"{SRC}/lib_part2.rs", "fn keep2() {}\n")
        self.wire("m1", "fn z() {\n    three();\n}\nfn a() {\n    one();\n    two();\n}\n")
        self.commit("refactor(cli-lib): extract m1.rs")
        self.tag("slice-01-r1", self.proof("m1"))
        res = self.run_checker()
        self.assertEqual(res.returncode, 0, res.stdout)

    def test_final_block_splits_at_a_module_seam(self):
        self.two_parts()
        self.write(f"{SRC}/lib.rs", self.read(f"{SRC}/lib.rs")
                   .replace('include!("lib_part1.rs");\n', "").replace('include!("lib_part2.rs");\n', ""))
        os.remove(self.path(f"{SRC}/lib_part1.rs"))
        os.remove(self.path(f"{SRC}/lib_part2.rs"))
        self.wire("m1", "fn a() {\n    one();\n    two();\n}\nfn z() {\n    three();\n}\n")
        self.wire("m2", "fn keep1() {}\nfn keep2() {}\n")
        self.commit("refactor(cli-lib): extract m2.rs (TASK-9)")
        self.tag("slice-01-r1", self.proof("m1", "m2"))
        res = self.run_checker()
        self.assertEqual(res.returncode, 0, res.stdout)
        self.assertIn("1 FINAL", res.stdout)

    def scan_guard(self, extra_include):
        self.write(f"{SRC}/lib_part1.rs", self.read(f"{SRC}/lib_part1.rs") + (
            "fn guard() {\n    let src = format!(\n        \"{}\\n{}\",\n"
            "        include_str!(\"lib.rs\"),\n        include_str!(\"lib_part1.rs\")\n    );\n}\n"))
        self.commit("init guard")
        self.base = self.head()
        self.move("m1", "a")
        self.write(f"{SRC}/lib_part1.rs", self.read(f"{SRC}/lib_part1.rs").replace(
            '        \"{}\\n{}\",\n', '        \"{}\\n{}\\n{}\",\n').replace(
            '        include_str!("lib_part1.rs")\n',
            f'        include_str!("lib_part1.rs"),\n        include_str!("{extra_include}")\n'))
        self.commit("retarget: guard scans include the moved module")
        self.tag("slice-01-r1", self.proof("m1"))

    def test_retarget_widening_a_scan_corpus_is_a_note(self):
        self.scan_guard("m1.rs")
        res = self.run_checker()
        self.assertEqual(res.returncode, 0, res.stdout)
        self.assertIn("NOTE 4: 3 scan-corpus line(s) widened", res.stdout)

    def test_retarget_scan_corpus_of_an_unmoved_file_fails(self):
        self.scan_guard("secrets.rs")
        self.assertFails("FAIL 4: retarget commit changes protected code")

    # --- extend: append-mode MOVEs -------------------------------------------
    # trace:TASK-1538 | ai:claude
    def extend(self, mod, fn, prefix_edit=False, lib_wiring="", chunk_extra="", shim="", vis=""):
        """Append `<vis>fn <fn>() {}` from lib_part1.rs to the existing <mod>.rs."""
        line = f"{vis}fn {fn}() {{}}\n"
        self.write(f"{SRC}/lib_part1.rs", self.read(f"{SRC}/lib_part1.rs").replace(line, ""))
        body = self.read(f"{SRC}/{mod}.rs") if os.path.exists(self.path(f"{SRC}/{mod}.rs")) else ""
        if prefix_edit:
            body = body.replace("fn a() {}", "fn a() { evil(); }")
        self.write(f"{SRC}/{mod}.rs", body + "\n" + chunk_extra + line)
        if lib_wiring or shim:
            self.write(f"{SRC}/lib.rs", lib_wiring + shim + self.read(f"{SRC}/lib.rs"))
        self.write(MANIFEST, self.read(MANIFEST) + f"[{mod}_more]\n")
        self.commit(f"refactor(cli-lib): extend {mod}.rs")

    def moved_base(self):
        """Rebase the fixture so m1.rs (holding `fn a`) already exists."""
        self.move("m1", "a")
        self.base = self.head()

    def test_extend_passes(self):
        self.moved_base()
        self.extend("m1", "b")
        self.retarget()
        self.tag("slice-02-r1", self.proof("m1"))
        res = self.run_checker()
        self.assertEqual(res.returncode, 0, res.stdout)
        self.assertIn("2 commit(s): 1 MOVE, 1 RETARGET", res.stdout)

    def test_extend_with_pub_use_shim_passes(self):
        self.write(f"{SRC}/lib_part1.rs", "fn a() {}\npub fn b() {}\nfn c() {}\n")
        self.commit("visibility")
        self.moved_base()
        self.extend("m1", "b", shim="pub use m1::b;\n", vis="pub ")
        self.tag("slice-02-r1", self.proof("m1"))
        res = self.run_checker()
        self.assertEqual(res.returncode, 0, res.stdout)

    def test_extend_editing_existing_lines_fails(self):
        self.moved_base()
        self.extend("m1", "b", prefix_edit=True)
        self.tag("slice-02-r1", self.proof("m1"))
        self.assertFails("FAIL 3: extend edits already-moved text")

    def test_extend_of_missing_module_fails(self):
        self.extend("m1", "b")
        self.tag("slice-01-r1", self.proof("m1"))
        self.assertFails("FAIL 3: extend must modify an existing")

    def test_extend_adding_mod_line_fails(self):
        self.moved_base()
        self.extend("m1", "b", lib_wiring="mod m1;\n")
        self.tag("slice-02-r1", self.proof("m1"))
        self.assertFails("FAIL 3: extend re-wires m1 in lib.rs (mod x1, use x0)")

    def test_extend_adding_header_is_residue(self):
        self.moved_base()
        self.extend("m1", "b", chunk_extra="use crate::*;\n")
        self.tag("slice-02-r1", self.proof("m1"))
        self.assertFails("FAIL 3: added/removed lines do not match a pure move")

    def test_extend_appending_mod_declaration_fails(self):
        self.moved_base()
        self.extend("m1", "b", chunk_extra="mod sneaky;\n")
        self.tag("slice-02-r1", self.proof("m1"))
        self.assertFails("FAIL 3: extend appends a submodule declaration: mod sneaky;")

    def test_extend_appending_path_attribute_fails(self):
        self.moved_base()
        self.extend("m1", "b", chunk_extra="#[path = \"x.rs\"]\n")
        self.tag("slice-02-r1", self.proof("m1"))
        self.assertFails("FAIL 3: extend appends a submodule declaration: #[path")

    def test_extend_carrying_its_test_module_passes(self):
        decl = "fn b() {}\n#[cfg(test)]\n#[path = \"tests/b_tests.rs\"]\nmod b_tests;\n"
        self.write(f"{SRC}/lib_part1.rs", self.read(f"{SRC}/lib_part1.rs").replace("fn b() {}\n", decl))
        self.write(f"{SRC}/tests/b_tests.rs", "// t\n")
        self.commit("test module base")
        self.moved_base()
        self.write(f"{SRC}/lib_part1.rs", self.read(f"{SRC}/lib_part1.rs").replace(decl, ""))
        self.write(f"{SRC}/m1.rs", self.read(f"{SRC}/m1.rs") + "\n" + decl)
        self.commit("refactor(cli-lib): extend m1.rs")
        self.tag("slice-02-r1", self.proof("m1"))
        res = self.run_checker()
        self.assertEqual(res.returncode, 0, res.stdout)

    def test_extend_v7_records_post_append_size(self):
        self.moved_base()
        before = self.proof("m1")
        self.extend("m1", "b")
        self.tag("slice-02-r1", before)
        self.assertFails("FAIL 6: tag slice-02-r1 says m1.rs=")

    # --- check 4/5 -----------------------------------------------------------
    def test_retarget_edits_lib_part(self):
        self.move("m1", "a")
        self.retarget(edit_lib=True)
        self.tag("slice-01-r1", self.proof("m1"))
        self.assertFails("FAIL 4: retarget commit changes protected code")

    def test_retarget_not_after_move(self):
        # Advisor finding 5.
        self.move("m1", "a")
        self.retarget()
        self.retarget(subject="retarget: second one")
        self.tag("slice-01-r1", self.proof("m1"))
        self.assertFails("FAIL 5: retarget: commit does not directly follow a MOVE")

    def test_tooling_anywhere_is_fine(self):
        self.retarget(subject="tooling: helper")
        self.move("m1", "a")
        self.tag("slice-01-r1", self.proof("m1"))
        res = self.run_checker()
        self.assertEqual(res.returncode, 0, res.stdout)

    # --- check 6: tags -------------------------------------------------------
    def test_lightweight_tag(self):
        self.move("m1", "a")
        self.tag("slice-01-r1")
        self.assertFails("FAIL 6: tag slice-01-r1 is not annotated")

    def test_tag_missing_v4(self):
        self.move("m1", "a")
        self.tag("slice-01-r1", "V3 warnings 7/7, V7 m1.rs=6")
        self.assertFails("FAIL 6: tag slice-01-r1 has no numeric V4 record")

    def test_placeholder_tag_fails(self):
        # Advisor finding 1: the round-3 placeholder proof.
        self.move("m1", "a")
        self.tag("slice-01-r1", "V3 warnings TODO, V4 tests TBD, V7 size ?")
        res = self.assertFails("FAIL 6: tag slice-01-r1 has no numeric V3 record")
        self.assertIn("has no numeric V4 record", res.stdout)
        self.assertIn("has no V7 record `m1.rs=<lines>`", res.stdout)

    def test_letter_placeholders_fail(self):
        self.move("m1", "a")
        self.tag("slice-01-r1", "V3 warnings A/A, V4 tests N/N green, V7 sizes. m1.rs=6")
        self.assertFails("has no numeric V3 record")

    def test_v7_size_mismatch_fails(self):
        self.move("m1", "a")
        self.tag("slice-01-r1", GOOD_PROOF.format(sizes="m1.rs=999"))
        self.assertFails("FAIL 6: tag slice-01-r1 says m1.rs=999, actual 6")

    def test_v7_missing_module_fails(self):
        self.move("m1", "a")
        self.move("m2", "b")
        self.tag("slice-01-r1", self.proof("m2"))
        self.assertFails("FAIL 6: tag slice-01-r1 has no V7 record `m1.rs=<lines>`")

    def test_untagged_move_fails(self):
        self.move("m1", "a")
        self.assertFails("is not covered by a slice-NN-rR tag")

    # --- check 7/8/9 -----------------------------------------------------------
    def test_size_cap(self):
        self.move("m1", "a", extra="\n" * 6001)
        self.tag("slice-01-r1", self.proof("m1"))
        self.assertFails("FAIL 7: m1.rs is")

    def test_merge_commit(self):
        self.move("m1", "a")
        git(self.tmp, "checkout", "-q", "-b", "other", self.base)
        self.retarget(subject="tooling: side")
        git(self.tmp, "checkout", "-q", "main")
        git(self.tmp, "merge", "-q", "other", "--no-ff", "-m", "merge")
        self.assertFails("FAIL 8: range contains merge commits")

    def test_unknown_flag_exits_nonzero(self):
        # Advisor finding 3: never accept a flag that does nothing.
        self.move("m1", "a")
        self.tag("slice-01-r1", self.proof("m1"))
        res = self.run_checker("--bulid")
        self.assertEqual(res.returncode, 2, res.stdout)
        self.assertIn("unknown option(s): --bulid", res.stderr)


if __name__ == "__main__":
    unittest.main()
