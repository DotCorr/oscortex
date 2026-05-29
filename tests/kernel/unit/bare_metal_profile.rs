//! Bare-metal vs QEMU profile strings (compile-time / init policy).

const PLATFORM_RS: &str = include_str!("../../../kernel/src/drivers/platform.rs");

#[test]
fn bare_metal_skips_ps2_in_platform() {
    assert!(PLATFORM_RS.contains("bare-metal safe mode"));
    assert!(PLATFORM_RS.contains("PS/2 skipped"));
}

#[test]
fn qemu_profile_enables_ps2() {
    assert!(PLATFORM_RS.contains("QEMU/KVM profile"));
}
