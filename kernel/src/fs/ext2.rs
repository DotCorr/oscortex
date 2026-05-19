//! Phase 51 — ext2 read-only filesystem driver.
//!
//! Mounts an ext2 volume from the virtio-blk device and provides:
//!   * `mount()`  — read superblock, validate magic, cache block-group table
//!   * `ls(path)` — list a directory
//!   * `read(path, buf)` — read a regular file into a user buffer
//!
//! Only the features needed for a read-only root mount are implemented:
//!   * Block sizes 1024 / 2048 / 4096
//!   * Single/double indirect blocks
//!   * Linear directory entries (no htree)
//!
//! All on-disk integers are little-endian; we run on x86_64 so no byte-swap is
//! needed.

use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

// ── ext2 on-disk structures ───────────────────────────────────────────────────

/// Superblock is at byte offset 1024, exactly 1024 bytes.
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Superblock {
    s_inodes_count:      u32,
    s_blocks_count:      u32,
    s_r_blocks_count:    u32,
    s_free_blocks_count: u32,
    s_free_inodes_count: u32,
    s_first_data_block:  u32,
    s_log_block_size:    u32,   // block_size = 1024 << s_log_block_size
    s_log_frag_size:     i32,
    s_blocks_per_group:  u32,
    s_frags_per_group:   u32,
    s_inodes_per_group:  u32,
    s_mtime:             u32,
    s_wtime:             u32,
    s_mnt_count:         u16,
    s_max_mnt_count:     i16,
    s_magic:             u16,   // must be 0xEF53
    s_state:             u16,
    s_errors:            u16,
    s_minor_rev_level:   u16,
    s_lastcheck:         u32,
    s_checkinterval:     u32,
    s_creator_os:        u32,
    s_rev_level:         u32,
    s_def_resuid:        u16,
    s_def_resgid:        u16,
    // EXT2_DYNAMIC_REV fields (rev_level >= 1)
    s_first_ino:         u32,
    s_inode_size:        u16,
    s_block_group_nr:    u16,
    s_feature_compat:    u32,
    s_feature_incompat:  u32,
    s_feature_ro_compat: u32,
    _pad:                [u8; 940],
}

const EXT2_MAGIC: u16 = 0xEF53;
const EXT2_FT_REG_FILE: u8 = 1;
const EXT2_FT_DIR:      u8 = 2;

/// Block group descriptor (32 bytes each).
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct GroupDesc {
    bg_block_bitmap:      u32,
    bg_inode_bitmap:      u32,
    bg_inode_table:       u32,
    bg_free_blocks_count: u16,
    bg_free_inodes_count: u16,
    bg_used_dirs_count:   u16,
    bg_pad:               u16,
    bg_reserved:          [u32; 3],
}

/// Inode (128 bytes for rev0, `s_inode_size` for rev1+).
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
struct Inode {
    i_mode:        u16,
    i_uid:         u16,
    i_size:        u32,
    i_atime:       u32,
    i_ctime:       u32,
    i_mtime:       u32,
    i_dtime:       u32,
    i_gid:         u16,
    i_links_count: u16,
    i_blocks:      u32,   // 512-byte blocks used
    i_flags:       u32,
    _osd1:         u32,
    i_block:       [u32; 15],  // direct[0..11], indirect, dindirect, tindirect
    i_generation:  u32,
    i_file_acl:    u32,
    i_dir_acl:     u32,
    i_faddr:       u32,
    _osd2:         [u8; 12],
}

const S_IFMT:  u16 = 0xF000;
const S_IFREG: u16 = 0x8000;
const S_IFDIR: u16 = 0x4000;

/// Linear directory entry (variable length).
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct DirEntry {
    inode:     u32,
    rec_len:   u16,
    name_len:  u8,
    file_type: u8,
    // name[name_len] follows
}

// ── Mounted filesystem state ──────────────────────────────────────────────────

struct Ext2Fs {
    block_size:       usize,
    inodes_per_group: u32,
    inode_size:       usize,
    groups:           Vec<GroupDesc>,
}

impl Ext2Fs {
    /// Read `n` sectors (512-byte) beginning at `sector` into a fresh Vec.
    fn read_sectors(&self, sector: u64, n: u64) -> Option<Vec<u8>> {
        let bytes = (n * 512) as usize;
        let mut buf = vec![0u8; bytes];
        crate::drivers::virtio_blk::read_sectors(sector, n, &mut buf).ok()?;
        Some(buf)
    }

    /// Read one filesystem block.
    fn read_block(&self, block: u32) -> Option<Vec<u8>> {
        if block == 0 { return None; }
        let sectors_per_block = (self.block_size / 512) as u64;
        self.read_sectors(block as u64 * sectors_per_block, sectors_per_block)
    }

    /// Look up an inode by number (1-based).
    fn read_inode(&self, ino: u32) -> Option<Inode> {
        if ino == 0 { return None; }
        let idx     = (ino - 1) as usize;
        let group   = idx / self.inodes_per_group as usize;
        let local   = idx % self.inodes_per_group as usize;
        let gd      = self.groups.get(group)?;
        let table   = { let t = gd.bg_inode_table; t } as u64;
        let byte_off = local * self.inode_size;
        let sector  = table * (self.block_size / 512) as u64 + (byte_off / 512) as u64;
        let data    = self.read_sectors(sector, 1)?;
        let in_sec  = byte_off % 512;
        if in_sec + self.inode_size > data.len() { return None; }
        Some(unsafe {
            core::ptr::read_unaligned(data[in_sec..].as_ptr() as *const Inode)
        })
    }

    /// Resolve a data block index within a file (handles indirect blocks).
    fn file_block(&self, inode: &Inode, logical: u32) -> Option<u32> {
        let ptrs_per_block = (self.block_size / 4) as u32;
        if logical < 12 {
            let b = { let x = inode.i_block[logical as usize]; x };
            if b == 0 { None } else { Some(b) }
        } else if logical < 12 + ptrs_per_block {
            let indirect = { let x = inode.i_block[12]; x };
            if indirect == 0 { return None; }
            let blk_data = self.read_block(indirect)?;
            let off = (logical - 12) as usize * 4;
            let ptr = u32::from_le_bytes(blk_data[off..off+4].try_into().ok()?);
            if ptr == 0 { None } else { Some(ptr) }
        } else {
            let l2 = logical - 12 - ptrs_per_block;
            let dindirect = { let x = inode.i_block[13]; x };
            if dindirect == 0 { return None; }
            let l1_data = self.read_block(dindirect)?;
            let l1_idx  = (l2 / ptrs_per_block) as usize * 4;
            let l1_blk  = u32::from_le_bytes(l1_data[l1_idx..l1_idx+4].try_into().ok()?);
            if l1_blk == 0 { return None; }
            let l2_data = self.read_block(l1_blk)?;
            let l2_idx  = (l2 % ptrs_per_block) as usize * 4;
            let ptr     = u32::from_le_bytes(l2_data[l2_idx..l2_idx+4].try_into().ok()?);
            if ptr == 0 { None } else { Some(ptr) }
        }
    }

    /// Read the full contents of a file given its inode.
    fn read_file_bytes(&self, inode: &Inode) -> Option<Vec<u8>> {
        let size = { inode.i_size } as usize;
        let mut out = vec![0u8; size];
        let mut written = 0usize;
        let mut logical_blk = 0u32;
        while written < size {
            let blk = self.file_block(inode, logical_blk)?;
            let data = self.read_block(blk)?;
            let to_copy = (size - written).min(self.block_size);
            out[written..written + to_copy].copy_from_slice(&data[..to_copy]);
            written += to_copy;
            logical_blk += 1;
        }
        Some(out)
    }

    /// Walk a directory inode, call `cb(name, child_ino, file_type)` for each entry.
    fn readdir<F>(&self, inode: &Inode, mut cb: F)
    where F: FnMut(&[u8], u32, u8) -> bool,  // return false to stop
    {
        let size = { inode.i_size } as usize;
        let mut file_off = 0usize;
        let mut logical_blk = 0u32;
        while file_off < size {
            let blk = match self.file_block(inode, logical_blk) {
                Some(b) => b,
                None => break,
            };
            let data = match self.read_block(blk) {
                Some(d) => d,
                None => break,
            };
            let mut in_blk = 0usize;
            while in_blk + core::mem::size_of::<DirEntry>() <= data.len() {
                let de = unsafe {
                    core::ptr::read_unaligned(data[in_blk..].as_ptr() as *const DirEntry)
                };
                let rec = { de.rec_len } as usize;
                if rec < 8 || in_blk + rec > data.len() { break; }
                let ino = { de.inode };
                if ino != 0 {
                    let nlen = de.name_len as usize;
                    let name = &data[in_blk + 8..in_blk + 8 + nlen];
                    if !cb(name, ino, de.file_type) { return; }
                }
                in_blk += rec;
                file_off += rec;
            }
            logical_blk += 1;
            if in_blk >= self.block_size { file_off = logical_blk as usize * self.block_size; }
        }
    }

    /// Lookup a path component starting from inode `parent_ino`.
    fn lookup(&self, parent_ino: u32, name: &[u8]) -> Option<(u32, u8)> {
        let inode = self.read_inode(parent_ino)?;
        let mut result = None;
        self.readdir(&inode, |n, ino, ft| {
            if n == name { result = Some((ino, ft)); false }
            else { true }
        });
        result
    }

    /// Resolve an absolute path to (inode_number, file_type).
    fn resolve_path(&self, path: &[u8]) -> Option<(u32, u8)> {
        let mut ino = 2u32; // root inode is always 2
        let mut ft  = EXT2_FT_DIR;
        let path = if path.starts_with(b"/") { &path[1..] } else { path };
        if path.is_empty() { return Some((ino, ft)); }
        for component in path.split(|&b| b == b'/') {
            if component.is_empty() { continue; }
            let (next_ino, next_ft) = self.lookup(ino, component)?;
            ino = next_ino;
            ft  = next_ft;
        }
        Some((ino, ft))
    }
}

// ── Global mounted filesystem ─────────────────────────────────────────────────

static FS: Mutex<Option<Ext2Fs>> = Mutex::new(None);

/// Mount the ext2 volume on virtio-blk device 0.
/// Returns Ok(()) on success, Err with a description on failure.
pub fn mount() -> Result<(), &'static str> {
    if !crate::drivers::virtio_blk::is_ready() {
        return Err("no virtio-blk device");
    }

    // Superblock is at byte 1024 = sector 2, spanning 2 sectors (1024 bytes).
    let mut sb_buf = vec![0u8; 1024];
    crate::drivers::virtio_blk::read_sectors(2, 2, &mut sb_buf)
        .map_err(|_| "block read failed")?;

    let sb = unsafe { core::ptr::read_unaligned(sb_buf.as_ptr() as *const Superblock) };
    if { sb.s_magic } != EXT2_MAGIC {
        return Err("ext2: bad magic (not an ext2 volume)");
    }

    let block_size   = 1024usize << { sb.s_log_block_size };
    let inode_size   = if { sb.s_rev_level } >= 1 { sb.s_inode_size as usize } else { 128 };
    let inodes_per_group = { sb.s_inodes_per_group };
    let blocks_per_group = { sb.s_blocks_per_group };
    let total_blocks = { sb.s_blocks_count };

    let num_groups = (total_blocks + blocks_per_group - 1) / blocks_per_group;

    // Block group descriptor table starts at block 1 for 1024-byte blocks,
    // block 2 for larger block sizes.
    let bgdt_block = if block_size == 1024 { 2u32 } else { 1u32 };
    let bgdt_size_bytes = num_groups as usize * core::mem::size_of::<GroupDesc>();
    let bgdt_sectors = ((bgdt_size_bytes + 511) / 512) as u64;
    let bgdt_start_sector = bgdt_block as u64 * (block_size / 512) as u64;

    let mut gd_buf = vec![0u8; bgdt_sectors as usize * 512];
    crate::drivers::virtio_blk::read_sectors(bgdt_start_sector, bgdt_sectors, &mut gd_buf)
        .map_err(|_| "ext2: failed to read block group table")?;

    let mut groups = Vec::with_capacity(num_groups as usize);
    for i in 0..num_groups as usize {
        let off = i * core::mem::size_of::<GroupDesc>();
        let gd = unsafe { core::ptr::read_unaligned(gd_buf[off..].as_ptr() as *const GroupDesc) };
        groups.push(gd);
    }

    let fs = Ext2Fs { block_size, inodes_per_group, inode_size, groups };
    log::info!("[ext2] mounted: block_size={} inode_size={} groups={} total_inodes={}",
        block_size, inode_size, num_groups, { sb.s_inodes_count });

    *FS.lock() = Some(fs);
    Ok(())
}

/// List a directory.  Returns newline-separated names, or an error string.
/// `out` must be large enough; we write at most `out.len()` bytes.
/// Returns the number of bytes written.
pub fn ls(path: &[u8], out: &mut [u8]) -> Result<usize, &'static str> {
    let guard = FS.lock();
    let fs = guard.as_ref().ok_or("ext2 not mounted")?;
    let (ino, ft) = fs.resolve_path(path).ok_or("ext2: path not found")?;
    if ft != EXT2_FT_DIR { return Err("ext2: not a directory"); }
    let inode = fs.read_inode(ino).ok_or("ext2: cannot read inode")?;
    let mut written = 0usize;
    fs.readdir(&inode, |name, _ino2, ft2| {
        if name == b"." || name == b".." { return true; }
        let suffix = if ft2 == EXT2_FT_DIR { b"/" as &[u8] } else { b"" };
        let needed = name.len() + suffix.len() + 1; // +1 for newline
        if written + needed > out.len() { return false; }
        out[written..written + name.len()].copy_from_slice(name);
        written += name.len();
        out[written..written + suffix.len()].copy_from_slice(suffix);
        written += suffix.len();
        out[written] = b'\n';
        written += 1;
        true
    });
    Ok(written)
}

/// Read a regular file into `buf`.  Returns bytes read.
pub fn read_file(path: &[u8], out: &mut [u8]) -> Result<usize, &'static str> {
    let guard = FS.lock();
    let fs = guard.as_ref().ok_or("ext2 not mounted")?;
    let (ino, ft) = fs.resolve_path(path).ok_or("ext2: path not found")?;
    if ft != EXT2_FT_REG_FILE {
        // also accept ft==0 (unknown) and check mode
        let inode2 = fs.read_inode(ino).ok_or("ext2: cannot read inode")?;
        if { inode2.i_mode } & S_IFMT != S_IFREG {
            return Err("ext2: not a regular file");
        }
    }
    let inode = fs.read_inode(ino).ok_or("ext2: cannot read inode")?;
    let data = fs.read_file_bytes(&inode).ok_or("ext2: read failed")?;
    let to_copy = data.len().min(out.len());
    out[..to_copy].copy_from_slice(&data[..to_copy]);
    Ok(to_copy)
}

/// Check if the filesystem is mounted.
pub fn is_mounted() -> bool {
    FS.lock().is_some()
}

/// Get size of a file in bytes, or -1 if not found.
pub fn file_size(path: &[u8]) -> i64 {
    let guard = FS.lock();
    let fs = match guard.as_ref() { Some(f) => f, None => return -1 };
    let (ino, _) = match fs.resolve_path(path) { Some(x) => x, None => return -1 };
    let inode = match fs.read_inode(ino) { Some(i) => i, None => return -1 };
    inode.i_size as i64
}
