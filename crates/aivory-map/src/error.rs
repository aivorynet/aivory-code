use std::path::PathBuf;

/// Why a map file could not be read, checked, or written.
///
/// Every `line` is 1-based and points into the `map.toml` text.
#[derive(Debug, thiserror::Error)]
pub enum MapError {
    /// The text is not valid TOML.
    #[error("line {line}: {message}")]
    Syntax { line: usize, message: String },
    /// A required key is absent. For a part, `line` is the `[[part]]` header.
    #[error("line {line}: missing `{field}`")]
    MissingField { line: usize, field: &'static str },
    /// A key has the wrong type or an unusable value.
    #[error("line {line}: `{field}` {message}")]
    InvalidField {
        line: usize,
        field: &'static str,
        message: String,
    },
    /// A part lists no paths, so it can never match a file.
    #[error("line {line}: part {part:?} has an empty `paths` list")]
    EmptyPaths { line: usize, part: String },
    /// A path pattern is not a valid glob. `line` is `None` when the parts
    /// come from memory and not from a file.
    #[error("{}bad glob {glob:?} in part {part:?}: {reason}", at(.line))]
    BadGlob {
        line: Option<usize>,
        part: String,
        glob: String,
        reason: String,
    },
    /// A part with this name exists already.
    #[error("part {part:?} exists already")]
    DuplicatePart { part: String },
    /// `map.toml` exists and the caller did not allow an overwrite.
    #[error("{} already exists", path.display())]
    Exists { path: PathBuf },
    /// A file system call failed.
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl MapError {
    /// The line in `map.toml` that caused the error, when there is one.
    pub fn line(&self) -> Option<usize> {
        match self {
            Self::Syntax { line, .. }
            | Self::MissingField { line, .. }
            | Self::InvalidField { line, .. }
            | Self::EmptyPaths { line, .. } => Some(*line),
            Self::BadGlob { line, .. } => *line,
            Self::Exists { .. } | Self::Io { .. } | Self::DuplicatePart { .. } => None,
        }
    }
}

fn at(line: &Option<usize>) -> String {
    line.map(|n| format!("line {n}: ")).unwrap_or_default()
}
