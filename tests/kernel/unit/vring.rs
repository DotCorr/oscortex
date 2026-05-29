#[path = "../../../kernel/src/drivers/common/vring.rs"]
mod vring;

use vring::{VringLayout, VIRTQ_DESC_F_NEXT, VIRTQ_DESC_F_WRITE};

#[test]
fn rejects_non_power_of_two_qsize() {
    assert!(VringLayout::for_qsize(0).is_none());
    assert!(VringLayout::for_qsize(3).is_none());
    assert!(VringLayout::for_qsize(255).is_none());
}

#[test]
fn layout_for_qsize_256() {
    let l = VringLayout::for_qsize(256).unwrap();
    assert_eq!(l.qsize, 256);
    assert!(l.page_count >= 2);
    assert!(l.used_ring_off > l.avail_ring_off);
}

#[test]
fn used_elem_and_avail_slot_offsets_wrap() {
    let l = VringLayout::for_qsize(4).unwrap();
    assert_eq!(l.used_elem_offset(0), l.used_elem_offset(4));
    assert_eq!(l.avail_slot_offset(1), l.avail_ring_off + 2);
}

#[test]
fn virtq_desc_flag_constants() {
    assert_eq!(VIRTQ_DESC_F_NEXT, 1);
    assert_eq!(VIRTQ_DESC_F_WRITE, 2);
}
