use super::wm_consumer_pid;
use super::fd::{read_user_bytes, write_user_bytes};
use crate::syscall::state::{ENGINE_HOST_PID, ENGINE_PROC_TABLE_PTR, FS_BOOTSTRAP_LOGGED, WM_WAITER_DEADLINE, WM_WAITER_PID};
use crate::embedder::abi as eabi;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ── Phase 30 Slice 3: kernel dynamic linker + anonymous mmap ─────────────────

pub(crate) fn sys_dlopen(path_ptr: u64, path_len: u64, _flags: u64) -> i64 {
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

pub(crate) fn sys_dlsym(handle: u64, name_ptr: u64, name_len: u64) -> i64 {
    if name_ptr == 0 { return 0; } // POSIX: NULL on bad args
    // The libc-ABI `dlsym(handle, name)` trampoline only passes 2 args, so
    // `name_len` here is garbage from the caller's rdx. If it's zero or
    // unreasonable, fall back to strlen on the C-string. dlsym MUST return
    // NULL (0) on any failure — never a kernel errno like -14, which the
    // engine would mistake for a valid pointer.
    let effective_len = if name_len == 0 || name_len > 0x1000 {
        crate::syscall::posix::sys_strlen(name_ptr) as usize
    } else {
        name_len as usize
    };
    let name = match unsafe { read_user_bytes(name_ptr, effective_len) } {
        Some(b) => b,
        None => return 0, // not found
    };
    match crate::process::dl::dlsym(handle as u32, name) {
        Some(addr) => addr as i64,
        None       => 0, // POSIX: NULL (0) means not found
    }
}

pub(crate) fn sys_dlclose(handle: u64) -> i64 {
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
pub(crate) fn sys_dl_get_init_array(handle: u64, out_init_fn: u64, out_array_va: u64, out_count: u64) -> i64 {
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

pub(crate) fn sys_mmap(hint_va: u64, size: u64, prot: u64) -> i64 {
    if size == 0 || size > 0x1000_0000 {
        return -22; // EINVAL, max 256 MiB
    }
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

pub(crate) fn sys_munmap(va: u64, size: u64) -> i64 {
    if size == 0 { return 0; }
    let pid = crate::process::current_pid();
    if pid == 0 { return -1; } // EPERM
    let pml4_phys = match crate::process::get_user_context(pid) {
        Some(ctx) => ctx.pml4_phys,
        None => return -9, // EBADF
    };
    crate::mm::paging::unmap_user_range(pml4_phys, va, size);
    0
}

pub(crate) fn sys_mprotect(va: u64, size: u64, prot: u64) -> i64 {
    if size == 0 { return 0; }
    let start_va = va & !0xFFF;
    let end_va = (va + size + 4095) & !0xFFF;
    let pid = crate::process::current_pid();
    if pid == 0 { return -1; } // EPERM
    let pml4_phys = match crate::process::get_user_context(pid) {
        Some(ctx) => ctx.pml4_phys,
        None => return -9, // EBADF
    };
    let writable = (prot & 0x2) != 0;
    let exec = (prot & 0x4) != 0;

    let mut curr_va = start_va;
    while curr_va < end_va {
        unsafe {
            if crate::mm::paging::update_user_page(pml4_phys, curr_va, writable, exec).is_err() {
                // If the range contains unmapped areas, mprotect on POSIX returns ENOMEM (-12)
                return -12;
            }
        }
        curr_va += 4096;
    }
    0
}

// ── Phase 31 Slice A: FlutterEngineProcTable bridge ──────────────────────────

/// Store the user-space VA of the embedder's `FlutterEngineProcTable` so any
/// kernel subsystem (or a second process via `sys_engine_proctable_ptr_get`)
/// can resolve engine entry points without a second dlsym round-trip.
pub(crate) fn sys_engine_proctable_set(ptr: u64, size: u64) -> i64 {
    // Basic sanity: pointer must be non-null and struct must be large enough.
    if ptr == 0 || (size as usize) < core::mem::size_of::<eabi::FlutterEngineProcTable>() {
        return -22; // EINVAL
    }
    ENGINE_PROC_TABLE_PTR.store(ptr, Ordering::Release);
    0
}

/// Return the previously registered proc-table VA, or 0 if none.
pub(crate) fn sys_engine_proctable_ptr_get() -> i64 {
    ENGINE_PROC_TABLE_PTR.load(Ordering::Acquire) as i64
}

/// Record a vsync baton posted by the embedder.
///
/// Flow: engine calls `vsync_callback(user_data, baton)` → embedder calls
/// `sys_engine_vsync_baton_post(baton)` → kernel stores it → on next vsync
/// the EV_VSYNC event carries `b = baton` → embedder reads it and calls
/// `FlutterEngineOnVsync(engine, baton, start_ns, target_ns)`.
pub(crate) fn sys_engine_vsync_baton_post(baton: u64) -> i64 {
    static VSYNC_BATON_POST_LOG: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    let n = VSYNC_BATON_POST_LOG.fetch_add(1, Ordering::Relaxed);
    if n < 32 || n % 256 == 0 {
        log::warn!(
            "[vsync-baton-post] #{} pid={} baton={:#x}",
            n,
            crate::process::current_pid(),
            baton
        );
    }
    crate::wm::set_vsync_baton(baton);
    // Embedder must run FlutterEngineOnVsync before the next push_vsync consumes
    // this baton — keep pid=1 runnable even when engine threads are spinning.
    if baton != 0 {
        crate::process::set_state(1, crate::process::ProcState::Running);
    }
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
pub(crate) fn sys_gpu_submit(surface_id: u64, pixel_ptr: u64, pixel_len: u64) -> i64 {
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
pub(crate) fn sys_platform_msg_post(ch_ptr: u64, ch_len: u64, data_ptr: u64, data_len: u64) -> i64 {
    static PLATFORM_POST_LOG: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    let channel = match unsafe { read_user_bytes(ch_ptr, ch_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    let payload = match unsafe { read_user_bytes(data_ptr, data_len as usize) } {
        Some(b) => b,
        None => return -14,
    };
    let pid = crate::process::current_pid();
    let k = PLATFORM_POST_LOG.fetch_add(1, Ordering::Relaxed);
    if k < 16 {
        log::info!(
            "[platform-post] #{} pid={} ch_len={} payload_len={}",
            k,
            pid,
            channel.len(),
            payload.len()
        );
    }
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
pub(crate) fn sys_platform_msg_recv(buf_ptr: u64, buf_len: u64) -> i64 {
    static PLATFORM_RECV_LOG: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    if buf_ptr == 0 || buf_len == 0 {
        return -14; // EFAULT
    }
    // Allocate a kernel-side buffer, fill via platform_channel, then copy out.
    let len = buf_len as usize;
    let mut kbuf: alloc::vec::Vec<u8> = alloc::vec![0u8; len];
    let written = crate::platform_channel::recv_into(&mut kbuf);
    let k = PLATFORM_RECV_LOG.fetch_add(1, Ordering::Relaxed);
    if k < 16 {
        log::info!(
            "[platform-recv] #{} pid={} req_buf_len={} wrote={}",
            k,
            crate::process::current_pid(),
            len,
            written
        );
    }
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
pub(crate) fn sys_platform_msg_reply(seq: u64, data_ptr: u64, data_len: u64) -> i64 {
    static PLATFORM_REPLY_LOG: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    let payload = match unsafe { read_user_bytes(data_ptr, data_len as usize) } {
        Some(b) => b,
        None => return -14,
    };
    let k = PLATFORM_REPLY_LOG.fetch_add(1, Ordering::Relaxed);
    if k < 16 {
        log::info!(
            "[platform-reply] #{} pid={} seq={} payload_len={}",
            k,
            crate::process::current_pid(),
            seq,
            payload.len()
        );
    }
    match crate::platform_channel::reply(seq, payload) {
        Ok(()) => 0,
        Err(_) => -3, // ESRCH
    }
}

/// Copy a reply (previously set via sys_platform_msg_reply) into buf and
/// remove the message from the queue.  Returns byte count or 0 if not ready.
/// `arg0` = seq, `arg1` = buf_ptr, `arg2` = buf_len
pub(crate) fn sys_platform_msg_ack(seq: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    static PLATFORM_ACK_LOG: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    if buf_ptr == 0 || buf_len == 0 {
        return -14;
    }
    let len = buf_len as usize;
    let mut kbuf: alloc::vec::Vec<u8> = alloc::vec![0u8; len];
    let n = crate::platform_channel::ack(seq, &mut kbuf);
    let k = PLATFORM_ACK_LOG.fetch_add(1, Ordering::Relaxed);
    if k < 16 {
        log::info!(
            "[platform-ack] #{} pid={} seq={} req_buf_len={} wrote={}",
            k,
            crate::process::current_pid(),
            seq,
            len,
            n
        );
    }
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
pub(crate) fn sys_gpu_submit_strided(surface_id: u64, pixel_ptr: u64, row_bytes: u64) -> i64 {
    // Derive total byte length from the surface dimensions.
    let (width, height) = match crate::compositor::surface_geometry_get(surface_id as u32) {
        Some((_, _, w, h)) => (w as usize, h as usize),
        None => return -3, // ESRCH
    };
    let buf_len = if row_bytes == 0 {
        width * height * 4
    } else {
        row_bytes as usize * height
    };
    let bytes = match unsafe { read_user_bytes(pixel_ptr, buf_len) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    let pid = crate::process::current_pid();
    match crate::compositor::gpu_submit_strided_for(pid, surface_id as u32, bytes, row_bytes as usize) {
        Ok(()) => {
            static GPU_SUBMIT_LOG: AtomicU32 = AtomicU32::new(0);
            let n = GPU_SUBMIT_LOG.fetch_add(1, Ordering::Relaxed);
            if n < 4 {
                let sample = if bytes.len() >= 4 {
                    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
                } else {
                    0
                };
                log::warn!(
                    "[frame-boundary] gpu_submit_strided #{} pid={} sid={} ptr={:#x} row_bytes={} sample={:#x}",
                    n,
                    pid,
                    surface_id,
                    pixel_ptr,
                    row_bytes,
                    sample
                );
            }
            0
        }
        Err("bad payload size") => -22,
        Err("permission denied") => -1,
        Err("no such surface") => -3,
        Err(e) => {
            static GPU_SUBMIT_ERR: AtomicU32 = AtomicU32::new(0);
            let n = GPU_SUBMIT_ERR.fetch_add(1, Ordering::Relaxed);
            if n < 8 {
                log::warn!(
                    "[gpu_submit_strided-err] #{} pid={} sid={} err={}",
                    n,
                    pid,
                    surface_id,
                    e
                );
            }
            -12
        }
    }
}

// ── Phase 33-A: vsync rate control ───────────────────────────────────────────

/// Set the hardware vsync rate.  `hz` must be 1–240.
/// Accepted values: 30, 60, 90, 120, 144, 240.  Any value in range is stored.
pub(crate) fn sys_vsync_set_hz(hz: u64) -> i64 {
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
pub(crate) fn sys_aot_snapshot_load(
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
    // Validate: accept ELF magic (AOT shared objects) or raw Dart snapshot
    // magic. Flutter JIT VM/isolate snapshots start with F5 F5 DC DC (the
    // little-endian Snapshot::kMagicValue = 0xDCDCF5F5); some older variants
    // use DC DC DC DC. Accept either to keep this loader format-agnostic.
    let valid = data.len() >= 4
        && (&data[..4] == b"\x7fELF"
            || data[..4] == [0xF5, 0xF5, 0xDC, 0xDC]
            || data[..4] == [0xDC, 0xDC, 0xDC, 0xDC]);
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
pub(crate) fn sys_isolate_spawn(aot_va: u64, aot_size: u64, entry_offset: u64, stack_size: u64) -> i64 {
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
pub(crate) fn sys_isolate_kill(id: u64) -> i64 {
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
pub(crate) fn sys_isolate_ctrl(id: u64, op: u64) -> i64 {
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
pub(crate) fn sys_isolate_msg_send(dst_id: u64, data_ptr: u64, data_len: u64) -> i64 {
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
pub(crate) fn sys_isolate_msg_recv(isolate_id: u64, buf_ptr: u64, buf_len: u64) -> i64 {
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
pub(crate) fn sys_isolate_msg_pending(isolate_id: u64) -> i64 {
    crate::isolate_msg::pending(isolate_id as u32) as i64
}

// ── Phase 37 — PS/2 input device query ────────────────────────────────────────

/// Return the number of detected input devices (keyboard + mouse).
pub(crate) fn sys_input_dev_count() -> i64 {
    crate::drivers::ps2::device_count() as i64
}

/// Return packed device descriptor for device index `n` (0-based).
/// Bit layout: bits[3:0]=type(1=kbd,2=mouse), bits[11:4]=IRQ, bits[15:12]=iface(0=PS/2).
/// Returns 0 if `n` is out of range.
pub(crate) fn sys_input_dev_info(n: u64) -> i64 {
    crate::drivers::ps2::device_info_packed(n as u32) as i64
}

// ── Phase 38 — .osx app registry ─────────────────────────────────────────────

/// Install a `.osx` bundle provided by the caller.
/// `arg0` = bundle_ptr, `arg1` = bundle_len, `arg2` = id_out_ptr (u32le).
pub(crate) fn sys_app_install(bundle_ptr: u64, bundle_len: u64, id_out_ptr: u64) -> i64 {
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
pub(crate) fn sys_app_list(buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 || buf_len == 0 {
        return crate::app_registry::count() as i64;
    }
    if buf_len > 0x20_0000 { return -22; } // EINVAL
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, buf_len as usize) };
    crate::app_registry::list(buf) as i64
}

/// Launch an installed app in a new host process.
/// Returns the new host PID, or -ERRNO.
pub(crate) fn sys_app_launch(app_id: u64, flags: u64) -> i64 {
    crate::app_registry::launch(app_id as u32, flags as u32)
}

/// Uninstall an installed app by `app_id`.
pub(crate) fn sys_app_uninstall(app_id: u64) -> i64 {
    if crate::app_registry::uninstall(app_id as u32) { 0 } else { -2 } // ENOENT
}

// ── On-demand package delivery ───────────────────────────────────────────────

/// Resolve a package by name — fetch on demand if not cached.
/// `arg0` = name_ptr, `arg1` = name_len.
/// Returns app_id on success, or negative errno.
pub(crate) fn sys_pkg_resolve(name_ptr: u64, name_len: u64) -> i64 {
    let name = match unsafe { read_user_bytes(name_ptr, name_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    match crate::pkg::resolver::resolve(name) {
        Ok(app_id) => app_id as i64,
        Err(crate::pkg::resolver::ResolveError::NotFound) => -2,    // ENOENT
        Err(crate::pkg::resolver::ResolveError::NoCatalog) => -2,   // ENOENT
        Err(crate::pkg::resolver::ResolveError::FetchFailed) => -5, // EIO
        Err(crate::pkg::resolver::ResolveError::HashMismatch) => -22, // EINVAL
        Err(crate::pkg::resolver::ResolveError::InstallFailed) => -12, // ENOMEM
        Err(crate::pkg::resolver::ResolveError::CacheFull) => -28,  // ENOSPC
    }
}

/// Serialise the package catalog into a user buffer.
/// Each entry is 128 bytes (PkgManifest).
/// If buf_ptr=0, returns the count of available packages.
pub(crate) fn sys_pkg_catalog(buf_ptr: u64, buf_len: u64) -> i64 {
    let catalog = crate::pkg::resolver::catalog();
    if buf_ptr == 0 || buf_len == 0 {
        return catalog.len() as i64;
    }
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, buf_len as usize) };
    let entry_size = crate::pkg::manifest::MANIFEST_SIZE;
    let mut written = 0usize;
    for entry in catalog.iter() {
        if written + entry_size > buf.len() { break; }
        let bytes = unsafe {
            core::slice::from_raw_parts(
                entry as *const crate::pkg::manifest::PkgManifest as *const u8,
                entry_size,
            )
        };
        buf[written..written + entry_size].copy_from_slice(bytes);
        written += entry_size;
    }
    catalog.len() as i64
}

/// Set the package server IP and port.
/// `arg0` = ip_packed_be (big-endian u32), `arg1` = port.
pub(crate) fn sys_pkg_set_server(ip: u64, port: u64) -> i64 {
    crate::pkg::resolver::set_server(ip as u32, port as u16);
    // Refresh catalog from the new server.
    let _ = crate::pkg::resolver::refresh_catalog();
    0
}

/// Evict a cached package by app_id.
pub(crate) fn sys_pkg_evict(app_id: u64) -> i64 {
    if crate::pkg::cache::remove(app_id as u32) { 0 } else { -2 } // ENOENT
}

// ── Phase 39 — Named port IPC namespace ──────────────────────────────────────

/// Bind the calling process's isolate under `name`.
/// `arg0` = name_ptr, `arg1` = name_len, `arg2` = isolate_id.
pub(crate) fn sys_port_bind(name_ptr: u64, name_len: u64, isolate_id: u64) -> i64 {
    let name = match unsafe { read_user_bytes(name_ptr, name_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    let pid = crate::process::current_pid();
    crate::port_ns::bind(name, isolate_id as u32, pid)
}

/// Look up a named port.
/// `arg0` = name_ptr, `arg1` = name_len, `arg2` = iso_id_out_ptr (u32le), `arg3` = pid_out_ptr (u32le).
pub(crate) fn sys_port_lookup(name_ptr: u64, name_len: u64, iso_out: u64, pid_out: u64) -> i64 {
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
pub(crate) fn sys_port_unbind(name_ptr: u64, name_len: u64) -> i64 {
    let name = match unsafe { read_user_bytes(name_ptr, name_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    crate::port_ns::unbind(name)
}

// ── Phase 41 — USB host-controller query ─────────────────────────────────────

pub(crate) fn sys_usb_controller_count() -> i64 {
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
pub(crate) fn sys_fb_map(info_out_ptr: u64) -> i64 {
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
    // Canonical shell render path uses gpu_submit → compositor (see docs/hardware.txt).
    // fb_map only exposes a read/write mapping for bring-up tools — it does not
    // bypass the compositor; use SYS_SURFACE_* + gpu_submit_strided for UI.
    crate::drivers::fb::disable_fb_logging();
    0
}

/// Pop the next WM event for the calling process.
///
/// Writes a 32-byte WmEvent to `event_out_ptr` on success (returns 0).
/// Returns -11 (EAGAIN) if no events are pending.
pub(crate) fn sys_wm_next_event(event_out_ptr: u64) -> i64 {
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
pub(crate) fn sys_vfs_list(path_ptr: u64, path_len: u64, buf_ptr: u64, buf_len: u64) -> i64 {
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
pub(crate) fn sys_vfs_read(path_ptr: u64, path_len: u64, buf_ptr: u64, buf_len: u64) -> i64 {
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
pub(crate) fn sys_vfs_write(path_ptr: u64, path_len: u64, data_ptr: u64, data_len: u64) -> i64 {
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
pub(crate) fn sys_vfs_stat(path_ptr: u64, path_len: u64, size_out_ptr: u64) -> i64 {
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
pub(crate) fn sys_net_info(buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 || buf_len == 0 { return -14; }
    let len = (buf_len as usize).min(256);
    let mut kbuf = [0u8; 256];
    let n = crate::drivers::virtio_net::info_text(&mut kbuf[..len]);
    unsafe { core::ptr::copy_nonoverlapping(kbuf.as_ptr(), buf_ptr as *mut u8, n); }
    n as i64
}

/// Enqueue a raw UDP-like payload as an Ethernet frame (stub).
/// `arg0` = dst_ip (u32 BE), `arg1` = dst_port (u16), `arg2` = data_ptr, `arg3` = data_len.
pub(crate) fn sys_net_send(dst_ip: u64, _dst_port: u64, data_ptr: u64, data_len: u64) -> i64 {
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
pub(crate) fn sys_net_recv(buf_ptr: u64, buf_len: u64, _src_ip_out: u64, _src_port_out: u64) -> i64 {
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
pub(crate) fn sys_fb_release() -> i64 {
    crate::compositor::set_fb_bypass(false);
    0
}

/// Create a surface that exactly covers the framebuffer.
/// Returns the new surface id, or negative errno.
pub(crate) fn sys_surface_fullscreen() -> i64 {
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
pub(crate) fn sys_serial_read(buf_ptr: u64, buf_len: u64) -> i64 {
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
pub(crate) fn sys_serial_write(buf_ptr: u64, buf_len: u64) -> i64 {
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
pub(crate) fn sys_blk_info(buf_ptr: u64) -> i64 {
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
pub(crate) fn sys_blk_read(sector: u64, count: u64, buf_ptr: u64, buf_len: u64) -> i64 {
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
pub(crate) fn sys_blk_write(sector: u64, count: u64, buf_ptr: u64, buf_len: u64) -> i64 {
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
pub(crate) fn sys_tcp_connect(dst_ip: u64, dst_port: u64) -> i64 {
    match crate::net::tcp::tcp_connect(dst_ip as u32, dst_port as u16) {
        Ok(fd)   => fd as i64,
        Err(e)   => e,
    }
}

/// Write to a TCP socket.  arg0=fd, arg1=buf_ptr, arg2=buf_len.
pub(crate) fn sys_tcp_write(fd: u64, buf_ptr: u64, buf_len: u64) -> i64 {
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
pub(crate) fn sys_tcp_read(fd: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 { return -14; }
    let cap = (buf_len as usize).min(65536);
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, cap) };
    match crate::net::tcp::tcp_read(fd as usize, buf) {
        Ok(n)  => n as i64,
        Err(e) => e,
    }
}

/// Close a TCP socket.
pub(crate) fn sys_tcp_close(fd: u64) -> i64 {
    match crate::net::tcp::tcp_close(fd as usize) {
        Ok(())  => 0,
        Err(e)  => e,
    }
}

/// Trigger DHCP DISCOVER.  Returns assigned IP as BE u32, or 0 on failure.
pub(crate) fn sys_dhcp_discover() -> i64 {
    crate::net::tcp::dhcp_discover() as i64
}

// ── Phase 51: ext2 read-only filesystem ──────────────────────────────────────

pub(crate) fn sys_ext2_mount() -> i64 {
    match crate::fs::ext2::mount() { Ok(()) => 0, Err(_) => -5 }
}

pub(crate) fn sys_ext2_ls(path_ptr: u64, path_len: u64, out_ptr: u64, out_len: u64) -> i64 {
    if path_ptr == 0 || out_ptr == 0 { return -14; }
    let path_bytes = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b, None => return -14,
    };
    let out = unsafe { core::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len as usize) };
    match crate::fs::ext2::ls(path_bytes, out) { Ok(n) => n as i64, Err(_) => -2 }
}

pub(crate) fn sys_ext2_read(path_ptr: u64, path_len: u64, out_ptr: u64, out_len: u64) -> i64 {
    if path_ptr == 0 || out_ptr == 0 { return -14; }
    let path_bytes = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b, None => return -14,
    };
    let out = unsafe { core::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len as usize) };
    match crate::fs::ext2::read_file(path_bytes, out) { Ok(n) => n as i64, Err(_) => -2 }
}

// ── Phase 53: scheduler extras ───────────────────────────────────────────────

pub(crate) fn sys_sched_yield() -> i64 {
    let cur = crate::process::current_pid();
    if cur == 0 {
        return 0;
    }
    if let Some(next) = crate::process::next_runnable_pid(cur) {
        if next != cur {
            let urip = crate::arch::syscall::user_rip();
            let ursp = crate::arch::syscall::user_rsp();
            crate::process::save_cooperative_yield_context(cur, urip, ursp);
            crate::process::set_rax(cur, 0);
            crate::process::save_xstate(cur);
            crate::process::set_state(cur, crate::process::ProcState::Running);
            crate::process::enter_user_by_pid_noreturn(next);
        } else if cur == 1 {
            // OnVsync posts work to runner threads — sleep briefly so they can run (SMP or IRQ wake).
            unsafe {
                core::arch::asm!("sti; hlt; cli", options(nomem, nostack));
            }
        } else {
            // No other runnable process — sleep until the next IRQ (timerfd, vsync…).
            unsafe {
                core::arch::asm!("sti; hlt; cli", options(nomem, nostack));
            }
        }
    }
    0
}

pub(crate) fn sys_get_cpu_time(pid: u64) -> i64 {
    let target = if pid == 0 { crate::process::current_pid() } else { pid as u32 };
    crate::process::get_cpu_ticks(target) as i64
}

// ── Phase 54: fork ────────────────────────────────────────────────────────────

pub(crate) fn sys_fork() -> i64 {
    match crate::process::fork_current() {
        Ok(child_pid) => child_pid as i64,
        Err(_) => -12, // ENOMEM
    }
}

// ── Phase 55: signals ─────────────────────────────────────────────────────────

pub(crate) fn sys_kill_signal(target_pid: u64, sig: u64) -> i64 {
    if sig > 31 { return -22; } // EINVAL
    crate::process::raise_signal(target_pid as u32, sig as u8);
    0
}

pub(crate) fn sys_sigaction(sig: u64, handler_ptr: u64) -> i64 {
    if sig == 0 || sig > 31 { return -22; }
    let pid = crate::process::current_pid();
    crate::process::set_signal_handler(pid, sig as u8, handler_ptr);
    0
}

pub(crate) fn sys_sigreturn() -> i64 {
    // Signal return — the user stack should have saved context; for now just
    // set running state and return 0 (real implementation needs the signal
    // frame layout to match the signal delivery code).
    0
}

// ── Phase 59: NVMe driver ─────────────────────────────────────────────────────

pub(crate) fn sys_nvme_info(buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 { return -14; }
    let out = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, buf_len as usize) };
    crate::drivers::nvme::info_text(out) as i64
}

pub(crate) fn sys_nvme_read(lba: u64, count: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 { return -14; }
    if buf_len < count * 512 { return -22; }
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, buf_len as usize) };
    match crate::drivers::nvme::read_sectors(lba, count as u32, buf) {
        Ok(()) => count as i64 * 512,
        Err(_) => -5,
    }
}

pub(crate) fn sys_nvme_write(lba: u64, count: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 { return -14; }
    if buf_len < count * 512 { return -22; }
    let buf = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, buf_len as usize) };
    match crate::drivers::nvme::write_sectors(lba, count as u32, buf) {
        Ok(()) => count as i64 * 512,
        Err(_) => -5,
    }
}
