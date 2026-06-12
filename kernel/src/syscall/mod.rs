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
//!   0x371          app_launch(app_id, flags) → host_pid
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

mod dispatch;
mod handlers;
pub(crate) mod poll;
mod posix;
mod state;
mod tables;
mod trace;
mod wait;

pub use dispatch::{dispatch_fast, dispatch_legacy};
pub use poll::{check_timerfds_and_wake, check_timerfds_and_wake_try, coop_target_ready, cooperative_sched_target, cooperative_yield_for_cond_resched, cooperative_yield_to, force_wake_all_task_runners, monotonic_ns, prefer_embedder_if_baton_due, KICK_REQUESTED, acqmutex_waiter_for};
pub use trace::{debug_dump_sync_states, dump_event_state, dump_recent_syscalls, dump_user_backtrace, init};

// Flat re-exports for `posix` and legacy `super::` call sites.
pub(crate) use handlers::{
    cond_miss_bridge, engine_broadcast_storm_wake, futex_wake_waiters, read_user_bytes, sys_exit, write_user_bytes, wm_consumer_pid,
};
pub(crate) use tables::{MAX_OPEN_FILES, OPEN_FILES, PIPES};
pub(crate) use state::{
    CondWaitState, COND_WAIT_STATE, ENGINE_HOST_PID, ENGINE_LIBRARY_PATH, ENGINE_PROC_TABLE_PTR,
    FS_BOOTSTRAP_LOGGED, WM_WAITER_DEADLINE, WM_WAITER_PID,
};
pub(crate) use trace::{POSTEXIT_TRACE_ACTIVE, POSTEXIT_TRACE_COUNT, POSTEXIT_TRACE_LIMIT};
pub(crate) use wait::{futex_waiter_add, futex_waiter_for, futex_waiter_present, futex_waiter_remove};
