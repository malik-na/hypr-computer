"""Measure hc-screen as the agent's screen. Run on the Linux machine, inside the desktop session:

    XDG_RUNTIME_DIR=... WAYLAND_DISPLAY=<host> HYPRLAND_INSTANCE_SIGNATURE=... \
        python3 measure.py --renderer gles|pixman [--pixman-dmabuf] --work DIR

Same method as the nested-compositor measurement: a page whose whole body is a button that counts
clicks; 10 injected clicks with the live view hidden, with the host pointer busy, and with the host
pointer moving over the visible live view; the host's workspace, focused window and pointer are
sampled before and after. The host's workspace and pointer are moved only inside a measurement and
put back.
"""
import argparse, json, os, re, signal, subprocess, sys, threading, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from hcctl import Screen

ap = argparse.ArgumentParser()
ap.add_argument("--renderer", default="gles")
ap.add_argument("--pixman-dmabuf", action="store_true")
ap.add_argument("--work", required=True)
ap.add_argument("--view-workspace", default="10")
args = ap.parse_args()

HERE = os.path.dirname(os.path.abspath(__file__))
BIN = os.path.join(HERE, "target", "release")
WORK = os.path.abspath(args.work)
WWW, SHOTS = os.path.join(WORK, "www"), os.path.join(WORK, "shots")
for d in (WWW, SHOTS, os.path.join(WORK, "config"), os.path.join(WORK, "cache")):
    os.makedirs(d, exist_ok=True)
RULE = "hc-proto-view"
CX, CY = 640, 400
out = {"renderer": args.renderer, "pixman_dmabuf": args.pixman_dmabuf}
procs = []


# --- host desktop --------------------------------------------------------------------------------
def hyprctl(*a):
    return subprocess.run(["hyprctl", *a], capture_output=True, text=True, timeout=5).stdout


def hj(what):
    return json.loads(hyprctl("-j", what))


def host_pos():
    return tuple(int(v) for v in hyprctl("cursorpos").split(","))


def host_seat():
    return (hj("activeworkspace")["name"], hj("activewindow").get("address"), host_pos())


def host_move(x, y):
    hyprctl("dispatch", f"hl.dsp.cursor.move({{x = {x}, y = {y}}})")


def view_window():
    return next((c for c in hj("clients") if c["class"] == "hc-view"), None)


def view_rule(enabled):
    extra = 'workspace = "%s silent", no_initial_focus = true, focus_on_activate = false' % args.view_workspace
    spec = f'name = "{RULE}", match = {{class = "hc-view"}}, ' + (extra if enabled else "enabled = false")
    return hyprctl("eval", f"hl.window_rule({{{spec}}})").strip()


# --- processes -----------------------------------------------------------------------------------
def spawn(argv, env, log):
    p = subprocess.Popen(argv, env=env, stdin=subprocess.DEVNULL, stdout=open(os.path.join(WORK, log), "w"),
                         stderr=subprocess.STDOUT, start_new_session=True)
    procs.append(p)
    return p


def kill_group(p, sig=signal.SIGTERM):
    try:
        os.killpg(p.pid, sig)
    except ProcessLookupError:
        pass


def app_env(screen_name):
    """What an agent's app sees: only the agent's display; nothing of the owner's config or cache."""
    env = {k: v for k, v in os.environ.items() if k not in ("DISPLAY", "HYPRLAND_INSTANCE_SIGNATURE")}
    env.update(WAYLAND_DISPLAY=screen_name, LANG="C.UTF-8", XDG_CONFIG_HOME=os.path.join(WORK, "config"),
               XDG_CACHE_HOME=os.path.join(WORK, "cache"))
    return env


def start_screen(name):
    argv = [os.path.join(BIN, "hc-screen"), "--name", name, "--renderer", args.renderer]
    if args.pixman_dmabuf:
        argv.append("--pixman-dmabuf")
    t = time.monotonic()
    p = spawn(argv, dict(os.environ), f"{name}.log")
    while not os.path.exists(f"{os.environ['XDG_RUNTIME_DIR']}/{name}.ctl"):
        if p.poll() is not None or time.monotonic() - t > 5:
            raise SystemExit(f"hc-screen {name} did not start: " + open(os.path.join(WORK, f"{name}.log")).read())
        time.sleep(0.005)
    s = Screen(name)
    s.pid, s.proc, s.startup_s = p.pid, p, round(time.monotonic() - t, 3)
    return s


def chromium(screen, profile, url):
    return spawn(["chromium", "--ozone-platform=wayland", f"--user-data-dir={os.path.join(WORK, profile)}",
                  "--no-first-run", "--no-default-browser-check", "--disable-extensions", "--password-store=basic",
                  url], app_env(screen.name), f"chromium-{screen.name}-{profile}.log")


def cpu_ticks(pid):
    f = open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()
    return int(f[11]) + int(f[12])


class Cpu:
    """CPU use of some pids as % of one core, over the time this object lives."""
    def __init__(self, **pids):
        self.pids, self.t, self.c = pids, time.monotonic(), {k: cpu_ticks(p) for k, p in pids.items()}

    def stop(self):
        dt = time.monotonic() - self.t
        hz = os.sysconf("SC_CLK_TCK")
        return {k: round((cpu_ticks(p) - self.c[k]) / hz / dt * 100, 1) for k, p in self.pids.items()}


def mem_mb(pid):
    text = open(f"/proc/{pid}/smaps_rollup").read()
    return {k.lower() + "_mb": round(int(re.search(rf"^{k}:\s+(\d+)", text, re.M).group(1)) / 1024, 1) for k in ("Rss", "Pss")}


# --- the screen ----------------------------------------------------------------------------------
def windows(s):
    return s("list")["windows"]


def shown(s):
    return next((w for w in windows(s) if w["shown"]), None)


def title(s):
    w = shown(s)
    return (w or {}).get("title") or ""


def wait_title(s, pred, timeout=10):
    t = time.monotonic()
    while time.monotonic() - t < timeout:
        if pred(title(s)):
            return round(time.monotonic() - t, 3)
        time.sleep(0.02)
    return None


def count(s):
    m = re.match(r"CLICKS (\d+)", title(s))
    return int(m.group(1)) if m else None


def shot(s, name):
    t = time.monotonic()
    r = s(f"shot {os.path.join(SHOTS, name)}")
    r["wall_ms"] = round((time.monotonic() - t) * 1000, 1)
    return r


def median(v):
    return sorted(v)[len(v) // 2]


def clicks(s, n, label, shots=True):
    ok, hashes, cap, wall, enc, latency = 0, [], [], [], [], []
    for i in range(n):
        before = count(s)
        t = time.monotonic()
        s(f"move {CX} {CY}")
        s("click")
        landed = None
        while time.monotonic() - t < 0.4:
            if count(s) == before + 1:
                landed = landed or round((time.monotonic() - t) * 1000)
                break
            time.sleep(0.01)
        time.sleep(max(0, 0.4 - (time.monotonic() - t)))
        ok += count(s) == before + 1
        if landed:
            latency.append(landed)
        if not shots:
            continue
        r = shot(s, f"{label}-{i}.png")
        hashes.append(r["hash"]); cap.append(r["capture_ms"]); enc.append(r["encode_ms"]); wall.append(r["wall_ms"])
    if not shots:
        return {"clicks_landed": f"{ok}/{n}"}
    return {"clicks_landed": f"{ok}/{n}", "distinct_screenshots": f"{len(set(hashes))}/{n}",
            "click_to_title_ms_median": median(latency) if latency else None,
            "shot_capture_ms_median": median(cap), "shot_capture_ms_max": max(cap),
            "shot_png_encode_ms_median": median(enc), "shot_wall_ms_median": median(wall), "shot_wall_ms_max": max(wall)}


def pixel_latency(s, n=5):
    """ms from the button press to the first captured frame that differs."""
    res = []
    for i in range(n):
        s(f"move {CX} {CY}"); time.sleep(0.3)
        h0 = s("shot -")["hash"]; t = time.monotonic(); s("click")
        while time.monotonic() - t < 1:
            if s("shot -")["hash"] != h0:
                res.append(round((time.monotonic() - t) * 1000, 1)); break
        time.sleep(0.3)
    return res


def goto(s, url, want):
    s("key ctrl+l"); time.sleep(0.15)
    s(f"type {url}\\n")
    return wait_title(s, lambda t: t.startswith(want), 10)


def ocr(path):
    return subprocess.run(["tesseract", path, "-", "--psm", "6"], capture_output=True, text=True, timeout=60).stdout


PAGES = {
    "c.html": "<title>CLICKS 0</title><body style='margin:0'><button id=b style='width:100vw;height:100vh;font:120px sans-serif' "
              "onclick=\"n=(window.n||0)+1;window.n=n;document.title='CLICKS '+n;b.textContent=n\">0</button>",
    "t.html": "<title>TYPE</title><body style='margin:0'><textarea id=a style='width:100vw;height:100vh;font:40px sans-serif' "
              "oninput=\"document.title='T:'+this.value\"></textarea>",
    "s.html": "<title>SEL</title><body style='margin:0'><select style='font:40px sans-serif;width:600px;height:80px' "
              "onchange=\"document.title='SEL:'+this.value\"><option>one<option>two<option>three<option>four</select>",
    "y.html": "<title>Y:0</title><body style='margin:0;height:6000px'><h1>scroll</h1>"
              "<script>addEventListener('scroll',()=>document.title='Y:'+Math.round(scrollY))</script>",
}
for name, html in PAGES.items():
    open(os.path.join(WWW, name), "w").write(html)
URL = lambda name: f"file://{WWW}/{name}"

stop = threading.Event()
def busy(points):
    while not stop.is_set():
        for x, y in points:
            host_move(x, y); time.sleep(0.15)

home = host_seat()          # where the owner is; everything is put back to this
out["host_seat_at_start"] = home
try:
    # ---- 0. the screen ----
    s = start_screen("hc-p0")
    out["screen_startup_s"] = s.startup_s
    out["screen_idle_empty"] = {"mem": mem_mb(s.pid)}
    c = Cpu(screen=s.pid); time.sleep(3); out["screen_idle_empty"]["cpu_pct"] = c.stop()

    # ---- 1. foot and Chromium each open a window ----
    t = time.monotonic()
    foot = spawn(["foot"], app_env(s.name), "foot.log")
    while not any(w["app_id"] == "foot" for w in windows(s)) and time.monotonic() - t < 10: time.sleep(0.01)
    out["1_foot_window_s"] = round(time.monotonic() - t, 3) if any(w["app_id"] == "foot" for w in windows(s)) else "NO WINDOW"
    time.sleep(1.0)

    # ---- 3a. typing lands in foot ----
    typed = os.path.join(WORK, "typed.txt")
    if os.path.exists(typed): os.remove(typed)
    s(f"type echo HC_$((6*7))_'a|B'~ > {typed}\\n"); time.sleep(0.8)
    got = open(typed).read().strip() if os.path.exists(typed) else None
    out["3_typing_foot"] = {"expected": "HC_42_a|B~", "got": got, "pass": got == "HC_42_a|B~"}
    shot(s, "foot.png")

    for run in ("cold", "warm"):
        t = time.monotonic()
        chrome = chromium(s, "profile", URL("c.html"))
        first = None
        while time.monotonic() - t < 30:
            ws = windows(s)
            if first is None and any("chrom" in (w["app_id"] or "").lower() for w in ws):
                first = round(time.monotonic() - t, 3)
            if first is not None and (title(s) or "").startswith("CLICKS"):
                break
            time.sleep(0.01)
        ready = round(time.monotonic() - t, 3) if (title(s) or "").startswith("CLICKS") else "NO PAGE"
        out[f"1_chromium_{run}"] = {"first_window_s": first if first is not None else "NO WINDOW", "page_ready_s": ready}
        if run == "cold":
            time.sleep(2); kill_group(chrome); chrome.wait(timeout=10); time.sleep(1.0)
    out["windows"] = windows(s)
    if count(s) is None:
        raise SystemExit("Chromium did not show the click page: " + json.dumps(out))
    time.sleep(1.5)

    # ---- live view: a window on the host, on a workspace the owner is not on ----
    out["view_rule"] = view_rule(True)
    view = spawn([os.path.join(BIN, "hc-view"), "--name", s.name], dict(os.environ), "view.log")
    t = time.monotonic()
    while not view_window() and time.monotonic() - t < 5: time.sleep(0.05)
    vw = view_window()
    out["view_window"] = {"workspace": vw["workspace"]["name"], "at": vw["at"], "size": vw["size"]} if vw else "NO WINDOW"
    out["view_open_left_host_seat_unchanged"] = host_seat() == home
    time.sleep(0.5)

    # ---- 7. idle cost, apps open, view hidden ----
    r0 = s("stats")["renders"]
    c = Cpu(screen=s.pid, view=view.pid); time.sleep(5)
    out["7_idle_apps_open_view_hidden"] = {"cpu_pct": c.stop(), "renders_in_5s": s("stats")["renders"] - r0, "mem": mem_mb(s.pid)}

    # ---- 2/4/6/7 A: view hidden, host idle ----
    seat0 = host_seat(); c = Cpu(screen=s.pid, view=view.pid)
    out["A_hidden_host_idle"] = clicks(s, 10, "A")
    out["A_hidden_host_idle"]["cpu_pct"] = c.stop()
    seat1 = host_seat()
    out["A_host_seat"] = {"before": seat0, "after": seat1, "unchanged": seat0 == seat1}
    c = Cpu(screen=s.pid, view=view.pid)
    out["A2_hidden_clicks_no_screenshots"] = clicks(s, 10, "A2", shots=False)
    out["A2_hidden_clicks_no_screenshots"]["cpu_pct"] = c.stop()
    out["A3_click_to_pixels_ms"] = pixel_latency(s)
    # the same 10 clicks with the test itself not polling the screen: what the screen costs on its own
    n0 = count(s); c = Cpu(screen=s.pid, view=view.pid)
    for _ in range(10):
        s(f"move {CX} {CY}"); s("click"); time.sleep(0.4)
    out["A4_hidden_clicks_no_polling"] = {"cpu_pct": c.stop(), "clicks_landed": f"{count(s) - n0}/10"}

    # ---- B: hidden, host pointer busy ----
    pos0 = host_pos(); stop.clear()
    th = threading.Thread(target=busy, args=([(300, 300), (900, 500), (500, 700)],)); th.start()
    out["B_hidden_host_busy"] = clicks(s, 10, "B"); stop.set(); th.join()
    host_move(*pos0)
    out["B_host_workspace_focus_unchanged"] = host_seat()[:2] == seat0[:2]

    # ---- C: view visible, host pointer moving over it ----
    seatC = host_seat(); pos0 = host_pos()
    hyprctl("dispatch", f"hl.dsp.focus({{workspace = {args.view_workspace}}})"); time.sleep(1.2)
    try:
        vw = view_window()
        x0, y0, w, h = vw["at"][0], vw["at"][1], vw["size"][0], vw["size"][1]
        over = [(x0 + int(w * fx), y0 + int(h * fy)) for fx, fy in ((0.25, 0.3), (0.7, 0.5), (0.5, 0.5), (0.4, 0.8))]
        s("move 100 100"); n0 = count(s); time.sleep(0.2)
        for x, y in over:
            host_move(x, y); time.sleep(0.25)
        p = s("pos")
        out["C_visible_agent_pointer_after_host_moves"] = [p["x"], p["y"]]     # [100, 100] if nothing leaks in
        out["C_visible_count_unchanged_by_host_hover"] = count(s) == n0
        stop.clear(); th = threading.Thread(target=busy, args=(over,)); th.start()
        c = Cpu(screen=s.pid, view=view.pid)
        out["C_visible_host_hovering"] = clicks(s, 10, "C")
        out["C_visible_host_hovering"]["cpu_pct"] = c.stop()
        stop.set(); th.join()
        # what the owner would see: the host's own screenshot of the view, read back by OCR
        grab = os.path.join(SHOTS, "host-view.png")
        subprocess.run(["grim", "-g", f"{x0},{y0} {w}x{h}", grab], timeout=10)
        seen = re.findall(r"\d+", ocr(grab))
        out["C_view_shows_screen"] = {"count_on_screen": count(s), "numbers_read_from_host_view": seen[:5],
                                      "pass": str(count(s)) in seen, "view_size": [w, h]}
        c = Cpu(screen=s.pid, view=view.pid); time.sleep(3)
        out["C_visible_idle_cpu_pct"] = c.stop()
    finally:
        stop.set()
        hyprctl("dispatch", f"hl.dsp.focus({{workspace = {seatC[0]}}})"); time.sleep(0.6)
        host_move(*pos0); time.sleep(0.2)
    seatC2 = host_seat()
    out["C_host_seat_restored"] = {"before": seatC, "after": seatC2, "same": seatC == seatC2}

    # ---- D: hidden again ----
    out["D_hidden_again"] = clicks(s, 3, "D")

    # ---- 3b. typing lands in a Chromium text field ----
    nav = goto(s, URL("t.html"), "TYPE")
    s(f"move {CX} {CY}"); s("click"); time.sleep(0.2)
    text = "Hello, World! 123 (a_b) @#%"
    s(f"type {text}")
    ok = wait_title(s, lambda t: t.startswith("T:" + text), 5)
    out["3_typing_chromium"] = {"url_typed_into_omnibox_s": nav, "expected": text, "title": title(s), "pass": ok is not None and nav is not None}
    shot(s, "typed.png")

    # ---- clipboard between two apps on the screen, and switching the shown app ----
    clip = os.path.join(WORK, "clip.txt")
    if os.path.exists(clip): os.remove(clip)
    s("key ctrl+a"); time.sleep(0.1); s("key ctrl+c"); time.sleep(0.3)
    browser_n = next(w["n"] for w in windows(s) if w["shown"])
    foot_n = next(w["n"] for w in windows(s) if w["app_id"] == "foot")
    h_browser = s("shot -")["hash"]
    s(f"focus {foot_n}"); time.sleep(0.4)
    h_foot = shot(s, "switched-to-foot.png")["hash"]
    s(f"type cat > {clip}\\n"); time.sleep(0.3); s("key ctrl+shift+v"); time.sleep(0.4)
    s("type \\n"); s("key ctrl+d"); time.sleep(0.4)
    got = open(clip).read().strip() if os.path.exists(clip) else None
    s(f"focus {browser_n}"); time.sleep(0.4)
    out["clipboard_and_switch"] = {"copied_in_chromium_pasted_in_foot": got, "pass": got == text,
                                   "switch_changed_screen": h_browser != h_foot,
                                   "switched_back": s("shot -")["hash"] == h_browser or title(s).startswith("T:")}

    # ---- scroll ----
    nav = goto(s, URL("y.html"), "Y:")
    s(f"move {CX} {CY}"); s("scroll 3")
    ok = wait_title(s, lambda t: t.startswith("Y:") and not t.startswith("Y:0"), 3)
    out["scroll"] = {"title": title(s), "pass": ok is not None}

    # ---- 5. popups: a <select> and the right-click menu ----
    nav = goto(s, URL("s.html"), "SEL")
    before = shot(s, "select-closed.png")["hash"]
    # the page starts 87px down (tab strip + toolbar): the control is at y 87..167, its list opens below
    s("move 300 127"); s("click"); time.sleep(0.6)
    popups_open = s("list")["popups"]
    opened = shot(s, "select-open.png")["hash"]
    s("move 300 300"); time.sleep(0.2); hovered = shot(s, "select-hover.png")["hash"]
    s("click")
    picked = title(s) if wait_title(s, lambda t: t.startswith("SEL:"), 2.0) is not None else None
    time.sleep(0.3)
    out["5_popup_select"] = {"popups_while_open": popups_open, "screenshot_changed": before != opened,
                             "hover_changed_popup": hovered != opened, "picked": picked, "popups_after": s("list")["popups"],
                             "pass": popups_open >= 1 and picked == "SEL:three - Chromium" and s("list")["popups"] == 0}
    s("move 900 500"); s("click right"); time.sleep(0.6)
    ctx = s("list")["popups"]; shot(s, "context-menu.png")
    s("key Escape"); time.sleep(0.4)
    out["5_popup_context_menu"] = {"popups_while_open": ctx, "popups_after_escape": s("list")["popups"],
                                   "pass": ctx >= 1 and s("list")["popups"] == 0}

    # ---- 9. does Chromium composite with the GPU? ----
    goto(s, "chrome://gpu", "GPU")
    time.sleep(4)
    for _ in range(3): s("key ctrl+="); time.sleep(0.2)
    time.sleep(1.0); shot(s, "chrome-gpu.png")
    text = ocr(os.path.join(SHOTS, "chrome-gpu.png"))
    st = s("stats")
    gpu_proc = subprocess.run(["pgrep", "-af", f"type=gpu-process.*{WORK}"], capture_output=True, text=True).stdout
    out["9_chromium_gpu"] = {"chrome_gpu_page_lines": [l.strip() for l in text.splitlines() if re.search(r"Compositing|Rasteriz|WebGL|Canvas|OpenGL|Vulkan|Skia|Video", l)][:14],
                             "client_buffers": {k: st[k] for k in ("commits_shm", "commits_dmabuf", "commits_other")},
                             "gpu_process_flags": sorted(set(re.findall(r"--(?:use-gl|use-angle|ozone-platform|disable-gpu[a-z-]*)(?:=\S+)?", gpu_proc)))}
    for _ in range(1): s("key ctrl+0")
    goto(s, URL("c.html"), "CLICKS")

    # ---- 8. two screens at once ----
    s2 = start_screen("hc-p1")
    chrome2 = chromium(s2, "profile-b", URL("c.html"))
    ready2 = wait_title(s2, lambda t: t.startswith("CLICKS"), 30)
    time.sleep(1.5)
    a0, b0 = count(s), count(s2)
    r2 = clicks(s2, 5, "two-b"); a1, b1 = count(s), count(s2)
    r1 = clicks(s, 5, "two-a"); a2, b2 = count(s), count(s2)
    both = []                                         # and both at the very same time
    ths = [threading.Thread(target=lambda scr=scr, lab=lab: both.append((lab, clicks(Screen(scr.name), 5, lab)["clicks_landed"])))
           for scr, lab in ((s, "two-a2"), (s2, "two-b2"))]
    [t.start() for t in ths]; [t.join() for t in ths]
    a3, b3 = count(s), count(s2)
    ha, hb = shot(s, "two-a-final.png")["hash"], shot(s2, "two-b-final.png")["hash"]
    out["8_two_screens"] = {"second_chromium_ready_s": ready2, "screen_b_5_clicks": r2["clicks_landed"], "a_unchanged_meanwhile": a1 == a0,
                            "screen_a_5_clicks": r1["clicks_landed"], "b_unchanged_meanwhile": b2 == b1,
                            "simultaneous": dict(both), "counts_after_simultaneous": {"a": [a2, a3], "b": [b2, b3]},
                            "screens_differ": ha != hb, "mem_b": mem_mb(s2.pid),
                            "pass": r2["clicks_landed"] == "5/5" and r1["clicks_landed"] == "5/5" and a1 == a0 and b2 == b1
                                    and a3 == a2 + 5 and b3 == b2 + 5}

    # ---- crash behaviour: kill -9 one screen ----
    t = time.monotonic(); os.kill(s2.pid, signal.SIGKILL)
    while chrome2.poll() is None and time.monotonic() - t < 10: time.sleep(0.01)
    gone = round(time.monotonic() - t, 3) if chrome2.poll() is not None else "STILL RUNNING"
    time.sleep(0.5)
    left = subprocess.run(["pgrep", "-fc", os.path.join(WORK, "profile-b")], capture_output=True, text=True).stdout.strip()
    after = clicks(s, 2, "after-kill")
    out["crash_kill9_screen_b"] = {"its_chromium_exited_s": gone, "its_chromium_processes_left": left,
                                   "screen_a_still_works": after["clicks_landed"],
                                   "stale_files": [f for f in os.listdir(os.environ["XDG_RUNTIME_DIR"]) if f.startswith("hc-p1")]}
    for f in out["crash_kill9_screen_b"]["stale_files"]:
        os.remove(os.path.join(os.environ["XDG_RUNTIME_DIR"], f))

    out["memory_end"] = {"screen": mem_mb(s.pid), "view": mem_mb(view.pid)}
    out["stats_end"] = s("stats")
    out["host_seat_at_end"] = host_seat()
    out["host_workspace_and_focus_same_as_start"] = out["host_seat_at_end"][:2] == home[:2]

    # ---- clean exit on SIGTERM ----
    t = time.monotonic(); os.kill(s.pid, signal.SIGTERM); s.proc.wait(timeout=5)
    out["sigterm_exit"] = {"exit_s": round(time.monotonic() - t, 3), "code": s.proc.returncode,
                           "files_left": [f for f in os.listdir(os.environ["XDG_RUNTIME_DIR"]) if f.startswith("hc-p0")]}
finally:
    stop.set()
    if hj("activeworkspace")["name"] != home[0]:
        hyprctl("dispatch", f"hl.dsp.focus({{workspace = {home[0]}}})")
    for p in procs:
        kill_group(p)
    time.sleep(1.0)
    for p in procs:
        kill_group(p, signal.SIGKILL)
    for p in procs:
        try: p.wait(timeout=5)
        except Exception: pass
    out["view_rule_removed"] = view_rule(False)
    for f in os.listdir(os.environ["XDG_RUNTIME_DIR"]):
        if f.startswith("hc-p"):
            os.remove(os.path.join(os.environ["XDG_RUNTIME_DIR"], f))
    out["left_running"] = [subprocess.run(["pgrep", *pat], capture_output=True, text=True).stdout.split()
                           for pat in (["-x", "hc-screen"], ["-x", "hc-view"], ["-f", f"user-data-dir={WORK}"])]
    out["view_windows_left"] = len([c for c in hj("clients") if c["class"] == "hc-view"])
    json.dump(out, open(os.path.join(WORK, "results.json"), "w"), indent=1)
    print(json.dumps(out, indent=1))
