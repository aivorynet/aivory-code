use std::path::Path;

use aivory_protocol::repos::MapFile;
use aivory_protocol::warroom::Part;

use crate::error::MapError;
use crate::file::{self, load, map_path, parse, write};
use crate::init::init;
use crate::matcher::PartMatcher;
use crate::stamp;

/// Add a `[[part]]` block to `<repo_root>/.aivory/map.toml` and return the new map.
///
/// The text of the file stays as it is (comments, order, other keys): the block goes to the
/// end, and the `updated` line gets the current time. A missing file starts with the
/// [`init`] guess. The checks: a name that no part has, at least one path, valid globs.
pub fn add_part(repo_root: &Path, name: &str, paths: &[String]) -> Result<MapFile, MapError> {
    let part = Part {
        name: name.trim().to_owned(),
        paths: paths.iter().map(|p| p.trim().to_owned()).collect(),
    };
    if part.name.is_empty() {
        return Err(MapError::InvalidField {
            line: 1,
            field: "name",
            message: "must not be empty".to_owned(),
        });
    }
    if part.paths.is_empty() {
        return Err(MapError::EmptyPaths {
            line: 1,
            part: part.name,
        });
    }
    // Checks every glob before anything touches the disk.
    PartMatcher::new(std::slice::from_ref(&part))?;
    let current = match load(repo_root) {
        Ok(map) => map,
        Err(MapError::Io { .. }) if !map_path(repo_root).exists() => init(repo_root),
        Err(e) => return Err(e),
    };
    if current.parts.iter().any(|p| p.name == part.name) {
        return Err(MapError::DuplicatePart { part: part.name });
    }
    let source = with_part(&current.source, &part);
    let next = parse(&file::codebase_id(repo_root), &source)?;
    write(repo_root, &next, true)?;
    Ok(next)
}

/// `source` with the `updated` line set to now and the block of `part` at the end.
fn with_part(source: &str, part: &Part) -> String {
    let stamp = toml::Value::String(stamp::format_ms(stamp::now_ms())).to_string();
    let mut text = String::with_capacity(source.len() + 128);
    let mut replaced = false;
    for line in source.split_inclusive('\n') {
        let key = line.trim_start();
        if !replaced
            && key.starts_with("updated")
            && key["updated".len()..].trim_start().starts_with('=')
        {
            text.push_str(&format!("updated = {stamp}"));
            text.push_str(if line.ends_with("\r\n") { "\r\n" } else { "\n" });
            replaced = true;
        } else {
            text.push_str(line);
        }
    }
    if !text.ends_with('\n') {
        text.push('\n');
    }
    let paths: Vec<String> = part
        .paths
        .iter()
        .map(|p| toml::Value::String(p.clone()).to_string())
        .collect();
    text.push_str(&format!(
        "\n[[part]]\nname  = {}\npaths = [{}]\n",
        toml::Value::String(part.name.clone()),
        paths.join(", ")
    ));
    text
}
