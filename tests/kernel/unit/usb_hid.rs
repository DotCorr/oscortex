#[path = "../../../kernel/src/drivers/common/usb_hid.rs"]
mod usb_hid;

use usb_hid::{handle_boot_keyboard_report, usb_keycode_to_scancode, HID_KEY_A};

#[test]
fn maps_hid_a_to_ps2_scancode() {
    assert_eq!(usb_keycode_to_scancode(HID_KEY_A), Some(0x1E));
}

#[test]
fn boot_report_parses_key_press() {
    let report = [0u8, 0u8, HID_KEY_A, 0, 0, 0, 0, 0];
    assert_eq!(handle_boot_keyboard_report(&report), Some((0x1E, true)));
}

#[test]
fn empty_report_is_none() {
    assert!(handle_boot_keyboard_report(&[0; 8]).is_none());
}
