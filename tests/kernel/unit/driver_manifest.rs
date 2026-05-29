//! Documentation-driven compliance: every native driver must appear in
//! `docs/hardware.txt` and `drivers/platform.rs`.

const HARDWARE_DOC: &str = include_str!("../../../docs/hardware.txt");
const PLATFORM_RS: &str = include_str!("../../../kernel/src/drivers/platform.rs");

const EXPECTED_NATIVE_DRIVERS: &[&str] = &[
    "ps2",
    "xhci",
    "uart",
    "virtio-net",
    "virtio-blk",
    "nvme",
];

const EXPECTED_DRIVER_MODULES: &[&str] = &[
    "drivers/virtio_net.rs",
    "drivers/virtio_blk.rs",
    "drivers/nvme.rs",
    "drivers/usb.rs",
    "drivers/usb_hid.rs",
    "drivers/ps2.rs",
    "drivers/uart.rs",
    "drivers/fb.rs",
    "drivers/pty.rs",
    "drivers/platform.rs",
    "drivers/registry.rs",
    "drivers/wasm_sandbox.rs",
];

#[test]
fn hardware_doc_lists_driver_modules() {
    for module in EXPECTED_DRIVER_MODULES {
        assert!(
            HARDWARE_DOC.contains(module),
            "docs/hardware.txt must mention {module}"
        );
    }
}

#[test]
fn platform_registers_native_drivers() {
    for name in EXPECTED_NATIVE_DRIVERS {
        assert!(
            PLATFORM_RS.contains(&format!("b\"{name}\"")),
            "platform.rs must register_native(b\"{name}\")"
        );
    }
}

#[test]
fn hardware_doc_has_compliance_table() {
    assert!(HARDWARE_DOC.contains("Verdict by driver"));
    assert!(HARDWARE_DOC.contains("virtio_net"));
    assert!(HARDWARE_DOC.contains("virtio_blk"));
}
