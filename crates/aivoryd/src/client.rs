//! A minimal blocking JSON-RPC client for the daemon endpoint.

use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

use crate::endpoint::{self, Endpoint};

/// Call one method. `on_notification` receives every notification that arrives before the
/// response. Returns the full response object (`result` or `error`).
pub fn call(
    endpoint: &Endpoint,
    method: &str,
    params: Value,
    on_notification: &mut dyn FnMut(&Value),
) -> io::Result<Value> {
    let mut conn = endpoint::connect(endpoint)?;
    let request = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let mut bytes = serde_json::to_vec(&request).map_err(io::Error::other)?;
    bytes.push(b'\n');
    conn.writer.write_all(&bytes)?;
    conn.writer.flush()?;
    let mut line = String::new();
    loop {
        line.clear();
        if conn.reader.read_line(&mut line)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the daemon closed the connection before it answered",
            ));
        }
        let value: Value = serde_json::from_str(&line).map_err(io::Error::other)?;
        if value.get("id").is_some_and(|id| !id.is_null()) {
            return Ok(value);
        }
        on_notification(&value);
    }
}
