# Reference

## Installing

The installer:

1. Downloads the latest release to `~/.local/share/hypr-computer`.
2. Installs `grim`, `wtype`, `xkbcli`, `uv` and `wl-mirror` with `sudo`, only if any of them is missing. `wl-mirror` is optional. If it can't be installed, the install goes on without it, and you can't watch the agent's workspace.
3. Downloads the Python dependencies at the versions pinned in `server.py.lock`, so agents start without network access.
4. Runs a self-check, which clicks nothing and types no text.
5. Registers hypr-computer with each agent you've used: Claude Code, Codex, opencode, Gemini CLI and Cursor.
6. Adds the `hypr-computer` command to `~/.local/bin`.

To pass options, put them after `bash -s --`:

```bash
curl -fsSL malik-na.github.io/hypr-computer | bash -s -- --clients=claude --yes
```

| Option | Effect |
| --- | --- |
| `--yes` | Don't ask for confirmation |
| `--dry-run` | Show every step and change nothing |
| `--clients=claude,codex` | Register only these agents (`none` for no agents) |
| `--no-system` | Don't install missing packages |
| `--dir=PATH` | Install somewhere other than `~/.local/share/hypr-computer` |
| `--ref=TAG` | Install a specific release, such as `v0.6.0` |

**Other agents** (VS Code, Zed, Goose, Cline, Windsurf and more): see [Connecting your agent](clients.md).

**Claude Code plugin.** Instead of the installer, Claude Code users can install hypr-computer as a plugin:

```bash
claude plugin marketplace add malik-na/hypr-computer
claude plugin install hypr-computer@hypr-computer
```

## Updating

```bash
hypr-computer update
```

Then restart your agents, because each agent keeps running the version it started with. If you forget, the agent is told once that a newer version is installed and passes that on to you.

- **Tested releases only.** It installs the newest tagged release, never work in progress.
- **Automatic rollback.** If the new version fails its self-check, your current version is restored.
- **Your edits are kept.** Local changes to the install are saved to a `.patch` file next to it first.
- **One command for everything.** It also updates the Claude Code plugin, if you use it.

On v0.5 or older there's no `hypr-computer` command yet. Run the install command once more to get it.

## The `hypr-computer` command

| Command | What it does |
| --- | --- |
| `hypr-computer update` | Install the latest release |
| `hypr-computer check` | Run the self-check, which shows a PASS, WARN or FAIL line for each requirement |
| `hypr-computer watch` | Show the workspace where you watch the agent at work. If you closed its live view, it opens again. See [The agent display](#the-agent-display) |
| `hypr-computer stop` | Block every tool call from all agents, screenshots included, at once |
| `hypr-computer resume` | Allow them again |
| `hypr-computer allow foot,firefox --for 30m` | Let agents click and type only in these apps, for 30 minutes. See [Allowed apps](#allowed-apps) |
| `hypr-computer allow --list` | Show which apps agents may use |
| `hypr-computer allow --off` | Let agents use every app again |
| `hypr-computer version` | Show the installed version |


## Tools

These are the tools your agent gets:

| Tool | What it does |
| --- | --- |
| `screenshot` | Captures the agent display, or all your monitors as one image, once the screen has settled |
| `click` | Left, right or middle click at a point; double and triple clicks too |
| `type_text` | Types text into the focused window of the screen the agent works on. On your screen, accents and other scripts work too |
| `key` | Presses a key or shortcut such as `Return`, `ctrl+l` or `super+Return` in the same window. On your screen, Hyprland keybinds work |
| `scroll` | Scrolls up, down, left or right at a point |
| `drag` | Drags with the left button from one point to another |
| `move` | Moves the pointer, for example to hover |
| `batch` | Runs several actions in one call, then returns a screenshot |
| `wait_for` | Waits until a window opens, the focused window's title changes or matches, focus moves, the screen stops changing, or some text appears on it |
| `find_text` | Finds text on the screen, or in one window, and returns up to 20 matches with their positions |
| `click_text` | Clicks text found on the screen, such as a button's label |
| `ui_find` | Finds buttons, links, fields and other elements in a window's accessibility tree, with their state and position |
| `ui_act` | Presses, checks or focuses an element from `ui_find` through the app itself, then reports its new state |
| `zoom` | Shows part of the screen at full resolution, for small text |
| `open_url` | Opens a page in your default browser, optionally in a private window, and returns a screenshot once it has loaded |
| `focus_window` | Brings one of your windows forward by name, switching workspace if needed. The agent must be working on your screen |
| `launch_app` | Starts an installed app by name |
| `list_windows` | Lists open windows with their workspace, title and position |
| `cursor_position` | Reports where the pointer is |
| `agent_display` | Starts, stops or reports on the agent's own screen. See [The agent display](#the-agent-display) |

`find_text`, `click_text` and `wait_for` text read the screen with [tesseract](https://github.com/tesseract-ocr/tesseract), which you install yourself: `sudo pacman -S tesseract tesseract-data-eng` (on Debian or Ubuntu, `sudo apt install tesseract-ocr`). `ui_find` and `ui_act` need your system Python with `python-gobject` and `at-spi2-core`, which most desktops already have. `hypr-computer check` reports both as optional.

Apps started with `launch_app` or `open_url` get their accessibility tree switched on (`ACCESSIBILITY_ENABLED=1`, `QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1`, and `--force-renderer-accessibility` for Chromium-based browsers). A browser that was already running keeps its old setting until you restart it. Terminals such as foot have no accessibility tree; `find_text` reads anything visible.

### The agent display

The agent works on a screen of its own, the agent display, so you can keep using yours. It starts by itself the first time the agent opens an app or a page, takes a screenshot or acts, unless that call names your screen. `agent_display(action="start")` starts it too, `action="stop"` removes it, and `action="status"` reports on it and on where to watch it. To have agents work on your screen, as in v0.7, set `HYPR_COMPUTER_WORKSPACE=user`.

- **Watching it.** When the agent display starts, `wl-mirror` shows a live view of it on your workspace 10. You stay on your workspace, and the live view never takes your focus. To see what the agent is doing, press Super+0 or run `hypr-computer watch`. To watch on another workspace, set `HYPR_COMPUTER_VIEW_WORKSPACE`. The live view closes when the agent display stops or the agent exits. It isn't on the agent display, so the agent's screenshots of its display never show it. While a password manager is on the agent display, the live view is blacked out of screenshots of your screen.
- **Without `wl-mirror`.** The agent still works on its display, and it tells you to run `sudo pacman -S wl-mirror`. `hypr-computer check` shows a WARN line for it.
- **Your screen.** The agent uses your screen only when a call names it: `target="user"` on `screenshot`, `zoom` or `find_text`, or `where="user"` on `launch_app` or `open_url`. After `target="user"`, later calls stay on your screen. The agent is told to do this only when the task needs your own windows or logged-in apps, and to ask you first. `focus_window` switches your workspace, so it's refused until the agent works on your screen.
- **What it is.** It's a virtual monitor, a Hyprland headless output named `hypr-computer`, showing a workspace of the same name. It's 1280x800 at scale 2, or narrower if you lower `HYPR_COMPUTER_MAX_EDGE`, so screenshots show it at full size and text on it is sharp enough to read. It sits 1,000 pixels above and to the left of your monitors, so your pointer can't wander onto it. Your monitors, workspaces, focus and pointer stay as they were. Apps on it can't take your focus, even when they ask to, as a browser does when it opens a page in a new tab.
- **Opening apps.** `launch_app` and `open_url` open apps on the agent display, where you see them only in the live view. With `where="user"` they open on your screen instead.
- **A browser of its own.** On the agent display, `open_url` runs Chromium-based browsers and Firefox with a separate profile in `$XDG_RUNTIME_DIR/hypr-computer-browser`. The page doesn't open in your browser's window, and it doesn't get your logins. The profile is deleted when you log out. A `HYPR_COMPUTER_BROWSER` command that names its own profile keeps that profile.
- **Seeing it.** `screenshot`, `zoom` and `find_text` take `target="agent"` or `target="user"`. Later calls stay on the display the agent saw last, and coordinates refer to it. `list_windows` marks the windows on the agent display. Password managers are blacked out there too.
- **Typing.** While the agent works on the agent display, `type_text` and `key` type into its window there. With several windows there, `window=` must name one. They never type into your focused window. Hyprland gives that window the keyboard for each key and then gives it back to your window, so your focus and pointer don't move. Your using the mouse doesn't pause this typing. Allowed apps and the deny-list apply to the window typed into.
- **Clicking.** There's one pointer, so a click or a scroll on the agent display borrows yours. Once you've left the mouse and keyboard alone for 2 seconds, the agent moves the pointer there, clicks, and puts back the pointer and your keyboard focus. On the M1, your pointer is away for less than 20 ms. While you're using the mouse or keyboard, the click is refused, and the agent is told to use keys or `ui_act`. `move` and `drag` work only on your screen.
- **Counting key presses.** To know when you're typing, the server adds a small handler to Hyprland that counts key presses, not which keys. Stopping the display removes it.
- **Stopping.** Stopping closes the live view and moves the display's windows to the hidden workspace `special:hypr-computer`. Starting it again brings them back. Once the agent stops it, the agent display starts again only when the agent starts it, or opens an app or a page. To see these windows yourself, run `hyprctl dispatch 'hl.dsp.workspace.toggle_special("hypr-computer")'`. Hyprland moves your pointer when it removes the display, and the server puts it back within a few milliseconds. The display also stops when the agent that started it exits. If an agent crashed and left one behind, `status` finds it by name, and the next `start` takes it over.

Its limits:

- Typing in the background takes ASCII text only, with US key positions.
- Each key typed in the background takes keyboard focus off your window for a moment, which can close a menu open in it.
- Chromium ignores some shortcuts, such as `ctrl+r`, while its window doesn't have focus.
- A key you press during a borrowed click can go to the agent's window.
- An app that's already running may open its new window on your screen.
- Tools that capture your whole desktop, rather than one monitor, also capture the agent display and the gap around it.
- A Hyprland config reload resets the display's size and place until the agent next uses it.
- `wait_for` with `title` or `focus` watches your focused window, not the windows on the agent display.
- Two agents that work on their own display share one agent display.

Every action takes `screenshot=true` to return the updated screen in the same call. `batch`, `open_url` and `wait_for` return one unless you pass `screenshot=false`. If the screen is identical to the last screenshot, the result says so instead of repeating the image. Each action's result also names the active window and the pointer position, and lists what Hyprland reported meanwhile: windows that opened or closed, focus and title changes. Coordinates are always pixels in the most recent screenshot.

## Settings

Everything works without configuration. To change a default, set one of these environment variables in your agent's MCP server settings:

| Variable | Default | Effect |
| --- | --- | --- |
| `HYPR_COMPUTER_DENY` | password managers | Regex of window classes and titles agents may not touch. Empty turns the deny-list off |
| `HYPR_COMPUTER_BROWSER` | your default browser | Browser command for `open_url`, such as `firefox` |
| `HYPR_COMPUTER_MAX_EDGE` | `1456` | Screenshot size (long edge, in pixels). Lower it for models with a smaller image limit |
| `HYPR_COMPUTER_SETTLE_MS` | `500` | The longest a screenshot right after an action waits for the screen to settle. It waits 120 to 300 ms by kind of action, longer while the window's title keeps changing, and the whole time when a window opens or closes |
| `HYPR_COMPUTER_WORKSPACE` | `agent` | `agent` has agents work on [the agent display](#the-agent-display) unless they ask for your screen. `user` has them work on your screen, as in v0.7 |
| `HYPR_COMPUTER_VIEW_WORKSPACE` | `10` | The workspace where you watch the agent display |
| `HYPR_COMPUTER_TOOLS` | `all` | `core` keeps only `screenshot`, the pointer and keyboard actions, `cursor_position` and `list_windows`, to save tokens. With `core`, agents work on your screen |
| `HYPR_COMPUTER_FILE_URLS` | none | Colon-separated folders whose files `open_url` may open as `file://` URLs. Without it, `open_url` opens only `http` and `https` URLs |
| `HYPR_COMPUTER_RECORD_DIR` | off | Saves every step, its timing and its screenshot to this folder, so you can review what an agent did. Grants and refused actions are saved too. The folder it creates and the files it writes are readable only by you. Typed text is saved as its length only |
| `HYPR_COMPUTER_SANDBOX` | `on` | `off` lets the server reach the network again, for debugging. See [Privacy](#privacy) |

## Privacy

Nothing about you or your screen leaves your machine through hypr-computer.

- **No telemetry, analytics or crash reports.** There's no tracking ID or account, and nothing to opt out of.
- **No network access, enforced.** The server connects only to your compositor, over local sockets, and runs only local tools (`grim`, `wtype`, `xkbcli`, `gtk-launch`, `xdg-mime`, `tesseract`, `wl-mirror`, and your system Python for the accessibility tree). The whole server is about 3,000 lines of Python in [`server.py`](../server.py), [`wl_input.py`](../wl_input.py) and [`atspi_helper.py`](../atspi_helper.py), so you can check this yourself. The server also cuts itself off from the network when it starts, as [the network sandbox](#the-network-sandbox) explains.
- **Reading text stays local.** Text recognition and the accessibility tree run on your machine. Text read from the screen reaches your agent like a screenshot does. Apps with accessibility on share their text with other programs running as you, as screen readers need.
- **Screenshots go only to your agent.** Your agent sends them to its model provider under that provider's terms. That part depends on the agent you choose.
- **Nothing is stored.** Screenshots stay in memory and never reach the disk, unless you turn on `HYPR_COMPUTER_RECORD_DIR`.
- **Network use is limited to installing and updating.** The installer and `hypr-computer update` download releases from GitHub, missing packages from your distribution, and the pinned Python dependencies (the MCP SDK and Pillow) from PyPI. A Claude Code plugin install downloads those dependencies on its first start instead. `uv` downloads them before the server starts, so the sandbox doesn't get in the way.

### The network sandbox

When the server starts, it moves itself into a new Linux user namespace and network namespace. The network namespace has no network interface except a loopback interface, which is down. Every program the server runs inherits the namespace, so `grim`, `wtype`, `tesseract`, `xdg-mime` and the accessibility helper can't reach the network either. Your files, Wayland, Hyprland's sockets and D-Bus work as before, because they are files or path sockets. The server keeps your user and group IDs.

- **Checked by `hypr-computer check`.** Its `network` line tries real connections to 1.1.1.1 and 2606:4700:4700::1111 from inside the sandbox and reports `blocked` when both fail. Nothing is sent, because the namespace has no route out.
- **Apps the agent opens keep their network.** `open_url` and `launch_app` ask Hyprland to start the app, so your browser runs outside the sandbox.
- **When the sandbox can't start, the server runs as before.** `hypr-computer check` shows a `WARN` line that says why, and the server writes the reason to standard error. This happens when your system doesn't let programs make user namespaces, or when `DBUS_SESSION_BUS_ADDRESS` names an abstract socket. Abstract sockets belong to one network namespace, so the sandbox would cut that bus off and stop `ui_find` and `ui_act`. A WARN line doesn't fail the check.
- **An accessibility bus on an abstract socket isn't detected.** Current AT-SPI uses a socket file in `$XDG_RUNTIME_DIR/at-spi`. If `ui_find` stops working in the sandbox on an older system, set `HYPR_COMPUTER_SANDBOX=off`.
- **The sandbox covers the network only.** Code in the server could still ask Hyprland to run a command outside the sandbox, as `launch_app` does.

## Safety

- **On-screen notice.** When an agent starts the agent display, or starts acting after a minute of quiet, a notification says "AI agent at work": on the agent display, with where to watch it; on your screen, that it's using your mouse and keyboard. It stays until you press **OK**, which lets the agent go on, or **Stop agent**, which stops every agent as `hypr-computer stop` does. When the agent's server exits, it changes to "AI agent done" and goes away. Without a notification service (`notify-send`), Hyprland shows the notice for a few seconds instead.
- **Stop switch.** `hypr-computer stop` blocks every tool call from all agents, screenshots included, until you run `hypr-computer resume`. A drag in progress stops at its next step. Agents are told to ask you before going on. To bind it to a key, add this to your Hyprland Lua config:
  ```lua
  hl.bind("SUPER + CTRL + ESCAPE", hl.dsp.exec_cmd("touch $XDG_RUNTIME_DIR/hypr-computer.stop"))
  ```
- **Allowed apps.** See [Allowed apps](#allowed-apps).
- **Pause when you take over.** If you move the mouse while an agent works, a `batch` stops before its next step. The agent's next click, key press or other action is refused once with "The user is using the mouse; ask before continuing". The server notices because the pointer isn't where the agent left it. These moves don't count:
  - Hyprland moving the pointer to the middle of a window that it focused.
  - A move after a minute with no action from the agent, so the first action of a new request goes ahead.

  Typing on your keyboard isn't noticed. Two agents working at the same time see each other's moves as yours. Typing on [the agent display](#the-agent-display) doesn't pause, and its clicks wait until you leave the mouse and keyboard alone.
- **Deny-list.** Agents can't click or type into 1Password, Bitwarden, KeePassXC, GNOME Secrets, Seahorse, Proton Pass, polkit or pinentry. Both the focused window and the window under the pointer are checked, for every point of a move or a drag too. A `batch` stops before its next step when one of these windows opens or comes to the front. Change the list with `HYPR_COMPUTER_DENY`.
- **Password managers never leave the machine.** Deny-listed windows are painted black in every image an agent gets and in what `find_text` reads. Their titles are hidden from window lists, events and status lines, `wait_for` never matches them, and `ui_find` never reads them. The whole screen is blacked out while one has focus (bars and its menus can show its title and contents anywhere), while one is in a window group (the group bar shows its title), and while windows are still moving around one. If Hyprland can't say where one is on screen, the capture is refused. A menu that a password manager leaves open after losing focus, reaching outside its window, isn't covered.
- **Web pages only.** `open_url` refuses `file://` URLs unless you list their folders in `HYPR_COMPUTER_FILE_URLS`.
- **Text on the screen is not an instruction.** The server tells agents that web pages, documents and messages are data, and that they should do only what you asked.
- **Your agent's approval prompts still apply.** Don't leave an agent unattended with sensitive apps open.

### Allowed apps

By default, agents may use every app except the deny-list. To keep them to the apps you name, give them a grant:

```bash
hypr-computer allow foot,firefox --for 30m
```

- **What a grant allows.** Agents can click, type, scroll, drag and press keys only in windows of these classes. `focus_window` and `ui_act` work only on them, and `launch_app` and `open_url` start only these apps. Screenshots, `find_text`, `ui_find`, `wait_for` and `list_windows` still read the whole screen.
- **Window classes** are the names that `hypr-computer allow --list` and `hyprctl clients` show, such as `foot`, `chromium` or `org.gnome.Nautilus`. Case doesn't matter.
- **Where the pointer acts.** The window under the pointer must be allowed. When a floating or fullscreen window is on top, that window counts. When two floating windows overlap, both must be allowed, because Hyprland doesn't say which one is on top. A point outside every window is refused, such as the bar, the desktop or a menu that reaches past its window's edge.
- **Key presses** go to the focused window, so it must be allowed. A key combo that Hyprland has a bind for goes to Hyprland instead, so it needs `hyprland` in the list too.
- **Refused actions** tell the agent which app it wasn't allowed to use, and to ask you.
- **Agents with a shell can change it.** A grant limits what an agent does through hypr-computer. An agent that can also run commands, such as Claude Code or Codex, could run `hypr-computer allow` itself, so keep your agent's approval prompts on for commands.
- **Time limits.** After `--for 30m`, `2h` or `1h30m` runs out, agents can only read the screen until you run `hypr-computer allow` again or turn grants off with `hypr-computer allow --off`. Without `--for`, a grant lasts until you turn it off or log out.
- **Where it is kept.** The grant is the file `$XDG_RUNTIME_DIR/hypr-computer.grant`, readable only by you. Every running agent follows it from its next action. If the file can't be read, agents can only read the screen.
