# AIDA Terminator bridge.
# AIDA-MANAGED: terminator-plugin-v1
#
# Trust boundary: this plugin exposes Focus(uuid) and Send(uuid, text) on the
# current user's DBus session bus only (net.aida.Terminator). Send injects text
# into a shell at the same trust level as Terminator's Custom Commands plugin:
# it calls terminal.vte.feed_child(text.encode()). Install and enable it only for
# local accounts where AIDA session control is expected.
#
# Optional token hardening: when ~/.config/terminator/aida-token exists, callers
# should use SendToken(uuid, text, token). Plain Send returns False while the
# token file is present. Keep the token readable only by the local user.

import os

import dbus
import dbus.service
from terminatorlib import plugin
from terminatorlib.terminator import Terminator


AVAILABLE = ["AidaTerminator"]
BUS_NAME = "net.aida.Terminator"
OBJECT_PATH = "/net/aida/Terminator"
IFACE = "net.aida.Terminator"
VERSION = "aida-terminator-1"


class AidaTerminator(plugin.Plugin):
    capabilities = ["aida_terminal_bridge"]

    def __init__(self):
        plugin.Plugin.__init__(self)
        self.bus = dbus.SessionBus()
        self.name = dbus.service.BusName(BUS_NAME, self.bus)
        self.service = AidaTerminatorService(self.bus, OBJECT_PATH)


class AidaTerminatorService(dbus.service.Object):
    def __init__(self, bus, object_path):
        dbus.service.Object.__init__(self, bus, object_path)

    @dbus.service.method(IFACE, in_signature="", out_signature="s")
    def Ping(self):
        return VERSION

    @dbus.service.method(IFACE, in_signature="s", out_signature="b")
    def Focus(self, uuid):
        terminal = self._find_terminal(uuid)
        if terminal is None:
            return False
        terminal.ensure_visible_and_focussed()
        return True

    @dbus.service.method(IFACE, in_signature="ss", out_signature="b")
    def Send(self, uuid, text):
        if self._token_required():
            return False
        return self._send(uuid, text)

    @dbus.service.method(IFACE, in_signature="sss", out_signature="b")
    def SendToken(self, uuid, text, token):
        expected = self._read_token()
        if expected is not None and token != expected:
            return False
        return self._send(uuid, text)

    def _find_terminal(self, uuid):
        if not uuid:
            return None
        return Terminator().find_terminal_by_uuid(str(uuid))

    def _send(self, uuid, text):
        terminal = self._find_terminal(uuid)
        if terminal is None:
            return False
        terminal.vte.feed_child(str(text).encode())
        return True

    def _token_required(self):
        return self._read_token() is not None

    def _read_token(self):
        path = os.path.expanduser("~/.config/terminator/aida-token")
        try:
            with open(path, "r", encoding="utf-8") as f:
                token = f.read().strip()
        except FileNotFoundError:
            return None
        if not token:
            return None
        return token
