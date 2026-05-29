//! USB HID boot-keyboard report parsing (pure, host-testable).

/// USB HID boot keyboard keycode for 'A' (press self-test).
pub const HID_KEY_A: u8 = 0x04;

/// Map a subset of USB HID boot keycodes to PS/2 set-1 scancodes for WM routing.
pub fn usb_keycode_to_scancode(hid: u8) -> Option<u32> {
    match hid {
        0x04 => Some(0x1E), // A
        0x05 => Some(0x30), // B
        0x1E => Some(0x11), // 1
        0x28 => Some(0x1C), // Enter
        0x29 => Some(0x01), // Escape
        _ => None,
    }
}

/// Parse an 8-byte boot keyboard input report → (scancode, pressed).
pub fn handle_boot_keyboard_report(report: &[u8]) -> Option<(u32, bool)> {
    if report.len() < 3 {
        return None;
    }
    let key = report[2];
    if key == 0 {
        return None;
    }
    usb_keycode_to_scancode(key).map(|s| (s, true))
}
