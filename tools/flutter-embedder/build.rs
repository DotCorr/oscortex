// tools/flutter-embedder/build.rs
// Pass the custom linker script to rustc so the embedder binary loads
// at USER_ELF_BASE (0x400000) in each process's address space.
fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    // The linker script is architecture-specific (OUTPUT_FORMAT/OUTPUT_ARCH
    // differ), so select it from the build target arch.
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let ld = match arch.as_str() {
        "aarch64" => "user_aarch64.ld",
        _ => "user.ld", // x86_64 / default
    };
    println!("cargo:rustc-link-arg=-T{dir}/{ld}");
    println!("cargo:rustc-link-arg=-z");
    println!("cargo:rustc-link-arg=noexecstack");
    println!("cargo:rerun-if-changed=user.ld");
    println!("cargo:rerun-if-changed=user_aarch64.ld");
    println!("cargo:rerun-if-changed=build.rs");
}
