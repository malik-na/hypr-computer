# Design of the Rust rewrite

hypr-computer is being rewritten in Rust with a new architecture. The Python server (v0.8) stays the released version, getting bug fixes only, until the rewrite reaches v1.0.0. The Rust code lives in [`rust/`](../rust) on `main`.

## Architecture

- **One daemon per login session**, run as a socket-activated systemd user service. It listens on `$XDG_RUNTIME_DIR/hypr-computer/daemon.sock`, in a directory only the user can open.
- **A tiny shim per agent.** The agent starts the shim as its MCP server. The shim only joins its stdin and stdout to the daemon's socket. Each connection is one agent session, and the session ends the moment its connection closes, however the agent died.
- **The `hypr-computer` commands** (`watch` first) ask the daemon on a socket of their own, `control.sock` next to `daemon.sock`, and print its answer. The daemon owns the sessions, so it does the work.
- **Each agent session gets its own screen**, never the user's. The screen is our own compositor, written in Rust with [Smithay](https://github.com/Smithay/smithay): one screen process per agent, on the GPU with a software fallback, one app full screen at a time, 1280x800 by default (configurable). A throwaway Smithay prototype must first pass the checks the nested-Hyprland prototype passed. If it can't, the fallback is nested Hyprland behind the same `Screen` interface.
- **Crates:** `domain` (state and rules, pure), `ports` (traits for the outside world), `adapters` (their real implementations), `tools` (one file per MCP tool and a registry), `daemon` and `shim`. There is no global mutable state: one task owns the sessions and others send it messages. Every tool call passes through one guard in the registry.

## Product

- **Hyprland first.** Code that talks to the host compositor sits behind one interface, so other hosts can follow.
- **The agent never works on the user's real screen.** Targeting the user's screen is gone for good.
- **Live view.** Every agent's screen is tiled on workspace 10, watch-only. Clicking into one takes it over: the agent's calls wait until the user gives it back with Escape twice or `hypr-computer give`.
- **Clipboard.** Each agent's clipboard is separate from the user's, and shared only during a take-over.
- **Browser.** A fresh profile by default; named, saved profiles are opt-in. Chromium-family browsers first.
- **Later:** X11 apps and a file sandbox.
- The name stays hypr-computer.

## Safety

- By default an agent may do anything on its own screen.
- Opt-in limits: allowed apps, allowed websites (through a proxy in the daemon), read-only mode, and password managers blocked.
- A stop and a notification per agent, plus stop-all.
- Every action is logged. Screenshots in the log are opt-in.

### Limits

`hypr-computer allow` sets the limits every agent works within (`domain::limits`, specs/Limits.tla). Each `allow` replaces the limits before it; `--for 30m` ends them after that long, and then nothing is limited; `--off` ends them at once; `--list` shows them.

- **Allowed apps.** `allow notes,foot browser` lets agents launch only those programs (by file name, any case) and give input only while a window of one of them is in front: one made by the process `launch_app` started, or by one of its children. `browser` is the one `open_url` starts.
- **Read-only.** `allow --read-only` lets agents only look: every tool that `tools/list` doesn't mark `readOnlyHint: true` (today, all but `screenshot`) is refused. It can't be given with apps or sites.
- **Allowed sites.** `allow --sites example.com,docs.rs` keeps agents' browsers to those sites and their subdomains (an IP address matches only itself), any port, http or https. The daemon runs a filtering HTTP proxy on 127.0.0.1 (`crates/daemon/src/proxy.rs`), and the browser is started pointed at it, with loopback pages going through it too. The proxy goes by the host in the request line, never the `Host` header. It checks the limits at every request, before it resolves or connects anything, and passes plain-http requests on one per connection so that every one is checked. https goes through a `CONNECT` tunnel, so only its host and port are checked. A refused request gets 403 and a page that names the site. `open_url` of an unlisted site is refused before the browser asks for it, and so is a call whose page lands on one after redirects. A tunnel that is already open stays open when the limits change.
- **Password managers, always.** Whatever the limits, a password manager or password prompt is never launched and never shown. `launch_app` of one is refused by its program's file name, and so is a launch whose arguments name one (a terminal running a vault's command line, say). The screen vets every window before it is first configured, and again when its app id changes, by app id and by the program of the process that made it. It never maps a password manager's window, and it kills that process at once, whoever started it. Vetting and drawing share one event loop, so a screenshot can't catch one in between, which was v0.8's capture race. One is known by a fixed list of names (`domain::limits::is_password_manager`), so a vault under an unknown name and app id isn't caught.
- **Where they live.** The limits are a file in the runtime dir, next to the daemon's socket, which the command writes and the daemon reads at every check. So a limit set mid-session applies to every agent's next call, `allow` works while no daemon runs, and a restart of the daemon doesn't lift them. A file the daemon can't read or trust counts as read-only, with no app or site allowed.
- **The guard checks them.** Each tool says what it does (`Tool::effect`: look, input, launch, open), and the guard asks the limits at every call, after the session has let the call through. A refused call is a tool error that says what was refused and tells the agent to ask the user.
- **Not a sandbox.** Agents' apps run as the user. A program on the screen, or an agent with a shell of its own, can change the limits file or run `hypr-computer allow` itself.

## Process

- Tools are redesigned from benchmark evidence, starting from the shape of Anthropic's computer-use tool.
- Test-driven: a test fails first, then the code makes it pass.
- Every stateful part (session and screen lifecycle, take-over, stop, notices) gets a TLA+ model in `specs/`, and each model must first fail on a real bug from the past.
- One GitHub issue per slice of work. Pull requests merge only after they're proven on their head commit; releases need the maintainer's approval.
- Prebuilt binaries for ARM64 and x86-64. Live tests on real Apple Silicon Linux machines before each release.
- **Cutover:** automatic at v1.0.0, with no previews. The bar for v1.0: benchmarks at least as good as v0.8, the live suite passing on every test machine, and every safety rule tested.

## What the prototypes taught

A nested compositor as the agent's screen:

- A compositor whose only output is a window on the host stops drawing while that window is hidden, so screenshots hang. The agent must work on a headless output, which the host window only mirrors.
- The host's pointer leaks into a nested compositor while it hovers the live view. The view must not take input from the host's devices unless the user takes over.
- Screen startup has a class of ordering bugs: focus can land on an output with no workspace, and then new apps never get a window. A screen is handed to the agent only once it is verified ready.

The daemon and shim: with two agents running, killing one with `kill -9` removed its screen, apps and server within about a tenth of a second, and the daemon and the other agent carried on.

Bugs of the Python version the design must make impossible:

- A server that didn't exit on SIGTERM or SIGINT left its display behind.
- The agent's browser windows opened on the user's workspace, and clicks were refused because the user was using the mouse.
- A browser extension installed system-wide opened its own pages in the agent's profile.
- Tests acted on the real desktop of the machine running them.
