//! Media of a transcript: an image, a video, a PDF, or a web link that a line shows as a
//! preview. A client without media support shows the text of the line.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum MediaKind {
    Image,
    Video,
    Pdf,
    /// A web page.
    Link,
}

/// One media item. `src` is an absolute file path on the daemon machine or an `http(s)` URL.
/// A remote client reads a file through `media/get`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Media {
    pub kind: MediaKind,
    pub src: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
}

impl Media {
    /// The media of a path or a URL by its extension. A URL without a media extension is a
    /// link; a path without one is `None`.
    pub fn of(src: &str) -> Option<Self> {
        let web = src.starts_with("http://") || src.starts_with("https://");
        let bare = src.split(['?', '#']).next().unwrap_or(src);
        let ext = bare
            .rsplit_once('.')
            .map(|(head, e)| (head, e.to_ascii_lowercase()))
            .filter(|(head, e)| !head.is_empty() && !e.contains(['/', '\\']))
            .map(|(_, e)| e);
        let (kind, mime) = match ext.as_deref() {
            Some("png") => (MediaKind::Image, "image/png"),
            Some("jpg" | "jpeg") => (MediaKind::Image, "image/jpeg"),
            Some("gif") => (MediaKind::Image, "image/gif"),
            Some("webp") => (MediaKind::Image, "image/webp"),
            Some("svg") => (MediaKind::Image, "image/svg+xml"),
            Some("mp4") => (MediaKind::Video, "video/mp4"),
            Some("webm") => (MediaKind::Video, "video/webm"),
            Some("mov") => (MediaKind::Video, "video/quicktime"),
            Some("pdf") => (MediaKind::Pdf, "application/pdf"),
            _ if web => (MediaKind::Link, ""),
            _ => return None,
        };
        Some(Self {
            kind,
            src: src.to_string(),
            mime: (!mime.is_empty()).then(|| mime.to_string()),
        })
    }

    /// The file name or the host and path of the source, for a caption.
    pub fn name(&self) -> &str {
        let s = self.src.trim_end_matches(['/', '\\']);
        if self.kind == MediaKind::Link {
            return s
                .trim_start_matches("https://")
                .trim_start_matches("http://");
        }
        s.rsplit(['/', '\\']).next().unwrap_or(s)
    }
}

/// `media/get`: read a media file of the daemon machine (a remote client shows a preview).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MediaGetParams {
    /// The absolute path of [`Media::src`].
    pub src: String,
}

/// The bytes of a media file, as base64.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MediaData {
    pub mime: String,
    pub data: String,
    pub size: u64,
}

/// The largest file that `media/get` sends.
pub const MEDIA_GET_MAX_BYTES: u64 = 20 * 1024 * 1024;

/// The media that a text names: absolute file paths (`C:\…`, `/…`, `~/…`, also as `@path`)
/// and `http(s)` URLs. Each source shows once, in text order. At most `max` items.
pub fn scan(text: &str, max: usize) -> Vec<Media> {
    let mut out: Vec<Media> = Vec::new();
    for raw in text.split(|c: char| {
        c.is_whitespace() || matches!(c, '(' | ')' | '<' | '>' | '"' | '`' | '\'' | '[' | ']')
    }) {
        if out.len() >= max {
            break;
        }
        let word = raw
            .trim_start_matches('@')
            .trim_end_matches(['.', ',', ';', ':', '!', '?']);
        let web = word.starts_with("http://") || word.starts_with("https://");
        let drive = word.len() > 3
            && word.as_bytes()[1] == b':'
            && matches!(word.as_bytes()[2], b'\\' | b'/')
            && word.as_bytes()[0].is_ascii_alphabetic();
        let unix = word.starts_with('/') && word.len() > 1 && !word.starts_with("//");
        let home = word.starts_with("~/");
        if !(web || drive || unix || home) {
            continue;
        }
        let Some(m) = Media::of(word) else { continue };
        // A bare link in text is common; only media links and files are previews.
        if m.kind == MediaKind::Link {
            continue;
        }
        if !out.iter().any(|x| x.src == m.src) {
            out.push(m);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_by_extension() {
        assert_eq!(Media::of("C:/x/a.PNG").unwrap().kind, MediaKind::Image);
        assert_eq!(Media::of("/tmp/v.mp4").unwrap().kind, MediaKind::Video);
        assert_eq!(
            Media::of("https://e.org/doc.pdf?x=1").unwrap().kind,
            MediaKind::Pdf
        );
        assert_eq!(
            Media::of("https://e.org/page").unwrap().kind,
            MediaKind::Link
        );
        assert_eq!(Media::of("C:/x/main.rs"), None);
        assert_eq!(Media::of("C:/x.dir/file"), None);
        assert_eq!(Media::of("C:/x/shot.png").unwrap().name(), "shot.png");
    }

    #[test]
    fn scan_finds_paths_and_media_urls_once() {
        let text = "Saved C:\\out\\chart.png and /tmp/demo.webm, see https://e.org/a.jpg \
                    and https://e.org/docs (not media). Again @C:\\out\\chart.png. src/lib.rs.";
        let found: Vec<String> = scan(text, 10).into_iter().map(|m| m.src).collect();
        assert_eq!(
            found,
            [
                "C:\\out\\chart.png",
                "/tmp/demo.webm",
                "https://e.org/a.jpg"
            ]
        );
        assert_eq!(scan(text, 1).len(), 1);
    }
}
