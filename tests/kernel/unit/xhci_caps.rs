#[path = "../../../kernel/src/drivers/common/xhci_caps.rs"]
mod xhci_caps;

use xhci_caps::{
    halted_after_reset, parse_capability, reset_complete, USBCMD_HCRST, USBSTS_HCH,
};

#[test]
fn parse_valid_capability_block() {
    let cap = 0x0100_0020u32;
    let hcsparams1 = 0x0800_0008u32;
    let c = parse_capability(cap, hcsparams1).unwrap();
    assert_eq!(c.cap_length, 0x20);
    assert_eq!(c.hci_version, 0x0100);
    assert_eq!(c.max_slots, 8);
    assert_eq!(c.max_ports, 8);
    assert_eq!(c.usbcmd_off, 0x20);
    assert_eq!(c.usbsts_off, 0x24);
}

#[test]
fn rejects_zero_caplength() {
    assert!(parse_capability(0, 0).is_err());
}

#[test]
fn reset_and_halt_state_helpers() {
    assert!(!reset_complete(USBCMD_HCRST));
    assert!(reset_complete(0));
    assert!(halted_after_reset(USBSTS_HCH));
    assert!(!halted_after_reset(0));
}
