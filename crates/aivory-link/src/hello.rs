//! The first message on a link stream: the device says who it is.
//!
//! The phone sends the JSON-RPC request `link/hello`. The daemon answers with a [`HelloReply`]
//! or with a JSON-RPC error (code [`UNAUTHORIZED`]) and closes the connection.

use serde::{Deserialize, Serialize};

pub const HELLO_METHOD: &str = "link/hello";

/// The JSON-RPC error code of a rejected device.
pub const UNAUTHORIZED: i64 = -32001;

/// Params of `link/hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    /// The pairing code. Needed only on the first connection of a device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default)]
    pub device: String,
    #[serde(default)]
    pub os: String,
    /// The Ed25519 public key of the device, 64 hex characters. It must be the key that the
    /// QUIC handshake proved.
    pub key: String,
    /// `true`: the phone has no LAN token and wants one. The daemon answers with a new token
    /// ([`HelloReply::token`]), so that the phone can also use the LAN link with this pairing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub need_token: bool,
}

/// Result of `link/hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloReply {
    pub device_id: String,
    pub name: String,
    /// `true` when this connection paired the device.
    pub newly_paired: bool,
    /// The node id of the daemon (hex).
    pub node: String,
    /// A LAN token for `?token=` on the WebSocket link. It comes on a new pairing and when the
    /// hello had `need_token`. The daemon keeps only its hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}
