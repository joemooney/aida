"""Forward ratchet for raw terminal marker literals (TASK-3779).

Existing sites are counted per source file; the inventory may only shrink.
Registry values, JSON values and non-output strings are intentionally excluded.
"""
import json
from pathlib import Path
import re
import tempfile
import unittest
from unittest.mock import patch
import unicodedata

ROOT = Path(__file__).resolve().parents[1]
BASELINE = Path(__file__).with_name('terminal_glyph_padding_baseline.json')
OUTPUT = re.compile(r'\b(?:e?print(?:ln)?|format)!\s*\(\s*"((?:[^"\\]|\\.)*)"', re.S)


def missing_padding(text):
    count = 0
    for i, char in enumerate(text):
        cp = ord(char)
        if not (0x1F000 <= cp <= 0x1FAFF or 0x2300 <= cp <= 0x23FF
                or 0x25A0 <= cp <= 0x27BF or char in '↑⇈↳'):
            continue
        end = i + 1
        while end < len(text) and text[end] in '\ufe0e\ufe0f':
            end += 1
        needed = 2 if unicodedata.east_asian_width(char) in 'WFA' else 1
        spaces = len(text[end:]) - len(text[end:].lstrip(' '))
        count += spaces < needed
    return count


def inventory():
    result = {}
    for crate in ('aida-cli', 'aida-cli-lib'):
        for path in sorted((ROOT / crate / 'src').rglob('*.rs')):
            if 'tests' in path.parts:
                continue
            count = sum(missing_padding(m.group(1)) for m in OUTPUT.finditer(path.read_text()))
            if count:
                result[str(path.relative_to(ROOT))] = count
    return result


class TerminalGlyphPadding(unittest.TestCase):
    def test_marker_spacing_policy(self):
        self.assertEqual(missing_padding('🤖x'), 1)
        self.assertEqual(missing_padding('🤖 x'), 1)
        self.assertEqual(missing_padding('🤖  x'), 0)
        self.assertEqual(missing_padding('⚠x'), 1)
        self.assertEqual(missing_padding('⚠ x'), 0)
        self.assertEqual(missing_padding('⏸️x'), 1)
        self.assertEqual(missing_padding('ASCII text ──'), 0)

    # trace:TASK-3779 | ai:codex
    def test_new_unpadded_output_fails_ratchet(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'aida-cli/src/new_marker.rs'
            source.parent.mkdir(parents=True)
            baseline = root / 'baseline.json'
            baseline.write_text('{}')
            with patch(__name__ + '.ROOT', root), patch(__name__ + '.BASELINE', baseline):
                for literal, expected_failures in [('🤖x', 1), ('🤖  x', 0)]:
                    with self.subTest(literal=literal):
                        source.write_text('fn main() { println!("' + literal + '"); }')
                        result = unittest.TestResult()
                        TerminalGlyphPadding('test_raw_output_padding_does_not_regress').run(result)
                        self.assertEqual(result.testsRun, 1)
                        self.assertEqual(result.errors, [])
                        self.assertEqual(len(result.failures), expected_failures)
                        if expected_failures:
                            self.assertIn('aida-cli/src/new_marker.rs', result.failures[0][1])

    def test_raw_output_padding_does_not_regress(self):
        baseline = json.loads(BASELINE.read_text())
        regressions = {path: count for path, count in inventory().items()
                       if count > baseline.get(path, 0)}
        self.assertFalse(regressions, 'Use glyphs::terminal_marker for new marker output: '
                         + str(regressions))


if __name__ == '__main__':
    unittest.main()
