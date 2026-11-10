//! `theme/get` and `theme/asset`: read-only views of an installed theme. A display reads JSON
//! only (ADR 0024 section 10). The theme is checked on each load.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use aivory_protocol::theme::{DisplayClass, ThemeAsset, ThemeBundle, ThemeFont};
use aivory_theme::archive::sha256_hex;
use aivory_theme::caps;
use aivory_theme::paths::{FileKind, check_relative, kind_of};
use aivory_theme::validate::{Options, format_issues, validate_dir};
use aivory_theme::{assets, package, strings};
use base64::Engine as _;

use super::lock::Lock;
use super::store::Store;

/// Check the theme and build the bundle of one display class.
pub fn get(store: &Store, id: &str, class: DisplayClass) -> Result<ThemeBundle, String> {
    let dir = store.dir(id)?;
    if !store.exists(id) {
        return Err(format!("theme \"{id}\" is not installed"));
    }
    let report = validate_dir(&dir, &Options::default());
    let Some(pkg) = report.package.filter(|_| report.issues.is_empty()) else {
        return Err(format!(
            "the theme is not valid:\n{}",
            format_issues(&report.issues)
        ));
    };
    let files = package::scan(&dir).map_err(|i| format_issues(&i))?;
    let mut fonts = Vec::new();
    let mut texts = BTreeMap::new();
    let mut preview = None;
    for f in &files {
        match kind_of(&f.rel) {
            Some(FileKind::Font) if f.rel.starts_with("assets/fonts/") => {
                let bytes = fs::read(&f.abs).map_err(|e| format!("{}: {e}", f.rel))?;
                if let Ok(family) = assets::check_font(&f.rel, &bytes) {
                    fonts.push(ThemeFont {
                        file: f.rel.clone(),
                        family,
                    });
                }
            }
            Some(FileKind::Toml) => {
                if let Some(lang) = strings::lang_of(&f.rel) {
                    let text = fs::read_to_string(&f.abs).map_err(|e| format!("{}: {e}", f.rel))?;
                    if let Ok(map) = strings::parse(&f.rel, &text) {
                        texts.insert(lang.to_string(), map);
                    }
                }
            }
            _ => {}
        }
        if f.rel == format!("previews/{}.png", class.dir()) {
            preview = Some(f.rel.clone());
        }
    }
    let sha256 = match Lock::read(&dir) {
        Ok(l) => l.archive_sha256,
        Err(_) => sha256_hex(pkg.tokens.to_string().as_bytes()),
    };
    Ok(ThemeBundle {
        id: id.to_string(),
        name: pkg.manifest.name.clone(),
        version: pkg.manifest.version.clone(),
        author: pkg.manifest.author.name.clone(),
        license: pkg.manifest.license.clone(),
        sha256,
        display: class,
        tokens: pkg.tokens.clone(),
        layouts: pkg.layouts.get(&class).cloned().unwrap_or_default(),
        parts: pkg.parts.clone(),
        strings: texts,
        fonts,
        preview,
    })
}

/// One file of `assets/` or `previews/`, within the asset cap.
pub fn asset(store: &Store, id: &str, path: &str) -> Result<ThemeAsset, String> {
    let dir = store.dir(id)?;
    if !store.exists(id) {
        return Err(format!("theme \"{id}\" is not installed"));
    }
    check_relative(path)?;
    if !(path.starts_with("assets/") || path.starts_with("previews/")) {
        return Err("only files of assets/ and previews/ are served".to_string());
    }
    let file = dir.join(path);
    read_asset(&dir, &file, path)
}

fn read_asset(dir: &Path, file: &Path, path: &str) -> Result<ThemeAsset, String> {
    let real = file
        .canonicalize()
        .map_err(|_| format!("no file \"{path}\" in the theme"))?;
    let root = dir.canonicalize().map_err(|e| e.to_string())?;
    if !real.starts_with(&root) {
        return Err("the path leaves the theme folder".to_string());
    }
    let meta = fs::metadata(&real).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err(format!("\"{path}\" is not a file"));
    }
    if meta.len() > caps::ASSET_BYTES {
        return Err(format!(
            "the file is larger than {} bytes",
            caps::ASSET_BYTES
        ));
    }
    let bytes = fs::read(&real).map_err(|e| e.to_string())?;
    Ok(ThemeAsset {
        path: path.to_string(),
        bytes: bytes.len() as u64,
        base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(id: &str) -> (tempfile::TempDir, Store) {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("themes"));
        let dir = store.dir(id).unwrap();
        for (rel, bytes) in aivory_theme::sample::files(id, "1.0.0") {
            let p = dir.join(&rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, bytes).unwrap();
        }
        (tmp, store)
    }

    #[test]
    fn get_serves_tokens_layouts_and_strings() {
        let (_t, store) = installed("my-theme");
        let b = get(&store, "my-theme", DisplayClass::Desktop).unwrap();
        assert_eq!(b.id, "my-theme");
        assert_eq!(b.tokens["colors"]["bg"], "#ffffff");
        assert_eq!(b.layouts.len(), 7);
        assert_eq!(b.strings["en"]["title"], "Fleet");
        assert!(
            get(&store, "my-theme", DisplayClass::Phone)
                .unwrap()
                .layouts
                .is_empty()
        );
        assert!(get(&store, "other-one", DisplayClass::Desktop).is_err());
    }

    #[test]
    fn asset_stays_inside_assets_and_previews() {
        let (_t, store) = installed("my-theme");
        let dir = store.dir("my-theme").unwrap();
        fs::create_dir_all(dir.join("previews")).unwrap();
        fs::write(dir.join("previews/desktop.png"), b"png").unwrap();
        let a = asset(&store, "my-theme", "previews/desktop.png").unwrap();
        assert_eq!(a.bytes, 3);
        assert_eq!(a.base64, "cG5n");
        for bad in [
            "theme.toml",
            "../x",
            "assets/../theme.toml",
            "/etc/passwd",
            "assets/none.png",
        ] {
            assert!(asset(&store, "my-theme", bad).is_err(), "{bad}");
        }
    }
}
