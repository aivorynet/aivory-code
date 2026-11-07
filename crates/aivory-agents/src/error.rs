use std::path::PathBuf;

use aivory_sessions::Provider;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("the {0} CLI was not found on PATH")]
    ProgramNotFound(String),
    #[error("the working directory of the session does not exist: {}", .0.display())]
    CwdMissing(PathBuf),
    #[error("{0} needs the path of the session file to resume")]
    MissingSessionPath(Provider),
    #[error("cannot start the {program} CLI: {source}")]
    Spawn {
        program: String,
        source: std::io::Error,
    },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
