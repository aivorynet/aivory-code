//! The whole check of a theme folder (`aivoryd theme check <dir>`): file types, caps, the manifest,
//! the tokens, the strings, the layouts and parts, the assets and their license files.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use aivory_protocol::theme::{DisplayClass, LayoutFile};
use serde_json::Value;

use crate::package::{Package, Semver, app_version};
use crate::paths::{FileKind, kind_of, size_cap};
use crate::{Issue, assets, caps, layout, package, strings, tokens};

/// The screens of ADR 0024 section 1.
pub const SCREENS: [&str; 7] = [
    "warroom", "session", "inbox", "tasks", "repos", "settings", "decision",
];
/// Screens that a desktop theme may have but does not need. `focus` is the old name of the
/// session screen: an old theme keeps it and the shell uses it when `session` is not enough.
const ACCEPTED: [&str; 8] = [
    "warroom", "session", "inbox", "tasks", "repos", "settings", "decision", "focus",
];
/// The screens of the `watch` class.
pub const WATCH_SCREENS: [&str; 2] = ["warroom", "decision"];

/// Options of a check.
#[derive(Debug, Clone, Default)]
pub struct Options<'a> {
    /// The Git tag of the release; `version` of the manifest must equal it without the `v`.
    pub tag: Option<&'a str>,
    /// The app version for `min_app`. `None`: the version of this build.
    pub app: Option<Semver>,
}

/// The result of a check: all issues, and the package when there is none.
#[derive(Debug, Clone)]
pub struct Report {
    pub issues: Vec<Issue>,
    pub package: Option<Package>,
}

impl Report {
    pub fn is_ok(&self) -> bool {
        self.issues.is_empty()
    }
}

/// The screens that a class may have.
pub fn screens_of(class: DisplayClass) -> &'static [&'static str] {
    if class == DisplayClass::Watch {
        &WATCH_SCREENS
    } else {
        &ACCEPTED
    }
}

/// Check the theme in `dir`.
pub fn validate_dir(dir: &Path, opts: &Options<'_>) -> Report {
    let mut issues = Vec::new();
    let files = match package::scan(dir) {
        Ok(f) => f,
        Err(i) => {
            return Report {
                issues: i,
                package: None,
            };
        }
    };
    let rels: BTreeSet<&str> = files.iter().map(|f| f.rel.as_str()).collect();
    let mut kinds: BTreeMap<&str, FileKind> = BTreeMap::new();
    for f in &files {
        match kind_of(&f.rel) {
            None => issues.push(Issue::new(
                &f.rel,
                "",
                "file type not allowed (a theme has TOML, JSON, Markdown, fonts and images only)",
            )),
            Some(k) => {
                let cap = size_cap(&f.rel, k);
                if f.size > cap {
                    issues.push(Issue::new(
                        &f.rel,
                        "",
                        format!("file is larger than {cap} bytes"),
                    ));
                }
                kinds.insert(f.rel.as_str(), k);
            }
        }
    }
    let read = |rel: &str, issues: &mut Vec<Issue>| -> Option<Vec<u8>> {
        match fs::read(dir.join(rel)) {
            Ok(b) => Some(b),
            Err(e) => {
                issues.push(Issue::new(rel, "", format!("cannot read the file: {e}")));
                None
            }
        }
    };
    let read_text = |rel: &str, issues: &mut Vec<Issue>| -> Option<String> {
        let b = read(rel, issues)?;
        match String::from_utf8(b) {
            Ok(t) => Some(t),
            Err(_) => {
                issues.push(Issue::new(rel, "", "the file is not UTF-8"));
                None
            }
        }
    };

    // Placement rules.
    for (rel, kind) in &kinds {
        if rel.ends_with(".upstream") || *kind == FileKind::Lock {
            continue;
        }
        match kind {
            FileKind::Json if !(rel.starts_with("layouts/") || rel.starts_with("parts/")) => {
                issues.push(Issue::new(
                    *rel,
                    "",
                    "a JSON file belongs in layouts/ or parts/",
                ));
            }
            FileKind::Font if !rel.starts_with("assets/fonts/") => {
                issues.push(Issue::new(*rel, "", "a font belongs in assets/fonts/"));
            }
            k if k.is_image()
                && !(rel.starts_with("assets/images/") || rel.starts_with("previews/")) =>
            {
                issues.push(Issue::new(
                    *rel,
                    "",
                    "an image belongs in assets/images/ or previews/",
                ));
            }
            FileKind::Toml if rel.starts_with("strings/") => match strings::lang_of(rel) {
                Some(l) if strings::lang_ok(l) => {}
                _ => issues.push(Issue::new(
                    *rel,
                    "",
                    "a strings file is strings/<lang>.toml (for example strings/en.toml)",
                )),
            },
            _ => {}
        }
    }

    // Manifest.
    let manifest = if rels.contains("theme.toml") {
        read_text("theme.toml", &mut issues).and_then(|text| match package::parse_manifest(&text) {
            Ok(m) => Some(m),
            Err(e) => {
                issues.push(Issue::new("theme.toml", "", e));
                None
            }
        })
    } else {
        issues.push(Issue::new("theme.toml", "", "the manifest is missing"));
        None
    };
    let app = opts.app.clone().unwrap_or_else(app_version);
    if let Some(m) = &manifest {
        issues.extend(package::check_manifest(m, opts.tag, &app));
    }

    // Tokens.
    let fonts: BTreeSet<String> = kinds
        .iter()
        .filter(|(r, k)| **k == FileKind::Font && !r.ends_with(".upstream"))
        .map(|(r, _)| (*r).to_string())
        .collect();
    let mut merged_tokens = Value::Null;
    if let Some(m) = &manifest {
        let mut texts = Vec::new();
        for t in &m.tokens {
            if !rels.contains(t.as_str()) {
                issues.push(Issue::new(
                    t,
                    "",
                    "the token file listed in theme.toml is missing",
                ));
            } else if let Some(text) = read_text(t, &mut issues) {
                texts.push((t.clone(), text));
            }
        }
        if texts.len() == m.tokens.len() {
            match tokens::merge(&texts) {
                Ok((typed, json)) => {
                    issues.extend(tokens::check(
                        &typed,
                        m.tokens.last().map_or("tokens.toml", String::as_str),
                        &fonts,
                    ));
                    merged_tokens = json;
                }
                Err(i) => issues.extend(i),
            }
        }
    }

    // Strings.
    let mut langs: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for rel in &rels {
        if let Some(lang) = strings::lang_of(rel)
            && strings::lang_ok(lang)
            && let Some(text) = read_text(rel, &mut issues)
        {
            match strings::parse(rel, &text) {
                Ok(m) => {
                    langs.insert(lang.to_string(), m);
                }
                Err(i) => issues.extend(i),
            }
        }
    }
    if !langs.is_empty() && !langs.contains_key("en") {
        issues.push(Issue::new(
            "strings/en.toml",
            "",
            "the strings tables need an English file, strings/en.toml",
        ));
    }
    let common = strings::common_keys(&langs);
    let strings_keys = (!langs.is_empty()).then_some(&common);

    // Parts and layouts.
    let part_names: BTreeSet<String> = rels
        .iter()
        .filter_map(|r| r.strip_prefix("parts/")?.strip_suffix(".json"))
        .filter(|n| !n.contains('/'))
        .map(str::to_string)
        .collect();
    for rel in &rels {
        if let Some(rest) = rel.strip_prefix("parts/")
            && !rel.ends_with(".upstream")
        {
            let ok = rest.strip_suffix(".json").is_some_and(|n| {
                !n.is_empty()
                    && n.bytes().all(|b| {
                        b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-'
                    })
            });
            if !ok {
                issues.push(Issue::new(
                    *rel,
                    "",
                    "a part file is parts/<name>.json with [a-z0-9_-]",
                ));
            }
        }
    }
    let load_json = |rel: &str, issues: &mut Vec<Issue>| -> Option<Value> {
        let text = read_text(rel, issues)?;
        match serde_json::from_str::<Value>(&text) {
            Ok(v) => Some(v),
            Err(e) => {
                issues.push(Issue::new(rel, "", format!("not valid JSON: {e}")));
                None
            }
        }
    };
    let layout_root = |v: &Value| -> Option<Value> {
        serde_json::from_value::<LayoutFile>(v.clone())
            .ok()
            .map(|l| l.root)
    };
    let mut parts: BTreeMap<String, Value> = BTreeMap::new();
    let mut part_uses: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for name in &part_names {
        let rel = format!("parts/{name}.json");
        if let Some(doc) = load_json(&rel, &mut issues) {
            let ctx = layout::Context {
                class: DisplayClass::Desktop,
                parts: &part_names,
                strings: strings_keys,
            };
            let c = layout::check(&rel, &doc, ctx);
            issues.extend(c.issues);
            part_uses.insert(name.clone(), c.uses);
            if let Some(r) = layout_root(&doc) {
                parts.insert(name.clone(), r);
            }
        }
    }
    for name in &part_names {
        if reaches(name, name, &part_uses, &mut BTreeSet::new()) {
            issues.push(Issue::new(
                format!("parts/{name}.json"),
                "",
                "a part refers to itself (directly or through other parts)",
            ));
        }
    }

    let declared: BTreeSet<DisplayClass> = manifest
        .as_ref()
        .map(|m| m.displays.iter().copied().collect())
        .unwrap_or_default();
    let mut layouts: BTreeMap<DisplayClass, BTreeMap<String, Value>> = BTreeMap::new();
    for rel in &rels {
        let Some(rest) = rel.strip_prefix("layouts/") else {
            continue;
        };
        if rel.ends_with(".upstream") {
            continue;
        }
        let Some((class_dir, file)) = rest.split_once('/') else {
            issues.push(Issue::new(
                *rel,
                "",
                "a layout file is layouts/<class>/<screen>.json",
            ));
            continue;
        };
        let Some(class) = DisplayClass::ALL
            .iter()
            .copied()
            .find(|c| c.dir() == class_dir)
        else {
            issues.push(Issue::new(
                *rel,
                "",
                format!("unknown display class folder \"{class_dir}\""),
            ));
            continue;
        };
        let screen = file
            .strip_suffix(".json")
            .filter(|s| screens_of(class).contains(s));
        let Some(screen) = screen else {
            issues.push(Issue::new(
                *rel,
                "",
                format!(
                    "unknown screen; the {class_dir} class has {}",
                    screens_of(class).join(", ")
                ),
            ));
            continue;
        };
        if manifest.is_some() && !declared.contains(&class) {
            issues.push(Issue::new(
                *rel,
                "",
                format!("the manifest does not list \"{class_dir}\" in displays"),
            ));
        }
        if let Some(doc) = load_json(rel, &mut issues) {
            let ctx = layout::Context {
                class,
                parts: &part_names,
                strings: strings_keys,
            };
            let c = layout::check(rel, &doc, ctx);
            issues.extend(c.issues);
            if let Some(r) = layout_root(&doc) {
                layouts
                    .entry(class)
                    .or_default()
                    .insert(screen.to_string(), r);
            }
        }
    }
    for class in &declared {
        let have = layouts.get(class).map_or(0, BTreeMap::len);
        if *class == DisplayClass::Desktop {
            for s in SCREENS {
                if !rels.contains(format!("layouts/desktop/{s}.json").as_str()) {
                    issues.push(Issue::new(
                        format!("layouts/desktop/{s}.json"),
                        "",
                        "required screen is missing",
                    ));
                }
            }
        } else if have == 0 {
            issues.push(Issue::new(
                format!("layouts/{}", class.dir()),
                "",
                "the manifest lists this class but it has no layouts",
            ));
        }
    }

    // Assets.
    let mut families = BTreeSet::new();
    for (rel, kind) in &kinds {
        if rel.ends_with(".upstream") {
            continue;
        }
        if *kind == FileKind::Font {
            if let Some(b) = read(rel, &mut issues) {
                match assets::check_font(rel, &b) {
                    Ok(f) => {
                        families.insert(f);
                    }
                    Err(m) => issues.push(Issue::new(*rel, "", m)),
                }
            }
        } else if kind.is_image()
            && let Some(b) = read(rel, &mut issues)
            && let Err(m) = assets::check_image(*kind, &b)
        {
            issues.push(Issue::new(*rel, "", m));
        }
    }
    if families.len() > caps::FONT_FAMILIES {
        issues.push(Issue::new(
            "assets/fonts",
            "",
            format!(
                "{} font families; the cap is {}",
                families.len(),
                caps::FONT_FAMILIES
            ),
        ));
    }
    for dir_name in ["assets/fonts", "assets/images"] {
        let prefix = format!("{dir_name}/");
        let any = kinds
            .iter()
            .any(|(r, k)| r.starts_with(&prefix) && *k != FileKind::License);
        let license = kinds.iter().any(|(r, k)| {
            *k == FileKind::License && r.strip_prefix(&prefix).is_some_and(|n| !n.contains('/'))
        });
        if any && !license {
            issues.push(Issue::new(
                dir_name,
                "",
                "the folder needs a LICENSE file next to its assets",
            ));
        }
    }

    if !issues.is_empty() {
        return Report {
            issues,
            package: None,
        };
    }
    let Some(manifest) = manifest else {
        return Report {
            issues,
            package: None,
        };
    };
    let bytes = files.iter().map(|f| f.size).sum();
    let package = Package {
        manifest,
        tokens: merged_tokens,
        layouts,
        parts,
        languages: langs.keys().cloned().collect(),
        files: files.len(),
        bytes,
    };
    Report {
        issues,
        package: Some(package),
    }
}

fn reaches(
    from: &str,
    target: &str,
    graph: &BTreeMap<String, BTreeSet<String>>,
    seen: &mut BTreeSet<String>,
) -> bool {
    for next in graph.get(from).into_iter().flatten() {
        if next == target {
            return true;
        }
        if seen.insert(next.clone()) && reaches(next, target, graph, seen) {
            return true;
        }
    }
    false
}

/// Format the issues, one per line.
pub fn format_issues(issues: &[Issue]) -> String {
    issues
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample;

    fn write(dir: &Path, files: &[(String, Vec<u8>)]) {
        for (rel, bytes) in files {
            let p = dir.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, bytes).unwrap();
        }
    }

    fn issues_of(files: &[(String, Vec<u8>)], tag: Option<&str>) -> Vec<String> {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), files);
        validate_dir(d.path(), &Options { tag, app: None })
            .issues
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn the_sample_package_is_good() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), &sample::files("my-theme", "1.0.0"));
        let r = validate_dir(
            d.path(),
            &Options {
                tag: Some("v1.0.0"),
                app: None,
            },
        );
        assert!(r.is_ok(), "{}", format_issues(&r.issues));
        let p = r.package.unwrap();
        assert_eq!(p.manifest.id, "my-theme");
        assert_eq!(p.layouts[&DisplayClass::Desktop].len(), 7);
        assert_eq!(p.languages, ["en"]);
    }

    #[test]
    fn the_tag_must_equal_the_version() {
        let f = sample::files("my-theme", "1.0.0");
        let issues = issues_of(&f, Some("v1.0.1"));
        assert!(
            issues.iter().any(|i| i.contains("must equal the tag")),
            "{issues:?}"
        );
    }

    #[test]
    fn rule_breaks_are_reported() {
        let mut f = sample::files("my-theme", "1.0.0");
        f.push(("run.sh".into(), b"x".to_vec()));
        f.push(("assets/images/a.png".into(), b"not a png".to_vec()));
        f.retain(|(r, _)| r != "layouts/desktop/inbox.json");
        let issues = issues_of(&f, None);
        let has = |n: &str| {
            assert!(
                issues.iter().any(|i| i.contains(n)),
                "{n} not in {issues:#?}"
            )
        };
        has("run.sh: file type not allowed");
        has("assets/images/a.png");
        has("assets/images: the folder needs a LICENSE");
        has("layouts/desktop/inbox.json: required screen is missing");
    }

    #[test]
    fn upstream_files_lock_and_strings_are_allowed() {
        let mut f = sample::files("my-theme", "1.0.0");
        f.push(("theme.lock".into(), b"source = \"x\"".to_vec()));
        f.push(("tokens.toml.upstream".into(), b"x".to_vec()));
        f.push(("strings/de.toml".into(), b"title = \"Flotte\"".to_vec()));
        assert!(issues_of(&f, None).is_empty());
    }

    #[test]
    fn a_part_cycle_fails() {
        let mut f = sample::files("my-theme", "1.0.0");
        f.push((
            "parts/a.json".into(),
            br#"{"root":{"type":"stack","children":[{"use":"b"}]}}"#.to_vec(),
        ));
        f.push((
            "parts/b.json".into(),
            br#"{"root":{"type":"stack","children":[{"use":"a"}]}}"#.to_vec(),
        ));
        let issues = issues_of(&f, None);
        assert!(
            issues.iter().any(|i| i.contains("refers to itself")),
            "{issues:?}"
        );
    }
}
