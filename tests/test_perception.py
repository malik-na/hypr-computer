import base64
import io
import json
import os
import re
import sys
import threading
import time

import pytest
from PIL import Image

import server
from fakes import fake_bin

HERE = os.path.dirname(os.path.abspath(__file__))
WS9, WS3 = {"id": 9, "name": "9"}, {"id": 3, "name": "3"}
A11Y_ON = "env ACCESSIBILITY_ENABLED=1 QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1"


def client(cls: str, title: str, address: str, at, size, workspace=WS9, pid=100, **extra) -> dict:
    return {"class": cls, "title": title, "address": address, "pid": pid, "at": list(at), "size": list(size),
            "workspace": workspace, "monitor": 0, "floating": False, "focusHistoryID": 1, "hidden": False, **extra}


# The screen fixture is the M1 panel, 1280x800 logical at scale 2, so logical and screenshot pixels match.
FOOT = client("foot", "~", "0xf", (250, 150), (500, 300), focusHistoryID=0)
KEEPASS = client("org.keepassxc.KeePassXC", "Bank logins - KeePassXC", "0xk", (250, 150), (100, 100))
SQUARE = "600,400,80"  # a white square at (300, 200) logical, 40 px on a side: inside FOOT and KEEPASS


def gray(jpeg: str, *points) -> list[int]:
    im = Image.open(io.BytesIO(base64.b64decode(jpeg))).convert("L")
    return [im.getpixel(p) for p in points]


def later(delay: float, fn) -> None:
    threading.Timer(delay, fn).start()


@pytest.fixture
def ocr(screen, tmp_path, monkeypatch):
    monkeypatch.setenv("FAKE_OCR_RUNS", str(tmp_path / "ocr-runs"))
    monkeypatch.setenv("FAKE_GRIM_MARK", SQUARE)
    screen.hypr.clients = [FOOT]
    screen.reads = lambda: len((tmp_path / "ocr-runs").read_text().splitlines())
    return screen


def test_find_text_says_where_text_is_in_screenshot_pixels_and_in_which_window(ocr, monkeypatch):
    monkeypatch.setenv("FAKE_OCR_TEXT", "Save draft")
    for window in ("", "foot"):
        first, *rows = ocr.call("find_text", query="DRAFT", window=window).splitlines()
        assert "untrusted" in first
        assert rows == ['1. "draft" at (330, 220), box (320, 200, 20x40) in foot: "Save draft"'], window
    monkeypatch.setattr(server, "MAX_EDGE", 640)  # screenshots at half the desktop's size
    assert ocr.call("find_text", query="draft").splitlines()[1].startswith('1. "draft" at (165, 110), box (160, 100, 10x20)')
    ocr.hypr.clients = [FOOT, client("chromium", "Docs", "0xc", (0, 0), (100, 100), workspace=WS3)]
    with pytest.raises(Exception, match="not on screen"):
        ocr.call("find_text", query="draft", window="chromium")


def test_click_text_clicks_the_nth_match_counting_top_to_bottom(ocr, monkeypatch):
    monkeypatch.setenv("FAKE_GRIM_MARK", "600,800,40;200,200,40")  # 20 px squares at (300, 400) and (100, 100)
    monkeypatch.setenv("FAKE_OCR_TEXT", "Apply")
    assert ocr.call("click_text", query="apply", nth=2).startswith('clicked "Apply" at (310, 410)')
    ocr.call("batch", steps=[{"action": "click_text", "query": "Apply"}], screenshot=False)
    assert ocr.hypr.dispatched("cursor.move") == ["/dispatch hl.dsp.cursor.move({x = 310, y = 410})",
                                                  "/dispatch hl.dsp.cursor.move({x = 110, y = 110})"]
    assert ocr.input.events == [("click", "left", 1)] * 2
    with pytest.raises(Exception, match="found 2 match"):
        ocr.call("click_text", query="apply", nth=3)


def test_the_same_frame_is_read_only_once(ocr, monkeypatch):
    ocr.call("find_text", query="submit")
    ocr.call("click_text", query="submit")
    assert ocr.reads() == 1
    monkeypatch.setenv("FAKE_GRIM_MARK", "1000,1000,40")
    assert '"Submit" at (510, 510)' in ocr.call("find_text", query="submit")
    assert ocr.reads() == 2


def test_wait_for_text_returns_once_the_text_is_on_screen(ocr, monkeypatch):
    monkeypatch.setenv("FAKE_GRIM_MARK", "")
    later(0.5, lambda: os.environ.__setitem__("FAKE_GRIM_MARK", SQUARE))
    out = ocr.call("wait_for", event="text", match="sub.it", timeout_ms=5000, screenshot=False)
    assert re.match(r'"Submit" is on screen after (\d+) ms', out) and int(out.split()[5]) >= 400, out
    monkeypatch.setenv("FAKE_GRIM_MARK", "")
    assert ocr.call("wait_for", event="text", match="submit", timeout_ms=300, screenshot=False).startswith("timed out")
    with pytest.raises(Exception, match="needs match"):
        ocr.call("wait_for", event="text", screenshot=False)


def test_without_tesseract_text_tools_say_how_to_install_it_and_the_check_calls_it_optional(
        screen, tmp_path, monkeypatch, capsys):
    monkeypatch.setenv("PATH", fake_bin(str(tmp_path / "bare"), tesseract=False))
    with pytest.raises(Exception, match="pacman -S tesseract"):
        screen.call("find_text", query="submit")
    server.self_check()
    (line,) = [x for x in capsys.readouterr().out.splitlines() if "  tesseract (" in x]
    assert line.startswith("PASS  tesseract (optional") and "not installed" in line and "pacman -S tesseract" in line
    monkeypatch.setenv("PATH", fake_bin(str(tmp_path / "full")))
    server.binary.cache_clear()
    server.self_check()
    (line,) = [x for x in capsys.readouterr().out.splitlines() if "  tesseract (" in x]
    assert line == "PASS  tesseract (optional, reading text on screen): tesseract 5.5.4 (fake)"


@pytest.fixture
def vault(ocr):
    ocr.hypr.clients = [FOOT, KEEPASS]
    return ocr


def test_a_deny_listed_window_is_black_in_every_image_and_unreadable(vault, monkeypatch):
    inside, margin, outside = (320, 220), (247, 147), (240, 140)
    images = []
    for tool, args in (("screenshot", {}), ("click", {"x": 900, "y": 600, "screenshot": True}),
                       ("batch", {"steps": [{"action": "move", "x": 900, "y": 650}]})):
        monkeypatch.setattr(server, "_shown", None)
        text, (image,) = vault.result(tool, **args)
        assert "Black areas hide deny-listed windows" in text, tool
        images.append(image)
    for image in images:
        black, border, background = gray(image, inside, margin, outside)
        assert black < 16 and border < 16 and 20 < background < 45, (black, border, background)
    text, (zoomed,) = vault.result("zoom", x=240, y=140, width=120, height=120)
    assert gray(zoomed, (100, 100))[0] < 16 and 20 < gray(zoomed, (4, 4))[0] < 45
    assert vault.call("find_text", query="submit") == ("No text matching 'submit' on screen. "
                                                       "Black areas hide deny-listed windows (password managers).")

    monkeypatch.setenv("HYPR_COMPUTER_DENY", "")
    monkeypatch.setattr(server, "_shown", None)
    assert gray(vault.result("screenshot")[1][0], inside)[0] > 200
    assert '"Submit" at (320, 220)' in vault.call("find_text", query="submit")


def test_deny_listed_titles_never_reach_the_agent(desktop):
    desktop.hypr.clients = [FOOT, KEEPASS]
    desktop.hypr.active = KEEPASS
    listed = desktop.call("list_windows")
    assert "org.keepassxc.KeePassXC — (title hidden" in listed and "Bank" not in listed
    later(0.1, lambda: desktop.hypr.emit("openwindow>>a11ce,9,pinentry-qt,Unlock key for alice@example.org"))
    out = desktop.call("wait_for", event="window", match="alice", timeout_ms=400, screenshot=False)
    assert out.startswith("timed out") and "alice@" not in out
    assert "Events: opened pinentry-qt (deny-listed)." in out
    assert "Active window: org.keepassxc.KeePassXC — (title hidden: deny-listed)." in out
    out = desktop.call("wait_for", event="title", match="bank", timeout_ms=100, screenshot=False)
    assert out.startswith("timed out")
    with pytest.raises(Exception, match="no window matches"):
        desktop.call("focus_window", query="bank")


def test_a_click_on_a_deny_listed_window_in_a_shown_special_workspace_is_refused(desktop):
    desktop.hypr.monitors = [{**desktop.hypr.monitors[0], "specialWorkspace": {"id": -98, "name": "special:magic"}}]
    desktop.hypr.clients = [{**KEEPASS, "workspace": {"id": -98, "name": "special:magic"}}]
    with pytest.raises(Exception, match="deny-list"):
        desktop.call("click", x=300, y=200)
    assert desktop.input.events == []


def test_a_deny_listed_window_on_a_shown_scratchpad_is_black_though_hyprland_gives_it_no_id(screen, monkeypatch):
    monkeypatch.setenv("FAKE_GRIM_MARK", SQUARE)
    scratchpad = {"address": "special:scratchpad", "type": "special", "name": "special:scratchpad"}  # as in 0.57
    screen.hypr.monitors = [{**screen.hypr.monitors[0], "specialWorkspace": scratchpad}]
    screen.hypr.clients = [FOOT, {**KEEPASS, "workspace": scratchpad}]
    text, (image,) = screen.result("screenshot")
    assert gray(image, (320, 220))[0] < 16 and "Black areas hide deny-listed windows" in text


def test_capture_is_refused_when_a_deny_listed_window_on_screen_has_no_position(screen):
    screen.hypr.clients = [{**KEEPASS, "size": [0, 0]}]
    for tool, args in (("screenshot", {}), ("zoom", {"x": 0, "y": 0, "width": 10, "height": 10}),
                       ("find_text", {"query": "submit"})):
        with pytest.raises(Exception, match="Refused to capture"):
            screen.call(tool, **args)
    screen.hypr.clients = [{**KEEPASS, "size": [0, 0], "workspace": WS3}]
    assert screen.result("screenshot")[1]


def test_while_windows_move_with_a_deny_listed_window_open_the_screen_is_blacked_out(screen, monkeypatch):
    monkeypatch.setenv("FAKE_GRIM_MARK", SQUARE)
    screen.hypr.clients = [{**KEEPASS, "workspace": WS3}]  # off screen, but a workspace switch slides it past
    screen.hypr.animations = [[{"name": "global", "overridden": True, "enabled": True, "speed": 30.0}], []]
    screen.hypr.emit("workspace>>9")
    text, (image,) = screen.result("screenshot")
    assert max(gray(image, (320, 220), (10, 10), (1270, 790))) < 16 and "blacked out" in text
    screen.hypr.animations[0][0]["speed"] = 2.0  # 200 ms, which the capture waits out
    screen.hypr.emit("workspace>>9")
    monkeypatch.setattr(server, "_shown", None)
    text, (image,) = screen.result("screenshot")
    assert gray(image, (320, 220))[0] > 200 and "Black" not in text


def blacked_out(screen) -> str:
    """The note of a screenshot that is black all over."""
    server._shown = None
    text, (image,) = screen.result("screenshot")
    assert max(gray(image, (320, 220), (10, 10), (1270, 790), (640, 400))) < 16, text
    return text


SLOW = [[{"name": "global", "overridden": True, "enabled": True, "speed": 11.5}], []]  # 1.15 s, past the wait


def test_the_screen_goes_black_when_a_deny_listed_window_has_focus_or_is_grouped(vault):
    vault.hypr.active = KEEPASS
    assert "has focus" in blacked_out(vault)
    vault.hypr.active = FOOT
    vault.hypr.clients = [FOOT, {**KEEPASS, "grouped": ["0xk", "0xf"]}]
    assert "window group" in blacked_out(vault)


def test_the_screen_goes_black_when_a_deny_listed_window_moves_during_the_capture(vault, tmp_path, monkeypatch):
    def once_captured(change):
        def on_request(request):
            if (tmp_path / "shots").exists() and not (tmp_path / "changed").exists():
                (tmp_path / "changed").touch()
                change()
        return on_request
    monkeypatch.setenv("FAKE_GRIM_MOVING", f"{tmp_path / 'shots'},0")  # counts captures
    vault.hypr.on_request = once_captured(lambda: setattr(vault.hypr, "clients", [FOOT, {**KEEPASS, "at": [700, 500]}]))
    assert "moving" in blacked_out(vault)

    def switch_to_its_workspace():  # it slides into view
        vault.hypr.monitors = [{**vault.hypr.monitors[0], "activeWorkspace": WS3}]
        vault.hypr.emit("workspace>>3")
    for path in ("shots", "changed"):
        (tmp_path / path).unlink()
    vault.hypr.clients = [FOOT, {**KEEPASS, "workspace": WS3}]
    vault.hypr.on_request = once_captured(switch_to_its_workspace)
    assert "moving" in blacked_out(vault)


def test_the_screen_goes_black_when_a_deny_listed_window_is_retiled_or_closed_without_time_to_finish(vault):
    vault.result("screenshot")
    vault.hypr.animations = SLOW
    time.sleep(1.3)  # long after anything moved
    vault.hypr.clients = [FOOT, {**KEEPASS, "at": [700, 500]}]  # Hyprland re-tiles it and sends no event
    assert "moving" in blacked_out(vault)
    time.sleep(1.3)
    vault.hypr.clients = [FOOT]
    vault.hypr.emit("closewindow>>k")  # it fades out where it was
    assert "moving" in blacked_out(vault)


def test_the_screen_goes_black_while_a_deny_listed_window_that_was_never_captured_fades_out(screen, monkeypatch):
    monkeypatch.setenv("FAKE_GRIM_MARK", SQUARE)
    screen.hypr.animations = SLOW
    screen.hypr.emit("openwindow>>9e11,9,pinentry-gnome3,Unlock OpenPGP key", "closewindow>>9e11")
    assert "moving" in blacked_out(screen)


CHROMIUM = client("chromium", "Probe Page - Chromium", "0xc", (647, 38), (621, 750), pid=4242)


def element(role: str, name: str, extents=None, actions=(), states=("enabled", "showing"), children=(), **extra):
    e = {"role": role, "name": name, "states": list(states), "actions": list(actions), "children": list(children),
         **extra}
    return e if extents is None else {**e, "extents": list(extents)}


def chromium_page(*page, toolbar=()) -> dict:
    """Chromium as AT-SPI shows it on a scale-2 monitor: its window and toolbar in logical pixels, the page
    in device pixels."""
    doc = element("document web", "Probe Page", (0, 174, 1242, 1326), children=page)
    bar = element("tool bar", "", (0, 40, 621, 46), children=toolbar)
    frame = element("frame", "Probe Page - Chromium", (0, 0, 621, 750), states=("active", "enabled"),
                    children=[element("panel", "", (0, 0, 621, 750), children=[bar, doc])])
    return {"children": [element("application", "Chromium", (0, 0, 0, 0), pid=4242, children=[frame])]}


APPLY = element("button", "Apply changes", (80, 470, 284, 76), ["press", "showContextMenu"])  # centre (758, 292)
REMEMBER = element("check box", "Remember me", (384, 494, 28, 28), ["check"], toggles={"check": ["checked"]})
FAR = element("link", "Apply elsewhere", (80, 3000, 200, 40), ["jump"])  # below the window's bottom edge
RELOAD = element("push button", "Reload", (40, 46, 34, 34), ["press"])  # centre (704, 101)


@pytest.fixture
def atspi(desktop, tmp_path, monkeypatch):
    tree = tmp_path / "atspi.json"
    monkeypatch.setattr(server, "A11Y_PYTHON", sys.executable)
    monkeypatch.setenv("PYTHONPATH", os.path.join(HERE, "fake_gi"))
    monkeypatch.setenv("FAKE_ATSPI", str(tree))
    desktop.hypr.clients = [CHROMIUM]
    desktop.hypr.active = CHROMIUM
    desktop.tree = lambda t=None: json.loads(tree.read_text()) if t is None else tree.write_text(json.dumps(t))
    return desktop


def test_ui_find_lists_matching_elements_with_refs_and_screen_positions(atspi, monkeypatch):
    atspi.tree(chromium_page(APPLY, REMEMBER, FAR, toolbar=[RELOAD]))
    first, *rows = atspi.call("ui_find", query="apply").splitlines()
    assert "untrusted" in first
    assert rows == ['e1 button "Apply changes" at (758, 292)',
                    'e2 link "Apply elsewhere" (not on screen, or its position is unknown)']
    rows = atspi.call("ui_find", role="button").splitlines()[1:]
    assert rows == ['e3 push button "Reload" at (704, 101)', 'e4 button "Apply changes" at (758, 292)']
    monkeypatch.setattr(server, "MAX_EDGE", 640)  # screenshots at half the desktop's size
    assert atspi.call("ui_find", query="apply changes").splitlines()[1] == 'e5 button "Apply changes" at (379, 146)'


def test_ui_find_leaves_out_positions_it_cannot_trust(atspi):
    for frame_extents, button_extents in (((0, 0, 0, 0), (0, 0, 0, 0)), ((0, 0, 300, 200), (10, 10, 50, 20))):
        button = element("push button", "Apply", button_extents, ["press"])
        frame = element("frame", "Probe Page - Chromium", frame_extents, children=[button])
        atspi.tree({"children": [element("application", "App", pid=4242, children=[frame])]})
        assert atspi.call("ui_find", query="apply").splitlines()[1].endswith(
            '"Apply" (not on screen, or its position is unknown)'), frame_extents


def test_ui_find_disambiguates_incognito_and_helper_frames(atspi):
    button = element("push button", "Reload", (40, 46, 34, 34), ["press"])
    helper1 = element("frame", "", (0, 0, 500, 88), states=("enabled", "showing"))
    helper2 = element("frame", "", (0, 0, 500, 89), states=("enabled", "showing"))
    # Chromium Incognito on the agent display: AT-SPI appends "(Incognito)", window is not active,
    # and two nameless helper frames sit alongside it.
    incognito = element("frame", "Probe Page - Chromium (Incognito)", (0, 0, 621, 750),
                        states=("enabled", "showing"), children=[button])
    atspi.tree({"children": [element("application", "Chromium", pid=4242, children=[incognito, helper1, helper2])]})
    assert atspi.call("ui_find", query="reload").splitlines()[1] == 'e1 push button "Reload" at (704, 101)'

    # Unrelated title, but only one frame matches the window's size.
    renamed = element("frame", "Different Title", (0, 0, 621, 750), states=("enabled", "showing"), children=[button])
    atspi.tree({"children": [element("application", "Chromium", pid=4242, children=[renamed, helper1])]})
    assert atspi.call("ui_find", query="reload").splitlines()[1] == 'e2 push button "Reload" at (704, 101)'

    # Two same-size frames with unrelated titles remain ambiguous.
    other = element("frame", "Other Window", (0, 0, 621, 750), states=("enabled", "showing"))
    atspi.tree({"children": [element("application", "Chromium", pid=4242, children=[renamed, other])]})
    with pytest.raises(Exception, match="can't tell which"):
        atspi.call("ui_find", query="reload")


def test_ui_act_finds_the_element_again_and_does_its_own_action(atspi):
    atspi.tree(chromium_page(APPLY, REMEMBER))
    atspi.call("ui_find", query="apply")
    atspi.call("ui_find", query="remember")
    tree = chromium_page(element("paragraph", "Saved!"), APPLY, REMEMBER)  # the page changed above them
    atspi.tree(tree)
    assert atspi.call("ui_act", ref="e1").startswith('Did "press" on e1 button at (758, 292)')
    assert atspi.call("ui_act", ref="e2").startswith('Did "check" on e2 check box [checked] at (846, 292)')
    assert atspi.tree()["log"] == [["Apply changes", "press"], ["Remember me", "check"]]
    assert atspi.input.events == []
    with pytest.raises(Exception, match="unknown ref"):
        atspi.call("ui_act", ref="e9")


def test_ui_act_clicks_an_element_without_an_action_of_its_own(atspi):
    atspi.tree(chromium_page(element("label", "Drop files here", (80, 470, 284, 76))))
    atspi.call("ui_find", query="drop")
    atspi.call("ui_act", ref="e1")
    assert atspi.hypr.dispatched("cursor.move") == ["/dispatch hl.dsp.cursor.move({x = 758, y = 292})"]
    assert atspi.input.events == [("click", "left", 1)]


def test_ui_find_gives_no_positions_in_a_window_that_is_not_on_screen(atspi):
    atspi.hypr.clients = [{**CHROMIUM, "workspace": WS3}]
    atspi.tree(chromium_page(APPLY, element("label", "Drop files here", (80, 600, 284, 76))))
    header, *rows = atspi.call("ui_find", role="button", window="chromium").splitlines()
    assert "not on screen" in header and rows == ['e1 button "Apply changes" (not on screen, or its position is unknown)']
    atspi.call("ui_find", query="drop", window="chromium")
    with pytest.raises(Exception, match="focus_window"):
        atspi.call("ui_act", ref="e2")
    assert atspi.input.events == [] and not atspi.hypr.dispatched("cursor.move")


def test_ui_find_reads_the_window_it_was_asked_for_and_skips_deny_listed_ones(atspi):
    vault = element("frame", "Vault | Bitwarden - Chromium", (0, 0, 621, 750), states=("active", "enabled"),
                    children=[element("button", "Unlock", (10, 10, 50, 20), ["press"])])
    tree = chromium_page(APPLY)
    tree["children"][0]["children"].insert(0, vault)
    atspi.tree(tree)
    assert atspi.call("ui_find", query="unlock").startswith("No element")
    atspi.hypr.clients = [{**CHROMIUM, "title": "Probe Page (renamed) - Chromium"}]  # no frame has this name
    atspi.hypr.active = atspi.hypr.clients[0]
    assert atspi.call("ui_find", query="unlock").startswith("No element")
    assert 'button "Apply changes"' in atspi.call("ui_find", query="apply")


def test_ui_find_reads_the_agent_displays_window_by_default_not_the_users(atspi, monkeypatch):
    monkeypatch.delenv("HYPR_COMPUTER_WORKSPACE")  # the agent works on its own display
    with pytest.raises(Exception, match="no window on the agent display yet"):
        atspi.call("ui_find", query="apply")
    atspi.tree(chromium_page(APPLY))
    assert 'button "Apply changes"' in atspi.call("ui_find", query="apply", window="chromium")


def test_ui_find_never_reads_deny_listed_windows_or_elements(atspi):
    atspi.hypr.clients = [CHROMIUM, KEEPASS]
    atspi.tree(chromium_page(APPLY, toolbar=[element("page tab", "Bitwarden Web Vault", (0, 0, 200, 40))]))
    with pytest.raises(Exception, match="deny-list"):
        atspi.call("ui_find", query="a", window="keepass")
    out = atspi.call("ui_find", query="vault")
    assert out.startswith("No element") and "Bitwarden" not in out


def test_ui_find_stops_at_its_node_budget(atspi, monkeypatch):
    monkeypatch.setattr(server, "A11Y_NODES", 50)
    atspi.tree(chromium_page(*[element("paragraph", f"row {i}") for i in range(100)], APPLY))
    assert "Stopped after 50 elements" in atspi.call("ui_find", query="apply")


def test_ui_find_says_what_it_needs_without_at_spi(atspi, tmp_path, monkeypatch):
    atspi.tree(chromium_page(APPLY))
    (tmp_path / "no_gi" / "gi").mkdir(parents=True)
    (tmp_path / "no_gi" / "gi" / "__init__.py").write_text("raise ImportError(\"No module named 'gi'\")\n")
    monkeypatch.setenv("PYTHONPATH", str(tmp_path / "no_gi"))
    with pytest.raises(Exception, match="python-gobject"):
        atspi.call("ui_find", query="apply")
    monkeypatch.setattr(server, "A11Y_PYTHON", "/nonexistent/python3")
    with pytest.raises(Exception, match="python-gobject"):
        atspi.call("ui_find", query="apply")


def test_open_url_starts_the_browser_with_accessibility_on(desktop, monkeypatch):
    for browser in ("chromium --user-data-dir=/tmp/x", "firefox"):
        monkeypatch.setenv("HYPR_COMPUTER_BROWSER", browser)
        desktop.call("open_url", url="https://example.com", wait_ms=0, screenshot=False)
    chromium, firefox = desktop.hypr.dispatched("exec_cmd")
    assert chromium.endswith(f'("{A11Y_ON} chromium --force-renderer-accessibility --user-data-dir=/tmp/x '
                             'https://example.com")')
    assert firefox.endswith(f'("{A11Y_ON} firefox https://example.com")')


def test_launch_app_starts_apps_with_accessibility_on(desktop, tmp_path, monkeypatch):
    apps = tmp_path / "applications"
    apps.mkdir()
    (apps / "foot.desktop").write_text("[Desktop Entry]\nName=Foot\nExec=foot\n")
    (apps / "chromium.desktop").write_text("[Desktop Entry]\nName=Chromium\nExec=/usr/bin/chromium %U\n")
    monkeypatch.setenv("XDG_DATA_HOME", str(tmp_path))
    monkeypatch.setenv("XDG_DATA_DIRS", str(tmp_path / "none"))
    gtk_launch = tmp_path / "bin" / "gtk-launch"
    gtk_launch.parent.mkdir()
    gtk_launch.write_text("#!/bin/sh\n")
    gtk_launch.chmod(0o755)
    monkeypatch.setenv("PATH", f"{gtk_launch.parent}:{os.environ['PATH']}")
    desktop.call("launch_app", name="foot")
    desktop.call("launch_app", name="Chromium")
    foot, chromium = desktop.hypr.dispatched("exec_cmd")
    assert foot.endswith(f'("{A11Y_ON} gtk-launch foot")')
    assert chromium.endswith(f'("{A11Y_ON} /usr/bin/chromium --force-renderer-accessibility")')
