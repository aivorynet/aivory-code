use serde_json::Value;

const TITLE_MAX_CHARS: usize = 120;

/// Collect the text of a message `content`, which is a string or a list of parts.
pub(crate) fn content_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(parts) => {
            let mut out = String::new();
            for p in parts {
                if let Some(t) = p.get("text").and_then(Value::as_str) {
                    out.push_str(t);
                }
            }
            out
        }
        _ => String::new(),
    }
}

/// Turn raw user text into a one-line title. Returns `None` for empty text and for
/// machine-generated text that starts with a markup tag.
pub(crate) fn clean_title(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|l| !l.is_empty())?;
    if line.starts_with('<') {
        return None;
    }
    Some(truncate_chars(line, TITLE_MAX_CHARS))
}

/// Like [`clean_title`] but accepts text that starts with a tag (used for last-resort titles).
pub(crate) fn clean_title_lenient(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(truncate_chars(line, TITLE_MAX_CHARS))
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_skips_tags_and_truncates() {
        assert_eq!(clean_title("<command-name>x</command-name>"), None);
        assert_eq!(
            clean_title("\n  hello world\nsecond").as_deref(),
            Some("hello world")
        );
        let long = "a".repeat(300);
        assert_eq!(clean_title(&long).unwrap().chars().count(), TITLE_MAX_CHARS);
    }

    #[test]
    fn content_text_handles_string_and_parts() {
        assert_eq!(content_text(&serde_json::json!("hi")), "hi");
        assert_eq!(
            content_text(&serde_json::json!([{"text": "a"}, {"type": "image"}, {"text": "b"}])),
            "ab"
        );
    }
}
