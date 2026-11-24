# Smithay screen prototype (throwaway)

**Question:** can hypr-computer use its own Wayland compositor, written in Rust with
[Smithay](https://github.com/Smithay/smithay), as "the agent's screen", instead of a nested Hyprland?

**Answer:** yes. Every check the nested-Hyprland prototype passed also passes here, on a GPU path and
on a software path, with faster screenshots, a third of the memory or less, and no startup ordering
to get right. Details and caveats below.

This code is a measurement rig, not a starting point for the product. It has no tests, one hard-coded
screen size, and a US-ASCII keyboard table.

## What is here

| File | What it is |
|---|---|
| `src/main.rs` | `hc-screen`: the headless compositor. One process is one screen (1280x800, scale 1). |
| `src/control.rs` | Its control socket: line commands in, one JSON line out. |
| `src/bin/hc_view.rs` | `hc-view`: the live view, a normal window on the host desktop. |
| `measure.py` | The measurement, same method as the nested-compositor one. |
| `anim.py` | Worst-case cost: a page repainting the whole screen every frame. |
| `hcctl.py` | Ten-line client for the control socket. |

How the parts fit:

- **Apps** connect to `hc-screen` as their Wayland display (`WAYLAND_DISPLAY=<name>`). Every toplevel is
  configured to the whole screen (maximized, tiled, no decorations). Only one is mapped at a time: the
  newest, or the one picked with `focus N`.
- **Input** is injected straight into Smithay's seat (`pointer.motion/button/axis`, `keyboard.input`).
  There is no input device and no virtual-pointer protocol.
- **Pixels** are rendered into an offscreen buffer and read back from it. There is no output on the
  host and no screencopy client. The screen renders when an app commits, whether or not anyone looks.
- **Live view** is a separate process and a plain `wl_shm` + `wp_viewporter` client of the host. It asks
  the screen for a frame each time the host asks it to draw. Hidden, it stops asking and nothing else
  changes. It never binds `wl_seat`, so the host's pointer and keyboard have no path to the agent's apps.
- **Renderers:** `--renderer gles` (EGL on the host's render node through GBM, `linux-dmabuf` for
  clients) or `--renderer pixman` (software, `wl_shm` only). `--pixman-dmabuf` is a third mix: apps
  render with the GPU, the screen maps their buffers and composites in software.

```
cargo build --release
target/release/hc-screen --name hc-0 --renderer gles &      # prints one JSON line when ready
WAYLAND_DISPLAY=hc-0 foot &
python3 hcctl.py hc-0 "type echo hello\n"
python3 hcctl.py hc-0 "shot /tmp/s.png"
target/release/hc-view --name hc-0                          # on the host's WAYLAND_DISPLAY
```

Commands: `move X Y`, `click [left|right|middle]`, `down`/`up`, `scroll DY [DX]`, `key ctrl+l`,
`type TEXT`, `shot PATH|-`, `frame`, `list`, `focus N`, `close N`, `pos`, `stats`, `quit`.

## Results

Apple Silicon Linux, Hyprland 0.57.2 host, Chromium 158, foot 1.28, Mesa 26.3-devel, Rust 1.100.
The owner's desktop stayed on its own workspace; the live view was on workspace 10.
GPU path: 3 runs (the first with a broken `<select>` test, see "Problems hit"). Software path: 2 runs.
Numbers are ranges across runs.

| # | Check | GPU (`gles`) | Software (`pixman`) |
|---|---|---|---|
| 1 | foot and Chromium each open a window | pass. foot 0.03 s; Chromium first window 0.18-0.23 s, page ready 0.21-0.26 s | pass. foot 0.03 s; Chromium 0.18-0.20 s, page 0.21-0.23 s |
| 2 | 10 clicks counted: view hidden / host pointer busy / host pointer moving over the visible view | pass. 10/10, 10/10, 10/10 in every run | pass. 10/10, 10/10, 10/10 in every run |
| 3 | Typing lands in foot and in a Chromium text field | pass (shifted characters included; the URL was typed into the omnibox too) | pass |
| 4 | Screenshot after each click differs, view hidden | pass, 10/10 distinct. Capture 1.3-2.1 ms median (max 6.4); as a PNG file 12-17 ms median (max 22) | pass, 10/10 distinct. Capture 2.4-7.3 ms median (max 8.6); as a PNG file 8-24 ms median (max 26) |
| 5 | Popups: `<select>` list opens, hover highlights, click picks an option; right-click menu opens and Escape closes it | pass | pass |
| 6 | Host workspace, focused window and pointer identical before and after | pass. Agent pointer stayed at 100,100 while the host pointer moved over the view | pass, same |
| 7 | Cost of the screen process | idle 0.2% of one core; 10 clicks 1.0%; memory 71-78 MB RSS (36-42 MB PSS) | idle 0.0-0.2%; 10 clicks 1.7%; memory 29-34 MB RSS (19-24 MB PSS) |
| 8 | Two screens at once (two processes, two Chromiums) | pass. 5/5 on each while the other did not move; 5/5 + 5/5 clicked at the same time | pass, same |
| 9 | Chromium composites with the GPU | yes. `chrome://gpu`: "Compositing: Hardware accelerated"; 277-325 dmabuf buffers committed by the time of the check | no. "Compositing: Software only"; every buffer is `wl_shm` |

Against the nested-Hyprland numbers on the same machine:

| | Nested Hyprland | This prototype, GPU | This prototype, software |
|---|---|---|---|
| Clicks, three conditions | 10/10, 10/10, 5/5 | 10/10, 10/10, 10/10 | 10/10, 10/10, 10/10 |
| Screenshot while hidden | 35-52 ms | 12-17 ms as PNG (1.3-2.1 ms capture) | 8-24 ms as PNG (2.4-7.3 ms capture) |
| CPU, one core | 0.7-3% | 0.2% idle, 1.0% clicking | 0.0-0.2% idle, 1.7% clicking |
| Memory | 251 MB | 71-78 MB | 29-34 MB |
| Chromium opened | 0.4 s | 0.21-0.26 s | 0.21-0.23 s |
| Screen ready after start | needs a verified-ready step | 0.03-0.04 s | 0.006-0.011 s |

More numbers:

- **CPU during the click test depends on what the test does.** 1.0% / 1.7% is ten clicks with nothing
  else. With the test polling the window list every 10 ms it is 3.2% / 4.0-4.7%. With a PNG screenshot
  after every click, encoded inside the screen process, it is 5.6-7.2% / 7.5-9.9%. The nested
  measurement took screenshots with `grim`, a separate process that was not counted.
- **Click to changed pixels:** 16-32 ms on the GPU path, 17-48 ms on the software path.
- **Live view:** 10 MB. While visible and the screen is changing: view 1.5-2.7%, screen unchanged.
  Visible and idle: both at or under 1%. The host's own screenshot of the view read back the right click count.
- **Worst case, whole screen repainted every frame:** GPU 60 fps at 6.6% of one core. Software 52 fps at
  23.4% (Chromium is rasterising in software too; its own CPU was not measured).
- **Also passed on both paths, not in the list above:** wheel scroll; copy in Chromium, switch to foot
  with `focus N`, paste there; `kill -9` of one screen (its Chromium was gone in 0.02-0.05 s, the other
  screen kept working; the dead screen's socket files stay behind); SIGTERM (exit in 0.003-0.03 s,
  nothing left).
- **`--pixman-dmabuf`** (1 run): everything passes and Chromium composites with the GPU, but the screen
  reads GPU memory from the CPU: 17% of one core while clicking, 46-50 ms click to pixels. Not worth it.

## What Smithay gave for free, and what had to be written

Smithay 0.7.0 (the current crates.io release), with `wayland_frontend`, `desktop`, `renderer_gl`,
`renderer_pixman`, `backend_egl`, `backend_gbm`, `backend_drm`. Also wayland-server 0.31.14,
calloop 0.14.5, xkbcommon 0.8.0, pixman 0.2.1, gbm 0.18.0, drm 0.14.1; for the view wayland-client
0.31.15 and wayland-protocols 0.32.13. 127 crates in the lockfile. Stripped `hc-screen` is 3.4 MB.

Free:

- The protocols: `wl_compositor` and subsurfaces, `wl_shm`, `xdg_shell` (toplevels, popups, positioner
  maths, popup grabs), `xdg-decoration`, `viewporter`, single-pixel-buffer, `wl_seat` (xkb keymap,
  focus, serials), `wl_data_device` (the clipboard between apps worked with no code of ours),
  `wl_output` and `xdg-output`, `linux-dmabuf` with feedback.
- Rendering: two renderers behind the same traits, buffer import (shm and dmabuf), the surface tree as
  render elements, damage tracking, offscreen targets, pixel readback.
- Desktop helpers: `Space`, `Window`, hit-testing through subsurfaces and popups, frame callbacks.
- An input API that takes injected events as-is.

Written, about 1,450 lines of Rust:

- Handler glue and setup for the above (~500 lines).
- The window policy: one app, full-screen; show, focus, close (~80).
- Offscreen render, readback, when to render and when to send frame callbacks (~100).
- GBM + EGL on the render node (10 lines).
- The control socket, commands and the keyboard table (270).
- The live view client (260).

It compiled on the first build and foot drew on the first run.

## Problems hit

1. **Upside-down screenshots on the GPU path.** `GlesMapping::flipped()` returns true, but an offscreen
   texture rendered with `Transform::Normal` reads back top-down already. Trusting the flag flipped it.
2. **The two renderers do not share a convenient bound.** The render function is a macro expanded once
   per renderer; a generic function was not attempted.
3. **Chromium delays window-title updates by about 200 ms.** Click-to-title measured 215-222 ms on both
   paths while pixels changed in 16-48 ms. A tool that reads titles to confirm an action must wait.
4. **Frame pacing is naive.** A 16 ms timer that restarts after the work is done gives 52 fps on the
   software path under full-screen animation. A real version needs a proper frame clock.
5. **`kill -9` leaves the socket, lock and frame files.** The daemon has to remove them.
6. **foot's warnings list what is missing:** fractional scale, cursor-shape, toplevel icon, text-input
   ("IME will be disabled").
7. **My own test bug:** the first `<select>` test clicked the browser toolbar (the page starts 87 px
   down). Popups were fine.

Nothing crashed or hung in any run.

## Not built, not tested

- **Text beyond US ASCII.** `type` presses keys from a fixed table. Other characters need either a
  keymap built on the fly or `text-input-v3`. IME is absent.
- **The pointer is not drawn** into screenshots or the view.
- **Take-over.** The view is watch-only and there is no code to let the user's input in.
- **Clipboard to and from the host**, primary selection, drag and drop (the protocol is there, nothing
  exercised it).
- **Scale.** Fixed at 1; no fractional-scale, no HiDPI test.
- **Apps.** Only Chromium and foot. No GTK or Qt app, no XWayland.
- **File dialogs.** Chromium's file picker goes through the desktop portal, which runs in the user's
  session and would open on the user's screen. Nested Hyprland has the same problem.
- **Zero-copy view.** The view copies frames through shared memory.
- **GPU loss, multiple GPUs, a machine with no render node.** Only the happy path ran; the software path
  is the fallback and it works.
- **Long runs.** The longest screen lived about 70 s.

## Read on production cost and risk

The compositor core for this narrow shape is small and Smithay carries most of it. The work is the
list above plus error handling and tests. A first version behind the `Screen` interface looks like
weeks of work, not months. The risks, most serious first:

- **The long tail of apps.** Each missing protocol shows up as a quirk in some app, one at a time. With
  nested Hyprland that tail is someone else's.
- **Text input.** Typing arbitrary Unicode reliably is the largest single item and nothing here
  de-risks it.
- **Smithay is 0.x.** 0.7.0 was tagged on 2025-06-24 and was still the newest release on crates.io on
  2026-11-24; development continues on its main branch. Pin the version and expect a porting cost per
  upgrade.
- **A panic takes the agent's apps with it.** One process per screen keeps that to one agent, and apps
  exit within 0.05 s when their screen dies.
- **GPU drivers.** One driver was tested. The software path costs little for the pages agents mostly
  see and should be the automatic fallback.

## Recommendation

Use the own compositor, and keep nested Hyprland behind the same interface as planned. The two
problems the nested design had to fix (rendering stops when the window is hidden; the host pointer
leaks in) cannot happen here by construction, and the startup ordering bug class has nothing to
attach to: the screen has no host output, no mirror and no focus to settle.
