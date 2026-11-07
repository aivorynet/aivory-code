//! The helpers behind the action methods of the desktop controls (`repos/add`, `project/new`,
//! `providers/addAcp`, `settings/importClaude`, `daemon/restart`). The live and the demo backend
//! share them, so that both answer the same way. Only `serde` and `std` do the work here: the
//! backends keep the state and send the deltas.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use aivory_protocol::rpc::{RpcError, code};
use aivory_protocol::{
    AcpAgent, AddAcpParams, Codebase, Effort, ImportChange, ImportReport, Mode, Part,
    ProjectNewParams, ProjectTemplate, ProviderInfo, ProviderPrefs, ProviderStatus, Settings,
};
use serde::Deserialize;
use serde_json::{Value, json};

/// The longest wait for the `--version` run of an ACP agent.
const DETECT_WAIT: Duration = Duration::from_secs(3);
/// The longest folder name of `project/new`.
const MAX_NAME: usize = 100;

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, message)
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The root of the git repository that holds `path`. A clear error when it is no repository.
pub fn git_root(path: &str) -> Result<PathBuf, RpcError> {
    let path = path.trim();
    if path.is_empty() {
        return Err(invalid("give the path of a folder"));
    }
    let dir = Path::new(path);
    if !dir.is_dir() {
        return Err(invalid(format!("'{path}' is not a folder")));
    }
    let top = git(dir, &["rev-parse", "--show-toplevel"]).filter(|t| !t.is_empty());
    let Some(top) = top else {
        return Err(invalid(format!(
            "'{path}' is not a git repository: run `git init` in it first"
        )));
    };
    // Git prints `C:/a/b`; the scanner uses `C:\a\b`. Rebuilding the path unifies them.
    let root: PathBuf = PathBuf::from(top).components().collect();
    // A git folder in the home folder (dotfile repositories) or at a drive root does not make
    // a codebase, as in the war room scanner.
    let home = std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).ok();
    let shown = root.display().to_string();
    if root.parent().is_none() || home.is_some_and(|h| same_path(&h, &shown)) {
        return Err(invalid(format!(
            "'{path}' is not in a git repository of its own: the repository of the home folder or of a drive does not count"
        )));
    }
    Ok(root)
}

/// The error of a map change: a bad input is `INVALID_PARAMS`, a disk problem `SERVER_ERROR`.
pub fn map_error(e: aivory_map::MapError) -> RpcError {
    use aivory_map::MapError;
    match e {
        MapError::Io { .. } | MapError::Exists { .. } => {
            RpcError::new(code::SERVER_ERROR, e.to_string())
        }
        other => invalid(other.to_string()),
    }
}

/// The branch of the repository, or an empty text for a repository without a commit.
pub fn branch_of(root: &Path) -> String {
    git(root, &["symbolic-ref", "--short", "HEAD"]).unwrap_or_default()
}

/// The parts of a repository: `.aivory/map.toml`, or the guess of `aivoryd map init`. Nothing
/// is written.
pub fn parts_of(root: &Path) -> Vec<Part> {
    let mut parts = aivory_map::load(root)
        .unwrap_or_else(|_| aivory_map::init(root))
        .parts;
    if parts.is_empty() {
        parts.push(Part {
            name: "root".into(),
            paths: vec!["**".into()],
        });
    }
    parts
}

/// The codebase of a repository root, as the war room shows it.
pub fn codebase_of(root: &Path, id: String, daemon: &str) -> Codebase {
    Codebase {
        id,
        path: root.display().to_string(),
        branch: branch_of(root),
        worktrees: 0,
        daemon: daemon.to_string(),
        languages: Vec::new(),
        parts: parts_of(root),
    }
}

/// True when two paths name the same folder (case-insensitive on Windows, `/` and `\` equal).
pub fn same_path(a: &str, b: &str) -> bool {
    let norm = |p: &str| {
        let p = p.replace('\\', "/");
        let p = p.trim_end_matches('/').to_string();
        if cfg!(windows) { p.to_lowercase() } else { p }
    };
    norm(a) == norm(b)
}

/// `base`, or `base-2`, `base-3`, ... until `taken` says no.
pub fn unique_id(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|id| !taken(id))
        .unwrap_or_else(|| base.to_string())
}

/// Check the name of a new project folder.
fn check_name(name: &str) -> Result<&str, RpcError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("give a project name"));
    }
    if name.len() > MAX_NAME {
        return Err(invalid(format!(
            "the project name has more than {MAX_NAME} characters"
        )));
    }
    if name == "." || name == ".." || name.ends_with('.') {
        return Err(invalid("the project name is not a valid folder name"));
    }
    if name.chars().any(|c| {
        c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
    }) {
        return Err(invalid(
            "the project name must not hold a path separator or one of < > : \" | ? *",
        ));
    }
    let stem = name.split('.').next().unwrap_or(name).to_ascii_lowercase();
    if matches!(
        stem.as_str(),
        "con" | "prn" | "aux" | "nul" | "com1" | "com2" | "com3" | "lpt1" | "lpt2" | "lpt3"
    ) {
        return Err(invalid("the project name is a reserved name of Windows"));
    }
    Ok(name)
}

/// The files of a template: relative path and text. Every template has a README and a
/// `.gitignore`.
fn template_files(name: &str, template: ProjectTemplate) -> Vec<(&'static str, String)> {
    let readme = format!("# {name}\n");
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    match template {
        ProjectTemplate::Empty => vec![
            ("README.md", readme),
            (".gitignore", "*.log\n.env\n".to_string()),
        ],
        ProjectTemplate::Rust => vec![
            ("README.md", readme),
            (".gitignore", "/target\n".to_string()),
            (
                "Cargo.toml",
                format!("[package]\nname = \"{slug}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n"),
            ),
            (
                "src/main.rs",
                "fn main() {\n    println!(\"Hello\");\n}\n".to_string(),
            ),
        ],
        ProjectTemplate::Node => vec![
            ("README.md", readme),
            (".gitignore", "node_modules/\n.env\n".to_string()),
            (
                "package.json",
                format!(
                    "{{\n  \"name\": \"{slug}\",\n  \"version\": \"0.1.0\",\n  \"main\": \"index.js\"\n}}\n"
                ),
            ),
            ("index.js", "console.log('Hello');\n".to_string()),
        ],
        ProjectTemplate::Python => vec![
            ("README.md", readme),
            (
                ".gitignore",
                "__pycache__/\n.venv/\n*.pyc\n.env\n".to_string(),
            ),
            (
                "pyproject.toml",
                format!("[project]\nname = \"{slug}\"\nversion = \"0.1.0\"\n"),
            ),
            ("main.py", "print(\"Hello\")\n".to_string()),
        ],
    }
}

/// Create `<parent>/<name>`: `git init`, the template files, and `.aivory/map.toml`. Returns
/// the root. A folder that exists and holds files stays untouched (`INVALID_STATE`).
pub fn create_project(p: &ProjectNewParams) -> Result<PathBuf, RpcError> {
    let name = check_name(&p.name)?;
    let parent = Path::new(p.parent.trim());
    if p.parent.trim().is_empty() || !parent.is_dir() {
        return Err(invalid(format!("'{}' is not a folder", p.parent)));
    }
    let root = parent.join(name);
    let existed = root.exists();
    if existed {
        let empty = root.is_dir()
            && std::fs::read_dir(&root)
                .map(|mut d| d.next().is_none())
                .unwrap_or(false);
        if !empty {
            return Err(RpcError::new(
                code::INVALID_STATE,
                format!("'{}' exists and is not empty", root.display()),
            ));
        }
    }
    let template = p.template.unwrap_or_default();
    let write = || -> Result<(), RpcError> {
        let io_err = |what: &str, e: io::Error| {
            RpcError::new(code::SERVER_ERROR, format!("cannot {what}: {e}"))
        };
        std::fs::create_dir_all(&root).map_err(|e| io_err("create the folder", e))?;
        let init = Command::new("git")
            .arg("-C")
            .arg(&root)
            .arg("init")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if !init.is_ok_and(|s| s.success()) {
            return Err(RpcError::new(
                code::SERVER_ERROR,
                "`git init` failed: is git installed?",
            ));
        }
        for (rel, text) in template_files(name, template) {
            let file = root.join(rel);
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir).map_err(|e| io_err("create a folder", e))?;
            }
            std::fs::write(&file, text).map_err(|e| io_err(&format!("write {rel}"), e))?;
        }
        let map = aivory_map::init(&root);
        aivory_map::write(&root, &map, false)
            .map_err(|e| RpcError::new(code::SERVER_ERROR, e.to_string()))?;
        // Stage the files so that the file tree shows them. A failure does not matter.
        let _ = git(&root, &["add", "-A"]);
        Ok(())
    };
    if let Err(e) = write() {
        // Take back what this call made.
        if existed {
            if let Ok(entries) = std::fs::read_dir(&root) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let _ = if path.is_dir() {
                        std::fs::remove_dir_all(path)
                    } else {
                        std::fs::remove_file(path)
                    };
                }
            }
        } else {
            let _ = std::fs::remove_dir_all(&root);
        }
        return Err(e);
    }
    Ok(root.components().collect())
}

// ---- ACP agents --------------------------------------------------------------------------

/// Check the input of `providers/addAcp` and make the agent. `taken` tells if an id exists.
pub fn acp_agent(p: &AddAcpParams, taken: impl Fn(&str) -> bool) -> Result<AcpAgent, RpcError> {
    let name = p.name.trim();
    let command = p.command.trim();
    if name.is_empty() || command.is_empty() {
        return Err(invalid("give a name and a command"));
    }
    if command.contains('\0') || p.args.iter().any(|a| a.contains('\0')) {
        return Err(invalid("the command and the arguments must be plain text"));
    }
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if slug.is_empty() {
        return Err(invalid("the name needs a letter or a digit"));
    }
    let id = format!("acp-{slug}");
    if taken(&id) {
        return Err(invalid(format!("the provider '{id}' exists already")));
    }
    let env = p.env.clone().unwrap_or_default();
    for key in env.keys() {
        let ok = key
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !ok {
            return Err(invalid(format!(
                "'{key}' is not an environment variable name"
            )));
        }
    }
    Ok(AcpAgent {
        id,
        name: name.to_string(),
        command: command.to_string(),
        args: p.args.clone(),
        env,
    })
}

/// True when `command` is a file, or a program on the PATH.
pub fn command_exists(command: &str) -> bool {
    let has_sep = command.contains('/') || command.contains('\\');
    if has_sep {
        return Path::new(command).is_file();
    }
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into())
            .split(';')
            .map(str::to_string)
            .collect()
    } else {
        vec![String::new()]
    };
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            exts.iter().any(|ext| {
                let file = dir.join(format!("{command}{ext}"));
                file.is_file()
            }) || dir.join(command).is_file()
        })
    })
}

/// Run `<command> --version` (3 s at most). Returns whether the agent exists and the first line
/// of its output. A command that cannot start still counts as found when its file exists.
pub fn acp_detect(agent: &AcpAgent) -> (bool, Option<String>) {
    let started = Command::new(&agent.command)
        .arg("--version")
        .envs(&agent.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = started else {
        return (command_exists(&agent.command), None);
    };
    let begin = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if begin.elapsed() < DETECT_WAIT => {
                std::thread::sleep(Duration::from_millis(25));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let version = status.filter(|s| s.success()).and_then(|_| {
        let out = child.wait_with_output().ok()?;
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(|l| l.chars().take(120).collect::<String>())
    });
    (true, version)
}

/// The settings card of an ACP agent. `seen` is the result of the last detection, if any.
pub fn acp_card(agent: &AcpAgent, seen: Option<&(bool, Option<String>)>) -> ProviderInfo {
    let found = seen.map_or_else(|| command_exists(&agent.command), |s| s.0);
    let detail = match seen.and_then(|s| s.1.as_deref()) {
        Some(version) => version.to_string(),
        None if found => "found".to_string(),
        None => "command not found".to_string(),
    };
    ProviderInfo {
        id: agent.id.clone(),
        name: agent.name.clone(),
        monogram: agent
            .name
            .chars()
            .next()
            .map_or("?".to_string(), |c| c.to_uppercase().to_string()),
        auth: format!("ACP agent · {} · {detail}", agent.command),
        status: ProviderStatus::Acp,
        models: Vec::new(),
        usage: Vec::new(),
    }
}

/// The default settings of a new ACP provider.
pub fn acp_prefs() -> ProviderPrefs {
    ProviderPrefs {
        enabled: true,
        model: String::new(),
        effort: Effort::High,
        mode: Mode::Edit,
    }
}

// ---- Import from Claude Code ------------------------------------------------------------

/// The two keys that the import reads. Serde skips every other key of the file, and the type
/// has no field for a token.
#[derive(Debug, Deserialize)]
struct ClaudeSettingsPeek {
    model: Option<Value>,
    permissions: Option<PermissionsPeek>,
}

#[derive(Debug, Deserialize)]
struct PermissionsPeek {
    #[serde(rename = "defaultMode")]
    default_mode: Option<Value>,
}

/// The mode of the composer for a `permissions.defaultMode` of Claude Code. Claude Code has
/// `default`, `acceptEdits`, `plan`, `bypassPermissions`. The daemon never imports the bypass.
fn mode_for(default_mode: &str) -> Option<Mode> {
    match default_mode {
        "plan" => Some(Mode::Plan),
        "default" => Some(Mode::Edit),
        "acceptEdits" => Some(Mode::Auto),
        _ => None,
    }
}

/// Read `model` and `permissions.defaultMode` from `<claude_home>/settings.json` and set
/// `providers.claude.model` and `providers.claude.mode`. Reads nothing else. Never writes to
/// the Claude folder.
pub fn import_claude(claude_home: &Path, settings: &mut Settings) -> ImportReport {
    let file = claude_home.join("settings.json");
    let mut report = ImportReport {
        found: false,
        changes: Vec::new(),
        skipped: Vec::new(),
        settings: settings.clone(),
    };
    let Ok(text) = std::fs::read_to_string(&file) else {
        report
            .skipped
            .push("settings.json of Claude Code not found".into());
        return report;
    };
    let Ok(peek) = serde_json::from_str::<ClaudeSettingsPeek>(&text) else {
        report
            .skipped
            .push("settings.json of Claude Code is no JSON object".into());
        return report;
    };
    report.found = true;
    let prefs = settings
        .providers
        .entry("claude".to_string())
        .or_insert_with(|| ProviderPrefs {
            enabled: true,
            model: "opus".to_string(),
            effort: Effort::High,
            mode: Mode::Edit,
        });
    match peek.model {
        Some(Value::String(model)) if !model.trim().is_empty() => {
            let model = model.trim().to_string();
            if prefs.model != model {
                report.changes.push(ImportChange {
                    key: "providers.claude.model".into(),
                    from: Some(json!(prefs.model)),
                    to: json!(model),
                });
                prefs.model = model;
            }
        }
        Some(_) => report.skipped.push("model is not a text".into()),
        None => report.skipped.push("no model in the file".into()),
    }
    match peek.permissions.and_then(|p| p.default_mode) {
        Some(Value::String(text)) => match mode_for(&text) {
            Some(mode) => {
                if prefs.mode != mode {
                    report.changes.push(ImportChange {
                        key: "providers.claude.mode".into(),
                        from: serde_json::to_value(prefs.mode).ok(),
                        to: serde_json::to_value(mode).unwrap_or(Value::Null),
                    });
                    prefs.mode = mode;
                }
            }
            None => report
                .skipped
                .push(format!("permissions.defaultMode '{text}' has no match")),
        },
        Some(_) => report
            .skipped
            .push("permissions.defaultMode is not a text".into()),
        None => report.skipped.push("no permissions.defaultMode".into()),
    }
    report.settings = settings.clone();
    report
}

// ---- Daemon ------------------------------------------------------------------------------

/// The name of this machine.
pub fn host_name() -> String {
    ["COMPUTERNAME", "HOSTNAME", "HOST"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))
        .or_else(|| {
            let out = Command::new("hostname")
                .stdin(Stdio::null())
                .output()
                .ok()?;
            let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
            (!text.is_empty()).then_some(text)
        })
        .unwrap_or_else(|| "localhost".to_string())
}

/// The working folder of this process.
pub fn working_dir() -> String {
    std::env::current_dir()
        .map(|d| d.display().to_string())
        .unwrap_or_default()
}

/// The environment variable that marks a process that a restart started: it waits for the
/// old process to release the pipe and the port.
pub const RESTARTED_ENV: &str = "AIVORYD_RESTARTED";

/// Start a new `aivoryd` with the arguments of this process. The new process waits until the
/// old one has released its endpoint (see [`retry_bind`]).
pub fn spawn_restart() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.args(std::env::args_os().skip(1))
        .env(RESTARTED_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW: the new daemon outlives this one.
        cmd.creation_flags(0x0000_0200 | 0x0800_0000);
    }
    cmd.spawn().map(|_| ())
}

/// Run `bind`. In a process that a restart started, retry for up to 30 s while the old process
/// still holds the endpoint.
pub fn retry_bind<T>(mut bind: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let restarted = std::env::var_os(RESTARTED_ENV).is_some();
    let begin = Instant::now();
    loop {
        match bind() {
            Err(_) if restarted && begin.elapsed() < Duration::from_secs(30) => {
                std::thread::sleep(Duration::from_millis(100));
            }
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_names_are_checked() {
        for bad in ["", "  ", "a/b", "a\\b", "..", "x.", "con", "a:b", "a*"] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
        assert_eq!(check_name(" my-app ").unwrap(), "my-app");
    }

    #[test]
    fn a_path_matches_itself_with_other_separators() {
        assert!(same_path("C:\\a\\b\\", "C:/a/b"));
        assert!(!same_path("C:/a/b", "C:/a/c"));
    }

    #[test]
    fn ids_get_a_number_when_taken() {
        let taken = ["x", "x-2"];
        assert_eq!(unique_id("x", |id| taken.contains(&id)), "x-3");
        assert_eq!(unique_id("y", |id| taken.contains(&id)), "y");
    }

    #[test]
    fn the_defaultmode_of_claude_maps_to_a_mode() {
        assert_eq!(mode_for("plan"), Some(Mode::Plan));
        assert_eq!(mode_for("default"), Some(Mode::Edit));
        assert_eq!(mode_for("acceptEdits"), Some(Mode::Auto));
        assert_eq!(mode_for("bypassPermissions"), None);
    }
}
