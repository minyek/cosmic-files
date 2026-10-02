#!/usr/bin/env python3
import argparse
import json
import os
from pathlib import Path
import subprocess
import time

from PIL import Image
from Xlib import X, XK, display
from Xlib.ext import xtest


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    parser.add_argument("--xvfb", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--dialog", action="store_true")
    parser.add_argument("--wayland", action="store_true")
    parser.add_argument("--click-hold", type=float, default=0.05)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    fixture = Path(os.environ["COSMIC_ACCEPTANCE_PROFILE"]) / "gui-fixtures"
    fixture.mkdir(exist_ok=True)
    large = fixture / "large"
    large.mkdir(exist_ok=True)
    for index in range(20000):
        path = large / f"entry-{index:05}.txt"
        if not path.exists():
            path.write_text(f"Log entry {index}\n" * (1 + index % 37))
    (fixture / "folder.desktop").mkdir(exist_ok=True)
    (fixture / "invalid.desktop").write_text("not a desktop entry\n")
    (fixture / "launcher.desktop").write_text(
        "[Desktop Entry]\nType=Application\nName=Acceptance Launcher\nExec=true\n"
    )
    Image.new("RGB", (320, 180), (26, 122, 210)).save(fixture / "photo.png")
    processes = []
    logs = []
    try:
        for name, command in [
            ("xvfb", [args.xvfb, ":97", "-screen", "0", "1280x900x24", "-ac", "-nolisten", "tcp", "-extension", "GLX"]),
        ]:
            log = (args.output / f"{name}.log").open("w")
            logs.append(log)
            processes.append(subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT))
        deadline = time.monotonic() + 10
        while not Path("/tmp/.X11-unix/X97").exists():
            if time.monotonic() > deadline or processes[0].poll() is not None:
                raise RuntimeError("private display failed to start")
            time.sleep(0.05)
        connection = display.Display(":97")
        root = connection.screen().root
        wayland_display = None
        if args.wayland:
            log = (args.output / "compositor.log").open("w")
            logs.append(log)
            compositor_env = os.environ | {
                "DISPLAY": ":97", "COSMIC_BACKEND": "winit",
                "__EGL_VENDOR_LIBRARY_FILENAMES": "/usr/share/glvnd/egl_vendor.d/50_mesa.json",
                "LIBGL_ALWAYS_SOFTWARE": "1",
            }
            compositor_env.pop("WAYLAND_DISPLAY", None)
            compositor = subprocess.Popen(["cosmic-comp", "--no-xwayland"], env=compositor_env, stdout=log, stderr=subprocess.STDOUT)
            processes.append(compositor)
            deadline = time.monotonic() + 20
            while wayland_display is None:
                for path in Path(os.environ["XDG_RUNTIME_DIR"]).glob("wayland-*"):
                    if path.is_socket():
                        wayland_display = path.name
                        break
                assert compositor.poll() is None, "nested compositor exited"
                assert time.monotonic() < deadline, "nested compositor did not start"
                time.sleep(0.1)
        log = (args.output / "application.log").open("w")
        logs.append(log)
        env = os.environ | {"DISPLAY": ":97", "WINIT_UNIX_BACKEND": "x11", "RUST_LOG": "cosmic_files=info"}
        env.pop("WAYLAND_DISPLAY", None)
        if wayland_display:
            env["WAYLAND_DISPLAY"] = wayland_display
            env.pop("WINIT_UNIX_BACKEND", None)
        app = subprocess.Popen([args.binary, "--no-daemon", str(fixture)], env=env, stdout=log, stderr=subprocess.STDOUT)
        processes.append(app)
        deadline = time.monotonic() + 40
        window = None
        while window is None:
            if app.poll() is not None:
                raise RuntimeError(f"application exited: {app.returncode}")
            for child in root.query_tree().children:
                if child.get_attributes().map_state == X.IsViewable and child.get_geometry().width > 100:
                    window = child
                    break
            if time.monotonic() > deadline:
                raise RuntimeError("application did not map a window")
            time.sleep(0.1)
        window.configure(x=0, y=0, width=1280, height=900)
        window.set_input_focus(X.RevertToParent, X.CurrentTime)
        connection.sync()
        time.sleep(3)

        def screenshot(name):
            assert app.poll() is None, f"application exited before {name}: {app.returncode}"
            raw = root.get_image(0, 0, 1280, 900, X.ZPixmap, 0xFFFFFFFF)
            image = Image.frombytes("RGB", (1280, 900), raw.data, "raw", "BGRX")
            image.save(args.output / f"{name}.png")
            return image

        def key(name, ctrl=False):
            control = connection.keysym_to_keycode(XK.string_to_keysym("Control_L"))
            code = connection.keysym_to_keycode(XK.string_to_keysym(name))
            if ctrl:
                xtest.fake_input(connection, X.KeyPress, control)
            xtest.fake_input(connection, X.KeyPress, code)
            xtest.fake_input(connection, X.KeyRelease, code)
            if ctrl:
                xtest.fake_input(connection, X.KeyRelease, control)
            connection.sync()
            time.sleep(0.5)

        def click(x, y, button=1, count=1, hold=None):
            xtest.fake_input(connection, X.MotionNotify, x=x, y=y)
            for _ in range(count):
                xtest.fake_input(connection, X.ButtonPress, button)
                connection.sync()
                time.sleep(args.click_hold if hold is None else hold)
                xtest.fake_input(connection, X.ButtonRelease, button)
                connection.sync()
                time.sleep(0.08)
            time.sleep(0.5)

        def type_text(text):
            for character in text:
                code = connection.keysym_to_keycode(ord(character))
                xtest.fake_input(connection, X.KeyPress, code)
                xtest.fake_input(connection, X.KeyRelease, code)
            connection.sync()
            time.sleep(0.3)

        if args.wayland:
            deadline = time.monotonic() + 20
            while True:
                frame = screenshot("window-setup")
                if any(g > 120 and b > 100 and r < g * 0.8 for r, g, b in frame.crop((20, 15, 45, 35)).getdata()):
                    break
                assert time.monotonic() < deadline, "application did not maximize"
                click(1002, 136, hold=0.1)
                time.sleep(1)
        screenshot("initial-grid")
        if args.dialog:
            click(60, 72)
            deadline = time.monotonic() + 15
            dialog = None
            while dialog is None:
                for child in root.query_tree().children:
                    if child.id != window.id and child.get_attributes().map_state == X.IsViewable and child.get_geometry().width > 100:
                        dialog = child
                        break
                assert time.monotonic() < deadline, "file picker did not open"
                assert app.poll() is None, "application exited while opening picker"
                time.sleep(0.1)
            dialog.configure(x=0, y=0, width=1280, height=900)
            dialog.set_input_focus(X.RevertToParent, X.CurrentTime)
            connection.sync()
            time.sleep(1)
            key("l", ctrl=True)
            key("a", ctrl=True)
            type_text(str(large))
            key("Return")
            time.sleep(2)
            key("1", ctrl=True)
            screenshot("picker-large-list")
            key("End")
            screenshot("picker-large-end")
            key("Home")
            added = large / "00000-picker-created.txt"
            added.write_text("created while the picker is open\n")
            time.sleep(2)
            screenshot("picker-watcher-created")
            added.unlink()
            time.sleep(2)
            screenshot("picker-watcher-removed")
            key("End")
            key("Return")
            time.sleep(1)
            screenshot("picker-selected-result")
            mapped = [child.id for child in root.query_tree().children if child.get_attributes().map_state == X.IsViewable]
            assert dialog.id not in mapped, "picker did not return the selected file"
            (args.output / "result.json").write_text(json.dumps({"picker_opened": True, "picker_returned": True, "fixture_count": 20000}, indent=2))
            return
        key("1", ctrl=True)
        screenshot("initial-list")
        for name, y in [("directory-menu", 140), ("launcher-menu", 240), ("invalid-launcher-menu", 290)]:
            click(430, y, button=3)
            screenshot(name)
            key("Escape")
        click(430, 240, button=3)
        screenshot("launcher-menu-pending")
        key("Escape")
        click(430, 240, button=3)
        screenshot("launcher-menu-ready")
        key("Escape")
        key("End")
        key("space", ctrl=True)
        screenshot("image-details")
        key("space", ctrl=True)
        click(430, 190, count=2)
        time.sleep(2)
        screenshot("large-list-top")
        key("End")
        screenshot("large-list-end")
        key("Home")
        xtest.fake_input(connection, X.MotionNotify, x=700, y=450)
        for _ in range(120):
            xtest.fake_input(connection, X.ButtonPress, 5)
            xtest.fake_input(connection, X.ButtonRelease, 5)
            connection.sync()
            time.sleep(0.01)
        time.sleep(0.5)
        screenshot("large-list-scrolled")
        key("Home")
        added = large / "00000-watcher-created.txt"
        added.write_text("created while the directory is visible\n")
        time.sleep(2)
        screenshot("watcher-created")
        added.unlink()
        time.sleep(2)
        screenshot("watcher-removed")
        key("2", ctrl=True)
        screenshot("large-grid-top")
        key("End")
        screenshot("large-grid-end")
        key("Home")
        for _ in range(120):
            xtest.fake_input(connection, X.ButtonPress, 5)
            xtest.fake_input(connection, X.ButtonRelease, 5)
            connection.sync()
            time.sleep(0.01)
        time.sleep(0.5)
        screenshot("large-grid-scrolled")
        assert app.poll() is None, "application exited during interaction"
        (args.output / "result.json").write_text(json.dumps({"mapped_window": window.get_wm_name(), "pid": app.pid, "fixture_count": 20000}, indent=2))
    finally:
        for process in reversed(processes):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
        for log in logs:
            log.close()


if __name__ == "__main__":
    main()
