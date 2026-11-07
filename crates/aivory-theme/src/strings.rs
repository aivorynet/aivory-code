//! `strings/<lang>.toml`: the text of a layout, one table per language (owner answer 2026-11-03).
//! A layout reads a text as `$strings.key`; a nested table gives a dotted key (`$strings.nav.home`).

use std::collections::{BTreeMap, BTreeSet};

use crate::{Issue, caps};

/// True for a language tag such as `en`, `de`, `pt-BR`.
pub fn lang_ok(lang: &str) -> bool {
    let mut parts = lang.split('-');
    let first = parts.next().unwrap_or_default();
    (2..=3).contains(&first.len())
        && first.bytes().all(|b| b.is_ascii_lowercase())
        && parts.all(|p| (2..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// The language of a strings file path, or `None` when the path is no strings file.
pub fn lang_of(path: &str) -> Option<&str> {
    path.strip_prefix("strings/")?.strip_suffix(".toml")
}

/// Parse one strings file into flat dotted keys. A value must be a string of 200 characters or
/// fewer.
pub fn parse(file: &str, text: &str) -> Result<BTreeMap<String, String>, Vec<Issue>> {
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| {
        vec![Issue::new(file, "", e.to_string().replace('\n', " "))]
    })?;
    let mut out = BTreeMap::new();
    let mut issues = Vec::new();
    flatten(file, "", "", &table, &mut out, &mut issues);
    if issues.is_empty() {
        Ok(out)
    } else {
        Err(issues)
    }
}

fn flatten(
    file: &str,
    key: &str,
    ptr: &str,
    table: &toml::Table,
    out: &mut BTreeMap<String, String>,
    issues: &mut Vec<Issue>,
) {
    for (k, v) in table {
        let key2 = if key.is_empty() {
            k.clone()
        } else {
            format!("{key}.{k}")
        };
        let ptr2 = format!("{ptr}/{}", crate::pointer_key(k));
        if k.is_empty() || !k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            issues.push(Issue::new(file, &ptr2, "a key must use [A-Za-z0-9_]"));
            continue;
        }
        match v {
            toml::Value::String(s) => {
                if s.chars().count() > caps::TEXT_CHARS {
                    issues.push(Issue::new(
                        file,
                        &ptr2,
                        format!("text is longer than {} characters", caps::TEXT_CHARS),
                    ));
                }
                out.insert(key2, s.clone());
            }
            toml::Value::Table(t) => flatten(file, &key2, &ptr2, t, out, issues),
            _ => issues.push(Issue::new(
                file,
                &ptr2,
                "a strings value must be a string or a table",
            )),
        }
    }
}

/// The keys that every language has.
pub fn common_keys(langs: &BTreeMap<String, BTreeMap<String, String>>) -> BTreeSet<String> {
    let mut it = langs.values();
    let Some(first) = it.next() else {
        return BTreeSet::new();
    };
    let mut keys: BTreeSet<String> = first.keys().cloned().collect();
    for l in it {
        keys.retain(|k| l.contains_key(k));
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_tables_flatten() {
        let m = parse(
            "strings/en.toml",
            "title = \"Fleet\"\n[nav]\nhome = \"Home\"\n",
        )
        .unwrap();
        assert_eq!(m["title"], "Fleet");
        assert_eq!(m["nav.home"], "Home");
        assert!(parse("strings/en.toml", "n = 3").is_err());
        assert!(parse("strings/en.toml", &format!("t = \"{}\"", "x".repeat(201))).is_err());
        assert_eq!(lang_of("strings/pt-BR.toml"), Some("pt-BR"));
        assert!(lang_ok("pt-BR") && !lang_ok("EN") && !lang_ok("e"));
    }
}
