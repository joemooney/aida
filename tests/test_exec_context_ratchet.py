"""Mutation fixtures for the staged execution-context ledger."""
# trace:STORY-2538 | ai:codex
import importlib.util
import json
import subprocess
import sys
import tempfile
from collections import Counter
from pathlib import Path
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("exec_ratchet", Path(__file__).resolve().parents[1] / "scripts/check-exec-context.py")
GUARD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GUARD)


class ExecContextRatchetTests(unittest.TestCase):
    def test_rust_site_golden_key(self):
        self.assertEqual(
            ["f:var:196145c86d8506ab9d01"],
            GUARD.rust_sites('fn f() { std::env::var("HOME"); }'),
            "Rust token fingerprint changed; review the encoding before updating the ledger",
        )

    def test_python_site_golden_keys(self):
        for expression, expected in [
            ('os.getenv("HOME")', "f:os.getenv:95e6a881ac9b4f838e78"),
            ('os.environ["HOME"]', "f:os.environ:41a731e6c09dd1a631a3"),
        ]:
            with self.subTest(expression=expression):
                source = f'import os\ndef f():\n return {expression}'
                self.assertEqual(
                    [expected], GUARD.python_sites(source),
                    "Python AST fingerprint changed; review the encoding before updating the ledger",
                )
                # Interpreter changes to ast.dump must never affect identity.
                with patch.object(GUARD.ast, "dump", side_effect=AssertionError("versioned AST display")):
                    self.assertEqual([expected], GUARD.python_sites(source))

    def test_python_encoding_ignores_absent_optional_fields(self):
        call = GUARD.ast.parse('os.getenv("HOME")', mode="eval").body
        expected = GUARD.python_expression(call)
        # Model the removal of Constant.kind and omission of empty Call
        # fields across Python versions, independently of this interpreter.
        literal = call.args[0]
        literal._fields = tuple(field for field in literal._fields if field != "kind")
        call._fields = tuple(field for field in call._fields if field != "keywords")
        self.assertEqual(expected, GUARD.python_expression(call))
        literal.value = "OTHER"
        self.assertNotEqual(expected, GUARD.python_expression(call))

    def test_qualified_imported_grouped_and_aliased_readers(self):
        for source in [
            'fn f() { std::env::var("HOME"); }',
            'fn f() { std::r#env::r#var("HOME"); }',
            'fn f() { ::std::env::var_os("HOME"); }',
            'fn f() { return ::std::env::var_os("HOME"); }',
            'use std::env::{self as ambient}; fn f() { ambient::vars_os(); }',
            'use std::env as ambient; fn f() { ambient::vars_os(); }',
            'use std::{env::{var as read, vars_os as all}}; fn f() { read("HOME"); all(); }',
            'use std as standard; use standard::env as ambient; fn f() { ambient::vars(); }',
            'use std::env::*; fn f() { var("HOME"); }',
            'fn f() { let read = std::env::var; helper(read); }',
            'pub use std::env::var as read;',
            'use std::env as std; fn f() { ::std::env::vars_os(); }',
        ]:
            with self.subTest(source=source):
                sites = GUARD.rust_sites(source)
                self.assertTrue(sites)
                if "fn f" in source:
                    self.assertTrue(any(not site.startswith("<import>") for site in sites), sites)

    def test_extern_crate_aliases_cannot_hide_new_sites(self):
        # trace:STORY-2538 | ai:codex
        empty = {"version": 1, "owner": "STORY-2538", "ceiling": 0, "sites": {}}
        for source, operation in [
            ('extern crate std as s; fn f() { s::env::var("X"); }', "var"),
            ('extern crate std as r#s; fn f() { ::s::env::vars_os(); }', "vars_os"),
            ('extern crate std as s; use s::env::var as read; fn f() { read("X"); }', "var"),
            ('extern crate std as s; fn f() { s::env::set_var("X", "Y"); }', "set_var"),
        ]:
            with self.subTest(source=source):
                sites = GUARD.rust_sites(source)
                self.assertTrue(any(site.startswith(f"f:{operation}:") for site in sites), sites)
                with self.assertRaisesRegex(ValueError, "new sites"):
                    GUARD.check(Counter(sites), empty)
        self.assertEqual([], GUARD.rust_sites(
            'extern crate other as s; fn f() { s::env::var("X"); }'
        ))

    def test_leading_absolute_paths_after_every_keyword(self):
        # Keep this independent of the scanner's keyword set so omitting a
        # keyword from the implementation is a failing mutant.
        keywords = """
            Self abstract as async auto await become box break const continue
            crate do dyn else enum extern false final fn for gen if impl in let
            loop macro macro_rules match mod move mut override priv pub raw ref return safe
            self static struct super trait true try type typeof union unsafe
            unsized use virtual where while yield
        """.split()
        for keyword in keywords:
            if keyword == "use":
                source = 'use ::std::env::var;'
            else:
                source = f'fn f() {{ {keyword} ::std::env::var("X") }}'
            with self.subTest(keyword=keyword):
                self.assertEqual(1, len(GUARD.rust_sites(source)))
        for expression in [
            'if ::std::env::var("X").is_ok() {}',
            'match ::std::env::var("X") { _ => () }',
            'while ::std::env::var("X").is_ok() {}',
            'let _ = &mut ::std::env::var("X");',
            'let _ = &raw const ::std::env::var("X");',
        ]:
            with self.subTest(expression=expression):
                self.assertEqual(1, len(GUARD.rust_sites('fn f() { ' + expression + ' }')))
        self.assertEqual([], GUARD.rust_sites('fn f() { unrelated::std::env::var("X"); }'))

    def test_function_scopes_end_and_restore_enclosing_scope(self):
        source = """
            fn outer() {
                if true { std::env::var("before"); }
                fn inner() { std::env::var("inner"); }
                std::env::var("after");
            }
            static S: Lazy<String> = Lazy::new(|| { std::env::var("static") });
            const C: fn() = || { std::env::var("const"); };
            mod nested {
                fn sibling() { std::env::var("sibling"); }
                static T: Lazy<String> = Lazy::new(|| { std::env::var("nested") });
            }
            trait Tr { fn declaration(); }
            extern "C" { fn declaration(); }
            fn last() { std::env::var("last"); }
        """
        self.assertEqual(
            ["outer", "inner", "outer", "<module>", "<module>", "sibling", "<module>", "last"],
            [site.split(":")[0] for site in GUARD.rust_sites(source)],
        )
        original = 'fn f() { std::env::var("X"); }'
        nested = 'fn f() { fn new_fn() {} std::env::var("X"); }'
        self.assertEqual(GUARD.rust_sites(original), GUARD.rust_sites(nested))
        module = 'static S: Lazy<String> = Lazy::new(|| std::env::var("X"));'
        self.assertEqual(GUARD.rust_sites(module), GUARD.rust_sites('fn f() {}' + module))

    def test_unrelated_self_named_import_does_not_expand(self):
        self.assertEqual([], GUARD.rust_sites('use anyhow::{anyhow, Result}; fn f() { anyhow!("x"); }'))

    def test_mutations_and_helper_bindings_are_sources(self):
        for source in [
            'use std::env::{set_var as write}; fn f() { write("K", "V"); }',
            'fn f() { std::env::remove_var("K"); }',
            'fn fixture() { helper(std::env::set_var); }',
            'use std::env; fn wrapper() { env::set_var("K", "V"); }',
        ]:
            self.assertTrue(GUARD.rust_sites(source), source)

    def test_comments_literals_nested_comments_and_lifetimes(self):
        source = '''
        /* outer /* inner */ std::env::var("K"); */
        // std::env::set_var("K", "V");
        fn fixture<'a>(x: &'a str) {
            let a = r##"std::env::vars_os()"##;
            let b = "std::env::var(\\\"K\\\")";
            let c = '"';
        }
        '''
        self.assertEqual([], GUARD.rust_sites(source))
        self.assertEqual(1, len(GUARD.rust_sites(source + 'fn f() { std::env::var("REAL"); }')))

    def test_exact_site_identity_ignores_comments_but_detects_replacement(self):
        original = GUARD.rust_sites('fn f() { std::env::var("A"); }')
        self.assertEqual(original, GUARD.rust_sites('// intro\n fn f() { /*comment*/ std::env::var ( "A" ); }'))
        self.assertNotEqual(original, GUARD.rust_sites('fn f() { std::env::var("B"); }'))
        self.assertNotEqual(original, GUARD.rust_sites('fn g() { std::env::var("A"); }'))

    def test_ledger_rejects_additions_duplicates_and_unpruned_retirements(self):
        ledger = {"version": 1, "owner": "STORY-2538", "ceiling": 1, "sites": {"a.rs:f:var:digest": 1}}
        GUARD.check(Counter({"a.rs:f:var:digest": 1}), ledger)
        for sites in [{"a.rs:f:var:digest": 2}, {"b.rs:f:var:changed": 1}, {}]:
            with self.assertRaises(ValueError):
                GUARD.check(Counter(sites), ledger)
        GUARD.check_growth({**ledger, "ceiling": 0, "sites": {}}, ledger)
        with self.assertRaises(ValueError):
            GUARD.check_growth({**ledger, "sites": {"b.rs:f:var:changed": 1}}, ledger)

    def test_growth_allows_only_identical_relocations_with_retired_capacity(self):
        def ledger(sites):
            return {"version": 1, "owner": "STORY-2538", "ceiling": sum(sites.values()), "sites": sites}

        old = ledger({"old.rs:f:var:digest": 2, "old.rs:g:set_var:write": 1})
        for sites in [
            {"moved.rs:f:var:digest": 2, "old.rs:g:set_var:write": 1},
            {"old.rs:renamed:var:digest": 2, "old.rs:g:set_var:write": 1},
            {"one.rs:f:var:digest": 1, "two.rs:f:var:digest": 1, "old.rs:g:set_var:write": 1},
            {"moved.rs:f:var:digest": 1},
            {},
        ]:
            with self.subTest(sites=sites):
                GUARD.check_growth(ledger(sites), old)
        for sites in [
            {"moved.rs:f:var:changed": 2},  # Expression replacement.
            {"moved.rs:f:var_os:digest": 2},  # Changed operation.
            {"moved.rs:f:var:digest": 3},  # Same total, wrong multiplicity.
            {"old.rs:f:var:digest": 2, "extra.rs:f:var:digest": 1},  # Retained sites have no spare capacity.
            {"moved.rs:f:var:digest": 2, "extra.rs:g:set_var:write": 2},  # Total growth.
        ]:
            with self.subTest(sites=sites), self.assertRaises(ValueError):
                GUARD.check_growth(ledger(sites), old)

    def test_scan_uses_tracked_member_and_test_sources_only(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)

            def write(name, content):
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(content.encode() if isinstance(content, str) else content)

            def git(*args):
                return subprocess.run(["git", "-c", "maintenance.auto=false", "-c", "gc.auto=0", *args],
                                      cwd=root, check=True, capture_output=True)

            git("init")
            write("Cargo.toml", '[workspace]\nmembers = ["aida-core", "other"]\n')
            write("aida-core/src/exec_context.rs", 'fn capture() { std::env::vars_os(); }')
            write("other/examples/fixture.rs", 'fn f() { std::env::var("X"); }')
            write("other/target/intentional.rs", 'fn f() { std::env::var("TRACKED"); }')
            write("tests/integration.rs", 'fn f() { std::env::var("Y"); }')
            write("tests/harness.py", 'import os\nos.getenv("X")')
            write("outside/ignored.rs", 'fn f() { std::env::var("Z"); }')
            write("other/notes.py", 'import os\nos.getenv("Z")')
            write(".gitignore", 'tests/.venv/\n')
            git("add", ".")
            git("add", "--force", "other/target/intentional.rs")
            write("other/scratch.rs", b"\xff")
            write("tests/.venv/lib/ignored.py", b"\xff")
            write("tests/scratch.py", 'import os\nos.getenv("Z")')
            sites = GUARD.scan(root)
            self.assertEqual(4, sum(sites.values()))
            self.assertEqual({"other/examples/fixture.rs", "other/target/intentional.rs", "tests/integration.rs", "tests/harness.py"},
                             {site.split(":")[0] for site in sites})
            # Working-tree edits to tracked sources still participate.
            write("other/examples/fixture.rs", 'fn f() {}')
            self.assertEqual(3, sum(GUARD.scan(root).values()))
            (root / "tests/integration.rs").unlink()
            self.assertEqual(2, sum(GUARD.scan(root).values()))
            write("tests/bad.py", b"\xff")
            git("add", "tests/bad.py")
            with self.assertRaisesRegex(ValueError, "tracked source is not UTF-8: tests/bad.py"):
                GUARD.scan(root)

    def test_merge_queue_uses_its_base_sha(self):
        workflow = (GUARD.ROOT / ".github/workflows/ci.yml").read_text()
        base = next(line for line in workflow.splitlines() if "EXEC_CONTEXT_BASE:" in line)
        self.assertIn("github.event.pull_request.base.sha || github.event.merge_group.base_sha || github.event.before", base)

    def test_write_ledger_bootstraps_and_refuses_growth_before_writing(self):
        # trace:STORY-2538 | ai:codex
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)

            def write(name, content):
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(content)

            def git(*args):
                return subprocess.run(
                    ["git", "-c", "maintenance.auto=false", "-c", "gc.auto=0",
                     "-c", "user.name=Fixture", "-c", "user.email=fixture@example.test", *args],
                    cwd=root, check=True, capture_output=True,
                )

            def run(*args):
                return subprocess.run(
                    [sys.executable, str(root / "scripts/check-exec-context.py"), *args],
                    cwd=root, capture_output=True, text=True,
                )

            git("init")
            write("scripts/check-exec-context.py", Path(GUARD.__file__).read_text())
            write("Cargo.toml", '[workspace]\nmembers = ["aida-core"]\n')
            write("aida-core/src/exec_context.rs", 'fn capture() { std::env::vars_os(); }')
            source = 'fn f() { std::env::var("Z"); std::env::var("A"); std::env::var("A"); }'
            write("aida-core/src/legacy.rs", source)
            git("add", ".")
            git("commit", "-m", "base without ledger")
            ledger_path = root / GUARD.LEDGER
            self.assertFalse(ledger_path.exists())
            result = run("--write-ledger", "--base", "HEAD")
            self.assertEqual(0, result.returncode, result.stderr)
            ledger = json.loads(ledger_path.read_text())
            actual = GUARD.scan(root)
            self.assertEqual(dict(actual), ledger["sites"])
            self.assertEqual(3, ledger["ceiling"])
            self.assertEqual([1, 2], sorted(ledger["sites"].values()))
            self.assertEqual(json.dumps(ledger, indent=2, sort_keys=True) + "\n", ledger_path.read_text())
            git("add", ".")
            git("commit", "-m", "base with ledger")
            original = ledger_path.read_bytes()

            for changed in [
                source.replace('var("Z")', 'var("NEW")'),  # Same count, changed expression.
                source.replace('var("Z");', 'var("Z"); std::env::var("Z");'),
            ]:
                with self.subTest(changed=changed):
                    write("aida-core/src/legacy.rs", changed)
                    result = run("--base", "HEAD", "--write-ledger")
                    self.assertNotEqual(0, result.returncode)
                    self.assertIn("may only shrink or relocate", result.stderr)
                    self.assertEqual(original, ledger_path.read_bytes())

            write("aida-core/src/legacy.rs", source.replace("fn f()", "fn renamed()"))
            result = run("--write-ledger", "--base", "HEAD")
            self.assertEqual(0, result.returncode, result.stderr)
            self.assertTrue(all(":renamed:" in key for key in json.loads(ledger_path.read_text())["sites"]))
            write("aida-core/src/legacy.rs", "fn f() {}")
            result = run("--write-ledger", "--base", "HEAD")
            self.assertEqual(0, result.returncode, result.stderr)
            self.assertEqual({"version": 1, "owner": "STORY-2538", "scope": GUARD.SCOPE,
                              "ceiling": 0, "sites": {}},
                             json.loads(ledger_path.read_text()))
            result = run("--base", "HEAD")
            self.assertEqual(0, result.returncode, result.stderr)

            before = ledger_path.read_bytes()
            result = run("--write-ledger", "--base", "missing-ref")
            self.assertNotEqual(0, result.returncode)
            self.assertEqual(before, ledger_path.read_bytes())

    def test_python_harness_sources_include_aliases_and_mutation(self):
        for source in [
            'import os as ambient\nambient.getenv("HOME")',
            'from os import *\ngetenv("HOME")',
            'from os import environ as e\ne["HOME"] = "fixture"',
            'import os\nhelper(os.environ)',
            'from os import putenv as write\nwrite("K", "V")',
        ]:
            self.assertTrue(GUARD.python_sites(source), source)
        self.assertEqual([], GUARD.python_sites('text = "os.environ" # os.getenv("K")'))

    def test_python_sites_bind_scope_and_exact_key(self):
        a = GUARD.python_sites('import os\ndef f():\n return os.environ["A"]')
        b = GUARD.python_sites('import os\ndef f():\n return os.environ["B"]')
        c = GUARD.python_sites('import os\ndef g():\n return os.environ["A"]')
        self.assertNotEqual(a, b)
        self.assertNotEqual(a, c)

    def test_repository_matches_exact_ledger(self):
        GUARD.check(GUARD.scan(), json.loads((GUARD.ROOT / GUARD.LEDGER).read_text()))


if __name__ == "__main__":
    unittest.main()
