use std::path::PathBuf;
use std::process::Command;

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
    PathBuf::from(manifest)
        .join("..")
        .join("..")
        .canonicalize()
        .unwrap()
}

fn build() {
    let root = workspace_root();
    let status = Command::new("cargo")
        .args([
            "+nightly",
            "build",
            "--release",
            "--package",
            "oscortex-kernel",
            "--target",
            "x86_64-unknown-none",
            "-Z",
            "build-std=core,compiler_builtins,alloc",
            "-Z",
            "build-std-features=compiler-builtins-mem",
        ])
        .current_dir(&root)
        .status()
        .expect("cargo build failed");
    if !status.success() {
        std::process::exit(1);
    }
}

/// Bootable kernel ISO (Limine BIOS + UEFI). This is the canonical, portable
/// ISO path used both locally and in CI/release — it depends only on the kernel
/// ELF, xorriso, and a Limine install. The full Flutter-shell ISO (engine module
/// + AOT app) is staged by scripts/build-iso.sh and is not reproduced here.
///
/// Limine binaries are located via $LIMINE_DIR (default: Homebrew prefix). In CI
/// the release workflow checks out the Limine binary branch and points LIMINE_DIR
/// at it, so there is no second ISO recipe to keep in sync.
fn iso() {
    build();
    let root = workspace_root();

    let limine_dir = PathBuf::from(
        std::env::var("LIMINE_DIR").unwrap_or_else(|_| "/opt/homebrew/share/limine".to_string()),
    );
    if !limine_dir.join("limine-bios-cd.bin").exists() {
        eprintln!(
            "Limine binaries not found in {}.\n  \
             Set LIMINE_DIR to a Limine install (brew install limine), or check out\n  \
             the Limine binary branch and point LIMINE_DIR at it.",
            limine_dir.display()
        );
        std::process::exit(1);
    }

    let iso_dir = root.join("iso_root");
    let _ = std::fs::remove_dir_all(&iso_dir);
    std::fs::create_dir_all(iso_dir.join("boot/limine")).unwrap();
    std::fs::create_dir_all(iso_dir.join("EFI/BOOT")).unwrap();

    let kernel_elf = root.join("target/x86_64-unknown-none/release/kernel");
    std::fs::copy(&kernel_elf, iso_dir.join("boot/kernel")).expect("kernel ELF missing");

    // Stage Limine boot files. *.EFI are optional on older Limine layouts.
    for f in [
        "limine-bios-cd.bin",
        "limine-bios.sys",
        "limine-uefi-cd.bin",
    ] {
        std::fs::copy(limine_dir.join(f), iso_dir.join("boot/limine").join(f))
            .unwrap_or_else(|e| panic!("copy {f}: {e}"));
    }
    for f in ["BOOTX64.EFI", "BOOTIA32.EFI"] {
        if limine_dir.join(f).exists() {
            std::fs::copy(limine_dir.join(f), iso_dir.join("EFI/BOOT").join(f)).unwrap();
        }
    }

    let conf_path = iso_dir.join("boot/limine/limine.conf");
    std::fs::write(&conf_path, LIMINE_CONF).unwrap();
    // Limine searches a few well-known locations for the config.
    std::fs::copy(&conf_path, iso_dir.join("limine.conf")).unwrap();
    std::fs::copy(&conf_path, iso_dir.join("EFI/BOOT/limine.conf")).unwrap();

    let out = root.join("oscortex.iso");
    let status = Command::new("xorriso")
        .args([
            "-as",
            "mkisofs",
            "-b",
            "boot/limine/limine-bios-cd.bin",
            "-no-emul-boot",
            "-boot-load-size",
            "4",
            "-boot-info-table",
            "--efi-boot",
            "boot/limine/limine-uefi-cd.bin",
            "-efi-boot-part",
            "--efi-boot-image",
            "--protective-msdos-label",
            iso_dir.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
        ])
        .status()
        .expect("xorriso not found — run: brew install xorriso (apt: xorriso)");
    if !status.success() {
        eprintln!("ISO build failed");
        std::process::exit(1);
    }

    // Install the BIOS stage to the ISO (no-op for pure-UEFI boot). Best effort:
    // not all Limine packages ship the `limine` host tool.
    if let Ok(s) = Command::new("limine")
        .args(["bios-install", out.to_str().unwrap()])
        .status()
    {
        if !s.success() {
            eprintln!("warning: `limine bios-install` returned non-zero (UEFI boot still works)");
        }
    } else {
        eprintln!(
            "warning: `limine` host tool not found — skipping bios-install (UEFI boot still works)"
        );
    }

    println!("ISO built: {}", out.display());
}

fn run() {
    iso();
    let root = workspace_root();
    let status = Command::new("qemu-system-x86_64")
        .args([
            "-cdrom",
            root.join("oscortex.iso").to_str().unwrap(),
            "-cpu",
            "qemu64,+x2apic",
            "-m",
            "512M",
            "-smp",
            "4",
            "-serial",
            "stdio",
            "-display",
            "none",
            "-no-reboot",
            "-d",
            "int",
            "-D",
            root.join("qemu-log.txt").to_str().unwrap(),
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

// Kernel-only boot entry. The full Flutter ISO (scripts/build-iso.sh) appends a
// libflutter_engine.so module; this minimal entry boots the kernel by itself.
const LIMINE_CONF: &str = r#"# OSCortex boot configuration
timeout: 0
serial: yes
verbose: yes

/OSCortex AI-First Kernel
    protocol: limine
    path: boot():/boot/kernel
    kaslr: no
"#;
