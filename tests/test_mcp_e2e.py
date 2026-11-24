import base64
import ctypes
import json
import os
import re
import shutil
import signal
import subprocess

import pytest

import server
from fakes import M1_PANEL, FakeHyprland, fake_bin, jpeg_size

ROOT = os.path.join(os.path.dirname(__file__), "..")
CORE_TOOLS = {"screenshot", "click", "move", "drag", "scroll", "type_text", "key", "cursor_position",
              "list_windows"}
TOOLS = {"screenshot", "click", "move", "drag", "scroll", "type_text", "key", "batch", "zoom",
         "focus_window", "launch_app", "open_url", "cursor_position", "list_windows", "wait_for",
         "find_text", "click_text", "ui_find", "ui_act", "agent_display"}
RESERVED_BY_CLAUDE_CODE = {"computer-use"}


def plugin_command() -> list[str]:
    servers = json.load(open(os.path.join(ROOT, ".mcp.json")))["mcpServers"]
    (entry,) = servers.values()
    return [entry["command"]] + [a.replace("${CLAUDE_PLUGIN_ROOT}", ROOT) for a in entry["args"]]


def rpc(proc, msg_id, method, params=None):
    proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": msg_id, "method": method,
                                 "params": params or {}}) + "\n")
    proc.stdin.flush()
    line = proc.stdout.readline()
    assert line, "server exited: " + proc.stderr.read()
    return json.loads(line)


class Session:
    def __init__(self, **env):
        self.proc = subprocess.Popen(plugin_command(), stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=subprocess.PIPE, text=True, env={**os.environ, **env})
        self.n = 0
        self.init = self.call("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                             "clientInfo": {"name": "test", "version": "0"}})
        self.proc.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")

    def call(self, method, params=None):
        self.n += 1
        return rpc(self.proc, self.n, method, params)["result"]

    def tools(self) -> set[str]:
        return {t["name"] for t in self.call("tools/list")["tools"]}

    def close(self):
        self.proc.kill()


needs_uv = pytest.mark.skipif(shutil.which("uv") is None, reason="needs uv")


@needs_uv
def test_server_starts_with_its_declared_dependencies_and_lists_tools():
    s = Session()
    try:
        assert s.init["serverInfo"]["name"] == "hypr-computer"
        assert s.tools() == TOOLS
    finally:
        s.close()


@needs_uv
def test_instructions_only_name_tools_the_agent_has():
    for env, expected in (({}, TOOLS), ({"HYPR_COMPUTER_TOOLS": "core"}, CORE_TOOLS)):
        s = Session(**env)
        try:
            assert s.tools() == expected
            named = set(re.findall(r"[a-z_]+", s.init["instructions"]))
            assert named & TOOLS <= expected, f"instructions name tools the agent lacks: {named & TOOLS - expected}"
            assert ("wait_for" in named) == ("wait_for" in expected)
        finally:
            s.close()


@needs_uv
def test_open_url_refuses_script_and_data_urls():
    s = Session(HYPR_COMPUTER_BROWSER="true")  # if the refusal ever breaks, nothing real opens
    try:
        for url in ("javascript:alert(1)", "data:text/html,<script>x</script>", "chrome://settings"):
            result = s.call("tools/call", {"name": "open_url", "arguments": {"url": url}})
            assert result["isError"], url
            assert "only http(s) URLs" in result["content"][0]["text"]
    finally:
        s.close()


def test_plugin_server_name_is_not_reserved():
    names = set(json.load(open(os.path.join(ROOT, ".mcp.json")))["mcpServers"])
    assert not names & RESERVED_BY_CLAUDE_CODE, (
        f"{names & RESERVED_BY_CLAUDE_CODE} is reserved by Claude Code and is refused at install; "
        "name the server 'hypr-computer'")


def brightness(jpeg: bytes, points: list[tuple[int, int]]) -> list[int]:
    read = ("import io, json, sys; from PIL import Image; im = Image.open(io.BytesIO(sys.stdin.buffer.read()))"
            ".convert('L'); print(json.dumps([im.getpixel(tuple(p)) for p in json.loads(sys.argv[1])]))")
    r = subprocess.run(["uv", "run", "--no-project", "--quiet", "--with", "pillow", "python", "-c", read,
                        json.dumps(points)], input=jpeg, capture_output=True, check=True)
    return json.loads(r.stdout)


class FakeDesktop:
    def __init__(self, tmp_path, monitor=M1_PANEL, mark=(0, 0, 1), **env):
        self.hypr = FakeHyprland(monitors=[monitor])
        s = monitor["scale"]
        self.session = Session(**self.hypr.env, **{"HYPR_COMPUTER_WORKSPACE": "user", **env},
                               PATH=f"{fake_bin(str(tmp_path / 'bin'))}:{os.environ['PATH']}",
                               FAKE_GRIM_SIZE=f"{monitor['width']}x{monitor['height']}",
                               FAKE_GRIM_MARK=",".join(str(round(v * s)) for v in mark),
                               HYPR_COMPUTER_SETTLE_MS="0")

    def call(self, tool, /, **args):
        result = self.session.call("tools/call", {"name": tool, "arguments": args})
        text = "\n".join(c["text"] for c in result["content"] if c["type"] == "text")
        images = [base64.b64decode(c["data"]) for c in result["content"] if c["type"] == "image"]
        return result.get("isError", False), text, images

    def close(self):
        self.session.close()
        self.hypr.close()


MBP_PANEL = {**M1_PANEL, "width": 3456, "height": 2234}


@needs_uv
@pytest.mark.parametrize("monitor, size", [(M1_PANEL, (1280, 800)), (MBP_PANEL, (1456, 941))])
def test_screenshot_keeps_its_size_and_maps_pixels_onto_the_desktop(tmp_path, monitor, size):
    x, y, side = 864, 558, 60
    d = FakeDesktop(tmp_path, monitor, mark=(x, y, side), HYPR_COMPUTER_RECORD_DIR=str(tmp_path / "rec"))
    try:
        err, text, (image,) = d.call("screenshot")
    finally:
        d.close()
    assert not err and f"Screenshot {size[0]}x{size[1]} px" in text
    assert jpeg_size(image) == size
    lay = server.layout_from_monitors([monitor])
    probes = [(x + side / 2, y + side / 2), (x - 8, y + side / 2), (x + side + 8, y + side / 2),
              (x + side / 2, y - 8), (x + side / 2, y + side + 8)]
    inside, *outside = brightness(image, [lay.to_shot(*p) for p in probes])
    assert inside > 200 and max(outside) < 80, (inside, outside)
    (step,) = [json.loads(line) for line in (tmp_path / "rec" / "steps.jsonl").read_text().splitlines()]
    assert step["capture_ms"] > 0 and step["total_ms"] >= step["capture_ms"] + step["ipc_ms"]


@needs_uv
def test_every_action_can_return_a_screenshot(tmp_path):
    d = FakeDesktop(tmp_path)
    d.hypr.clients = [{"class": "foot", "title": "t", "address": "0xf", "workspace": {"id": 9, "name": "9"},
                       "at": [0, 0], "size": [100, 100]}]
    try:
        err, text, images = d.call("focus_window", query="foot", screenshot=True)
    finally:
        d.close()
    assert not err and len(images) == 1 and jpeg_size(images[0]) == (1280, 800)
    assert "Screenshot 1280x800 px" in text and "Pointer at (0, 0)." in text


@needs_uv
def test_open_url_opens_files_only_under_the_listed_folders(tmp_path):
    www = tmp_path / "www"
    www.mkdir()
    (www / "page.html").write_text("hi")
    d = FakeDesktop(tmp_path, HYPR_COMPUTER_BROWSER="true", HYPR_COMPUTER_FILE_URLS=str(www))
    try:
        opened = d.call("open_url", url=f"file://{www}/page.html", wait_ms=0)
        refused = d.call("open_url", url="file:///etc/passwd", wait_ms=0)
    finally:
        d.close()
    assert not opened[0] and len(opened[2]) == 1
    (cmd,) = d.hypr.dispatched("exec_cmd")
    assert f"file://{www}/page.html" in cmd
    assert refused[0] and "HYPR_COMPUTER_FILE_URLS" in refused[1]


TGKILL = {"x86_64": 234, "aarch64": 131}.get(os.uname().machine)


def sigterm_to_a_thread_other_than_main(uv_pid: int) -> None:
    """kill() lets the kernel pick the thread that gets the signal; this picks one the main thread isn't."""
    (server_pid,) = [c for t in os.listdir(f"/proc/{uv_pid}/task")
                     for c in open(f"/proc/{uv_pid}/task/{t}/children").read().split()]
    tids = [t for t in os.listdir(f"/proc/{server_pid}/task") if t != server_pid]
    libc = ctypes.CDLL(None, use_errno=True)
    assert libc.syscall(TGKILL, int(server_pid), int(tids[0]), signal.SIGTERM) == 0, ctypes.get_errno()


@needs_uv
@pytest.mark.parametrize("exit_by", ["closing stdin", "SIGTERM", "SIGTERM to another thread"])
def test_the_server_turns_off_the_agent_display_it_started_and_its_live_view_when_it_exits(tmp_path, exit_by):
    d = FakeDesktop(tmp_path)
    try:
        assert not d.call("agent_display", action="start")[0]
        (view,) = d.hypr.processes
        proc = d.session.proc
        if exit_by == "closing stdin":
            proc.stdin.close()
        elif exit_by == "SIGTERM":
            proc.terminate()
        else:
            sigterm_to_a_thread_other_than_main(proc.pid)
        proc.wait(timeout=30)
        view.wait(timeout=5)
    finally:
        d.close()
    assert "/output remove hypr-computer" in d.hypr.requests


@needs_uv
def test_a_server_leaves_an_agent_display_it_didnt_start(tmp_path):
    d = FakeDesktop(tmp_path)
    try:
        d.hypr.reply("/output create headless hypr-computer")  # left on by an earlier server
        assert "left on by an earlier session" in d.call("agent_display", action="status")[1]
        d.session.proc.stdin.close()
        d.session.proc.wait(timeout=30)
    finally:
        d.close()
    assert not [r for r in d.hypr.requests if r.startswith("/output remove")]


@needs_uv
def test_dependencies_are_locked_and_pinned_to_a_date():
    script = os.path.join(ROOT, "server.py")
    header = open(script).read().split("# ///")[1]
    assert re.search(r'^# exclude-newer = "\d{4}-\d\d-\d\dT', header, re.M), header
    r = subprocess.run(["uv", "lock", "--script", script, "--check"], capture_output=True, text=True)
    assert r.returncode == 0, "server.py.lock is missing or stale; run uv lock --script server.py\n" + r.stderr
