//! QEMU `ramfb` display via the `fw_cfg` interface — aarch64 `-M virt`.
//!
//! `qemu-system-aarch64 -M virt` provides no Limine framebuffer (Limine is the
//! x86_64 boot protocol). QEMU's `-device ramfb` instead exposes a software
//! framebuffer that the guest configures through **fw_cfg**: the guest writes a
//! big-endian [`RamfbCfg`] describing a guest-RAM buffer (physical address,
//! DRM fourcc pixel format, dimensions, stride) to the `etc/ramfb` fw_cfg file,
//! and QEMU thereafter scans that buffer out to the display every refresh.
//!
//! ## fw_cfg on `-M virt`
//! The MMIO fw_cfg device sits at a fixed base (QEMU virt: `0x0902_0000`). Its
//! register block (big-endian, "DMA" interface) is:
//!
//!   * `+0x00` (8 bytes)  `data`     — legacy data port (unused here).
//!   * `+0x08` (2 bytes)  `selector` — write the 16-bit file/key selector (BE).
//!   * `+0x10` (8 bytes)  `dma`      — write the **physical** address of a
//!                                      [`FwCfgDmaAccess`] control block (BE) to
//!                                      kick a DMA transfer.
//!
//! To configure ramfb we:
//!   1. Read the fw_cfg file directory (`FW_CFG_FILE_DIR`, selector `0x0019`)
//!      and find the `etc/ramfb` entry → its write-only selector key.
//!   2. Allocate a framebuffer in guest RAM (identity-mapped, so VA == PA).
//!   3. DMA-write a big-endian [`RamfbCfg`] into that file selector.
//!
//! After step 3 QEMU latches the configuration and the framebuffer is live; any
//! pixels the kernel writes into the buffer appear on screen at the next refresh.
//!
//! The framebuffer is then published to the shared [`crate::drivers::fb`] console
//! through its arch-neutral `init_raw` hook, so the existing compositor / fb text
//! console (which x86 feeds from Limine) works unchanged on ARM.

use core::sync::atomic::{AtomicU64, Ordering};

/// fw_cfg MMIO base on QEMU `-M virt`.
const FW_CFG_BASE: u64 = 0x0902_0000;

/// MMIO register offset for the fw_cfg DMA control-block pointer (big-endian).
/// All fw_cfg access here goes through the DMA interface, which folds item
/// selection into the DMA control word, so the legacy selector port is unused.
const FW_CFG_REG_DMA: u64 = 0x10;

/// Well-known fw_cfg selector for the file directory.
const FW_CFG_FILE_DIR: u16 = 0x0019;

/// fw_cfg DMA control bits (the `control` field of [`FwCfgDmaAccess`], BE).
const FW_CFG_DMA_CTL_ERROR: u32 = 0x01;
const FW_CFG_DMA_CTL_READ: u32 = 0x02;
const FW_CFG_DMA_CTL_SKIP: u32 = 0x04;
const FW_CFG_DMA_CTL_SELECT: u32 = 0x08;
const FW_CFG_DMA_CTL_WRITE: u32 = 0x10;

/// DRM fourcc for `XRGB8888` (little-endian: byte0=B, byte1=G, byte2=R, byte3=X)
/// — the format the shared fb console expects (32 bpp, X in the high byte).
const DRM_FORMAT_XRGB8888: u32 = 0x3432_5258; // 'XR24'

/// Default display geometry. QEMU ramfb is fully software-defined, so any size
/// the guest picks is honoured; 1280×800 matches the x86 default shell surface.
const FB_WIDTH: u32 = 1280;
const FB_HEIGHT: u32 = 800;
const FB_BPP: u32 = 4; // bytes per pixel (XRGB8888)

/// fw_cfg DMA access control block. All fields big-endian per the fw_cfg spec.
#[repr(C)]
struct FwCfgDmaAccess {
    control: u32, // BE
    length: u32,  // BE
    address: u64, // BE (physical address of the data buffer)
}

/// The `etc/ramfb` configuration record. **All fields big-endian.**
///
/// `#[repr(C, packed)]` is required: the on-wire record is exactly 28 bytes
/// (8+4+4+4+4+4), but a naturally-aligned `repr(C)` would pad the leading `u64`
/// out to 32, and QEMU rejects an `etc/ramfb` write whose length exceeds the
/// file's declared 28-byte size (the cause of the earlier DMA error bit).
#[repr(C, packed)]
struct RamfbCfg {
    addr: u64,   // BE — physical address of the framebuffer
    fourcc: u32, // BE — DRM fourcc pixel format
    flags: u32,  // BE — reserved (0)
    width: u32,  // BE
    height: u32, // BE
    stride: u32, // BE — bytes per row
}

/// One fw_cfg file-directory entry (big-endian fields), spec layout.
#[repr(C)]
struct FwCfgFile {
    size: u32,       // BE — file size in bytes
    select: u16,     // BE — selector key
    reserved: u16,   // BE
    name: [u8; 56],  // NUL-padded ASCII name
}

/// Published framebuffer physical base once ramfb is configured (0 = not up).
static FB_PHYS: AtomicU64 = AtomicU64::new(0);

#[inline(always)]
unsafe fn mmio_write_u64(off: u64, v: u64) {
    core::ptr::write_volatile((FW_CFG_BASE + off) as *mut u64, v);
}

/// Kick a fw_cfg DMA transfer described by `dma` (which must be a populated,
/// big-endian [`FwCfgDmaAccess`] in guest RAM) and spin until QEMU clears the
/// busy control bits. Returns `false` if QEMU flagged an error.
unsafe fn fw_cfg_dma(dma: &mut FwCfgDmaAccess) -> bool {
    // The DMA address register is big-endian; write the *physical* address of the
    // control block (identity map → VA == PA).
    let dma_pa = dma as *mut FwCfgDmaAccess as u64;
    mmio_write_u64(FW_CFG_REG_DMA, dma_pa.to_be());

    // QEMU processes the transfer synchronously under TCG, but per spec we must
    // poll `control` until all bits except a possible ERROR are cleared.
    loop {
        let ctl = core::ptr::read_volatile(&dma.control as *const u32);
        let ctl = u32::from_be(ctl);
        if ctl & FW_CFG_DMA_CTL_ERROR != 0 {
            log::warn!("[ramfb] fw_cfg DMA error: control={:#x}", ctl);
            return false;
        }
        // All action bits cleared → transfer complete.
        if ctl
            & (FW_CFG_DMA_CTL_READ
                | FW_CFG_DMA_CTL_WRITE
                | FW_CFG_DMA_CTL_SELECT
                | FW_CFG_DMA_CTL_SKIP)
            == 0
        {
            return true;
        }
        core::hint::spin_loop();
    }
}

/// Select a fw_cfg item and DMA-read `len` bytes of it into `buf` (physical).
unsafe fn fw_cfg_read(selector: u16, buf: *mut u8, len: u32) -> bool {
    let mut dma = FwCfgDmaAccess {
        control: ((FW_CFG_DMA_CTL_SELECT | FW_CFG_DMA_CTL_READ)
            | ((selector as u32) << 16))
            .to_be(),
        length: len.to_be(),
        address: (buf as u64).to_be(),
    };
    fw_cfg_dma(&mut dma)
}

/// Select a fw_cfg item by key and DMA-write `len` bytes from `buf` into it.
///
/// QEMU rejects a combined SELECT+WRITE in one DMA control word (it returns the
/// error bit), so we issue a SELECT-only DMA first to point fw_cfg at the file,
/// then a WRITE-only DMA carrying the payload.
unsafe fn fw_cfg_write(selector: u16, buf: *const u8, len: u32) -> bool {
    // 1. SELECT-only: point fw_cfg at the target file (resets its offset to 0).
    let mut sel = FwCfgDmaAccess {
        control: (FW_CFG_DMA_CTL_SELECT | ((selector as u32) << 16)).to_be(),
        length: 0u32.to_be(),
        address: 0u64.to_be(),
    };
    if !fw_cfg_dma(&mut sel) {
        return false;
    }
    // 2. WRITE-only: stream the payload into the now-selected file.
    let mut dma = FwCfgDmaAccess {
        control: FW_CFG_DMA_CTL_WRITE.to_be(),
        length: len.to_be(),
        address: (buf as u64).to_be(),
    };
    fw_cfg_dma(&mut dma)
}

/// Walk the fw_cfg file directory and return the selector key of `etc/ramfb`.
unsafe fn find_ramfb_selector() -> Option<u16> {
    // The directory begins with a big-endian u32 count, then `count` FwCfgFile
    // records. Read the count first.
    let mut count_be: u32 = 0;
    if !fw_cfg_read(
        FW_CFG_FILE_DIR,
        &mut count_be as *mut u32 as *mut u8,
        4,
    ) {
        return None;
    }
    let count = u32::from_be(count_be);
    if count == 0 || count > 1024 {
        return None;
    }

    // Re-select + re-read so the directory stream restarts at the count, then
    // skip the 4-byte count and stream the entries.
    let entries_len = count
        .saturating_mul(core::mem::size_of::<FwCfgFile>() as u32)
        .saturating_add(4);
    let buf_pa = crate::mm::frame_allocator::alloc_contiguous_frames(
        ((entries_len as usize) + 4095) / 4096,
    )?;
    let ok = fw_cfg_read(FW_CFG_FILE_DIR, buf_pa as *mut u8, entries_len);
    if !ok {
        crate::mm::frame_allocator::free_frame(buf_pa);
        return None;
    }

    let mut found: Option<u16> = None;
    let base = (buf_pa + 4) as *const FwCfgFile; // skip the leading count
    for i in 0..count as usize {
        let f = &*base.add(i);
        // Match the NUL-terminated name against "etc/ramfb".
        let name = &f.name;
        let target = b"etc/ramfb";
        let mut matches = true;
        for (j, &c) in target.iter().enumerate() {
            if name[j] != c {
                matches = false;
                break;
            }
        }
        if matches && name[target.len()] == 0 {
            found = Some(u16::from_be(f.select));
            log::debug!(
                "[ramfb] fw_cfg dir: etc/ramfb sel={:#x} size={} (of {} files)",
                u16::from_be(f.select),
                u32::from_be(f.size),
                count
            );
            break;
        }
    }

    crate::mm::frame_allocator::free_frame(buf_pa);
    found
}

/// Bring up the QEMU ramfb display and publish it to the shared fb console.
///
/// Returns `(phys_addr, width, height, stride_bytes)` on success. Must be called
/// after the frame allocator is online (so we can reserve the framebuffer) and
/// with the low-1 GiB device MMIO identity-mapped (so fw_cfg is reachable).
pub fn init() -> Option<(u64, u32, u32, u32)> {
    unsafe {
        let selector = match find_ramfb_selector() {
            Some(s) => s,
            None => {
                log::warn!("[ramfb] etc/ramfb not found in fw_cfg directory");
                return None;
            }
        };

        let stride = FB_WIDTH * FB_BPP;
        let fb_bytes = (stride as usize) * (FB_HEIGHT as usize);
        let pages = (fb_bytes + 4095) / 4096;
        let fb_phys = crate::mm::frame_allocator::alloc_contiguous_frames(pages)?;

        // Zero the framebuffer (identity-mapped → write directly).
        {
            let p = fb_phys as *mut u32;
            for i in 0..(fb_bytes / 4) {
                core::ptr::write_volatile(p.add(i), 0x0000_0000);
            }
        }

        // Build + DMA-write the big-endian RAMFBCfg into etc/ramfb.
        let cfg = RamfbCfg {
            addr: fb_phys.to_be(),
            fourcc: DRM_FORMAT_XRGB8888.to_be(),
            flags: 0u32.to_be(),
            width: FB_WIDTH.to_be(),
            height: FB_HEIGHT.to_be(),
            stride: stride.to_be(),
        };
        let ok = fw_cfg_write(
            selector,
            &cfg as *const RamfbCfg as *const u8,
            core::mem::size_of::<RamfbCfg>() as u32,
        );
        if !ok {
            log::warn!("[ramfb] fw_cfg write of etc/ramfb failed");
            return None;
        }

        FB_PHYS.store(fb_phys, Ordering::Release);
        log::info!(
            "[ramfb] up: {}x{} stride={} @ phys {:#x} (sel={:#x})",
            FB_WIDTH,
            FB_HEIGHT,
            stride,
            fb_phys,
            selector
        );

        Some((fb_phys, FB_WIDTH, FB_HEIGHT, stride))
    }
}

/// Physical base of the ramfb framebuffer (0 until [`init`] succeeds).
pub fn fb_phys() -> u64 {
    FB_PHYS.load(Ordering::Acquire)
}
