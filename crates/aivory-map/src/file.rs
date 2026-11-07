use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use aivory_protocol::repos::MapFile;
use aivory_protocol::warroom::Part;
use toml::Spanned;
use toml::de::{DeTable, DeValue};

use crate::error::MapError;
use crate::matcher::{PartMatcher, compile};
use crate::stamp;

/// Folder of the map file, relative to the repository root.
pub const MAP_DIR: &str = ".aivory";
/// Name of the map file inside [`MAP_DIR`].
pub const MAP_FILE: &str = "map.toml";

/// `<repo_root>/.aivory/map.toml`.
pub fn map_path(repo_root: &Path) -> PathBuf {
    repo_root.join(MAP_DIR).join(MAP_FILE)
}

/// Read and check the map file of a repository.
///
/// The codebase id is the name of the repository folder.
pub fn load(repo_root: &Path) -> Result<MapFile, MapError> {
    let path = map_path(repo_root);
    let source = fs::read_to_string(&path).map_err(|source| MapError::Io { path, source })?;
    parse(&codebase_id(repo_root), &source)
}

/// Write `file.source` to `<repo_root>/.aivory/map.toml`, and return the path.
///
/// The text must pass [`parse`] first. Without `overwrite`, an existing file
/// stays untouched and the result is [`MapError::Exists`].
pub fn write(repo_root: &Path, file: &MapFile, overwrite: bool) -> Result<PathBuf, MapError> {
    parse(&file.codebase, &file.source)?;
    let io_err = |path: &Path| {
        let path = path.to_path_buf();
        move |source| MapError::Io { path, source }
    };
    if !repo_root.is_dir() {
        let err = io::Error::new(io::ErrorKind::NotFound, "not a directory");
        return Err(io_err(repo_root)(err));
    }
    let path = map_path(repo_root);
    let dir = repo_root.join(MAP_DIR);
    fs::create_dir_all(&dir).map_err(io_err(&dir))?;
    let mut options = fs::OpenOptions::new();
    options.write(true);
    if overwrite {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    let mut out = options.open(&path).map_err(|e| {
        if e.kind() == io::ErrorKind::AlreadyExists {
            MapError::Exists { path: path.clone() }
        } else {
            io_err(&path)(e)
        }
    })?;
    out.write_all(file.source.as_bytes())
        .map_err(io_err(&path))?;
    Ok(path)
}

/// The codebase id for a repository folder: the folder name.
pub fn codebase_id(repo_root: &Path) -> String {
    let name = |p: &Path| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
    };
    name(repo_root)
        .or_else(|| fs::canonicalize(repo_root).ok().and_then(|p| name(&p)))
        .unwrap_or_else(|| "repo".to_owned())
}

/// Parse the text of a `map.toml` and check it.
///
/// The result keeps `text` as `source`. The checks: valid TOML, `version` and
/// `updated` present, every part has a `name` and a non-empty `paths`, and every
/// path is a valid glob. Other keys are ignored.
pub fn parse(codebase: &str, text: &str) -> Result<MapFile, MapError> {
    let lines = Lines::new(text);
    let doc = DeTable::parse(text).map_err(|e| MapError::Syntax {
        line: e.span().map_or(1, |s| lines.line_of(s.start)),
        message: e.message().to_owned(),
    })?;
    let root = doc.get_ref();

    let version = version(&lines, root)?;
    let updated = updated(&lines, root)?;
    let author = match root.get("author") {
        None => None,
        Some(v) => match v.get_ref() {
            DeValue::String(s) if !s.trim().is_empty() => Some(s.to_string()),
            DeValue::String(_) => None,
            _ => {
                let line = lines.line_of(v.span().start);
                return Err(invalid(line, "author", "must be a string"));
            }
        },
    };

    let mut parts = Vec::new();
    if let Some(entry) = root.get("part") {
        let line = lines.line_of(entry.span().start);
        let DeValue::Array(items) = entry.get_ref() else {
            return Err(invalid(line, "part", "must be a list of [[part]] blocks"));
        };
        for item in items.iter() {
            parts.push(read_part(&lines, item)?);
        }
    }
    Ok(MapFile {
        codebase: codebase.to_owned(),
        version,
        updated,
        author,
        parts,
        source: text.to_owned(),
    })
}

/// Check `parts` and render them to a [`MapFile`] with a canonical `source`.
///
/// `updated` is in Unix milliseconds. The text uses whole seconds.
pub fn build(
    codebase: &str,
    version: u32,
    updated: i64,
    author: Option<&str>,
    parts: Vec<Part>,
) -> Result<MapFile, MapError> {
    for part in &parts {
        if part.name.trim().is_empty() {
            return Err(invalid(1, "name", "must not be empty"));
        }
        if part.paths.is_empty() {
            return Err(MapError::EmptyPaths {
                line: 1,
                part: part.name.clone(),
            });
        }
    }
    PartMatcher::new(&parts)?;
    Ok(assemble(codebase, version, updated, author, parts))
}

/// Render without checks. The caller guarantees valid parts.
pub(crate) fn assemble(
    codebase: &str,
    version: u32,
    updated: i64,
    author: Option<&str>,
    parts: Vec<Part>,
) -> MapFile {
    let updated = updated.div_euclid(1000) * 1000;
    let mut source = format!(
        "# AIVory part map: written by agents, reviewed by you\nversion = {version}\nupdated = {}\n",
        quote(&stamp::format_ms(updated))
    );
    if let Some(author) = author {
        source.push_str(&format!("author = {}\n", quote(author)));
    }
    for part in &parts {
        let paths: Vec<String> = part.paths.iter().map(|p| quote(p)).collect();
        source.push_str(&format!(
            "\n[[part]]\nname  = {}\npaths = [{}]\n",
            quote(&part.name),
            paths.join(", ")
        ));
    }
    MapFile {
        codebase: codebase.to_owned(),
        version,
        updated,
        author: author.map(str::to_owned),
        parts,
        source,
    }
}

fn quote(text: &str) -> String {
    toml::Value::String(text.to_owned()).to_string()
}

fn version(lines: &Lines, root: &DeTable<'_>) -> Result<u32, MapError> {
    let Some(v) = root.get("version") else {
        return Err(missing(1, "version"));
    };
    let line = lines.line_of(v.span().start);
    let DeValue::Integer(n) = v.get_ref() else {
        return Err(invalid(line, "version", "must be an integer"));
    };
    let digits = n.as_str().replace('_', "");
    u32::from_str_radix(&digits, n.radix()).map_err(|_| {
        invalid(
            line,
            "version",
            "must be a whole number from 0 to 4294967295",
        )
    })
}

fn updated(lines: &Lines, root: &DeTable<'_>) -> Result<i64, MapError> {
    let Some(v) = root.get("updated") else {
        return Err(missing(1, "updated"));
    };
    let line = lines.line_of(v.span().start);
    let text = match v.get_ref() {
        DeValue::String(s) => s.to_string(),
        DeValue::Datetime(d) => d.to_string(),
        _ => return Err(invalid(line, "updated", "must be an RFC 3339 time stamp")),
    };
    stamp::parse_ms(&text).ok_or_else(|| {
        invalid(
            line,
            "updated",
            "must be an RFC 3339 time stamp with an offset, for example 2026-10-21T14:12:00Z",
        )
    })
}

fn read_part(lines: &Lines, item: &Spanned<DeValue<'_>>) -> Result<Part, MapError> {
    let line = lines.line_of(item.span().start);
    let DeValue::Table(table) = item.get_ref() else {
        return Err(invalid(line, "part", "must be a table"));
    };
    let name = match table.get("name") {
        None => return Err(missing(line, "name")),
        Some(v) => {
            let at = lines.line_of(v.span().start);
            match v.get_ref() {
                DeValue::String(s) if !s.trim().is_empty() => s.to_string(),
                DeValue::String(_) => return Err(invalid(at, "name", "must not be empty")),
                _ => return Err(invalid(at, "name", "must be a string")),
            }
        }
    };
    let Some(entry) = table.get("paths") else {
        return Err(missing(line, "paths"));
    };
    let entry_line = lines.line_of(entry.span().start);
    let DeValue::Array(list) = entry.get_ref() else {
        return Err(invalid(entry_line, "paths", "must be a list of globs"));
    };
    if list.is_empty() {
        return Err(MapError::EmptyPaths {
            line: entry_line,
            part: name,
        });
    }
    let mut paths = Vec::with_capacity(list.len());
    for glob in list.iter() {
        let glob_line = lines.line_of(glob.span().start);
        let DeValue::String(text) = glob.get_ref() else {
            return Err(invalid(glob_line, "paths", "must hold strings only"));
        };
        compile(text).map_err(|reason| MapError::BadGlob {
            line: Some(glob_line),
            part: name.clone(),
            glob: text.to_string(),
            reason,
        })?;
        paths.push(text.to_string());
    }
    Ok(Part { name, paths })
}

fn missing(line: usize, field: &'static str) -> MapError {
    MapError::MissingField { line, field }
}

fn invalid(line: usize, field: &'static str, message: &str) -> MapError {
    MapError::InvalidField {
        line,
        field,
        message: message.to_owned(),
    }
}

/// Byte offset to line number.
struct Lines {
    starts: Vec<usize>,
}

impl Lines {
    fn new(text: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Self { starts }
    }

    fn line_of(&self, offset: usize) -> usize {
        self.starts.partition_point(|&s| s <= offset)
    }
}
