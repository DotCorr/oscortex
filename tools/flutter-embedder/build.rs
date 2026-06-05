// tools/flutter-embedder/build.rs
// Pass the custom linker script to rustc so the embedder binary loads
// at USER_ELF_BASE (0x400000) in each process's address space.
fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-arg=-T{dir}/user.ld");
    println!("cargo:rustc-link-arg=-z");
    println!("cargo:rustc-link-arg=noexecstack");
    println!("cargo:rerun-if-changed=user.ld");
    println!("cargo:rerun-if-changed=build.rs");
}
