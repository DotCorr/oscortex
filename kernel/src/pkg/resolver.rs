//! Package resolver — the on-demand delivery pipeline.
//!
//! ```text
//! resolve(name)
//!   1. Check local catalog → find PkgManifest for name
//!   2. Check cache (by hash) → if hit, touch + return app_id
//!   3. HTTP fetch: GET /pkg/<hex(hash)>.osx
//!   4. SHA-256 verify downloaded bytes
//!   5. Install into app_registry (ephemeral)
//!   6. Insert into cache
//!   7. Return app_id
//! ```

use alloc::vec::Vec;
use spin::Mutex;

use super::cache;
use super::http;
use super::manifest::PkgManifest;
use super::sha256;

/// Resolver errors.
#[derive(Debug)]
pub enum ResolveError {
    /// No catalog loaded — call `refresh_catalog()` first.
    NoCatalog,
    /// Package name not found in catalog.
    NotFound,
    /// HTTP fetch failed.
    FetchFailed,
    /// SHA-256 hash mismatch after download.
    HashMismatch,
    /// Failed to install into app_registry.
    InstallFailed,
    /// Cache is full and all slots are pinned.
    CacheFull,
}

// ── Server configuration ──────────────────────────────────────────────────────

struct ServerConfig {
    /// Big-endian IPv4 address of the package server.
    ip: u32,
    /// TCP port.
    port: u16,
}

/// Default: QEMU user-mode gateway.
static SERVER: Mutex<ServerConfig> = Mutex::new(ServerConfig {
    ip: u32::from_be_bytes([10, 0, 2, 2]),
    port: 8080,
});

/// Set the package server address.
pub fn set_server(ip: u32, port: u16) {
    let mut srv = SERVER.lock();
    srv.ip = ip;
    srv.port = port;
    let bytes = ip.to_be_bytes();
    log::info!(
        "[pkg] server set to {}.{}.{}.{}:{}",
        bytes[0], bytes[1], bytes[2], bytes[3], port
    );
}

// ── Catalog ───────────────────────────────────────────────────────────────────

static CATALOG: Mutex<Vec<PkgManifest>> = Mutex::new(Vec::new());

/// Refresh the catalog from the package server.
pub fn refresh_catalog() -> Result<usize, ResolveError> {
    let (ip, port) = {
        let srv = SERVER.lock();
        (srv.ip, srv.port)
    };

    let body = http::get(ip, port, "/catalog.bin").map_err(|e| {
        log::warn!("[pkg] catalog fetch failed: {:?}", e);
        ResolveError::FetchFailed
    })?;

    let entries = super::manifest::parse_catalog(&body).ok_or_else(|| {
        log::warn!("[pkg] catalog parse failed");
        ResolveError::FetchFailed
    })?;

    let count = entries.len();
    *CATALOG.lock() = entries;
    log::info!("[pkg] catalog refreshed: {} packages available", count);
    Ok(count)
}

/// Get the current catalog (cloned).
pub fn catalog() -> Vec<PkgManifest> {
    CATALOG.lock().clone()
}

/// Find a manifest by name.
fn find_manifest(name: &[u8]) -> Option<PkgManifest> {
    let catalog = CATALOG.lock();
    let name_end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
    let name_str = &name[..name_end];

    catalog.iter().find(|m| {
        let m_name = unsafe { core::ptr::addr_of!(m.name).read_unaligned() };
        let m_end = m_name.iter().position(|&b| b == 0).unwrap_or(64);
        &m_name[..m_end] == name_str
    }).copied()
}

// ── Resolve ───────────────────────────────────────────────────────────────────

/// Resolve a package by name — the main on-demand entry point.
///
/// 1. Look up the manifest in the catalog.
/// 2. Check if already cached (by content hash).
/// 3. If cache miss: fetch → verify → install → cache.
/// 4. Return the `app_id` ready for `app_launch`.
pub fn resolve(name: &[u8]) -> Result<u32, ResolveError> {
    // 1. Find manifest
    let manifest = find_manifest(name).ok_or(ResolveError::NotFound)?;
    let hash = unsafe { core::ptr::addr_of!(manifest.hash).read_unaligned() };

    // 2. Cache hit?
    if let Some(app_id) = cache::lookup(&hash) {
        log::info!("[pkg] cache hit for '{}'", manifest.name_str());
        return Ok(app_id);
    }

    // 3. Fetch from server
    let (ip, port) = {
        let srv = SERVER.lock();
        (srv.ip, srv.port)
    };

    let mut hex_hash = [0u8; 64];
    sha256::hash_hex(&hash, &mut hex_hash);
    let hex_str = core::str::from_utf8(&hex_hash).unwrap_or("bad");
    let path = alloc::format!("/pkg/{}.osx", hex_str);

    let size = unsafe { core::ptr::addr_of!(manifest.size_bytes).read_unaligned() };
    log::info!("[pkg] fetching '{}' ({} bytes)…", manifest.name_str(), size);

    let bundle = http::get(ip, port, &path).map_err(|e| {
        log::warn!("[pkg] fetch failed for '{}': {:?}", manifest.name_str(), e);
        ResolveError::FetchFailed
    })?;

    // 4. Verify SHA-256
    let computed = sha256::hash(&bundle);
    if !sha256::hash_eq(&computed, &hash) {
        log::error!("[pkg] hash mismatch for '{}'!", manifest.name_str());
        return Err(ResolveError::HashMismatch);
    }

    log::info!("[pkg] verified '{}' ({} bytes)", manifest.name_str(), bundle.len());

    // 5. Install into app_registry (ephemeral — not persisted to disk)
    let app_id = crate::app_registry::install(&bundle).ok_or_else(|| {
        log::warn!("[pkg] install failed for '{}'", manifest.name_str());
        ResolveError::InstallFailed
    })?;

    // 6. Insert into cache
    let pinned = manifest.is_system();
    if !cache::insert(app_id, hash, pinned) {
        // Cache full, all pinned — the app is installed but not cache-tracked.
        // It will work but won't be evictable. Log a warning.
        log::warn!("[pkg] cache full — app_id={} installed but not cache-tracked", app_id);
    }

    log::info!("[pkg] resolved '{}' → app_id={}", manifest.name_str(), app_id);
    Ok(app_id)
}

/// Resolve and immediately launch a package.
///
/// Convenience that combines `resolve()` + `app_registry::launch()`.
pub fn resolve_and_launch(name: &[u8]) -> Result<u32, ResolveError> {
    let app_id = resolve(name)?;
    cache::touch(app_id);

    let pid = crate::app_registry::launch(app_id, 0);
    if pid < 0 {
        log::warn!("[pkg] launch failed for app_id={}: errno={}", app_id, pid);
        return Err(ResolveError::InstallFailed);
    }

    Ok(pid as u32)
}
