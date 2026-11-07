//! The pairing link of the QR code:
//! `aivory://pair?node=<node id>&relay=<relay url>&host=<ip:port>&direct=<ip:port,...>&code=<AIV-xxxxxx>`.
//! Every field is optional on parse; the order on output is fixed.

use crate::LinkError;

const PREFIX: &str = "aivory://pair?";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PairLink {
    /// The iroh node id of the daemon (hex).
    pub node: Option<String>,
    pub relay: Option<String>,
    /// The LAN address of the WebSocket link.
    pub host: Option<String>,
    /// The direct addresses of the iroh endpoint of the daemon (`ip:port`). A phone dials them
    /// when the daemon has no relay.
    pub direct: Vec<String>,
    pub code: Option<String>,
}

impl PairLink {
    pub fn to_url(&self) -> String {
        let fields = [
            ("node", &self.node),
            ("relay", &self.relay),
            ("host", &self.host),
            ("code", &self.code),
        ];
        let mut query: Vec<String> = fields
            .iter()
            .filter_map(|(k, v)| v.as_ref().map(|v| format!("{k}={}", encode(v))))
            .collect();
        if !self.direct.is_empty() {
            let list: Vec<String> = self.direct.iter().map(|a| encode(a)).collect();
            // `direct` comes before `code`.
            let at = query
                .iter()
                .position(|f| f.starts_with("code="))
                .unwrap_or(query.len());
            query.insert(at, format!("direct={}", list.join(",")));
        }
        format!("{PREFIX}{}", query.join("&"))
    }

    pub fn parse(url: &str) -> Result<Self, LinkError> {
        let query = url
            .strip_prefix(PREFIX)
            .ok_or_else(|| LinkError::Invalid("not an aivory://pair link".into()))?;
        let mut link = Self::default();
        for (k, raw) in query.split('&').filter_map(|kv| kv.split_once('=')) {
            let v = decode(raw);
            match k {
                "node" => link.node = Some(v),
                "relay" => link.relay = Some(v),
                "host" => link.host = Some(v),
                "direct" => {
                    link.direct = raw
                        .split(',')
                        .map(decode)
                        .filter(|a| !a.is_empty())
                        .collect();
                }
                "code" => link.code = Some(v),
                _ => {}
            }
        }
        Ok(link)
    }
}

/// Percent-encode everything except unreserved characters and `:`.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(b) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_round_trips() {
        let link = PairLink {
            node: Some("ab".repeat(32)),
            relay: Some("https://relay.example.com./path?x=1".into()),
            host: Some("192.168.1.5:7420".into()),
            direct: vec!["192.168.1.5:51000".into(), "[fe80::1]:51000".into()],
            code: Some("AIV-0A1B2C".into()),
        };
        let url = link.to_url();
        assert!(
            url.contains("&direct=192.168.1.5:51000,%5Bfe80::1%5D:51000&code="),
            "{url}"
        );
        assert!(url.starts_with("aivory://pair?node="));
        assert!(!url.contains("https://"));
        assert_eq!(PairLink::parse(&url).unwrap(), link);
    }

    #[test]
    fn a_lan_link_keeps_the_old_shape() {
        let link = PairLink {
            host: Some("127.0.0.1:7420".into()),
            code: Some("AIV-000000".into()),
            ..PairLink::default()
        };
        assert_eq!(
            link.to_url(),
            "aivory://pair?host=127.0.0.1:7420&code=AIV-000000"
        );
        assert!(PairLink::parse("http://x").is_err());
    }
}
