//! Build the iroh endpoint of the daemon or of the phone.
//!
//! The endpoint uses no address lookup service (no DNS or Pkarr publishing): the pairing link
//! carries the node id and the relay URL, so the daemon publishes nothing to a third party.

use std::net::SocketAddr;

use iroh::endpoint::presets;
use iroh::{Endpoint, RelayMode, RelayUrl, SecretKey};

use crate::{ALPN, LinkError};

/// Which relay an endpoint uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum RelayChoice {
    /// The iroh default relays.
    #[default]
    Default,
    /// One relay of our own (setting `remote.relay`).
    Custom(String),
    /// No relay: direct paths only (tests, a LAN).
    Disabled,
}

impl RelayChoice {
    /// An empty text or `default` means the default relays; `none` or `off` disables the relay.
    pub fn from_setting(text: Option<&str>) -> Self {
        match text.map(str::trim) {
            None | Some("" | "default") => Self::Default,
            Some("none" | "off") => Self::Disabled,
            Some(url) => Self::Custom(url.to_string()),
        }
    }

    /// The relay URL that a pairing link names, when it is known without a network call.
    pub fn url(&self) -> Option<&str> {
        match self {
            Self::Custom(url) => Some(url),
            _ => None,
        }
    }

    fn mode(&self) -> Result<RelayMode, LinkError> {
        Ok(match self {
            Self::Default => iroh::endpoint::default_relay_mode(),
            Self::Disabled => RelayMode::Disabled,
            Self::Custom(url) => RelayMode::custom([parse_relay(url)?]),
        })
    }
}

pub fn parse_relay(url: &str) -> Result<RelayUrl, LinkError> {
    url.trim()
        .parse::<RelayUrl>()
        .map_err(|e| LinkError::Invalid(format!("relay URL '{url}': {e}")))
}

#[derive(Debug, Clone)]
pub struct EndpointConfig {
    pub secret: SecretKey,
    pub relay: RelayChoice,
    /// Bind one IPv4 socket on this address (for example `127.0.0.1:0` in tests). `None`:
    /// all interfaces.
    pub bind: Option<SocketAddr>,
    /// `true` for the daemon: accept connections with the ALPN of the link.
    pub accept: bool,
}

/// Bind the endpoint.
pub async fn bind(config: EndpointConfig) -> Result<Endpoint, LinkError> {
    let mut builder = Endpoint::builder(presets::Minimal)
        .secret_key(config.secret)
        .relay_mode(config.relay.mode()?);
    if config.accept {
        builder = builder.alpns(vec![ALPN.to_vec()]);
    }
    if let Some(addr) = config.bind {
        builder = builder
            .clear_ip_transports()
            .bind_addr(addr)
            .map_err(|e| LinkError::Invalid(format!("bind address {addr}: {e}")))?;
    }
    builder
        .bind()
        .await
        .map_err(|e| LinkError::Connect(format!("cannot bind the iroh endpoint: {e}")))
}
