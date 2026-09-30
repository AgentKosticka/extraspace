#!/usr/bin/python3
"""Optional GUI lifecycle test on a private D-Bus; no tablet/session changes.

Run from a graphical session:
  dbus-run-session -- /usr/bin/python3 scripts/tests/test_tray.py ./target/release/extraspace
Requires system Python's GI bindings. A private watcher exercises real SNI IPC.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

from gi.repository import Gio, GLib

APP = "io.github.tymonoman.Extraspace"
bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)


def watcher():
    xml = """<node><interface name="org.kde.StatusNotifierWatcher">
      <method name="RegisterStatusNotifierItem"><arg type="s" direction="in"/></method>
      <property name="IsStatusNotifierHostRegistered" type="b" access="read"/>
      <property name="ProtocolVersion" type="i" access="read"/>
      <property name="RegisteredStatusNotifierItems" type="as" access="read"/>
    </interface></node>"""
    interface = Gio.DBusNodeInfo.new_for_xml(xml).interfaces[0]
    def method(_connection, _sender, _path, _iface, _method, _args, invocation):
        invocation.return_value(GLib.Variant("()", ()))
    def prop(_connection, _sender, _path, _iface, name):
        return {"IsStatusNotifierHostRegistered": GLib.Variant("b", True),
                "ProtocolVersion": GLib.Variant("i", 0),
                "RegisteredStatusNotifierItems": GLib.Variant("as", [])}[name]
    bus.register_object("/StatusNotifierWatcher", interface, method, prop, None)
    Gio.bus_own_name_on_connection(bus, "org.kde.StatusNotifierWatcher", Gio.BusNameOwnerFlags.NONE, None, None)
    GLib.MainLoop().run()


def call(destination, path, interface, method, args=None):
    return bus.call_sync(destination, path, interface, method, args, None,
                         Gio.DBusCallFlags.NONE, 3000, None).unpack()


def wait_for(condition, description):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        try:
            if condition():
                return
        except GLib.Error:
            pass
        time.sleep(0.1)
    raise AssertionError("Timed out: " + description)


def action(name):
    call(APP, "/io/github/tymonoman/Extraspace", "org.gtk.Actions", "Activate",
         GLib.Variant("(sava{sv})", (name, [], {})))


def test(binary):
    # Never take over a real session watcher or an existing Extraspace process.
    names = call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "ListNames")[0]
    assert APP not in names and "org.kde.StatusNotifierWatcher" not in names, "Use dbus-run-session for this test"
    with tempfile.TemporaryDirectory(prefix="extraspace-tray-") as folder:
        root = Path(folder)
        config = root / "config/extraspace"
        config.mkdir(parents=True)
        (config / "config.json").write_text(json.dumps({"auto_connect": False, "keep_running_in_tray": True}))
        env = dict(os.environ, XDG_CONFIG_HOME=str(root / "config"),
                   XDG_STATE_HOME=str(root / "state"), RUST_LOG="info")
        log = root / "app.log"
        host = subprocess.Popen([sys.executable, __file__, "--watcher"])
        app = None
        try:
            wait_for(lambda: call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "NameHasOwner",
                                  GLib.Variant("(s)", ("org.kde.StatusNotifierWatcher",)))[0], "private tray watcher")
            with log.open("w") as output:
                app = subprocess.Popen([str(binary)], env=env, stdout=output, stderr=output)
                wait_for(lambda: call(APP, "/io/github/tymonoman/Extraspace", "org.gtk.Actions", "DescribeAll")[0]["hide-window"][0], "tray registration")
                action("close-window")
                wait_for(lambda: "window closed to tray" in log.read_text(), "close to tray")
                assert app.poll() is None, "closing the window stopped the app"
                # Removing the tray while hidden must restore the window.
                host.terminate()
                host.wait(timeout=5)
                wait_for(lambda: "tray disappeared; restoring hidden window" in log.read_text(), "fallback window")
                # Closing with a missing tray must keep the window accessible.
                action("close-window")
                assert app.poll() is None
                action("quit")
                assert app.wait(timeout=10) == 0
            print("PASS: close to tray, watcher loss, accessible fallback, explicit quit")
        finally:
            for process in (app, host):
                if process is not None and process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)


if __name__ == "__main__":
    if sys.argv[1:] == ["--watcher"]:
        watcher()
    elif len(sys.argv) == 2:
        test(Path(sys.argv[1]).resolve())
    else:
        raise SystemExit(__doc__)
