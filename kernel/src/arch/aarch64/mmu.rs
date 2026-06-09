//! ARMv8-A MMU bring-up — translation tables + SCTLR_EL1.M enable.
//!
//! Configuration (the simplest correct setup for QEMU `-M virt`):
//!   * 4 KiB granule, 48-bit VA (T0SZ = T1SZ = 16).
//!   * TTBR0_EL1 walks the low half (identity map covering devices + RAM).
//!   * TTBR1_EL1 walks the kernel high half (mirror of the same identity map
//!     at 0xFFFF_0000_0000_0000+ so the kernel can later run higher-half).
//!   * MAIR attr0 = Normal Write-Back, attr1 = Device-nGnRE.
//!
//! We use L1 1 GiB block descriptors — the whole address space is covered by a
//! single 512-entry L1 table per TTBR, no L2/L3 needed for the identity map.
//! Region 0 (0x0–0x4000_0000) holds the GIC/UART MMIO and is mapped Device;
//! regions 1..N (RAM from 0x4000_0000) are mapped Normal cacheable.
//!
//! After this runs the MMU is ON and the kernel executes translated (the
//! identity map means VAs equal PAs, so code/stack/UART pointers stay valid).

use core::sync::atomic::{AtomicBool, Ordering};

// ── Descriptor + register bit definitions ───────────────────────────────────

/// L1 block descriptor: valid, block (bit[1]=0), AF, plus attr/shareability.
const DESC_VALID: u64 = 1 << 0;
const DESC_BLOCK: u64 = 0 << 1; // 0 = block at L1/L2; 1 = table/page
const DESC_AF: u64 = 1 << 10; // Access Flag (else first access faults)
const DESC_SH_INNER: u64 = 3 << 8; // Inner shareable
const DESC_AP_RW_EL1: u64 = 0 << 6; // EL1 RW, EL0 no access
const DESC_AP_RW_ALL: u64 = 1 << 6; // EL1 RW, EL0 RW

// AttrIndx[2:0] selects a MAIR attr byte.
const ATTR_IDX_NORMAL: u64 = 0 << 2; // MAIR attr0
const ATTR_IDX_DEVICE: u64 = 1 << 2; // MAIR attr1

/// MAIR_EL1: attr0 = Normal Inner/Outer Write-Back non-transient (0xFF),
/// attr1 = Device-nGnRE (0x04).
const MAIR_VALUE: u64 = 0x04 << 8 | 0xFF;

/// TCR_EL1 for 4 KiB granule, 39-bit VA on both halves, WB-cacheable walks,
/// inner-shareable. IPS = 40-bit (1 TiB) physical, plenty for QEMU virt.
///
/// T0SZ/T1SZ = 25 → 39-bit VA (512 GiB). With a 4 KiB granule this makes the
/// translation walk *start at level 1*, so a single 512-entry L1 table with
/// 1 GiB block descriptors covers the whole address space — no L0 needed.
const TCR_T0SZ: u64 = 25; // 64-25 = 39-bit VA (TTBR0)
const TCR_T1SZ: u64 = 25 << 16; // 39-bit VA (TTBR1)
const TCR_IRGN0_WB: u64 = 1 << 8;
const TCR_ORGN0_WB: u64 = 1 << 10;
const TCR_SH0_INNER: u64 = 3 << 12;
const TCR_TG0_4K: u64 = 0 << 14; // 4 KiB granule (TTBR0)
const TCR_IRGN1_WB: u64 = 1 << 24;
const TCR_ORGN1_WB: u64 = 1 << 26;
const TCR_SH1_INNER: u64 = 3 << 28;
const TCR_TG1_4K: u64 = 2 << 30; // 4 KiB granule (TTBR1)
const TCR_IPS_40BIT: u64 = 2 << 32;

const TCR_VALUE: u64 = TCR_T0SZ
    | TCR_T1SZ
    | TCR_IRGN0_WB
    | TCR_ORGN0_WB
    | TCR_SH0_INNER
    | TCR_TG0_4K
    | TCR_IRGN1_WB
    | TCR_ORGN1_WB
    | TCR_SH1_INNER
    | TCR_TG1_4K
    | TCR_IPS_40BIT;

// SCTLR_EL1 bits.
const SCTLR_M: u64 = 1 << 0; // MMU enable
const SCTLR_C: u64 = 1 << 2; // data cache enable
const SCTLR_I: u64 = 1 << 12; // instruction cache enable

// ── Translation tables (BSS-resident, page-aligned) ─────────────────────────

/// A single 512-entry, 4 KiB-aligned translation table.
#[repr(C, align(4096))]
struct Table([u64; 512]);

/// L1 table for the low half (TTBR0): identity map.
static mut L1_LOW: Table = Table([0; 512]);
/// L1 table for the high half (TTBR1): identity map mirror.
static mut L1_HIGH: Table = Table([0; 512]);

static MMU_ON: AtomicBool = AtomicBool::new(false);

/// Build an L1 block descriptor mapping the 1 GiB region at `pa` with the given
/// attribute index and access permissions.
#[inline]
fn block_desc(pa: u64, attr_idx: u64, ap: u64) -> u64 {
    (pa & 0x0000_FFFF_C000_0000) // 1 GiB-aligned output address (bits[47:30])
        | DESC_VALID
        | DESC_BLOCK
        | DESC_AF
        | DESC_SH_INNER
        | attr_idx
        | ap
}

/// Populate one L1 table with an identity map: index 0 (0x0–0x4000_0000) is
/// Device memory (covers GIC @ 0x0800_0000 and PL011 @ 0x0900_0000), indices
/// 1..=`ram_gibs` are Normal cacheable RAM starting at 0x4000_0000.
unsafe fn fill_identity(table: &mut Table, ram_gibs: u64) {
    // Entry 0: low 1 GiB = device MMIO (also covers the flash/boot ROM region).
    // Kernel-only (EL1 RW), Device-nGnRE.
    table.0[0] = block_desc(0x0000_0000, ATTR_IDX_DEVICE, DESC_AP_RW_EL1);
    // RAM region: 0x4000_0000 onward, one 1 GiB block per L1 entry. Kernel-only
    // RW+executable (EL0 has no access to the kernel identity map).
    let mut g = 1u64;
    while g <= ram_gibs && g < 512 {
        let pa = g << 30; // g GiB
        table.0[g as usize] = block_desc(pa, ATTR_IDX_NORMAL, DESC_AP_RW_EL1);
        g += 1;
    }
}

/// Returns the physical address of the active TTBR0 L1 table (for the shared
/// `arch::memory::read_cr3` view). Until per-process tables exist, all address
/// spaces share this identity map.
pub fn kernel_ttbr0() -> u64 {
    core::ptr::addr_of!(L1_LOW) as u64
}

/// Build the tables, program MAIR/TCR/TTBR, and enable the MMU + caches.
///
/// `ram_gibs` is how many GiB of RAM (from 0x4000_0000) to identity-map.
///
/// # Safety
/// Must run once, at EL1, with the MMU OFF and interrupts masked.
pub unsafe fn enable(ram_gibs: u64) {
    let l1_low = &mut *core::ptr::addr_of_mut!(L1_LOW);
    let l1_high = &mut *core::ptr::addr_of_mut!(L1_HIGH);
    fill_identity(l1_low, ram_gibs);
    fill_identity(l1_high, ram_gibs);

    let ttbr0 = core::ptr::addr_of!(L1_LOW) as u64;
    let ttbr1 = core::ptr::addr_of!(L1_HIGH) as u64;

    core::arch::asm!(
        // Program memory attributes + translation control.
        "msr mair_el1, {mair}",
        "msr tcr_el1, {tcr}",
        "msr ttbr0_el1, {ttbr0}",
        "msr ttbr1_el1, {ttbr1}",
        "isb",
        // Invalidate all TLB entries for EL1, then barrier.
        "tlbi vmalle1",
        "dsb nsh",
        "isb",
        mair = in(reg) MAIR_VALUE,
        tcr = in(reg) TCR_VALUE,
        ttbr0 = in(reg) ttbr0,
        ttbr1 = in(reg) ttbr1,
        options(nostack, preserves_flags),
    );

    // Enable MMU + I/D caches in SCTLR_EL1.
    let mut sctlr: u64;
    core::arch::asm!("mrs {}, sctlr_el1", out(reg) sctlr, options(nomem, nostack));
    sctlr |= SCTLR_M | SCTLR_C | SCTLR_I;
    core::arch::asm!(
        "msr sctlr_el1, {}",
        "isb",
        in(reg) sctlr,
        options(nostack, preserves_flags),
    );

    MMU_ON.store(true, Ordering::Release);
}

/// True once [`enable`] has turned the MMU on.
pub fn is_on() -> bool {
    MMU_ON.load(Ordering::Acquire)
}

// ── EL0 user mapping (bring-up) ──────────────────────────────────────────────

/// Table descriptor: valid + table type (points at the next-level table).
const DESC_TABLE: u64 = (1 << 0) | (1 << 1);

/// Page (L3) descriptor type bit (bit[1]=1 means page at L3).
const DESC_PAGE: u64 = (1 << 0) | (1 << 1);

/// UXN/PXN — execute-never controls (bits[54]=UXN, [53]=PXN).
const DESC_UXN: u64 = 1 << 54;
const DESC_PXN: u64 = 1 << 53;

/// L2 + L3 tables backing one user region (covers up to 2 MiB of 4 KiB pages
/// under a single L2 entry). One static instance is enough for the bring-up
/// EL0 test process.
static mut USER_L2: Table = Table([0; 512]);
static mut USER_L3: Table = Table([0; 512]);

/// Map a single 4 KiB physical page `pa` at user VA `va` with EL0 RW + execute
/// (PXN=1 so EL1 can't execute it, UXN=0 so EL0 can). The VA must lie in the
/// 2 MiB window selected by the L1/L2 indices computed here, and `va`'s L1
/// index must be free in the current TTBR0 L1 table.
///
/// # Safety
/// Must run at EL1 with the MMU on; mutates the shared TTBR0 L1 plus the static
/// USER_L2/USER_L3 tables. Intended for the single bring-up user region.
pub unsafe fn map_user_page(va: u64, pa: u64) {
    let l1_idx = ((va >> 30) & 0x1FF) as usize;
    let l2_idx = ((va >> 21) & 0x1FF) as usize;
    let l3_idx = ((va >> 12) & 0x1FF) as usize;

    let l1 = &mut *core::ptr::addr_of_mut!(L1_LOW);
    let l2 = &mut *core::ptr::addr_of_mut!(USER_L2);
    let l3 = &mut *core::ptr::addr_of_mut!(USER_L3);

    // L1[l1_idx] → USER_L2 table.
    l1.0[l1_idx] = (core::ptr::addr_of!(USER_L2) as u64 & 0x0000_FFFF_FFFF_F000) | DESC_TABLE;
    // L2[l2_idx] → USER_L3 table.
    l2.0[l2_idx] = (core::ptr::addr_of!(USER_L3) as u64 & 0x0000_FFFF_FFFF_F000) | DESC_TABLE;
    // L3[l3_idx] → the 4 KiB page, EL0 RW + executable, Normal memory.
    l3.0[l3_idx] = (pa & 0x0000_FFFF_FFFF_F000)
        | DESC_PAGE
        | DESC_AF
        | DESC_SH_INNER
        | ATTR_IDX_NORMAL
        | DESC_AP_RW_ALL // EL0 + EL1 RW
        | DESC_PXN; // privileged execute-never; UXN=0 → EL0 may execute

    // Make the new mappings visible and flush the affected VA.
    core::arch::asm!(
        "dsb ishst",
        "tlbi vmalle1is",
        "dsb ish",
        "isb",
        options(nostack, preserves_flags),
    );
}
