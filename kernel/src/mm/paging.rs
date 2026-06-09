//! Virtual memory / paging.
//!
//! x86_64: full 4-level page table walker that extends Limine's CR3 in-place.
//! aarch64 / riscv64: stubs — architecture-specific walkers added in later milestones.

use bitflags::bitflags;
use spin::Mutex;
use crate::mm::frame_allocator;

pub(crate) static PAGE_TABLE_LOCK: Mutex<()> = Mutex::new(());

// ─── Page-table entry flags (shared across architectures) ─────────────────────

bitflags! {
    /// Page-table entry attribute bits (x86_64 layout; other arches use same API).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct PageFlags: u64 {
        const PRESENT       = 1 << 0;
        const WRITABLE      = 1 << 1;
        const USER          = 1 << 2;
        const WRITE_THROUGH = 1 << 3;
        const CACHE_DISABLE = 1 << 4;
        const ACCESSED      = 1 << 5;
        const DIRTY         = 1 << 6;
        const HUGE          = 1 << 7;
        const GLOBAL        = 1 << 8;
        const NO_EXECUTE    = 1 << 63;
    }
}

impl PageFlags {
    /// Flags for kernel MMIO mappings: present, writable, uncacheable, no-execute.
    #[inline]
    pub fn mmio() -> Self {
        Self::PRESENT | Self::WRITABLE | Self::CACHE_DISABLE | Self::WRITE_THROUGH | Self::NO_EXECUTE
    }

    /// Flags for normal kernel RW data pages.
    #[inline]
    pub fn kernel_rw() -> Self {
        Self::PRESENT | Self::WRITABLE | Self::NO_EXECUTE
    }
}

// ─── x86_64 implementation ────────────────────────────────────────────────────

#[cfg(target_arch = "x86_64")]
mod x86_64_impl {
    use core::arch::asm;
    use super::PageFlags;
    use crate::mm::frame_allocator;

    const PAGE_SIZE: usize = 4096;
    const PHYS_MASK: u64   = 0x000f_ffff_ffff_f000;

    #[inline]
    fn read_cr3() -> u64 {
        let cr3: u64;
        unsafe { asm!("mov {}, cr3", out(reg) cr3, options(nostack, nomem)) };
        cr3
    }

    #[inline]
    unsafe fn invlpg(virt: u64) {
        unsafe { asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags)) };
    }

    #[inline(never)]
    fn phys_to_virt(phys: u64) -> u64 {
        // QEMU runs with at most a few GB of RAM. Any "physical" address
        // beyond ~16 GiB is almost certainly garbage from a corrupted PTE.
        // Adding the HHDM base to such a value would yield a non-canonical
        // virtual address, which then page-faults as a #GP when dereferenced.
        if phys >= 0x0000_0004_0000_0000 {
            panic!(
                "phys_to_virt: corrupt PTE phys={:#x} — a page table was overwritten",
                phys
            );
        }
        // The HHDM base lives in the high-half (e.g. 0xffff_8000_0000_0000),
        // so any non-trivial `phys` makes the sum > u64::MAX under signed
        // arithmetic — Rust's debug-build overflow checks then panic on a
        // perfectly valid wrap. Use wrapping_add: the result is the correct
        // 64-bit virtual address either way.
        phys.wrapping_add(frame_allocator::hhdm_offset())
    }

    fn alloc_page_table() -> Option<u64> {
        let phys = frame_allocator::alloc_frame()?;
        unsafe { core::ptr::write_bytes(phys_to_virt(phys) as *mut u8, 0, PAGE_SIZE) };
        Some(phys)
    }

    unsafe fn ensure_next_table(entry: *mut u64) -> Option<u64> {
        let e = unsafe { entry.read_volatile() };
        if e & PageFlags::PRESENT.bits() != 0 {
            Some(e & PHYS_MASK)
        } else {
            let phys = alloc_page_table()?;
            unsafe {
                entry.write_volatile(phys | (PageFlags::PRESENT | PageFlags::WRITABLE).bits());
            }
            Some(phys)
        }
    }

    pub unsafe fn map_page(virt: u64, phys: u64, flags: PageFlags) {
        let pml4_idx = ((virt >> 39) & 0x1ff) as usize;
        let pdpt_idx = ((virt >> 30) & 0x1ff) as usize;
        let pd_idx   = ((virt >> 21) & 0x1ff) as usize;
        let pt_idx   = ((virt >> 12) & 0x1ff) as usize;

        let cr3_phys = read_cr3() & PHYS_MASK;

        let pml4 = phys_to_virt(cr3_phys) as *mut u64;
        let pdpt_phys = match unsafe { ensure_next_table(pml4.add(pml4_idx)) } {
            Some(p) => p,
            None => { log::error!("[MM::Paging] OOM: PDPT alloc failed (virt={:#x})", virt); return; }
        };
        let pdpt = phys_to_virt(pdpt_phys) as *mut u64;
        let pd_phys = match unsafe { ensure_next_table(pdpt.add(pdpt_idx)) } {
            Some(p) => p,
            None => { log::error!("[MM::Paging] OOM: PD alloc failed (virt={:#x})", virt); return; }
        };
        let pd = phys_to_virt(pd_phys) as *mut u64;
        let pt_phys = match unsafe { ensure_next_table(pd.add(pd_idx)) } {
            Some(p) => p,
            None => { log::error!("[MM::Paging] OOM: PT alloc failed (virt={:#x})", virt); return; }
        };
        let pt = phys_to_virt(pt_phys) as *mut u64;
        unsafe {
            pt.add(pt_idx).write_volatile((phys & PHYS_MASK) | flags.bits());
            invlpg(virt);
        }
    }

    pub unsafe fn map_mmio(phys: u64, virt: u64, size: usize) {
        let pages = size.div_ceil(PAGE_SIZE);
        let flags = PageFlags::mmio();
        for i in 0..pages as u64 {
            unsafe {
                map_page(
                    virt + i * PAGE_SIZE as u64,
                    phys + i * PAGE_SIZE as u64,
                    flags,
                );
            }
        }
        log::info!(
            "[MM::Paging] MMIO mapped: phys={:#x} → virt={:#x}, {} page(s)",
            phys, virt, pages
        );
    }

    pub fn init(_hhdm_offset: u64) {
        let cr3 = read_cr3() & PHYS_MASK;
        crate::logger::early_print("[MM::Paging] Virtual memory manager online (CR3=");
        let mut buf = [0u8; 18];
        buf[0] = b'0'; buf[1] = b'x';
        let hex = b"0123456789abcdef";
        for i in 0..16 {
            buf[2 + i] = hex[((cr3 >> (60 - i * 4)) & 0xf) as usize];
        }
        if let Ok(s) = core::str::from_utf8(&buf) {
            crate::logger::early_print(s);
        }
        crate::logger::early_print(")\r\n");
    }

    /// Allocate a new PML4 for a user process.
    /// The upper-half kernel mappings from the BSP's PML4 are copied in so
    /// that kernel code can run after a CR3 switch (e.g. during a syscall).
    pub fn alloc_user_pml4() -> Option<u64> {
        let new_pml4_phys = alloc_page_table()?;
        let new_pml4 = phys_to_virt(new_pml4_phys) as *mut u64;

        // Copy the upper-half kernel PML4 entries (indices 256–511) from the
        // current (kernel) PML4 into the new one.
        let kernel_cr3 = read_cr3() & PHYS_MASK;
        let kernel_pml4 = phys_to_virt(kernel_cr3) as *const u64;
        unsafe {
            for i in 256..512usize {
                new_pml4.add(i).write_volatile(kernel_pml4.add(i).read_volatile());
            }
        }
        Some(new_pml4_phys)
    }

    /// Map a single 4-KiB page in a *specific* PML4 (not the current CR3).
    /// Used to set up user address spaces before switching to them.
    ///
    /// # Safety
    /// `pml4_phys` must be a valid, allocated page-table root.
    pub unsafe fn map_page_in(pml4_phys: u64, virt: u64, phys: u64, flags: PageFlags)
        -> Result<(), &'static str>
    {
        let pml4_idx = ((virt >> 39) & 0x1ff) as usize;
        let pdpt_idx = ((virt >> 30) & 0x1ff) as usize;
        let pd_idx   = ((virt >> 21) & 0x1ff) as usize;
        let pt_idx   = ((virt >> 12) & 0x1ff) as usize;

        let pml4 = phys_to_virt(pml4_phys) as *mut u64;
        let user_flags = PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::USER;

        let pdpt_phys = unsafe { ensure_next_table_flags(pml4.add(pml4_idx), user_flags) }
            .ok_or("OOM: PDPT")?;
        let pdpt = phys_to_virt(pdpt_phys) as *mut u64;
        let pd_phys = unsafe { ensure_next_table_flags(pdpt.add(pdpt_idx), user_flags) }
            .ok_or("OOM: PD")?;
        let pd = phys_to_virt(pd_phys) as *mut u64;
        let pt_phys = unsafe { ensure_next_table_flags(pd.add(pd_idx), user_flags) }
            .ok_or("OOM: PT")?;
        let pt = phys_to_virt(pt_phys) as *mut u64;
        unsafe {
            pt.add(pt_idx).write_volatile((phys & PHYS_MASK) | flags.bits());
            let active_cr3 = read_cr3() & PHYS_MASK;
            if pml4_phys == active_cr3 {
                invlpg(virt);
            }
            if virt >= 0x112670000 && virt <= 0x112690000 {
                log::warn!("[DEBUG-PAGING] mapped virt={:#x} phys={:#x} in pml4={:#x} pt_entry={:#x}",
                    virt, phys, pml4_phys, pt.add(pt_idx).read_volatile());
            }
        }
        Ok(())
    }

    /// Walk a user PML4 and return the physical frame mapped at `virt`, or
    /// `None` if no mapping exists for that page.
    pub fn translate_user_page(pml4_phys: u64, virt: u64) -> Option<u64> {
        let pml4_idx = ((virt >> 39) & 0x1ff) as usize;
        let pdpt_idx = ((virt >> 30) & 0x1ff) as usize;
        let pd_idx   = ((virt >> 21) & 0x1ff) as usize;
        let pt_idx   = ((virt >> 12) & 0x1ff) as usize;
        let present  = PageFlags::PRESENT.bits();

        unsafe {
            let pml4 = phys_to_virt(pml4_phys) as *const u64;
            let pml4e = pml4.add(pml4_idx).read_volatile();
            if pml4e & present == 0 { return None; }
            let pdpt = phys_to_virt(pml4e & PHYS_MASK) as *const u64;
            let pdpte = pdpt.add(pdpt_idx).read_volatile();
            if pdpte & present == 0 { return None; }
            let pd = phys_to_virt(pdpte & PHYS_MASK) as *const u64;
            let pde = pd.add(pd_idx).read_volatile();
            if pde & present == 0 { return None; }
            let pt = phys_to_virt(pde & PHYS_MASK) as *const u64;
            let pte = pt.add(pt_idx).read_volatile();
            if pte & present == 0 { return None; }
            Some(pte & PHYS_MASK)
        }
    }

    pub fn translate_user_page_flags(pml4_phys: u64, virt: u64) -> Option<(u64, bool, bool)> {
        let pml4_idx = ((virt >> 39) & 0x1ff) as usize;
        let pdpt_idx = ((virt >> 30) & 0x1ff) as usize;
        let pd_idx   = ((virt >> 21) & 0x1ff) as usize;
        let pt_idx   = ((virt >> 12) & 0x1ff) as usize;
        let present  = PageFlags::PRESENT.bits();

        unsafe {
            let pml4 = phys_to_virt(pml4_phys) as *const u64;
            let pml4e = pml4.add(pml4_idx).read_volatile();
            if pml4e & present == 0 { return None; }
            let pdpt = phys_to_virt(pml4e & PHYS_MASK) as *const u64;
            let pdpte = pdpt.add(pdpt_idx).read_volatile();
            if pdpte & present == 0 { return None; }
            let pd = phys_to_virt(pdpte & PHYS_MASK) as *const u64;
            let pde = pd.add(pd_idx).read_volatile();
            if pde & present == 0 { return None; }
            let pt = phys_to_virt(pde & PHYS_MASK) as *const u64;
            let pte = pt.add(pt_idx).read_volatile();
            if pte & present == 0 { return None; }
            let phys = pte & PHYS_MASK;
            let writable = (pte & PageFlags::WRITABLE.bits()) != 0;
            let exec = (pte & PageFlags::NO_EXECUTE.bits()) == 0;
            Some((phys, writable, exec))
        }
    }

    pub unsafe fn update_user_page_flags(
        pml4_phys: u64,
        virt: u64,
        writable: bool,
        exec: bool,
    ) -> Result<(), &'static str> {
        let pml4_idx = ((virt >> 39) & 0x1ff) as usize;
        let pdpt_idx = ((virt >> 30) & 0x1ff) as usize;
        let pd_idx   = ((virt >> 21) & 0x1ff) as usize;
        let pt_idx   = ((virt >> 12) & 0x1ff) as usize;
        let present  = PageFlags::PRESENT.bits();

        let pml4 = phys_to_virt(pml4_phys) as *mut u64;
        let pml4e = pml4.add(pml4_idx).read_volatile();
        if pml4e & present == 0 { return Err("not present"); }
        let pdpt = phys_to_virt(pml4e & PHYS_MASK) as *mut u64;
        let pdpte = pdpt.add(pdpt_idx).read_volatile();
        if pdpte & present == 0 { return Err("not present"); }
        let pd = phys_to_virt(pdpte & PHYS_MASK) as *mut u64;
        let pde = pd.add(pd_idx).read_volatile();
        if pde & present == 0 { return Err("not present"); }
        let pt = phys_to_virt(pde & PHYS_MASK) as *mut u64;
        let pte_ptr = pt.add(pt_idx);
        let pte = pte_ptr.read_volatile();
        if pte & present == 0 { return Err("not present"); }

        let phys = pte & PHYS_MASK;
        let mut flags = PageFlags::PRESENT | PageFlags::USER;
        if writable { flags |= PageFlags::WRITABLE; }
        if !exec    { flags |= PageFlags::NO_EXECUTE; }

        pte_ptr.write_volatile(phys | flags.bits());
        invlpg(virt);
        Ok(())
    }


    /// Walk a user PML4 and free every page frame reachable through it
    /// (but not the kernel-half entries 256–511, which are shared).
    pub fn free_user_pml4(pml4_phys: u64) {
        let pml4 = phys_to_virt(pml4_phys) as *const u64;
        // Only walk lower-half entries (0–255).
        for i in 0..256usize {
            let pml4e = unsafe { pml4.add(i).read_volatile() };
            if pml4e & PageFlags::PRESENT.bits() == 0 { continue; }
            let pdpt_phys = pml4e & PHYS_MASK;
            let pdpt = phys_to_virt(pdpt_phys) as *const u64;
            for j in 0..512usize {
                let pdpte = unsafe { pdpt.add(j).read_volatile() };
                if pdpte & PageFlags::PRESENT.bits() == 0 { continue; }
                if pdpte & PageFlags::HUGE.bits() != 0 {
                    frame_allocator::free_frame(pdpte & PHYS_MASK);
                    continue;
                }
                let pd_phys = pdpte & PHYS_MASK;
                let pd = phys_to_virt(pd_phys) as *const u64;
                for k in 0..512usize {
                    let pde = unsafe { pd.add(k).read_volatile() };
                    if pde & PageFlags::PRESENT.bits() == 0 { continue; }
                    if pde & PageFlags::HUGE.bits() != 0 {
                        frame_allocator::free_frame(pde & PHYS_MASK);
                        continue;
                    }
                    let pt_phys = pde & PHYS_MASK;
                    let pt = phys_to_virt(pt_phys) as *const u64;
                    for l in 0..512usize {
                        let pte = unsafe { pt.add(l).read_volatile() };
                        if pte & PageFlags::PRESENT.bits() != 0 {
                            frame_allocator::free_frame(pte & PHYS_MASK);
                        }
                    }
                    frame_allocator::free_frame(pt_phys);
                }
                frame_allocator::free_frame(pd_phys);
            }
            frame_allocator::free_frame(pdpt_phys);
        }
        frame_allocator::free_frame(pml4_phys);
    }

    // Like ensure_next_table but takes explicit flags for USER-accessible tables.
    unsafe fn ensure_next_table_flags(entry: *mut u64, flags: PageFlags) -> Option<u64> {
        let e = unsafe { entry.read_volatile() };
        if e & PageFlags::PRESENT.bits() != 0 {
            Some(e & PHYS_MASK)
        } else {
            let phys = alloc_page_table()?;
            unsafe { entry.write_volatile(phys | flags.bits()); }
            Some(phys)
        }
    }

    /// Walk a user PML4 and free a mapped user page.
    pub unsafe fn unmap_user_page(pml4_phys: u64, virt: u64) -> Result<u64, &'static str> {
        let pml4_idx = ((virt >> 39) & 0x1ff) as usize;
        let pdpt_idx = ((virt >> 30) & 0x1ff) as usize;
        let pd_idx   = ((virt >> 21) & 0x1ff) as usize;
        let pt_idx   = ((virt >> 12) & 0x1ff) as usize;
        let present  = PageFlags::PRESENT.bits();

        let pml4 = phys_to_virt(pml4_phys) as *mut u64;
        let pml4e = pml4.add(pml4_idx).read_volatile();
        if pml4e & present == 0 { return Err("not present"); }
        let pdpt = phys_to_virt(pml4e & PHYS_MASK) as *mut u64;
        let pdpte = pdpt.add(pdpt_idx).read_volatile();
        if pdpte & present == 0 { return Err("not present"); }
        let pd = phys_to_virt(pdpte & PHYS_MASK) as *mut u64;
        let pde = pd.add(pd_idx).read_volatile();
        if pde & present == 0 { return Err("not present"); }
        let pt = phys_to_virt(pde & PHYS_MASK) as *mut u64;
        let pte_ptr = pt.add(pt_idx);
        let pte = pte_ptr.read_volatile();
        if pte & present == 0 { return Err("not present"); }

        let phys = pte & PHYS_MASK;
        pte_ptr.write_volatile(0);
        invlpg(virt);
        if virt >= 0x112670000 && virt <= 0x112690000 {
            log::warn!("[DEBUG-PAGING] unmapped virt={:#x} in pml4={:#x}", virt, pml4_phys);
        }
        Ok(phys)
    }
}

// ─── Non-x86_64 stubs ─────────────────────────────────────────────────────────

// ─── aarch64 implementation ───────────────────────────────────────────────────
//
// A 4 KiB-granule, 4-level (L0→L1→L2→L3) translation-table walker for the EL0
// (TTBR0) low half. Per-process address spaces each get a fresh L0 table from
// the frame allocator. The bring-up identity-maps RAM (VA == PA), so a table's
// physical address is also its directly-accessible kernel virtual address
// (HHDM offset is 0 on aarch64); we read/write tables through their PA.
//
// This matches the TCR_EL1 the bring-up programs (T0SZ=25 → 39-bit VA). With a
// 39-bit VA and 4 KiB granule, translation starts at level 1, so the top table
// is an L1 (512 entries × 1 GiB). We treat the per-process root as that L1.
#[cfg(target_arch = "aarch64")]
mod aarch64_impl {
    use super::PageFlags;
    use crate::mm::frame_allocator;

    const PAGE_SIZE: usize = 4096;
    /// Output-address mask for a table/page descriptor (bits[47:12]).
    const ADDR_MASK: u64 = 0x0000_FFFF_FFFF_F000;

    // Descriptor type/attribute bits (ARMv8-A, stage-1).
    const DESC_VALID: u64 = 1 << 0;
    const DESC_TABLE: u64 = 1 << 1; // at L1/L2: 1 = table, 0 = block. at L3: must be 1 (page).
    const DESC_AF: u64 = 1 << 10; // Access Flag
    const DESC_SH_INNER: u64 = 3 << 8; // inner shareable
    const DESC_AP_RW_ALL: u64 = 1 << 6; // EL0+EL1 RW (AP[1]=1)
    const DESC_AP_RO_ALL: u64 = 3 << 6; // EL0+EL1 RO
    const DESC_UXN: u64 = 1 << 54; // unprivileged execute-never
    const DESC_PXN: u64 = 1 << 53; // privileged execute-never
    const ATTR_IDX_NORMAL: u64 = 0 << 2; // MAIR attr0 = Normal WB (matches mmu.rs)

    /// 39-bit VA index extraction. With start level 1: L1[38:30], L2[29:21],
    /// L3[20:12].
    #[inline]
    fn idx_l1(va: u64) -> usize { ((va >> 30) & 0x1FF) as usize }
    #[inline]
    fn idx_l2(va: u64) -> usize { ((va >> 21) & 0x1FF) as usize }
    #[inline]
    fn idx_l3(va: u64) -> usize { ((va >> 12) & 0x1FF) as usize }

    /// A table is 512 u64 entries at a 4 KiB-aligned physical address. On the
    /// identity map, the PA is directly dereferenceable.
    #[inline]
    unsafe fn entry(table_pa: u64, idx: usize) -> *mut u64 {
        (table_pa as *mut u64).add(idx)
    }

    /// Allocate and zero a fresh translation table; returns its physical address.
    fn alloc_table() -> Option<u64> {
        let pa = frame_allocator::alloc_frame()?;
        unsafe { core::ptr::write_bytes(pa as *mut u8, 0, PAGE_SIZE) };
        Some(pa)
    }

    /// Allocate a fresh per-process root table (the L1 for a 39-bit VA space).
    ///
    /// CRITICAL: on the aarch64 bring-up the kernel itself runs from the low
    /// (TTBR0) half — its code, stack, vectors, and the UART/GIC MMIO are in the
    /// identity map programmed at boot. A per-process TTBR0 that contained only
    /// user pages would unmap the kernel the instant `write_cr3` loaded it, so
    /// the very next instruction fetch (and the fault handler) would fault with
    /// no way to recover. We therefore seed each new root with a copy of the
    /// kernel's identity-map L1 entries (the 1 GiB block descriptors for the
    /// device region + RAM). User L1 slots (which the kernel map leaves empty)
    /// are then filled by `map_page_in` for the process's own pages.
    pub fn alloc_user_pml4() -> Option<u64> {
        let root = alloc_table()?;
        let kernel_l1 = crate::arch::aarch64::mmu::kernel_ttbr0();
        unsafe {
            // Copy all 512 L1 entries from the kernel identity map. The kernel
            // map uses 1 GiB block descriptors at low indices; user VAs live at
            // higher indices (e.g. ~511 for the stack) which are zero in the
            // kernel map and get populated per-process.
            core::ptr::copy_nonoverlapping(
                kernel_l1 as *const u64,
                root as *mut u64,
                512,
            );
        }
        Some(root)
    }

    /// Ensure the next-level table referenced by `*slot` exists, allocating it if
    /// the descriptor is empty. Returns the child table's physical address.
    unsafe fn ensure_table(slot: *mut u64) -> Option<u64> {
        let desc = core::ptr::read_volatile(slot);
        if desc & DESC_VALID != 0 {
            return Some(desc & ADDR_MASK);
        }
        let child = alloc_table()?;
        core::ptr::write_volatile(slot, (child & ADDR_MASK) | DESC_VALID | DESC_TABLE);
        Some(child)
    }

    /// Build an L3 page descriptor for a user page.
    fn page_desc(phys: u64, flags: PageFlags) -> u64 {
        let mut d = (phys & ADDR_MASK)
            | DESC_VALID
            | DESC_TABLE // bit[1]=1 → page at L3
            | DESC_AF
            | DESC_SH_INNER
            | ATTR_IDX_NORMAL;
        if flags.contains(PageFlags::WRITABLE) {
            d |= DESC_AP_RW_ALL;
        } else {
            d |= DESC_AP_RO_ALL;
        }
        // Execution: x86 NO_EXECUTE bit → UXN here. We always set PXN (EL1 must
        // never execute user pages). When NO_EXECUTE is set, also set UXN.
        d |= DESC_PXN;
        if flags.contains(PageFlags::NO_EXECUTE) || !flags.contains(PageFlags::USER) {
            d |= DESC_UXN;
        }
        d
    }

    /// Map a 4 KiB page `phys` at user VA `virt` in the address space rooted at
    /// `root_pa` (the L1 table), allocating intermediate tables as needed.
    ///
    /// # Safety
    /// `root_pa` must be a valid table from [`alloc_user_pml4`]; `virt`/`phys`
    /// must be 4 KiB-aligned.
    pub unsafe fn map_page_in(
        root_pa: u64,
        virt: u64,
        phys: u64,
        flags: PageFlags,
    ) -> Result<(), &'static str> {
        let l1 = root_pa;
        let l2 = ensure_table(entry(l1, idx_l1(virt))).ok_or("OOM: l2 table")?;
        let l3 = ensure_table(entry(l2, idx_l2(virt))).ok_or("OOM: l3 table")?;
        core::ptr::write_volatile(entry(l3, idx_l3(virt)), page_desc(phys, flags));
        // Publish the new mapping + flush this VA in the EL0 space.
        core::arch::asm!(
            "dsb ishst",
            "tlbi vmalle1is",
            "dsb ish",
            "isb",
            options(nostack, preserves_flags),
        );
        Ok(())
    }

    /// Walk to the L3 descriptor for `virt`; returns (l3_slot, descriptor) if the
    /// full path is present, else None.
    unsafe fn walk(root_pa: u64, virt: u64) -> Option<(*mut u64, u64)> {
        let l1d = core::ptr::read_volatile(entry(root_pa, idx_l1(virt)));
        if l1d & DESC_VALID == 0 { return None; }
        let l2 = l1d & ADDR_MASK;
        let l2d = core::ptr::read_volatile(entry(l2, idx_l2(virt)));
        if l2d & DESC_VALID == 0 { return None; }
        let l3 = l2d & ADDR_MASK;
        let slot = entry(l3, idx_l3(virt));
        let d = core::ptr::read_volatile(slot);
        if d & DESC_VALID == 0 { return None; }
        Some((slot, d))
    }

    pub fn translate_user_page(root_pa: u64, virt: u64) -> Option<u64> {
        let page = virt & !0xFFF;
        let off = virt & 0xFFF;
        unsafe { walk(root_pa, page).map(|(_, d)| (d & ADDR_MASK) | off) }
    }

    pub fn translate_user_page_flags(root_pa: u64, virt: u64) -> Option<(u64, bool, bool)> {
        let page = virt & !0xFFF;
        unsafe {
            walk(root_pa, page).map(|(_, d)| {
                let phys = d & ADDR_MASK;
                let writable = (d & DESC_AP_RW_ALL) == DESC_AP_RW_ALL;
                let exec = (d & DESC_UXN) == 0;
                (phys, writable, exec)
            })
        }
    }

    /// Update the permission bits of an already-mapped user page.
    ///
    /// # Safety
    /// `root_pa` must be a valid allocated table.
    pub unsafe fn update_user_page_flags(
        root_pa: u64,
        virt: u64,
        writable: bool,
        exec: bool,
    ) -> Result<(), &'static str> {
        let page = virt & !0xFFF;
        let (slot, d) = walk(root_pa, page).ok_or("page not mapped")?;
        let mut nd = d & !(DESC_AP_RO_ALL | DESC_UXN);
        nd |= if writable { DESC_AP_RW_ALL } else { DESC_AP_RO_ALL };
        if !exec { nd |= DESC_UXN; }
        core::ptr::write_volatile(slot, nd);
        core::arch::asm!(
            "dsb ishst", "tlbi vmalle1is", "dsb ish", "isb",
            options(nostack, preserves_flags),
        );
        Ok(())
    }

    /// Unmap a single page; returns the physical frame it pointed at.
    ///
    /// # Safety
    /// `root_pa` must be a valid allocated table.
    pub unsafe fn unmap_user_page(root_pa: u64, virt: u64) -> Result<u64, &'static str> {
        let page = virt & !0xFFF;
        let (slot, d) = walk(root_pa, page).ok_or("not present")?;
        let phys = d & ADDR_MASK;
        core::ptr::write_volatile(slot, 0);
        core::arch::asm!(
            "dsb ishst", "tlbi vmalle1is", "dsb ish", "isb",
            options(nostack, preserves_flags),
        );
        Ok(phys)
    }

    /// Walk all three levels of a user root table and free every backing frame
    /// (leaf pages + intermediate tables + the root). Best-effort.
    ///
    /// IMPORTANT: each per-process root is seeded (in `alloc_user_pml4`) with a
    /// copy of the kernel's identity-map L1 entries, which are **1 GiB block
    /// descriptors** (bit[1] == 0), not table pointers. We MUST NOT recurse into
    /// or free those — their "address" is kernel RAM, not a process-owned table.
    /// Only L1 entries that are *table* descriptors (bit[1] == 1), i.e. the L2
    /// tables created for this process's own high-VA pages, are walked and freed.
    pub fn free_user_pml4(root_pa: u64) {
        if root_pa == 0 { return; }
        unsafe {
            for i1 in 0..512usize {
                let l1d = core::ptr::read_volatile(entry(root_pa, i1));
                // Skip invalid AND block descriptors (the shared kernel identity
                // map). Only table descriptors point at process-owned L2 tables.
                if l1d & DESC_VALID == 0 || l1d & DESC_TABLE == 0 { continue; }
                let l2 = l1d & ADDR_MASK;
                for i2 in 0..512usize {
                    let l2d = core::ptr::read_volatile(entry(l2, i2));
                    if l2d & DESC_VALID == 0 || l2d & DESC_TABLE == 0 { continue; }
                    let l3 = l2d & ADDR_MASK;
                    for i3 in 0..512usize {
                        let l3d = core::ptr::read_volatile(entry(l3, i3));
                        if l3d & DESC_VALID != 0 {
                            frame_allocator::free_frame(l3d & ADDR_MASK);
                        }
                    }
                    frame_allocator::free_frame(l3);
                }
                frame_allocator::free_frame(l2);
            }
            frame_allocator::free_frame(root_pa);
        }
    }

    // Kernel-space single mappings: the bring-up identity-maps devices + RAM, so
    // a fresh kernel `map_page`/`map_mmio` is only needed for windows outside the
    // boot map. For now these are no-ops (everything the kernel touches is in the
    // identity map); a follow-on can extend the kernel L1 if a new window is
    // required.
    pub unsafe fn map_page(_virt: u64, _phys: u64, _flags: PageFlags) {}
    pub unsafe fn map_mmio(_phys: u64, _virt: u64, _size: usize) {}

    pub fn init(_hhdm_offset: u64) {
        crate::logger::early_print("[MM::Paging] aarch64 TTBR0 page-table walker online\r\n");
    }
}

// ─── non-x86, non-aarch64 stub ────────────────────────────────────────────────
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
mod stub_impl {
    use super::PageFlags;

    pub unsafe fn map_page(_virt: u64, _phys: u64, _flags: PageFlags) {}
    pub unsafe fn map_mmio(_phys: u64, _virt: u64, _size: usize) {}

    pub fn init(_hhdm_offset: u64) {
        crate::logger::early_print("[MM::Paging] Virtual memory manager online (stub — non-x86)\r\n");
    }
}

// ─── Public API (dispatches to the right impl) ────────────────────────────────

/// Initialise the virtual memory manager for the current architecture.
pub fn init(hhdm_offset: u64) {
    #[cfg(target_arch = "x86_64")]
    x86_64_impl::init(hhdm_offset);
    #[cfg(target_arch = "aarch64")]
    aarch64_impl::init(hhdm_offset);
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    stub_impl::init(hhdm_offset);
}

/// Map a single 4-KiB page: virtual `virt` → physical `phys` with `flags`.
///
/// # Safety
/// * `set_hhdm_offset()` must have run before this.
/// * `virt` and `phys` must be 4-KiB aligned.
pub unsafe fn map_page(virt: u64, phys: u64, flags: PageFlags) {
    let _lock = PAGE_TABLE_LOCK.lock();
    #[cfg(target_arch = "x86_64")]
    unsafe { x86_64_impl::map_page(virt, phys, flags) }
    #[cfg(target_arch = "aarch64")]
    unsafe { aarch64_impl::map_page(virt, phys, flags) }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    unsafe { stub_impl::map_page(virt, phys, flags) }
}

/// Map a physical MMIO window `[phys, phys + size)` to virtual `[virt, virt + size)`.
///
/// # Safety
/// Same preconditions as [`map_page`].
pub unsafe fn map_mmio(phys: u64, virt: u64, size: usize) {
    let _lock = PAGE_TABLE_LOCK.lock();
    #[cfg(target_arch = "x86_64")]
    unsafe { x86_64_impl::map_mmio(phys, virt, size) }
    #[cfg(target_arch = "aarch64")]
    unsafe { aarch64_impl::map_mmio(phys, virt, size) }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    unsafe { stub_impl::map_mmio(phys, virt, size) }
}

/// Handle a demand-page fault. Returns `true` if the fault was resolved.
pub fn demand_page(cr2: u64, error: u64) -> bool {
    if error & 0x1 != 0 { return false; } // page present — protection fault, not demand page

    // Read current CR3 to get the active PML4.
    let cr3_phys = crate::arch::memory::read_cr3() & 0x000f_ffff_ffff_f000;

    let page_va = cr2 & !0xFFF;

    // Check if the page is already mapped (stale TLB entry due to another CPU core mapping it first)
    if translate_user_page(cr3_phys, page_va).is_some() {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            core::arch::asm!("invlpg [{}]", in(reg) page_va, options(nostack, preserves_flags));
        }
        #[cfg(target_arch = "aarch64")]
        unsafe {
            // Invalidate the stale TLB entry for this VA (EL1, inner-shareable),
            // then re-execute the faulting access.
            core::arch::asm!(
                "dsb ishst",
                "tlbi vmalle1is",
                "dsb ish",
                "isb",
                options(nostack, preserves_flags),
            );
        }
        return true;
    }

    // Check if the fault address is in the mirroring range [0x1_0000_0000, 0x3_0000_0000)
    if page_va >= 0x1_0000_0000 && page_va < 0x3_0000_0000 {
        let mirrored_va = page_va - 0x1_0000_0000;
        
        // Translate the mirrored page to get its physical frame and flags.
        let target = translate_user_page_flags(cr3_phys, mirrored_va);
        
        if let Some((phys, writable, exec)) = target {
            if let Err(e) = unsafe { map_user_page_with_flags(cr3_phys, page_va, phys, writable, exec) } {
                log::error!("[demand_page] mirrored map failed at {:#x}: {}", page_va, e);
                return false;
            }
            return true;
        }
    }

    // Demand-page anonymous memory in [0x3_0000_0000, 0x100_0000_0000).
    // Upper bound extended to cover ANON_VA_BASE (0x60_0000_0000 = 384 GiB)
    // and any thread stacks / TLS blocks placed by the bump allocator.
    if page_va >= 0x3_0000_0000 && page_va < 0x100_0000_0000 {
        let frame_opt = frame_allocator::alloc_frame();
        if let Some(phys) = frame_opt {
            let hhdm = (phys + frame_allocator::hhdm_offset()) as *mut u8;
            unsafe { core::ptr::write_bytes(hhdm, 0, 4096); }
            // Map anon memory executable (RWX). Dart AOT mmaps executable regions
            // (mmap prot=7) that get relocated into this anon window; the lazy demand
            // pager must honor execute or calls into them NX-fault. W^X isn't a concern
            // on this single-app bare-metal OS.
            let map_res = unsafe { map_user_page_with_flags(cr3_phys, page_va, phys, true, true) };
            if map_res.is_ok() {
                return true;
            } else {
                log::error!("[demand_page] map_user_page_with_flags failed at {:#x}: {:?}", page_va, map_res.err());
                frame_allocator::free_frame(phys);
            }
        } else {
            log::error!("[demand_page] OOM: alloc_frame returned None for addr {:#x}", page_va);
        }
    }

    // Reject all other page faults outside the mirrored range.
    false
}

// ── User address space helpers ────────────────────────────────────────────────

/// Allocate a fresh PML4 for a user process with kernel upper-half shared.
pub fn alloc_user_pml4() -> Option<u64> {
    #[cfg(target_arch = "x86_64")]
    return x86_64_impl::alloc_user_pml4();
    #[cfg(target_arch = "aarch64")]
    return aarch64_impl::alloc_user_pml4();
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    return None;
}

/// Map a 4-KiB user page in an *off-CPU* PML4 (RW + USER, no-execute).
///
/// # Safety
/// `pml4_phys` must be a valid allocated PML4.
pub unsafe fn map_user_page(pml4_phys: u64, virt: u64, phys: u64)
    -> Result<(), &'static str>
{
    let _lock = PAGE_TABLE_LOCK.lock();
    #[cfg(target_arch = "x86_64")]
    return unsafe {
        x86_64_impl::map_page_in(
            pml4_phys, virt, phys,
            PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::USER | PageFlags::NO_EXECUTE,
        )
    };
    #[cfg(target_arch = "aarch64")]
    return unsafe {
        aarch64_impl::map_page_in(
            pml4_phys, virt, phys,
            PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::USER | PageFlags::NO_EXECUTE,
        )
    };
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    { let _ = (pml4_phys, virt, phys); Ok(()) }
}

/// Map a 4-KiB user page with explicit writable/exec flags.
///
/// # Safety
/// `pml4_phys` must be a valid allocated PML4.
pub unsafe fn map_user_page_with_flags(
    pml4_phys: u64, virt: u64, phys: u64,
    writable: bool, exec: bool,
) -> Result<(), &'static str> {
    let _lock = PAGE_TABLE_LOCK.lock();
    #[cfg(target_arch = "x86_64")]
    {
        let mut flags = PageFlags::PRESENT | PageFlags::USER;
        if writable { flags |= PageFlags::WRITABLE; }
        if !exec    { flags |= PageFlags::NO_EXECUTE; }
        return unsafe { x86_64_impl::map_page_in(pml4_phys, virt, phys, flags) };
    }
    #[cfg(target_arch = "aarch64")]
    {
        let mut flags = PageFlags::PRESENT | PageFlags::USER;
        if writable { flags |= PageFlags::WRITABLE; }
        if !exec    { flags |= PageFlags::NO_EXECUTE; }
        return unsafe { aarch64_impl::map_page_in(pml4_phys, virt, phys, flags) };
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    { let _ = (pml4_phys, virt, phys, writable, exec); Ok(()) }
}

/// Walk a user PML4 and return the physical frame already mapped at `virt`,
/// or `None` if the page is not yet mapped.
pub fn translate_user_page(pml4_phys: u64, virt: u64) -> Option<u64> {
    let _lock = PAGE_TABLE_LOCK.lock();
    #[cfg(target_arch = "x86_64")]
    { x86_64_impl::translate_user_page(pml4_phys, virt) }
    #[cfg(target_arch = "aarch64")]
    { aarch64_impl::translate_user_page(pml4_phys, virt) }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    { let _ = (pml4_phys, virt); None }
}

/// Walk a user PML4 and return the physical frame mapped at `virt` along with
/// its writable and executable flags.
pub fn translate_user_page_flags(pml4_phys: u64, virt: u64) -> Option<(u64, bool, bool)> {
    let _lock = PAGE_TABLE_LOCK.lock();
    #[cfg(target_arch = "x86_64")]
    { x86_64_impl::translate_user_page_flags(pml4_phys, virt) }
    #[cfg(target_arch = "aarch64")]
    { aarch64_impl::translate_user_page_flags(pml4_phys, virt) }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    { let _ = (pml4_phys, virt); None }
}

/// Free all frames in a user PML4 and the PML4 itself.
pub fn free_user_pml4(pml4_phys: u64) {
    let _lock = PAGE_TABLE_LOCK.lock();
    #[cfg(target_arch = "x86_64")]
    x86_64_impl::free_user_pml4(pml4_phys);
    #[cfg(target_arch = "aarch64")]
    aarch64_impl::free_user_pml4(pml4_phys);
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    let _ = pml4_phys;
}

/// Update the page flags of an existing mapped user page.
///
/// # Safety
/// `pml4_phys` must be a valid allocated PML4.
pub unsafe fn update_user_page(
    pml4_phys: u64,
    virt: u64,
    writable: bool,
    exec: bool,
) -> Result<(), &'static str> {
    let _lock = PAGE_TABLE_LOCK.lock();
    #[cfg(target_arch = "x86_64")]
    {
        unsafe { x86_64_impl::update_user_page_flags(pml4_phys, virt, writable, exec) }
    }
    #[cfg(target_arch = "aarch64")]
    {
        unsafe { aarch64_impl::update_user_page_flags(pml4_phys, virt, writable, exec) }
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        let _ = (pml4_phys, virt, writable, exec);
        Ok(())
    }
}

/// Unmap a single 4-KiB page in a specific PML4 and return its physical frame.
///
/// # Safety
/// `pml4_phys` must be a valid allocated PML4.
pub unsafe fn unmap_user_page(pml4_phys: u64, virt: u64) -> Result<u64, &'static str> {
    let _lock = PAGE_TABLE_LOCK.lock();
    #[cfg(target_arch = "x86_64")]
    {
        unsafe { x86_64_impl::unmap_user_page(pml4_phys, virt) }
    }
    #[cfg(target_arch = "aarch64")]
    {
        unsafe { aarch64_impl::unmap_user_page(pml4_phys, virt) }
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        let _ = (pml4_phys, virt);
        Err("unsupported")
    }
}

/// Unmap a range of virtual addresses in a specific PML4 and free their physical frames.
pub static UNMAP_NOT_PRESENT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static UNMAP_FREED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Unmap a range of virtual addresses in a specific PML4 and free their physical frames.
pub fn unmap_user_range(pml4_phys: u64, start_va: u64, size: u64) {
    if size == 0 { return; }
    let start = start_va & !0xFFF;
    let end = (start_va + size + 4095) & !0xFFF;
    let mut local_freed = 0;
    let mut local_not_present = 0;
    for virt in (start..end).step_by(4096) {
        unsafe {
            match unmap_user_page(pml4_phys, virt) {
                Ok(phys) => {
                    crate::mm::frame_allocator::free_frame(phys);
                    local_freed += 1;
                }
                Err(_) => {
                    local_not_present += 1;
                }
            }
        }
    }
    UNMAP_FREED.fetch_add(local_freed, core::sync::atomic::Ordering::Relaxed);
    UNMAP_NOT_PRESENT.fetch_add(local_not_present, core::sync::atomic::Ordering::Relaxed);

    static UNMAP_LOG_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let count = UNMAP_LOG_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if count % 1000 == 999 {
        log::warn!("[unmap_user_range] stats: calls={} last_freed={}/{} (total_freed={} total_not_present={})",
            count + 1, local_freed, local_freed + local_not_present,
            UNMAP_FREED.load(core::sync::atomic::Ordering::Relaxed),
            UNMAP_NOT_PRESENT.load(core::sync::atomic::Ordering::Relaxed));
    }
}


