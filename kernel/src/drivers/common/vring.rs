//! Legacy virtio split-ring layout math (virtio 0.9 / 1.0 legacy PCI).

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct VirtqDesc {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

pub const VIRTQ_DESC_F_NEXT: u16 = 1;
pub const VIRTQ_DESC_F_WRITE: u16 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VringLayout {
    pub qsize: u16,
    pub avail_idx_off: u32,
    pub avail_ring_off: u32,
    pub used_idx_off: u32,
    pub used_ring_off: u32,
    pub page_count: usize,
}

impl VringLayout {
    /// Compute ring layout for a power-of-two queue size.
    pub fn for_qsize(qsize: u16) -> Option<Self> {
        if qsize == 0 || (qsize & (qsize - 1)) != 0 {
            return None;
        }
        let q = qsize as u32;
        let desc_sz = 16 * q;
        let avail_ring_off = desc_sz + 4;
        let after_avail = avail_ring_off + 2 * q;
        let used_base = (after_avail + 4095) & !4095;
        let used_idx_off = used_base + 2;
        let used_ring_off = used_idx_off + 2;
        let used_ring_end = used_ring_off + 8 * q;
        let page_count = ((used_ring_end as usize) + 4095) / 4096;
        Some(Self {
            qsize,
            avail_idx_off: desc_sz + 2,
            avail_ring_off,
            used_idx_off,
            used_ring_off,
            page_count,
        })
    }

    pub fn used_elem_offset(&self, used_index: u16) -> u32 {
        let qmask = (self.qsize - 1) as u32;
        let slot = (used_index as u32) & qmask;
        self.used_ring_off + slot * 8
    }

    pub fn avail_slot_offset(&self, avail_index: u16) -> u32 {
        let qmask = (self.qsize - 1) as u32;
        let slot = (avail_index as u32) & qmask;
        self.avail_ring_off + slot * 2
    }
}
