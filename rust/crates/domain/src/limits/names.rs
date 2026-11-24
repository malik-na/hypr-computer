//! The programs and windows the limits know by name: password managers and prompts, and web browsers.

/// The name an app limit gives the agent's browser, the one open_url starts: `hypr-computer allow browser`.
pub const BROWSER: &str = "browser";

/// What a password manager's or a password prompt's program file name or Wayland app id contains.
const PASSWORD_MANAGERS: &[&str] = &[
    "1password",
    "bitwarden",
    "keepass",
    "lastpass",
    "enpass",
    "dashlane",
    "proton-pass",
    "protonpass",
    "proton pass",
    "seahorse",
    "org.gnome.world.secrets",
    "gnome-secrets",
    "passwordsafe",
    "kwallet",
    "pinentry",
    "polkit",
    "gcr-prompter",
    "askpass",
];

/// The file names of web browsers' programs, and `browser`.
const BROWSERS: &[&str] = &[
    BROWSER,
    "chromium",
    "chromium-browser",
    "chrome",
    "google-chrome",
    "google-chrome-stable",
    "google-chrome-beta",
    "google-chrome-unstable",
    "brave",
    "brave-browser",
    "microsoft-edge",
    "microsoft-edge-stable",
    "vivaldi",
    "vivaldi-stable",
    "opera",
    "thorium-browser",
    "firefox",
    "firefox-esr",
    "librewolf",
    "zen-browser",
    "floorp",
    "epiphany",
    "falkon",
    "qutebrowser",
];

/// Whether `name`, a program's file name or a window's app id, is a password manager or a password prompt.
/// Case-insensitive; true when it contains any of a fixed list of names.
pub fn is_password_manager(name: &str) -> bool {
    let name = name.to_lowercase();
    PASSWORD_MANAGERS.iter().any(|known| name.contains(known))
}

/// Whether `program` (a file name, or a path, whose file name counts) is a web browser, which only open_url
/// may start, pointed at the proxy. Case-insensitive, the whole file name.
pub fn is_browser(program: &str) -> bool {
    let name = program.rsplit('/').next().unwrap_or(program).to_lowercase();
    BROWSERS.contains(&name.as_str())
}
