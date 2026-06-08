//! GICv2 interrupt controller — QEMU `-M virt` (distributor + CPU interface).
//!
//! `-M virt` defaults to GICv2 with:
//!   * GICD (distributor)   @ 0x0800_0000
//!   * GICC (CPU interface) @ 0x0801_0000
//!
//! We enable the distributor + this core's CPU interface, set a permissive
//! priority mask, and provide enable/EOI for individual interrupt IDs. The ARM
//! generic timer's EL1 physical-timer interrupt is PPI 30 on `-M virt`.

const GICD_BASE: u64 = 0x0800_0000;
const GICC_BASE: u64 = 0x0801_0000;

// Distributor registers (offsets from GICD_BASE).
const GICD_CTLR: u64 = 0x000; // Distributor control
const GICD_ISENABLER: u64 = 0x100; // Set-enable (1 bit/IRQ, 32/reg)
const GICD_ICENABLER: u64 = 0x180; // Clear-enable
const GICD_IPRIORITYR: u64 = 0x400; // Priority (1 byte/IRQ)
const GICD_ITARGETSR: u64 = 0x800; // CPU targets (1 byte/IRQ, SPIs only)
const GICD_ICFGR: u64 = 0xC00; // Config (2 bits/IRQ: edge/level)

// CPU interface registers (offsets from GICC_BASE).
const GICC_CTLR: u64 = 0x00; // CPU interface control
const GICC_PMR: u64 = 0x04; // Priority mask
const GICC_BPR: u64 = 0x08; // Binary point
const GICC_IAR: u64 = 0x0C; // Interrupt acknowledge
const GICC_EOIR: u64 = 0x10; // End of interrupt

#[inline(always)]
unsafe fn gicd_read(off: u64) -> u32 {
    core::ptr::read_volatile((GICD_BASE + off) as *const u32)
}
#[inline(always)]
unsafe fn gicd_write(off: u64, v: u32) {
    core::ptr::write_volatile((GICD_BASE + off) as *mut u32, v);
}
#[inline(always)]
unsafe fn gicc_read(off: u64) -> u32 {
    core::ptr::read_volatile((GICC_BASE + off) as *const u32)
}
#[inline(always)]
unsafe fn gicc_write(off: u64, v: u32) {
    core::ptr::write_volatile((GICC_BASE + off) as *mut u32, v);
}

/// Initialise the distributor (system-wide, call once on the BSP).
pub fn init_distributor() {
    unsafe {
        // Disable the distributor while configuring.
        gicd_write(GICD_CTLR, 0);
        // Leave SGIs/PPIs (0..31) at reset; they're per-CPU and routed locally.
        // Enable the distributor (Group 0 forwarding).
        gicd_write(GICD_CTLR, 1);
    }
}

/// Initialise this core's CPU interface (call on every core).
pub fn init_cpu_interface() {
    unsafe {
        // Lowest priority threshold = 0xFF lets all priorities through.
        gicc_write(GICC_PMR, 0xFF);
        // No sub-priority grouping.
        gicc_write(GICC_BPR, 0);
        // Enable signalling of Group 0 interrupts to this CPU.
        gicc_write(GICC_CTLR, 1);
    }
}

/// Enable a specific interrupt ID at the distributor.
pub fn enable_irq(intid: u32) {
    let reg = (intid / 32) as u64;
    let bit = intid % 32;
    unsafe {
        gicd_write(GICD_ISENABLER + reg * 4, 1 << bit);
    }
}

/// Disable a specific interrupt ID.
pub fn disable_irq(intid: u32) {
    let reg = (intid / 32) as u64;
    let bit = intid % 32;
    unsafe {
        gicd_write(GICD_ICENABLER + reg * 4, 1 << bit);
    }
}

/// Set an interrupt's priority (0 = highest). Lower numbers preempt higher.
pub fn set_priority(intid: u32, prio: u8) {
    unsafe {
        let addr = GICD_BASE + GICD_IPRIORITYR + intid as u64;
        core::ptr::write_volatile(addr as *mut u8, prio);
    }
}

/// Route an SPI (intid >= 32) to CPU 0. PPIs/SGIs are implicitly local.
pub fn route_to_cpu0(intid: u32) {
    if intid >= 32 {
        unsafe {
            let addr = GICD_BASE + GICD_ITARGETSR + intid as u64;
            core::ptr::write_volatile(addr as *mut u8, 0x01);
        }
    }
}

/// Acknowledge the pending interrupt; returns its INTID (1023 = spurious).
pub fn acknowledge() -> u32 {
    unsafe { gicc_read(GICC_IAR) & 0x3FF }
}

/// Signal end-of-interrupt for `intid`.
pub fn eoi(intid: u32) {
    unsafe { gicc_write(GICC_EOIR, intid) };
}
