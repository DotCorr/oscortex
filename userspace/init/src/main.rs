//! OSCortex PID 1 — thin supervisor.
//!
//! Spawns the system shell Flutter host (`/bin/oscortex-host`) and restarts it
//! on exit. Reaps zombie child processes (app hosts).

#![no_std]
#![no_main]

use core::arch::asm;

const SYS_WRITE: u64 = 1;
const SYS_EXIT: u64 = 60;
const SYS_WAITPID: u64 = 61;
const SYS_REAP_CHILDREN: u64 = 0x39F;
const SYS_SCHED_YIELD: u64 = 0x390;
const SYS_APP_LAUNCH_PATH: u64 = 0x342;

const HOST_MODE_SHELL: u64 = 1;

static HOST_PATH: &[u8] = b"/bin/oscortex-host\0";

#[inline(always)]
unsafe fn syscall0(n: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!(
            "syscall",
            in("rax") n,
            lateout("rax") ret,
            options(nostack),
        );
    }
    ret
}

fn reap_zombies() {
    let _ = unsafe { syscall0(SYS_REAP_CHILDREN) };
}

#[inline(always)]
unsafe fn syscall3(n: u64, a0: u64, a1: u64, a2: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!(
            "syscall",
            in("rax") n,
            in("rdi") a0,
            in("rsi") a1,
            in("rdx") a2,
            lateout("rax") ret,
            options(nostack),
        );
    }
    ret
}

fn write(msg: &[u8]) {
    let _ = unsafe { syscall3(SYS_WRITE, 1, msg.as_ptr() as u64, msg.len() as u64) };
}

fn spawn_shell_host() -> i64 {
    unsafe {
        syscall3(
            SYS_APP_LAUNCH_PATH,
            HOST_PATH.as_ptr() as u64,
            (HOST_PATH.len() - 1) as u64,
            HOST_MODE_SHELL,
        )
    }
}

fn waitpid(pid: u32) -> i64 {
    unsafe { syscall3(SYS_WAITPID, pid as u64, 0, 0) }
}

fn sched_yield() {
    let _ = unsafe { syscall3(SYS_SCHED_YIELD, 0, 0, 0) };
}

#[unsafe(no_mangle)]
#[unsafe(naked)]
unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "xor rbp, rbp",
        "and rsp, -16",
        "call {main}",
        "mov rax, 60",
        "xor rdi, rdi",
        "syscall",
        main = sym supervisor_main,
    );
}

extern "C" fn supervisor_main() -> ! {
    write(b"[init] OSCortex supervisor starting\n");

    loop {
        let shell_pid = spawn_shell_host();
        if shell_pid <= 0 {
            write(b"[init] failed to spawn shell host\n");
            for _ in 0..1_000_000 {
                sched_yield();
            }
            continue;
        }

        write(b"[init] shell host spawned\n");

        loop {
            let rc = waitpid(shell_pid as u32);
            if rc >= 0 {
                write(b"[init] shell host exited, restarting\n");
                break;
            }
            reap_zombies();
            sched_yield();
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    write(b"[init] panic\n");
    loop {
        unsafe { syscall3(SYS_EXIT, 99, 0, 0) };
    }
}
