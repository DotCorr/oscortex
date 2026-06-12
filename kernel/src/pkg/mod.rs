//! On-demand package delivery — Fuchsia-inspired ephemeral package system.
//!
//! Apps are fetched from a package server over HTTP, verified with SHA-256,
//! cached in an LRU registry, and launched on demand. System apps are pinned
//! (never evicted); user apps are ephemeral (evicted when cache is full).
//!
//! ## Architecture
//!
//! ```text
//! Shell (Flutter) ──→ pkg_resolve syscall ──→ resolver
//!                                              │
//!                          ┌───────────────────┤
//!                          ▼                   ▼
//!                      cache (hit?)        http::get()
//!                          │                   │
//!                          │              sha256::verify
//!                          │                   │
//!                          │             app_registry::install
//!                          │                   │
//!                          └───→ app_id ←──────┘
//!                                  │
//!                          app_registry::launch
//! ```
//!
//! ## Syscalls
//!
//! | Num   | Name            | Args                        | Returns        |
//! |-------|-----------------|-----------------------------|----------------|
//! | 0x390 | pkg_resolve     | (name_ptr, name_len)        | app_id / -ERR  |
//! | 0x391 | pkg_catalog     | (buf_ptr, buf_len)          | count          |
//! | 0x392 | pkg_set_server  | (ip_packed_be, port)        | 0              |
//! | 0x393 | pkg_evict       | (app_id)                    | 0 / -ERRNO     |

pub mod sha256;
pub mod http;
pub mod manifest;
pub mod cache;
pub mod resolver;

/// Initialise the on-demand package subsystem.
///
/// Called during kernel boot, after the network stack is up.
/// Attempts to refresh the catalog from the configured server;
/// continues silently if the server is unreachable (offline mode).
pub fn init() {
    log::info!("[pkg] on-demand package subsystem initialising…");

    // Attempt initial catalog refresh (best-effort).
    match resolver::refresh_catalog() {
        Ok(n) => log::error!("[pkg-test] catalog loaded (SIGNED+verified): {} packages", n),
        Err(e) => log::error!("[pkg-test] catalog refresh failed: {:?}", e),
    }
    match resolver::resolve(b"Demo") {
        Ok(id) => log::error!("[pkg-test] signed resolve OK: Demo -> app_id={}", id),
        Err(e) => log::error!("[pkg-test] signed resolve FAILED: {:?}", e),
    }
}
