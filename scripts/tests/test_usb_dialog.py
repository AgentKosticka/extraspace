#!/usr/bin/python3
"""Private-bus GTK smoke test; never connects to a tablet.
Run: dbus-run-session -- /usr/bin/python3 scripts/tests/test_usb_dialog.py PATH_TO_BINARY
"""
from gi.repository import Gio, GLib
import subprocess, tempfile, json, time, os, sys
from pathlib import Path
binary = Path(sys.argv[1]).resolve()
with tempfile.TemporaryDirectory(prefix="extraspace-usb-menu-") as d:
    settings = Path(d) / "extraspace"
    settings.mkdir()
    (settings / "config.json").write_text(json.dumps({"auto_connect": False}))
    with tempfile.TemporaryFile() as log:
        process = subprocess.Popen([str(binary)], env=dict(os.environ, XDG_CONFIG_HOME=d),
                                   stdout=log, stderr=subprocess.STDOUT)
        bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
        def call(method, args=None):
            return bus.call_sync("io.github.tymonoman.Extraspace", "/io/github/tymonoman/Extraspace",
                                 "org.gtk.Actions", method, args, None,
                                 Gio.DBusCallFlags.NONE, 1000, None).unpack()
        try:
            actions = []
            for _ in range(80):
                try:
                    actions = call("List")[0]
                    if "usb-connection" in actions: break
                except GLib.Error: pass
                time.sleep(.1)
            assert "usb-connection" in actions
            def action(name): call("Activate", GLib.Variant("(sava{sv})", (name, [], {})))
            action("usb-connection")
            time.sleep(.5)
            assert process.poll() is None
            action("quit")
            assert process.wait(timeout=8) == 0
            log.seek(0)
            output = log.read().decode()
            assert "CRITICAL" not in output, output
            print("PASS: disconnected USB dialog and Quit with the dialog open")
        finally:
            if process.poll() is None: process.terminate(); process.wait(timeout=5)
