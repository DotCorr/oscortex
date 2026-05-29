//! virtio-net frame length helpers.

pub const VIRTIO_NET_HDR_SIZE: usize = 10;
pub const MTU: usize = 1514;
pub const RX_BUF_LEN: u32 = (VIRTIO_NET_HDR_SIZE + MTU) as u32;

/// Ethernet payload length from a virtio-net RX completion total length.
pub fn frame_len_from_rx_total(total_len: u32) -> Option<usize> {
    if total_len <= VIRTIO_NET_HDR_SIZE as u32 {
        return None;
    }
    let frame_len = (total_len as usize - VIRTIO_NET_HDR_SIZE).min(MTU);
    if frame_len == 0 {
        None
    } else {
        Some(frame_len)
    }
}

/// Validate a raw Ethernet frame before TX submission.
pub fn validate_tx_frame(frame_len: usize) -> Result<(), &'static str> {
    if frame_len == 0 {
        Err("empty frame")
    } else if frame_len > MTU {
        Err("frame too large")
    } else {
        Ok(())
    }
}

/// Total TX buffer length including the virtio-net header.
pub fn tx_total_len(frame_len: usize) -> Result<usize, &'static str> {
    validate_tx_frame(frame_len)?;
    Ok(VIRTIO_NET_HDR_SIZE + frame_len)
}
