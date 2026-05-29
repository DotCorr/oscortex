use std::process::Command;
use std::path::PathBuf;

fn main() {
    let task = std::env::args().nth(1);
    match task.as_deref() {
        Some("build") => build(),
        Some("run") => run(),
        Some("iso") => iso(),
        Some("test") => test(false),
        Some("test-qemu") => test(true),
        _ => {
            eprintln!("Usage: cargo xtask <build|run|iso|test|test-qemu>");
            eprintln!("  build      — build the kernel ELF");
            eprintln!("  run        — build and launch in QEMU");
            eprintln!("  iso        — build bootable ISO image");
            eprintln!("  test       — host kernel driver unit tests");
            eprintln!("  test-qemu  — unit tests + QEMU driver integration");
        }
    }
}

fn workspace_root() -> PathBuf {
    let manifest = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest).join("..").join("..").canonicalize().unwrap()
}

fn build() {
    let root = workspace_root();
    let status = Command::new("cargo")
        .args([
            "+nightly",
            "build",
            "--release",
            "--package", "oscortex-kernel",
            "--target", "x86_64-unknown-none",
            "-Z", "build-std=core,compiler_builtins,alloc",
            "-Z", "build-std-features=compiler-builtins-mem",
        ])
        .current_dir(&root)
        .status()
        .expect("cargo build failed");
    if !status.success() {
        std::process::exit(1);
    }
}

fn iso() {
    build();
    let root = workspace_root();
    // Generate limine.conf and build ISO using xorriso + limine
    let iso_dir = root.join("iso_root");
    std::fs::create_dir_all(iso_dir.join("boot/limine")).unwrap();
    std::fs::create_dir_all(iso_dir.join("EFI/BOOT")).unwrap();

    // Copy kernel ELF
    let kernel_elf = root.join("target/x86_64-unknown-none/release/kernel");
    std::fs::copy(&kernel_elf, iso_dir.join("boot/kernel")).unwrap();

    // Write limine config
    std::fs::write(iso_dir.join("boot/limine/limine.conf"), LIMINE_CONF).unwrap();

    // Run xorriso to create ISO
    let status = Command::new("xorriso")
        .args([
            "-as", "mkisofs",
            "-b", "boot/limine/limine-bios-cd.bin",
            "-no-emul-boot", "-boot-load-size", "4", "-boot-info-table",
            "--efi-boot", "boot/limine/limine-uefi-cd.bin",
            "-efi-boot-part", "--efi-boot-image", "--protective-msdos-label",
            iso_dir.to_str().unwrap(),
            "-o", root.join("oscortex.iso").to_str().unwrap(),
        ])
        .status()
        .expect("xorriso not found — run: brew install xorriso");
    if !status.success() {
        eprintln!("ISO build failed");
        std::process::exit(1);
    }
    println!("ISO built: oscortex.iso");
}

fn run() {
    iso();
    let root = workspace_root();
    let status = Command::new("qemu-system-x86_64")
        .args([
            "-cdrom", root.join("oscortex.iso").to_str().unwrap(),
            "-cpu", "qemu64,+x2apic",
            "-m", "512M",
            "-smp", "4",
            "-serial", "stdio",
            "-display", "none",
            "-no-reboot",
            "-d", "int",
            "-D", root.join("qemu-log.txt").to_str().unwrap(),
        ])
        .status()
        .expect("qemu-system-x86_64 not found — run: brew install qemu");
    if !status.success() {
        std::process::exit(1);
    }
}

fn test(qemu: bool) {
    let root = workspace_root();
    let script = root.join("tests/run_all.sh");
    let mut cmd = Command::new("bash");
    cmd.arg(script);
    if qemu {
        cmd.arg("--qemu");
    }
    let status = cmd.status().expect("tests/run_all.sh failed");
    if !status.success() {
        std::process::exit(1);
    }
}

const LIMINE_CONF: &str = r#"
# OSCortex boot configuration
default_entry: 1
timeout: 3

/OSCortex (AI-First Kernel)
    protocol: limine
    kernel_path: boot():/boot/kernel
    kaslr: no
"#;
