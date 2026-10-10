# trace:TASK-1722 | ai:antigravity
import unittest
import tempfile
import subprocess
import os

def git(repo, *args, check=True):
    res = subprocess.run(["git", "-C", repo, *args], capture_output=True, text=True)
    if check and res.returncode != 0:
        raise Exception(f"git error: {res.stderr}")
    return res.stdout

class TestVerifyStack(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        git(self.tmp, "init", "-b", "main")
        git(self.tmp, "config", "user.email", "test@test.com")
        git(self.tmp, "config", "user.name", "test")
        
        os.makedirs(os.path.join(self.tmp, "aida-cli-lib/src"))
        with open(os.path.join(self.tmp, "aida-cli-lib/src/lib.rs"), "w") as f:
            f.write("// lib\nlet x = 1;\n")
        with open(os.path.join(self.tmp, "aida-cli-lib/src/lib_part1.rs"), "w") as f:
            f.write("let y = 2;\n")
        git(self.tmp, "add", ".")
        git(self.tmp, "commit", "-m", "init")
        self.base = git(self.tmp, "rev-parse", "HEAD").strip()
        
    def _run_checker(self, head):
        checker = os.path.abspath("scripts/split-lib/verify_stack.py")
        res = subprocess.run([checker, self.base, head], cwd=self.tmp, capture_output=True, text=True)
        return res

    def _add_move(self, mod, add_lines="", extra_wiring=False):
        lib = []
        with open(os.path.join(self.tmp, "aida-cli-lib/src/lib.rs")) as f:
            lib = f.readlines()
            
        with open(os.path.join(self.tmp, f"aida-cli-lib/src/{mod}.rs"), "w") as f:
            f.write("//!\n// trace:TASK-1\nuse crate::*;\n" + add_lines)
            
        with open(os.path.join(self.tmp, "aida-cli-lib/src/lib.rs"), "w") as f:
            f.write(f"mod {mod};\nuse {mod}::*;\n")
            if extra_wiring:
                f.write(f"mod {mod};\n")
            f.writelines(lib)
            
        git(self.tmp, "add", ".")
        git(self.tmp, "commit", "-m", f"refactor(cli-lib): extract {mod}.rs")
        
    def _add_retarget(self, tag_name=None, tag_msg=None, edit_lib=False):
        if edit_lib:
            with open(os.path.join(self.tmp, "aida-cli-lib/src/lib_part1.rs"), "w") as f:
                f.write("let y = 3;\n")
        else:
            with open(os.path.join(self.tmp, "dummy.txt"), "w") as f:
                f.write("dummy\n")
        git(self.tmp, "add", ".")
        git(self.tmp, "commit", "-m", "retarget: update")
        
        if tag_name:
            if tag_msg:
                git(self.tmp, "tag", "-a", tag_name, "-m", tag_msg)
            else:
                git(self.tmp, "tag", tag_name)

    def test_valid(self):
        self._add_move("m1")
        self._add_retarget("slice-01-r1", "V3 warnings A/A, V4 tests N/N green, V7 sizes db=1")
        head = git(self.tmp, "rev-parse", "HEAD").strip()
        res = self._run_checker(head)
        self.assertEqual(res.returncode, 0, res.stdout)
        
    def test_move_edits_one_line(self):
        self._add_move("m1", add_lines="let new = 1;\n")
        head = git(self.tmp, "rev-parse", "HEAD").strip()
        res = self._run_checker(head)
        self.assertNotEqual(res.returncode, 0)
        self.assertIn("FAIL: Added/removed lines do not match pure move", res.stdout)
        
    def test_spec_id(self):
        self._add_move("m1")
        git(self.tmp, "commit", "--amend", "-m", "refactor(cli-lib): extract m1.rs (TASK-123)")
        head = git(self.tmp, "rev-parse", "HEAD").strip()
        res = self._run_checker(head)
        self.assertNotEqual(res.returncode, 0)
        self.assertIn("FAIL: Contains spec ID in subject", res.stdout)
        
    def test_retarget_edits_lib_part(self):
        self._add_move("m1")
        self._add_retarget(edit_lib=True)
        head = git(self.tmp, "rev-parse", "HEAD").strip()
        res = self._run_checker(head)
        self.assertNotEqual(res.returncode, 0)
        self.assertIn("FAIL: retarget/tooling commit edits protected code", res.stdout)
        
    def test_lightweight_tag(self):
        self._add_move("m1")
        self._add_retarget("slice-01-r1")
        head = git(self.tmp, "rev-parse", "HEAD").strip()
        res = self._run_checker(head)
        self.assertNotEqual(res.returncode, 0)
        self.assertIn("FAIL: Tag slice-01-r1 is not annotated", res.stdout)
        
    def test_tag_missing_v4(self):
        self._add_move("m1")
        self._add_retarget("slice-01-r1", "V3 warnings A/A, V7 sizes db=1")
        head = git(self.tmp, "rev-parse", "HEAD").strip()
        res = self._run_checker(head)
        self.assertNotEqual(res.returncode, 0)
        self.assertIn("FAIL: Tag slice-01-r1 missing V4 record", res.stdout)
        
    def test_merge_commit(self):
        self._add_move("m1")
        git(self.tmp, "checkout", "-b", "other", self.base)
        with open(self.tmp + "/dummy2.txt", "w") as f: f.write("dummy2\\n")
        git(self.tmp, "add", ".")
        git(self.tmp, "commit", "-m", "dummy")
        git(self.tmp, "checkout", "main")
        git(self.tmp, "merge", "other", "--no-ff", "-m", "merge")
        head = git(self.tmp, "rev-parse", "HEAD").strip()
        res = self._run_checker(head)
        self.assertNotEqual(res.returncode, 0)
        self.assertIn("FAIL: Range contains merge commits", res.stdout)
        
    def test_wired_twice(self):
        self._add_move("m1", extra_wiring=True)
        head = git(self.tmp, "rev-parse", "HEAD").strip()
        res = self._run_checker(head)
        self.assertNotEqual(res.returncode, 0)
        self.assertIn("FAIL: Module m1 not wired exactly once in lib.rs", res.stdout)

if __name__ == '__main__':
    unittest.main()
