use std::ffi::OsString;
use std::path::{Path, PathBuf};

use aivory_sessions::Provider;

use crate::error::AgentError;

/// What to resume and how.
#[derive(Debug, Clone)]
pub struct ResumeRequest {
    pub provider: Provider,
    pub session_id: String,
    /// Working directory of the session. The CLI looks the session up from here.
    pub cwd: PathBuf,
    pub prompt: String,
    /// Path of the session file. Gemini needs it (`--session-file`).
    pub session_path: Option<PathBuf>,
    pub model: Option<String>,
    /// Extra CLI arguments, passed through before the prompt, for example a permission mode.
    pub extra_args: Vec<String>,
    /// Use this binary instead of the one found on PATH.
    pub program: Option<PathBuf>,
}

/// A fully described child process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    /// Bare program name (`claude`) or an explicit path.
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// Text to write to stdin. The prompt goes here when the CLI can read it from stdin.
    pub stdin: Option<String>,
}

/// Build the command line that resumes a session non-interactively with JSON output.
///
/// Claude and Codex read the prompt from stdin, so that the prompt needs no shell quoting
/// (on Windows the npm shims are `.cmd` files). Gemini takes the prompt as `--prompt=<text>`.
pub fn build_command(req: &ResumeRequest) -> Result<CommandSpec, AgentError> {
    let mut args: Vec<String> = Vec::new();
    let mut stdin = None;
    match req.provider {
        Provider::Claude => {
            args.extend(
                [
                    "-p",
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    "--include-partial-messages",
                    "--resume",
                    &req.session_id,
                ]
                .map(String::from),
            );
            if let Some(m) = &req.model {
                args.extend(["--model".to_string(), m.clone()]);
            }
            args.extend(req.extra_args.iter().cloned());
            stdin = Some(req.prompt.clone());
        }
        Provider::Codex => {
            args.extend(["exec", "--json", "--skip-git-repo-check"].map(String::from));
            if let Some(m) = &req.model {
                args.extend(["-m".to_string(), m.clone()]);
            }
            args.extend(req.extra_args.iter().cloned());
            args.extend([
                "resume".to_string(),
                req.session_id.clone(),
                "-".to_string(),
            ]);
            stdin = Some(req.prompt.clone());
        }
        Provider::Gemini => {
            let path = req
                .session_path
                .as_ref()
                .ok_or(AgentError::MissingSessionPath(Provider::Gemini))?;
            args.extend(["--session-file".to_string(), path.display().to_string()]);
            args.extend(["-o".to_string(), "stream-json".to_string()]);
            if let Some(m) = &req.model {
                args.extend(["-m".to_string(), m.clone()]);
            }
            args.extend(req.extra_args.iter().cloned());
            args.push(format!("--prompt={}", req.prompt));
        }
    }
    let program = req
        .program
        .clone()
        .unwrap_or_else(|| PathBuf::from(req.provider.as_str()));
    Ok(CommandSpec {
        program,
        args,
        cwd: req.cwd.clone(),
        stdin,
    })
}

/// Find a program on PATH. On Windows this tries every PATHEXT extension, so that the npm
/// shims `claude.cmd`, `codex.cmd`, and `gemini.cmd` resolve.
pub fn resolve_program(program: &Path) -> Result<PathBuf, AgentError> {
    if program.components().count() > 1 || program.is_absolute() {
        return if program.is_file() {
            Ok(program.to_path_buf())
        } else {
            Err(AgentError::ProgramNotFound(program.display().to_string()))
        };
    }
    let name = program.as_os_str();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let exts: Vec<OsString> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(OsString::from)
            .collect()
    } else {
        vec![OsString::new()]
    };
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let mut file = name.to_os_string();
            file.push(ext);
            let candidate = dir.join(file);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err(AgentError::ProgramNotFound(program.display().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(provider: Provider) -> ResumeRequest {
        ResumeRequest {
            provider,
            session_id: "sid-1".into(),
            cwd: PathBuf::from("."),
            prompt: "Reply with OK".into(),
            session_path: Some(PathBuf::from("session.jsonl")),
            model: None,
            extra_args: Vec::new(),
            program: None,
        }
    }

    #[test]
    fn claude_command_reads_prompt_from_stdin() {
        let c = build_command(&req(Provider::Claude)).unwrap();
        assert_eq!(c.program, PathBuf::from("claude"));
        assert_eq!(
            &c.args[..4],
            ["-p", "--output-format", "stream-json", "--verbose"]
        );
        assert!(c.args.windows(2).any(|w| w == ["--resume", "sid-1"]));
        assert_eq!(c.stdin.as_deref(), Some("Reply with OK"));
    }

    #[test]
    fn codex_command_puts_resume_last() {
        let mut r = req(Provider::Codex);
        r.model = Some("m1".into());
        let c = build_command(&r).unwrap();
        assert_eq!(
            c.args,
            [
                "exec",
                "--json",
                "--skip-git-repo-check",
                "-m",
                "m1",
                "resume",
                "sid-1",
                "-"
            ]
        );
        assert_eq!(c.stdin.as_deref(), Some("Reply with OK"));
    }

    #[test]
    fn gemini_command_needs_the_session_file() {
        let c = build_command(&req(Provider::Gemini)).unwrap();
        assert_eq!(
            c.args,
            [
                "--session-file",
                "session.jsonl",
                "-o",
                "stream-json",
                "--prompt=Reply with OK"
            ]
        );
        assert_eq!(c.stdin, None);
        let mut r = req(Provider::Gemini);
        r.session_path = None;
        assert!(matches!(
            build_command(&r),
            Err(AgentError::MissingSessionPath(_))
        ));
    }
}
