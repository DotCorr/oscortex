#[path = "../../../kernel/src/drivers/common/pci_bar.rs"]
mod pci_bar;

use pci_bar::{
    decode_bar0_io, decode_bar0_mmio, is_nvme, is_xhci, pci_class_fields, pci_class_matches,
};

#[test]
fn decode_32bit_mmio_bar() {
    let bar0_lo = 0xFE000004u32;
    assert_eq!(decode_bar0_mmio(bar0_lo, 0), Some(0xFE000000));
}

#[test]
fn decode_64bit_mmio_bar() {
    let bar0_lo = 0x0000000Cu32;
    let bar0_hi = 0x00000001u32;
    assert_eq!(decode_bar0_mmio(bar0_lo, bar0_hi), Some(0x1_00000000));
}

#[test]
fn rejects_io_bar_for_mmio_decode() {
    assert_eq!(decode_bar0_mmio(0x0001, 0), None);
}

#[test]
fn decode_io_bar() {
    assert_eq!(decode_bar0_io(0xC001), Some(0xC000));
    assert_eq!(decode_bar0_io(0xFE000004), None);
}

#[test]
fn pci_class_match_xhci_and_nvme() {
    let xhci = 0x0C033000u32;
    let nvme = 0x01080200u32;
    assert!(is_xhci(xhci));
    assert!(!is_nvme(xhci));
    assert!(is_nvme(nvme));
    assert_eq!(pci_class_fields(xhci), (0x0C, 0x03, 0x30));
    assert!(pci_class_matches(nvme, 0x01, 0x08, 0x02));
}
