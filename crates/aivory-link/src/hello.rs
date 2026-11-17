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

/// The first request on a mesh connection (ALPN [`crate::MESH_ALPN`]).
pub const MESH_HELLO_METHOD: &str = "mesh/hello";

/// A peer asks who the other machine is now. Result: `aivory_protocol::mesh::MeshInfo`.
pub const MESH_PING_METHOD: &str = "mesh/ping";

/// Params of `mesh/hello`: a daemon says who it is to a peer daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshHello {
    /// The code of the invite link. Needed only on the first connection (`peers/join`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// The node id of the caller (64 hex). It must be the key that the QUIC handshake proved.
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub version: String,
    /// The current direct addresses of the caller (`ip:port`).
    #[serde(default)]
    pub direct: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay: Option<String>,
}

/// Result of `mesh/hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshHelloReply {
    pub node: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub direct: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay: Option<String>,
    /// `true` when this connection paired the two machines.
    #[serde(default)]
    pub newly_paired: bool,
}
