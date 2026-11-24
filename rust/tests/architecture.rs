//! Checks on the workspace's structure that the compiler can't make. Each
//! failure says what to change.
#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// Crates `domain` may depend on, each with why it is pure. Add one only if
/// it does no I/O and needs no async runtime.
const DOMAIN_MAY_USE: &[(&str, &str)] = &[];

/// Things `domain`'s code may not mention, and why.
const DOMAIN_MUST_NOT_USE: &[(&str, &str)] = &[
    ("std::fs", "files are I/O"),
    ("std::net", "sockets are I/O"),
    ("std::io", "I/O"),
    ("std::os", "OS calls are I/O"),
    ("std::process", "processes are I/O"),
    ("std::thread", "threads belong to the daemon"),
    (
        "std::env",
        "the environment is input; take the value as an argument",
    ),
    ("tokio", "async belongs to the daemon"),
    ("async ", "async belongs to the daemon"),
    (".await", "async belongs to the daemon"),
    (
        "Instant::now",
        "reading the clock is input; use ports::Clock and pass the time in",
    ),
    (
        "SystemTime::now",
        "reading the clock is input; use ports::Clock and pass the time in",
    ),
];

/// Files in tools/src that aren't tools.
const TOOLS_NOT_A_TOOL: &[&str] = &["lib.rs", "registry.rs", "args.rs"];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn metadata() -> Value {
    let out = Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ])
        .arg("--manifest-path")
        .arg(workspace().join("Cargo.toml"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn packages(meta: &Value) -> Vec<&Value> {
    meta["packages"].as_array().unwrap().iter().collect()
}

fn package<'a>(meta: &'a Value, name: &str) -> &'a Value {
    packages(meta)
        .into_iter()
        .find(|p| p["name"] == name)
        .unwrap_or_else(|| panic!("no `{name}` crate in the workspace"))
}

fn relative(path: &Path) -> String {
    path.strip_prefix(workspace())
        .unwrap_or(path)
        .display()
        .to_string()
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(rust_files(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            files.push(path);
        }
    }
    files.sort();
    files
}

/// Lines of non-test code: everything before the file's `#[cfg(test)]`
/// module, without comments.
fn code_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .take_while(|(_, l)| l.trim() != "#[cfg(test)]")
        .map(|(i, l)| (i + 1, l.split("//").next().unwrap_or("")))
}

fn report(problems: Vec<String>) {
    assert!(problems.is_empty(), "\n\n{}\n", problems.join("\n\n"));
}

#[test]
fn domain_depends_on_no_other_workspace_crate() {
    let meta = metadata();
    let members: Vec<&str> = packages(&meta)
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    let mut problems = Vec::new();
    for dep in package(&meta, "domain")["dependencies"].as_array().unwrap() {
        let name = dep["name"].as_str().unwrap();
        if members.contains(&name) {
            problems.push(format!(
                "domain depends on the workspace crate `{name}`. domain is the bottom layer: \
                 remove `{name}` from rust/crates/domain/Cargo.toml, and move the type domain \
                 needs into domain or the code that needs `{name}` out of it."
            ));
        }
    }
    report(problems);
}

#[test]
fn domain_depends_only_on_pure_crates() {
    let meta = metadata();
    let mut problems = Vec::new();
    for dep in package(&meta, "domain")["dependencies"].as_array().unwrap() {
        let name = dep["name"].as_str().unwrap();
        let is_member = packages(&meta).iter().any(|p| p["name"] == name);
        if dep["kind"] == "dev" || is_member || DOMAIN_MAY_USE.iter().any(|(n, _)| *n == name) {
            continue;
        }
        problems.push(format!(
            "domain depends on `{name}`. domain must stay pure: no I/O, no async. Remove \
             `{name}` from rust/crates/domain/Cargo.toml and put the code that needs it in \
             `adapters`, behind a trait in `ports`. If `{name}` really does no I/O and needs no \
             async runtime, add it to DOMAIN_MAY_USE in rust/tests/architecture.rs with the reason."
        ));
    }
    report(problems);
}

#[test]
fn domain_code_does_no_io_and_no_async() {
    let src = workspace().join("crates/domain/src");
    let mut problems = Vec::new();
    for file in rust_files(&src) {
        let text = fs::read_to_string(&file).unwrap();
        for (line, code) in code_lines(&text) {
            for (pattern, why) in DOMAIN_MUST_NOT_USE {
                if code.contains(pattern) {
                    problems.push(format!(
                        "{}:{line} uses `{pattern}` ({why}). domain is pure: take the value as an \
                         argument, or add a trait to `ports`, implement it in `adapters`, and call \
                         it from the daemon.",
                        relative(&file)
                    ));
                }
            }
        }
    }
    report(problems);
}

#[test]
fn every_file_in_tools_is_a_registered_tool() {
    let src = workspace().join("crates/tools/src");
    let lib = fs::read_to_string(src.join("lib.rs")).unwrap();
    let registry = fs::read_to_string(src.join("registry.rs")).unwrap();
    let start = registry
        .find("const TOOLS: &[&dyn Tool] = &[")
        .expect("registry.rs has `const TOOLS: &[&dyn Tool] = &[`");
    let tools_list = &registry[start..];
    let tools_list = &tools_list[..tools_list.find("];").expect("TOOLS ends with `];`")];

    let mut problems = Vec::new();
    for entry in fs::read_dir(&src).unwrap() {
        let path = entry.unwrap().path();
        let file = path.file_name().unwrap().to_string_lossy().to_string();
        if TOOLS_NOT_A_TOOL.contains(&file.as_str()) {
            continue;
        }
        let module = file.trim_end_matches(".rs").to_owned();
        let declared = code_lines(&lib).any(|(_, l)| {
            let l = l.trim();
            l == format!("mod {module};") || l == format!("pub mod {module};")
        });
        let listed = code_lines(tools_list).any(|(_, l)| l.contains(&format!("{module}::")));
        if !(declared && listed) {
            problems.push(format!(
                "rust/crates/tools/src/{file} is not a registered tool. A tool is one file plus \
                 two lines: add `mod {module};` to rust/crates/tools/src/lib.rs and \
                 `&{module}::<YourTool>,` to TOOLS in rust/crates/tools/src/registry.rs, then \
                 give it a test. If it isn't a tool, it doesn't belong in tools/src."
            ));
        }
    }
    report(problems);
}

#[test]
fn every_crate_uses_the_workspace_lints() {
    let meta = metadata();
    let mut problems = Vec::new();
    for p in packages(&meta) {
        let manifest = PathBuf::from(p["manifest_path"].as_str().unwrap());
        let parsed: toml::Table = fs::read_to_string(&manifest).unwrap().parse().unwrap();
        let opted_in = parsed
            .get("lints")
            .and_then(|l| l.get("workspace"))
            .and_then(toml::Value::as_bool)
            == Some(true);
        if !opted_in {
            problems.push(format!(
                "{} doesn't use the workspace's lints (no unsafe code, no unwrap/expect/panic \
                 outside tests). Add to it:\n\n[lints]\nworkspace = true",
                relative(&manifest)
            ));
        }
    }
    report(problems);
}

#[test]
fn the_workspace_lints_stay_strict() {
    let manifest = workspace().join("Cargo.toml");
    let parsed: toml::Table = fs::read_to_string(&manifest).unwrap().parse().unwrap();
    let lints = &parsed["workspace"]["lints"];
    let wanted = [
        ("rust", "unsafe_code", "forbid"),
        ("clippy", "unwrap_used", "deny"),
        ("clippy", "expect_used", "deny"),
        ("clippy", "panic", "deny"),
    ];
    let mut problems = Vec::new();
    for (group, lint, level) in wanted {
        let got = lints
            .get(group)
            .and_then(|g| g.get(lint))
            .and_then(toml::Value::as_str);
        if got != Some(level) {
            problems.push(format!(
                "rust/Cargo.toml must set `{lint} = \"{level}\"` under [workspace.lints.{group}] \
                 (found {got:?}). Put it back; handle the error with `?` or a match instead."
            ));
        }
    }
    report(problems);
}

#[test]
fn every_crate_root_forbids_unsafe_code() {
    let meta = metadata();
    let mut problems = Vec::new();
    for p in packages(&meta) {
        for target in p["targets"].as_array().unwrap() {
            if target["kind"]
                .as_array()
                .unwrap()
                .iter()
                .any(|k| k == "custom-build")
            {
                continue;
            }
            let root = PathBuf::from(target["src_path"].as_str().unwrap());
            let text = fs::read_to_string(&root).unwrap();
            if !text.lines().any(|l| l.trim() == "#![forbid(unsafe_code)]") {
                problems.push(format!(
                    "{} is a crate root without `#![forbid(unsafe_code)]`. Add that line after \
                     its `//!` doc comment.",
                    relative(&root)
                ));
            }
        }
    }
    report(problems);
}

#[test]
fn no_code_switches_off_the_strict_lints() {
    let lints = [
        "unsafe_code",
        "clippy::unwrap_used",
        "clippy::expect_used",
        "clippy::panic",
    ];
    let mut problems = Vec::new();
    for crate_dir in fs::read_dir(workspace().join("crates")).unwrap() {
        let src = crate_dir.unwrap().path().join("src");
        if !src.is_dir() {
            continue;
        }
        for file in rust_files(&src) {
            let text = fs::read_to_string(&file).unwrap();
            for (line, code) in code_lines(&text) {
                let code = code.trim();
                let is_attr = code.starts_with("#[") || code.starts_with("#![");
                let relaxes = ["allow(", "expect(", "warn("]
                    .iter()
                    .any(|a| code.contains(a));
                if let Some(lint) = lints.iter().find(|l| code.contains(*l))
                    && is_attr
                    && relaxes
                {
                    problems.push(format!(
                        "{}:{line} switches off `{lint}`. Remove the attribute and handle the \
                         error with `?`, a match, or a default. Tests may unwrap without it \
                         (rust/clippy.toml allows it in #[test] and #[cfg(test)] code).",
                        relative(&file)
                    ));
                }
            }
        }
    }
    report(problems);
}
