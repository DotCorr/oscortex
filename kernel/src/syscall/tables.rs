use spin::Mutex;

// ── Open-file table ───────────────────────────────────────────────────────────

pub(crate) struct OpenFile {
    pub data:   &'static [u8],
    pub offset: u64,
    pub used:   bool,
    pub is_dir: bool,
    pub dir_path: [u8; 192],
    pub dir_path_len: usize,
    pub pipe_id: i16,
    pub pipe_is_write: bool,
}

pub(crate) const MAX_OPEN_FILES: usize = 64;
pub(crate) static OPEN_FILES: Mutex<[OpenFile; MAX_OPEN_FILES]> = Mutex::new(
    [const { OpenFile { data: &[], offset: 0, used: false, is_dir: false, dir_path: [0; 192], dir_path_len: 0, pipe_id: -1, pipe_is_write: false } }; MAX_OPEN_FILES]
);

// ── Pipe buffers ──────────────────────────────────────────────────────────────

pub(crate) const PIPE_BUF_SIZE: usize = 4096;
const MAX_PIPES: usize = 32;

pub(crate) struct PipeBuf {
    pub buf: [u8; PIPE_BUF_SIZE],
    pub head: usize,
    pub tail: usize,
    pub len:  usize,
    pub r_refs: u8,
    pub w_refs: u8,
}

pub(crate) static PIPES: Mutex<[PipeBuf; MAX_PIPES]> = Mutex::new(
    [const { PipeBuf { buf: [0; PIPE_BUF_SIZE], head: 0, tail: 0, len: 0, r_refs: 0, w_refs: 0 } }; MAX_PIPES]
);

pub(crate) fn pipe_readable_for_fd(fd: u32) -> bool {
    let fd_idx = fd as usize;
    if fd_idx >= MAX_OPEN_FILES {
        return false;
    }
    let open = OPEN_FILES.lock();
    if open[fd_idx].used && open[fd_idx].pipe_id >= 0 && !open[fd_idx].pipe_is_write {
        let pid = open[fd_idx].pipe_id as usize;
        drop(open);
        let pipes = PIPES.lock();
        pipes[pid].len > 0
    } else {
        false
    }
}
