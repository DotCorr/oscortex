//! Global Descriptor Table — x86_64.
//!
//! We set up a minimal GDT:
//!   0x00 — null
//!   0x08 — kernel code (ring 0, 64-bit)
//!   0x10 — kernel data (ring 0)
//!   0x18 — user code  (ring 3, 64-bit) [for future userspace]
//!   0x20 — user data  (ring 3)
//!   0x28 — TSS descriptor (16 bytes, takes two slots)

use core::arch::asm;
use spin::Once;
use super::smp::MAX_CPUS;

pub const KERNEL_CS: u16 = 0x08;
pub const KERNEL_DS: u16 = 0x10;
/// Placeholder selector at 0x18 required by SYSRET64 GDT layout.
pub const USER_CS_BASE: u16 = 0x18;
pub const USER_CS: u16 = 0x2B;   // 0x28 | RPL 3  — SYSRET64: CS = STAR[63:48]+16
pub const USER_DS: u16 = 0x23;   // 0x20 | RPL 3  — SYSRET64: SS = STAR[63:48]+8
pub const TSS_SELECTOR: u16 = 0x30;

// ── TSS ──────────────────────────────────────────────────────────────────────

/// Task State Segment. We use IST entries for double-fault / NMI stacks.
#[repr(C, packed)]
struct Tss {
    _reserved0: u32,
    rsp: [u64; 3],          // RSP0/1/2 — RSP0 is the kernel stack on interrupt
    _reserved1: u64,
    ist: [u64; 7],          // IST1 = double-fault stack
    _reserved2: u64,
    _reserved3: u16,
    iomap_base: u16,
}

#[repr(C, align(16))]
struct InterruptStack([u8; 16 * 1024]);

static mut DOUBLE_FAULT_STACK: [u8; 16 * 1024] = [0; 16 * 1024];
static mut INTERRUPT_STACKS: [InterruptStack; MAX_CPUS] = [const { InterruptStack([0; 16 * 1024]) }; MAX_CPUS];
static mut INTERRUPT_STACK_TOPS: [u64; MAX_CPUS] = [0; MAX_CPUS];

static mut TSS_ARR: [Tss; MAX_CPUS] = [const { Tss {
    _reserved0: 0,
    rsp: [0; 3],
    _reserved1: 0,
    ist: [0; 7],
    _reserved2: 0,
    _reserved3: 0,
    iomap_base: 104,
} }; MAX_CPUS];

// ── GDT ──────────────────────────────────────────────────────────────────────

#[repr(C, packed)]
struct GdtEntry(u64);

impl GdtEntry {
    const NULL: Self = Self(0);

    /// 64-bit code segment descriptor.
    const fn code64(dpl: u8) -> Self {
        let dpl = (dpl as u64 & 3) << 45;
        // Present | DPL | code | execute/read | long mode
        Self(0x00AF_9A00_0000_FFFF | dpl)
    }

    /// Data segment descriptor (works for 64-bit too).
    const fn data(dpl: u8) -> Self {
        let dpl = (dpl as u64 & 3) << 45;
        Self(0x00CF_9200_0000_FFFF | dpl)
    }

    /// Low u64 of a TSS descriptor.
    fn tss_low(tss: *const Tss) -> Self {
        let base = tss as u64;
        let limit = (core::mem::size_of::<Tss>() - 1) as u64;
        // Type=9 (available 64-bit TSS), Present
        let lo = (limit & 0xFFFF)
            | ((base & 0xFF_FFFF) << 16)
            | (0x89u64 << 40)
            | ((limit >> 16) << 48)
            | ((base >> 24) & 0xFF) << 56;
        Self(lo)
    }

    /// High u64 of a TSS descriptor (upper 32 bits of base).
    fn tss_high(tss: *const Tss) -> Self {
        let base = tss as u64;
        Self(base >> 32)
    }
}

#[repr(C, packed)]
struct GdtPointer {
    limit: u16,
    base: u64,
}

/// 8 entries: null + kernel_cs + kernel_ds + user_cs32_placeholder + user_ds + user_cs64 + tss_lo + tss_hi
/// SYSRET64 layout: STAR[63:48]=0x18 → SS=0x20|3, CS=0x28|3
#[repr(C, align(16))]
struct Gdt {
    entries: [GdtEntry; 8],
}

static mut GDTS: [Gdt; MAX_CPUS] = [const { Gdt { entries: [GdtEntry::NULL; 8] } }; MAX_CPUS];

pub fn init() {
    unsafe {
        let df_stack_top =
            DOUBLE_FAULT_STACK.as_ptr().add(DOUBLE_FAULT_STACK.len()) as u64;

        for i in 0..MAX_CPUS {
            let top = INTERRUPT_STACKS[i].0.as_ptr().add(16 * 1024) as u64;
            INTERRUPT_STACK_TOPS[i] = top;

            TSS_ARR[i].rsp[0] = top;
            TSS_ARR[i].ist[0] = df_stack_top; // IST1

            let t = &TSS_ARR[i] as *const Tss;
            GDTS[i].entries[0] = GdtEntry::NULL;
            GDTS[i].entries[1] = GdtEntry::code64(0);  // kernel CS 0x08
            GDTS[i].entries[2] = GdtEntry::data(0);    // kernel DS 0x10
            GDTS[i].entries[3] = GdtEntry::code64(3);  // user CS32 placeholder 0x18 (SYSRET base)
            GDTS[i].entries[4] = GdtEntry::data(3);    // user DS   0x20 (SYSRET SS = base+8)
            GDTS[i].entries[5] = GdtEntry::code64(3);  // user CS64 0x28 (SYSRET CS = base+16)
            GDTS[i].entries[6] = GdtEntry::tss_low(t); // TSS lo    0x30
            GDTS[i].entries[7] = GdtEntry::tss_high(t);// TSS hi    0x38
        }

        let ptr = GdtPointer {
            limit: (core::mem::size_of::<Gdt>() - 1) as u16,
            base: (&GDTS[0] as *const Gdt) as u64,
        };

        asm!(
            "lgdt [{ptr}]",
            "mov ax, {ds}",
            "mov ds, ax",
            "mov es, ax",
            "mov ss, ax",
            "push {cs}",
            "lea rax, [rip + 2f]",
            "push rax",
            "retfq",
            "2:",
            ptr = in(reg) &ptr,
            cs = const KERNEL_CS as u64,
            ds = const KERNEL_DS,
            out("rax") _,
        );

        // Load TSS.
        asm!("ltr ax", in("ax") TSS_SELECTOR);
    }
}

/// Reload GDT on an AP (uses already-initialised static GDT).
pub fn init_ap(cpu_idx: u32) {
    unsafe {
        let ptr = GdtPointer {
            limit: (core::mem::size_of::<Gdt>() - 1) as u16,
            base: (&GDTS[cpu_idx as usize] as *const Gdt) as u64,
        };
        asm!(
            "lgdt [{ptr}]",
            "mov ax, {ds}",
            "mov ds, ax",
            "mov es, ax",
            "mov ss, ax",
            "push {cs}",
            "lea rax, [rip + 2f]",
            "push rax",
            "retfq",
            "2:",
            ptr = in(reg) &ptr,
            cs = const KERNEL_CS as u64,
            ds = const KERNEL_DS,
            out("rax") _,
        );

        // Load TSS.
        asm!("ltr ax", in("ax") TSS_SELECTOR);
    }
}

