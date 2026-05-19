//! APIC — x2APIC MSR-based implementation.
//!
//! Uses x2APIC (ECX bit 21 of CPUID leaf 1) so no MMIO mapping is needed.
//! All register access is via RDMSR / WRMSR, available as soon as the CPU is
//! up — no dependency on mm::init().
//!
//! x2APIC register map:  MSR address = 0x800 + (xAPIC_byte_offset / 0x10)
//!   xAPIC 0xB0  → MSR 0x80B  (EOI, write-only)
//!   xAPIC 0xF0  → MSR 0x80F  (Spurious Interrupt Vector)
//!   xAPIC 0x320 → MSR 0x832  (LVT Timer)
//!   xAPIC 0x380 → MSR 0x838  (Initial Count)
//!   xAPIC 0x390 → MSR 0x839  (Current Count, read-only)
//!   xAPIC 0x3E0 → MSR 0x83E  (Divide Configuration)

use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

const IA32_APIC_BASE_MSR: u32 = 0x1B;
const X2APIC_EOI:          u32 = 0x80B;
const X2APIC_SPURIOUS:     u32 = 0x80F;
const X2APIC_TIMER_LVT:    u32 = 0x832;
const X2APIC_TIMER_INIT:   u32 = 0x838;
const X2APIC_TIMER_DIVIDE: u32 = 0x83E;

// xAPIC MMIO register offsets (byte offsets from base)
const XAPIC_OFF_SPURIOUS:  u32 = 0x0F0;
const XAPIC_OFF_EOI:       u32 = 0x0B0;
const XAPIC_OFF_LINT0:     u32 = 0x350;
const XAPIC_OFF_TMR_LVT:   u32 = 0x320;
const XAPIC_OFF_TMR_INIT:  u32 = 0x380;
const XAPIC_OFF_TMR_DIV:   u32 = 0x3E0;

/// True once x2APIC has been successfully initialised on the BSP.
static X2APIC_READY: AtomicBool = AtomicBool::new(false);
/// True once xAPIC (MMIO) has been initialised (fallback when x2APIC absent).
static XAPIC_READY:  AtomicBool = AtomicBool::new(false);
/// Virtual address of xAPIC MMIO region (HHDM-mapped).
static XAPIC_BASE_VA: AtomicU64 = AtomicU64::new(0);

// ── Vsync timing ─────────────────────────────────────────────────────────────

/// Desired vsync rate in Hz (1–240).  Default 60.
static VSYNC_HZ: AtomicU32 = AtomicU32::new(60);

/// Assumed TSC frequency in Hz.  Default 2 GHz (conservative; works on any
/// modern x86_64).  Updated by `set_tsc_hz` if the embedder calibrates it.
static TSC_HZ: AtomicU64 = AtomicU64::new(2_000_000_000);

/// TSC ticks between vsync fires.  Re-computed whenever Hz or TSC freq changes.
static VSYNC_TSC_PERIOD: AtomicU64 = AtomicU64::new(2_000_000_000 / 60);

/// TSC value at the last vsync fire.
static VSYNC_LAST_TSC: AtomicU64 = AtomicU64::new(0);

fn compute_period(hz: u32, tsc_hz: u64) -> u64 {
    if hz == 0 { u64::MAX } else { tsc_hz / hz as u64 }
}

/// Set the desired vsync rate (1–240 Hz).  Called from `SYS_VSYNC_SET_HZ`.
pub fn set_vsync_hz(hz: u32) {
    let hz = hz.clamp(1, 240);
    VSYNC_HZ.store(hz, Ordering::Release);
    let period = compute_period(hz, TSC_HZ.load(Ordering::Relaxed));
    VSYNC_TSC_PERIOD.store(period, Ordering::Release);
}

/// Update the assumed TSC frequency so vsync periods are accurate.
/// Call this if you have a measured TSC Hz value (e.g. from PIT calibration).
pub fn set_tsc_hz(tsc_hz: u64) {
    TSC_HZ.store(tsc_hz, Ordering::Release);
    let hz = VSYNC_HZ.load(Ordering::Relaxed);
    let period = compute_period(hz, tsc_hz);
    VSYNC_TSC_PERIOD.store(period, Ordering::Release);
}

/// Returns `true` and updates the last-vsync timestamp if enough TSC time has
/// elapsed for the next vsync.  Designed to be called from the APIC timer ISR.
#[inline]
pub fn vsync_due() -> bool {
    let period = VSYNC_TSC_PERIOD.load(Ordering::Relaxed);
    if period == u64::MAX { return false; }
    let now  = crate::arch::rdtsc();
    let last = VSYNC_LAST_TSC.load(Ordering::Relaxed);
    if now.wrapping_sub(last) >= period {
        VSYNC_LAST_TSC.store(now, Ordering::Relaxed);
        true
    } else {
        false
    }
}

/// Return the current vsync Hz.
pub fn vsync_hz() -> u32 {
    VSYNC_HZ.load(Ordering::Relaxed)
}

// ── MSR helpers ───────────────────────────────────────────────────────────────

#[inline]
fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi, options(nomem, nostack));
    }
    ((hi as u64) << 32) | lo as u64
}

#[inline]
fn wrmsr(msr: u32, val: u64) {
    unsafe {
        asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") val as u32,
            in("edx") (val >> 32) as u32,
            options(nomem, nostack),
        );
    }
}

// ── Feature detection ─────────────────────────────────────────────────────────

/// Returns true if the CPU advertises x2APIC support (CPUID.01H:ECX[21]).
fn has_x2apic() -> bool {
    let (_, _, ecx, _) = crate::arch::x86_64::cpu::cpuid(1, 0);
    ecx & (1 << 21) != 0
}

// ── Public API ────────────────────────────────────────────────────────────────

// ── xAPIC MMIO helpers ───────────────────────────────────────────────────────

#[inline]
unsafe fn xapic_write(offset: u32, val: u32) {
    let base = XAPIC_BASE_VA.load(Ordering::Relaxed);
    let ptr = (base + offset as u64) as *mut u32;
    core::ptr::write_volatile(ptr, val);
}

#[inline]
unsafe fn xapic_read(offset: u32) -> u32 {
    let base = XAPIC_BASE_VA.load(Ordering::Relaxed);
    let ptr = (base + offset as u64) as *const u32;
    core::ptr::read_volatile(ptr)
}

// ── Init ─────────────────────────────────────────────────────────────────────

/// Initialise the local APIC on the Boot Strap Processor.
///
/// Prefers x2APIC (MSR-based, no MMIO mapping needed).  Falls back to xAPIC
/// (MMIO) on CPUs/VMs that don't advertise x2APIC — this includes the default
/// QEMU `qemu64` CPU model.
pub fn init_bsp() {
    if has_x2apic() {
        // ── x2APIC path ──────────────────────────────────────────────────
        let base = rdmsr(IA32_APIC_BASE_MSR);
        wrmsr(IA32_APIC_BASE_MSR, base | (1 << 11) | (1 << 10));
        let sv = rdmsr(X2APIC_SPURIOUS) as u32;
        wrmsr(X2APIC_SPURIOUS, u64::from(sv | 0x100 | 0xFF));
        wrmsr(X2APIC_TIMER_DIVIDE, 0x3);
        wrmsr(X2APIC_TIMER_LVT, 0x0002_0030);
        wrmsr(X2APIC_TIMER_INIT, 0x0010_0000);
        X2APIC_READY.store(true, Ordering::Release);
        crate::logger::early_print("[APIC] x2APIC online — timer armed (vec 0x30)\r\n");
    } else {
        // ── xAPIC MMIO fallback (e.g. QEMU qemu64 default CPU) ───────────
        // Phase 1 (early): record phys base; do NOT access MMIO yet.
        // MMIO mapping requires the frame allocator which isn't up yet.
        // Call apic::finish_xapic_init() after mm::init() to complete setup.
        let base_msr  = rdmsr(IA32_APIC_BASE_MSR);
        let phys_base = base_msr & 0xFFFF_FFFF_FFFF_F000;
        let hhdm      = crate::mm::frame_allocator::hhdm_offset();
        let virt_base = phys_base + hhdm;
        XAPIC_BASE_VA.store(virt_base, Ordering::Release);
        crate::logger::early_print("[APIC] xAPIC detected — deferred MMIO init pending mm::init()\r\n");
    }
}

/// Phase-2 xAPIC init: map the MMIO region and arm the timer.
/// Must be called after mm::init() (needs the frame allocator for page mapping).
/// No-op if x2APIC was already initialised or if xAPIC is not pending.
pub fn finish_xapic_init() {
    if X2APIC_READY.load(Ordering::Acquire) { return; }
    if XAPIC_READY.load(Ordering::Acquire)  { return; }
    let virt_base = XAPIC_BASE_VA.load(Ordering::Acquire);
    if virt_base == 0 { return; }
    let hhdm      = crate::mm::frame_allocator::hhdm_offset();
    let phys_base = virt_base - hhdm;

    // Map 4 KiB MMIO (one page covers all xAPIC registers up to 0x3FF).
    unsafe { crate::mm::paging::map_mmio(phys_base, virt_base, 0x1000); }

    unsafe {
        // Ensure xAPIC enabled (bit 11) in base MSR.
        let base_msr = rdmsr(IA32_APIC_BASE_MSR);
        wrmsr(IA32_APIC_BASE_MSR, base_msr | (1 << 11));
        // Software-enable + spurious vector 0xFF.
        let sv = xapic_read(XAPIC_OFF_SPURIOUS);
        xapic_write(XAPIC_OFF_SPURIOUS, sv | 0x100 | 0xFF);
        // Timer: ÷16, periodic (bit 17), vector 0x30.
        xapic_write(XAPIC_OFF_TMR_DIV,  0x3);
        xapic_write(XAPIC_OFF_TMR_LVT,  0x0002_0030);
        xapic_write(XAPIC_OFF_TMR_INIT, 0x0010_0000);
    }
    XAPIC_READY.store(true, Ordering::Release);
    crate::logger::early_print("[APIC] xAPIC (MMIO) online — timer armed (vec 0x30)\r\n");
}

/// Initialise the local APIC on an Application Processor.
pub fn init_ap() {
    if has_x2apic() {
        let base = rdmsr(IA32_APIC_BASE_MSR);
        wrmsr(IA32_APIC_BASE_MSR, base | (1 << 11) | (1 << 10));
        let sv = rdmsr(X2APIC_SPURIOUS) as u32;
        wrmsr(X2APIC_SPURIOUS, u64::from(sv | 0x100 | 0xFF));
        wrmsr(X2APIC_TIMER_DIVIDE, 0x3);
        wrmsr(X2APIC_TIMER_LVT, 0x0002_0030);
        wrmsr(X2APIC_TIMER_INIT, 0x0010_0000);
    } else {
        // xAPIC path — reuse the MMIO base set up by the BSP.
        unsafe {
            let sv = xapic_read(XAPIC_OFF_SPURIOUS);
            xapic_write(XAPIC_OFF_SPURIOUS, sv | 0x100 | 0xFF);
            xapic_write(XAPIC_OFF_TMR_DIV,  0x3);
            xapic_write(XAPIC_OFF_TMR_LVT,  0x0002_0030);
            xapic_write(XAPIC_OFF_TMR_INIT, 0x0010_0000);
        }
    }
}

/// Send End-Of-Interrupt signal to the local APIC.
///
/// Works in both x2APIC and xAPIC modes.
#[inline]
pub fn eoi() {
    if X2APIC_READY.load(Ordering::Acquire) {
        wrmsr(X2APIC_EOI, 0);
    } else if XAPIC_READY.load(Ordering::Acquire) {
        unsafe { xapic_write(XAPIC_OFF_EOI, 0); }
    }
}

/// Configure LINT0 for ExtINT delivery so legacy 8259 PIC IRQs flow through
/// the APIC to the CPU.  Safe to call regardless of x2APIC/xAPIC mode; if
/// neither APIC is active the function is a no-op (PIC uses the INTR pin
/// directly in that case).
pub fn configure_lint0_for_extint() {
    // Delivery mode 7 (ExtINT), edge-triggered, unmasked = 0x700
    if X2APIC_READY.load(Ordering::Acquire) {
        wrmsr(0x835, 0x700); // x2APIC LINT0 MSR
    } else if XAPIC_READY.load(Ordering::Acquire) {
        unsafe { xapic_write(XAPIC_OFF_LINT0, 0x700); }
    }
    // else: APIC not active — legacy INTR wiring handles PIC delivery
}

/// Return the local APIC ID of the current CPU.
///
/// Handles both x2APIC (MSR 0x802) and xAPIC (MMIO register 0x20, bits[31:24]).
#[inline]
pub fn local_apic_id() -> u32 {
    if X2APIC_READY.load(Ordering::Acquire) {
        // x2APIC mode: ID is in MSR 0x802.
        let id: u64;
        unsafe {
            asm!("rdmsr", in("ecx") 0x802u32, out("eax") id, out("edx") _, options(nomem, nostack));
        }
        id as u32
    } else {
        // xAPIC MMIO mode: ID is at APIC_MMIO + 0x20, bits [31:24].
        let base = XAPIC_BASE_VA.load(Ordering::Relaxed);
        if base == 0 { return 0; }
        let reg = unsafe { core::ptr::read_volatile((base + 0x20) as *const u32) };
        (reg >> 24) & 0xFF
    }
}

