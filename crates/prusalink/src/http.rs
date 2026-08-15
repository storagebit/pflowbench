// Minimal HTTP/1.1 over TcpStream: one request per connection
// (Connection: close), Content-Length and chunked response bodies.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    pub fn body_str(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

pub(crate) fn raw_request(
    addr: &str,
    method: &str,
    path: &str,
    headers: &[(String, String)],
    body: &[u8],
    timeout: Duration,
) -> std::io::Result<Response> {
    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let host = addr.split(':').next().unwrap_or(addr);
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if !body.is_empty() || method == "PUT" || method == "POST" {
        req.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    req.push_str("\r\n");
    let mut wire = req.into_bytes();
    wire.extend_from_slice(body);
    stream.write_all(&wire)?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "no header end"))?;
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut body_bytes = raw[split + 4..].to_vec();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "bad status line"))?;
    let mut headers_out = Vec::new();
    let mut chunked = false;
    let mut content_length: Option<usize> = None;
    for l in lines {
        if let Some(p) = l.find(':') {
            let k = l[..p].trim().to_string();
            let v = l[p + 1..].trim().to_string();
            if k.eq_ignore_ascii_case("transfer-encoding") && v.eq_ignore_ascii_case("chunked") {
                chunked = true;
            }
            if k.eq_ignore_ascii_case("content-length") {
                content_length = v.parse().ok();
            }
            headers_out.push((k, v));
        }
    }
    if chunked {
        body_bytes = dechunk(&body_bytes);
    } else if let Some(cl) = content_length {
        body_bytes.truncate(cl);
    }
    Ok(Response { status, headers: headers_out, body: body_bytes })
}

fn dechunk(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < data.len() {
        let line_end = match data[i..].windows(2).position(|w| w == b"\r\n") {
            Some(p) => i + p,
            None => break,
        };
        let size_str = String::from_utf8_lossy(&data[i..line_end]);
        let size = usize::from_str_radix(size_str.trim().split(';').next().unwrap_or("0"), 16)
            .unwrap_or(0);
        if size == 0 {
            break;
        }
        let start = line_end + 2;
        let end = (start + size).min(data.len());
        out.extend_from_slice(&data[start..end]);
        i = end + 2;
    }
    out
}
