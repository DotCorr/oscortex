#[path = "../../../kernel/src/drivers/common/virtio_net_frame.rs"]
mod virtio_net_frame;

use virtio_net_frame::{
    frame_len_from_rx_total, tx_total_len, validate_tx_frame, MTU, RX_BUF_LEN,
    VIRTIO_NET_HDR_SIZE,
};

#[test]
fn rx_buf_len_includes_header() {
    assert_eq!(RX_BUF_LEN, (VIRTIO_NET_HDR_SIZE + MTU) as u32);
}

#[test]
fn frame_len_from_rx_total_strips_header() {
    assert_eq!(frame_len_from_rx_total(10), None);
    assert_eq!(frame_len_from_rx_total(74), Some(64));
}

#[test]
fn validate_tx_frame_bounds() {
    assert!(validate_tx_frame(64).is_ok());
    assert!(validate_tx_frame(0).is_err());
    assert!(validate_tx_frame(MTU + 1).is_err());
    assert_eq!(tx_total_len(64).unwrap(), VIRTIO_NET_HDR_SIZE + 64);
}
