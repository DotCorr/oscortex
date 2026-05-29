#[path = "../../../kernel/src/drivers/common/nvme_regs.rs"]
mod nvme_regs;

use nvme_regs::{doorbell_offset, doorbell_stride_from_cap};

#[test]
fn doorbell_stride_from_cap_register() {
    let cap = 0u64 | (2u64 << 32);
    assert_eq!(doorbell_stride_from_cap(cap), 16);
}

#[test]
fn doorbell_offset_queue_pairs() {
    assert_eq!(doorbell_offset(0, true, 4), 0x1000);
    assert_eq!(doorbell_offset(0, false, 4), 0x1004);
    assert_eq!(doorbell_offset(1, true, 4), 0x1008);
}
