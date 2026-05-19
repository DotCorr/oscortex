//! x86_64 memory helpers (page table manipulation, etc.)

/// CR3 read (current PML4 physical base).
pub fn read_cr3() -> u64 {
    let cr3: u64;
    unsafe { core::arch::asm!("mov {}, cr3", out(reg) cr3) };
    cr3
}

pub fn write_cr3(val: u64) {
    unsafe { core::arch::asm!("mov cr3, {}", in(reg) val) };
}
