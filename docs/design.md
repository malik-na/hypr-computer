# Design of the Rust rewrite

hypr-computer is being rewritten in Rust with a new architecture. The Python server (v0.8) stays the released version, getting bug fixes only, until the rewrite reaches v1.0.0. The Rust code lives in [`rust/`](../rust) on `main`.

## Architecture

- **One daemon per login session**, run as a socket-activated systemd user service. It listens on `$XDG_RUNTIME_DIR/hypr-computer/daemon.sock`, in a directory only the user can open.
- **A tiny shim per agent.** The agent starts the shim as its MCP server. The shim only joins its stdin and stdout to the daemon's socket. Each connection is one agent session, and the session ends the moment its connection closes, however the agent died.
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
