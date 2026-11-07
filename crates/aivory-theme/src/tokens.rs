//! `tokens.toml`: merge the token files, then check names, formats and ranges (ADR 0024 section 2).

use std::collections::BTreeSet;

use aivory_protocol::theme::TokenFile;
use serde_json::Value;

use crate::Issue;

/// The generic font families.
pub const GENERIC_FONTS: [&str; 3] = ["sans", "serif", "mono"];
/// The easing names.
pub const EASINGS: [&str; 5] = ["linear", "ease", "easeIn", "easeOut", "easeInOut"];

/// Merge the token files in order. A later file overrides the keys of an earlier file; tables
/// merge key by key. `texts` has `(file, text)`. Returns the typed tokens and the merged JSON.
pub fn merge(texts: &[(String, String)]) -> Result<(TokenFile, Value), Vec<Issue>> {
    let mut merged = toml::Table::new();
    let mut issues = Vec::new();
    for (file, text) in texts {
        match text.parse::<toml::Table>() {
            Ok(t) => merge_tables(&mut merged, t),
            Err(e) => issues.push(Issue::new(file, "", e.to_string().replace('\n', " "))),
        }
    }
    if !issues.is_empty() {
        return Err(issues);
    }
    let json = serde_json::to_value(&merged).unwrap_or(Value::Null);
    let typed: TokenFile =
        toml::Value::Table(merged)
            .try_into()
            .map_err(|e: toml::de::Error| {
                vec![Issue::new(
                    texts.last().map_or("tokens.toml", |t| t.0.as_str()),
                    "",
                    e.to_string().replace('\n', " "),
                )]
            })?;
    Ok((typed, json))
}

fn merge_tables(into: &mut toml::Table, from: toml::Table) {
    for (k, v) in from {
        match (into.get_mut(&k), v) {
            (Some(toml::Value::Table(a)), toml::Value::Table(b)) => merge_tables(a, b),
            (_, v) => {
                into.insert(k, v);
            }
        }
    }
}

fn color_ok(s: &str) -> bool {
    let Some(h) = s.strip_prefix('#') else {
        return false;
    };
    (h.len() == 6 || h.len() == 8) && h.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Check the merged tokens. `bundled_fonts` holds the paths of the font files of the package.
pub fn check(t: &TokenFile, file: &str, bundled_fonts: &BTreeSet<String>) -> Vec<Issue> {
    let mut out = Vec::new();
    let mut bad = |ptr: &str, rule: String| out.push(Issue::new(file, ptr, rule));
    let mut color = |ptr: &str, v: &Option<String>| match v {
        None => bad(ptr, "required color is missing".to_string()),
        Some(s) if !color_ok(s) => bad(
            ptr,
            format!("\"{s}\" is not a color (#rrggbb or #rrggbbaa)"),
        ),
        Some(_) => {}
    };
    color("/colors/bg", &t.colors.bg);
    color("/colors/surface", &t.colors.surface);
    color("/colors/text", &t.colors.text);
    color("/colors/muted", &t.colors.muted);
    color("/colors/accent", &t.colors.accent);
    color("/colors/analysis", &t.colors.analysis);
    match &t.colors.state {
        None => color("/colors/state", &None),
        Some(s) => {
            color("/colors/state/needsYou", &s.needs_you);
            color("/colors/state/working", &s.working);
            color("/colors/state/idle", &s.idle);
            color("/colors/state/parked", &s.parked);
            color("/colors/state/error", &s.error);
            color("/colors/state/done", &s.done);
        }
    }
    for (ptr, v) in [("/fonts/ui", &t.fonts.ui), ("/fonts/mono", &t.fonts.mono)] {
        match v {
            None => bad(ptr, "required font family is missing".to_string()),
            Some(f) if GENERIC_FONTS.contains(&f.as_str()) || bundled_fonts.contains(f) => {}
            Some(f) => bad(
                ptr,
                format!("\"{f}\" is not sans, serif, mono or a font file of the package"),
            ),
        }
    }
    for (ptr, v) in [
        ("/type/xs", t.type_sizes.xs),
        ("/type/sm", t.type_sizes.sm),
        ("/type/md", t.type_sizes.md),
        ("/type/lg", t.type_sizes.lg),
        ("/type/xl", t.type_sizes.xl),
    ] {
        match v {
            None => bad(ptr, "required text size is missing".to_string()),
            Some(n) if !(6.0..=72.0).contains(&n) => {
                bad(ptr, "text size must be 6 to 72 px".to_string())
            }
            Some(_) => {}
        }
    }
    for (ptr, map) in [("/space", &t.space), ("/radii", &t.radii)] {
        for (k, n) in map {
            if !(0.0..=200.0).contains(n) {
                bad(
                    &format!("{ptr}/{}", crate::pointer_key(k)),
                    "value must be 0 to 200".to_string(),
                );
            }
        }
    }
    for (k, ms) in &t.motion.durations {
        if *ms > 5000 {
            bad(
                &format!("/motion/durations/{}", crate::pointer_key(k)),
                "duration must be 0 to 5000 ms".to_string(),
            );
        }
    }
    if let Some(e) = &t.motion.easing
        && !EASINGS.contains(&e.as_str())
    {
        bad(
            "/motion/easing",
            format!("easing must be one of {}", EASINGS.join(", ")),
        );
    }
    for (ptr, v, hi) in [
        ("/effects/glow", t.effects.glow, 1.0),
        ("/effects/scanlines", t.effects.scanlines, 1.0),
        ("/effects/glass_alpha", t.effects.glass_alpha, 1.0),
        ("/effects/blur", t.effects.blur, 40.0),
    ] {
        if let Some(n) = v
            && !(0.0..=hi).contains(&n)
        {
            bad(ptr, format!("value must be 0 to {hi}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const GOOD: &str = r##"
[colors]
bg = "#ffffff"
surface = "#f4f5f7"
text = "#15161b"
muted = "#6b7280"
accent = "#2563eb"
analysis = "#7c3aed"
[colors.state]
needsYou = "#d97706"
working = "#2563eb"
idle = "#6b7280"
parked = "#9ca3af"
error = "#dc2626"
done = "#16a34a"
[fonts]
ui = "sans"
mono = "mono"
[type]
xs = 11
sm = 12
md = 14
lg = 18
xl = 24
[radii]
card = 8
"##;

    #[test]
    fn good_tokens_pass_and_a_later_file_overrides() {
        let over = "[colors]\nbg = \"#000000\"\n".to_string();
        let (t, json) = merge(&[
            ("tokens.toml".into(), GOOD.into()),
            ("dark.toml".into(), over),
        ])
        .unwrap();
        assert!(check(&t, "tokens.toml", &BTreeSet::new()).is_empty());
        assert_eq!(json["colors"]["bg"], "#000000");
        assert_eq!(json["colors"]["text"], "#15161b");
    }

    #[test]
    fn bad_tokens_name_the_pointer() {
        let text = GOOD
            .replace("#2563eb\"\nanalysis", "red\"\nanalysis")
            .replace("md = 14", "md = 3");
        let (t, _) = merge(&[("tokens.toml".into(), text)]).unwrap();
        let issues = check(&t, "tokens.toml", &BTreeSet::new());
        let ptrs: Vec<_> = issues.iter().map(|i| i.pointer.as_str()).collect();
        assert!(ptrs.contains(&"/colors/accent"), "{ptrs:?}");
        assert!(ptrs.contains(&"/type/md"), "{ptrs:?}");
        assert!(merge(&[("t.toml".into(), "[bogus]\nx = 1".into())]).is_err());
    }
}
