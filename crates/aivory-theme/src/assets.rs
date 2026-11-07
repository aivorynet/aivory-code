//! Fonts and images (ADR 0024 section 8): parse them and check the caps. SVG must be static.

use crate::caps;
use crate::paths::FileKind;

/// Check a font with `ttf-parser`. Returns the family name (or the file stem).
pub fn check_font(path: &str, bytes: &[u8]) -> Result<String, String> {
    let face = ttf_parser::Face::parse(bytes, 0).map_err(|e| format!("not a valid font: {e}"))?;
    let family = face
        .names()
        .into_iter()
        .filter(|n| {
            n.name_id == ttf_parser::name_id::TYPOGRAPHIC_FAMILY
                || n.name_id == ttf_parser::name_id::FAMILY
        })
        .find_map(|n| n.to_string())
        .unwrap_or_else(|| {
            let name = path.rsplit('/').next().unwrap_or(path);
            name.rsplit_once('.').map_or(name, |p| p.0).to_string()
        });
    Ok(family)
}

/// The pixel size of a PNG, JPEG or WebP image from its header.
pub fn image_size(kind: FileKind, b: &[u8]) -> Result<(u32, u32), String> {
    let bad = || "the image header is not valid".to_string();
    match kind {
        FileKind::Png => {
            if b.len() < 24 || &b[..8] != b"\x89PNG\r\n\x1a\n" || &b[12..16] != b"IHDR" {
                return Err(bad());
            }
            Ok((be32(&b[16..20]), be32(&b[20..24])))
        }
        FileKind::Jpeg => jpeg_size(b).ok_or_else(bad),
        FileKind::Webp => webp_size(b).ok_or_else(bad),
        _ => Err(bad()),
    }
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn jpeg_size(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 4 || b[0] != 0xFF || b[1] != 0xD8 {
        return None;
    }
    let mut i = 2;
    while i + 4 <= b.len() {
        if b[i] != 0xFF {
            return None;
        }
        let marker = b[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        if (0xD0..=0xD9).contains(&marker) || marker == 0x01 {
            i += 2;
            continue;
        }
        let len = usize::from(b[i + 2]) << 8 | usize::from(b[i + 3]);
        if matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            let s = b.get(i + 5..i + 9)?;
            let h = u32::from(s[0]) << 8 | u32::from(s[1]);
            let w = u32::from(s[2]) << 8 | u32::from(s[3]);
            return Some((w, h));
        }
        if len < 2 {
            return None;
        }
        i += 2 + len;
    }
    None
}

fn webp_size(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 30 || &b[..4] != b"RIFF" || &b[8..12] != b"WEBP" {
        return None;
    }
    let le16 = |o: usize| u32::from(b[o]) | u32::from(b[o + 1]) << 8;
    let le24 = |o: usize| u32::from(b[o]) | u32::from(b[o + 1]) << 8 | u32::from(b[o + 2]) << 16;
    match &b[12..16] {
        b"VP8 " => Some((le16(26) & 0x3FFF, le16(28) & 0x3FFF)),
        b"VP8L" => {
            let bits = u32::from_le_bytes([b[21], b[22], b[23], b[24]]);
            Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
        }
        b"VP8X" => Some((le24(24) + 1, le24(27) + 1)),
        _ => None,
    }
}

/// Check an image: the header must parse and each side must be 4096 px or fewer.
pub fn check_image(kind: FileKind, bytes: &[u8]) -> Result<(), String> {
    if kind == FileKind::Svg {
        return check_svg(bytes);
    }
    let (w, h) = image_size(kind, bytes)?;
    if w == 0 || h == 0 || w > caps::IMAGE_PX || h > caps::IMAGE_PX {
        return Err(format!(
            "image is {w}x{h} px; the cap is {0}x{0}",
            caps::IMAGE_PX
        ));
    }
    Ok(())
}

/// An SVG must be static: no `script`, no `foreignObject`, no event attributes, no external
/// `href`.
pub fn check_svg(bytes: &[u8]) -> Result<(), String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "the SVG is not UTF-8".to_string())?;
    let lower = text.to_ascii_lowercase();
    for bad in [
        "<script",
        "<foreignobject",
        "<iframe",
        "<embed",
        "<object",
        "<audio",
        "<video",
        "<animate",
        "<set ",
        "<!entity",
        "javascript:",
        "@import",
        "<?xml-stylesheet",
    ] {
        if lower.contains(bad) {
            return Err(format!("the SVG is not static: it contains \"{bad}\""));
        }
    }
    // Attributes: scan every `name=` pair inside tags.
    let b = lower.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        let end = lower[i..].find('>').map_or(b.len(), |e| i + e);
        check_tag(&lower[i..end])?;
        i = end + 1;
    }
    // url(...) in a style or an attribute must stay local.
    let mut rest = lower.as_str();
    while let Some(p) = rest.find("url(") {
        let after = rest[p + 4..].trim_start_matches(['\'', '"', ' ']);
        if !after.starts_with('#') {
            return Err("the SVG is not static: url() must point to an id (#...)".to_string());
        }
        rest = &rest[p + 4..];
    }
    Ok(())
}

fn check_tag(tag: &str) -> Result<(), String> {
    let mut rest = tag;
    while let Some(eq) = rest.find('=') {
        let name = rest[..eq]
            .rsplit(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '<')
            .next()
            .unwrap_or("");
        let after = rest[eq + 1..].trim_start();
        let (value, consumed) = match after.chars().next() {
            Some(q @ ('"' | '\'')) => match after[1..].find(q) {
                Some(e) => (&after[1..1 + e], 2 + e),
                None => (&after[1..], after.len()),
            },
            _ => {
                let e = after
                    .find(|c: char| c.is_whitespace())
                    .unwrap_or(after.len());
                (&after[..e], e)
            }
        };
        if name.starts_with("on") && name.len() > 2 {
            return Err(format!("the SVG is not static: event attribute \"{name}\""));
        }
        if matches!(name, "href" | "xlink:href" | "src") {
            let v = value.trim();
            let image = v.starts_with("data:image/png;")
                || v.starts_with("data:image/jpeg;")
                || v.starts_with("data:image/webp;");
            if !(v.starts_with('#') || image) {
                return Err(format!("the SVG is not static: external {name} \"{v}\""));
            }
        }
        rest = &after[consumed.min(after.len())..];
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn png(w: u32, h: u32) -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        v.extend(w.to_be_bytes());
        v.extend(h.to_be_bytes());
        v.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
        v
    }

    #[test]
    fn image_caps() {
        assert!(check_image(FileKind::Png, &png(1440, 900)).is_ok());
        assert!(check_image(FileKind::Png, &png(5000, 10)).is_err());
        assert!(check_image(FileKind::Png, b"not a png at all, not a png at all").is_err());
    }

    #[test]
    fn svg_must_be_static() {
        assert!(check_svg(br##"<svg xmlns="http://www.w3.org/2000/svg"><rect width="1" fill="url(#g)"/><use href="#a"/></svg>"##).is_ok());
        for bad in [
            r#"<svg><script>alert(1)</script></svg>"#,
            r#"<svg><foreignObject/></svg>"#,
            r#"<svg onload="x()"></svg>"#,
            r#"<svg><image href="https://evil.example/a.png"/></svg>"#,
            r#"<svg><a xlink:href="//evil"/></svg>"#,
            r#"<svg><rect style="fill:url(https://x)"/></svg>"#,
        ] {
            assert!(check_svg(bad.as_bytes()).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_bad_font_fails() {
        assert!(check_font("assets/fonts/a.ttf", b"not a font").is_err());
    }
}
