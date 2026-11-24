"""Worst-case cost: a page repainting the whole 1280x800 screen every frame.
    python3 anim.py gles|pixman WORKDIR      (same environment as measure.py; no host window is opened)"""
import json, os, re, signal, subprocess, sys, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from hcctl import Screen

renderer, WORK = sys.argv[1], os.path.abspath(sys.argv[2])
BIN = os.path.join(os.path.dirname(os.path.abspath(__file__)), "target", "release")
RUN = os.environ["XDG_RUNTIME_DIR"]
os.makedirs(WORK, exist_ok=True)
page = os.path.join(WORK, "anim.html")
open(page, "w").write("<title>FPS ?</title><body><script>let n=0,t=performance.now();function f(){n++;"
    "document.body.style.background='hsl('+(n*7%360)+',80%,50%)';const now=performance.now();"
    "if(now-t>=1000){document.title='FPS '+n;n=0;t=now}requestAnimationFrame(f)}f()</script>")
procs = []
def spawn(argv, env):
    p = subprocess.Popen(argv, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
    procs.append(p); return p
def ticks(pid):
    f = open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split(); return int(f[11]) + int(f[12])
try:
    scr = spawn([os.path.join(BIN, "hc-screen"), "--name", "hc-a0", "--renderer", renderer], dict(os.environ))
    while not os.path.exists(f"{RUN}/hc-a0.ctl"): time.sleep(0.01)
    s = Screen("hc-a0")
    env = {k: v for k, v in os.environ.items() if k not in ("DISPLAY", "HYPRLAND_INSTANCE_SIGNATURE")}
    env.update(WAYLAND_DISPLAY="hc-a0", LANG="C.UTF-8", XDG_CONFIG_HOME=os.path.join(WORK, "config"), XDG_CACHE_HOME=os.path.join(WORK, "cache"))
    spawn(["chromium", "--ozone-platform=wayland", f"--user-data-dir={os.path.join(WORK, 'profile')}", "--no-first-run",
           "--no-default-browser-check", "--disable-extensions", "--password-store=basic", "file://" + page], env)
    time.sleep(5)
    r0, c0, t0 = s("stats")["renders"], ticks(scr.pid), time.monotonic()
    fps = []
    for _ in range(5):
        time.sleep(1)
        m = re.match(r"FPS (\d+)", next((w["title"] or "" for w in s("list")["windows"] if w["shown"]), ""))
        fps.append(int(m.group(1)) if m else None)
    dt = time.monotonic() - t0
    shots = [s("shot -")["capture_ms"] for _ in range(20)]
    print(json.dumps({"renderer": renderer, "page_fps": fps, "screen_renders_per_s": round((s("stats")["renders"] - r0) / dt, 1),
                      "screen_cpu_pct_of_one_core": round((ticks(scr.pid) - c0) / os.sysconf("SC_CLK_TCK") / dt * 100, 1),
                      "shot_capture_ms_median_while_animating": sorted(shots)[10]}))
finally:
    for p in procs:
        try: os.killpg(p.pid, signal.SIGTERM)
        except ProcessLookupError: pass
    time.sleep(1)
    for p in procs:
        try: os.killpg(p.pid, signal.SIGKILL)
        except ProcessLookupError: pass
        p.wait()
    for f in os.listdir(RUN):
        if f.startswith("hc-a0"): os.remove(os.path.join(RUN, f))
