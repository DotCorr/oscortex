//! System call dispatch.
//!
//! Syscall ABI (x86_64):
//!   RAX = syscall number
//!   RDI = arg0, RSI = arg1, RDX = arg2, R10 = arg3, R8 = arg4, R9 = arg5
//!   Return value in RAX.
//!
//! Syscall numbers (Linux-compatible where possible):
//!   0  read        3  close      39 getpid
//!   1  write       57 fork       59 execve
//!   2  open        60 exit       61 wait4
//!   62 kill      231 exit_group
//!   0x1000–0x100F  Cortex PID-0 API
//!   0x200          ipc_send     0x201 ipc_recv
//!   0x300          surface_create
//!   0x301          surface_move
//!   0x302          surface_destroy
//!   0x303          surface_upload_rgba32
//!   0x304          surface_present
//!   0x305          fb_size_packed
//!   0x306          vsync_counter
//!   0x307          vsync_wait_nonblock
//!   0x308          surface_owner_pid
//!   0x309          surface_z_get
//!   0x30A          surface_z_set
//!   0x30B          surface_geometry_get
//!   0x30C          surface_geometry_set
//!   0x30D          surface_visibility_get
//!   0x30E          surface_visibility_set
//!   0x30F          surface_clip_set
//!   0x310          surface_damage_set
//!   0x311          surface_damage_get
//!   0x312          surface_flip
//!   0x320          wm_event_poll
//!   0x321          wm_event_read
//!   0x322          wm_event_inject
//!   0x323          wm_event_wait
//!   0x330          embedder_abi_version
//!   0x331          wm_event_size
//!   0x332          wm_event_stats_packed
//!   0x333          wm_focus_pid_get
//!   0x334          wm_focus_surface_set
//!   0x335          wm_focus_mirror_get
//!   0x336          wm_focus_mirror_set
//!   0x340          app_notify
//!   0x341          proc_surface_count
//!   0x342          app_launch_path
//!   0x343          engine_policy_get
//!   0x344          engine_version_packed
//!   0x345          engine_host_register
//!   0x346          engine_host_pid_get
//!   0x347          engine_library_path_read
//!   0x350          dlopen(path_ptr, path_len, flags) → handle
//!   0x351          dlsym(handle, name_ptr, name_len) → vaddr
//!   0x352          dlclose(handle) → 0
//!   0x353          mmap(hint_va, size, prot) → va
//!   0x354          munmap(va, size) → 0  (stub)
//!   0x355          mprotect(va, size, prot) → 0  (stub)
//!   0x366          aot_snapshot_load(path_ptr, path_len, out_va_ptr, out_size_ptr) → 0
//!   0x367          isolate_spawn(aot_va, aot_size, entry_offset, stack_size) → id
//!   0x368          isolate_kill(id) → 0
//!   0x369          isolate_ctrl(id, op) → state
//!   0x36A          isolate_msg_send(dst_id, data_ptr, data_len) → 0
//!   0x36B          isolate_msg_recv(isolate_id, buf_ptr, buf_len) → bytes
//!   0x36C          isolate_msg_pending(isolate_id) → count
//!   0x36D          input_dev_count() → n
//!   0x36E          input_dev_info(n) → packed_u32
//!   0x36F          app_install(bundle_ptr, bundle_len, id_out_ptr) → 0
//!   0x370          app_list(buf_ptr, buf_len) → count
//!   0x371          app_launch(app_id, flags) → isolate_id
//!   0x372          app_uninstall(app_id) → 0
//!   0x373          port_bind(name_ptr, name_len, isolate_id) → 0
//!   0x374          port_lookup(name_ptr, name_len, iso_out, pid_out) → 0
//!   0x375          port_unbind(name_ptr, name_len) → 0
//!   0x376          usb_controller_count() → n
//!   0x377          fb_map(info_out_ptr) → 0 / -ENXIO
//!   0x378          wm_next_event(event_out_ptr) → 0 / -EAGAIN
//!   0x379          vfs_list(path_ptr, path_len, buf_ptr, buf_len) → bytes
//!   0x37A          vfs_read(path_ptr, path_len, buf_ptr, buf_len) → bytes
//!   0x37B          vfs_write(path_ptr, path_len, data_ptr, data_len) → 0
//!   0x37C          vfs_stat(path_ptr, path_len, size_out_ptr) → 0 / -ENOENT
//!   0x37D          net_info(buf_ptr, buf_len) → bytes
//!   0x37E          net_send(dst_ip, dst_port, data_ptr, data_len) → 0
//!   0x37F          net_recv(buf_ptr, buf_len, src_ip_out, src_port_out) → bytes
//!   0x380          fb_release() → 0
//!   0x381          surface_fullscreen() → surface_id
//!   0xC0           sys_poweroff (ACPI S5)

use crate::embedder::abi as eabi;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const ENGINE_LIBRARY_PATH: &str = "/system/lib/libflutter_engine.so";
static ENGINE_HOST_PID: AtomicU32 = AtomicU32::new(0);
/// VA of the `FlutterEngineProcTable` registered by the engine host.
static ENGINE_PROC_TABLE_PTR: AtomicU64 = AtomicU64::new(0);
static FS_BOOTSTRAP_LOGGED: AtomicU32 = AtomicU32::new(0);

const SYSCALL_TRACE_DEPTH: usize = 64;

#[derive(Clone, Copy)]
struct SyscallTraceEntry {
    nr: u64,
    a0: u64,
    a1: u64,
    a2: u64,
    rip: u64,
}

static SYSCALL_TRACE_BUF: spin::Mutex<[SyscallTraceEntry; SYSCALL_TRACE_DEPTH]> = spin::Mutex::new(
    [const { SyscallTraceEntry { nr: 0, a0: 0, a1: 0, a2: 0, rip: 0 } }; SYSCALL_TRACE_DEPTH]
);
static SYSCALL_TRACE_HEAD: AtomicU32 = AtomicU32::new(0);
static FUTEX_WAITERS: spin::Mutex<BTreeMap<u64, Vec<u32>>> = spin::Mutex::new(BTreeMap::new());

#[inline]
fn record_syscall_trace(nr: u64, a0: u64, a1: u64, a2: u64, rip: u64) {
    let idx = (SYSCALL_TRACE_HEAD.fetch_add(1, Ordering::Relaxed) as usize) % SYSCALL_TRACE_DEPTH;
    let mut buf = SYSCALL_TRACE_BUF.lock();
    buf[idx] = SyscallTraceEntry { nr, a0, a1, a2, rip };
}

pub fn dump_recent_syscalls(limit: usize) {
    let depth = SYSCALL_TRACE_DEPTH;
    let count = limit.min(depth);
    let head = SYSCALL_TRACE_HEAD.load(Ordering::Relaxed) as usize;
    let buf = SYSCALL_TRACE_BUF.lock();

    log::error!("[syscall-trace] last {} syscall(s):", count);
    for i in 0..count {
        let idx = (head + depth - 1 - i) % depth;
        let e = buf[idx];
        if e.nr == 0 && e.rip == 0 {
            continue;
        }
        log::error!(
            "[syscall-trace] #{:02} nr={:#x} a0={:#x} a1={:#x} a2={:#x} rip={:#x}",
            i,
            e.nr,
            e.a0,
            e.a1,
            e.a2,
            e.rip
        );
    }
}

pub fn init() {}

// ── Open-file table ───────────────────────────────────────────────────────────
//
// Simple global open-file table (max 64 entries).  No per-process isolation —
// acceptable for a single-user kernel where one process runs at a time.
// FDs 0..2 = stdin/stdout/stderr (hardwired).  FD 3+ = VFS files.

struct OpenFile {
    data:   &'static [u8],
    offset: u64,
    used:   bool,
}

const MAX_OPEN_FILES: usize = 64;
static OPEN_FILES: spin::Mutex<[OpenFile; MAX_OPEN_FILES]> = spin::Mutex::new(
    [const { OpenFile { data: &[], offset: 0, used: false } }; MAX_OPEN_FILES]
);

/// Allocate an fd (≥3) backed by `data` from the VFS.
fn fd_alloc(data: &'static [u8]) -> Option<u64> {
    let mut tbl = OPEN_FILES.lock();
    for (i, slot) in tbl.iter_mut().enumerate().skip(3) {
        if !slot.used {
            *slot = OpenFile { data, offset: 0, used: true };
            return Some(i as u64);
        }
    }
    None
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Copy a byte slice from a user virtual address (minimal validation).
/// In a real kernel this would use access_ok + copy_from_user.
/// Here we accept the pointer directly as kernel == user space pre-M13.
unsafe fn read_user_bytes(ptr: u64, len: usize) -> Option<&'static [u8]> {
    if ptr == 0 || len > 0x20_0000 { return None; }
    Some(unsafe { core::slice::from_raw_parts(ptr as *const u8, len) })
}

unsafe fn write_user_bytes(ptr: u64, src: &[u8]) -> bool {
    if ptr == 0 || src.len() > 0x20_0000 {
        return false;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(src.as_ptr(), ptr as *mut u8, src.len());
    }
    true
}

// ── Syscall implementations ───────────────────────────────────────────────────

fn sys_write(fd: u64, buf_ptr: u64, len: u64) -> i64 {
    if fd != 1 && fd != 2 { return -9; } // EBADF
    let bytes = match unsafe { read_user_bytes(buf_ptr, len as usize) } {
        Some(b) => b,
        None => {
            log::warn!("[sys_write] EFAULT fd={} ptr={:#x} len={}", fd, buf_ptr, len);
            return -14; // EFAULT
        }
    };
    if let Ok(s) = core::str::from_utf8(bytes) {
        crate::logger::early_print(s);
    }
    len as i64
}

fn sys_read(fd: u64, buf_ptr: u64, len: u64) -> i64 {
    if len == 0 { return 0; }
    if buf_ptr == 0 { return -14; } // EFAULT
    let to_read = len as usize;
    let mut tbl = OPEN_FILES.lock();
    let idx = fd as usize;
    if idx >= MAX_OPEN_FILES || !tbl[idx].used {
        return -9; // EBADF
    }
    let offset = tbl[idx].offset as usize;
    let data   = tbl[idx].data;
    if offset >= data.len() { return 0; } // EOF
    let available = data.len() - offset;
    let n = to_read.min(available);
    unsafe {
        core::ptr::copy_nonoverlapping(
            data.as_ptr().add(offset),
            buf_ptr as *mut u8,
            n,
        );
    }
    tbl[idx].offset += n as u64;
    n as i64
}

fn sys_open(path_ptr: u64, path_len: u64, _flags: u64) -> i64 {
    let bytes = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    let path = match core::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return -22, // EINVAL
    };
    // Strip null terminator if present.
    let path = path.trim_end_matches('\0');
    match crate::fs::lookup(path) {
        Some(data) => {
            match fd_alloc(data) {
                Some(fd) => {
                    log::debug!("[sys_open] '{}' → fd={} ({} bytes)", path, fd, data.len());
                    fd as i64
                }
                None => -23, // ENFILE (too many open files)
            }
        }
        None => {
            log::debug!("[sys_open] '{}' → ENOENT", path);
            -2 // ENOENT
        }
    }
}

fn sys_close(fd: u64) -> i64 {
    let idx = fd as usize;
    if idx < 3 || idx >= MAX_OPEN_FILES { return 0; }
    let mut tbl = OPEN_FILES.lock();
    if tbl[idx].used {
        tbl[idx].used = false;
        tbl[idx].data = &[];
        tbl[idx].offset = 0;
    }
    0
}

fn sys_lseek(fd: u64, offset: i64, whence: u64) -> i64 {
    let idx = fd as usize;
    if idx < 3 || idx >= MAX_OPEN_FILES { return -9; } // EBADF
    let mut tbl = OPEN_FILES.lock();
    if !tbl[idx].used { return -9; }
    let size = tbl[idx].data.len() as i64;
    let new_off: i64 = match whence {
        0 => offset,                                    // SEEK_SET
        1 => tbl[idx].offset as i64 + offset,           // SEEK_CUR
        2 => size + offset,                             // SEEK_END
        _ => return -22, // EINVAL
    };
    if new_off < 0 { return -22; }
    tbl[idx].offset = new_off as u64;
    new_off
}

fn sys_fstat(fd: u64, stat_ptr: u64) -> i64 {
    // Minimal stat: only st_size matters for the Flutter engine.
    let idx = fd as usize;
    let size: u64 = if idx < 3 {
        0
    } else {
        let tbl = OPEN_FILES.lock();
        if !tbl[idx].used { return -9; }
        tbl[idx].data.len() as u64
    };
    if stat_ptr != 0 {
        // Write a minimal stat64 struct — st_size is at offset 48 in Linux stat64.
        unsafe {
            let p = stat_ptr as *mut u8;
            // Zero out 144 bytes (sizeof(struct stat64))
            core::ptr::write_bytes(p, 0, 144);
            // st_size at offset 48
            (p.add(48) as *mut u64).write_unaligned(size);
            // st_blksize at offset 56 = 4096
            (p.add(56) as *mut u64).write_unaligned(4096);
            // st_blocks at offset 64 = (size+511)/512
            (p.add(64) as *mut u64).write_unaligned((size + 511) / 512);
            // st_mode at offset 24 = 0100644 (regular file)
            (p.add(24) as *mut u32).write_unaligned(0o100644);
        }
    }
    0
}

fn sys_getpid() -> i64 {
    let pid = crate::process::current_pid();
    if pid == 0 { 1 } else { pid as i64 }
}

/// Linux `stat(const char *path, struct stat *buf)` — path is NUL-terminated.
/// Used by glibc `stat()`/`lstat()` and consequently by Flutter's
/// `fml::IsFile()` to verify `kernel_blob.bin` and similar assets exist.
fn sys_stat_path(path_ptr: u64, stat_ptr: u64) -> i64 {
    if path_ptr == 0 { return -14; } // EFAULT
    // Determine NUL-terminated length (bounded probe).
    let mut len: usize = 0;
    unsafe {
        let p = path_ptr as *const u8;
        while len < 4096 && *p.add(len) != 0 { len += 1; }
    }
    let fd = sys_open(path_ptr, len as u64, 0);
    if fd < 0 { return fd; }
    let r = sys_fstat(fd as u64, stat_ptr);
    let _ = sys_close(fd as u64);
    r
}

/// Linux `access(const char *path, int mode)` — returns 0 if file exists.
fn sys_access_path(path_ptr: u64, _mode: u64) -> i64 {
    if path_ptr == 0 { return -14; }
    let mut len: usize = 0;
    unsafe {
        let p = path_ptr as *const u8;
        while len < 4096 && *p.add(len) != 0 { len += 1; }
    }
    let fd = sys_open(path_ptr, len as u64, 0);
    if fd < 0 { return fd; }
    let _ = sys_close(fd as u64);
    0
}

/// Linux `newfstatat(int dirfd, const char *path, struct stat *buf, int flags)`.
/// We ignore `dirfd` (treat path as absolute) and delegate to `sys_stat_path`.
fn sys_newfstatat(_dirfd: u64, path_ptr: u64, stat_ptr: u64, _flags: u64) -> i64 {
    sys_stat_path(path_ptr, stat_ptr)
}

fn sys_exit(code: u64) -> i64 {
    let pid = crate::process::current_pid();
    if pid != 0 {
        // Read parent BEFORE exit() so we can wake them after.
        let parent_pid = crate::process::get_parent_of(pid);
        crate::process::exit(pid, code as i32);

        if let Some(ppid) = parent_pid {
            // Parent was blocking in sys_exec_wait — wake it with exit code.
            crate::process::reap_zombie(pid);
            crate::process::set_rax(ppid, code);
            crate::process::set_state(ppid, crate::process::ProcState::Running);
            crate::process::set_current_pid(ppid);
            log::warn!("[trace] sys_exit: pid={} → wake parent ppid={} code={:#x}", pid, ppid, code);
            crate::process::enter_user_by_pid_noreturn(ppid);
        }
        log::warn!("[trace] sys_exit: pid={} no parent — falling through to scheduler", pid);

        if let Some(next_pid) = crate::process::next_runnable_pid(0) {
            crate::process::enter_user_by_pid_noreturn(next_pid);
        }
    }
    // No runnable user process left.
    loop {
        unsafe { core::arch::asm!("sti; hlt; cli", options(nomem, nostack)); }
    }
}

fn sys_waitpid(pid: u64, _status_ptr: u64, _options: u64) -> i64 {
    match crate::process::waitpid(pid as u32) {
        Ok(code) => code as i64,
        Err("not exited") => -11,      // EAGAIN
        Err("no such process") => -10, // ECHILD
        Err(_) => -22,                  // EINVAL
    }
}

fn sys_kill(pid: u64, sig: u64) -> i64 {
    if sig != 9 {
        return -22; // EINVAL (only SIGKILL wired today)
    }
    match crate::process::kill(pid as u32) {
        Ok(()) => 0,
        Err(_) => -3, // ESRCH
    }
}

fn sys_arch_prctl(code: u64, addr: u64) -> i64 {
    // x86_64 Linux arch_prctl operations used by libc TLS setup.
    const ARCH_SET_FS: u64 = 0x1002;
    const ARCH_GET_FS: u64 = 0x1003;

    match code {
        ARCH_SET_FS => {
            crate::arch::cpu::set_fs_base(addr);
            let pid = crate::process::current_pid();
            if pid != 0 {
                crate::process::set_proc_fs_base(pid, addr);
            }
            0
        }
        ARCH_GET_FS => {
            if addr == 0 {
                return -14; // EFAULT
            }
            let fs = crate::arch::cpu::get_fs_base();
            unsafe { *(addr as *mut u64) = fs; }
            0
        }
        _ => -22, // EINVAL
    }
}

fn sys_exec(path_ptr: u64, path_len: u64) -> i64 {
    let bytes = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b,
        None => return -14,
    };
    let path = match core::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return -22,
    };
    let elf = match crate::fs::lookup(path) {
        Some(data) => data,
        None => return -2, // ENOENT
    };
    match crate::process::spawn(elf, path) {
        Ok(pid) => pid as i64,
        Err(_)  => -12, // ENOMEM
    }
}

/// Phase 47 — SYS_EXEC_WAIT (0x382): spawn a child, block the calling
/// process, context-switch to the child immediately, and return the child's
/// exit code when it calls sys_exit.
///
/// This is a cooperative "exec-and-wait" that works without a preemptive
/// scheduler: the parent blocks, the child runs to completion, then sys_exit
/// restores the parent with RAX = exit_code.
fn sys_exec_wait(path_ptr: u64, path_len: u64) -> i64 {
    let bytes = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b,
        None    => return -14, // EFAULT
    };
    let path = match core::str::from_utf8(bytes) {
        Ok(s)  => s,
        Err(_) => return -22, // EINVAL
    };
    let elf = match crate::fs::lookup(path) {
        Some(data) => data,
        None       => return -2, // ENOENT
    };
    let parent_pid = crate::process::current_pid();
    if parent_pid == 0 { return -1; } // must have an active user process

    let child_pid = match crate::process::spawn(elf, path) {
        Ok(pid) => pid,
        Err(_)  => return -12, // ENOMEM
    };

    // Wire the child back to the parent so sys_exit knows who to wake.
    crate::process::set_child_parent(child_pid, parent_pid);

    // Save the parent's return context (RIP = next instr after syscall, RSP).
    let parent_rip = crate::arch::syscall::user_rip();
    let parent_rsp = crate::arch::syscall::user_rsp();
    crate::process::save_return_context(parent_pid, parent_rip, parent_rsp);

    // Block the parent — it will be unblocked by the child's sys_exit.
    crate::process::set_state(parent_pid, crate::process::ProcState::Blocked);

    // Switch the kernel's notion of current PID and SYSRET into the child.
    crate::process::enter_user_by_pid_noreturn(child_pid)
}

fn sys_poweroff() -> i64 {
    log::info!("[syscall] sys_poweroff requested — triggering ACPI S5 shutdown");
    #[cfg(target_arch = "x86_64")]
    crate::arch::acpi_shutdown();
    #[cfg(not(target_arch = "x86_64"))]
    loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)); } }
}

fn sys_ipc_send(dst_pid: u64, msg_ptr: u64, msg_len: u64) -> i64 {
    let bytes = match unsafe { read_user_bytes(msg_ptr, (msg_len as usize).min(64)) } {
        Some(b) => b,
        None => return -14,
    };
    crate::ipc::send(dst_pid as u32, bytes);
    0
}

fn sys_ipc_recv(buf_ptr: u64, buf_len: u64) -> i64 {
    let pid = crate::process::current_pid();
    if pid == 0 {
        return -11; // no active userspace pid yet
    }
    match crate::ipc::recv(pid) {
        Some(msg) => {
            let copy_len = (buf_len as usize).min(msg.len());
            if buf_ptr == 0 { return -14; }
            unsafe {
                core::ptr::copy_nonoverlapping(
                    msg.as_ptr(),
                    buf_ptr as *mut u8,
                    copy_len,
                );
            }
            copy_len as i64
        }
        None => -11, // EAGAIN
    }
}

fn sys_surface_create(width: u64, height: u64) -> i64 {
    match crate::compositor::create_surface_for(wm_consumer_pid(), width as u32, height as u32) {
        Ok(id) => id as i64,
        Err("invalid size") => -22, // EINVAL
        Err(_) => -12,               // ENOMEM/table full
    }
}

fn sys_surface_move(id: u64, packed_xy: u64, z: u64) -> i64 {
    let x = ((packed_xy >> 32) as u32) as i32;
    let y = (packed_xy as u32) as i32;
    match crate::compositor::move_surface_for(wm_consumer_pid(), id as u32, x, y, z as i32) {
        Ok(()) => 0,
        Err("permission denied") => -1, // EPERM
        Err(_) => -3, // ESRCH
    }
}

fn sys_surface_destroy(id: u64) -> i64 {
    match crate::compositor::destroy_surface_for(wm_consumer_pid(), id as u32) {
        Ok(()) => 0,
        Err("permission denied") => -1, // EPERM
        Err(_) => -3, // ESRCH
    }
}

fn sys_surface_upload(id: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    let bytes = match unsafe { read_user_bytes(buf_ptr, buf_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    match crate::compositor::upload_surface_rgba32_for(wm_consumer_pid(), id as u32, bytes) {
        Ok(()) => 0,
        Err("bad payload size") => -22,
        Err("permission denied") => -1,
        Err("no such surface") => -3,
        Err(_) => -12,
    }
}

fn sys_surface_present(id: u64) -> i64 {
    match crate::compositor::present_surface_for(wm_consumer_pid(), id as u32) {
        Ok(()) => 0,
        Err("permission denied") => -1,
        Err(_) => -3,
    }
}

fn sys_fb_size_packed() -> i64 {
    crate::compositor::framebuffer_size_packed() as i64
}

fn sys_vsync_counter() -> i64 {
    crate::compositor::frame_counter() as i64
}

fn sys_vsync_wait_nonblock(last_seen: u64) -> i64 {
    match crate::compositor::wait_vsync(last_seen) {
        Some(v) => v as i64,
        None => -11, // EAGAIN
    }
}

fn sys_surface_owner(id: u64) -> i64 {
    match crate::compositor::surface_owner(id as u32) {
        Some(pid) => pid as i64,
        None => -3, // ESRCH
    }
}

fn sys_surface_z_get(id: u64) -> i64 {
    match crate::compositor::surface_z_get(id as u32) {
        Some(z) => z as i64,
        None => -3, // ESRCH
    }
}

fn sys_surface_z_set(id: u64, z: u64) -> i64 {
    match crate::compositor::surface_z_set_for(wm_consumer_pid(), id as u32, z as i32) {
        Ok(()) => 0,
        Err("permission denied") => -1, // EPERM
        Err(_) => -3, // ESRCH
    }
}

fn sys_surface_geometry_get(id: u64) -> i64 {
    match crate::compositor::surface_geometry_get(id as u32) {
        Some((x, y, w, h)) => {
            // Pack as: a[31:0]=x, a[63:32]=y; b[31:0]=w, b[63:32]=h
            // Return as two i64 halves combined: ((y as u64) << 32) | (x as u32 as u64)
            // Actually, we need to return packed geometry. Use a convention:
            // RAX returns lower 64 bits: ((x as u32 as u64) << 32) | (y as u32 as u64)
            // This is the coordinate pair.
            let x_packed = ((x as u32 as u64) << 32) | (y as u32 as u64);
            x_packed as i64
        }
        None => -3, // ESRCH
    }
}

fn sys_surface_geometry_set(id: u64, xy_packed: u64, wh_packed: u64) -> i64 {
    let x = ((xy_packed >> 32) as u32) as i32;
    let y = (xy_packed as u32) as i32;
    let w = ((wh_packed >> 32) as u32) as u32;
    let h = (wh_packed as u32) as u32;
    
    match crate::compositor::surface_geometry_set_for(wm_consumer_pid(), id as u32, x, y, w, h) {
        Ok(()) => 0,
        Err("invalid size") => -22, // EINVAL
        Err("surface too large") => -12, // ENOMEM
        Err("permission denied") => -1, // EPERM
        Err(_) => -3, // ESRCH
    }
}

fn sys_surface_visibility_get(id: u64) -> i64 {
    match crate::compositor::surface_visibility_get(id as u32) {
        Some(visible) => if visible { 1 } else { 0 },
        None => -3, // ESRCH
    }
}

fn sys_surface_visibility_set(id: u64, visible: u64) -> i64 {
    match crate::compositor::surface_visibility_set_for(wm_consumer_pid(), id as u32, visible != 0) {
        Ok(()) => 0,
        Err("permission denied") => -1,
        Err(_) => -3,
    }
}

fn sys_surface_clip_set(id: u64, xy_packed: u64, wh_packed: u64) -> i64 {
    let x = ((xy_packed >> 32) as u32) as i32;
    let y = (xy_packed as u32) as i32;
    let w = ((wh_packed >> 32) as u32) as u32;
    let h = (wh_packed as u32) as u32;
    
    match crate::compositor::surface_clip_set_for(wm_consumer_pid(), id as u32, x, y, w, h) {
        Ok(()) => 0,
        Err("permission denied") => -1,
        Err(_) => -3,
    }
}

fn sys_surface_damage_set(id: u64, xy_packed: u64, wh_packed: u64) -> i64 {
    let x = ((xy_packed >> 32) as u32) as i32;
    let y = (xy_packed as u32) as i32;
    let w = ((wh_packed >> 32) as u32) as u32;
    let h = (wh_packed as u32) as u32;
    match crate::compositor::surface_damage_set_for(wm_consumer_pid(), id as u32, x, y, w, h) {
        Ok(()) => 0,
        Err("permission denied") => -1,
        Err(_) => -3,
    }
}

fn sys_surface_damage_get(id: u64) -> i64 {
    match crate::compositor::surface_damage_get(id as u32) {
        Some((x, y, _w, _h, true)) => {
            // Return packed xy; caller uses separate surface_geometry syscall for dimensions.
            ((x as u32 as u64) << 32 | y as u32 as u64) as i64
        }
        Some((_, _, _, _, false)) => -11, // EAGAIN — no pending damage
        None => -3,                        // ESRCH
    }
}

fn sys_surface_flip(id: u64) -> i64 {
    match crate::compositor::surface_flip_for(wm_consumer_pid(), id as u32) {
        Ok(()) => 0,
        Err("permission denied") => -1,
        Err(_) => -3,
    }
}

fn sys_app_notify(target_pid: u64, subkind: u64, surface_id: u64) -> i64 {
    crate::wm::push_app_event(target_pid as u32, subkind as u32, surface_id as u32);
    0
}

fn sys_proc_surface_count(pid: u64) -> i64 {
    crate::compositor::surface_count_for(pid as u32) as i64
}

fn sys_app_launch_path(path_ptr: u64, path_len: u64, _flags: u64) -> i64 {
    let bytes = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b,
        None => return -14,
    };
    let path = match core::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return -22,
    };
    let elf = match crate::fs::lookup(path) {
        Some(data) => data,
        None => return -2,
    };
    match crate::process::spawn(elf, path) {
        Ok(pid) => {
            crate::wm::push_app_event(pid, eabi::APP_LAUNCH, 0);
            let host_pid = ENGINE_HOST_PID.load(Ordering::Acquire);
            if host_pid != 0 {
                // Notify the runtime host about the launched app PID.
                crate::wm::push_event_for(host_pid, eabi::EV_APP, eabi::APP_LAUNCH, pid as u64, 0);
            }
            pid as i64
        }
        Err(_) => -12,
    }
}

fn sys_engine_policy_get() -> i64 {
    // packed: [63:32]=engine target family, [31:0]=loader strategy
    let packed = ((eabi::ENGINE_TARGET_FLUTTER_3_29 as u64) << 32)
        | (eabi::ENGINE_LOADER_DYNAMIC as u64);
    packed as i64
}

fn sys_engine_version_packed() -> i64 {
    // Flutter 3.29.0 baseline.
    let packed = (3u64 << 32) | (29u64 << 16);
    packed as i64
}

fn sys_engine_host_register(_flags: u64) -> i64 {
    let pid = crate::process::current_pid();
    if pid == 0 {
        return -1; // EPERM (kernel context cannot be engine host)
    }
    ENGINE_HOST_PID.store(pid, Ordering::Release);
    pid as i64
}

fn sys_engine_host_pid_get() -> i64 {
    ENGINE_HOST_PID.load(Ordering::Acquire) as i64
}

fn sys_engine_library_path_read(dst_ptr: u64, dst_len: u64) -> i64 {
    let bytes = ENGINE_LIBRARY_PATH.as_bytes();
    if (dst_len as usize) < bytes.len() {
        return -22; // EINVAL
    }
    if unsafe { write_user_bytes(dst_ptr, bytes) } {
        bytes.len() as i64
    } else {
        -14 // EFAULT
    }
}

#[inline(always)]
fn wm_consumer_pid() -> u32 {
    let pid = crate::process::current_pid();
    if pid == 0 { 1 } else { pid }
}

fn sys_wm_event_poll() -> i64 {
    crate::wm::pending_count_for(wm_consumer_pid()) as i64
}

fn sys_wm_event_read(ev_ptr: u64, ev_len: u64) -> i64 {
    let need = crate::wm::event_size();
    if (ev_len as usize) < need {
        return -22; // EINVAL
    }

    let ev = match crate::wm::pop_event_for(wm_consumer_pid()) {
        Some(e) => e,
        None => return -11, // EAGAIN
    };

    let bytes = unsafe {
        core::slice::from_raw_parts(
            (&ev as *const crate::wm::WmEvent) as *const u8,
            need,
        )
    };
    if unsafe { write_user_bytes(ev_ptr, bytes) } {
        need as i64
    } else {
        -14 // EFAULT
    }
}

fn sys_wm_event_inject(kind: u64, arg1: u64, arg2: u64) -> i64 {
    let caller = wm_consumer_pid();
    match kind as u32 {
        eabi::EV_POINTER => {
            let x = ((arg1 >> 32) as u32) as i32;
            let y = (arg1 as u32) as i32;
            crate::wm::push_pointer(x, y, arg2 as u32);
            0
        }
        eabi::EV_KEY => {
            crate::wm::push_key(arg1 as u32, arg2 != 0);
            0
        }
        eabi::EV_APP => {
            crate::wm::push_event_for(caller, kind as u32, 0, arg1, arg2);
            0
        }
        eabi::EV_VSYNC => {
            crate::wm::push_event(kind as u32, 0, arg1, arg2);
            0
        }
        _ => -22,
    }
}

fn sys_embedder_abi_version() -> i64 {
    eabi::EMBEDDER_ABI_VERSION as i64
}

fn sys_wm_event_size() -> i64 {
    eabi::WM_EVENT_SIZE as i64
}

fn sys_wm_event_stats_packed() -> i64 {
    let pending = crate::wm::pending_count_for(wm_consumer_pid()).min(u32::MAX as usize) as u64;
    let dropped = crate::wm::dropped_count().min(u32::MAX as u64);
    // packed: (dropped << 32) | pending
    ((dropped << 32) | pending) as i64
}

fn sys_wm_focus_pid_get() -> i64 {
    crate::wm::focus_pid() as i64
}

fn sys_wm_focus_surface_set(surface_id: u64) -> i64 {
    let caller = wm_consumer_pid();
    let owner = match crate::compositor::surface_owner(surface_id as u32) {
        Some(pid) => pid,
        None => return -3, // ESRCH
    };
    if owner != caller {
        return -1; // EPERM
    }
    crate::wm::set_focus_pid(owner);
    owner as i64
}

fn sys_wm_focus_mirror_get() -> i64 {
    if crate::wm::focus_mirror_enabled() { 1 } else { 0 }
}

fn sys_wm_focus_mirror_set(enabled: u64) -> i64 {
    let on = enabled != 0;
    crate::wm::set_focus_mirror_enabled(on);
    if on { 1 } else { 0 }
}

fn sys_wm_event_wait(ev_ptr: u64, ev_len: u64, max_halts: u64) -> i64 {
    // Fast path first.
    let n = sys_wm_event_read(ev_ptr, ev_len);
    if n >= 0 {
        return n;
    }

    // Bounded blocking wait: sleep on interrupts to avoid busy loops.
    let loops = (max_halts as usize).min(64).max(1);
    for _ in 0..loops {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            core::arch::asm!("sti; hlt; cli", options(nomem, nostack));
        }
        let n2 = sys_wm_event_read(ev_ptr, ev_len);
        if n2 >= 0 {
            return n2;
        }
    }
    -11 // EAGAIN
}

// ── Phase 30 Slice 3: kernel dynamic linker + anonymous mmap ─────────────────

fn sys_dlopen(path_ptr: u64, path_len: u64, _flags: u64) -> i64 {
    let bytes = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    let path = match core::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return -22, // EINVAL
    };
    let pid = crate::process::current_pid();
    let pml4_phys = match crate::process::get_user_context(pid) {
        Some(ctx) => ctx.pml4_phys,
        None => return -9, // EBADF
    };
    // Serve libflutter_engine.so from the Limine module if available.
    let engine_guard;
    let elf: &[u8] = if path.contains("libflutter_engine") {
        engine_guard = crate::FLUTTER_ENGINE_BYTES.lock();
        if let Some(data) = engine_guard.as_ref() {
            log::info!("[dlopen] Serving libflutter_engine.so from Limine module ({} bytes)", data.len());
            data
        } else {
            log::warn!("[dlopen] libflutter_engine.so requested but not available as Limine module");
            return -2; // ENOENT
        }
    } else {
        drop({
            // Need to satisfy borrow checker — create a temporary guard that we immediately drop.
            // We don't actually use it here.
            let _unused = crate::FLUTTER_ENGINE_BYTES.lock();
            _unused
        });
        match crate::fs::lookup(path) {
            Some(data) => data,
            None => return -2, // ENOENT
        }
    };
    match crate::process::dl::dlopen(pid, pml4_phys, elf) {
        Ok(h)  => h as i64,
        Err(e) => {
            log::warn!("[syscall] dlopen '{}' failed: {}", path, e);
            -12 // ENOMEM / generic load failure
        }
    }
}

fn sys_dlsym(handle: u64, name_ptr: u64, name_len: u64) -> i64 {
    let name = match unsafe { read_user_bytes(name_ptr, name_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    match crate::process::dl::dlsym(handle as u32, name) {
        Some(addr) => addr as i64,
        None       => 0, // POSIX: NULL (0) means not found
    }
}

fn sys_dlclose(handle: u64) -> i64 {
    let pid = crate::process::current_pid();
    crate::process::dl::dlclose(handle as u32, pid);
    0
}

/// Return init function info for a dynamically loaded library so the embedder
/// can call C++ constructors from user space.
///
/// Writes to the three out-pointers (any of which may be null to skip):
///   `*out_init_fn`   ← DT_INIT function VA  (0 if none)
///   `*out_array_va`  ← VA of first DT_INIT_ARRAY entry  (0 if none)
///   `*out_count`     ← number of entries in the array  (0 if none)
fn sys_dl_get_init_array(handle: u64, out_init_fn: u64, out_array_va: u64, out_count: u64) -> i64 {
    let pid = crate::process::current_pid();
    match crate::process::dl::get_init_fns(handle as u32, pid) {
        Some((init_fn, array_va, count)) => {
            // SAFETY: pointers are in user address space while CR3 = user PML4.
            unsafe {
                if out_init_fn  != 0 { core::ptr::write_unaligned(out_init_fn  as *mut u64, init_fn);       }
                if out_array_va != 0 { core::ptr::write_unaligned(out_array_va as *mut u64, array_va);      }
                if out_count    != 0 { core::ptr::write_unaligned(out_count    as *mut u64, count as u64);  }
            }
            0
        }
        None => -9, // EBADF — unknown handle
    }
}

fn sys_mmap(hint_va: u64, size: u64, prot: u64) -> i64 {
    if size == 0 || size > 0x1000_0000 { return -22; } // EINVAL, max 256 MiB
    let pid = crate::process::current_pid();
    if pid == 0 { return -1; } // EPERM
    let pml4_phys = match crate::process::get_user_context(pid) {
        Some(ctx) => ctx.pml4_phys,
        None => return -9, // EBADF
    };
    let pages = ((size as usize) + 4095) / 4096;
    let va = crate::process::dl::mmap_anon(pid, pml4_phys, hint_va, pages, prot);
    if va == u64::MAX { -12 } else { va as i64 } // ENOMEM or success
}

fn sys_munmap(_va: u64, _size: u64) -> i64 {
    // Stub — frames remain mapped until process exit.
    // Full unmapping requires reverse page-table mappings (future milestone).
    0
}

fn sys_mprotect(_va: u64, _size: u64, _prot: u64) -> i64 {
    // Stub — permissions are set at map time for now.
    // A full implementation would update PTE flags and flush the TLB range.
    0
}

// ── Phase 31 Slice A: FlutterEngineProcTable bridge ──────────────────────────

/// Store the user-space VA of the embedder's `FlutterEngineProcTable` so any
/// kernel subsystem (or a second process via `sys_engine_proctable_ptr_get`)
/// can resolve engine entry points without a second dlsym round-trip.
fn sys_engine_proctable_set(ptr: u64, size: u64) -> i64 {
    // Basic sanity: pointer must be non-null and struct must be large enough.
    if ptr == 0 || (size as usize) < core::mem::size_of::<eabi::FlutterEngineProcTable>() {
        return -22; // EINVAL
    }
    ENGINE_PROC_TABLE_PTR.store(ptr, Ordering::Release);
    0
}

/// Return the previously registered proc-table VA, or 0 if none.
fn sys_engine_proctable_ptr_get() -> i64 {
    ENGINE_PROC_TABLE_PTR.load(Ordering::Acquire) as i64
}

/// Record a vsync baton posted by the embedder.
///
/// Flow: engine calls `vsync_callback(user_data, baton)` → embedder calls
/// `sys_engine_vsync_baton_post(baton)` → kernel stores it → on next vsync
/// the EV_VSYNC event carries `b = baton` → embedder reads it and calls
/// `FlutterEngineOnVsync(engine, baton, start_ns, target_ns)`.
fn sys_engine_vsync_baton_post(baton: u64) -> i64 {
    crate::wm::set_vsync_baton(baton);
    0
}

// ── Phase 31 Slice B: GPU / software-rasterizer fast path ────────────────────

/// Upload RGBA32 pixel data and present a surface in a single call.
///
/// This is the kernel fast-path for the Flutter engine's software-renderer
/// `present_callback(user_data, allocation, row_bytes, height)`.  The caller
/// passes the full tightly-packed (row_bytes == width*4) RGBA32 buffer.
///
/// `arg0` = surface_id, `arg1` = pixel_ptr, `arg2` = pixel_len (bytes)
fn sys_gpu_submit(surface_id: u64, pixel_ptr: u64, pixel_len: u64) -> i64 {
    let bytes = match unsafe { read_user_bytes(pixel_ptr, pixel_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    let pid = wm_consumer_pid();
    match crate::compositor::gpu_submit_for(pid, surface_id as u32, bytes) {
        Ok(()) => 0,
        Err("bad payload size") => -22, // EINVAL
        Err("permission denied") => -1, // EPERM
        Err("no such surface") => -3,   // ESRCH
        Err(_) => -12,
    }
}

// ── Phase 32-C: Platform channel syscalls ────────────────────────────────────

/// Post a message from a native caller to the Flutter platform channel bridge.
/// `arg0` = channel_ptr, `arg1` = channel_len, `arg2` = data_ptr, `arg3` = data_len
/// Returns the `u64` sequence number on success, or a negative errno.
fn sys_platform_msg_post(ch_ptr: u64, ch_len: u64, data_ptr: u64, data_len: u64) -> i64 {
    let channel = match unsafe { read_user_bytes(ch_ptr, ch_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    let payload = match unsafe { read_user_bytes(data_ptr, data_len as usize) } {
        Some(b) => b,
        None => return -14,
    };
    let pid = crate::process::current_pid();
    match crate::platform_channel::post(pid, channel, payload) {
        Ok(seq) => {
            // Compute a quick FNV-1a hash of the channel name for the WM event.
            let hash = channel.iter().fold(0x811c9dc5u32, |h, &b| {
                h.wrapping_mul(0x01000193) ^ b as u32
            });
            crate::platform_channel::notify_engine_host(seq, hash);
            seq as i64
        }
        Err("payload too large") => -27,  // EFBIG
        Err("platform message queue full") => -11, // EAGAIN
        Err(_) => -1,
    }
}

/// Copy the oldest pending platform-channel message into a user buffer.
/// `arg0` = buf_ptr, `arg1` = buf_len
/// Returns the number of bytes written, or 0 if no message is pending.
fn sys_platform_msg_recv(buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 || buf_len == 0 {
        return -14; // EFAULT
    }
    // Allocate a kernel-side buffer, fill via platform_channel, then copy out.
    let len = buf_len as usize;
    let mut kbuf: alloc::vec::Vec<u8> = alloc::vec![0u8; len];
    let written = crate::platform_channel::recv_into(&mut kbuf);
    if written == 0 {
        return 0;
    }
    // SAFETY: caller guarantees buf_ptr is valid in their VA space.
    unsafe {
        core::ptr::copy_nonoverlapping(kbuf.as_ptr(), buf_ptr as *mut u8, written);
    }
    written as i64
}

/// Record the engine's reply for a platform-channel message.
/// `arg0` = seq, `arg1` = data_ptr, `arg2` = data_len
fn sys_platform_msg_reply(seq: u64, data_ptr: u64, data_len: u64) -> i64 {
    let payload = match unsafe { read_user_bytes(data_ptr, data_len as usize) } {
        Some(b) => b,
        None => return -14,
    };
    match crate::platform_channel::reply(seq, payload) {
        Ok(()) => 0,
        Err(_) => -3, // ESRCH
    }
}

/// Copy a reply (previously set via sys_platform_msg_reply) into buf and
/// remove the message from the queue.  Returns byte count or 0 if not ready.
/// `arg0` = seq, `arg1` = buf_ptr, `arg2` = buf_len
fn sys_platform_msg_ack(seq: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 || buf_len == 0 {
        return -14;
    }
    let len = buf_len as usize;
    let mut kbuf: alloc::vec::Vec<u8> = alloc::vec![0u8; len];
    let n = crate::platform_channel::ack(seq, &mut kbuf);
    if n == 0 {
        return 0;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(kbuf.as_ptr(), buf_ptr as *mut u8, n);
    }
    n as i64
}

// ── Phase 32-D: Stride-aware GPU blit ────────────────────────────────────────

/// Stride-aware GPU submit. `arg0` = surface_id, `arg1` = pixel_ptr,
/// `arg2` = row_bytes (bytes per scanline; 0 = tight-packed).
fn sys_gpu_submit_strided(surface_id: u64, pixel_ptr: u64, row_bytes: u64) -> i64 {
    // Derive total byte length from the surface dimensions.
    let (width, height) = {
        let packed = crate::compositor::framebuffer_size_packed();
        // Surface dimensions are not in the framebuffer size; use compositor.
        // We pass row_bytes=0 to compute length inside gpu_submit_strided_for.
        let _ = packed;
        (0usize, 0usize) // sentinel: let compositor query its own table
    };
    let _ = (width, height); // compositor will validate internally
    // Estimate buffer length: if row_bytes == 0 we don't know, so read conservatively.
    // The compositor validates the exact size against the surface dimensions.
    let buf_len = if row_bytes == 0 {
        0x800000usize // 8 MiB upper bound — compositor will reject if wrong
    } else {
        // We need height from compositor; pass a generous estimate.
        (row_bytes as usize).saturating_mul(4096) // up to 4096 rows
    };
    let bytes = match unsafe { read_user_bytes(pixel_ptr, buf_len) } {
        Some(b) => b,
        None => return -14,
    };
    let pid = wm_consumer_pid();
    match crate::compositor::gpu_submit_strided_for(pid, surface_id as u32, bytes, row_bytes as usize) {
        Ok(()) => 0,
        Err("bad payload size") => -22,
        Err("permission denied") => -1,
        Err("no such surface") => -3,
        Err(_) => -12,
    }
}

// ── Phase 33-A: vsync rate control ───────────────────────────────────────────

/// Set the hardware vsync rate.  `hz` must be 1–240.
/// Accepted values: 30, 60, 90, 120, 144, 240.  Any value in range is stored.
fn sys_vsync_set_hz(hz: u64) -> i64 {
    let hz = (hz as u32).clamp(1, 240);
    crate::arch::apic::set_vsync_hz(hz);
    0
}

// ── Phase 34-C: Dart AOT snapshot loader ─────────────────────────────────────

/// Load a Dart AOT snapshot from the VFS into the calling process's address
/// space and return its mapped VA via `out_va_ptr`.
///
/// Accepts ELF-wrapped AOT snapshots (`\x7fELF`) and raw Dart snapshots
/// (magic `\xDC\xDC\xDC\xDC`).  The data is copied into fresh anonymous
/// pages mapped PROT_READ|PROT_EXEC.
///
/// `arg0` = path_ptr, `arg1` = path_len,
/// `arg2` = out_va_ptr (`*mut u64`), `arg3` = out_size_ptr (`*mut u64`)
fn sys_aot_snapshot_load(
    path_ptr:     u64,
    path_len:     u64,
    out_va_ptr:   u64,
    out_size_ptr: u64,
) -> i64 {
    let bytes = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    let path = match core::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return -22, // EINVAL
    };
    let data = match crate::fs::lookup(path) {
        Some(d) => d,
        None => return -2, // ENOENT
    };
    // Validate: accept ELF magic or raw Dart snapshot magic (0xDC DC DC DC).
    let valid = (data.len() >= 4 && &data[..4] == b"\x7fELF")
        || (data.len() >= 4 && data[..4] == [0xDC, 0xDC, 0xDC, 0xDC]);
    if !valid {
        return -22; // EINVAL — not a recognised snapshot format
    }
    let pid = crate::process::current_pid();
    if pid == 0 { return -1; } // EPERM
    let pml4_phys = match crate::process::get_user_context(pid) {
        Some(ctx) => ctx.pml4_phys,
        None => return -9, // EBADF
    };
    let pages = (data.len() + 4095) / 4096;
    // Map R|W|X so we can copy the file bytes in; Dart AOT instructions need
    // execute, and BSS-style sections within the AOT image may need write
    // access at engine init time. Userspace cannot exploit this — the mapping
    // is owned by the process and contains only the AOT blob.
    let va = crate::process::dl::mmap_anon(pid, pml4_phys, 0, pages, 7); // R|W|X
    if va == u64::MAX { return -12; } // ENOMEM
    unsafe {
        core::ptr::copy_nonoverlapping(data.as_ptr(), va as *mut u8, data.len());
    }
    if out_va_ptr != 0 {
        unsafe { core::ptr::write(out_va_ptr as *mut u64, va); }
    }
    if out_size_ptr != 0 {
        unsafe { core::ptr::write(out_size_ptr as *mut u64, data.len() as u64); }
    }
    0
}

// ── Phase 35: Dart isolate lifecycle ─────────────────────────────────────────

/// Spawn a Dart isolate in the calling process's AOT snapshot.
///
/// `arg0` = aot_va, `arg1` = aot_size, `arg2` = entry_offset (0 = root isolate),
/// `arg3` = stack_size (0 → default 128 KiB).
/// Returns new isolate ID (>0) on success, or negative errno.
fn sys_isolate_spawn(aot_va: u64, aot_size: u64, entry_offset: u64, stack_size: u64) -> i64 {
    let pid = crate::process::current_pid();
    if pid == 0 { return -1; } // EPERM
    match crate::isolate::spawn(pid, aot_va, aot_size, entry_offset, stack_size as usize) {
        Ok(id) => id as i64,
        Err("spawn_thread failed") => -12, // ENOMEM
        Err("isolate table full")  => -11, // EAGAIN
        Err("invalid aot_va")      => -22, // EINVAL
        Err(_)                     => -22,
    }
}

/// Kill a Dart isolate and free its table slot.
/// `arg0` = isolate_id.
fn sys_isolate_kill(id: u64) -> i64 {
    match crate::isolate::kill(id as u32) {
        Ok(())               => 0,
        Err("no such isolate") => -3, // ESRCH
        Err(_)               => -1,
    }
}

/// Query or change an isolate's state.
///
/// `arg0` = isolate_id, `arg1` = op (0=get, 1=pause, 2=resume).
/// Returns state value (1/2/3) for op=0, or 0 for op=1/2.
fn sys_isolate_ctrl(id: u64, op: u64) -> i64 {
    match crate::isolate::ctrl(id as u32, op as u32) {
        Ok(v)                  => v as i64,
        Err("no such isolate") => -3,  // ESRCH
        Err("unknown op")      => -22, // EINVAL
        Err(_)                 => -1,
    }
}

// ── Phase 36: Dart isolate message passing ────────────────────────────────────────

/// Send a message to a destination isolate's inbox.
///
/// `arg0` = dst_isolate_id, `arg1` = data_ptr, `arg2` = data_len.
/// Returns 0 on success or negative errno.
fn sys_isolate_msg_send(dst_id: u64, data_ptr: u64, data_len: u64) -> i64 {
    if data_len as usize > crate::isolate_msg::MAX_MSG_SIZE {
        return -27; // EFBIG
    }
    let data = match unsafe { read_user_bytes(data_ptr, data_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    match crate::isolate_msg::send(dst_id as u32, data) {
        Ok(())                     => 0,
        Err("message too large")   => -27, // EFBIG
        Err("no inbox for isolate") => -3,  // ESRCH
        Err(_)                     => -1,
    }
}

/// Copy the next message for `isolate_id` from its inbox into a user buffer.
///
/// `arg0` = isolate_id, `arg1` = buf_ptr, `arg2` = buf_len.
/// Returns bytes written (>0), 0 if inbox empty, or negative errno.
fn sys_isolate_msg_recv(isolate_id: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 || buf_len == 0 {
        return -14; // EFAULT
    }
    let cap = (buf_len as usize).min(crate::isolate_msg::MAX_MSG_SIZE);
    let mut kbuf: alloc::vec::Vec<u8> = alloc::vec![0u8; cap];
    let n = crate::isolate_msg::recv(isolate_id as u32, &mut kbuf);
    if n == 0 {
        return 0; // EAGAIN: no messages
    }
    if unsafe { write_user_bytes(buf_ptr, &kbuf[..n]) } {
        n as i64
    } else {
        -14 // EFAULT
    }
}

/// Return the number of pending messages in `isolate_id`'s inbox.
/// `arg0` = isolate_id.  Returns count (≥0) or -3 (ESRCH) if unknown isolate.
fn sys_isolate_msg_pending(isolate_id: u64) -> i64 {
    crate::isolate_msg::pending(isolate_id as u32) as i64
}

// ── Phase 37 — PS/2 input device query ────────────────────────────────────────

/// Return the number of detected input devices (keyboard + mouse).
fn sys_input_dev_count() -> i64 {
    crate::drivers::ps2::device_count() as i64
}

/// Return packed device descriptor for device index `n` (0-based).
/// Bit layout: bits[3:0]=type(1=kbd,2=mouse), bits[11:4]=IRQ, bits[15:12]=iface(0=PS/2).
/// Returns 0 if `n` is out of range.
fn sys_input_dev_info(n: u64) -> i64 {
    crate::drivers::ps2::device_info_packed(n as u32) as i64
}

// ── Phase 38 — .oscapp app registry ──────────────────────────────────────────

/// Install a `.oscapp` bundle provided by the caller.
/// `arg0` = bundle_ptr, `arg1` = bundle_len, `arg2` = id_out_ptr (u32le).
fn sys_app_install(bundle_ptr: u64, bundle_len: u64, id_out_ptr: u64) -> i64 {
    let bytes = match unsafe { read_user_bytes(bundle_ptr, bundle_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    match crate::app_registry::install(bytes) {
        Some(app_id) => {
            if id_out_ptr != 0 {
                unsafe {
                    core::ptr::write_unaligned(id_out_ptr as *mut u32, app_id);
                }
            }
            0
        }
        None => -22, // EINVAL — bad bundle or table full
    }
}

/// Serialise the installed app list into a user buffer.
/// Each entry is 88 bytes: [id: u32le][name: u8×64][version: u8×16][aot_len: u32le].
/// Returns the total number of installed apps (may exceed buf capacity).
fn sys_app_list(buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 || buf_len == 0 {
        return crate::app_registry::count() as i64;
    }
    if buf_len > 0x20_0000 { return -22; } // EINVAL
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, buf_len as usize) };
    crate::app_registry::list(buf) as i64
}

/// Launch an installed app.
/// `arg0` = app_id, `arg1` = flags (reserved, pass 0).
/// Returns the new isolate_id, or -ERRNO.
fn sys_app_launch(app_id: u64, flags: u64) -> i64 {
    crate::app_registry::launch(app_id as u32, flags as u32)
}

/// Uninstall an installed app by `app_id`.
fn sys_app_uninstall(app_id: u64) -> i64 {
    if crate::app_registry::uninstall(app_id as u32) { 0 } else { -2 } // ENOENT
}

// ── Phase 39 — Named port IPC namespace ──────────────────────────────────────

/// Bind the calling process's isolate under `name`.
/// `arg0` = name_ptr, `arg1` = name_len, `arg2` = isolate_id.
fn sys_port_bind(name_ptr: u64, name_len: u64, isolate_id: u64) -> i64 {
    let name = match unsafe { read_user_bytes(name_ptr, name_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    let pid = crate::process::current_pid();
    crate::port_ns::bind(name, isolate_id as u32, pid)
}

/// Look up a named port.
/// `arg0` = name_ptr, `arg1` = name_len, `arg2` = iso_id_out_ptr (u32le), `arg3` = pid_out_ptr (u32le).
fn sys_port_lookup(name_ptr: u64, name_len: u64, iso_out: u64, pid_out: u64) -> i64 {
    let name = match unsafe { read_user_bytes(name_ptr, name_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    let mut iso = 0u32;
    let mut pid = 0u32;
    let rc = crate::port_ns::lookup(name, &mut iso, &mut pid);
    if rc == 0 {
        if iso_out != 0 {
            unsafe { core::ptr::write_unaligned(iso_out as *mut u32, iso); }
        }
        if pid_out != 0 {
            unsafe { core::ptr::write_unaligned(pid_out as *mut u32, pid); }
        }
    }
    rc
}

/// Unbind a named port.
fn sys_port_unbind(name_ptr: u64, name_len: u64) -> i64 {
    let name = match unsafe { read_user_bytes(name_ptr, name_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    crate::port_ns::unbind(name)
}

// ── Phase 41 — USB host-controller query ─────────────────────────────────────

fn sys_usb_controller_count() -> i64 {
    crate::drivers::usb::xhci_count() as i64
}

// ── Phase 42 — Userspace framebuffer map + WM event dequeue ─────────────────

/// Map the framebuffer into the calling process's address space.
///
/// Writes a 24-byte FbInfo struct to `info_out_ptr`:
///   u64 addr        – user VA of the mapped framebuffer
///   u32 width       – pixels wide
///   u32 height      – pixels tall
///   u32 pitch       – bytes per row
///   u32 bpp         – bits per pixel (always 32)
fn sys_fb_map(info_out_ptr: u64) -> i64 {
    let (hhdm_va, width, height, pitch_bytes) = match crate::drivers::fb::fb_info() {
        Some(i) => i,
        None    => {
            log::warn!("[sys_fb_map] fb_info() returned None");
            return -6; // ENXIO — framebuffer not ready
        }
    };

    let hhdm_off = crate::mm::frame_allocator::hhdm_offset();
    let fb_phys  = hhdm_va - hhdm_off;

    let pid = crate::process::current_pid();
    let pml4_phys = match crate::process::get_user_context(pid) {
        Some(ctx) => ctx.pml4_phys,
        None      => {
            log::warn!("[sys_fb_map] get_user_context({}) returned None", pid);
            return -9; // EBADF
        }
    };
    log::debug!("[sys_fb_map] pid={} fb={}x{} pitch={} phys={:#x}", pid, width, height, pitch_bytes, fb_phys);

    // Fixed user VA for the framebuffer (1 GiB mark — above typical ELF load).
    const FB_USER_BASE: u64 = 0x4000_0000;
    let fb_size    = height as u64 * pitch_bytes as u64;
    let page_count = (fb_size + 0xFFF) / 0x1000;

    for i in 0..page_count {
        let phys_page = fb_phys + i * 0x1000;
        let user_va   = FB_USER_BASE + i * 0x1000;
        if unsafe { crate::mm::paging::map_user_page(pml4_phys, user_va, phys_page) }.is_err() {
            return -12; // ENOMEM
        }
    }

    // Write FbInfo to user buffer (byte-by-byte to avoid alignment UB).
    unsafe {
        let dst = info_out_ptr as *mut u8;
        let user_addr: u64 = FB_USER_BASE;
        dst.add( 0).cast::<[u8;8]>().write_unaligned(user_addr.to_le_bytes());
        dst.add( 8).cast::<[u8;4]>().write_unaligned(width.to_le_bytes());
        dst.add(12).cast::<[u8;4]>().write_unaligned(height.to_le_bytes());
        dst.add(16).cast::<[u8;4]>().write_unaligned(pitch_bytes.to_le_bytes());
        dst.add(20).cast::<[u8;4]>().write_unaligned(32u32.to_le_bytes());
    }
    // Signal compositor that a process now owns the FB directly.
    crate::compositor::set_fb_bypass(true);
    0
}

/// Pop the next WM event for the calling process.
///
/// Writes a 32-byte WmEvent to `event_out_ptr` on success (returns 0).
/// Returns -11 (EAGAIN) if no events are pending.
fn sys_wm_next_event(event_out_ptr: u64) -> i64 {
    let pid = crate::process::current_pid();
    match crate::wm::pop_event_for(pid) {
        Some(ev) => {
            // WmEvent is #[repr(C)] — copy 32 bytes to user buffer.
            unsafe {
                let src = &ev as *const crate::embedder::abi::WmEvent as *const u8;
                core::ptr::copy_nonoverlapping(src, event_out_ptr as *mut u8, 32);
            }
            0
        }
        None => -11, // EAGAIN
    }
}

// ── Phase 43 — VFS query API (no open-fd) ────────────────────────────────────

/// List files whose path starts with `path`, writing "name\n" per entry.
/// `arg0` = path_ptr, `arg1` = path_len, `arg2` = buf_ptr, `arg3` = buf_len.
/// Returns total bytes written.
fn sys_vfs_list(path_ptr: u64, path_len: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    let pb = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b,
        None => return -14,
    };
    let path = match core::str::from_utf8(pb) {
        Ok(s) => s,
        Err(_) => return -22,
    };
    if buf_ptr == 0 || buf_len == 0 { return -14; }
    let n = crate::fs::list_prefix(path, buf_ptr as *mut u8, buf_len as usize);
    n as i64
}

/// One-shot VFS file read without an fd.
/// `arg0` = path_ptr, `arg1` = path_len, `arg2` = buf_ptr, `arg3` = buf_len.
/// Returns bytes copied.
fn sys_vfs_read(path_ptr: u64, path_len: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    let pb = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b,
        None => return -14,
    };
    let path = match core::str::from_utf8(pb) {
        Ok(s) => s,
        Err(_) => return -22,
    };
    let data = match crate::fs::lookup(path) {
        Some(d) => d,
        None => return -2, // ENOENT
    };
    if buf_ptr == 0 { return -14; }
    let copy_len = (buf_len as usize).min(data.len());
    unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), buf_ptr as *mut u8, copy_len); }
    copy_len as i64
}

// ── Phase 44 — writable ramdisk (/tmp/) ──────────────────────────────────────

/// Write data to `/tmp/<name>`.
/// `arg0` = path_ptr, `arg1` = path_len, `arg2` = data_ptr, `arg3` = data_len.
fn sys_vfs_write(path_ptr: u64, path_len: u64, data_ptr: u64, data_len: u64) -> i64 {
    let pb = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b,
        None => return -14,
    };
    let path = match core::str::from_utf8(pb) {
        Ok(s) => s,
        Err(_) => return -22,
    };
    let data = match unsafe { read_user_bytes(data_ptr, data_len as usize) } {
        Some(b) => b,
        None => return -14,
    };
    match crate::fs::write(path, data) {
        Ok(()) => 0,
        Err("not under /tmp/")        => -13, // EACCES
        Err("ramdisk full")           => -28, // ENOSPC
        Err("ramdisk slot table full")=> -28,
        Err("path too long")          => -36, // ENAMETOOLONG
        Err(_)                        => -1,
    }
}

/// Stat a VFS path — writes `u64` file size to `size_out_ptr`.
/// Returns 0 on success, -ENOENT if not found.
fn sys_vfs_stat(path_ptr: u64, path_len: u64, size_out_ptr: u64) -> i64 {
    let pb = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b,
        None => return -14,
    };
    let path = match core::str::from_utf8(pb) {
        Ok(s) => s,
        Err(_) => return -22,
    };
    match crate::fs::stat(path) {
        Some(sz) => {
            if size_out_ptr != 0 {
                unsafe { core::ptr::write_unaligned(size_out_ptr as *mut u64, sz); }
            }
            0
        }
        None => -2, // ENOENT
    }
}

// ── Phase 45 — virtio-net networking ─────────────────────────────────────────

/// Write virtio-net info (MAC, IP) as text into user buffer.
/// Returns bytes written.
fn sys_net_info(buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 || buf_len == 0 { return -14; }
    let len = (buf_len as usize).min(256);
    let mut kbuf = [0u8; 256];
    let n = crate::drivers::virtio_net::info_text(&mut kbuf[..len]);
    unsafe { core::ptr::copy_nonoverlapping(kbuf.as_ptr(), buf_ptr as *mut u8, n); }
    n as i64
}

/// Enqueue a raw UDP-like payload as an Ethernet frame (stub).
/// `arg0` = dst_ip (u32 BE), `arg1` = dst_port (u16), `arg2` = data_ptr, `arg3` = data_len.
fn sys_net_send(dst_ip: u64, _dst_port: u64, data_ptr: u64, data_len: u64) -> i64 {
    if !crate::drivers::virtio_net::is_ready() { return -6; } // ENXIO
    let data = match unsafe { read_user_bytes(data_ptr, (data_len as usize).min(1514)) } {
        Some(b) => b,
        None => return -14,
    };
    // Build a minimal Ethernet frame (broadcast dst, random src).
    let mut frame = [0u8; 1514 + 14];
    frame[0..6].copy_from_slice(&[0xFF; 6]); // dst MAC: broadcast
    frame[6..12].copy_from_slice(&[0x52, 0x54, 0x00, 0x12, 0x34, 0x56]); // src MAC: QEMU default
    frame[12] = 0x08; frame[13] = 0x00; // EtherType IPv4
    let payload_end = 14 + data.len().min(1500);
    frame[14..payload_end].copy_from_slice(&data[..data.len().min(1500)]);
    match crate::drivers::virtio_net::send_raw(&frame[..payload_end]) {
        Ok(()) => 0,
        Err(_) => -11, // EAGAIN
    }
}

/// Poll for a received raw packet.
/// `arg0` = buf_ptr, `arg1` = buf_len, `arg2` = src_ip_out (*mut u32), `arg3` = src_port_out (*mut u16).
fn sys_net_recv(buf_ptr: u64, buf_len: u64, _src_ip_out: u64, _src_port_out: u64) -> i64 {
    if buf_ptr == 0 || buf_len == 0 { return -14; }
    let mut kbuf = [0u8; 1514 + 14];
    match crate::drivers::virtio_net::recv_raw(&mut kbuf) {
        Some(n) => {
            let copy = n.min(buf_len as usize);
            unsafe { core::ptr::copy_nonoverlapping(kbuf.as_ptr(), buf_ptr as *mut u8, copy); }
            copy as i64
        }
        None => -11, // EAGAIN
    }
}

// ── Phase 46 — compositor bypass + full-screen surface ───────────────────────

/// Release the direct-FB mapping so the compositor can resume rendering.
fn sys_fb_release() -> i64 {
    crate::compositor::set_fb_bypass(false);
    0
}

/// Create a surface that exactly covers the framebuffer.
/// Returns the new surface id, or negative errno.
fn sys_surface_fullscreen() -> i64 {
    let (w, h) = crate::drivers::fb::size_px().unwrap_or((0, 0));
    if w == 0 { return -6; } // ENXIO
    let pid = wm_consumer_pid();
    match crate::compositor::create_surface_for(pid, w, h) {
        Ok(id) => {
            let _ = crate::compositor::move_surface_for(pid, id, 0, 0, 0);
            id as i64
        }
        Err(_) => -12, // ENOMEM
    }
}

// ── Phase 48 — UART 16550 serial console ─────────────────────────────────────

/// Read up to `buf_len` bytes from COM1 into user buffer.
/// Returns bytes read, or -11 (EAGAIN) if no data is available.
fn sys_serial_read(buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 { return -14; } // EFAULT
    let cap = (buf_len as usize).min(256);
    let mut count = 0usize;
    while count < cap {
        match crate::drivers::uart::read_byte_nonblocking() {
            Some(b) => {
                unsafe { (buf_ptr as *mut u8).add(count).write_volatile(b); }
                count += 1;
            }
            None => break,
        }
    }
    if count == 0 { -11 } else { count as i64 }
}

/// Write `buf_len` bytes from user buffer to COM1.
fn sys_serial_write(buf_ptr: u64, buf_len: u64) -> i64 {
    let bytes = match unsafe { read_user_bytes(buf_ptr, (buf_len as usize).min(4096)) } {
        Some(b) => b,
        None => return -14,
    };
    crate::drivers::uart::write_bytes(bytes);
    bytes.len() as i64
}

// ── Phase 49 — virtio-blk block device ───────────────────────────────────────

/// Write device info into user buffer.
/// Returns bytes written.  `arg0` = buf_ptr (64-byte minimum recommended).
fn sys_blk_info(buf_ptr: u64) -> i64 {
    if buf_ptr == 0 { return -14; }
    let mut tmp = [0u8; 128];
    let n = crate::drivers::virtio_blk::info_text(&mut tmp);
    if n == 0 { return 0; }
    unsafe {
        core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf_ptr as *mut u8, n);
    }
    n as i64
}

/// Read sectors from the block device into a user buffer.
/// arg0=sector, arg1=count, arg2=buf_ptr, arg3=buf_len
fn sys_blk_read(sector: u64, count: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 { return -14; }
    let needed = (count * 512) as usize;
    if buf_len < needed as u64 { return -22; } // EINVAL
    if needed > 1 << 20 { return -22; } // cap at 1 MiB
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, needed) };
    match crate::drivers::virtio_blk::read_sectors(sector, count, buf) {
        Ok(()) => needed as i64,
        Err(_)  => -5, // EIO
    }
}

/// Write sectors to the block device from a user buffer.
fn sys_blk_write(sector: u64, count: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    let needed = (count * 512) as usize;
    if buf_len < needed as u64 { return -22; }
    let data = match unsafe { read_user_bytes(buf_ptr, needed) } {
        Some(b) => b,
        None => return -14,
    };
    match crate::drivers::virtio_blk::write_sectors(sector, count, data) {
        Ok(()) => needed as i64,
        Err(_)  => -5,
    }
}

// ── Phase 50 — smoltcp TCP/IP ────────────────────────────────────────────────

/// Open a TCP connection.  arg0 = dst_ip (BE u32), arg1 = dst_port.
/// Returns socket fd ≥ 0 on success, or negative errno.
fn sys_tcp_connect(dst_ip: u64, dst_port: u64) -> i64 {
    match crate::net::tcp::tcp_connect(dst_ip as u32, dst_port as u16) {
        Ok(fd)   => fd as i64,
        Err(e)   => e,
    }
}

/// Write to a TCP socket.  arg0=fd, arg1=buf_ptr, arg2=buf_len.
fn sys_tcp_write(fd: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    let data = match unsafe { read_user_bytes(buf_ptr, (buf_len as usize).min(65536)) } {
        Some(b) => b,
        None => return -14,
    };
    match crate::net::tcp::tcp_write(fd as usize, data) {
        Ok(n)  => n as i64,
        Err(e) => e,
    }
}

/// Read from a TCP socket.  arg0=fd, arg1=buf_ptr, arg2=buf_len.
fn sys_tcp_read(fd: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 { return -14; }
    let cap = (buf_len as usize).min(65536);
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, cap) };
    match crate::net::tcp::tcp_read(fd as usize, buf) {
        Ok(n)  => n as i64,
        Err(e) => e,
    }
}

/// Close a TCP socket.
fn sys_tcp_close(fd: u64) -> i64 {
    match crate::net::tcp::tcp_close(fd as usize) {
        Ok(())  => 0,
        Err(e)  => e,
    }
}

/// Trigger DHCP DISCOVER.  Returns assigned IP as BE u32, or 0 on failure.
fn sys_dhcp_discover() -> i64 {
    crate::net::tcp::dhcp_discover() as i64
}

// ── Phase 51: ext2 read-only filesystem ──────────────────────────────────────

fn sys_ext2_mount() -> i64 {
    match crate::fs::ext2::mount() { Ok(()) => 0, Err(_) => -5 }
}

fn sys_ext2_ls(path_ptr: u64, path_len: u64, out_ptr: u64, out_len: u64) -> i64 {
    if path_ptr == 0 || out_ptr == 0 { return -14; }
    let path_bytes = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b, None => return -14,
    };
    let out = unsafe { core::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len as usize) };
    match crate::fs::ext2::ls(path_bytes, out) { Ok(n) => n as i64, Err(_) => -2 }
}

fn sys_ext2_read(path_ptr: u64, path_len: u64, out_ptr: u64, out_len: u64) -> i64 {
    if path_ptr == 0 || out_ptr == 0 { return -14; }
    let path_bytes = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b, None => return -14,
    };
    let out = unsafe { core::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len as usize) };
    match crate::fs::ext2::read_file(path_bytes, out) { Ok(n) => n as i64, Err(_) => -2 }
}

// ── Phase 53: scheduler extras ───────────────────────────────────────────────

fn sys_sched_yield() -> i64 {
    let cur = crate::process::current_pid();
    if let Some(next) = crate::process::next_runnable_pid(cur) {
        if next != cur {
            crate::process::set_state(cur, crate::process::ProcState::Running);
            crate::process::set_current_pid(next);
            // Context switch will happen on next APIC timer or we do it inline.
        }
    }
    0
}

fn sys_get_cpu_time(pid: u64) -> i64 {
    let target = if pid == 0 { crate::process::current_pid() } else { pid as u32 };
    crate::process::get_cpu_ticks(target) as i64
}

// ── Phase 54: fork ────────────────────────────────────────────────────────────

fn sys_fork() -> i64 {
    match crate::process::fork_current() {
        Ok(child_pid) => child_pid as i64,
        Err(_) => -12, // ENOMEM
    }
}

// ── Phase 55: signals ─────────────────────────────────────────────────────────

fn sys_kill_signal(target_pid: u64, sig: u64) -> i64 {
    if sig > 31 { return -22; } // EINVAL
    crate::process::raise_signal(target_pid as u32, sig as u8);
    0
}

fn sys_sigaction(sig: u64, handler_ptr: u64) -> i64 {
    if sig == 0 || sig > 31 { return -22; }
    let pid = crate::process::current_pid();
    crate::process::set_signal_handler(pid, sig as u8, handler_ptr);
    0
}

fn sys_sigreturn() -> i64 {
    // Signal return — the user stack should have saved context; for now just
    // set running state and return 0 (real implementation needs the signal
    // frame layout to match the signal delivery code).
    0
}

// ── Phase 59: NVMe driver ─────────────────────────────────────────────────────

fn sys_nvme_info(buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 { return -14; }
    let out = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, buf_len as usize) };
    crate::drivers::nvme::info_text(out) as i64
}

fn sys_nvme_read(lba: u64, count: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 { return -14; }
    if buf_len < count * 512 { return -22; }
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, buf_len as usize) };
    match crate::drivers::nvme::read_sectors(lba, count as u32, buf) {
        Ok(()) => count as i64 * 512,
        Err(_) => -5,
    }
}

fn sys_nvme_write(lba: u64, count: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 { return -14; }
    if buf_len < count * 512 { return -22; }
    let buf = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, buf_len as usize) };
    match crate::drivers::nvme::write_sectors(lba, count as u32, buf) {
        Ok(()) => count as i64 * 512,
        Err(_) => -5,
    }
}

// ── Phase 62: futex ───────────────────────────────────────────────────────────
//
// Minimal Linux-compatible futex(2) subset for pthread / Dart VM support.
// Supported ops:
//   FUTEX_WAIT (0): sleep until *uaddr changes from val or a wake arrives.
//   FUTEX_WAKE (1): wake up to val waiters on uaddr.
// Returns 0 on success, negative errno on error.
// This keeps a tiny in-kernel waiter map so pthread condition variables and
// mutexes can park instead of burning CPU in a busy loop.

const FUTEX_WAIT: u32 = 0;
const FUTEX_WAKE: u32 = 1;

fn futex_waiter_add(addr: u64, pid: u32) {
    let mut table = FUTEX_WAITERS.lock();
    let waiters = table.entry(addr).or_insert_with(Vec::new);
    if !waiters.contains(&pid) {
        waiters.push(pid);
    }
}

fn futex_waiter_remove(addr: u64, pid: u32) {
    let mut table = FUTEX_WAITERS.lock();
    if let Some(waiters) = table.get_mut(&addr) {
        waiters.retain(|&waiter| waiter != pid);
        if waiters.is_empty() {
            table.remove(&addr);
        }
    }
}

fn futex_waiter_present(addr: u64, pid: u32) -> bool {
    let table = FUTEX_WAITERS.lock();
    table.get(&addr).map_or(false, |waiters| waiters.contains(&pid))
}

fn futex_wake_waiters(addr: u64, count: u32) -> i64 {
    let wake_list = {
        let mut table = FUTEX_WAITERS.lock();
        let Some(waiters) = table.get_mut(&addr) else { return 0; };
        let wake_count = waiters.len().min(count as usize);
        let mut woke = Vec::with_capacity(wake_count);
        for _ in 0..wake_count {
            woke.push(waiters.remove(0));
        }
        if waiters.is_empty() {
            table.remove(&addr);
        }
        woke
    };

    for pid in &wake_list {
        // Best-effort: ensure waker target is marked Running so a scheduler
        // that has parked it can pick it up again.
        crate::process::set_state(*pid, crate::process::ProcState::Running);
    }

    wake_list.len() as i64
}

fn sys_futex(uaddr: u64, op: u32, val: u32) -> i64 {
    let op_base = op & 0x7F; // strip FUTEX_PRIVATE_FLAG etc.
    match op_base {
        FUTEX_WAIT => {
            if uaddr == 0 || uaddr & 3 != 0 { return -22; } // EINVAL: unaligned
            let cur = unsafe { core::ptr::read_volatile(uaddr as *const u32) };
            if cur != val {
                return -11; // EAGAIN: value changed before we could sleep
            }

            let pid = crate::process::current_pid();
            if pid == 0 {
                return 0;
            }

            futex_waiter_add(uaddr, pid);

            // Try to cooperatively hand the CPU to a sibling user thread that
            // is runnable. This is the only path by which other threads of
            // PID 1 (e.g. the Flutter engine worker) can actually execute,
            // since the APIC timer ISR does not preempt user mode during
            // bring-up. We save our user return context (with RAX=0 so the
            // resumed syscall reports success), mark ourselves Blocked, and
            // SYSRET into the sibling. A subsequent FUTEX_WAKE will mark us
            // Running again and the next yielding thread will pick us up.
            if let Some(next) = crate::process::next_runnable_pid(pid) {
                if next != pid {
                    let urip = crate::arch::syscall::user_rip();
                    let ursp = crate::arch::syscall::user_rsp();
                    log::warn!(
                        "[futex] yield pid={} -> pid={} uaddr={:#x} val={} rip={:#x}",
                        pid, next, uaddr, val, urip
                    );
                    crate::process::save_return_context(pid, urip, ursp);
                    crate::process::set_rax(pid, 0);
                    crate::process::save_xstate(pid);
                    crate::process::set_state(pid, crate::process::ProcState::Blocked);
                    crate::process::enter_user_by_pid_noreturn(next);
                }
            }

            // No sibling to switch into — fall back to a bounded spin so the
            // single-thread case still makes forward progress on a real wake.
            let mut iters: u32 = 0;
            const FUTEX_MAX_SPIN: u32 = 4_000_000;
            while iters < FUTEX_MAX_SPIN
                && unsafe { core::ptr::read_volatile(uaddr as *const u32) } == val
                && futex_waiter_present(uaddr, pid)
            {
                core::hint::spin_loop();
                iters = iters.wrapping_add(1);
            }

            futex_waiter_remove(uaddr, pid);
            0
        }
        FUTEX_WAKE => futex_wake_waiters(uaddr, val),
        _ => -22, // EINVAL: unsupported op
    }
}

// ── Phase 60: pty/tty ─────────────────────────────────────────────────────────

fn sys_pty_open(flags: u64) -> i64 {
    let _ = flags;
    match crate::drivers::pty::open() {
        Ok((master, slave)) => ((master as i64) << 32) | (slave as i64),
        Err(_) => -24, // ENFILE
    }
}

fn sys_pty_read(fd: u64, buf_ptr: u64, count: u64) -> i64 {
    if buf_ptr == 0 { return -14; }
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, count as usize) };
    match crate::drivers::pty::read(fd as u32, buf) {
        Ok(n) => n as i64,
        Err(_) => -5,
    }
}

fn sys_pty_write(fd: u64, buf_ptr: u64, count: u64) -> i64 {
    if buf_ptr == 0 { return -14; }
    let buf = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, count as usize) };
    match crate::drivers::pty::write(fd as u32, buf) {
        Ok(n) => n as i64,
        Err(_) => -5,
    }
}

fn sys_pty_ioctl(fd: u64, cmd: u64, arg: u64) -> i64 {
    crate::drivers::pty::ioctl(fd as u32, cmd, arg)
}

// ── Phase 31 Slice C: shared-address-space threads ───────────────────────────

/// Create a thread in the current process's address space.
///
/// Supports two calling conventions on syscall 0x35A:
/// - Raw OSCortex ABI: `(entry_fn, arg, stack_size)` -> returns `tid`.
/// - POSIX pthread ABI: `(pthread_t* out, attr*, start_routine, arg)` ->
///   writes `*out = tid`, returns `0` on success.
fn sys_thread_create(arg0: u64, arg1: u64, arg2: u64, arg3: u64) -> i64 {
    let parent_pid = crate::process::current_pid();
    if parent_pid == 0 {
        return -1; // EPERM
    }

    let spawn = |entry_fn: u64, arg: u64, stack_size: u64| -> Result<u32, i64> {
        let sz = if stack_size == 0 { 65536 } else { stack_size as usize };
        crate::process::spawn_thread(parent_pid, entry_fn, arg, sz).map_err(|_| -12)
    };

    // Heuristic for pthread_create-style call shape:
    // arg0 = pointer to pthread_t out, arg2 = start routine, arg3 = arg.
    // Raw ABI uses arg2 as stack_size, which is typically small; pthread
    // start-routine pointers are code VAs and thus much larger.
    if arg2 >= 0x10_0000 {
        let mut stack_size = 0u64;
        if arg1 != 0 {
            // Best-effort read of attr->stack_size at +8 (glibc/musl-compatible).
            if let Some(raw) = unsafe { read_user_bytes(arg1 + 8, 8) } {
                let mut tmp = [0u8; 8];
                tmp.copy_from_slice(raw);
                stack_size = u64::from_le_bytes(tmp);
            }
        }
        let r = match spawn(arg2, arg3, stack_size) {
            Ok(tid) => {
                unsafe { *(arg0 as *mut u64) = tid as u64; }
                0
            }
            Err(errno) => -errno,
        };
        log::error!("[trace] sys_thread_create POSIX out={:#x} attr={:#x} entry={:#x} arg={:#x} stk={:#x} -> {}",
            arg0, arg1, arg2, arg3, stack_size, r);
        // Dump first 32 bytes at entry function to identify shape (jmp/tail vs prologue).
        if r == 0 {
            let mut bytes = [0u8; 32];
            unsafe {
                for i in 0..32 {
                    bytes[i] = core::ptr::read_volatile((arg2 + i as u64) as *const u8);
                }
            }
            log::warn!("[trace] start_routine@{:#x} bytes={:02x?}", arg2, bytes);
        }
        r
    } else {
        // Raw OSCortex ABI.
        match spawn(arg0, arg1, arg2) {
            Ok(tid) => tid as i64,
            Err(errno) => errno,
        }
    }
}

/// Exit the current thread (or process if not a thread).
fn sys_thread_exit(code: u64) -> i64 {
    let pid = crate::process::current_pid();
    let user_rip = crate::arch::syscall::user_rip();
    let user_rsp = crate::arch::syscall::user_rsp();
    log::warn!("[trace] sys_thread_exit pid={} code={:#x} rip={:#x} rsp={:#x}",
        pid, code, user_rip, user_rsp);
    sys_exit(code)
}

/// Wait for a thread to finish and return its exit code.
fn sys_thread_join(tid: u64) -> i64 {
    match crate::process::waitpid(tid as u32) {
        Ok(code) => code as i64,
        Err("not exited") => -11,  // EAGAIN
        Err(_) => -10,             // ECHILD
    }
}

/// Linux-compat `clone(2)` — only thread-creation mode (CLONE_VM) supported.
///
/// Registers at syscall entry (SysV SYSCALL convention):
///   RAX=56, RDI=flags, RSI=stack_top, RDX=parent_tidptr (ignored),
///   R10=child_tidptr (ignored), R8=tls (ignored)
///
/// When CLONE_VM (0x100) is set the child shares the parent's address space.
/// The child's RIP is the SYSCALL return address (user_rip), the child's RSP
/// is `stack_top` (arg1).  Parent receives child TID; child receives 0.
fn sys_clone(flags: u64, stack_top: u64) -> i64 {
    const CLONE_VM: u64 = 0x0000_0100;
    if flags & CLONE_VM == 0 {
        return -38; // ENOSYS — only thread-clone supported
    }
    let parent_pid = crate::process::current_pid();
    if parent_pid == 0 {
        return -1; // EPERM
    }
    // Use the SYSCALL return address as the child's starting RIP.
    let child_rip = crate::arch::syscall::user_rip();
    if child_rip == 0 {
        return -22; // EINVAL
    }
    // stack_top == 0 means "copy parent stack" — not supported; reject.
    if stack_top == 0 {
        return -22; // EINVAL
    }
    match crate::process::clone_thread(parent_pid, child_rip, stack_top) {
        Ok(tid) => {
            // The child slot already has RAX=0 set by clone_thread.
            tid as i64 // parent sees child TID
        }
        Err(_) => -12, // ENOMEM
    }
}

// ── POSIX compatibility shim implementations ──────────────────────────────────

mod posix {
    //! Implementations of POSIX/glibc functions dispatched via syscall stubs
    //! from the trampoline page mapped by posix_trampolines.rs.

    use super::{read_user_bytes, write_user_bytes};
    use crate::process::{self, dl::mmap_anon};
    use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
    use spin::Mutex;
    use alloc::vec::Vec;
    use alloc::collections::BTreeMap;

    // ── Per-process TLS area ──────────────────────────────────────────────────
    //
    // Each process gets a 128 KiB TLS area starting at a bumped user VA.
    // We store (pid → tls_base) here; the area is allocated on first use.

    static TLS_TABLE: Mutex<BTreeMap<u32, u64>> = Mutex::new(BTreeMap::new());

    fn get_or_alloc_tls(pid: u32, pml4_phys: u64) -> u64 {
        let mut table = TLS_TABLE.lock();
        if let Some(&base) = table.get(&pid) {
            return base;
        }
        // Allocate 128 KiB for TLS in the current process.
        let pages = 32; // 32 × 4 KiB = 128 KiB
        let base = mmap_anon(pid, pml4_phys, 0, pages, 3);
        if base != u64::MAX {
            table.insert(pid, base);
        }
        base
    }

    // ── Per-process key-value store (pthread_key_*) ───────────────────────────

    // key: (pid, thread_id, key_id) → value (u64)
    static KEY_TABLE: Mutex<BTreeMap<(u32, u32, u32), u64>> = Mutex::new(BTreeMap::new());
    static NEXT_KEY: AtomicU32 = AtomicU32::new(1);

    // ── Per-thread pthread_self object ─────────────────────────────────────
    //
    // glibc/libstdc++ often treat pthread_t as a pointer to thread state and
    // dereference fixed offsets (for example +0x68). Returning a small integer
    // TID causes null-ish derefs inside C++ constructors.
    //
    // key: tid(pid) -> user VA of synthetic pthread object (256 bytes)
    static PTHREAD_SELF_TABLE: Mutex<BTreeMap<u32, u64>> = Mutex::new(BTreeMap::new());

    // ── Per-process malloc tracking ───────────────────────────────────────────
    // (pid, user_va) → pages_allocated
    // Used so free/realloc can skip already-allocated regions.
    // Since munmap is a stub (no actual unmapping), free is a no-op here.

    // ── Random state ─────────────────────────────────────────────────────────

    static RAND_STATE: AtomicU64 = AtomicU64::new(1234567890);
    static LOCALE_OBJ: AtomicU64 = AtomicU64::new(0);
    static LOCALE_CURRENT: AtomicU64 = AtomicU64::new(0);

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn pid_and_pml4() -> (u32, u64) {
        let pid = process::current_pid();
        let pml4 = process::get_user_context(pid)
            .map(|ctx| ctx.pml4_phys)
            .unwrap_or(0);
        (pid, pml4)
    }

    /// Read a null-terminated C string from user memory (max 4096 bytes).
    unsafe fn read_cstr(ptr: u64) -> Option<&'static [u8]> {
        if ptr == 0 { return None; }
        let p = ptr as *const u8;
        let mut len = 0usize;
        unsafe {
            while len < 4096 && *p.add(len) != 0 { len += 1; }
        }
        Some(unsafe { core::slice::from_raw_parts(p, len) })
    }

    /// Write a value to a user pointer (8-byte atomic write).
    unsafe fn write_u64_user(ptr: u64, v: u64) {
        if ptr != 0 { unsafe { *(ptr as *mut u64) = v; } }
    }
    unsafe fn write_u32_user(ptr: u64, v: u32) {
        if ptr != 0 { unsafe { *(ptr as *mut u32) = v; } }
    }

    // ── Memory allocation ─────────────────────────────────────────────────────

    pub fn sys_malloc(size: u64) -> i64 {
        if size == 0 { return 8; } // non-null sentinel
        let (pid, pml4) = pid_and_pml4();
        if pid == 0 { return 0; }
        // Allocate size + 16 bytes (header stores size for realloc).
        let alloc_size = size as usize + 16;
        let pages = alloc_size.div_ceil(4096);
        let va = mmap_anon(pid, pml4, 0, pages, 3);
        if va == u64::MAX { return 0; }
        // Write allocation size into header (kernel can write since user VA is
        // directly accessible in our single-space model).
        unsafe { write_u64_user(va, size); }
        (va + 16) as i64  // return ptr past header
    }

    pub fn sys_free(_ptr: u64) -> i64 {
        // munmap is a stub → leak is acceptable for demo.
        0
    }

    pub fn sys_calloc(n: u64, size: u64) -> i64 {
        // mmap_anon zeroes pages, so malloc already gives zeroed memory.
        sys_malloc(n.saturating_mul(size))
    }

    pub fn sys_realloc(ptr: u64, size: u64) -> i64 {
        if ptr == 0 { return sys_malloc(size); }
        if size == 0 { sys_free(ptr); return 0; }
        // Get old size from header (ptr - 16).
        let old_size = if ptr >= 16 {
            unsafe { *((ptr - 16) as *const u64) }
        } else { 0 };
        if old_size >= size { return ptr as i64; } // already big enough
        let new_ptr = sys_malloc(size);
        if new_ptr == 0 || ptr == 0 { return new_ptr; }
        // Copy old data.
        let copy_len = old_size.min(size) as usize;
        unsafe {
            core::ptr::copy_nonoverlapping(ptr as *const u8, new_ptr as *mut u8, copy_len);
        }
        new_ptr
    }

    pub fn sys_aligned_alloc(_align: u64, size: u64) -> i64 {
        // mmap always returns page-aligned memory.
        sys_malloc(size)
    }

    pub fn sys_posix_memalign(pptr: u64, _align: u64, size: u64) -> i64 {
        let ptr = sys_malloc(size);
        unsafe { write_u64_user(pptr, ptr as u64); }
        if ptr == 0 { 12 } else { 0 } // 12 = ENOMEM
    }

    pub fn sys_malloc_usable_size(ptr: u64) -> i64 {
        if ptr < 16 { return 0; }
        let size = unsafe { *((ptr - 16) as *const u64) };
        size as i64
    }

    pub fn sys_strdup(s: u64) -> i64 {
        let len = sys_strlen(s) as u64;
        let new_ptr = sys_malloc(len + 1);
        if new_ptr == 0 { return 0; }
        sys_memcpy(new_ptr as u64, s, len + 1);
        new_ptr
    }

    pub fn sys_strndup(s: u64, n: u64) -> i64 {
        let slen = sys_strlen(s) as u64;
        let len = slen.min(n);
        let new_ptr = sys_malloc(len + 1);
        if new_ptr == 0 { return 0; }
        sys_memcpy(new_ptr as u64, s, len);
        // null-terminate
        unsafe { *((new_ptr as u64 + len) as *mut u8) = 0; }
        new_ptr
    }

    // ── String operations ─────────────────────────────────────────────────────

    pub fn sys_strlen(s: u64) -> i64 {
        if s == 0 { return 0; }
        let mut len = 0i64;
        unsafe {
            let mut p = s as *const u8;
            while *p != 0 { p = p.add(1); len += 1; if len > 1024*1024 { break; } }
        }
        len
    }

    pub fn sys_memcpy(dst: u64, src: u64, n: u64) -> i64 {
        if dst == 0 || src == 0 || n == 0 { return dst as i64; }
        unsafe { core::ptr::copy_nonoverlapping(src as *const u8, dst as *mut u8, n as usize); }
        dst as i64
    }

    pub fn sys_memset(dst: u64, val: u64, n: u64) -> i64 {
        if dst == 0 || n == 0 { return dst as i64; }
        unsafe { core::ptr::write_bytes(dst as *mut u8, val as u8, n as usize); }
        dst as i64
    }

    pub fn sys_memmove(dst: u64, src: u64, n: u64) -> i64 {
        if dst == 0 || src == 0 || n == 0 { return dst as i64; }
        unsafe { core::ptr::copy(src as *const u8, dst as *mut u8, n as usize); }
        dst as i64
    }

    pub fn sys_memcmp(a: u64, b: u64, n: u64) -> i64 {
        if n == 0 { return 0; }
        let sa = unsafe { core::slice::from_raw_parts(a as *const u8, n as usize) };
        let sb = unsafe { core::slice::from_raw_parts(b as *const u8, n as usize) };
        for i in 0..n as usize {
            let diff = sa[i] as i32 - sb[i] as i32;
            if diff != 0 { return diff as i64; }
        }
        0
    }

    pub fn sys_memchr(s: u64, c: u64, n: u64) -> i64 {
        if s == 0 || n == 0 { return 0; }
        let slice = unsafe { core::slice::from_raw_parts(s as *const u8, n as usize) };
        for (i, &b) in slice.iter().enumerate() {
            if b == c as u8 { return (s + i as u64) as i64; }
        }
        0
    }

    pub fn sys_bzero(dst: u64, n: u64) -> i64 {
        sys_memset(dst, 0, n)
    }

    pub fn sys_strcmp(a: u64, b: u64) -> i64 {
        if a == 0 || b == 0 { return if a == b { 0 } else { 1 }; }
        let mut pa = a as *const u8;
        let mut pb = b as *const u8;
        loop {
            let ca = unsafe { *pa };
            let cb = unsafe { *pb };
            if ca != cb { return (ca as i64) - (cb as i64); }
            if ca == 0 { return 0; }
            pa = unsafe { pa.add(1) };
            pb = unsafe { pb.add(1) };
        }
    }

    pub fn sys_strncmp(a: u64, b: u64, n: u64) -> i64 {
        if n == 0 { return 0; }
        let mut pa = a as *const u8;
        let mut pb = b as *const u8;
        for _ in 0..n {
            let ca = unsafe { *pa };
            let cb = unsafe { *pb };
            if ca != cb { return (ca as i64) - (cb as i64); }
            if ca == 0 { return 0; }
            pa = unsafe { pa.add(1) };
            pb = unsafe { pb.add(1) };
        }
        0
    }

    pub fn sys_strcpy(dst: u64, src: u64) -> i64 {
        if dst == 0 || src == 0 { return dst as i64; }
        let mut ps = src as *const u8;
        let mut pd = dst as *mut u8;
        loop {
            let c = unsafe { *ps };
            unsafe { *pd = c; }
            if c == 0 { break; }
            ps = unsafe { ps.add(1) };
            pd = unsafe { pd.add(1) };
        }
        dst as i64
    }

    pub fn sys_strncpy(dst: u64, src: u64, n: u64) -> i64 {
        if dst == 0 { return 0; }
        let mut ps = src as *const u8;
        let mut pd = dst as *mut u8;
        for _ in 0..n {
            let c = if src != 0 { unsafe { let v = *ps; ps = ps.add(1); v } } else { 0 };
            unsafe { *pd = c; pd = pd.add(1); }
        }
        dst as i64
    }

    pub fn sys_strcat(dst: u64, src: u64) -> i64 {
        let dlen = sys_strlen(dst) as u64;
        sys_strcpy(dst + dlen, src);
        dst as i64
    }

    pub fn sys_strncat(dst: u64, src: u64, n: u64) -> i64 {
        let dlen = sys_strlen(dst) as u64;
        sys_strncpy(dst + dlen, src, n);
        // ensure null terminator
        let total = dlen + n;
        unsafe { *((dst + total) as *mut u8) = 0; }
        dst as i64
    }

    pub fn sys_strstr(hay: u64, needle: u64) -> i64 {
        if hay == 0 || needle == 0 { return 0; }
        let nlen = sys_strlen(needle) as usize;
        if nlen == 0 { return hay as i64; }
        let hlen = sys_strlen(hay) as usize;
        if nlen > hlen { return 0; }
        let hs = unsafe { core::slice::from_raw_parts(hay as *const u8, hlen) };
        let ns = unsafe { core::slice::from_raw_parts(needle as *const u8, nlen) };
        for i in 0..=(hlen - nlen) {
            if hs[i..i+nlen] == *ns { return (hay + i as u64) as i64; }
        }
        0
    }

    pub fn sys_strchr(s: u64, c: u64) -> i64 {
        if s == 0 { return 0; }
        let mut p = s as *const u8;
        loop {
            let b = unsafe { *p };
            if b == c as u8 { return p as i64; }
            if b == 0 { return 0; }
            p = unsafe { p.add(1) };
        }
    }

    pub fn sys_strrchr(s: u64, c: u64) -> i64 {
        if s == 0 { return 0; }
        let mut p = s as *const u8;
        let mut last = 0i64;
        loop {
            let b = unsafe { *p };
            if b == c as u8 { last = p as i64; }
            if b == 0 { break; }
            p = unsafe { p.add(1) };
        }
        last
    }

    pub fn sys_strnlen(s: u64, n: u64) -> i64 {
        if s == 0 { return 0; }
        let mut len = 0u64;
        let mut p = s as *const u8;
        while len < n {
            if unsafe { *p } == 0 { break; }
            p = unsafe { p.add(1) };
            len += 1;
        }
        len as i64
    }

    pub fn sys_strcspn(s: u64, reject: u64) -> i64 {
        if s == 0 { return 0; }
        let rlen = sys_strlen(reject) as usize;
        let rs = if reject != 0 { unsafe { core::slice::from_raw_parts(reject as *const u8, rlen) } } else { &[] };
        let mut len = 0i64;
        let mut p = s as *const u8;
        loop {
            let c = unsafe { *p };
            if c == 0 { break; }
            if rs.contains(&c) { break; }
            p = unsafe { p.add(1) };
            len += 1;
        }
        len
    }

    pub fn sys_strspn(s: u64, accept: u64) -> i64 {
        if s == 0 { return 0; }
        let alen = sys_strlen(accept) as usize;
        let acc = if accept != 0 { unsafe { core::slice::from_raw_parts(accept as *const u8, alen) } } else { &[] };
        let mut len = 0i64;
        let mut p = s as *const u8;
        loop {
            let c = unsafe { *p };
            if c == 0 || !acc.contains(&c) { break; }
            p = unsafe { p.add(1) };
            len += 1;
        }
        len
    }

    pub fn sys_strcasestr(hay: u64, needle: u64) -> i64 {
        // Simple case-insensitive substring search.
        if hay == 0 || needle == 0 { return 0; }
        let hlen = sys_strlen(hay) as usize;
        let nlen = sys_strlen(needle) as usize;
        if nlen == 0 { return hay as i64; }
        if nlen > hlen { return 0; }
        let hs = unsafe { core::slice::from_raw_parts(hay as *const u8, hlen) };
        let ns = unsafe { core::slice::from_raw_parts(needle as *const u8, nlen) };
        'outer: for i in 0..=(hlen - nlen) {
            for j in 0..nlen {
                if hs[i+j].to_ascii_lowercase() != ns[j].to_ascii_lowercase() {
                    continue 'outer;
                }
            }
            return (hay + i as u64) as i64;
        }
        0
    }

    pub fn sys_strtol(s: u64, endptr: u64, base: u64) -> i64 {
        if s == 0 { return 0; }
        let bytes = unsafe { read_cstr(s) }.unwrap_or(&[]);
        let s = core::str::from_utf8(bytes).unwrap_or("").trim_start();
        let (neg, s) = if s.starts_with('-') { (true, &s[1..]) } else { (false, s) };
        let base = if base == 0 { 10 } else { base };
        let mut val: i64 = 0;
        let mut consumed = 0usize;
        for c in s.bytes() {
            let d = match c {
                b'0'..=b'9' => (c - b'0') as i64,
                b'a'..=b'f' => (c - b'a' + 10) as i64,
                b'A'..=b'F' => (c - b'A' + 10) as i64,
                _ => break,
            };
            if d >= base as i64 { break; }
            val = val * base as i64 + d;
            consumed += 1;
        }
        if endptr != 0 {
            unsafe { *(endptr as *mut u64) = s.as_ptr() as u64 + consumed as u64 + if neg { 1 } else { 0 }; }
        }
        if neg { -val } else { val }
    }

    pub fn sys_strtoul(s: u64, endptr: u64, base: u64) -> i64 {
        sys_strtol(s, endptr, base)
    }

    pub fn sys_strtoll(s: u64, endptr: u64, base: u64) -> i64 {
        sys_strtol(s, endptr, base)
    }

    pub fn sys_strtoull(s: u64, endptr: u64, base: u64) -> i64 {
        sys_strtol(s, endptr, base)
    }

    pub fn sys_atoi(s: u64) -> i64 {
        sys_strtol(s, 0, 10)
    }

    pub fn sys_rand() -> i64 {
        // Xorshift64
        let mut x = RAND_STATE.load(Ordering::Relaxed);
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        RAND_STATE.store(x, Ordering::Relaxed);
        (x & 0x7FFFFFFF) as i64
    }

    pub fn sys_srand(seed: u32) {
        RAND_STATE.store(seed as u64 | 1, Ordering::Relaxed);
    }

    // ── Threading ─────────────────────────────────────────────────────────────

    pub fn sys_pthread_self() -> i64 {
        let tid = process::current_pid();
        if tid == 0 { return 0; }

        // Fast path: already allocated for this tid.
        if let Some(&va) = PTHREAD_SELF_TABLE.lock().get(&tid) {
            return va as i64;
        }

        // Allocate a synthetic thread object in user VA and publish it.
        // Keep it larger than common pthread header offsets used by runtimes.
        let obj = sys_malloc(256) as u64;
        if obj == 0 {
            // OOM fallback: return a non-null tagged value rather than 0.
            return ((tid as u64) << 12 | 1) as i64;
        }

        unsafe {
            // Clear object and set a few self-referential fields expected by
            // pointer-based pthread_t implementations.
            core::ptr::write_bytes(obj as *mut u8, 0, 256);
            *(obj as *mut u64) = obj;              // self pointer
            *((obj + 8) as *mut u64) = tid as u64; // tid
            *((obj + 0x68) as *mut u64) = obj;     // non-null guard field
        }

        PTHREAD_SELF_TABLE.lock().insert(tid, obj);
        obj as i64
    }

    pub fn sys_pthread_mutex_init(mutex: u64) -> i64 {
        // A mutex is a 64-bit value: 0 = unlocked.
        if mutex != 0 { unsafe { *(mutex as *mut u64) = 0; } }
        0
    }

    pub fn sys_pthread_mutex_lock(mutex: u64) -> i64 {
        if mutex == 0 { return 22; } // EINVAL
        let atom = unsafe { &*(mutex as *const core::sync::atomic::AtomicU32) };
        loop {
            if atom.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire).is_ok() {
                return 0;
            }
            // Wait for the owner to clear the mutex word, then retry.
            let _ = super::sys_futex(mutex, super::FUTEX_WAIT, 1);
        }
    }

    pub fn sys_pthread_mutex_unlock(mutex: u64) -> i64 {
        if mutex == 0 { return 22; }
        let atom = unsafe { &*(mutex as *const core::sync::atomic::AtomicU32) };
        atom.store(0, Ordering::Release);
        sys_futex_wake(mutex, 1);
        0
    }

    pub fn sys_pthread_mutex_trylock(mutex: u64) -> i64 {
        if mutex == 0 { return 22; }
        let atom = unsafe { &*(mutex as *const core::sync::atomic::AtomicU32) };
        if atom.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire).is_ok() { 0 } else { 16 }
    }

    pub fn sys_pthread_once(once: u64, func: u64) -> i64 {
        if once == 0 || func == 0 { return 22; }
        let atom = unsafe { &*(once as *const core::sync::atomic::AtomicU32) };
        // State: 0=uninit, 1=in-progress, 2=done
        if atom.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire).is_ok() {
            // We won the race — call func.
            let f: extern "C" fn() = unsafe { core::mem::transmute(func) };
            f();
            atom.store(2, Ordering::Release);
        } else {
            // Spin until done.
            loop {
                let v = atom.load(Ordering::Acquire);
                if v == 2 { break; }
                unsafe { core::arch::asm!("pause", options(nomem, nostack, preserves_flags)); }
            }
        }
        0
    }

    pub fn sys_pthread_key_create(key_ptr: u64, _dtor: u64) -> i64 {
        let key = NEXT_KEY.fetch_add(1, Ordering::Relaxed);
        unsafe { write_u32_user(key_ptr, key); }
        0
    }

    pub fn sys_pthread_setspecific(key: u64, value: u64) -> i64 {
        let pid = process::current_pid();
        let tid = pid; // simplified: single thread per process
        KEY_TABLE.lock().insert((pid, tid, key as u32), value);
        0
    }

    pub fn sys_pthread_getspecific(key: u64) -> i64 {
        let pid = process::current_pid();
        let tid = pid;
        KEY_TABLE.lock().get(&(pid, tid, key as u32)).copied().unwrap_or(0) as i64
    }

    pub fn sys_pthread_cond_wait(cond: u64, mutex: u64) -> i64 {
        if cond == 0 || mutex == 0 { return 22; }
        let atom = unsafe { &*(cond as *const core::sync::atomic::AtomicU32) };
        let seq = atom.load(Ordering::Acquire);
        // Release the mutex, sleep until the condvar sequence advances, then
        // reacquire the mutex before returning.
        sys_pthread_mutex_unlock(mutex);
        let _ = super::sys_futex(cond, super::FUTEX_WAIT, seq);
        sys_pthread_mutex_lock(mutex)
    }

    pub fn sys_pthread_cond_timedwait(cond: u64, mutex: u64, _timeout: u64) -> i64 {
        sys_pthread_cond_wait(cond, mutex)
    }

    pub fn sys_pthread_cond_signal(cond: u64) -> i64 {
        if cond == 0 { return 22; }
        let atom = unsafe { &*(cond as *const core::sync::atomic::AtomicU32) };
        atom.fetch_add(1, Ordering::Release);
        sys_futex_wake(cond, 1)
    }

    pub fn sys_pthread_cond_broadcast(cond: u64) -> i64 {
        if cond == 0 { return 22; }
        let atom = unsafe { &*(cond as *const core::sync::atomic::AtomicU32) };
        atom.fetch_add(1, Ordering::Release);
        sys_futex_wake(cond, i32::MAX as u32)
    }

    pub fn sys_pthread_attr_init(attr: u64) -> i64 {
        log::error!("[trace] sys_pthread_attr_init attr={:#x}", attr);
        // pthread_attr_t is typically 56 bytes; zero it out.
        if attr != 0 { unsafe { core::ptr::write_bytes(attr as *mut u8, 0, 56); } }
        0
    }

    pub fn sys_pthread_attr_destroy(attr: u64) -> i64 {
        log::error!("[trace] sys_pthread_attr_destroy attr={:#x}", attr);
        0
    }

    pub fn sys_pthread_attr_setstacksize(attr: u64, stacksize: u64) -> i64 {
        log::error!("[trace] sys_pthread_attr_setstacksize attr={:#x} size={:#x}", attr, stacksize);
        // Store stacksize at offset 8 of the attr struct (glibc layout).
        if attr != 0 { unsafe { *((attr + 8) as *mut u64) = stacksize; } }
        0
    }

    pub fn sys_pthread_attr_setdetachstate(attr: u64, state: u64) -> i64 {
        log::error!("[trace] sys_pthread_attr_setdetachstate attr={:#x} state={}", attr, state);
        // Store detach state at offset 0.
        if attr != 0 { unsafe { *(attr as *mut u64) = state; } }
        0
    }

    pub fn sys_pthread_attr_getstack(attr: u64, base_out: u64, size_out: u64) -> i64 {
        log::error!("[trace] sys_pthread_attr_getstack attr={:#x}", attr);
        unsafe { write_u64_user(base_out, 0); write_u64_user(size_out, 0); }
        0
    }

    pub fn sys_pthread_setname_np(thread: u64, name: u64) -> i64 {
        log::error!("[trace] sys_pthread_setname_np thread={:#x} name_ptr={:#x}", thread, name);
        0
    }

    pub fn sys_pthread_attr_getter_noop(nr: u64, a0: u64, a1: u64, a2: u64) -> i64 {
        log::error!("[trace] pthread_attr/sched noop nr={:#x} a0={:#x} a1={:#x} a2={:#x}", nr, a0, a1, a2);
        0
    }

    // ── Semaphores ────────────────────────────────────────────────────────────

    pub fn sys_sem_init(sem: u64, _pshared: u64, value: u64) -> i64 {
        if sem == 0 { return 22; }
        unsafe { *(sem as *mut u32) = value as u32; }
        0
    }

    pub fn sys_sem_wait(sem: u64) -> i64 {
        if sem == 0 { return 22; }
        let atom = unsafe { &*(sem as *const core::sync::atomic::AtomicU32) };
        loop {
            let v = atom.load(Ordering::Acquire);
            if v > 0 && atom.compare_exchange(v, v - 1, Ordering::Acquire, Ordering::Acquire).is_ok() {
                break;
            }
            unsafe { core::arch::asm!("pause", options(nomem, nostack, preserves_flags)); }
        }
        0
    }

    pub fn sys_sem_trywait(sem: u64) -> i64 {
        if sem == 0 { return 22; }
        let atom = unsafe { &*(sem as *const core::sync::atomic::AtomicU32) };
        let v = atom.load(Ordering::Acquire);
        if v == 0 { return -11; }
        if atom.compare_exchange(v, v - 1, Ordering::Acquire, Ordering::Acquire).is_ok() { 0 } else { -11 }
    }

    pub fn sys_sem_post(sem: u64) -> i64 {
        if sem == 0 { return 22; }
        let atom = unsafe { &*(sem as *const core::sync::atomic::AtomicU32) };
        atom.fetch_add(1, Ordering::Release);
        0
    }

    // ── TLS ───────────────────────────────────────────────────────────────────

    pub fn sys_tls_get_addr(ti_ptr: u64) -> i64 {
        // `ti_ptr` points to a TLS descriptor: [module_id: u64, offset: u64].
        let offset = if ti_ptr != 0 {
            unsafe { *((ti_ptr + 8) as *const u64) }
        } else { 0 };
        let (pid, pml4) = pid_and_pml4();
        let tls_base = get_or_alloc_tls(pid, pml4);
        if tls_base == u64::MAX { return 0; }
        // Return base + (offset & 0x1FFFF) — keep within 128 KiB.
        (tls_base + (offset & 0x1_FFFF)) as i64
    }

    // ── GNU emulated TLS ──────────────────────────────────────────────────────
    //
    // Flutter's libflutter_engine.so is commonly built without a native TLS
    // segment (no PT_TLS).  The compiler emits calls to __emutls_get_address
    // instead of the ELF TLS descriptor protocol.
    //
    // __emutls_object layout (all fields pointer-sized, i.e. 8 bytes):
    //   +0  size   – size of the variable's storage
    //   +8  align  – required alignment (unused here, we always 8-align via malloc)
    //  +16  ptr    – pointer to allocated storage (0 if not yet allocated)
    //  +24  templ  – pointer to initializer template (NULL → zero-init)

    pub fn sys_emutls_get_address(obj: u64) -> i64 {
        if obj == 0 { return 0; }
        // Fetch current ptr (offset +16).
        let cur_ptr = unsafe { *((obj + 16) as *const u64) };
        if cur_ptr != 0 { return cur_ptr as i64; }
        // Allocate storage for this TLS variable.
        let size = unsafe { *(obj as *const u64) };
        let sz = size.max(8);
        let ptr_va = sys_malloc(sz) as u64;
        if ptr_va == 0 { return 0; }
        // Copy template or zero-init.
        let templ = unsafe { *((obj + 24) as *const u64) };
        if templ != 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    templ as *const u8,
                    ptr_va as *mut u8,
                    sz as usize,
                );
            }
        } else {
            unsafe { core::ptr::write_bytes(ptr_va as *mut u8, 0, sz as usize); }
        }
        // Persist ptr back into the object so future calls are O(1).
        unsafe { *((obj + 16) as *mut u64) = ptr_va; }
        ptr_va as i64
    }

    pub fn sys_emutls_register_common(obj: u64, size: u64, align: u64, templ: u64) -> i64 {
        if obj == 0 { return 0; }
        unsafe {
            *(obj as *mut u64)          = size;
            *((obj + 8)  as *mut u64)   = align;
            *((obj + 16) as *mut u64)   = 0;    // clear cached ptr
            *((obj + 24) as *mut u64)   = templ;
        }
        0
    }

    // ── System ────────────────────────────────────────────────────────────────

    pub fn sys_abort() -> ! {
        // Delegate to sys_exit so we never return into the dead process's
        // user code path. sys_exit performs the context switch to the next
        // runnable process (or halts if none), and is `-> i64` only because
        // it threads through the normal dispatch table; in practice it never
        // returns.
        log::error!("[sys_abort] pid={} aborting — dumping recent syscalls:",
            crate::process::current_pid());
        super::dump_recent_syscalls(32);
        super::sys_exit((-6i64) as u64);
        loop {
            unsafe { core::arch::asm!("sti; hlt; cli", options(nomem, nostack)); }
        }
    }

    pub fn sys_sysconf(name: i32) -> i64 {
        match name {
            // _SC_PAGESIZE / _SC_PAGE_SIZE
            30 | 47 => 4096,
            // _SC_NPROCESSORS_ONLN / _SC_NPROCESSORS_CONF
            84 | 83 => 2,
            // _SC_PHYS_PAGES
            85 => 131072, // 512 MiB / 4096
            // _SC_CLK_TCK
            2 => 100,
            // _SC_OPEN_MAX
            4 => 256,
            // _SC_GETPW_R_SIZE_MAX
            70 => 1024,
            // _SC_GETGR_R_SIZE_MAX
            69 => 1024,
            _ => -1,
        }
    }

    pub fn sys_nanosleep(req: u64, _rem: u64) -> i64 {
        if req == 0 { return 0; }
        // Read tv_sec from req (offset 0) and tv_nsec (offset 8).
        let secs = unsafe { *(req as *const u64) };
        // Yield for approximately secs * 1000 iterations (very rough).
        for _ in 0..(secs * 1000).min(10000) {
            unsafe { core::arch::asm!("pause", options(nomem, nostack, preserves_flags)); }
        }
        0
    }

    /// Fake monotonic time: use TSC / 1000 as nanoseconds.
    fn read_tsc() -> u64 {
        let lo: u32;
        let hi: u32;
        unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack)); }
        ((hi as u64) << 32) | lo as u64
    }

    pub fn sys_gettimeofday(tv: u64, _tz: u64) -> i64 {
        if tv == 0 { return 0; }
        // tv_sec at offset 0, tv_usec at offset 8.
        let tsc = read_tsc() / 3_000; // ~3 GHz TSC → microseconds
        let sec  = 1_700_000_000u64 + tsc / 1_000_000;
        let usec = tsc % 1_000_000;
        unsafe {
            *(tv as *mut u64)       = sec;
            *((tv + 8) as *mut u64) = usec;
        }
        0
    }

    pub fn sys_clock_gettime(clock_id: i32, tp: u64) -> i64 {
        if tp == 0 { return -22; }
        let tsc_ns = read_tsc() / 3; // ~3 GHz → nanoseconds
        let (sec, nsec) = match clock_id {
            0 | 1 => { // CLOCK_REALTIME or CLOCK_MONOTONIC
                let secs = 1_700_000_000u64 + tsc_ns / 1_000_000_000;
                let ns   = tsc_ns % 1_000_000_000;
                (secs, ns)
            }
            _ => (0, tsc_ns),
        };
        unsafe {
            *(tp as *mut u64)       = sec;
            *((tp + 8) as *mut u64) = nsec;
        }
        0
    }

    pub fn sys_time(tloc: u64) -> i64 {
        let tsc = read_tsc() / 3_000_000_000; // seconds
        let t = 1_700_000_000u64 + tsc;
        if tloc != 0 { unsafe { *(tloc as *mut u64) = t; } }
        t as i64
    }

    pub fn sys_getrusage(_who: u64, usage: u64) -> i64 {
        if usage != 0 { unsafe { core::ptr::write_bytes(usage as *mut u8, 0, 144); } }
        0
    }

    pub fn sys_getcwd(buf: u64, size: u64) -> i64 {
        let path = b"/\0";
        if buf == 0 || size < 2 { return 0; }
        unsafe { core::ptr::copy_nonoverlapping(path.as_ptr(), buf as *mut u8, 2); }
        buf as i64
    }

    pub fn sys_gethostname(buf: u64, size: u64) -> i64 {
        let name = b"oscortex\0";
        let n = name.len().min(size as usize);
        if buf == 0 { return -1; }
        unsafe { core::ptr::copy_nonoverlapping(name.as_ptr(), buf as *mut u8, n); }
        0
    }

    pub fn sys_uname(buf: u64) -> i64 {
        // struct utsname: 6 fields × 65 bytes each = 390 bytes.
        if buf == 0 { return -1; }
        unsafe { core::ptr::write_bytes(buf as *mut u8, 0, 390); }
        let copy = |off: usize, s: &[u8]| {
            let n = s.len().min(64);
            unsafe { core::ptr::copy_nonoverlapping(s.as_ptr(), (buf + off as u64) as *mut u8, n); }
        };
        copy(0,   b"Linux");
        copy(65,  b"oscortex");
        copy(130, b"6.1.0");
        copy(195, b"#1 SMP");
        copy(260, b"x86_64");
        0
    }

    /// Return a static error string for the given errno.
    pub fn sys_strerror(errnum: i32) -> i64 {
        // Embed static strings at a fixed kernel address.
        // The user gets a pointer to read-only kernel text — valid since
        // user processes can read the kernel text in our current model.
        let s: &'static [u8] = match errnum.unsigned_abs() {
            0  => b"Success\0",
            1  => b"Operation not permitted\0",
            2  => b"No such file or directory\0",
            11 => b"Resource temporarily unavailable\0",
            12 => b"Out of memory\0",
            13 => b"Permission denied\0",
            14 => b"Bad address\0",
            17 => b"File exists\0",
            19 => b"No such device\0",
            22 => b"Invalid argument\0",
            28 => b"No space left on device\0",
            38 => b"Function not implemented\0",
            _  => b"Unknown error\0",
        };
        s.as_ptr() as i64
    }

    pub fn sys_strerror_r(errnum: i32, buf: u64, n: u64) -> i64 {
        let s_ptr = sys_strerror(errnum) as u64;
        let s_len = sys_strlen(s_ptr) as u64 + 1;
        let copy = s_len.min(n);
        if buf != 0 && copy > 0 {
            sys_memcpy(buf, s_ptr, copy);
        }
        0
    }

    pub fn sys_passthrough_syscall(nr: u64, a0: u64, a1: u64, a2: u64) -> i64 {
        // Allow userspace `syscall(NR, ...)` to dispatch to our kernel.
        crate::syscall::dispatch_fast(nr, a0, a1, a2, 0, 0)
    }

    pub fn sys_getenv(_name_ptr: u64, _name_len: u64) -> i64 {
        // No environment variables in our OS.
        0 // NULL
    }

    fn ensure_locale_obj() -> u64 {
        let cur = LOCALE_OBJ.load(Ordering::Acquire);
        if cur != 0 { return cur; }

        let obj = sys_malloc(256) as u64;
        if obj == 0 { return 0; }
        unsafe {
            core::ptr::write_bytes(obj as *mut u8, 0, 256);
            *(obj as *mut u64) = obj;
            *((obj + 0x68) as *mut u64) = obj;
        }

        match LOCALE_OBJ.compare_exchange(0, obj, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => {
                LOCALE_CURRENT.store(obj, Ordering::Release);
                obj
            }
            Err(existing) => existing,
        }
    }

    pub fn sys_setlocale(_category: u64, _locale_ptr: u64) -> i64 {
        // Return pointer to a stable locale string "C".
        static C_LOCALE: &[u8] = b"C\0";
        C_LOCALE.as_ptr() as i64
    }

    pub fn sys_uselocale(locale: u64) -> i64 {
        let prev = LOCALE_CURRENT.load(Ordering::Acquire);
        // locale_t special values:
        //   0   => query current locale
        //   -1  => use global locale
        let use_ptr = if locale == 0 || locale == u64::MAX {
            ensure_locale_obj()
        } else {
            locale
        };
        if use_ptr != 0 {
            LOCALE_CURRENT.store(use_ptr, Ordering::Release);
        }
        if prev != 0 { prev as i64 } else { ensure_locale_obj() as i64 }
    }

    pub fn sys_newlocale(_mask: u64, _name_ptr: u64, base: u64) -> i64 {
        // Some runtimes pass small sentinels here that are not real locale_t
        // pointers; only trust clearly pointer-like values.
        if base >= 0x10000 { return base as i64; }
        ensure_locale_obj() as i64
    }

    pub fn sys_freelocale(_locale: u64) -> i64 {
        // Locale objects are process-lifetime for now.
        0
    }

    // ── IO ────────────────────────────────────────────────────────────────────

    pub fn sys_open64(path_ptr: u64, _flags: u64, _mode: u64, _arg3: u64) -> i64 {
        // libc open64(path, flags, mode): arg1 is flags, NOT a path length.
        // Compute strlen ourselves so sys_open can read the user path.
        if path_ptr == 0 { return -14; } // EFAULT
        let path_len = sys_strlen(path_ptr) as u64 + 1; // include NUL
        crate::syscall::dispatch_fast(2, path_ptr, path_len, 0, 0, 0)
    }

    pub fn sys_lseek64(fd: u64, offset: i64, whence: i32) -> i64 {
        crate::syscall::dispatch_fast(8, fd, offset as u64, whence as u64, 0, 0)
    }

    pub fn sys_fopen64(path_ptr: u64, path_len: u64, _mode_ptr: u64, _mode_len: u64) -> i64 {
        // Open the VFS file via sys_open and return a tiny heap-allocated FILE*.
        // FILE layout: [i64 fd][u64 size][u64 pos]  = 24 bytes
        let path_end = if path_len > 0 { path_len } else { sys_strlen(path_ptr) as u64 + 1 };
        let fd = crate::syscall::dispatch_fast(2, path_ptr, path_end, 0, 0, 0);
        if fd < 0 { return 0; } // NULL on error
        // Query file size via fstat.
        let stat_buf = sys_malloc(144 + 16);
        if stat_buf == 0 { crate::syscall::dispatch_fast(3, fd as u64, 0, 0, 0, 0); return 0; }
        crate::syscall::dispatch_fast(5, fd as u64, stat_buf as u64, 0, 0, 0); // fstat
        let size = unsafe { *((stat_buf as u64 + 48) as *const u64) }; // st_size offset=48
        sys_free(stat_buf as u64);
        // Allocate FILE struct: [fd:i64][size:u64][pos:u64]
        let fp = sys_malloc(24);
        if fp == 0 { crate::syscall::dispatch_fast(3, fd as u64, 0, 0, 0, 0); return 0; }
        unsafe {
            *((fp as u64) as *mut i64)      = fd;
            *((fp as u64 + 8) as *mut u64)  = size;
            *((fp as u64 + 16) as *mut u64) = 0; // pos
        }
        fp
    }

    pub fn sys_fclose(fp: u64) -> i64 {
        if fp == 0 { return -1; }
        let fd = unsafe { *(fp as *const i64) };
        sys_free(fp);
        crate::syscall::dispatch_fast(3, fd as u64, 0, 0, 0, 0) // sys_close
    }

    pub fn sys_fread(buf: u64, size: u64, count: u64, fp: u64) -> i64 {
        if fp == 0 || buf == 0 { return 0; }
        let fd = unsafe { *(fp as *const i64) } as u64;
        let total = size * count;
        let r = crate::syscall::dispatch_fast(0, fd, buf, total, 0, 0); // sys_read
        if r <= 0 { return 0; }
        // Update pos in FILE struct.
        unsafe { *((fp + 16) as *mut u64) += r as u64; }
        if size == 0 { 0 } else { r / size as i64 }
    }

    pub fn sys_fwrite(buf: u64, size: u64, count: u64, fp: u64) -> i64 {
        let fd: u64 = if fp == 0 { 1 } else { unsafe { *(fp as *const i64) as u64 } };
        let total = size * count;
        let r = crate::syscall::dispatch_fast(1, fd, buf, total, 0, 0);
        if r < 0 { 0 } else { r / size as i64 }
    }

    pub fn sys_fseek(fp: u64, offset: i64, whence: i32) -> i64 {
        if fp == 0 { return -1; }
        let fd = unsafe { *(fp as *const i64) } as u64;
        let new_pos = crate::syscall::dispatch_fast(8, fd, offset as u64, whence as u64, 0, 0);
        if new_pos < 0 { return -1; }
        unsafe { *((fp + 16) as *mut u64) = new_pos as u64; }
        0
    }

    pub fn sys_ftell(fp: u64) -> i64 {
        if fp == 0 { return -1; }
        let pos = unsafe { *((fp + 16) as *const u64) };
        pos as i64
    }

    pub fn sys_fgets(buf: u64, size: i32, fp: u64) -> i64 {
        if fp == 0 || buf == 0 || size <= 0 { return 0; }
        let fd = unsafe { *(fp as *const i64) } as u64;
        let cap = (size - 1).max(0) as u64;
        // Read byte by byte until newline or EOF.
        let mut n: u64 = 0;
        while n < cap {
            let mut ch: u8 = 0;
            let r = crate::syscall::dispatch_fast(0, fd, &mut ch as *mut u8 as u64, 1, 0, 0);
            if r <= 0 { break; }
            unsafe { *((buf + n) as *mut u8) = ch; }
            n += 1;
            if ch == b'\n' { break; }
        }
        unsafe { *((buf + n) as *mut u8) = 0; }
        if n == 0 { 0 } else { buf as i64 }
    }

    pub fn sys_fileno(fp: u64) -> i64 {
        if fp == 0 { return -1; }
        unsafe { *(fp as *const i64) }
    }

    pub fn sys_feof(fp: u64) -> i64 {
        if fp == 0 { return 1; }
        let fd  = unsafe { *(fp as *const i64) } as usize;
        let tbl = super::OPEN_FILES.lock();
        if fd < super::MAX_OPEN_FILES && tbl[fd].used {
            if tbl[fd].offset >= tbl[fd].data.len() as u64 { 1 } else { 0 }
        } else {
            1
        }
    }

    pub fn sys_ferror(_fp: u64) -> i64 { 0 }

    pub fn sys_clearerr(_fp: u64) -> i64 { 0 }

    pub fn sys_rewind(fp: u64) -> i64 {
        sys_fseek(fp, 0, 0 /* SEEK_SET */)
    }

    pub fn sys_fflush(_fp: u64) -> i64 { 0 }

    pub fn sys_stat(path_ptr: u64, path_len: u64, stat_ptr: u64) -> i64 {
        // The POSIX `stat()` ABI is `stat(const char *path, struct stat *buf)`.
        // When called via the libc trampoline (syscall 0x441), `arg1` is the
        // stat buffer pointer and `arg2` is unused — there is no length. Detect
        // that case by checking whether `stat_ptr` is zero, and treat
        // `path_len` as the real stat buffer.
        let (mut real_path_len, real_stat_ptr) = if stat_ptr == 0 {
            (0u64, path_len)
        } else {
            (path_len, stat_ptr)
        };
        // If no path length was supplied, compute it from the NUL terminator.
        if real_path_len == 0 && path_ptr != 0 {
            let mut len: usize = 0;
            unsafe {
                let p = path_ptr as *const u8;
                while len < 4096 && *p.add(len) != 0 { len += 1; }
            }
            real_path_len = len as u64;
        }
        // Open the file, stat it, close it.
        let fd = crate::syscall::dispatch_fast(2, path_ptr, real_path_len, 0, 0, 0);
        if fd < 0 { return -2; } // ENOENT
        let r = crate::syscall::dispatch_fast(5, fd as u64, real_stat_ptr, 0, 0, 0);
        crate::syscall::dispatch_fast(3, fd as u64, 0, 0, 0, 0);
        r
    }

    pub fn sys_snprintf(buf: u64, _size: u64, fmt_ptr: u64, first_vararg: u64, second_vararg: u64) -> i64 {
        // Debug: log the format string and (likely) first vararg (often a
        // file path or pointer-to-cstring). This is invaluable for
        // diagnosing engine-side error messages that would otherwise be
        // unreadable.
        if fmt_ptr != 0 {
            let flen = sys_strlen(fmt_ptr) as usize;
            let mut fbuf = [0u8; 192];
            let n = flen.min(fbuf.len());
            unsafe {
                core::ptr::copy_nonoverlapping(fmt_ptr as *const u8, fbuf.as_mut_ptr(), n);
            }
            let fmt_s = core::str::from_utf8(&fbuf[..n]).unwrap_or("<non-utf8>");
            // Try to read first vararg as a C string (it often is one).
            let mut vbuf = [0u8; 128];
            let mut vn = 0usize;
            if first_vararg != 0 && first_vararg > 0x1000 {
                unsafe {
                    let p = first_vararg as *const u8;
                    while vn < vbuf.len() && *p.add(vn) != 0 { vbuf[vn] = *p.add(vn); vn += 1; }
                }
            }
            let v_s = core::str::from_utf8(&vbuf[..vn]).unwrap_or("");
            // Try to read 3rd vararg from saved user r9 (typically __FUNCTION__).
            let third_vararg = crate::arch::syscall::user_r9();
            let mut wbuf = [0u8; 128];
            let mut wn = 0usize;
            if third_vararg > 0x1000 {
                unsafe {
                    let p = third_vararg as *const u8;
                    while wn < wbuf.len() && *p.add(wn) != 0 { wbuf[wn] = *p.add(wn); wn += 1; }
                }
            }
            let w_s = core::str::from_utf8(&wbuf[..wn]).unwrap_or("");
            log::warn!("[snprintf] fmt='{}' arg1='{}' (raw={:#x}) arg2={} (={:#x}) arg3='{}' (raw={:#x})", fmt_s, v_s, first_vararg, second_vararg as i64, second_vararg, w_s, third_vararg);
        }
        // Copy format string to buf (no actual formatting).
        let len = sys_strlen(fmt_ptr) as u64;
        sys_memcpy(buf, fmt_ptr, len + 1);
        len as i64
    }

    pub fn sys_printf(fmt_ptr: u64, _fmt_len: u64) -> i64 {
        let len = sys_strlen(fmt_ptr) as u64;
        crate::syscall::dispatch_fast(1, 1, fmt_ptr, len, 0, 0)
    }

    pub fn sys_puts(s: u64) -> i64 {
        let len = sys_strlen(s) as u64;
        crate::syscall::dispatch_fast(1, 1, s, len, 0, 0);
        crate::syscall::dispatch_fast(1, 1, b"\n".as_ptr() as u64, 1, 0, 0);
        0
    }

    pub fn sys_perror(msg: u64) -> i64 {
        if msg != 0 {
            let mlen = sys_strlen(msg) as u64;
            crate::syscall::dispatch_fast(1, 2, msg, mlen, 0, 0);
            crate::syscall::dispatch_fast(1, 2, b": ".as_ptr() as u64, 2, 0, 0);
        }
        // Write ENOSYS strerror.
        let s = sys_strerror(38) as u64;
        let slen = sys_strlen(s) as u64;
        crate::syscall::dispatch_fast(1, 2, s, slen, 0, 0);
        crate::syscall::dispatch_fast(1, 2, b"\n".as_ptr() as u64, 1, 0, 0);
        0
    }

    pub fn sys_fprintf(fp: u64, fmt_ptr: u64, fmt_len: u64) -> i64 {
        // Simplified: write format string as-is (no argument expansion).
        let fd: u64 = if fp == 0 || fp == 1 { 1 }
                      else if fp == 2 { 2 }
                      else { unsafe { *(fp as *const i64) as u64 } };
        let len = if fmt_len > 0 { fmt_len } else { sys_strlen(fmt_ptr) as u64 };
        crate::syscall::dispatch_fast(1, fd, fmt_ptr, len, 0, 0)
    }

    // ── Signals ───────────────────────────────────────────────────────────────

    pub fn sys_sigfillset(set: u64) -> i64 {
        // Fill all bits in sigset_t (128 bytes on Linux x86_64).
        if set != 0 { unsafe { core::ptr::write_bytes(set as *mut u8, 0xFF, 128); } }
        0
    }

    // ── Wide chars ────────────────────────────────────────────────────────────

    pub fn sys_wcslen(s: u64) -> i64 {
        // wchar_t is 4 bytes on Linux.
        if s == 0 { return 0; }
        let mut len = 0i64;
        let mut p = s as *const u32;
        unsafe { while *p != 0 { p = p.add(1); len += 1; } }
        len
    }

    pub fn sys_mbrtowc(pwc: u64, s: u64, n: u64, _ps: u64) -> i64 {
        // C locale: 1-byte encoding.
        if s == 0 { return 0; }
        if n == 0 { return -2; } // incomplete sequence
        let b = unsafe { *(s as *const u8) };
        if b == 0 {
            if pwc != 0 { unsafe { *(pwc as *mut u32) = 0; } }
            return 0;
        }
        if pwc != 0 { unsafe { *(pwc as *mut u32) = b as u32; } }
        1
    }

    pub fn sys_mbsnrtowcs(dst: u64, srcp: u64, nmc: u64, len: u64, _ps: u64) -> i64 {
        if srcp == 0 { return -1; }
        let src_ptr = unsafe { *(srcp as *const u64) };
        if src_ptr == 0 { return 0; }

        let mut src = src_ptr as *const u8;
        let mut converted: u64 = 0;
        let mut remaining = nmc;
        while remaining > 0 {
            let b = unsafe { *src };
            if b == 0 {
                unsafe { *(srcp as *mut u64) = 0; }
                break;
            }
            if dst != 0 {
                if converted >= len { break; }
                unsafe { *((dst as *mut u32).add(converted as usize)) = b as u32; }
            }
            src = unsafe { src.add(1) };
            converted += 1;
            remaining -= 1;
        }

        if unsafe { *src } != 0 {
            unsafe { *(srcp as *mut u64) = src as u64; }
        }
        converted as i64
    }

    pub fn sys_mbsrtowcs(dst: u64, srcp: u64, len: u64, ps: u64) -> i64 {
        // Unbounded input bytes for C locale path.
        sys_mbsnrtowcs(dst, srcp, u64::MAX / 2, len, ps)
    }

    pub fn sys_mbtowc(pwc: u64, s: u64, n: u64) -> i64 {
        if s == 0 { return 0; } // reset shift state
        sys_mbrtowc(pwc, s, n, 0)
    }

    pub fn sys_wcrtomb(s: u64, wc: u64, _ps: u64) -> i64 {
        // C locale: only single-byte characters.
        if s == 0 { return 1; } // state reset query
        let ch = wc as u32;
        if ch > 0xFF { return -1; }
        unsafe { *(s as *mut u8) = ch as u8; }
        1
    }

    pub fn sys_wmemchr(s: u64, wc: u64, n: u64) -> i64 {
        if s == 0 || n == 0 { return 0; }
        let target = wc as u32;
        let p = s as *const u32;
        for i in 0..(n as usize) {
            let cur = unsafe { *p.add(i) };
            if cur == target {
                return (s + (i as u64) * 4) as i64;
            }
        }
        0
    }

    // ── Futex helpers ─────────────────────────────────────────────────────────

    fn sys_futex_wake(addr: u64, count: u32) -> i64 {
        // Dispatch to the existing futex syscall.
        crate::syscall::dispatch_fast(0x39D, addr, 129, count as u64, 0, 0) // FUTEX_WAKE = 1
    }
}

// ── Dispatcher ────────────────────────────────────────────────────────────────

/// Fast SYSCALL entry path (called from assembly stub).
#[unsafe(no_mangle)]
pub extern "C" fn dispatch_fast(number: u64, arg0: u64, arg1: u64, arg2: u64, arg3: u64, arg4: u64) -> i64 {
    let user_rip = crate::arch::syscall::user_rip();
    record_syscall_trace(number, arg0, arg1, arg2, user_rip);

    // ── Stability: persist user return context into the PCB immediately on
    // every SYSCALL entry. Several syscall handlers may invoke
    // `enter_user_by_pid_noreturn` (futex yield, exec/wait, exit-to-parent,
    // exec child switch) which reloads the target PID's saved RIP/RSP from
    // p.regs. Without this persist, a process that has run for a while will
    // be re-entered at its original ELF entry point (typically 0x400000),
    // losing all forward progress. This is the foundation of cooperative
    // user-process scheduling on OSCortex v0.1.
    let cur_pid = crate::process::current_pid();
    if cur_pid != 0 {
        let user_rsp = crate::arch::syscall::user_rsp();
        crate::process::save_return_context(cur_pid, user_rip, user_rsp);
    }

    // Bootstraps a non-zero FS base for userspace if no runtime has set one
    // yet via arch_prctl(ARCH_SET_FS). This prevents null fs:offset accesses
    // in early libc/libstdc++ constructor paths. Also fires for new pthreads
    // (which start with FS_BASE cleared to 0 by enter_user_by_pid_noreturn),
    // giving each thread a distinct TLS area near its own stack top.
    if cur_pid != 0 {
        let fs = crate::arch::cpu::get_fs_base();
        if fs == 0 {
            let user_rsp = crate::arch::syscall::user_rsp();
            // Place FS near current user stack top; keep room for small offsets.
            let bootstrap_fs = user_rsp.saturating_sub(0x200);
            if bootstrap_fs != 0 {
                crate::arch::cpu::set_fs_base(bootstrap_fs);
                crate::process::set_proc_fs_base(cur_pid, bootstrap_fs);
                // Log per-pid bootstrap so we can verify each pthread gets a
                // distinct TLS area. Cap at first 8 to keep logs small.
                static FS_BOOT_COUNT: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                let n = FS_BOOT_COUNT.fetch_add(1, Ordering::AcqRel);
                if n < 8 {
                    log::warn!(
                        "[syscall] bootstrapped FS base pid={} fs={:#x} rsp={:#x}",
                        cur_pid,
                        bootstrap_fs,
                        user_rsp
                    );
                }
            }
        }
    }

    // Trace stat-family calls so we can verify Flutter's IsFile() reaches us.
    if matches!(number, 4 | 6 | 21 | 257 | 262 | 269 | 0x441 | 0x45A | 0x45B | 0x45C | 0x45D | 0x458) {
        let path_arg = match number {
            4 | 6 | 21 | 0x441 => arg0,
            _ => arg1,
        };
        let mut buf = [0u8; 96];
        let mut n = 0usize;
        if path_arg != 0 {
            unsafe {
                let p = path_arg as *const u8;
                while n < buf.len() && *p.add(n) != 0 { buf[n] = *p.add(n); n += 1; }
            }
        }
        let s = core::str::from_utf8(&buf[..n]).unwrap_or("<non-utf8>");
        log::warn!("[stat-trace] nr={:#x} path='{}'", number, s);
    }
    match number {
        // POSIX-compatible
        0   => sys_read(arg0, arg1, arg2),
        1   => sys_write(arg0, arg1, arg2),
        2   => sys_open(arg0, arg1, arg2),
        3   => sys_close(arg0),
        4   => sys_stat_path(arg0, arg1),    // Linux stat (path, statbuf)
        5   => sys_fstat(arg0, arg1),         // Linux fstat
        6   => sys_stat_path(arg0, arg1),    // Linux lstat (no symlinks)
        8   => sys_lseek(arg0, arg1 as i64, arg2), // Linux lseek
        9   => sys_mmap(arg0, arg1, arg2),   // Linux mmap (anonymous; MAP_ANON)
        10  => sys_mprotect(arg0, arg1, arg2),
        11  => sys_munmap(arg0, arg1),
        12  => 0,                            // brk stub — return 0 (= current break)
        21  => sys_access_path(arg0, arg1),  // Linux access(path, mode)
        39  => sys_getpid(),
        56  => sys_clone(arg0, arg1),        // clone(2): thread creation (CLONE_VM only)
        57  => -38,  // fork — not implemented (ENOSYS)
        59  => sys_exec(arg0, arg1),
        60  => sys_exit(arg0),
        61  => sys_waitpid(arg0, arg1, arg2),
        62  => sys_kill(arg0, arg1),
        158 => sys_arch_prctl(arg0, arg1), // Linux arch_prctl (TLS FS base)
        186 => sys_getpid(),                 // gettid → same as getpid (each thread has a slot)
        231 => sys_exit(arg0),  // exit_group
        257 => {
            // Linux openat(dirfd, path, flags, mode) — path is NUL-terminated.
            let path_ptr = arg1;
            let mut len: usize = 0;
            unsafe {
                if path_ptr != 0 {
                    let p = path_ptr as *const u8;
                    while len < 4096 && *p.add(len) != 0 { len += 1; }
                }
            }
            sys_open(path_ptr, len as u64, arg2)
        }
        262 => sys_newfstatat(arg0, arg1, arg2, arg3), // newfstatat
        269 => sys_access_path(arg1, arg2),            // faccessat(dirfd, path, mode, flags)

        // IPC
        0x200 => sys_ipc_send(arg0, arg1, arg2),
        0x201 => sys_ipc_recv(arg0, arg1),

        // Compositor (M13 private ABI)
        eabi::SYS_SURFACE_CREATE => sys_surface_create(arg0, arg1),
        eabi::SYS_SURFACE_MOVE => sys_surface_move(arg0, arg1, arg2),
        eabi::SYS_SURFACE_DESTROY => sys_surface_destroy(arg0),
        eabi::SYS_SURFACE_UPLOAD_RGBA32 => sys_surface_upload(arg0, arg1, arg2),
        eabi::SYS_SURFACE_PRESENT => sys_surface_present(arg0),
        eabi::SYS_FB_SIZE_PACKED => sys_fb_size_packed(),
        eabi::SYS_VSYNC_COUNTER => sys_vsync_counter(),
        eabi::SYS_VSYNC_WAIT_NONBLOCK => sys_vsync_wait_nonblock(arg0),
        eabi::SYS_SURFACE_OWNER => sys_surface_owner(arg0),
        eabi::SYS_SURFACE_Z_GET => sys_surface_z_get(arg0),
        eabi::SYS_SURFACE_Z_SET => sys_surface_z_set(arg0, arg1),
        eabi::SYS_SURFACE_GEOMETRY_GET => sys_surface_geometry_get(arg0),
        eabi::SYS_SURFACE_GEOMETRY_SET => sys_surface_geometry_set(arg0, arg1, arg2),
        eabi::SYS_SURFACE_VISIBILITY_GET => sys_surface_visibility_get(arg0),
        eabi::SYS_SURFACE_VISIBILITY_SET => sys_surface_visibility_set(arg0, arg1),
        eabi::SYS_SURFACE_CLIP_SET => sys_surface_clip_set(arg0, arg1, arg2),
        eabi::SYS_SURFACE_DAMAGE_SET => sys_surface_damage_set(arg0, arg1, arg2),
        eabi::SYS_SURFACE_DAMAGE_GET => sys_surface_damage_get(arg0),
        eabi::SYS_SURFACE_FLIP => sys_surface_flip(arg0),
        eabi::SYS_WM_EVENT_POLL => sys_wm_event_poll(),
        eabi::SYS_WM_EVENT_READ => sys_wm_event_read(arg0, arg1),
        eabi::SYS_WM_EVENT_INJECT => sys_wm_event_inject(arg0, arg1, arg2),
        eabi::SYS_WM_EVENT_WAIT => sys_wm_event_wait(arg0, arg1, arg2),
        eabi::SYS_EMBEDDER_ABI_VERSION => sys_embedder_abi_version(),
        eabi::SYS_WM_EVENT_SIZE => sys_wm_event_size(),
        eabi::SYS_WM_EVENT_STATS => sys_wm_event_stats_packed(),
        eabi::SYS_WM_FOCUS_PID_GET => sys_wm_focus_pid_get(),
        eabi::SYS_WM_FOCUS_SURFACE_SET => sys_wm_focus_surface_set(arg0),
        eabi::SYS_WM_FOCUS_MIRROR_GET => sys_wm_focus_mirror_get(),
        eabi::SYS_WM_FOCUS_MIRROR_SET => sys_wm_focus_mirror_set(arg0),
        eabi::SYS_APP_NOTIFY => sys_app_notify(arg0, arg1, arg2),
        eabi::SYS_PROC_SURFACE_COUNT => sys_proc_surface_count(arg0),
        eabi::SYS_APP_LAUNCH_PATH => sys_app_launch_path(arg0, arg1, arg2),
        eabi::SYS_ENGINE_POLICY_GET => sys_engine_policy_get(),
        eabi::SYS_ENGINE_VERSION_PACKED => sys_engine_version_packed(),
        eabi::SYS_ENGINE_HOST_REGISTER => sys_engine_host_register(arg0),
        eabi::SYS_ENGINE_HOST_PID_GET => sys_engine_host_pid_get(),
        eabi::SYS_ENGINE_LIBRARY_PATH_READ => sys_engine_library_path_read(arg0, arg1),

        // Phase 30 Slice 3 — kernel dynamic linker + anonymous mmap
        eabi::SYS_DLOPEN   => sys_dlopen(arg0, arg1, arg2),
        eabi::SYS_DLSYM    => sys_dlsym(arg0, arg1, arg2),
        eabi::SYS_DLCLOSE  => sys_dlclose(arg0),
        eabi::SYS_DL_GET_INIT_ARRAY => sys_dl_get_init_array(arg0, arg1, arg2, arg3),
        eabi::SYS_MMAP     => sys_mmap(arg0, arg1, arg2),
        eabi::SYS_MUNMAP   => sys_munmap(arg0, arg1),
        eabi::SYS_MPROTECT => sys_mprotect(arg0, arg1, arg2),

        // Phase 31 Slice A — FlutterEngineProcTable bridge
        eabi::SYS_ENGINE_PROCTABLE_SET     => sys_engine_proctable_set(arg0, arg1),
        eabi::SYS_ENGINE_PROCTABLE_PTR_GET => sys_engine_proctable_ptr_get(),
        eabi::SYS_ENGINE_VSYNC_BATON_POST  => sys_engine_vsync_baton_post(arg0),

        // Phase 31 Slice B — GPU fast path
        eabi::SYS_GPU_SUBMIT => sys_gpu_submit(arg0, arg1, arg2),

        // Phase 32-C — platform channel
        eabi::SYS_PLATFORM_MSG_POST  => sys_platform_msg_post(arg0, arg1, arg2, arg3),
        eabi::SYS_PLATFORM_MSG_RECV  => sys_platform_msg_recv(arg0, arg1),
        eabi::SYS_PLATFORM_MSG_REPLY => sys_platform_msg_reply(arg0, arg1, arg2),
        eabi::SYS_PLATFORM_MSG_ACK   => sys_platform_msg_ack(arg0, arg1, arg2),

        // Phase 32-D — stride-aware GPU blit
        eabi::SYS_GPU_SUBMIT_STRIDED => sys_gpu_submit_strided(arg0, arg1, arg2),

        // Phase 33-A — vsync rate control
        eabi::SYS_VSYNC_SET_HZ => sys_vsync_set_hz(arg0),

        // Phase 34-C — Dart AOT snapshot loader
        eabi::SYS_AOT_SNAPSHOT_LOAD => sys_aot_snapshot_load(arg0, arg1, arg2, arg3),

        // Phase 35 — Dart isolate lifecycle
        eabi::SYS_ISOLATE_SPAWN => sys_isolate_spawn(arg0, arg1, arg2, arg3),
        eabi::SYS_ISOLATE_KILL  => sys_isolate_kill(arg0),
        eabi::SYS_ISOLATE_CTRL  => sys_isolate_ctrl(arg0, arg1),

        // Phase 36 — Dart isolate message passing
        eabi::SYS_ISOLATE_MSG_SEND    => sys_isolate_msg_send(arg0, arg1, arg2),
        eabi::SYS_ISOLATE_MSG_RECV    => sys_isolate_msg_recv(arg0, arg1, arg2),
        eabi::SYS_ISOLATE_MSG_PENDING => sys_isolate_msg_pending(arg0),

        // Phase 37 — PS/2 input device query
        eabi::SYS_INPUT_DEV_COUNT => sys_input_dev_count(),
        eabi::SYS_INPUT_DEV_INFO  => sys_input_dev_info(arg0),

        // Phase 38 — .oscapp app registry
        eabi::SYS_APP_INSTALL   => sys_app_install(arg0, arg1, arg2),
        eabi::SYS_APP_LIST      => sys_app_list(arg0, arg1),
        eabi::SYS_APP_LAUNCH    => sys_app_launch(arg0, arg1),
        eabi::SYS_APP_UNINSTALL => sys_app_uninstall(arg0),

        // Phase 39 — Named port IPC namespace
        eabi::SYS_PORT_BIND   => sys_port_bind(arg0, arg1, arg2),
        eabi::SYS_PORT_LOOKUP => sys_port_lookup(arg0, arg1, arg2, arg3),
        eabi::SYS_PORT_UNBIND => sys_port_unbind(arg0, arg1),

        // Phase 41 — USB host-controller query
        eabi::SYS_USB_CONTROLLER_COUNT => sys_usb_controller_count(),

        // Phase 42 — Framebuffer map + WM event dequeue
        eabi::SYS_FB_MAP        => sys_fb_map(arg0),
        eabi::SYS_WM_NEXT_EVENT => sys_wm_next_event(arg0),

        // Phase 43 — VFS query (no open-fd)
        eabi::SYS_VFS_LIST      => sys_vfs_list(arg0, arg1, arg2, arg3),
        eabi::SYS_VFS_READ      => sys_vfs_read(arg0, arg1, arg2, arg3),

        // Phase 44 — Writable ramdisk (/tmp/)
        eabi::SYS_VFS_WRITE     => sys_vfs_write(arg0, arg1, arg2, arg3),
        eabi::SYS_VFS_STAT      => sys_vfs_stat(arg0, arg1, arg2),

        // Phase 45 — virtio-net networking
        eabi::SYS_NET_INFO      => sys_net_info(arg0, arg1),
        eabi::SYS_NET_SEND      => sys_net_send(arg0, arg1, arg2, arg3),
        eabi::SYS_NET_RECV      => sys_net_recv(arg0, arg1, arg2, arg3),

        // Phase 46 — Compositor bypass + full-screen surface
        eabi::SYS_FB_RELEASE    => sys_fb_release(),
        eabi::SYS_SURFACE_FULLSCREEN => sys_surface_fullscreen(),

        // Phase 47 — exec + blocking wait (cooperative exec round-trip)
        0x382 => sys_exec_wait(arg0, arg1),

        // Phase 48 — UART 16550 serial console
        0x383 => sys_serial_read(arg0, arg1),
        0x384 => sys_serial_write(arg0, arg1),

        // Phase 49 — virtio-blk block device
        0x385 => sys_blk_info(arg0),
        0x386 => sys_blk_read(arg0, arg1, arg2, arg3),
        0x387 => sys_blk_write(arg0, arg1, arg2, arg3),

        // Phase 50 — TCP/IP (smoltcp)
        0x388 => sys_tcp_connect(arg0, arg1),
        0x389 => sys_tcp_write(arg0, arg1, arg2),
        0x38A => sys_tcp_read(arg0, arg1, arg2),
        0x38B => sys_tcp_close(arg0),
        0x38C => sys_dhcp_discover(),

        // Phase 51 — ext2 read-only filesystem
        0x38D => sys_ext2_mount(),
        0x38E => sys_ext2_ls(arg0, arg1, arg2, arg3),
        0x38F => sys_ext2_read(arg0, arg1, arg2, arg3),

        // Phase 53 — preemptive scheduler extras
        0x390 => sys_sched_yield(),
        0x391 => sys_get_cpu_time(arg0),

        // Phase 54 — fork
        0x392 => sys_fork(),

        // Phase 55 — signals
        0x393 => sys_kill_signal(arg0, arg1),
        0x394 => sys_sigaction(arg0, arg1),
        0x395 => sys_sigreturn(),

        // Phase 60 — pty/tty
        0x396 => sys_pty_open(arg0),
        0x397 => sys_pty_read(arg0, arg1, arg2),
        0x398 => sys_pty_write(arg0, arg1, arg2),
        0x399 => sys_pty_ioctl(arg0, arg1, arg2),

        // Phase 59 — NVMe
        0x39A => sys_nvme_info(arg0, arg1),
        0x39B => sys_nvme_read(arg0, arg1, arg2, arg3),
        0x39C => sys_nvme_write(arg0, arg1, arg2, arg3),

        // Phase 62 — Futex (needed by pthreads / Flutter Dart VM)
        0x39D => sys_futex(arg0, arg1 as u32, arg2 as u32),

        // Phase 31 Slice C — threads
        eabi::SYS_THREAD_CREATE => sys_thread_create(arg0, arg1, arg2, arg3),
        eabi::SYS_THREAD_EXIT   => sys_thread_exit(arg0),
        eabi::SYS_THREAD_JOIN   => sys_thread_join(arg0),

        // ── POSIX compat shim (0x3A0+) ───────────────────────────────────
        // Memory
        0x3A0 => posix::sys_malloc(arg0),
        0x3A1 => posix::sys_free(arg0),
        0x3A2 => posix::sys_calloc(arg0, arg1),
        0x3A3 => posix::sys_realloc(arg0, arg1),
        0x3A4 => posix::sys_aligned_alloc(arg0, arg1),
        0x3A5 => posix::sys_posix_memalign(arg0, arg1, arg2),
        0x3A6 => posix::sys_malloc_usable_size(arg0),
        0x3A7 => posix::sys_strdup(arg0),
        0x3A8 => posix::sys_strndup(arg0, arg1),
        // Strings
        0x3B0 => posix::sys_strlen(arg0),
        0x3B1 => posix::sys_memcpy(arg0, arg1, arg2),
        0x3B2 => posix::sys_memset(arg0, arg1, arg2),
        0x3B3 => posix::sys_memmove(arg0, arg1, arg2),
        0x3B4 => posix::sys_memcmp(arg0, arg1, arg2),
        0x3B5 => posix::sys_memchr(arg0, arg1, arg2),
        0x3B6 => posix::sys_bzero(arg0, arg1),
        0x3B7 => posix::sys_strcmp(arg0, arg1),
        0x3B8 => posix::sys_strncmp(arg0, arg1, arg2),
        0x3B9 => posix::sys_strcpy(arg0, arg1),
        0x3BA => posix::sys_strncpy(arg0, arg1, arg2),
        0x3BB => posix::sys_strcat(arg0, arg1),
        0x3BC => posix::sys_strncat(arg0, arg1, arg2),
        0x3BD => posix::sys_strstr(arg0, arg1),
        0x3BE => posix::sys_strchr(arg0, arg1),
        0x3BF => posix::sys_strrchr(arg0, arg1),
        0x3C0 => posix::sys_strnlen(arg0, arg1),
        0x3C1 => posix::sys_strcspn(arg0, arg1),
        0x3C2 => posix::sys_strspn(arg0, arg1),
        0x3C3 => 0, // strtok_r stub
        0x3C4 => posix::sys_strcasestr(arg0, arg1),
        0x3C5 => posix::sys_strtol(arg0, arg1, arg2),
        0x3C6 => posix::sys_strtoul(arg0, arg1, arg2),
        0x3C7 => posix::sys_strtoll(arg0, arg1, arg2),
        0x3C8 => posix::sys_strtoull(arg0, arg1, arg2),
        0x3C9 => 0, // strtod → 0.0 (integer path)
        0x3CA => posix::sys_atoi(arg0),
        0x3CB => 0, // qsort stub (noop)
        0x3CC => posix::sys_rand(),
        0x3CD => { posix::sys_srand(arg0 as u32); 0 },
        // Threading
        0x3D0 => 0, // pthread_detach noop
        0x3D1 => posix::sys_pthread_self(),
        0x3D2 => posix::sys_pthread_mutex_init(arg0),
        0x3D3 => posix::sys_pthread_mutex_lock(arg0),
        0x3D4 => posix::sys_pthread_mutex_unlock(arg0),
        0x3D5 => 0, // pthread_mutex_destroy noop
        0x3D6 => posix::sys_pthread_mutex_trylock(arg0),
        0x3D7 => posix::sys_pthread_once(arg0, arg1),
        0x3D8 => posix::sys_pthread_key_create(arg0, arg1),
        0x3D9 => 0, // pthread_key_delete noop
        0x3DA => posix::sys_pthread_setspecific(arg0, arg1),
        0x3DB => posix::sys_pthread_getspecific(arg0),
        0x3DC => posix::sys_pthread_mutex_init(arg0), // cond_init: zero the struct
        0x3DD => posix::sys_pthread_cond_wait(arg0, arg1),
        0x3DE => posix::sys_pthread_cond_signal(arg0),
        0x3DF => posix::sys_pthread_cond_broadcast(arg0),
        0x3E0 => 0, // cond_destroy noop
        0x3E1 => posix::sys_pthread_cond_timedwait(arg0, arg1, arg2),
        0x3E2 => posix::sys_pthread_mutex_lock(arg0),   // rwlock_rdlock
        0x3E3 => posix::sys_pthread_mutex_lock(arg0),   // rwlock_wrlock
        0x3E4 => posix::sys_pthread_mutex_unlock(arg0), // rwlock_unlock
        0x3E5 => posix::sys_pthread_mutex_init(arg0),   // rwlock_init
        0x3E6 => 0, // rwlock_destroy
        0x3E7 => posix::sys_pthread_attr_init(arg0),
        0x3E8 => posix::sys_pthread_attr_destroy(arg0),
        0x3E9 => posix::sys_pthread_attr_setstacksize(arg0, arg1),
        0x3EA => posix::sys_pthread_attr_setdetachstate(arg0, arg1),
        0x3EB => posix::sys_pthread_attr_getstack(arg0, arg1, arg2),
        0x3EC..=0x3EE => 0, // mutexattr_* noops
        0x3EF..=0x3F1 => 0, // condattr_* noops
        0x3F2 => 0, // pthread_sigmask noop
        0x3F3 => 0, // pthread_kill noop
        0x3F4 => posix::sys_pthread_setname_np(arg0, arg1),
        0x3F5..=0x3F7 => posix::sys_pthread_attr_getter_noop(number, arg0, arg1, arg2),
        // Semaphores
        0x3F8 => posix::sys_sem_init(arg0, arg1, arg2),
        0x3F9 => 0, // sem_destroy noop
        0x3FA => posix::sys_sem_wait(arg0),
        0x3FB => posix::sys_sem_trywait(arg0),
        0x3FC => posix::sys_sem_post(arg0),
        // TLS
        0x3FD => posix::sys_tls_get_addr(arg0),
        // System
        0x400 => { posix::sys_abort(); },
        0x401 => 4096, // getpagesize
        0x402 => posix::sys_sysconf(arg0 as i32),
        0x403 => posix::sys_nanosleep(arg0, arg1),
        0x404 => posix::sys_gettimeofday(arg0, arg1),
        0x405 => posix::sys_clock_gettime(arg0 as i32, arg1),
        0x406 => posix::sys_time(arg0),
        0x407 => 0, // wait stub
        0x408 => posix::sys_getrusage(arg0, arg1),
        0x409 => posix::sys_getcwd(arg0, arg1),
        0x40A => posix::sys_gethostname(arg0, arg1),
        0x40B..=0x40D => 0, // prctl, setsid, setpriority stubs
        0x40E => posix::sys_uname(arg0),
        0x40F => posix::sys_strerror(arg0 as i32),
        0x410 => posix::sys_strerror_r(arg0 as i32, arg1, arg2),
        0x411 => posix::sys_passthrough_syscall(arg0, arg1, arg2, arg3),
        0x412 => 0, // madvise noop
        0x413 => 0, // dladdr stub
        0x414 => 0, // dlerror → NULL (no error)
        0x415 => posix::sys_getenv(arg0, arg1),
        0x416 => posix::sys_setlocale(arg0, arg1),
        0x417 => posix::sys_uselocale(arg0),
        0x418 => posix::sys_newlocale(arg0, arg1, arg2),
        0x419 => posix::sys_freelocale(arg0),
        0x41A..=0x41C => 0, // cat* stubs
        0x41D => 0, // __cxa_atexit noop
        // IO (most are stubs)
        0x420 => posix::sys_open64(arg0, arg1, arg2, arg3),
        0x421 => posix::sys_lseek64(arg0, arg1 as i64, arg2 as i32),
        0x422 => posix::sys_fopen64(arg0, arg1, arg2, arg3),
        0x423 => posix::sys_fclose(arg0),
        0x424 => posix::sys_fread(arg0, arg1, arg2, arg3),
        0x425 => posix::sys_fwrite(arg0, arg1, arg2, arg3),
        0x426 => posix::sys_fseek(arg0, arg1 as i64, arg2 as i32),
        0x427 => posix::sys_ftell(arg0),
        0x428 => posix::sys_feof(arg0),
        0x429 => posix::sys_ferror(arg0),
        0x42A => posix::sys_fflush(arg0),
        0x42B => posix::sys_fgets(arg0, arg1 as i32, arg2),
        0x42C => 0,                    // fputc stub
        0x42D => posix::sys_puts(arg0), // fputs(str, FILE*) -> emit string
        0x42E..=0x431 => 0,            // fputwc/getwc/ungetc/ungetwc stubs
        0x432 => posix::sys_fprintf(arg0, arg1, arg2),
        0x433 => posix::sys_fprintf(arg0, arg1, arg2), // vfprintf alias
        0x434 => posix::sys_snprintf(arg0, arg1, arg2, arg3, arg4),
        0x435 => posix::sys_snprintf(arg0, arg1, arg2, arg3, arg4), // vsnprintf alias
        0x436 => posix::sys_snprintf(arg0, arg1, arg2, arg3, arg4), // sprintf alias
        0x437 => posix::sys_printf(arg0, arg1),
        0x438 => posix::sys_puts(arg0),
        0x439 => posix::sys_perror(arg0),
        0x43A => 0, // vasprintf stub
        0x43B..=0x43D => 0, // isoc99_*scanf stubs
        0x43E => posix::sys_fileno(arg0),
        0x43F => posix::sys_rewind(arg0),
        0x440 => posix::sys_clearerr(arg0),
        0x441 => posix::sys_stat(arg0, arg1, arg2),
        0x442..=0x44F => 0, // fdopen/opendir/closedir etc stubs
        0x44A..=0x459 => 0, // filesystem stubs all return 0
        0x45A => sys_fstat(arg1, arg2),         // __fxstat64(ver, fd, stat_ptr)
        0x45B => 0,                             // __fxstatat64 stub
        0x45C => posix::sys_stat(arg1, 0, arg2), // __lxstat64(ver, path, stat_ptr)
        0x45D => posix::sys_stat(arg1, 0, arg2), // __xstat64(ver, path, stat_ptr)
        // Network — all return -ENOSYS (no network stack from POSIX syscalls)
        0x460..=0x477 => -38,
        // Epoll/inotify — return fake fd 5 for epoll_create, -1 for others
        0x478 | 0x479 => 5, // epoll_create/create1 → fake fd 5
        0x47A => 0,         // epoll_ctl → success
        0x47B => 0,         // epoll_wait → 0 events (timeout)
        0x47C => 5,         // inotify_init1 → fake fd
        0x47D => 1,         // inotify_add_watch → watch descriptor 1
        0x47E => 0,         // inotify_rm_watch
        0x47F => 5,         // timerfd_create → fake fd
        0x480 => 0,         // timerfd_settime → success
        0x481 => 0,         // poll → 0 events
        0x482 => 0,         // ioctl → success
        0x483 => 0, // sigemptyset: zero the set (ptr in arg0)
        0x484 => 0, // sigaddset noop
        0x485 => posix::sys_sigfillset(arg0),
        0x486..=0x487 => 0, // setjmp/longjmp stubs
        0x488 => posix::sys_wcslen(arg0),
        0x489 => posix::sys_mbrtowc(arg0, arg1, arg2, arg3),
        0x48A => posix::sys_mbsnrtowcs(arg0, arg1, arg2, arg3, 0),
        0x48B => posix::sys_mbsrtowcs(arg0, arg1, arg2, arg3),
        0x48C => posix::sys_mbtowc(arg0, arg1, arg2),
        0x48D => posix::sys_wcrtomb(arg0, arg1, arg2),
        0x48E => posix::sys_wmemchr(arg0, arg1, arg2),
        0x48F..=0x494 => 0, // locale string conversion stubs
        0x495..=0x499 => 0, // tcgetattr/tcsetattr/tzset/localtime_r/dup3 stubs
        0x49A => -1, // execvp → ENOEXEC
        0x49B => 0, // syslog noop
        0x49C..=0x4A2 => 0, // ilogbf/modff/nextafterf/scalbnf/llround/llroundf/remainder stubs

    // GNU emulated TLS
    0x4B0 => posix::sys_emutls_get_address(arg0),
    0x4B1 => posix::sys_emutls_register_common(arg0, arg1, arg2, arg3),

        // Power
        0xC0 => sys_poweroff(),

        // Cortex PID-0 API
        0x1000..=0x100F => crate::cortex::pid0::dispatch(number, arg0, arg1, arg2),

        // Unknown
        _ => -38, // ENOSYS
    }
}

/// Legacy INT 0x80 syscall path.
pub fn dispatch_legacy() {
    // TODO: read registers from saved context and dispatch.
}

