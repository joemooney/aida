import importlib.util
import os
import sys
import tempfile
import types
import unittest
from pathlib import Path


PLUGIN = (
    Path(__file__).resolve().parents[1]
    / "aida-core"
    / "templates"
    / "terminal"
    / "terminator"
    / "aida_terminator.py"
)


class FakeVte:
    def __init__(self):
        self.sent = []

    def feed_child(self, payload):
        self.sent.append(payload)


class FakeTerminal:
    def __init__(self):
        self.vte = FakeVte()
        self.focused = False

    def ensure_visible_and_focussed(self):
        self.focused = True


class FakeTerminator:
    terminals = {}

    def find_terminal_by_uuid(self, uuid):
        return self.terminals.get(uuid)


def install_fake_modules():
    dbus = types.ModuleType("dbus")
    dbus.SessionBus = lambda: object()
    service = types.ModuleType("dbus.service")
    service.BusName = lambda *args, **kwargs: object()

    class Object:
        def __init__(self, *args, **kwargs):
            pass

    service.Object = Object
    service.method = lambda *args, **kwargs: (lambda fn: fn)
    dbus.service = service
    sys.modules["dbus"] = dbus
    sys.modules["dbus.service"] = service

    terminatorlib = types.ModuleType("terminatorlib")
    plugin = types.ModuleType("terminatorlib.plugin")

    class Plugin:
        def __init__(self):
            pass

    plugin.Plugin = Plugin
    terminator = types.ModuleType("terminatorlib.terminator")
    terminator.Terminator = FakeTerminator
    sys.modules["terminatorlib"] = terminatorlib
    sys.modules["terminatorlib.plugin"] = plugin
    sys.modules["terminatorlib.terminator"] = terminator


class AidaTerminatorPluginTest(unittest.TestCase):
    def setUp(self):
        self.home = tempfile.TemporaryDirectory()
        self.old_home = os.environ.get("HOME")
        os.environ["HOME"] = self.home.name
        install_fake_modules()
        spec = importlib.util.spec_from_file_location("aida_terminator", PLUGIN)
        self.module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.module)
        FakeTerminator.terminals = {"term-1": FakeTerminal()}
        self.service = self.module.AidaTerminatorService(object(), "/fake")

    def tearDown(self):
        if self.old_home is None:
            os.environ.pop("HOME", None)
        else:
            os.environ["HOME"] = self.old_home
        self.home.cleanup()

    def test_focus_calls_terminator_focus_api(self):
        self.assertTrue(self.service.Focus("term-1"))
        self.assertTrue(FakeTerminator.terminals["term-1"].focused)
        self.assertFalse(self.service.Focus("missing"))

    def test_send_feeds_child_bytes(self):
        self.assertTrue(self.service.Send("term-1", "hello\n"))
        self.assertEqual(FakeTerminator.terminals["term-1"].vte.sent, [b"hello\n"])
        self.assertFalse(self.service.Send("missing", "ignored"))


if __name__ == "__main__":
    unittest.main()
