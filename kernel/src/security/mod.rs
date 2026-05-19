//! Security and capability subsystem.
//!
//! OSCortex uses a capability-based security model instead of classic POSIX UIDs.
//! Every process holds a set of capability tokens. Capabilities are immutable
//! once granted and can only be reduced (dropped), never amplified.
//!
//! ## Built-in capabilities
//!
//!   CAP_CORTEX     — Access PID-0 Cortex API (admin-level, init only)
//!   CAP_DRIVER     — Load/unload kernel drivers
//!   CAP_NET        — Raw network access
//!   CAP_FS         — Raw filesystem access (bypass VFS)
//!   CAP_PROC       — Create/kill processes outside own subtree
//!   CAP_MEM        — Map physical memory into userspace
//!   CAP_IRQ        — Register userspace IRQ handlers
//!   CAP_AUDIT      — Read kernel audit log
//!   CAP_TIME       — Set system time

use bitflags::bitflags;

bitflags! {
    /// Capability bitfield — fits in a u64.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Capabilities: u64 {
        const CORTEX = 1 << 0;
        const DRIVER = 1 << 1;
        const NET    = 1 << 2;
        const FS     = 1 << 3;
        const PROC   = 1 << 4;
        const MEM    = 1 << 5;
        const IRQ    = 1 << 6;
        const AUDIT  = 1 << 7;
        const TIME   = 1 << 8;
    }
}

/// All capabilities — granted only to the initial userspace process (init).
pub const ALL_CAPS: Capabilities = Capabilities::all();
/// No capabilities — default for spawned processes.
pub const NO_CAPS: Capabilities = Capabilities::empty();

/// Verify that `holder` has all the capabilities in `required`.
#[inline]
pub fn check(holder: Capabilities, required: Capabilities) -> bool {
    holder.contains(required)
}

pub fn init() {
    log::info!("[Security] Capability-based security model active");
}
