//! USB HID → WM routing (boot keyboard reports).

use crate::drivers::common::usb_hid;

static mut SELF_TEST_DONE: bool = false;

/// Exercise HID report parse + WM `push_key` (integration marker on serial).
pub fn wm_route_self_test() {
    unsafe {
        if SELF_TEST_DONE {
            return;
        }
        SELF_TEST_DONE = true;
    }

    let report = [0u8, 0u8, usb_hid::HID_KEY_A, 0, 0, 0, 0, 0];
    match usb_hid::handle_boot_keyboard_report(&report) {
        Some((sc, pressed)) => {
            crate::wm::push_key(sc, pressed);
            log::info!("[USB-HID] WM route OK scancode={:#x}", sc);
        }
        None => {
            log::warn!("[USB-HID] WM route self-test parse failed");
        }
    }
}
