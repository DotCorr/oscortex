//! Memory-mapped I/O accessors — all driver MMIO goes through here.

#[inline(always)]
pub unsafe fn read32(base: u64, offset: usize) -> u32 {
    core::ptr::read_volatile((base + offset as u64) as *const u32)
}

#[inline(always)]
pub unsafe fn write32(base: u64, offset: usize, value: u32) {
    core::ptr::write_volatile((base + offset as u64) as *mut u32, value);
}

#[inline(always)]
pub unsafe fn read64(base: u64, offset: usize) -> u64 {
    core::ptr::read_volatile((base + offset as u64) as *const u64)
}

#[inline(always)]
pub unsafe fn write64(base: u64, offset: usize, value: u64) {
    core::ptr::write_volatile((base + offset as u64) as *mut u64, value);
}
