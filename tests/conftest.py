import asyncio
import collections
import importlib.util
import os
import sys
import tempfile
import time

import pytest

GUARD = os.path.join(os.path.dirname(__file__), "guard")
GUARD_LOG = os.path.join(tempfile.mkdtemp(prefix="hc-guard-"), "touched")
os.environ["HYPR_COMPUTER_TEST_GUARD"] = GUARD_LOG
os.environ["PYTHONPATH"] = os.pathsep.join(filter(None, [GUARD, os.environ.get("PYTHONPATH")]))
_spec = importlib.util.spec_from_file_location("guard", os.path.join(GUARD, "sitecustomize.py"))
_spec.loader.exec_module(importlib.util.module_from_spec(_spec))  # the guard, here as in every process started

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
import server  # noqa: E402
from fakes import FakeHyprland, FakeInput, fake_bin  # noqa: E402


def pytest_sessionfinish(session):
    touched = open(GUARD_LOG).read() if os.path.exists(GUARD_LOG) else ""
    if touched:
        print(f"\nTests touched the real desktop (pid, what):\n{touched}", file=sys.stderr)
        session.exitstatus = pytest.ExitCode.TESTS_FAILED


@pytest.fixture(autouse=True)
def fresh_binaries():
    server.binary.cache_clear()


@pytest.fixture(autouse=True)
def no_real_desktop(monkeypatch, tmp_path_factory):
    """Points every test, and every server it starts, at an empty runtime folder. Otherwise the server finds the
    Hyprland session and the notification service of the machine the tests run on, and acts on them."""
    run = tmp_path_factory.mktemp("run")
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(run))
    monkeypatch.setenv("DBUS_SESSION_BUS_ADDRESS", f"unix:path={run}/bus")
    stubs = run / "bin"  # notify-send with no service to reach hangs: servers the tests start get one that fails
    stubs.mkdir()
    (stubs / "notify-send").write_text("#!/bin/sh\nexit 1\n")
    (stubs / "notify-send").chmod(0o755)
    monkeypatch.setenv("PATH", f"{stubs}:{os.environ['PATH']}")
    monkeypatch.setenv("HYPRLAND_INSTANCE_SIGNATURE", "none")
    monkeypatch.setenv("WAYLAND_DISPLAY", "wayland-none")


@pytest.fixture
def hypr(monkeypatch):
    fake = FakeHyprland()
    for k, v in fake.env.items():
        monkeypatch.setenv(k, v)
    yield fake
    fake.close()


class Desktop:
    """The in-process MCP server on a fake Hyprland, with a recording virtual input."""

    def __init__(self, hypr: FakeHyprland, vi: FakeInput):
        self.hypr, self.input, self.mcp = hypr, vi, server.build_server()

    def result(self, tool: str, /, **args) -> tuple[str, list[bytes]]:
        out = asyncio.run(self.mcp.call_tool(tool, args))
        blocks = out[0] if isinstance(out, tuple) else out
        return ("\n".join(b.text for b in blocks if b.type == "text"),
                [b.data for b in blocks if b.type == "image"])

    def call(self, tool: str, /, **args) -> str:
        return self.result(tool, **args)[0]

    def tools(self) -> list:
        return asyncio.run(self.mcp.list_tools())


@pytest.fixture
def desktop(hypr, monkeypatch):
    """In HYPR_COMPUTER_WORKSPACE=user, which works on the user's screen as v0.7 did, and without wl-mirror."""
    vi = FakeInput()
    monkeypatch.setattr(server, "_input", vi)
    monkeypatch.setattr(server, "SETTLE_S", 0)
    monkeypatch.setattr(server, "EVENTS", server.HyprEvents())
    monkeypatch.setattr(server, "_shown", None)
    monkeypatch.setattr(server, "_last_input", (0.0, 0.0))
    monkeypatch.setattr(server, "_last_action", 0.0)
    monkeypatch.setattr(server, "_waited", 0.0)
    monkeypatch.setattr(server, "_ocr_cache", collections.OrderedDict())
    monkeypatch.setattr(server, "_refs", {})
    monkeypatch.setattr(server, "_goals", {})
    monkeypatch.setattr(server, "_pointer", None)
    monkeypatch.setattr(server, "_grant_noted", server.NO_GRANT)
    monkeypatch.setattr(server, "_view", "")
    monkeypatch.setattr(server, "_note", "")
    monkeypatch.setattr(server, "_noticed", 0.0)
    monkeypatch.setattr(server, "_notice", None)
    monkeypatch.setattr(server, "_notice_id", "")
    monkeypatch.setattr(server, "NOTIFY_SEND", os.path.join(hypr.root, "no-notify-send"))
    monkeypatch.setattr(server, "_started", False)
    monkeypatch.setattr(server, "_stopped", False)
    monkeypatch.setattr(server, "_still", None)
    monkeypatch.setattr(server, "WL_MIRROR", os.path.join(hypr.root, "no-wl-mirror"))
    monkeypatch.setenv("HYPR_COMPUTER_WORKSPACE", "user")
    monkeypatch.delenv("HYPR_COMPUTER_VIEW_WORKSPACE", raising=False)
    monkeypatch.delenv("HYPR_COMPUTER_TOOLS", raising=False)
    monkeypatch.delenv("HYPR_COMPUTER_RECORD_DIR", raising=False)
    d = Desktop(hypr, vi)
    end = time.monotonic() + 5
    while not server.EVENTS.connected:
        assert time.monotonic() < end, "the server never connected to the event socket"
        time.sleep(0.01)
    yield d
    server.EVENTS.close()


@pytest.fixture
def screen(desktop, tmp_path, monkeypatch):
    """A fake grim for the 2560x1600 panel: a dark screen with a white square at (100, 100) logical."""
    monkeypatch.setenv("PATH", f"{fake_bin(str(tmp_path / 'bin'))}:{os.environ['PATH']}")
    monkeypatch.setenv("FAKE_GRIM_SIZE", "2560x1600")
    monkeypatch.setenv("FAKE_GRIM_MARK", "200,200,40")
    return desktop
