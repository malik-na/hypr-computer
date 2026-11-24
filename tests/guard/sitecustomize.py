"""Keeps tests, and every Python process they start (servers included), off the real desktop of the machine
running them. Python loads this module at startup from PYTHONPATH, which conftest.py sets. A test may run only
Python, uv and programs in temporary folders (the fakes), and connect only to sockets in temporary folders.
Anything else is the real Hyprland, D-Bus, browser or notification service: the call fails and is logged, and
the test run fails at the end."""
import os
import shutil
import socket
import subprocess
import tempfile

LOG = os.environ.get("HYPR_COMPUTER_TEST_GUARD")
FIX = ("tests must not touch the real desktop. Add a fake to tests/fakes.py (fake_bin) or a stub in "
       "conftest.no_real_desktop, and point the test at it.")


def temporary(path: str) -> bool:
    real = os.path.realpath(path)
    return any(real.startswith(os.path.realpath(t) + os.sep) for t in {tempfile.gettempdir(), "/tmp"})


def refuse(what: str) -> None:
    with open(LOG, "a") as f:
        f.write(f"{os.getpid()} {what}\n")
    raise PermissionError(f"{what}: {FIX}")


def allowed_program(path: str) -> bool:
    name = os.path.basename(os.path.realpath(path))
    return temporary(path) or name.startswith(("python", "uv"))


if LOG:
    _popen_init = subprocess.Popen.__init__
    _connect = socket.socket.connect

    def guarded_popen(self, args, *rest, **kw):
        program = args if isinstance(args, (str, bytes, os.PathLike)) else args[0]
        program = os.fsdecode(program)
        if kw.get("shell"):
            program = program.split()[0]
        env = kw.get("env") if kw.get("env") is not None else os.environ
        found = shutil.which(program, path=env.get("PATH")) or program
        if not allowed_program(found):
            refuse(f"ran {found}")
        _popen_init(self, args, *rest, **kw)

    def guarded_connect(self, address):
        if self.family == socket.AF_UNIX:
            path = os.fsdecode(address)
            if path.startswith("\0") or not temporary(path):
                refuse(f"connected to {path!r}")
        return _connect(self, address)

    subprocess.Popen.__init__ = guarded_popen
    socket.socket.connect = guarded_connect
