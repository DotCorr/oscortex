//! Cortex inter-core IPC — Ring-0 messages between Cortex shards on different CPUs.

use spin::Mutex;

const MAILBOX_CAP: usize = 32;

#[derive(Clone, Copy)]
struct IpcMsg {
    kind: u8,
    arg0: u64,
    arg1: u64,
}

impl IpcMsg {
    const fn empty() -> Self {
        Self {
            kind: 0,
            arg0: 0,
            arg1: 0,
        }
    }
}

static MAILBOX: Mutex<[IpcMsg; MAILBOX_CAP]> =
    Mutex::new([const { IpcMsg::empty() }; MAILBOX_CAP]);
static MAILBOX_HEAD: Mutex<usize> = Mutex::new(0);
static MAILBOX_TAIL: Mutex<usize> = Mutex::new(0);

/// Enqueue a Cortex IPC message (safe from any CPU).
pub fn post(kind: u8, arg0: u64, arg1: u64) {
    let mut head = MAILBOX_HEAD.lock();
    let tail = *MAILBOX_TAIL.lock();
    let next = (*head + 1) % MAILBOX_CAP;
    if next == tail {
        return; // full — drop
    }
    MAILBOX.lock()[*head] = IpcMsg { kind, arg0, arg1 };
    *head = next;
}

pub fn handle_ipc_interrupt() {
    loop {
        let mut tail = MAILBOX_TAIL.lock();
        let head = *MAILBOX_HEAD.lock();
        if *tail == head {
            break;
        }
        let msg = MAILBOX.lock()[*tail];
        *tail = (*tail + 1) % MAILBOX_CAP;
        drop(tail);
        dispatch(msg);
    }
}

fn dispatch(msg: IpcMsg) {
    match msg.kind {
        1 => {
            crate::cortex::with_state(|s| {
                crate::cortex::context::flush_pending(s);
            });
        }
        2 => {
            crate::cortex::healing::execute_manual(
                crate::cortex::inference::HealingActionKind::ReloadDriver,
                msg.arg0,
            );
        }
        3 => {
            let _ = crate::drivers::registry::reload(msg.arg0 as u32);
        }
        _ => {
            log::debug!(
                "[Cortex::IPC] unhandled kind={} arg0={} arg1={}",
                msg.kind,
                msg.arg0,
                msg.arg1
            );
        }
    }
}

pub fn init() {
    log::info!("[Cortex::IPC] Per-core mailbox online (cap={})", MAILBOX_CAP);
}
