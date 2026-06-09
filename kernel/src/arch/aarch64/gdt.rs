//! Segment-selector compatibility shim — aarch64.
//!
//! AArch64 has no GDT/segmentation. The shared kernel still references the
//! `USER_CS` / `USER_DS` selector constants when building an initial user
//! context (the values are stored into the saved register frame and ignored by
//! the aarch64 exception-return path, which uses SPSR_EL1 instead). We keep the
//! symbols so cross-arch process-setup code compiles; the values are nominal.

/// User code "selector" placeholder (no segmentation on aarch64).
pub const USER_CS: u16 = 0;
/// User data "selector" placeholder (no segmentation on aarch64).
pub const USER_DS: u16 = 0;

/// Load the (nonexistent) descriptor table on the BSP. No-op on aarch64.
pub fn init() {}

/// Per-AP descriptor-table load. No-op on aarch64.
pub fn init_ap(_cpu_idx: u32) {}
