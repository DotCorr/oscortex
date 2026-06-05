//! pkg-server — serves .osx application bundles over HTTP.
//!
//! Usage: pkg-server [BUNDLE_DIR]
//!
//! Scans BUNDLE_DIR (default: cwd) for *.osx files, parses their headers,
//! computes SHA-256 hashes, builds a catalog.bin, and serves everything on
//! 0.0.0.0:8080.

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

// ---------------------------------------------------------------------------
// PkgManifest — 128-byte catalog entry (repr C, packed)
// ---------------------------------------------------------------------------

const MANIFEST_SIZE: usize = 128;
const OSX_MAGIC: &[u8; 4] = b"OSCP";

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct PkgManifest {
    name: [u8; 64],
    version: [u8; 16],
    hash: [u8; 32],
    size_bytes: u32,
    flags: u32,
    _reserved: [u8; 8],
}

impl PkgManifest {
    fn as_bytes(&self) -> &[u8] {
        // SAFETY: repr(C, packed), all fields are integral / byte arrays.
        unsafe {
            std::slice::from_raw_parts(self as *const Self as *const u8, MANIFEST_SIZE)
        }
    }

    fn hash_hex(&self) -> String {
        self.hash.iter().map(|b| format!("{b:02x}")).collect()
    }
}

// ---------------------------------------------------------------------------
// Shared server state
// ---------------------------------------------------------------------------

struct ServerState {
    catalog_bin: Vec<u8>,
    /// Map from lowercase hex SHA-256 → full .osx file bytes
    packages: HashMap<String, Vec<u8>>,
}

// ---------------------------------------------------------------------------
// .osx parsing + catalog building
// ---------------------------------------------------------------------------

fn parse_osx(path: &Path) -> Result<(PkgManifest, Vec<u8>), String> {
    let data = fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;

    if data.len() < 88 {
        return Err(format!("{}: too small ({} bytes)", path.display(), data.len()));
    }
    if &data[0..4] != OSX_MAGIC {
        return Err(format!("{}: bad magic", path.display()));
    }
    let fmt_ver = u32::from_le_bytes(data[4..8].try_into().unwrap());
    if fmt_ver != 1 {
        return Err(format!("{}: unsupported format version {fmt_ver}", path.display()));
    }

    let mut name = [0u8; 64];
    name.copy_from_slice(&data[8..72]);

    let mut version = [0u8; 16];
    version.copy_from_slice(&data[72..88]);

    let mut hasher = Sha256::new();
    hasher.update(&data);
    let digest = hasher.finalize();

    let mut hash = [0u8; 32];
    hash.copy_from_slice(&digest);

    let manifest = PkgManifest {
        name,
        version,
        hash,
        size_bytes: data.len() as u32,
        flags: 0,
        _reserved: [0u8; 8],
    };

    Ok((manifest, data))
}

fn build_catalog(dir: &Path) -> ServerState {
    let mut manifests: Vec<PkgManifest> = Vec::new();
    let mut packages: HashMap<String, Vec<u8>> = HashMap::new();

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("[pkg-server] cannot read directory {}: {e}", dir.display());
            return ServerState {
                catalog_bin: build_catalog_bin(&[]),
                packages,
            };
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        match path.extension().and_then(|s| s.to_str()) {
            Some("osx") => {}
            _ => continue,
        }

        match parse_osx(&path) {
            Ok((manifest, data)) => {
                let hex = manifest.hash_hex();
                let name_str = String::from_utf8_lossy(&manifest.name);
                let name_trimmed = name_str.trim_end_matches('\0');
                println!(
                    "[pkg-server] indexed {} — {} bytes — hash {}",
                    name_trimmed,
                    data.len(),
                    &hex[..16],
                );
                packages.insert(hex, data);
                manifests.push(manifest);
            }
            Err(e) => {
                eprintln!("[pkg-server] skip: {e}");
            }
        }
    }

    println!("[pkg-server] catalog: {} package(s)", manifests.len());

    ServerState {
        catalog_bin: build_catalog_bin(&manifests),
        packages,
    }
}

fn build_catalog_bin(manifests: &[PkgManifest]) -> Vec<u8> {
    let entry_count = manifests.len() as u32;
    let mut buf = Vec::with_capacity(4 + manifests.len() * MANIFEST_SIZE);
    buf.extend_from_slice(&entry_count.to_le_bytes());
    for m in manifests {
        buf.extend_from_slice(m.as_bytes());
    }
    buf
}

// ---------------------------------------------------------------------------
// HTTP server (minimal HTTP/1.1 over raw TCP)
// ---------------------------------------------------------------------------

fn handle_connection(stream: &mut TcpStream, state: &ServerState) {
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "?".into());

    // Read request (up to 4 KiB is plenty for a simple GET)
    let mut buf = [0u8; 4096];
    let n = match stream.read(&mut buf) {
        Ok(n) if n > 0 => n,
        _ => return,
    };

    let request = String::from_utf8_lossy(&buf[..n]);
    let first_line = request.lines().next().unwrap_or("");
    let parts: Vec<&str> = first_line.split_whitespace().collect();

    if parts.len() < 2 {
        let _ = send_response(stream, 400, "text/plain", b"Bad Request");
        return;
    }

    let method = parts[0];
    let path = parts[1];

    let start = Instant::now();

    if method != "GET" {
        println!("[{peer}] {method} {path} → 405 ({:.1}ms)", start.elapsed().as_secs_f64() * 1000.0);
        let _ = send_response(stream, 405, "text/plain", b"Method Not Allowed");
        return;
    }

    match path {
        "/catalog.bin" => {
            let _ = send_response(stream, 200, "application/octet-stream", &state.catalog_bin);
            println!(
                "[{peer}] GET /catalog.bin → 200 ({} bytes, {:.1}ms)",
                state.catalog_bin.len(),
                start.elapsed().as_secs_f64() * 1000.0,
            );
        }
        p if p.starts_with("/pkg/") && p.ends_with(".osx") => {
            let hex = &p[5..p.len() - 4]; // strip "/pkg/" and ".osx"
            if let Some(data) = state.packages.get(hex) {
                let _ = send_response(stream, 200, "application/octet-stream", data);
                println!(
                    "[{peer}] GET {p} → 200 ({} bytes, {:.1}ms)",
                    data.len(),
                    start.elapsed().as_secs_f64() * 1000.0,
                );
            } else {
                let _ = send_response(stream, 404, "text/plain", b"Package Not Found");
                println!(
                    "[{peer}] GET {p} → 404 ({:.1}ms)",
                    start.elapsed().as_secs_f64() * 1000.0,
                );
            }
        }
        _ => {
            let _ = send_response(stream, 404, "text/plain", b"Not Found");
            println!(
                "[{peer}] GET {path} → 404 ({:.1}ms)",
                start.elapsed().as_secs_f64() * 1000.0,
            );
        }
    }
}

fn send_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Unknown",
    };

    let header = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n",
        body.len(),
    );

    stream.write_all(header.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    let bundle_dir: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("cannot determine cwd"));

    if !bundle_dir.is_dir() {
        eprintln!("[pkg-server] error: {} is not a directory", bundle_dir.display());
        std::process::exit(1);
    }

    println!("[pkg-server] scanning {} for .osx bundles…", bundle_dir.display());
    let state = Arc::new(build_catalog(&bundle_dir));

    let bind = "0.0.0.0:8080";
    let listener = TcpListener::bind(bind).unwrap_or_else(|e| {
        eprintln!("[pkg-server] failed to bind {bind}: {e}");
        std::process::exit(1);
    });

    println!("[pkg-server] listening on http://{bind}");
    println!("[pkg-server]   GET /catalog.bin");
    println!("[pkg-server]   GET /pkg/<sha256>.osx");

    for stream in listener.incoming() {
        match stream {
            Ok(mut s) => {
                let st = Arc::clone(&state);
                // Simple per-connection thread. Fine for a dev/LAN tool.
                std::thread::spawn(move || {
                    handle_connection(&mut s, &st);
                });
            }
            Err(e) => {
                eprintln!("[pkg-server] accept error: {e}");
            }
        }
    }
}
