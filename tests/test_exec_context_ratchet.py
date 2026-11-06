"""Mutation fixtures for the staged execution-context ledger."""
# trace:STORY-2538 | ai:codex
import importlib.util
from pathlib import Path
import unittest

SPEC = importlib.util.spec_from_file_location("exec_ratchet", Path(__file__).resolve().parents[1] / "scripts/check-exec-context.py")
GUARD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GUARD)


class ExecContextRatchetTests(unittest.TestCase):
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
        from collections import Counter
        ledger = {"version": 1, "owner": "STORY-2538", "ceiling": 1, "sites": {"a": 1}}
        GUARD.check(Counter({"a": 1}), ledger)
        for sites in [{"a": 2}, {"b": 1}, {}]:
            with self.assertRaises(ValueError):
                GUARD.check(Counter(sites), ledger)
        GUARD.check_growth({**ledger, "ceiling": 0, "sites": {}}, ledger)
        with self.assertRaises(ValueError):
            GUARD.check_growth({**ledger, "sites": {"b": 1}}, ledger)

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
        import json
        GUARD.check(GUARD.scan(), json.loads((GUARD.ROOT / GUARD.LEDGER).read_text()))


if __name__ == "__main__":
    unittest.main()
