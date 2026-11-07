//! Just enough HTTP/1.1 for one local MCP client: one request per connection, no chunked bodies.

use std::io::{self, Read, Write};
use std::net::TcpStream;

const MAX_HEAD_BYTES: usize = 32 * 1024;
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug)]
pub(super) struct Request {
    pub method: String,
    pub path: String,
    headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    /// The first header with this name (ASCII case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug)]
pub(super) enum ReadError {
    Io,
    /// The request is malformed. The text says why.
    Bad(&'static str),
    TooLarge,
}

impl From<io::Error> for ReadError {
    fn from(_: io::Error) -> Self {
        Self::Io
    }
}

/// Read one request: the head, then `Content-Length` bytes of body.
pub(super) fn read_request(stream: &mut TcpStream) -> Result<Request, ReadError> {
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > MAX_HEAD_BYTES {
            return Err(ReadError::TooLarge);
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(ReadError::Bad("the connection closed inside the head"));
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = std::str::from_utf8(&buf[..head_end])
        .map_err(|_| ReadError::Bad("the head is not UTF-8"))?;
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split(' ');
    let (Some(method), Some(path), Some(version)) = (first.next(), first.next(), first.next())
    else {
        return Err(ReadError::Bad("the request line is malformed"));
    };
    if !version.starts_with("HTTP/1.") {
        return Err(ReadError::Bad("only HTTP/1.x is supported"));
    }
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    let mut req = Request {
        method: method.to_string(),
        path: path.to_string(),
        headers,
        body: Vec::new(),
    };
    if req.header("transfer-encoding").is_some() {
        return Err(ReadError::Bad("chunked request bodies are not supported"));
    }
    let len: usize = match req.header("content-length") {
        None => 0,
        Some(v) => v
            .parse()
            .map_err(|_| ReadError::Bad("the content length is not a number"))?,
    };
    if len > MAX_BODY_BYTES {
        return Err(ReadError::TooLarge);
    }
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(ReadError::Bad("the connection closed inside the body"));
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(len);
    req.body = body;
    Ok(req)
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        _ => "Error",
    }
}

/// Write a complete response and flush it. The connection closes after it.
pub(super) fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: Option<&str>,
    extra: &[(&str, &str)],
    body: &[u8],
) -> io::Result<()> {
    let mut head = format!("HTTP/1.1 {status} {}\r\n", reason(status));
    if let Some(ct) = content_type {
        head.push_str(&format!("Content-Type: {ct}\r\n"));
    }
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    ));
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

/// Start a server-sent-events response. The body has no length; it ends when the connection
/// closes. This lets the server keep the call open for as long as the user needs.
pub(super) fn write_sse_head(stream: &mut TcpStream) -> io::Result<()> {
    stream.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
    )?;
    stream.flush()
}

/// Write one chunk of an SSE body and flush it.
pub(super) fn write_sse(stream: &mut TcpStream, text: &str) -> io::Result<()> {
    stream.write_all(text.as_bytes())?;
    stream.flush()
}
