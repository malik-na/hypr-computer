import json
import os
import stat
import struct
import sys

import pytest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
import server  # noqa: E402
import wl_input  # noqa: E402
from fakes import fake_bin  # noqa: E402

MBP = {"name": "eDP-1", "x": 0, "y": 0, "width": 3456, "height": 2234, "scale": 2, "transform": 0}


def test_single_hidpi_monitor_layout():
    lay = server.layout_from_monitors([MBP])
    assert (lay.x0, lay.y0, lay.width, lay.height) == (0, 0, 1728, 1117)
    assert lay.factor == pytest.approx(1456 / 1728)


def test_coordinates_round_trip():
    lay = server.layout_from_monitors([MBP])
    assert lay.to_logical(728, 470) == (864, 558)
    assert lay.to_shot(864, 558) == (728, 470)
    for x, y in [(0, 0), (1455, 940), (123, 456)]:
        assert lay.to_shot(*lay.to_logical(x, y)) == pytest.approx((x, y), abs=1)


def test_multi_monitor_with_negative_offset_and_rotation():
    left = {"name": "DP-1", "x": -1080, "y": -200, "width": 1920, "height": 1080,
            "scale": 1, "transform": 1}
    lay = server.layout_from_monitors([MBP, left])
    assert (lay.x0, lay.y0) == (-1080, -200)
    assert (lay.width, lay.height) == (1080 + 1728, 1920)
    assert lay.to_logical(0, 0) == (-1080, -200)


def test_small_screen_is_not_upscaled():
    lay = server.layout_from_monitors([{**MBP, "width": 1280, "height": 800, "scale": 1}])
    assert lay.factor == 1.0


@pytest.mark.parametrize("max_edge, size", [(1456, (1280, 800)), (640.64, (640, 400)), (910.9888, (910, 569))])
def test_screenshot_size_is_what_grim_s_produced(monkeypatch, max_edge, size):
    # measured on a 1280x800 desktop with grim -s 1, -s 0.5005 and -s 0.71171: grim truncates
    monkeypatch.setattr(server, "MAX_EDGE", max_edge)
    lay = server.layout_from_monitors([{**MBP, "width": 2560, "height": 1600}])
    assert lay.shot_size == size


def test_no_monitors_raises():
    with pytest.raises(RuntimeError):
        server.layout_from_monitors([])


def test_parse_combo():
    assert server.parse_combo("super+Return") == [125, 28]
    assert server.parse_combo("Ctrl + Shift + T") == [29, 42, 20]
    assert server.parse_combo("F12") == [88]
    assert server.parse_combo("ctrl+plus") == [29, 13]
    assert server.parse_combo("ctrl++") == [29, 13]
    assert server.parse_combo("ctrl+minus") == [29, 12]
    assert server.parse_combo("PgDn") == [109]


@pytest.mark.parametrize("bad", ["", "ctrl+nope", "hyper+a"])
def test_parse_combo_rejects_unknown(bad):
    with pytest.raises(ValueError):
        server.parse_combo(bad)


def test_us_table_covers_printable_ascii():
    for ch in map(chr, range(32, 127)):
        assert wl_input.VirtualInput.typeable(ch), ch
    assert wl_input.US_KEYS["A"] == (30, True)
    assert wl_input.US_KEYS["?"] == (53, True)
    assert wl_input.US_KEYS["/"] == (53, False)
    assert wl_input.US_KEYS["\n"] == (28, False)
    assert not wl_input.VirtualInput.typeable("ü")


def test_wayland_string_encoding_is_padded():
    enc = wl_input._string("wl_seat")
    assert struct.unpack("<I", enc[:4])[0] == 8 and len(enc) == 12
    enc = wl_input._string("abc")
    assert len(enc) == 8
    enc = wl_input._string("abcd")
    assert len(enc) == 12 and enc.endswith(b"\0\0\0\0")


def test_deny_list_matches_class_or_title():
    pat = server.deny_pattern()
    assert server.denied_window([{"class": "org.keepassxc.KeePassXC", "title": "db"}], pat)
    assert server.denied_window([{"class": "chromium", "title": "Bitwarden Web Vault"}], pat)
    assert server.denied_window([{"class": "chromium", "title": "YouTube"}], pat) is None
    assert server.denied_window([{}], pat) is None


def test_deny_list_can_be_disabled(monkeypatch):
    monkeypatch.setenv("HYPR_COMPUTER_DENY", "")
    assert server.deny_pattern() is None


def test_windows_at_point():
    clients = [
        {"class": "a", "at": [0, 0], "size": [800, 600], "workspace": {"id": 1, "name": "1"}},
        {"class": "b", "at": [800, 0], "size": [800, 600], "workspace": {"id": 1, "name": "1"}},
        {"class": "c", "at": [0, 0], "size": [800, 600], "workspace": {"id": 2, "name": "2"}},
    ]
    assert [c["class"] for c in server.windows_at(clients, 100, 100, {"1"})] == ["a"]
    assert [c["class"] for c in server.windows_at(clients, 900, 100, {"1"})] == ["b"]
    assert server.windows_at(clients, 2000, 100, {"1"}) == []


def test_session_env_forces_utf8(monkeypatch):
    for k in ("LC_ALL", "LC_CTYPE", "LANG"):
        monkeypatch.delenv(k, raising=False)
    assert server.session_env()["LC_ALL"] == "C.UTF-8"


def test_matching_windows_prefers_exact_class_then_recent():
    clients = [
        {"class": "foot", "title": "chromium notes", "focusHistoryID": 0},
        {"class": "chromium", "title": "YouTube", "focusHistoryID": 3},
        {"class": "chromium", "title": "Docs", "focusHistoryID": 1},
    ]
    hits = server.matching_windows(clients, "Chromium")
    assert [h["title"] for h in hits] == ["Docs", "YouTube", "chromium notes"]
    assert server.matching_windows(clients, "nothing") == []


def test_desktop_entries(tmp_path, monkeypatch):
    apps = tmp_path / "applications"
    apps.mkdir()
    (apps / "org.example.Viewer.desktop").write_text("[Desktop Entry]\nName=Image Viewer\nExec=viewer\n")
    (apps / "hidden.desktop").write_text("[Desktop Entry]\nName=Hidden\nNoDisplay=true\n")
    monkeypatch.setenv("XDG_DATA_HOME", str(tmp_path))
    monkeypatch.setenv("XDG_DATA_DIRS", str(tmp_path / "none"))
    found = server.desktop_entries()
    assert found["image viewer"] == "org.example.Viewer"
    assert found["org.example.viewer"] == "org.example.Viewer"
    assert "hidden" not in found


def test_record_is_off_by_default_and_local_when_on(tmp_path, monkeypatch):
    monkeypatch.delenv("HYPR_COMPUTER_RECORD_DIR", raising=False)
    server.record("click", {"x": 1}, "clicked", None, {})
    monkeypatch.setenv("HYPR_COMPUTER_RECORD_DIR", str(tmp_path / "rec"))
    server.record("click", {"x": 1}, "clicked", None, {})
    server.record("screenshot", {}, "shot", b"\xff\xd8jpeg", {})
    lines = (tmp_path / "rec" / "steps.jsonl").read_text().splitlines()
    assert len(lines) == 2 and '"tool": "click"' in lines[0]
    assert (tmp_path / "rec" / "0001.jpg").read_bytes() == b"\xff\xd8jpeg"


def test_recorded_screenshots_never_overwrite_each_other(tmp_path, monkeypatch):
    monkeypatch.setenv("HYPR_COMPUTER_RECORD_DIR", str(tmp_path))
    for shot in (b"one", b"two", b"three"):
        server.record("screenshot", {}, "shot", shot, {})
    (tmp_path / "0001.jpg").unlink()
    server.record("screenshot", {}, "shot", b"four", {})
    assert [p.read_bytes() for p in sorted(tmp_path.glob("*.jpg"))] == [b"two", b"three", b"four"]


def test_recording_is_private_and_holds_no_typed_text(desktop, tmp_path, monkeypatch):
    rec = tmp_path / "rec"
    monkeypatch.setenv("HYPR_COMPUTER_RECORD_DIR", str(rec))
    desktop.call("type_text", text="hunter2 is my password")
    desktop.call("batch", steps=[{"action": "type", "text": "hunter2"}, {"action": "type", "text": 5}],
                 screenshot=False)
    steps = (rec / "steps.jsonl").read_text()
    assert "hunter2" not in steps
    typed, batched = map(json.loads, steps.splitlines())
    assert typed["args"]["text_length"] == 22 and batched["args"]["steps"][0]["text_length"] == 7
    assert stat.S_IMODE(rec.stat().st_mode) == 0o700
    assert stat.S_IMODE((rec / "steps.jsonl").stat().st_mode) == 0o600


def test_every_tool_call_is_timed_in_the_recording(desktop, tmp_path, monkeypatch):
    monkeypatch.setenv("HYPR_COMPUTER_RECORD_DIR", str(tmp_path))
    desktop.call("list_windows")
    desktop.call("key", combo="shift")
    with pytest.raises(Exception):
        desktop.call("key", combo="ctrl+nope")
    lines = [json.loads(line) for line in (tmp_path / "steps.jsonl").read_text().splitlines()]
    assert [line["tool"] for line in lines] == ["list_windows", "key", "key"]
    for line in lines:
        assert {"settle_ms", "capture_ms", "ipc_ms", "total_ms"} <= line.keys(), line
        assert 0 <= line["ipc_ms"] <= line["total_ms"]
    assert lines[1]["ipc_ms"] > 0 and lines[2]["result"].startswith("error: unknown key")


def test_stop_file_blocks_every_tool(desktop):
    open(desktop.hypr.stop_file, "w").close()
    sample = {"number": 10, "integer": 1, "string": "foot", "boolean": False, "array": []}
    tools = desktop.tools()
    assert len(tools) == 20
    not_blocked = []
    for t in tools:
        args = {k: sample[v["type"]] for k, v in t.inputSchema["properties"].items()
                if k in t.inputSchema.get("required", [])}
        try:
            not_blocked.append(f"{t.name} returned {desktop.call(t.name, **args)!r}")
        except Exception as e:  # noqa: BLE001
            if "Stopped by the user" not in str(e) or "Ask the user" not in str(e):
                not_blocked.append(f"{t.name}: {e}")
    assert not not_blocked
    assert desktop.input.events == [] and not desktop.hypr.dispatched("")


KEEPASS = {"class": "org.keepassxc.KeePassXC", "title": "Passwords", "at": [600, 0], "size": [200, 800],
           "workspace": {"id": 9, "name": "9"}, "address": "0xk"}


def test_move_refuses_a_deny_listed_window_under_the_pointer(desktop):
    desktop.hypr.clients = [KEEPASS]
    with pytest.raises(Exception, match="deny-list"):
        desktop.call("move", x=700, y=400)
    assert not desktop.hypr.dispatched("cursor.move")


def test_drag_through_a_deny_listed_window_is_refused_before_pressing(desktop):
    desktop.hypr.clients = [KEEPASS]
    with pytest.raises(Exception, match="deny-list"):
        desktop.call("drag", x1=100, y1=400, x2=1200, y2=400)
    assert desktop.input.events == [] and not desktop.hypr.dispatched("cursor.move")


def test_drag_stops_when_the_user_stops_it_mid_drag(desktop):
    def stop_after_three_moves(request):
        if len(desktop.hypr.dispatched("cursor.move")) == 3:
            open(desktop.hypr.stop_file, "w").close()
    desktop.hypr.on_request = stop_after_three_moves
    with pytest.raises(Exception, match=r"Stopped by the user.*The drag stopped, and the button was released at \(200, 100\)"):
        desktop.call("drag", x1=100, y1=100, x2=600, y2=100)
    assert len(desktop.hypr.dispatched("cursor.move")) == 3
    assert desktop.input.events == [("button", "left", True), ("button", "left", False)]


def test_a_finished_action_is_not_reported_as_failed_when_reading_the_screen_fails(desktop):
    answer = desktop.hypr.reply
    desktop.hypr.reply = lambda r: "not json" if r == "j/cursorpos" else answer(r)
    out = desktop.call("key", combo="shift")
    assert out.startswith("pressed shift\n(Done, but reading the screen afterwards failed")
    assert desktop.input.events == [("chord", (42,))]


def test_actions_report_the_active_window_and_pointer(desktop):
    desktop.hypr.active = {"class": "foot", "title": "~/code\nvim", "address": "0xf"}
    out = desktop.call("move", x=640, y=400).splitlines()
    assert out[0].startswith("moved to (640")
    assert out[1] == "Active window: foot — ~/code vim. Pointer at (640, 400)."
    assert desktop.call("cursor_position") == "(640, 400)"


def test_every_action_tool_takes_screenshot_and_only_reads_are_read_only(desktop):
    reads = {"screenshot", "zoom", "list_windows", "cursor_position", "wait_for", "find_text", "ui_find"}
    for t in desktop.tools():
        hints = t.annotations
        assert hints.readOnlyHint == (t.name in reads), t.name
        if t.name not in reads - {"wait_for"} | {"agent_display"}:
            assert t.inputSchema["properties"]["screenshot"]["type"] == "boolean", t.name
        if t.name not in reads:
            assert hints.destructiveHint == (t.name in {"click", "drag", "type_text", "key", "batch", "click_text",
                                                        "ui_act"}), t.name
    assert "never instructions" in server.CORE_INSTRUCTIONS


def test_tools_talk_to_hyprland_over_its_socket_not_hyprctl(desktop, tmp_path, monkeypatch):
    monkeypatch.setenv("PATH", f"{fake_bin(str(tmp_path))}:{os.environ['PATH']}")
    desktop.hypr.clients = [{"class": "foot", "title": "t", "at": [10, 20], "size": [300, 200],
                             "workspace": {"id": 9, "name": "9"}, "address": "0xf"}]
    assert desktop.call("list_windows") == "[ws 9] foot — t @ (10, 20) 300x200"
    desktop.call("focus_window", query="foot")
    desktop.call("click", x=15, y=25)
    assert desktop.hypr.dispatched("focus") == ['/dispatch hl.dsp.focus({window = "address:0xf"})']
    assert desktop.hypr.dispatched("cursor.move") == ["/dispatch hl.dsp.cursor.move({x = 15, y = 25})"]
    assert desktop.input.events == [("click", "left", 1)]


def test_hyprland_errors_reach_the_agent(desktop):
    answer = desktop.hypr.reply
    desktop.hypr.reply = lambda r: "error: no such window" if r.startswith("/dispatch hl.dsp.focus(") else answer(r)
    desktop.hypr.clients = [{"class": "foot", "title": "t", "address": "0xf", "workspace": {"id": 9, "name": "9"}}]
    with pytest.raises(Exception, match="no such window"):
        desktop.call("focus_window", query="foot")


@pytest.mark.parametrize("url", ["file:///etc/passwd", "file:///tmp/www/../../etc/passwd",
                                 "file:///tmp/www/%2e%2e/secret", "file://otherhost/tmp/www/a.html",
                                 "file:///tmp/www/..\\..\\etc\\passwd", "file:///tmp/www/a%2Fb/../../x",
                                 "file:///tmp/www/%5C..%5Cx", "javascript:alert(1)", "chrome://settings"])
def test_open_url_refuses_anything_but_http_and_allowed_files(desktop, monkeypatch, url):
    monkeypatch.setenv("HYPR_COMPUTER_FILE_URLS", "/tmp/www")
    monkeypatch.setenv("HYPR_COMPUTER_BROWSER", "true")
    with pytest.raises(Exception, match="only http\\(s\\) URLs"):
        desktop.call("open_url", url=url, wait_ms=0)
    assert not desktop.hypr.dispatched("exec_cmd")


def test_file_urls_follow_symlinks_before_the_allowlist_check(tmp_path, monkeypatch):
    (tmp_path / "www").mkdir()
    (tmp_path / "www" / "page.html").write_text("hi")
    (tmp_path / "www" / "escape").symlink_to("/etc")
    monkeypatch.setenv("HYPR_COMPUTER_FILE_URLS", f"/nonexistent:{tmp_path / 'www'}")
    assert server.checked_url(f"file://{tmp_path}/www/page.html") == f"file://{tmp_path}/www/page.html"
    with pytest.raises(ValueError):
        server.checked_url(f"file://{tmp_path}/www/escape/passwd")
    monkeypatch.delenv("HYPR_COMPUTER_FILE_URLS")
    with pytest.raises(ValueError):
        server.checked_url(f"file://{tmp_path}/www/page.html")
    assert server.checked_url("example.com") == "https://example.com"


def test_binaries_are_resolved_once_so_a_later_path_change_cannot_swap_them(tmp_path, monkeypatch):
    for d, answer in (("a", "first.desktop"), ("b", "second.desktop")):
        (tmp_path / d).mkdir()
        (tmp_path / d / "xdg-mime").write_text(f"#!/bin/sh\necho {answer}\n")
        (tmp_path / d / "xdg-mime").chmod(0o755)
    monkeypatch.setenv("PATH", f"{tmp_path / 'a'}:{os.environ['PATH']}")
    assert server.run("xdg-mime").decode().strip() == "first.desktop"
    monkeypatch.setenv("PATH", f"{tmp_path / 'b'}:{os.environ['PATH']}")
    assert server.run("xdg-mime").decode().strip() == "first.desktop"


def test_browser_command_override_adds_private_flag(monkeypatch):
    monkeypatch.setenv("HYPR_COMPUTER_BROWSER", "chromium --user-data-dir=/tmp/x")
    assert server.browser_command(False) == (["chromium", "--user-data-dir=/tmp/x"], ("chromium",))
    assert server.browser_command(True)[0][-1] == "--incognito"
    monkeypatch.setenv("HYPR_COMPUTER_BROWSER", "/usr/bin/firefox")
    assert server.browser_command(True) == (["/usr/bin/firefox", "--private-window"], ("firefox",))


def test_page_ready_ignores_blank_and_loading_tabs():
    before = ("0xa", "Docs - Chromium")
    assert not server.page_ready(before, before)
    assert not server.page_ready(before, ("0xb", "New Tab - Chromium"))
    assert not server.page_ready(before, ("0xb", "New Incognito Tab - Chromium"))
    assert not server.page_ready(before, ("", ""))
    assert server.page_ready(before, ("0xb", "Example Domain - Chromium"))
    assert server.page_ready(before, ("0xa", "Daily News - Chromium"))


def test_update_note_once_when_disk_version_changes(tmp_path, monkeypatch):
    pj = tmp_path / "plugin.json"
    pj.write_text('{"version": "1.0.0"}')
    monkeypatch.setattr(server, "PLUGIN_JSON", str(pj))
    monkeypatch.setattr(server, "RUNNING_VERSION", "1.0.0")
    monkeypatch.setattr(server, "_update_noted", False)
    assert server.update_note() == ""
    pj.write_text('{"version": "1.1.0"}')
    note = server.update_note()
    assert "1.1.0 is installed" in note and "restart" in note
    assert server.update_note() == ""


def test_lua_literals_survive_quotes_backslashes_and_newlines():
    assert server.lua_literal({"x": 12, "y": 3.5}) == "{x = 12, y = 3.5}"
    assert server.lua_literal('say "hi" \\ there\n') == r'"say \034hi\034 \092 there\010"'
    assert server.lua_literal("café") == '"café"'
    with pytest.raises(TypeError):
        server.lua_literal({"not an identifier": 1})


def test_every_hyprland_dispatch_goes_through_dispatch():
    import ast
    tree = ast.parse(open(server.__file__).read())
    raw = [node.lineno for fn in ast.walk(tree) if isinstance(fn, ast.FunctionDef) and fn.name != "dispatch"
           for node in ast.walk(fn) if isinstance(node, ast.Constant) and isinstance(node.value, str)
           and ("hl.dsp" in node.value or node.value.startswith(("dispatch", "/dispatch")))]
    assert not raw, (f"server.py lines {raw} build a Hyprland dispatch by hand; call "
                     "dispatch('name', {...}) so the Lua syntax is generated")


def test_tests_cannot_reach_the_hyprland_of_the_machine_they_run_on():
    with pytest.raises(RuntimeError, match="can't reach Hyprland"):
        server.hypr_json("monitors")


def test_batch_stops_at_the_first_failing_step(desktop):
    log, ok = server.run_steps([{"action": "wait", "ms": 0}, {"action": "key", "combo": "ctrl+nope"},
                                {"action": "wait", "ms": 0}])
    assert not ok and len(log) == 2 and log[0] == "step 1: waited 0 ms"
    assert "step 2 (key) failed" in log[1]
    log, ok = server.run_steps([{"action": "zzz"}])
    assert not ok and "unknown action" in log[0]


def test_private_window_uses_the_default_browsers_own_private_action(tmp_path, monkeypatch):
    apps = tmp_path / "applications"
    apps.mkdir()
    (apps / "mybrowser.desktop").write_text(
        "[Desktop Entry]\nName=My Browser\nExec=mybrowser %U\n\n[Desktop Action new-private-window]\n"
        "Name=New Private Window\nExec=mybrowser --secret-mode\n")
    xdg_mime = tmp_path / "bin" / "xdg-mime"
    xdg_mime.parent.mkdir()
    xdg_mime.write_text("#!/bin/sh\necho mybrowser.desktop\n")
    xdg_mime.chmod(0o755)
    monkeypatch.setenv("PATH", f"{xdg_mime.parent}:{os.environ['PATH']}")
    monkeypatch.setenv("XDG_DATA_HOME", str(tmp_path))
    monkeypatch.delenv("HYPR_COMPUTER_BROWSER", raising=False)
    assert server.browser_command(False) == (["mybrowser"], ("mybrowser",))
    assert server.browser_command(True) == (["mybrowser", "--secret-mode"], ("mybrowser",))


def test_browser_command_unwraps_omarchy_browser_wrapper(tmp_path, monkeypatch):
    apps = tmp_path / "applications"
    apps.mkdir()
    (apps / "omarchy-browser.desktop").write_text(
        "[Desktop Entry]\nName=Omarchy Browser\nExec=omarchy-launch-browser %U\n")
    (apps / "chromium.desktop").write_text(
        "[Desktop Entry]\nName=Chromium\nExec=/usr/bin/chromium %U\n\n"
        "[Desktop Action new-private-window]\nName=New Incognito Window\nExec=/usr/bin/chromium --incognito\n")
    bindir = tmp_path / "bin"
    bindir.mkdir()
    for name, out in (("xdg-mime", "omarchy-browser.desktop"), ("omarchy-cmd-default-browser", "chromium.desktop")):
        script = bindir / name
        script.write_text(f"#!/bin/sh\necho {out}\n")
        script.chmod(0o755)
    monkeypatch.setenv("PATH", f"{bindir}:{os.environ['PATH']}")
    monkeypatch.setenv("XDG_DATA_HOME", str(tmp_path))
    monkeypatch.delenv("HYPR_COMPUTER_BROWSER", raising=False)
    assert server.browser_command(False) == (["/usr/bin/chromium"], ("chromium",))
    assert server.browser_command(True) == (["/usr/bin/chromium", "--incognito"], ("chromium",))
