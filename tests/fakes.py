import json
import os
import re
import shlex
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time

M1_PANEL = {"id": 0, "name": "eDP-1", "x": 0, "y": 0, "width": 2560, "height": 1600, "scale": 2.0,
            "transform": 0, "activeWorkspace": {"id": 9, "name": "9"}}


LUA_STRING = r'"(?:[^"\\]|\\.)*"'


def lua_value(v: str):
    if v.startswith('"'):
        return re.sub(r"\\(\d{3})", lambda m: chr(int(m[1])), v[1:-1])
    return {"true": True, "false": False}[v] if v in ("true", "false") else json.loads(v)


def lua_table(text: str) -> dict:
    """The fields of a flat Lua table literal, such as `{output = "x", scale = 1, default = true}`."""
    nested = {k: lua_table(v) for k, v in re.findall(r"(\w+) = (\{[^{}]*\})", text)}
    flat = re.sub(r"\w+ = \{[^{}]*\}", "", text[1:-1] if text.startswith("{") else text)
    return {**{k: lua_value(v) for k, v in re.findall(rf'(\w+) = ({LUA_STRING}|[^,}}\s]+)', flat)}, **nested}


def named_workspace(name: str) -> dict:
    """A named or special workspace as Hyprland 0.57 describes it: with no id."""
    return {"address": name, "type": "special" if name.startswith("special:") else "normal", "name": name}


class FakeHyprland:
    """Hyprland 0.57's IPC socket: one request per connection, written as `flags/command`
    (`j/clients`, `/dispatch hl.dsp...`), answered with JSON, `ok` or `error: ...`.
    Its event socket sends `name>>data` lines to every reader, with addresses lacking the `0x`
    that `j/clients` gives them, as 0.57.2 does. With `warps`, focusing a window moves the pointer to its
    middle, as Hyprland does unless `cursor:no_warps` is set.

    Headless outputs come and go as measured on the M1: one takes the size and place of its `hl.monitor` rule
    and shows the workspace its `hl.workspace_rule` makes the default; removing it leaves its windows on the
    first monitor and the pointer in that monitor's bottom right corner. Warping the pointer onto another
    monitor focuses that monitor and the window under the pointer; focusing a monitor puts the pointer in its
    middle and focuses its most recently focused window. `keys` is what the `input.keyboard.key` counter the
    server installs has counted, None until it is installed. `activate` is a window asking to be focused, which
    Hyprland grants with `misc:focus_on_activate`, as Omarchy sets it, unless a window rule says otherwise.

    `exec_cmd` of wl-mirror starts a real process, whose window opens on the workspace the exec rules name, as
    wl-mirror titles it, and closes when the process exits. Without " silent", Hyprland shows that workspace and
    focuses the window. Other commands are only recorded."""

    def __init__(self, monitors=None, clients=None, active=None):
        self.root = tempfile.mkdtemp(prefix="hc", dir="/tmp")  # AF_UNIX paths max out at 108 bytes
        self.monitors = [dict(m) for m in monitors or [M1_PANEL]]
        self.clients = clients or []
        self.active = active or {}
        self.cursor = (0, 0)
        self.focused = self.monitors[0]["name"]
        self.rules: dict[str, dict] = {}
        self.workspace_rules: list[dict] = []
        self.window_rules: dict[str, dict] = {}
        self.keys: int | None = None
        self.warps = False
        self.default_workspace_rules = True  # False: like Hyprland 0.56, a new output shows a numbered workspace
        self.binds: list[dict] = []
        self.animations = [[{"name": "global", "overridden": True, "enabled": True, "speed": 1.0}], []]
        self.requests: list[str] = []
        self.processes: list[subprocess.Popen] = []
        self.on_request = lambda request: None
        self.readers: list[socket.socket] = []
        self.listen("fake")

    def listen(self, signature: str) -> None:
        self.instance = os.path.join(self.root, "hypr", signature)
        os.makedirs(self.instance)
        self.server, self.events = (socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) for _ in range(2))
        for sock, name in ((self.server, ".socket.sock"), (self.events, ".socket2.sock")):
            sock.bind(os.path.join(self.instance, name))
            sock.listen(8)
        threading.Thread(target=self.serve, args=(self.server,), daemon=True).start()
        threading.Thread(target=self.serve_events, args=(self.events,), daemon=True).start()

    @property
    def env(self) -> dict[str, str]:
        return {"XDG_RUNTIME_DIR": self.root, "HYPRLAND_INSTANCE_SIGNATURE": "fake",
                "WAYLAND_DISPLAY": "wayland-none"}

    @property
    def stop_file(self) -> str:
        return os.path.join(self.root, "hypr-computer.stop")

    def serve(self, server: socket.socket) -> None:
        while True:
            try:
                conn, _ = server.accept()
            except OSError:
                return
            with conn:
                request = conn.recv(65536).decode()
                self.requests.append(request)
                self.on_request(request)
                conn.sendall(self.reply(request).encode())

    def monitor_at(self, x: float, y: float) -> dict | None:
        return next((m for m in self.monitors if m["x"] <= x < m["x"] + m["width"] / m["scale"]
                     and m["y"] <= y < m["y"] + m["height"] / m["scale"]), None)

    def window_at(self, x: float, y: float, monitor: dict) -> dict:
        shown = monitor["activeWorkspace"]["name"]
        return next((c for c in self.clients if (c.get("workspace") or {}).get("name") == shown
                     and c["at"][0] <= x < c["at"][0] + c["size"][0] and c["at"][1] <= y < c["at"][1] + c["size"][1]), {})

    def output(self, request: str) -> str:
        verb, *rest = request.split()[1:]
        name = rest[-1]
        if verb == "create":
            rule = self.rules.get(name, {"mode": "1920x1080", "position": "auto"})
            w, h = map(int, rule["mode"].split("x"))
            right = max(m["x"] + m["width"] / m["scale"] for m in self.monitors)
            x, y = (right, 0) if rule["position"] == "auto" else map(int, rule["position"].split("x"))
            ws = next((r["workspace"].removeprefix("name:") for r in self.workspace_rules
                       if r.get("monitor") == name and r.get("default") and self.default_workspace_rules),
                      str(20 + len(self.monitors)))
            m = {"id": max(m["id"] for m in self.monitors) + 1, "name": name, "x": x, "y": y, "width": w,
                 "height": h, "scale": float(rule.get("scale", 1)), "transform": 0,
                 "activeWorkspace": named_workspace(ws), "specialWorkspace": named_workspace("")}
            self.monitors = self.monitors + [m]
            for c in self.clients:
                if (c.get("workspace") or {}).get("name") == ws:
                    c["monitor"] = m["id"]
        else:
            gone = next(m for m in self.monitors if m["name"] == name)
            self.monitors = [m for m in self.monitors if m is not gone]
            first = self.monitors[0]
            for c in self.clients:
                if c.get("monitor") == gone["id"]:
                    c["monitor"] = first["id"]
            self.cursor = (first["x"] + first["width"] / first["scale"] - 1,
                           first["y"] + first["height"] / first["scale"] - 1)
            self.focused = first["name"]
        return "ok"

    def eval(self, code: str) -> str:
        if code.startswith("hl.monitor("):
            rule = self.rules[lua_table(code)["output"]] = lua_table(code)
            for m in self.monitors:
                if m["name"] == rule["output"]:
                    m["width"], m["height"] = map(int, rule["mode"].split("x"))
                    m["x"], m["y"] = map(int, rule["position"].split("x"))
                    m["scale"] = float(rule.get("scale", 1))
        elif code.startswith("hl.workspace_rule("):
            self.workspace_rules.append(lua_table(code))
        elif code.startswith("hl.window_rule("):
            rule = lua_table(code)
            self.window_rules[rule["name"]] = rule
        elif code.startswith("if not HYPR_COMPUTER_KEYS"):
            self.keys = 0 if self.keys is None else self.keys
        elif code.startswith("if HYPR_COMPUTER_KEYS_SUB"):
            self.keys = None
        else:
            return f"error: unknown Lua {code!r}"
        return "ok"

    def open_window(self, w: dict) -> dict:
        """A window opening as Hyprland opens it: a rule matching its class may put it on another workspace."""
        for r in self.window_rules.values():
            pattern = r.get("match", {}).get("class")
            if r.get("enabled", True) and pattern and "workspace" in r and re.fullmatch(pattern, w["class"]):
                name = r["workspace"].removesuffix(" silent").removeprefix("name:")
                shown = next((m for m in self.monitors if m["activeWorkspace"]["name"] == name), None)
                w = {**w, "workspace": {"id": -1, "name": name}, "monitor": shown["id"] if shown else w["monitor"]}
        self.clients.append(w)
        return w

    def activate(self, w: dict) -> None:
        ruled = [r for r in self.window_rules.values() if r.get("enabled", True)
                 and r.get("match", {}).get("workspace", "").removeprefix("name:") == w["workspace"]["name"]]
        if all(r.get("focus_on_activate", True) for r in ruled):
            self.active, self.cursor = w, (w["at"][0] + w["size"][0] // 2, w["at"][1] + w["size"][1] // 2)
            self.focused = next(m["name"] for m in self.monitors if m["id"] == w["monitor"])

    def dispatch(self, name: str, args: dict) -> None:
        if name == "cursor.move":
            was = self.monitor_at(*self.cursor)
            self.cursor = (args["x"], args["y"])
            now = self.monitor_at(*self.cursor)
            if now is not None and now is not was:
                self.focused = now["name"]
                self.active = self.window_at(*self.cursor, now) or self.active
        elif name == "focus" and "monitor" in args:
            m = next(m for m in self.monitors if m["name"] == args["monitor"])
            self.focused = m["name"]
            self.cursor = (m["x"] + m["width"] / m["scale"] / 2, m["y"] + m["height"] / m["scale"] / 2)
            shown = [c for c in self.clients if (c.get("workspace") or {}).get("name") == m["activeWorkspace"]["name"]]
            self.active = min(shown, key=lambda c: c.get("focusHistoryID", 99), default={})
        elif name == "focus" and "workspace" in args:  # shows it on the monitor a rule binds it to, and focuses that
            ws = str(args["workspace"]).removeprefix("name:")
            bound = next((r["monitor"] for r in self.workspace_rules if r["workspace"].removeprefix("name:") == ws), None)
            m = next(m for m in self.monitors if m["name"] == (bound or self.focused))
            m["activeWorkspace"] = named_workspace(ws)
            self.dispatch("focus", {"monitor": m["name"]})
        elif name == "focus" and "window" in args:
            w = next((c for c in self.clients if "address:" + c.get("address", "") == args["window"]), None)
            if w and self.warps:
                self.active, self.cursor = w, (w["at"][0] + w["size"][0] // 2, w["at"][1] + w["size"][1] // 2)
        elif name == "window.move":
            w = next(c for c in self.clients if "address:" + c["address"] == args["window"])
            ws = args["workspace"].removeprefix("name:")
            w["workspace"] = named_workspace(ws)
            w["monitor"] = next((m["id"] for m in self.monitors if m["activeWorkspace"]["name"] == ws),
                                next(m["id"] for m in self.monitors if m["name"] == self.focused))

    def exec_cmd(self, request: str) -> None:
        call = re.fullmatch(rf"/dispatch hl\.dsp\.exec_cmd\(({LUA_STRING})(?:, (\{{.*\}}))?\)", request, re.S)
        argv, rules = shlex.split(lua_value(call[1])), lua_table(call[2] or "")
        if os.path.basename(argv[0]) != "wl-mirror":
            return
        proc = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
        self.processes.append(proc)
        here = next(m for m in self.monitors if m["name"] == self.focused)
        ws, _, silent = rules.get("workspace", here["activeWorkspace"]["name"]).partition(" ")
        title = argv[argv.index("--title") + 1] if "--title" in argv else f"Wayland Output Mirror for {argv[-1]}"
        w = {"class": "at.yrlf.wl_mirror", "title": title, "address": f"0x{proc.pid:x}", "pid": proc.pid,
             "at": [here["x"], here["y"]], "size": [here["width"] / here["scale"], here["height"] / here["scale"]],
             "workspace": named_workspace(ws), "monitor": here["id"], "floating": False,
             "focusHistoryID": len(self.clients), "hidden": False}
        self.clients.append(w)
        if silent != "silent":
            here["activeWorkspace"], self.active = named_workspace(ws), w

    def reply(self, request: str) -> str:
        gone = {f"0x{p.pid:x}" for p in self.processes if p.poll() is not None}
        if gone & {c.get("address") for c in self.clients}:
            self.clients[:] = [c for c in self.clients if c.get("address") not in gone]
        if request.startswith("/dispatch hl.dsp.exec_cmd("):
            self.exec_cmd(request)
        if request.startswith("/output "):
            return self.output(request)
        if request.startswith("/eval "):
            return self.eval(request[len("/eval "):])
        if request == "/repl return HYPR_COMPUTER_KEYS":
            return "nil" if self.keys is None else str(self.keys)
        monitors = [{**m, "focused": m["name"] == self.focused} for m in self.monitors]
        answers = {"j/monitors": monitors, "j/clients": self.clients, "j/activewindow": self.active,
                   "j/cursorpos": {"x": self.cursor[0], "y": self.cursor[1]}, "j/version": {"version": "0.57.2"},
                   "j/animations": self.animations, "j/binds": self.binds}
        if request in answers:
            return json.dumps(answers[request])
        call = re.fullmatch(r"/dispatch hl\.dsp\.([\w.]+)\((\{.*\})?.*\)", request, re.S)
        if call:
            self.dispatch(call[1], lua_table(call[2] or ""))
        if request.startswith(("/dispatch hl.dsp.", "/notify ")):
            return "ok"
        return f"error: unknown request {request!r}"

    def dispatched(self, name: str) -> list[str]:
        return [r for r in self.requests if r.startswith(f"/dispatch hl.dsp.{name}(")]

    def serve_events(self, server: socket.socket) -> None:
        while True:
            try:
                self.readers.append(server.accept()[0])
            except OSError:
                return

    def emit(self, *events: str) -> None:
        data = "".join(e + "\n" for e in events).encode()
        for conn in list(self.readers):
            try:
                conn.sendall(data)
            except OSError:
                self.readers.remove(conn)

    def wait_for_reader(self, timeout: float = 5) -> None:
        end = time.monotonic() + timeout
        while not self.readers:
            assert time.monotonic() < end, "nothing connected to the event socket"
            time.sleep(0.01)

    def stop(self) -> None:
        for sock in [self.server, self.events, *self.readers]:
            sock.close()
        self.readers = []
        shutil.rmtree(self.instance, ignore_errors=True)

    def restart(self, signature: str) -> None:
        self.stop()
        self.listen(signature)

    def close(self) -> None:
        self.stop()
        for proc in self.processes:
            proc.kill()
            proc.wait()
        shutil.rmtree(self.root, ignore_errors=True)


class FakeInput:
    """Stands in for wl_input.VirtualInput and records what an agent pressed. on_input runs
    after each input, as the desktop would react to it."""

    def __init__(self):
        self.events: list[tuple] = []
        self.on_input = lambda event: None

    def record(self, *event):
        self.events.append(event)
        self.on_input(event)

    def click(self, button="left", count=1):
        self.record("click", button, count)

    def button(self, button, pressed):
        self.record("button", button, pressed)

    def scroll(self, dx, dy):
        self.record("scroll", dx, dy)

    def chord(self, codes):
        self.record("chord", tuple(codes))

    def type_ascii(self, text):
        self.record("type", text)


FAKE_GRIM = r'''#!/usr/bin/env python3
"""grim that only captures at full resolution: `grim [-o OUTPUT | -g "X,Y WxH"] -t ppm -`. The user's monitor,
the first one Hyprland lists, is FAKE_GRIM_SIZE=WxH, with white squares on a dark screen at physical pixels
FAKE_GRIM_MARK=x,y,side[;x,y,side...]; `-g` must ask for exactly that monitor. `-o` of another output draws
FAKE_GRIM_OUTPUT_MARK on it. Without -o or -g, real grim captures the box around every monitor, so with more
than one this fails rather than draw an image that size. FAKE_GRIM_MOVING=file,n moves the user's squares
right on each of the first n captures, counted in file."""
import json, os, socket, sys
args = sys.argv[1:]
if args[-3:] != ["-t", "ppm", "-"] or len(args) not in (3, 5) or (len(args) == 5 and args[0] not in ("-o", "-g")):
    sys.exit(f"fake grim: unsupported arguments {args}")


def monitors():
    env = os.environ
    path = os.path.join(env.get("XDG_RUNTIME_DIR", ""), "hypr", env.get("HYPRLAND_INSTANCE_SIGNATURE", ""),
                        ".socket.sock")
    try:
        with socket.socket(socket.AF_UNIX) as s:
            s.connect(path)
            s.sendall(b"j/monitors")
            s.shutdown(socket.SHUT_WR)
            return json.loads(b"".join(iter(lambda: s.recv(65536), b"")))
    except OSError:
        return []


def screen(w, h, marks, shift=0):
    px = bytearray(b"\x20" * (w * h * 3))
    for mx, my, side in marks:
        mx += side * shift
        for row in range(my, my + side):
            px[(row * w + mx) * 3:(row * w + mx + side) * 3] = b"\xff" * (side * 3)
    return w, h, px


def marks(var):
    return [tuple(map(int, m.split(","))) for m in os.environ.get(var, "").split(";") if m]


mons = monitors()
user = mons[0] if mons else None
if args[0] == "-o" and user and args[1] != user["name"]:
    out = next(m for m in mons if m["name"] == args[1])
    w, h, px = screen(out["width"], out["height"], marks("FAKE_GRIM_OUTPUT_MARK"))
else:
    if args[0] == "-g":
        want = "{:.0f},{:.0f} {:.0f}x{:.0f}".format(user["x"], user["y"], user["width"] / user["scale"],
                                                    user["height"] / user["scale"])
        if args[1] != want:
            sys.exit(f"fake grim: -g {args[1]!r} is not the user's monitor {want!r}")
    elif args[0] == "-t" and len(mons) > 1:
        sys.exit(f"fake grim: capturing every monitor would cover {[m['name'] for m in mons]} and the gaps between")
    shift = 0
    if os.environ.get("FAKE_GRIM_MOVING"):
        path, n = os.environ["FAKE_GRIM_MOVING"].split(",")
        shots = int(open(path).read() or 0) if os.path.exists(path) else 0
        open(path, "w").write(str(shots + 1))
        shift = min(shots, int(n))
    w, h, px = screen(*map(int, os.environ["FAKE_GRIM_SIZE"].split("x")), marks("FAKE_GRIM_MARK"), shift)
sys.stdout.buffer.write(b"P6\n%d %d\n255\n" % (w, h) + px)
'''

FAKE_TESSERACT = r'''#!/usr/bin/env python3
"""tesseract reading `stdin stdout --psm 3 tsv` from a PGM image. Each bright patch reads as one line of
text: the next of FAKE_OCR_TEXT's lines (separated by "|", default "Submit"), its words spread evenly across
the patch. Each read adds a line to the file FAKE_OCR_RUNS, if set."""
import os, re, sys
if sys.argv[1:] == ["--version"]:
    sys.exit(print("tesseract 5.5.4 (fake)"))
if sys.argv[1:] != ["stdin", "stdout", "--psm", "3", "tsv"]:
    sys.exit(f"fake tesseract: unsupported arguments {sys.argv[1:]}")
data = sys.stdin.buffer.read()
head = re.match(rb"P5\s+(\d+)\s+(\d+)\s+255\s", data)
w, h = int(head[1]), int(head[2])
px = memoryview(data)[head.end():]
patches = []
for y in range(h):
    for m in re.finditer(rb"[\xc8-\xff]+", px[y * w:(y + 1) * w].tobytes()):
        a, b = m.span()
        p = next((p for p in patches if p[3] == y and p[0] < b and a < p[2]), None)
        if p:
            p[0], p[2], p[3] = min(p[0], a), max(p[2], b), y + 1
        else:
            patches.append([a, y, b, y + 1])
if os.environ.get("FAKE_OCR_RUNS"):
    open(os.environ["FAKE_OCR_RUNS"], "a").write("read\n")
lines = (os.environ.get("FAKE_OCR_TEXT") or "Submit").split("|")
print("level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext")
print(f"1\t1\t0\t0\t0\t0\t0\t0\t{w}\t{h}\t-1\t")
for i, (left, top, right, bottom) in enumerate(sorted(patches, key=lambda p: (p[1], p[0]))):
    words = lines[i % len(lines)].split()
    step = (right - left) / len(words)
    print(f"4\t1\t{i + 1}\t1\t1\t0\t{left}\t{top}\t{right - left}\t{bottom - top}\t-1\t")
    for j, word in enumerate(words):
        print(f"5\t1\t{i + 1}\t1\t1\t{j + 1}\t{round(left + j * step)}\t{top}\t{round(step)}\t{bottom - top}\t95\t{word}")
'''


def fake_bin(directory: str, tesseract: bool = True) -> str:
    """A folder with fake grim and tesseract, a hyprctl that must not run, a wl-mirror that only Hyprland may
    run, and the test's python3, so a PATH of this folder alone works."""
    os.makedirs(directory, exist_ok=True)
    fakes = [("grim", FAKE_GRIM), ("hyprctl", "#!/bin/sh\necho 'hyprctl must not run' >&2\nexit 1\n"),
             ("wl-mirror", "#!/bin/sh\necho 'wl-mirror must be started through Hyprland' >&2\nexit 1\n")]
    for name, body in fakes + ([("tesseract", FAKE_TESSERACT)] if tesseract else []):
        path = os.path.join(directory, name)
        with open(path, "w") as f:
            f.write(body)
        os.chmod(path, 0o755)
    if not os.path.exists(os.path.join(directory, "python3")):
        os.symlink(sys.executable, os.path.join(directory, "python3"))
    return directory


def jpeg_size(data: bytes) -> tuple[int, int]:
    i = 2
    while i < len(data):
        marker, length = data[i + 1], int.from_bytes(data[i + 2:i + 4], "big")
        if marker in (0xC0, 0xC1, 0xC2):
            return int.from_bytes(data[i + 7:i + 9], "big"), int.from_bytes(data[i + 5:i + 7], "big")
        i += 2 + length
    raise ValueError("no JPEG frame header")
