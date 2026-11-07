//! Find the agent CLIs that are installed on this machine, the way the flexible-ai JetBrains
//! plugin does: look on PATH (every PATHEXT extension on Windows) and in the usual install
//! folders, then run `--version` to confirm that the program starts.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// How far AIVory Code can wrap a CLI today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    /// Native sessions are listed, resumed and run live.
    Sessions,
    /// The CLI is found and shown; a driver comes later (or through ACP).
    Detected,
}

/// A CLI that AIVory Code knows.
#[derive(Debug, Clone, Copy)]
pub struct KnownCli {
    /// The provider id of the protocol (`claude`, `codex`, ...).
    pub id: &'static str,
    pub name: &'static str,
    /// Program names to look for, in order.
    pub programs: &'static [&'static str],
    pub version_args: &'static [&'static str],
    pub install: &'static str,
    pub support: Support,
}

pub const KNOWN: &[KnownCli] = &[
    KnownCli {
        id: "claude",
        name: "Claude Code",
        programs: &["claude"],
        version_args: &["--version"],
        install: "npm install -g @anthropic-ai/claude-code",
        support: Support::Sessions,
    },
    KnownCli {
        id: "codex",
        name: "Codex CLI",
        programs: &["codex"],
        version_args: &["--version"],
        install: "npm install -g @openai/codex",
        support: Support::Sessions,
    },
    KnownCli {
        id: "gemini",
        name: "Gemini CLI",
        programs: &["gemini"],
        version_args: &["--version"],
        install: "npm install -g @google/gemini-cli",
        support: Support::Sessions,
    },
    KnownCli {
        id: "opencode",
        name: "opencode",
        programs: &["opencode"],
        version_args: &["--version"],
        install: "npm install -g opencode-ai",
        support: Support::Detected,
    },
    KnownCli {
        id: "goose",
        name: "Goose",
        programs: &["goose"],
        version_args: &["--version"],
        install: "https://block.github.io/goose",
        support: Support::Detected,
    },
    KnownCli {
        id: "cursor",
        name: "Cursor Agent",
        programs: &["cursor-agent"],
        version_args: &["--version"],
        install: "https://cursor.com/cli",
        support: Support::Detected,
    },
    KnownCli {
        id: "qwen",
        name: "Qwen Code",
        programs: &["qwen"],
        version_args: &["--version"],
        install: "npm install -g @qwen-code/qwen-code",
        support: Support::Detected,
    },
    KnownCli {
        id: "copilot",
        name: "GitHub Copilot CLI",
        programs: &["copilot"],
        version_args: &["--version"],
        install: "npm install -g @github/copilot",
        support: Support::Detected,
    },
    KnownCli {
        id: "amp",
        name: "Amp",
        programs: &["amp"],
        version_args: &["--version"],
        install: "npm install -g @sourcegraph/amp",
        support: Support::Detected,
    },
    KnownCli {
        id: "crush",
        name: "Crush",
        programs: &["crush"],
        version_args: &["--version"],
        install: "https://github.com/charmbracelet/crush",
        support: Support::Detected,
    },
    KnownCli {
        id: "aider",
        name: "Aider",
        programs: &["aider"],
        version_args: &["--version"],
        install: "pip install aider-chat",
        support: Support::Detected,
    },
    KnownCli {
        id: "antigravity",
        name: "Antigravity",
        programs: &["agy", "antigravity"],
        version_args: &["--version"],
        install: "https://antigravity.google",
        support: Support::Detected,
    },
];

/// A CLI that was found and answered `--version`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedCli {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    /// The first line of the `--version` output.
    pub version: String,
    pub support: Support,
}

/// The time one `--version` call may take. Node CLIs start slowly on Windows.
pub const VERSION_TIMEOUT: Duration = Duration::from_secs(10);

/// Detect every known CLI, in parallel. The result keeps the order of [`KNOWN`].
pub fn detect_all() -> Vec<DetectedCli> {
    let dirs = search_dirs();
    let handles: Vec<_> = KNOWN
        .iter()
        .map(|cli| {
            let dirs = dirs.clone();
            thread::spawn(move || detect_in(cli, &dirs))
        })
        .collect();
    handles
        .into_iter()
        .filter_map(|h| h.join().ok().flatten())
        .collect()
}

/// Detect one CLI.
pub fn detect(cli: &KnownCli) -> Option<DetectedCli> {
    detect_in(cli, &search_dirs())
}

fn detect_in(cli: &KnownCli, dirs: &[PathBuf]) -> Option<DetectedCli> {
    cli.programs.iter().find_map(|program| {
        let path = find_program(program, dirs)?;
        let version = version(&path, cli.version_args, VERSION_TIMEOUT)?;
        Some(DetectedCli {
            id: cli.id.to_string(),
            name: cli.name.to_string(),
            path,
            version,
            support: cli.support,
        })
    })
}

/// The folders to search: PATH first, then the usual install folders that a GUI app may not
/// have on its PATH (npm, bun, pipx, Homebrew, WinGet, scoop).
pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let home =
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    if let Some(home) = &home {
        for sub in [".local/bin", ".npm-global/bin", ".bun/bin", ".cargo/bin"] {
            dirs.push(home.join(sub));
        }
    }
    if cfg!(windows) {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            dirs.push(PathBuf::from(appdata).join("npm"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(
                PathBuf::from(&local)
                    .join("Microsoft")
                    .join("WinGet")
                    .join("Links"),
            );
        }
        if let Some(home) = &home {
            dirs.push(home.join("scoop").join("shims"));
        }
    } else {
        for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
            dirs.push(PathBuf::from(dir));
        }
    }
    let mut seen = Vec::new();
    dirs.retain(|d| {
        let new = !seen.contains(d);
        seen.push(d.clone());
        new
    });
    dirs
}

/// The first `program` in `dirs`. On Windows each PATHEXT extension is tried, so the npm
/// shims (`claude.cmd`) resolve.
pub fn find_program(program: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let exts = extensions();
    for dir in dirs {
        for ext in &exts {
            let mut file = OsString::from(program);
            file.push(ext);
            let candidate = dir.join(file);
            if is_program(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn extensions() -> Vec<OsString> {
    if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(OsString::from)
            .collect()
    } else {
        vec![OsString::new()]
    }
}

#[cfg(unix)]
fn is_program(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_program(path: &Path) -> bool {
    path.is_file()
}

/// Run `path args` and return the first output line. A non-zero exit with output still
/// counts (some CLIs print the version on stderr and exit 1).
pub fn version(path: &Path, args: &[&str], timeout: Duration) -> Option<String> {
    let mut child = Command::new(path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    let out = thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let err = thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < timeout => thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = out.join().unwrap_or_default();
    let err = err.join().unwrap_or_default();
    first_line(&out).or_else(|| first_line(&err))
}

fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aivory-detect-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn known_ids_are_unique() {
        let mut ids: Vec<&str> = KNOWN.iter().map(|c| c.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), KNOWN.len());
    }

    #[test]
    fn finds_a_program_in_the_first_dir_that_has_it() {
        let a = temp_dir("a");
        let b = temp_dir("b");
        let name = if cfg!(windows) {
            "fake-agent.cmd"
        } else {
            "fake-agent"
        };
        std::fs::write(b.join(name), "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(b.join(name), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // Windows paths ignore case; PATHEXT gives `.CMD`.
        let found = find_program("fake-agent", &[a.clone(), b.clone()])
            .map(|p| p.to_string_lossy().to_lowercase());
        assert_eq!(found, Some(b.join(name).to_string_lossy().to_lowercase()));
        assert_eq!(find_program("missing-agent", &[a, b]), None);
    }

    /// A script that prints an empty line and then `lines`; `slow` waits 5 s first.
    #[cfg(windows)]
    fn script(dir: &Path, name: &str, lines: &[&str], slow: bool) -> PathBuf {
        let path = dir.join(format!("{name}.cmd"));
        let mut text = String::from("@echo off\r\necho.\r\n");
        if slow {
            text.push_str("ping -n 6 127.0.0.1 >nul\r\n");
        }
        for l in lines {
            text.push_str(&format!("echo {l}\r\n"));
        }
        std::fs::write(&path, text).unwrap();
        path
    }

    #[cfg(unix)]
    fn script(dir: &Path, name: &str, lines: &[&str], slow: bool) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        let mut text = String::from("#!/bin/sh\necho\n");
        if slow {
            text.push_str("sleep 5\n");
        }
        for l in lines {
            text.push_str(&format!("echo {l}\n"));
        }
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn version_reads_the_first_line_and_times_out() {
        let dir = temp_dir("v");
        let fast = script(&dir, "v", &["fake 1.2.3", "second"], false);
        let slow = script(&dir, "slow", &["late"], true);
        assert_eq!(
            version(&fast, &[], Duration::from_secs(10)).as_deref(),
            Some("fake 1.2.3")
        );
        assert_eq!(version(&slow, &[], Duration::from_millis(300)), None);
    }
}
