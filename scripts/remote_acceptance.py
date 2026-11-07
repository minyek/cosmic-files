#!/usr/bin/python3
import ctypes
import errno
import json
import os
import subprocess
from pathlib import Path

import gi

gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib


URI = os.environ["COSMIC_TEST_REMOTE_URI"]
ROOT = Path(os.environ["COSMIC_TEST_REMOTE_ROOT"])
AT_FDCWD = -100
RENAME_NOREPLACE = 1
ATTRIBUTES = ",".join(
    (
        Gio.FILE_ATTRIBUTE_STANDARD_NAME,
        Gio.FILE_ATTRIBUTE_STANDARD_DISPLAY_NAME,
        Gio.FILE_ATTRIBUTE_STANDARD_TYPE,
        Gio.FILE_ATTRIBUTE_STANDARD_SIZE,
        Gio.FILE_ATTRIBUTE_STANDARD_ICON,
        Gio.FILE_ATTRIBUTE_STANDARD_IS_HIDDEN,
        Gio.FILE_ATTRIBUTE_TIME_MODIFIED,
    )
)


def scan(uri: str) -> list[dict]:
    directory = Gio.File.new_for_uri(uri)
    enumerator = directory.enumerate_children(
        ATTRIBUTES,
        Gio.FileQueryInfoFlags.NONE,
        None,
    )
    items = []
    while True:
        info = enumerator.next_file(None)
        if info is None:
            break
        child = directory.get_child(info.get_name())
        items.append(
            {
                "name": info.get_name(),
                "display_name": info.get_display_name(),
                "type": "directory"
                if info.get_file_type() == Gio.FileType.DIRECTORY
                else "file",
                "size": info.get_size(),
                "hidden": info.get_is_hidden(),
                "uri": child.get_uri(),
                "path": child.get_path(),
            }
        )
    enumerator.close(None)
    return sorted(items, key=lambda item: item["name"])


def assert_shape(items: list[dict], expected: dict[str, str]) -> None:
    actual = {item["name"]: item["type"] for item in items}
    assert actual == expected, (actual, expected)


def mount_anonymously(file: Gio.File) -> None:
    loop = GLib.MainLoop()
    error = []
    operation = Gio.MountOperation()

    def ask_password(op, _message, _default_user, _default_domain, _flags):
        op.set_anonymous(True)
        op.reply(Gio.MountOperationResult.HANDLED)

    def mounted(source, result, _data):
        try:
            source.mount_enclosing_volume_finish(result)
        except GLib.Error as err:
            error.append(err)
        finally:
            loop.quit()

    operation.connect("ask-password", ask_password)
    file.mount_enclosing_volume(
        Gio.MountMountFlags.NONE,
        operation,
        None,
        mounted,
        None,
    )
    loop.run()
    if error:
        raise error[0]


def unmount(file: Gio.File) -> None:
    mount = file.find_enclosing_mount(None)
    loop = GLib.MainLoop()
    error = []

    def unmounted(source, result, _data):
        try:
            source.unmount_with_operation_finish(result)
        except GLib.Error as err:
            error.append(err)
        finally:
            loop.quit()

    mount.unmount_with_operation(
        Gio.MountUnmountFlags.NONE,
        None,
        None,
        unmounted,
        None,
    )
    loop.run()
    if error:
        raise error[0]


production_probe = os.environ["COSMIC_PROBE_BIN"]
production_results = {}


def run_production(label: str, uri: str) -> None:
    result = subprocess.run(
        (production_probe, label, uri),
        check=True,
        text=True,
        capture_output=True,
    )
    production_results[label] = result.stdout.strip().splitlines()


def probe_no_replace_rename(items: list[dict]) -> dict:
    source = Path(next(item["path"] for item in items if item["name"] == "root-file.txt"))
    destination = source.with_name("renamed-root.txt")
    libc = ctypes.CDLL(None, use_errno=True)
    rename = libc.renameat2
    rename.argtypes = (ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint)
    rename.restype = ctypes.c_int
    result = rename(AT_FDCWD, os.fsencode(source), AT_FDCWD, os.fsencode(destination), RENAME_NOREPLACE)
    if result != 0:
        error = ctypes.get_errno()
        if error not in (errno.EINVAL, errno.ENOSYS, errno.EOPNOTSUPP):
            raise OSError(error, os.strerror(error))
        assert (ROOT / "root-file.txt").read_bytes() == b"root payload\n"
        assert not (ROOT / "renamed-root.txt").exists()
        return {"supported": False, "errno": error}
    assert (ROOT / "renamed-root.txt").read_bytes() == b"root payload\n"
    (ROOT / "renamed-root.txt").rename(ROOT / "root-file.txt")
    existing = ROOT / "renamed-root.txt"
    existing.write_bytes(b"existing destination\n")
    try:
        result = rename(AT_FDCWD, os.fsencode(source), AT_FDCWD, os.fsencode(destination), RENAME_NOREPLACE)
        assert result == -1 and ctypes.get_errno() == errno.EEXIST
        assert (ROOT / "root-file.txt").read_bytes() == b"root payload\n"
        assert existing.read_bytes() == b"existing destination\n"
    finally:
        existing.unlink()
    return {"supported": True, "existing_destination_preserved": True}


root_file = Gio.File.new_for_uri(URI)
mount_anonymously(root_file)
target_info = root_file.query_info(
    "standard::target-uri",
    Gio.FileQueryInfoFlags.NONE,
    None,
)
fs_info = root_file.query_filesystem_info(
    f"{Gio.FILE_ATTRIBUTE_FILESYSTEM_TYPE},{Gio.FILE_ATTRIBUTE_FILESYSTEM_REMOTE}",
    None,
)

initial = scan(URI)
assert_shape(
    initial,
    {"empty-folder": "directory", "nested": "directory", "root-file.txt": "file"},
)
run_production("initial", URI)

nested = scan(f"{URI}nested/")
assert_shape(nested, {"nested-file.txt": "file"})
run_production("nested", f"{URI}nested/")

(ROOT / "created-after-mount.txt").write_text("created payload\n")
(ROOT / "created-folder").mkdir()
after_create = scan(URI)
assert_shape(
    after_create,
    {
        "created-after-mount.txt": "file",
        "created-folder": "directory",
        "empty-folder": "directory",
        "nested": "directory",
        "root-file.txt": "file",
    },
)
run_production("after-create", URI)

(ROOT / "created-after-mount.txt").unlink()
(ROOT / "created-folder").rmdir()
after_remove = scan(URI)
assert_shape(
    after_remove,
    {"empty-folder": "directory", "nested": "directory", "root-file.txt": "file"},
)
run_production("after-remove", URI)
rename_capability = probe_no_replace_rename(after_remove)

print(
    json.dumps(
        {
            "backend": {
                "filesystem_type": fs_info.get_attribute_string(
                    Gio.FILE_ATTRIBUTE_FILESYSTEM_TYPE
                ),
                "filesystem_remote": fs_info.get_attribute_boolean(
                    Gio.FILE_ATTRIBUTE_FILESYSTEM_REMOTE
                ),
                "root_path": root_file.get_path(),
                "target_uri": target_info.get_attribute_string("standard::target-uri"),
            },
            "initial": initial,
            "nested": nested,
            "after_create": after_create,
            "after_remove": after_remove,
            "production_scan": production_results,
            "rename_noreplace": rename_capability,
            "assertions": "PASS",
        },
        indent=2,
        sort_keys=True,
    )
)
unmount(root_file)
