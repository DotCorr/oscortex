fn main() {
    let dir  = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let out  = std::env::var("OUT_DIR").unwrap();

    let ld = match arch.as_str() {
        "aarch64" => "aarch64.ld",
        "riscv64" => "riscv64.ld",
        _         => "x86_64.ld",   // default / x86_64
    };

    println!("cargo:rustc-link-arg=-T{dir}/linker/{ld}");
    println!("cargo:rustc-link-arg=-z");
    println!("cargo:rustc-link-arg=noexecstack");
    println!("cargo:rerun-if-changed=linker/{ld}");
    println!("cargo:rerun-if-changed=linker/x86_64.ld");
    println!("cargo:rerun-if-changed=linker/aarch64.ld");
    println!("cargo:rerun-if-changed=linker/riscv64.ld");
    println!("cargo:rerun-if-changed=build.rs");

    // ── Initramfs ─────────────────────────────────────────────────────────
    // Build directly from `initramfs/` contents. Boot policy and required
    // artifacts are enforced by scripts/build-iso.sh.
    let ws_root      = std::path::Path::new(&dir).parent().unwrap().to_path_buf();
    let out_tar      = std::path::Path::new(&out).join("initramfs.tar");
    let initramfs_dir = ws_root.join("initramfs");

    // Collect tar entries: (archive_path, file_contents)
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();

    if initramfs_dir.is_dir() {
        println!("cargo:rerun-if-changed={}", initramfs_dir.display());
        collect_dir(&initramfs_dir, &initramfs_dir, &mut entries);
    }

    // Phase 33-B: inject liboscortex.so syscall shim.
    entries.retain(|(n, _)| n != "system/lib/liboscortex.so");
    entries.push((
        "system/lib/liboscortex.so".to_string(),
        generate_liboscortex_shim(),
    ));

    // Phase 34-B: inject liboscortex_embedder.so platform-callback stubs.
    entries.retain(|(n, _)| n != "system/lib/liboscortex_embedder.so");
    entries.push((
        "system/lib/liboscortex_embedder.so".to_string(),
        generate_liboscortex_embedder_shim(),
    ));

    // aarch64 port: provide a native EL0 `/init` so the production kernel_main can
    // spawn a userspace process + service its first syscalls on ARM. The x86 build
    // ships the Flutter host as `/init` (staged by scripts/build-iso.sh); on ARM
    // that host doesn't exist yet (engine is a follow-on), so without this there is
    // no native `/init` to spawn. This minimal program issues a couple of `write`
    // syscalls, a `getpid`, then `exit` — exercising the full spawn → SVC dispatch
    // → exit path through the SHARED syscall layer.
    if arch == "aarch64" {
        entries.retain(|(n, _)| n != "init");
        entries.push(("init".to_string(), generate_aarch64_init_elf()));
    }

    // Write the USTAR tar to OUT_DIR.
    let tar_bytes = build_ustar_tar(&entries);
    std::fs::write(&out_tar, &tar_bytes).unwrap();
}

// ── Directory walker ──────────────────────────────────────────────────────────

fn collect_dir(
    base: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<(String, Vec<u8>)>,
) {
    let rd = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return,
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let rel  = path.strip_prefix(base).unwrap();
        let name = rel.to_string_lossy().replace('\\', "/");

        if path.is_dir() {
            collect_dir(base, &path, out);
        } else {
            // Skip .gitkeep markers, hidden files, backup copies, and the
            // raw Flutter engine module which is handled separately.
            let fname = path.file_name().unwrap_or_default().to_string_lossy();
            if fname.starts_with('.') { continue; }
            // JIT MODE: every Flutter host (the shell AND each launched app) runs from
            // its own kernel_blob.bin, so they MUST be embedded. Keep the shell blob at
            // system/flutter/flutter_assets/ and each app blob at
            // Applications/<name>.app/flutter_assets/. Exclude any other stray blobs
            // (e.g. a legacy system/apps/ duplicate) to limit bloat.
            // NOTE (scalability follow-up): embedding ~40MB per app does not scale; the
            // real fix is to deliver app blobs as Limine modules / from disk so the
            // kernel ELF stays lean. This embed is the working-proof path.
            if fname == "kernel_blob.bin"
                && name != "system/flutter/flutter_assets/kernel_blob.bin"
                && !(name.starts_with("Applications/") && name.ends_with(".app/flutter_assets/kernel_blob.bin"))
            { continue; }
            if fname == "libflutter_engine.so" || fname.ends_with(".bak") { continue; }
            println!("cargo:rerun-if-changed={}", path.display());
            let data = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            out.push((name.to_owned(), data));
        }
    }
}

// ── USTAR tar builder ─────────────────────────────────────────────────────────

fn build_ustar_tar(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, data) in entries {
        ustar_append(&mut out, name, data);
    }
    // End-of-archive: two 512-byte zero blocks.
    out.extend_from_slice(&[0u8; 1024]);
    out
}

fn ustar_append(out: &mut Vec<u8>, name: &str, data: &[u8]) {
    let mut hdr = [0u8; 512];

    // Filename: up to 100 bytes.
    let nb = name.as_bytes();
    let nlen = nb.len().min(100);
    hdr[..nlen].copy_from_slice(&nb[..nlen]);

    // File mode 0755.
    hdr[100..107].copy_from_slice(b"0000755");
    // UID / GID = 0.
    hdr[108..115].copy_from_slice(b"0000000");
    hdr[116..123].copy_from_slice(b"0000000");
    // File size (octal, 11 chars + space).
    let size_str = format!("{:011o} ", data.len());
    hdr[124..136].copy_from_slice(size_str.as_bytes());
    // mtime = 0.
    hdr[136..147].copy_from_slice(b"00000000000");
    // typeflag '0' = regular file.
    hdr[156] = b'0';
    // USTAR magic + version.
    hdr[257..262].copy_from_slice(b"ustar");
    hdr[262] = 0;
    hdr[263..265].copy_from_slice(b"00");

    // Checksum: sum of header bytes with checksum field treated as spaces.
    let base_sum: u32 = hdr.iter().map(|&b| b as u32).sum::<u32>()
        + 8 * b' ' as u32   // checksum field will be spaces during calculation
        - hdr[148..156].iter().map(|&b| b as u32).sum::<u32>();
    // Write checksum: 6-octal-digit NUL space.
    let ck = format!("{:06o}\0 ", base_sum);
    let ck_bytes = ck.as_bytes();
    let ck_len = ck_bytes.len().min(8);
    hdr[148..148 + ck_len].copy_from_slice(&ck_bytes[..ck_len]);

    out.extend_from_slice(&hdr);
    out.extend_from_slice(data);

    // Pad data to 512-byte boundary.
    let rem = (512 - (data.len() % 512)) % 512;
    out.extend(std::iter::repeat(0u8).take(rem));
}

// ── liboscortex.so syscall shim ELF generator (Phase 33-B) ───────────────────

/// Generate a minimal ELF64 ET_DYN that exports every core OSCortex syscall
/// as a real, working C-ABI function.  Each stub does:
///
///   mov r10, rcx   ; forward 4th C arg (rcx) to Linux/OSCortex syscall ABI (r10)
///   mov eax, <nr>  ; syscall number (zero-extended to rax)
///   syscall
///   ret
///
/// Dart FFI can `DynamicLibrary.open('/system/lib/liboscortex.so')` and call
/// these directly with zero serialisation overhead — the FFI call compiles to a
/// single `call [rip+offset]` that runs two instructions and a `syscall`.
///
/// Stub size: 11 bytes, padded to 16.
fn generate_liboscortex_shim() -> Vec<u8> {
    // (exported_name, syscall_number)
    const SYMS: &[(&str, u32)] = &[
        // POSIX-like
        ("oscortex_write",                  1),
        ("oscortex_getpid",                 39),
        ("oscortex_exit",                   60),
        // Compositor
        ("oscortex_surface_create",         0x300),
        ("oscortex_surface_move",           0x301),
        ("oscortex_surface_destroy",        0x302),
        ("oscortex_surface_upload_rgba32",  0x303),
        ("oscortex_surface_present",        0x304),
        ("oscortex_fb_size_packed",         0x305),
        ("oscortex_vsync_counter",          0x306),
        ("oscortex_vsync_wait_nonblock",    0x307),
        ("oscortex_surface_geometry_get",   0x30B),
        ("oscortex_surface_geometry_set",   0x30C),
        ("oscortex_surface_flip",           0x312),
        // WM
        ("oscortex_wm_event_poll",          0x320),
        ("oscortex_wm_event_read",          0x321),
        ("oscortex_wm_event_wait",          0x323),
        // Engine host
        ("oscortex_engine_host_register",   0x345),
        ("oscortex_engine_proctable_set",   0x356),
        ("oscortex_engine_vsync_baton_post",0x358),
        // Dynamic linker
        ("oscortex_dlopen",                 0x350),
        ("oscortex_dlsym",                  0x351),
        ("oscortex_dlclose",                0x352),
        ("oscortex_mmap",                   0x353),
        // GPU
        ("oscortex_gpu_submit",             0x359),
        ("oscortex_gpu_submit_strided",     0x364),
        // Platform channels
        ("oscortex_platform_msg_post",      0x360),
        ("oscortex_platform_msg_recv",      0x361),
        ("oscortex_platform_msg_reply",     0x362),
        ("oscortex_platform_msg_ack",       0x363),
        // Vsync rate
        ("oscortex_vsync_set_hz",           0x365),
        // Phase 34-C: AOT snapshot loader
        ("oscortex_aot_snapshot_load",       0x366),
        // Phase 35: Dart isolate lifecycle
        ("oscortex_isolate_spawn",           0x367),
        ("oscortex_isolate_kill",            0x368),
        ("oscortex_isolate_ctrl",            0x369),
        // Phase 36: Dart isolate message passing
        ("oscortex_isolate_msg_send",        0x36A),
        ("oscortex_isolate_msg_recv",        0x36B),
        ("oscortex_isolate_msg_pending",     0x36C),
        // Phase 37: PS/2 input device query
        ("oscortex_input_dev_count",         0x36D),
        ("oscortex_input_dev_info",          0x36E),
        // Phase 38: .osx app registry
        ("oscortex_app_install",             0x36F),
        ("oscortex_app_list",                0x370),
        ("oscortex_app_launch",              0x371),
        ("oscortex_app_uninstall",           0x372),
        // Phase 39: Named port IPC namespace
        ("oscortex_port_bind",               0x373),
        ("oscortex_port_lookup",             0x374),
        ("oscortex_port_unbind",             0x375),
        // Phase 41: USB host-controller query
        ("oscortex_usb_controller_count",    0x376),
        // Phase 42: userspace framebuffer map + WM event dequeue
        ("oscortex_fb_map",                  0x377),
        ("oscortex_wm_next_event",           0x378),
        // Phase 43: VFS query (no open-fd)
        ("oscortex_vfs_list",                0x379),
        ("oscortex_vfs_read",                0x37A),
        // Phase 44: writable ramdisk
        ("oscortex_vfs_write",               0x37B),
        ("oscortex_vfs_stat",                0x37C),
        // Phase 45: virtio-net networking
        ("oscortex_net_info",                0x37D),
        ("oscortex_net_send",                0x37E),
        ("oscortex_net_recv",                0x37F),
        // Phase 46: compositor bypass + full-screen surface
        ("oscortex_fb_release",              0x380),
        ("oscortex_surface_fullscreen",      0x381),
        // Phase 47: ELF exec round-trip
        ("oscortex_exec_wait",               0x39E),
        // Phase 48: UART serial console
        ("oscortex_serial_read",             0x383),
        ("oscortex_serial_write",            0x384),
        // Phase 49: virtio-blk
        ("oscortex_blk_info",                0x385),
        ("oscortex_blk_read",                0x386),
        ("oscortex_blk_write",               0x387),
        // Phase 50: smoltcp TCP/IP
        ("oscortex_tcp_connect",             0x388),
        ("oscortex_tcp_write",               0x389),
        ("oscortex_tcp_read",                0x38A),
        ("oscortex_tcp_close",               0x38B),
        ("oscortex_dhcp_discover",           0x38C),
        // Phase 51: ext2
        ("oscortex_ext2_mount",              0x38D),
        ("oscortex_ext2_ls",                 0x38E),
        ("oscortex_ext2_read",               0x38F),
        // Phase 53: preemptive scheduler
        ("oscortex_sched_yield",             0x390),
        ("oscortex_get_cpu_time",            0x391),
        // Phase 54: fork
        ("oscortex_fork",                    0x392),
        // Phase 55: signals
        ("oscortex_kill_signal",             0x393),
        ("oscortex_sigaction",               0x394),
        ("oscortex_sigreturn",               0x395),
        // Phase 60: pty/tty
        ("oscortex_pty_open",                0x396),
        ("oscortex_pty_read",                0x397),
        ("oscortex_pty_write",               0x398),
        ("oscortex_pty_ioctl",               0x399),
        // Phase 59: NVMe
        ("oscortex_nvme_info",               0x39A),
        ("oscortex_nvme_read",               0x39B),
        ("oscortex_nvme_write",              0x39C),
    ];

    let n = SYMS.len();

    // Each stub:
    //   49 89 CA          mov r10, rcx      (3 bytes)
    //   B8 xx xx xx xx    mov eax, imm32    (5 bytes)
    //   0F 05             syscall           (2 bytes)
    //   C3                ret               (1 byte)
    //   90 90 90 90 90    nop×5 padding     (5 bytes)
    // Total: 16 bytes per stub.
    const STUB_SZ: usize = 16;

    let text_off  = 0x00B0usize;
    let text_size = n * STUB_SZ;
    let dynsym_off = align8(text_off + text_size);
    let sym_ent   = 24usize;
    let n_sym_ent = n + 1;
    let dynsym_sz = n_sym_ent * sym_ent;
    let dynstr_off = dynsym_off + dynsym_sz;

    let mut dynstr = vec![0u8; 1];
    let mut name_offs: Vec<u32> = Vec::with_capacity(n);
    for &(sym, _) in SYMS {
        name_offs.push(dynstr.len() as u32);
        dynstr.extend_from_slice(sym.as_bytes());
        dynstr.push(0);
    }
    let dynstr_sz = dynstr.len();

    let dynamic_off = align8(dynstr_off + dynstr_sz);
    let n_dyn       = 5usize;
    let dynamic_sz  = n_dyn * 16;
    let file_sz     = dynamic_off + dynamic_sz;

    let mut elf = vec![0u8; file_sz];

    // ELF header
    elf[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
    elf[4] = 2; elf[5] = 1; elf[6] = 1; elf[7] = 0;
    write_u16(&mut elf, 16, 3);   // ET_DYN
    write_u16(&mut elf, 18, 62);  // EM_X86_64
    write_u32(&mut elf, 20, 1);   // e_version
    write_u64(&mut elf, 32, 64);  // e_phoff
    write_u16(&mut elf, 52, 64);  // e_ehsize
    write_u16(&mut elf, 54, 56);  // e_phentsize
    write_u16(&mut elf, 56, 2);   // e_phnum
    write_u16(&mut elf, 58, 64);  // e_shentsize

    // PT_LOAD
    let p0 = 64usize;
    write_u32(&mut elf, p0,      1);              // PT_LOAD
    write_u32(&mut elf, p0 + 4,  7);              // PF_R|PF_W|PF_X
    write_u64(&mut elf, p0 + 8,  0);
    write_u64(&mut elf, p0 + 16, 0);
    write_u64(&mut elf, p0 + 24, 0);
    write_u64(&mut elf, p0 + 32, file_sz as u64);
    write_u64(&mut elf, p0 + 40, file_sz as u64);
    write_u64(&mut elf, p0 + 48, 0x1000);

    // PT_DYNAMIC
    let p1 = 64 + 56;
    write_u32(&mut elf, p1,      2);
    write_u32(&mut elf, p1 + 4,  6);
    write_u64(&mut elf, p1 + 8,  dynamic_off as u64);
    write_u64(&mut elf, p1 + 16, dynamic_off as u64);
    write_u64(&mut elf, p1 + 24, dynamic_off as u64);
    write_u64(&mut elf, p1 + 32, dynamic_sz as u64);
    write_u64(&mut elf, p1 + 40, dynamic_sz as u64);
    write_u64(&mut elf, p1 + 48, 8);

    // .text: real syscall stubs
    for (i, &(_, nr)) in SYMS.iter().enumerate() {
        let off = text_off + i * STUB_SZ;
        // mov r10, rcx  (REX.WR + 89 /r: 4C 89 CA or use 49 89 CA)
        elf[off]     = 0x49;  // REX.WB
        elf[off + 1] = 0x89;  // MOV r/m64, r64
        elf[off + 2] = 0xCA;  // r10, rcx (ModRM: mod=11 reg=001(rcx) rm=010(r10))
        // mov eax, imm32
        elf[off + 3] = 0xB8;
        elf[off + 4] = (nr & 0xFF) as u8;
        elf[off + 5] = ((nr >> 8) & 0xFF) as u8;
        elf[off + 6] = ((nr >> 16) & 0xFF) as u8;
        elf[off + 7] = ((nr >> 24) & 0xFF) as u8;
        // syscall
        elf[off + 8]  = 0x0F;
        elf[off + 9]  = 0x05;
        // ret
        elf[off + 10] = 0xC3;
        // nop padding (bytes 11-15 already zero)
    }

    // .dynsym
    for i in 0..n {
        let entry_off = dynsym_off + (i + 1) * sym_ent;
        let st_value  = (text_off + i * STUB_SZ) as u64;
        write_u32_le(&mut elf, entry_off,      name_offs[i]);
        elf[entry_off + 4] = 0x12; // STB_GLOBAL | STT_FUNC
        elf[entry_off + 5] = 0;
        write_u16(&mut elf, entry_off + 6,  0xFFF1); // SHN_ABS
        write_u64(&mut elf, entry_off + 8,  st_value);
        write_u64(&mut elf, entry_off + 16, STUB_SZ as u64);
    }

    // .dynstr
    elf[dynstr_off..dynstr_off + dynstr_sz].copy_from_slice(&dynstr);

    // .dynamic
    write_dyn(&mut elf, dynamic_off,      6,  dynsym_off as u64); // DT_SYMTAB
    write_dyn(&mut elf, dynamic_off + 16, 5,  dynstr_off as u64); // DT_STRTAB
    write_dyn(&mut elf, dynamic_off + 32, 11, sym_ent as u64);     // DT_SYMENT
    write_dyn(&mut elf, dynamic_off + 48, 10, dynstr_sz as u64);   // DT_STRSZ
    write_dyn(&mut elf, dynamic_off + 64, 0,  0);                  // DT_NULL

    elf
}

// ── liboscortex_embedder.so platform-callback stub ELF (Phase 34-B) ──────────

/// Generate a minimal ELF64 ET_DYN for `liboscortex_embedder.so`.
///
/// This library exports the **platform-side callbacks** that a separately-
/// compiled Flutter engine binary would resolve by name at load time:
///
/// | Symbol                            | Role                                   |
/// |-----------------------------------|----------------------------------------|
/// | `oscortex_surface_present`        | software-renderer present callback     |
/// | `oscortex_vsync_callback`         | vsync baton request from engine        |
/// | `oscortex_platform_msg_callback`  | Dart → native platform channel         |
/// | `oscortex_log_callback`           | engine log output                      |
/// | `oscortex_task_post`              | Flutter task-runner post               |
/// | `oscortex_embedder_init`          | one-shot embedder initialisation       |
/// | `oscortex_get_display_size`       | returns (width<<32)|height             |
/// | `oscortex_embedder_version`       | returns packed ABI version (9)         |
///
/// Each stub is a 4-byte `xor eax,eax; ret; nop×2` — return-stub only.
/// The real implementations live in the embedder binary's static functions;
/// this `.so` is the stable symbol interface for future separately-compiled
/// engine binaries.
fn generate_liboscortex_embedder_shim() -> Vec<u8> {
    const SYMS: &[&str] = &[
        "oscortex_surface_present",
        "oscortex_vsync_callback",
        "oscortex_platform_msg_callback",
        "oscortex_log_callback",
        "oscortex_task_post",
        "oscortex_embedder_init",
        "oscortex_get_display_size",
        "oscortex_embedder_version",
    ];
    generate_ret_stub_elf(SYMS)
}

// ── aarch64 native `/init` EL0 program ───────────────────────────────────────

/// Build a minimal statically-linked AArch64 ELF64 (ET_EXEC, EM_AARCH64) that
/// runs at EL0 and issues syscalls through the OSCortex shared dispatcher.
///
/// Syscall ABI (OSCortex / x86-numbered, as understood by `dispatch_fast`):
///   nr in x8, args in x0..x2, return in x0, trap via `svc #0`.
///     1  = write(fd, buf, len)   → fd 1/2 print to the kernel serial console
///     39 = getpid()              → returns the process id
///     60 = exit(code)            → terminate the process
///
/// Layout: a single RWX PT_LOAD at vaddr 0x400000 (USER_ELF_BASE). Code starts
/// at the entry; two NUL-free messages live at fixed offsets in the same page.
fn generate_aarch64_init_elf() -> Vec<u8> {
    // Link the init program at 64 GiB (L1 index 64), well clear of the kernel's
    // low identity map. The bring-up kernel runs from the TTBR0 low half, so each
    // per-process root is seeded with the kernel's 1 GiB block descriptors for
    // the device region (index 0) and RAM (indices 1..2). A user page at a low VA
    // would collide with those blocks; placing the program at a high, otherwise-
    // empty L1 index lets the page-table walker create fresh L2/L3 tables there.
    const VBASE: u64 = 0x10_0000_0000; // 64 GiB

    // Messages placed after the code, at page offsets 0x200 / 0x280.
    let msg1: &[u8] = b"[arm-init] hello from EL0 userspace via svc #0\n";
    let msg2: &[u8] = b"[arm-init] getpid ok; calling exit(0)\n";
    let msg1_off: u64 = 0x200;
    let msg2_off: u64 = 0x280;
    let msg1_va = VBASE + msg1_off;
    let msg2_va = VBASE + msg2_off;

    // ── Assemble the code (offset 0) ────────────────────────────────────────
    let mut code: Vec<u32> = Vec::new();

    // write(1, msg1_va, msg1.len())
    a64_load_imm(&mut code, 0, 1);
    a64_load_imm(&mut code, 1, msg1_va);
    a64_load_imm(&mut code, 2, msg1.len() as u64);
    a64_load_imm(&mut code, 8, 1);
    code.push(0xD400_0001); // svc #0

    // getpid()
    a64_load_imm(&mut code, 8, 39);
    code.push(0xD400_0001);

    // write(1, msg2_va, msg2.len())
    a64_load_imm(&mut code, 0, 1);
    a64_load_imm(&mut code, 1, msg2_va);
    a64_load_imm(&mut code, 2, msg2.len() as u64);
    a64_load_imm(&mut code, 8, 1);
    code.push(0xD400_0001);

    // exit(0)
    a64_load_imm(&mut code, 0, 0);
    a64_load_imm(&mut code, 8, 60);
    code.push(0xD400_0001);

    // Safety net: spin if exit ever returns to EL0.
    code.push(0x1400_0000); // b .

    // ── Compose the single loadable page image ──────────────────────────────
    let page_sz = 0x1000usize;
    let mut img = vec![0u8; page_sz];
    for (i, w) in code.iter().enumerate() {
        let o = i * 4;
        img[o..o + 4].copy_from_slice(&w.to_le_bytes());
    }
    img[msg1_off as usize..msg1_off as usize + msg1.len()].copy_from_slice(msg1);
    img[msg2_off as usize..msg2_off as usize + msg2.len()].copy_from_slice(msg2);

    // ── ELF header + one program header ─────────────────────────────────────
    let ehsize = 64usize;
    let phentsize = 56usize;
    let phoff = ehsize;
    let data_off = phoff + phentsize; // file offset of the loadable image

    let file_sz = data_off + img.len();
    let mut elf = vec![0u8; file_sz];

    // e_ident
    elf[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
    elf[4] = 2; // ELFCLASS64
    elf[5] = 1; // ELFDATA2LSB
    elf[6] = 1; // EV_CURRENT
    write_u16(&mut elf, 16, 2);   // e_type = ET_EXEC
    write_u16(&mut elf, 18, 183); // e_machine = EM_AARCH64
    write_u32(&mut elf, 20, 1);   // e_version
    write_u64(&mut elf, 24, VBASE); // e_entry
    write_u64(&mut elf, 32, phoff as u64); // e_phoff
    write_u16(&mut elf, 52, ehsize as u16);    // e_ehsize
    write_u16(&mut elf, 54, phentsize as u16); // e_phentsize
    write_u16(&mut elf, 56, 1);                // e_phnum

    // PT_LOAD program header (RWX).
    let p = phoff;
    write_u32(&mut elf, p,      1); // p_type = PT_LOAD
    write_u32(&mut elf, p + 4,  7); // p_flags = R|W|X
    write_u64(&mut elf, p + 8,  data_off as u64); // p_offset
    write_u64(&mut elf, p + 16, VBASE);           // p_vaddr
    write_u64(&mut elf, p + 24, VBASE);           // p_paddr
    write_u64(&mut elf, p + 32, img.len() as u64); // p_filesz
    write_u64(&mut elf, p + 40, img.len() as u64); // p_memsz
    write_u64(&mut elf, p + 48, 0x1000);           // p_align

    elf[data_off..data_off + img.len()].copy_from_slice(&img);
    elf
}

/// Emit a MOVZ/MOVK sequence loading the 64-bit immediate `imm` into x`reg`.
fn a64_load_imm(code: &mut Vec<u32>, reg: u32, imm: u64) {
    let mut first = true;
    for hw in 0..4u32 {
        let imm16 = ((imm >> (hw * 16)) & 0xFFFF) as u32;
        if imm16 == 0 && !first {
            continue; // skip zero halfwords once at least one MOVZ emitted
        }
        let base = if first { 0xD280_0000 } else { 0xF280_0000 };
        code.push(base | (hw << 21) | (imm16 << 5) | reg);
        first = false;
    }
    if first {
        code.push(0xD280_0000 | reg); // imm == 0 → MOVZ Xreg, #0
    }
}

/// Generate a minimal ELF64 ET_DYN where every exported symbol is a
/// 4-byte `xor eax,eax; ret; nop; nop` stub.  Used for both
/// `libflutter_engine.so` (existing) and `liboscortex_embedder.so`.
fn generate_ret_stub_elf(syms: &[&str]) -> Vec<u8> {
    let n = syms.len();
    const STUB_SZ: usize = 4;

    let text_off   = 0x0080usize;
    let text_size  = n * STUB_SZ;
    let dynsym_off = align8(text_off + text_size);
    let sym_ent    = 24usize;
    let dynsym_sz  = (n + 1) * sym_ent;
    let dynstr_off = dynsym_off + dynsym_sz;

    let mut dynstr = vec![0u8; 1];
    let mut name_offs: Vec<u32> = Vec::with_capacity(n);
    for &sym in syms {
        name_offs.push(dynstr.len() as u32);
        dynstr.extend_from_slice(sym.as_bytes());
        dynstr.push(0);
    }
    let dynstr_sz  = dynstr.len();
    let dynamic_off = align8(dynstr_off + dynstr_sz);
    let dynamic_sz  = 5 * 16;
    let file_sz     = dynamic_off + dynamic_sz;

    let mut elf = vec![0u8; file_sz];

    // ELF header
    elf[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
    elf[4] = 2; elf[5] = 1; elf[6] = 1;
    write_u16(&mut elf, 16, 3);   // ET_DYN
    write_u16(&mut elf, 18, 62);  // EM_X86_64
    write_u32(&mut elf, 20, 1);
    write_u64(&mut elf, 32, 64);  // e_phoff
    write_u16(&mut elf, 52, 64);  // e_ehsize
    write_u16(&mut elf, 54, 56);  // e_phentsize
    write_u16(&mut elf, 56, 2);   // e_phnum

    // PT_LOAD
    let p0 = 64usize;
    write_u32(&mut elf, p0,      1); write_u32(&mut elf, p0+4,  7);
    write_u64(&mut elf, p0+8,   0); write_u64(&mut elf, p0+16, 0);
    write_u64(&mut elf, p0+24,  0); write_u64(&mut elf, p0+32, file_sz as u64);
    write_u64(&mut elf, p0+40, file_sz as u64); write_u64(&mut elf, p0+48, 0x1000);

    // PT_DYNAMIC
    let p1 = 120usize;
    write_u32(&mut elf, p1,      2); write_u32(&mut elf, p1+4,  6);
    write_u64(&mut elf, p1+8,   dynamic_off as u64);
    write_u64(&mut elf, p1+16,  dynamic_off as u64);
    write_u64(&mut elf, p1+24,  dynamic_off as u64);
    write_u64(&mut elf, p1+32,  dynamic_sz  as u64);
    write_u64(&mut elf, p1+40,  dynamic_sz  as u64);
    write_u64(&mut elf, p1+48,  8);

    // .text: xor eax,eax; ret; nop; nop
    for i in 0..n {
        let off = text_off + i * STUB_SZ;
        elf[off]   = 0x31; elf[off+1] = 0xC0; // xor eax,eax
        elf[off+2] = 0xC3;                     // ret
        elf[off+3] = 0x90;                     // nop
    }

    // .dynsym
    for i in 0..n {
        let eo = dynsym_off + (i + 1) * sym_ent;
        let va = (text_off + i * STUB_SZ) as u64;
        write_u32_le(&mut elf, eo, name_offs[i]);
        elf[eo + 4] = 0x12;
        write_u16(&mut elf, eo + 6, 0xFFF1);
        write_u64(&mut elf, eo + 8, va);
        write_u64(&mut elf, eo + 16, STUB_SZ as u64);
    }

    // .dynstr
    elf[dynstr_off..dynstr_off + dynstr_sz].copy_from_slice(&dynstr);

    // .dynamic
    write_dyn(&mut elf, dynamic_off,       6,  dynsym_off as u64);
    write_dyn(&mut elf, dynamic_off + 16,  5,  dynstr_off as u64);
    write_dyn(&mut elf, dynamic_off + 32,  11, sym_ent    as u64);
    write_dyn(&mut elf, dynamic_off + 48,  10, dynstr_sz  as u64);
    write_dyn(&mut elf, dynamic_off + 64,  0,  0);

    elf
}

// ── ELF writing helpers ───────────────────────────────────────────────────────

fn align8(x: usize) -> usize { (x + 7) & !7 }

fn write_u16(buf: &mut [u8], off: usize, v: u16) {
    buf[off..off + 2].copy_from_slice(&v.to_le_bytes());
}
fn write_u32(buf: &mut [u8], off: usize, v: u32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
fn write_u32_le(buf: &mut [u8], off: usize, v: u32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
fn write_u64(buf: &mut [u8], off: usize, v: u64) {
    buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
}
fn write_i64(buf: &mut [u8], off: usize, v: i64) {
    buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
}
/// Write a DT (dynamic-section tag+value) entry at `off`.
fn write_dyn(buf: &mut [u8], off: usize, tag: i64, val: u64) {
    write_i64(buf, off,     tag);
    write_u64(buf, off + 8, val);
}

