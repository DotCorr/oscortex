//! NVMe register layout helpers (pure, no MMIO).

/// Doorbell register byte offset for queue `qid`.
pub fn doorbell_offset(qid: u16, tail: bool, doorbell_stride: u32) -> usize {
    let s = doorbell_stride as usize;
    0x1000 + (qid as usize * 2 + if tail { 0 } else { 1 }) * s
}

/// Extract doorbell stride (DSTRD) from CAP register.
pub fn doorbell_stride_from_cap(cap: u64) -> u32 {
    let dstrd = ((cap >> 32) & 0xF) as u32;
    4 << dstrd
}
