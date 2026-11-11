"""SPIKE-714: closures must retain the full registered handler expression."""
# trace:SPIKE-714 | ai:codex
import importlib.util
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('command_inventory', ROOT / 'scripts/command-inventory.py')
inventory = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(inventory)


class HandlerExtractionTests(unittest.TestCase):
    def test_nested_call_and_opaque_delimiters(self):
        expression = '''|matches, _| {
            /* ) /* } */ ] */
            let _literal = r###")}]"###;
            let _escaped = "\\\"}";
            let _character = ')';
            let args = parse(matches, nested([1, 2]))?;
            handle(&args.path)
        }'''
        block = 'name: "fixture", run: crate::cmdkit::Handler::Early(' + expression + '),'
        self.assertEqual(inventory.extract_handler(block), ('Early', expression))

    def test_rejects_truncated_and_mismatched_expression(self):
        for block in ('run: Handler::Early(|_, _| handle(',
                      'run: Handler::Early(|_, _| handle(])'):
            with self.subTest(block=block), self.assertRaises(ValueError):
                inventory.extract_handler(block)

    def test_all_three_real_closures_are_complete(self):
        expected = {
            ('presence_cmd', 'AWAY'): '|_, _| handle_away_command()',
            ('presence_cmd', 'HOME'): '|_, _| handle_home_command()',
        }
        for (module, symbol), handler in expected.items():
            source = (ROOT / f'aida-cli-lib/src/{module}.rs').read_text()
            _, block = inventory.static_block(source, symbol)
            self.assertEqual(inventory.extract_handler(block), ('Early', handler))
        source = (ROOT / 'aida-cli-lib/src/session_misc_cmd.rs').read_text()
        _, block = inventory.static_block(source, 'BG_FETCH')
        dispatch, handler = inventory.extract_handler(block)
        self.assertEqual(dispatch, 'Early')
        self.assertTrue(handler.startswith('|matches, _| {'))
        self.assertIn('from_arg_matches(matches)?;', handler)
        self.assertTrue(handler.endswith('handle_bg_fetch_command(&args.store_path)\n    }'))

    def test_static_bounds_ignore_commented_declaration_and_literal_braces(self):
        source = '''// static EXAMPLE: CommandReg = CommandReg { bogus };
        static EXAMPLE: CommandReg = CommandReg {
            name: "} ;",
            run: Handler::Store(run_example),
        };
        static OTHER: CommandReg = CommandReg { run: Handler::Early(other) };'''
        offset, block = inventory.static_block(source, 'EXAMPLE')
        self.assertTrue(source[offset:].startswith('static EXAMPLE:'))
        self.assertEqual(inventory.extract_handler(block), ('Store', 'run_example'))
        self.assertNotIn('OTHER', block)


if __name__ == '__main__':
    unittest.main()
