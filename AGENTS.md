# Agent instructions

## Setting up computer use for a user

If you're an agent asked to set up computer use on this machine:

1. **Check support.** Run `hyprctl version | head -1`. You need **Hyprland 0.55 or newer**. If `hyprctl` is missing or older, stop and tell the user that other compositors aren't supported. Running over SSH, with a `tty` or empty `$XDG_SESSION_TYPE`, is fine.
2. **Ask the user first.** Tell them what will be installed and wait for their OK. `sudo` is used only if `grim`, `wtype`, `xkbcli`, `uv` or `wl-mirror` is missing. To show them the exact plan, run the command below with `--dry-run` first.
3. **Install.** Replace `<client>` with the agent you're running in: `claude`, `codex`, `opencode`, `gemini` or `cursor`.
   ```bash
   curl -fsSL malik-na.github.io/hypr-computer | bash -s -- --yes --clients=<client>
   ```
   If `sudo` can't prompt from your environment, ask the user to run the command in a terminal.
4. **Read the output.** The self-check prints one PASS, WARN or FAIL line per check and ends with `hypr-computer is ready.` Each FAIL line names its fix. A WARN line doesn't stop the install.
5. **Other clients.** For VS Code, Zed, Goose, Cline or Windsurf, add the matching snippet from [Connecting your agent](docs/clients.md), with the absolute path `/home/<user>/.local/share/hypr-computer/server.py`.
6. **Restart, then smoke test.** Ask the user to restart the agent. Then call `list_windows` and `screenshot`. If you can see the screenshot, setup is done. The screenshot shows your own screen, the agent display. Tell the user they can watch it with Super+0 or `hypr-computer watch`. Don't click or type during the test unless the user asks.

## Working on the code

```bash
uv run --no-project --with pytest --with 'mcp<2' --with pillow pytest -q tests   # unit and MCP tests (also run in CI)
bash tests/e2e/update.sh                                          # install, update and rollback, on a Hyprland machine
python3 bench/run.py --host <machine> --setup-only                # every benchmark check must fail before an agent acts
scripts/release.sh patch                                          # release a fix: bump, wait for CI, tag, publish
```

- Test the installer safely with `bash install.sh --dry-run`.
- Release with `scripts/release.sh patch` (fixes) or `minor` (features). Never tag by hand.

## Rust rewrite

The rewrite lives in [`rust/`](rust), a Cargo workspace; [docs/design.md](docs/design.md) has the design. Run the checks from `rust/`:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace       # unit tests, black-box tests of the real binaries, structure checks
```

CI runs all three on x86-64 and ARM64. They need Linux, with pixman and libxkbcommon for the agent's screen (Debian and Ubuntu: `libpixman-1-dev libxkbcommon-dev`; Arch: `pixman libxkbcommon`).

- **Adding a tool** is one file, one registry line and its test: write `rust/crates/tools/src/<name>.rs`, add `mod <name>;` to `rust/crates/tools/src/lib.rs` and `&<name>::<Tool>,` to `TOOLS` in `rust/crates/tools/src/registry.rs`. Tools are called only through `Registry::call`, the one guard; checks every call needs go there, not in the tool.
- **`domain` stays pure:** no I/O, no async, no clock reads, no other workspace crate. Code that needs the outside world goes in `adapters`, behind a trait in `ports`.
- **No unsafe code, and no `unwrap`, `expect` or `panic` outside tests.** Lint attributes that switch these off fail the build.
- **Every stateful part needs a TLA+ spec in `specs/`** that fails first on a real past bug.
- **The agent's screen** is `rust/crates/screen`: the compositor binary, `hypr-computer-screen`, in `src/compositor/`, and the daemon's handle on it, which implements the `Screen` traits in `ports`. The daemon reaches a screen only through those traits. It runs the screen found next to its own binary, or `$HYPR_COMPUTER_SCREEN`.
- **Black-box tests** start the real daemon, shim and screen with `rust/tests/support`, which gives each test its own runtime dir and a cleared environment. `support::app` is a tiny Wayland app to put on a screen. Never point a test at the real desktop or the real `$XDG_RUNTIME_DIR`.
- **Tests and code come from different agents.** For each ticket a test agent writes the tests from the spec and pushes them to `tests/<ticket>` (they fail there: nothing is implemented). The implementation agent branches from that branch and opens the PR; it may not add, change or delete tests, and a wrong test is changed by the test agent on `tests/<ticket>`, then you rebase.
- **The PR body says which tests it was built on:** a line `Tests-From: <full sha of the tests/<ticket> head>`. CI (`scripts/check-tests-untouched.sh`, also runnable locally) fails a PR that changes tests and other files unless every test file matches that commit.
- **Test files** are everything under `tests/`, `rust/tests/` and `rust/crates/*/tests/`, plus `*.proptest-regressions`; no `#[test]`, `proptest!` or `#[cfg(test)]` goes into source files.
