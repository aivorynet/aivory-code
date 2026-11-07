//! Download of theme archives (ADR 0024 section 8 and 9). Only `aivoryd` downloads, only on a
//! user command, only over HTTPS (also on each redirect), and with a size stop.

use std::fmt;
use std::io::Read;
use std::time::Duration;

use serde::Deserialize;

/// The cap of an archive.
pub const ARCHIVE_MAX: u64 = aivory_theme::caps::ARCHIVE_BYTES;
/// The cap of a JSON answer (a release, a registry index).
pub const JSON_MAX: u64 = 2 * 1024 * 1024;

/// A failed download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchError {
    pub status: Option<u16>,
    pub message: String,
}

impl FetchError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            status: None,
            message: message.into(),
        }
    }
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// The download seam. Tests use a fake. `get` returns at most `max_bytes` bytes; more is an
/// error.
pub trait Fetcher: Send + Sync + fmt::Debug {
    fn get(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, FetchError>;
}

/// The real fetcher: `ureq` with rustls. It follows at most 5 redirects itself and refuses a
/// redirect to a URL that is not HTTPS.
#[derive(Debug)]
pub struct HttpFetcher {
    agent: ureq::Agent,
}

impl Default for HttpFetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpFetcher {
    pub fn new() -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .https_only(true)
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(60)))
            .user_agent("aivoryd-theme")
            .build()
            .into();
        Self { agent }
    }
}

/// Require an `https://` URL.
pub fn require_https(url: &str) -> Result<(), FetchError> {
    if url.starts_with("https://") && url.len() <= 2048 && !url.contains(char::is_whitespace) {
        Ok(())
    } else {
        Err(FetchError::new(format!(
            "only HTTPS URLs are allowed, not \"{url}\""
        )))
    }
}

fn origin(url: &str) -> &str {
    let rest = &url["https://".len()..];
    let end = rest.find('/').map_or(url.len(), |i| "https://".len() + i);
    &url[..end]
}

impl Fetcher for HttpFetcher {
    fn get(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, FetchError> {
        let mut url = url.to_string();
        for _ in 0..=5 {
            require_https(&url)?;
            let mut resp = self
                .agent
                .get(&url)
                .header(
                    "Accept",
                    "application/vnd.github+json, application/octet-stream, */*",
                )
                .call()
                .map_err(|e| FetchError::new(format!("cannot reach {url}: {e}")))?;
            let status = resp.status().as_u16();
            if (300..400).contains(&status) {
                let next = resp
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| FetchError::new("a redirect without a Location header"))?
                    .to_string();
                url = if next.starts_with('/') {
                    format!("{}{next}", origin(&url))
                } else {
                    next
                };
                continue;
            }
            if status != 200 {
                return Err(FetchError {
                    status: Some(status),
                    message: format!("{url} answered with status {status}"),
                });
            }
            let mut body = Vec::new();
            resp.body_mut()
                .as_reader()
                .take(max_bytes + 1)
                .read_to_end(&mut body)
                .map_err(|e| FetchError::new(format!("download failed: {e}")))?;
            if body.len() as u64 > max_bytes {
                return Err(FetchError::new(format!(
                    "the download is larger than {max_bytes} bytes"
                )));
            }
            return Ok(body);
        }
        Err(FetchError::new("too many redirects"))
    }
}

/// Where a theme comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub owner: String,
    pub repo: String,
    pub tag: Option<String>,
}

fn name_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn tag_ok(s: &str) -> bool {
    name_ok(s)
        || (s.len() <= 100
            && !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+'))
            && s != "."
            && s != "..")
}

impl Source {
    /// `github:<owner>/<repo>[@<tag>]`, `https://github.com/<owner>/<repo>`, or
    /// `https://github.com/<owner>/<repo>/releases/tag/<tag>`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let bad = || {
            format!(
                "cannot read the source \"{text}\"; use github:<owner>/<repo>[@<tag>] or a https://github.com/<owner>/<repo> link"
            )
        };
        let (path, tag) = if let Some(rest) = text.strip_prefix("github:") {
            match rest.split_once('@') {
                Some((p, t)) => (p.to_string(), Some(t.to_string())),
                None => (rest.to_string(), None),
            }
        } else if let Some(rest) = text.strip_prefix("https://github.com/") {
            let rest = rest
                .split(['?', '#'])
                .next()
                .unwrap_or(rest)
                .trim_end_matches('/');
            let parts: Vec<&str> = rest.split('/').collect();
            match parts.as_slice() {
                [o, r] => (format!("{o}/{r}"), None),
                [o, r, "releases", "tag", t] => (format!("{o}/{r}"), Some((*t).to_string())),
                _ => return Err(bad()),
            }
        } else {
            return Err(bad());
        };
        let (owner, repo) = path.split_once('/').ok_or_else(bad)?;
        let repo = repo.strip_suffix(".git").unwrap_or(repo);
        if !name_ok(owner) || !name_ok(repo) {
            return Err(bad());
        }
        if let Some(t) = &tag
            && !tag_ok(t)
        {
            return Err(format!("\"{t}\" is not a valid tag"));
        }
        Ok(Self {
            owner: owner.to_string(),
            repo: repo.to_string(),
            tag,
        })
    }

    /// The text that `theme.lock` stores: `github:<owner>/<repo>`.
    pub fn canonical(&self) -> String {
        format!("github:{}/{}", self.owner, self.repo)
    }

    pub fn with_tag(&self, tag: Option<String>) -> Self {
        Self {
            tag,
            ..self.clone()
        }
    }
}

/// A resolved download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub tag: String,
    pub url: String,
    /// The archive is the zip of the tag: it has a top folder.
    pub strip_top: bool,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

fn tag_zip(src: &Source, tag: &str) -> Resolved {
    Resolved {
        tag: tag.to_string(),
        url: format!(
            "https://github.com/{}/{}/archive/refs/tags/{tag}.zip",
            src.owner, src.repo
        ),
        strip_top: true,
    }
}

/// Find the archive of a release. Without a tag: the latest release. The release asset
/// `*.aivory-theme.zip` wins; without one, the zip of the tag.
pub fn resolve(f: &dyn Fetcher, src: &Source) -> Result<Resolved, String> {
    let api = match &src.tag {
        Some(t) => format!(
            "https://api.github.com/repos/{}/{}/releases/tags/{t}",
            src.owner, src.repo
        ),
        None => format!(
            "https://api.github.com/repos/{}/{}/releases/latest",
            src.owner, src.repo
        ),
    };
    let body = match f.get(&api, JSON_MAX) {
        Ok(b) => b,
        Err(e) if e.status == Some(404) => {
            return match &src.tag {
                // A tag without a release: take the zip of the tag.
                Some(t) => Ok(tag_zip(src, t)),
                None => Err(format!("{} has no release", src.canonical())),
            };
        }
        Err(e) => return Err(e.to_string()),
    };
    let release: Release = serde_json::from_slice(&body)
        .map_err(|e| format!("unexpected answer of the release API: {e}"))?;
    if !tag_ok(&release.tag_name) {
        return Err("the release has an invalid tag name".to_string());
    }
    let prefix = format!(
        "https://github.com/{}/{}/releases/download/",
        src.owner, src.repo
    );
    if let Some(a) = release.assets.iter().find(|a| {
        a.name.ends_with(".aivory-theme.zip") && a.browser_download_url.starts_with(&prefix)
    }) {
        return Ok(Resolved {
            tag: release.tag_name,
            url: a.browser_download_url.clone(),
            strip_top: false,
        });
    }
    Ok(tag_zip(src, &release.tag_name))
}

/// The bytes of an archive with its origin.
#[derive(Debug, Clone)]
pub struct Downloaded {
    pub bytes: Vec<u8>,
    pub tag: String,
    pub strip_top: bool,
}

/// Resolve and download. The stream stops at 10 MB.
pub fn download(f: &dyn Fetcher, src: &Source) -> Result<Downloaded, String> {
    let r = resolve(f, src)?;
    let bytes = f.get(&r.url, ARCHIVE_MAX).map_err(|e| e.to_string())?;
    Ok(Downloaded {
        bytes,
        tag: r.tag,
        strip_top: r.strip_top,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_parse() {
        let s = Source::parse("github:acme/neon-theme@v1.2.0").unwrap();
        assert_eq!(
            (s.owner.as_str(), s.repo.as_str(), s.tag.as_deref()),
            ("acme", "neon-theme", Some("v1.2.0"))
        );
        assert_eq!(
            Source::parse("https://github.com/acme/neon-theme")
                .unwrap()
                .tag,
            None
        );
        let s = Source::parse("https://github.com/acme/neon-theme/releases/tag/v2.0.0").unwrap();
        assert_eq!(s.tag.as_deref(), Some("v2.0.0"));
        for bad in [
            "http://github.com/a/b",
            "https://evil.example/a/b",
            "github:a",
            "github:../b",
            "github:a/b@../x",
            "https://github.com/a/b/tree/main",
            "ftp://x",
            "github:a/b c",
        ] {
            assert!(Source::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn https_only() {
        assert!(require_https("https://x.example/a").is_ok());
        assert!(require_https("http://x.example/a").is_err());
        assert!(require_https("file:///etc/passwd").is_err());
    }
}
