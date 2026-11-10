//! One user turn of a live run: the text, image files, and a new mode, model, or effort.

use std::fs;
use std::path::{Path, PathBuf};

use aivory_protocol::Line;
use base64::Engine as _;

use super::StartOptions;

/// The largest image file that a turn sends as data. A larger file goes as a path.
pub const IMAGE_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// One user turn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Input {
    pub text: String,
    /// Image files (png, jpeg, gif, webp).
    pub images: Vec<PathBuf>,
    /// A new model for this turn and the turns after it. `None` keeps the model.
    pub model: Option<String>,
    /// A new effort level (`low`, `medium`, `high`, `xhigh`, `max`). Gemini ignores it.
    pub effort: Option<String>,
    /// A new permission mode, in the words of the CLI (see [`StartOptions::permission_mode`]).
    pub permission_mode: Option<String>,
    /// Codex only: a new sandbox mode.
    pub sandbox: Option<String>,
}

/// An image file read into memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub path: PathBuf,
    pub mime: &'static str,
    pub bytes: Vec<u8>,
}

impl Image {
    pub fn base64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(&self.bytes)
    }
}

/// The media type of an image file by its extension. `None` for a file that is no image the
/// agents take.
pub fn image_mime(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    })
}

impl Input {
    pub fn text(text: &str) -> Self {
        Self {
            text: text.to_string(),
            ..Self::default()
        }
    }

    /// The first turn of a run: the prompt and the images of the start options.
    pub fn first(opts: &StartOptions) -> Self {
        Self {
            text: opts.prompt.clone(),
            images: opts.images.clone(),
            ..Self::default()
        }
    }

    /// Read the images. A file that is missing, too large, or of another type is skipped and
    /// comes back in the second list, so the caller can name it by path.
    pub fn load_images(&self) -> (Vec<Image>, Vec<PathBuf>) {
        let mut ok = Vec::new();
        let mut skipped = Vec::new();
        for path in &self.images {
            let image = image_mime(path).and_then(|mime| {
                let len = fs::metadata(path).ok()?.len();
                if len == 0 || len > IMAGE_MAX_BYTES {
                    return None;
                }
                let bytes = fs::read(path).ok()?;
                Some(Image {
                    path: path.clone(),
                    mime,
                    bytes,
                })
            });
            match image {
                Some(i) => ok.push(i),
                None => skipped.push(path.clone()),
            }
        }
        (ok, skipped)
    }

    /// The transcript lines of this turn: the user text, then one media line per image.
    /// `skipped` images also get a meta line that says why they went as a path.
    pub fn lines(&self, skipped: &[PathBuf]) -> Vec<Line> {
        let mut out = vec![Line::user(&self.text)];
        for path in &self.images {
            let src = path.to_string_lossy();
            out.push(match aivory_protocol::Media::of(&src) {
                Some(m) => Line::media(m),
                None => Line::meta(format!("image {src}")),
            });
        }
        for path in skipped {
            out.push(Line::meta(format!(
                "image {} not sent: missing, too large, or no png, jpeg, gif, or webp",
                path.display()
            )));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_load_by_type_and_size() {
        let dir = std::env::temp_dir().join(format!("aivory-input-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let png = dir.join("a.png");
        fs::write(&png, [0x89, b'P', b'N', b'G']).unwrap();
        let txt = dir.join("b.txt");
        fs::write(&txt, "x").unwrap();
        let input = Input {
            text: "look".into(),
            images: vec![png.clone(), txt.clone(), dir.join("missing.jpg")],
            ..Input::default()
        };
        let (ok, skipped) = input.load_images();
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].mime, "image/png");
        assert_eq!(ok[0].base64(), "iVBORw==");
        assert_eq!(skipped, vec![txt, dir.join("missing.jpg")]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn mime_by_extension() {
        assert_eq!(image_mime(Path::new("x.JPG")), Some("image/jpeg"));
        assert_eq!(image_mime(Path::new("x.webp")), Some("image/webp"));
        assert_eq!(image_mime(Path::new("x.pdf")), None);
    }
}
