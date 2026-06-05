//! Virtual memory / paging.
//!
//! x86_64: full 4-level page table walker that extends Limine's CR3 in-place.
//! aarch64 / riscv64: stubs — architecture-specific walkers added in later milestones.

use bitflags::bitflags;
use spin::Mutex;

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

#[cfg(not(target_arch = "x86_64"))]
mod stub_impl {
    use super::PageFlags;

    pub unsafe fn map_page(_virt: u64, _phys: u64, _flags: PageFlags) {
        // Milestone 4b: aarch64/riscv64 page table walker (TTBR0/TTBR1, SATP).
    }

    pub unsafe fn map_mmio(_phys: u64, _virt: u64, _size: usize) {
        // Milestone 4b: identity/MMIO map via arch-specific page tables.
    }

    pub fn init(_hhdm_offset: u64) {
        crate::logger::early_print("[MM::Paging] Virtual memory manager online (stub — non-x86)\r\n");
    }
}

// ─── Public API (dispatches to the right impl) ────────────────────────────────

/// Initialise the virtual memory manager for the current architecture.
pub fn init(hhdm_offset: u64) {
    #[cfg(target_arch = "x86_64")]
    x86_64_impl::init(hhdm_offset);
    #[cfg(not(target_arch = "x86_64"))]
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
    #[cfg(not(target_arch = "x86_64"))]
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
    #[cfg(not(target_arch = "x86_64"))]
    unsafe { stub_impl::map_mmio(phys, virt, size) }
}

/// Handle a demand-page fault. Returns `true` if the fault was resolved.
pub fn demand_page(cr2: u64, error: u64) -> bool {
    if error & 0x1 != 0 { return false; } // page present — protection fault, not demand page

    // Read current CR3 to get the active PML4.
    let cr3_phys: u64;
    unsafe { core::arch::asm!("mov {}, cr3", out(reg) cr3_phys) };
    let cr3_phys = cr3_phys & 0x000f_ffff_ffff_f000;

    // Check if the fault address is in the mirroring range [0x1_0000_0000, 0x3_0000_0000)
    let page_va = cr2 & !0xFFF;
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

    // Phase 32-C: Demand page anonymous memory in range [0x3_0000_0000, 0x4_0000_0000)
    if page_va >= 0x3_0000_0000 && page_va < 0x4_0000_0000 {
        if let Some(phys) = frame_allocator::alloc_frame() {
            let hhdm = (phys + frame_allocator::hhdm_offset()) as *mut u8;
            unsafe { core::ptr::write_bytes(hhdm, 0, 4096); }
            if unsafe { map_user_page_with_flags(cr3_phys, page_va, phys, true, false) }.is_ok() {
                return true;
            } else {
                frame_allocator::free_frame(phys);
            }
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
    #[cfg(not(target_arch = "x86_64"))]
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
    #[cfg(not(target_arch = "x86_64"))]
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
    #[cfg(not(target_arch = "x86_64"))]
    { let _ = (pml4_phys, virt, phys, writable, exec); Ok(()) }
}

/// Walk a user PML4 and return the physical frame already mapped at `virt`,
/// or `None` if the page is not yet mapped.
pub fn translate_user_page(pml4_phys: u64, virt: u64) -> Option<u64> {
    let _lock = PAGE_TABLE_LOCK.lock();
    #[cfg(target_arch = "x86_64")]
    { x86_64_impl::translate_user_page(pml4_phys, virt) }
    #[cfg(not(target_arch = "x86_64"))]
    { let _ = (pml4_phys, virt); None }
}

/// Walk a user PML4 and return the physical frame mapped at `virt` along with
/// its writable and executable flags.
pub fn translate_user_page_flags(pml4_phys: u64, virt: u64) -> Option<(u64, bool, bool)> {
    let _lock = PAGE_TABLE_LOCK.lock();
    #[cfg(target_arch = "x86_64")]
    { x86_64_impl::translate_user_page_flags(pml4_phys, virt) }
    #[cfg(not(target_arch = "x86_64"))]
    { let _ = (pml4_phys, virt); None }
}

/// Free all frames in a user PML4 and the PML4 itself.
pub fn free_user_pml4(pml4_phys: u64) {
    let _lock = PAGE_TABLE_LOCK.lock();
    #[cfg(target_arch = "x86_64")]
    x86_64_impl::free_user_pml4(pml4_phys);
    #[cfg(not(target_arch = "x86_64"))]
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
    #[cfg(not(target_arch = "x86_64"))]
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
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = (pml4_phys, virt);
        Err("unsupported")
    }
}

/// Unmap a range of virtual addresses in a specific PML4 and free their physical frames.
pub fn unmap_user_range(pml4_phys: u64, start_va: u64, size: u64) {
    if size == 0 { return; }
    let start = start_va & !0xFFF;
    let end = (start_va + size + 4095) & !0xFFF;
    for virt in (start..end).step_by(4096) {
        unsafe {
            if let Ok(phys) = unmap_user_page(pml4_phys, virt) {
                crate::mm::frame_allocator::free_frame(phys);
            }
        }
    }
}


