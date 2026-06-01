//! Minimal HTTP/1.1 GET client over the kernel's smoltcp TCP stack.
//!
//! Used exclusively for on-demand package fetches from the package server.
//! No TLS (LAN-only for now), no keep-alive, no redirects.
//!
//! ```rust
//! let body = http::get(ip_be, 8080, "/pkg/abc123.osx")?;
//! ```

use alloc::vec::Vec;

/// HTTP fetch error.
#[derive(Debug)]
pub enum HttpError {
    /// TCP connection failed.
    Connect,
    /// Failed to send request.
    Send,
    /// Failed to read response.
    Recv,
    /// HTTP status code was not 200.
    Status(u16),
    /// Response was malformed.
    Malformed,
    /// Response body exceeded maximum size.
    TooLarge,
}

/// Maximum response body size (8 MiB — larger bundles need streaming in future).
const MAX_BODY_SIZE: usize = 8 * 1024 * 1024;

/// Perform an HTTP/1.1 GET request and return the response body.
///
/// `host_ip` is a big-endian packed IPv4 address (e.g. `0x0A000202` for 10.0.2.2).
/// `port` is the TCP port (e.g. 8080).
/// `path` is the request path (e.g. "/catalog.bin" or "/pkg/abc.osx").
pub fn get(host_ip: u32, port: u16, path: &str) -> Result<Vec<u8>, HttpError> {
    // 1. Connect
    let fd = crate::net::tcp::tcp_connect(host_ip, port).map_err(|_| HttpError::Connect)?;

    // Busy-poll until connection is established (max ~2 seconds).
    for _ in 0..120 {
        crate::net::tcp::poll();
        // Try a zero-byte read to check if connected.
        let mut probe = [0u8; 0];
        match crate::net::tcp::tcp_read(fd, &mut probe) {
            Ok(_) => break,
            Err(-11) => {
                // EAGAIN — still connecting, wait a tick.
                for _ in 0..50_000u64 { core::hint::spin_loop(); }
            }
            Err(_) => break, // connected or error — proceed
        }
    }

    // 2. Build and send GET request
    let ip_bytes = host_ip.to_be_bytes();
    let host_str = format_ip(ip_bytes, port);
    let request = build_request(path, &host_str);
    let req_bytes = request.as_bytes();

    let mut sent = 0usize;
    while sent < req_bytes.len() {
        crate::net::tcp::poll();
        match crate::net::tcp::tcp_write(fd, &req_bytes[sent..]) {
            Ok(n) => sent += n,
            Err(-11) => {
                for _ in 0..10_000u64 { core::hint::spin_loop(); }
            }
            Err(_) => {
                let _ = crate::net::tcp::tcp_close(fd);
                return Err(HttpError::Send);
            }
        }
    }

    // 3. Read response (headers + body)
    let mut response = Vec::with_capacity(4096);
    let mut consecutive_eagain = 0u32;

    loop {
        crate::net::tcp::poll();
        let mut chunk = [0u8; 4096];
        match crate::net::tcp::tcp_read(fd, &mut chunk) {
            Ok(0) => break, // EOF
            Ok(n) => {
                consecutive_eagain = 0;
                response.extend_from_slice(&chunk[..n]);
                if response.len() > MAX_BODY_SIZE + 4096 {
                    let _ = crate::net::tcp::tcp_close(fd);
                    return Err(HttpError::TooLarge);
                }
            }
            Err(-11) => {
                // EAGAIN
                consecutive_eagain += 1;
                if consecutive_eagain > 300 {
                    // ~5 seconds of no data — assume done
                    break;
                }
                for _ in 0..10_000u64 { core::hint::spin_loop(); }
            }
            Err(_) => break, // Connection closed or error
        }
    }

    let _ = crate::net::tcp::tcp_close(fd);

    // 4. Parse HTTP response
    parse_response(&response)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn build_request(path: &str, host: &str) -> alloc::string::String {
    use alloc::format;
    format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nUser-Agent: OSCortex/1.0\r\n\r\n",
        path, host
    )
}

fn format_ip(bytes: [u8; 4], port: u16) -> alloc::string::String {
    use alloc::format;
    format!("{}.{}.{}.{}:{}", bytes[0], bytes[1], bytes[2], bytes[3], port)
}

fn parse_response(data: &[u8]) -> Result<Vec<u8>, HttpError> {
    // Find the end of headers: \r\n\r\n
    let header_end = find_header_end(data).ok_or(HttpError::Malformed)?;
    let headers = core::str::from_utf8(&data[..header_end]).map_err(|_| HttpError::Malformed)?;

    // Parse status line: "HTTP/1.1 200 OK"
    let status_line = headers.lines().next().ok_or(HttpError::Malformed)?;
    let status_code = parse_status_code(status_line).ok_or(HttpError::Malformed)?;
    if status_code != 200 {
        return Err(HttpError::Status(status_code));
    }

    // Body starts after \r\n\r\n
    let body_start = header_end + 4;
    if body_start >= data.len() {
        return Ok(Vec::new());
    }

    let body = &data[body_start..];

    // Check Content-Length if present
    if let Some(content_length) = parse_content_length(headers) {
        if content_length > MAX_BODY_SIZE {
            return Err(HttpError::TooLarge);
        }
        let actual = body.len().min(content_length);
        return Ok(body[..actual].to_vec());
    }

    // No Content-Length — return whatever body we got
    Ok(body.to_vec())
}

fn find_header_end(data: &[u8]) -> Option<usize> {
    for i in 0..data.len().saturating_sub(3) {
        if &data[i..i + 4] == b"\r\n\r\n" {
            return Some(i);
        }
    }
    None
}

fn parse_status_code(line: &str) -> Option<u16> {
    // "HTTP/1.1 200 OK" → split by space, take index 1
    let mut parts = line.split_whitespace();
    let _version = parts.next()?;
    let code_str = parts.next()?;
    code_str.parse::<u16>().ok()
}

fn parse_content_length(headers: &str) -> Option<usize> {
    for line in headers.lines() {
        // Case-insensitive check for "content-length:"
        if line.len() >= 15 {
            let prefix = &line[..15];
            if prefix.eq_ignore_ascii_case("content-length:") {
                let val = line[15..].trim();
                return val.parse::<usize>().ok();
            }
        }
    }
    None
}
