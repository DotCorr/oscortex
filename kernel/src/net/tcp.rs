//! Phase 50 — smoltcp TCP/IP integration.
//!
//! Wraps our `virtio_net` raw Ethernet driver in a smoltcp `Device`, creates
//! an interface with a static IPv4 address, and provides:
//!
//! * UDP socket (replaces the hand-rolled UDP from Phase 45)
//! * TCP socket pool (up to `MAX_TCP_SOCKETS` simultaneous connections)
//! * ICMP echo (for `ping`)
//!
//! ## Syscall surface
//!   SYS_TCP_CONNECT  (0x388) dst_ip_packed, dst_port → socket_fd
//!   SYS_TCP_WRITE    (0x389) fd, buf_ptr, buf_len   → bytes_sent
//!   SYS_TCP_READ     (0x38A) fd, buf_ptr, buf_len   → bytes_recv
//!   SYS_TCP_CLOSE    (0x38B) fd                     → 0
//!   SYS_DHCP_DISCOVER(0x38C)                        → assigned_ip (u32 BE)
//!
//! ## Design notes
//! * `poll()` is called lazily on every TCP syscall — good enough for a
//!   single-threaded kernel without preemptive scheduling.
//! * Timestamps are provided by a monotonic tick counter incremented by the
//!   APIC timer (Phase 13 vsync counter).

use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::tcp::{self as tcp_sock, Socket as TcpSocket};
use smoltcp::socket::udp::{self as udp_sock, Socket as UdpSocket};
use smoltcp::socket::icmp::{self as icmp_sock, Socket as IcmpSocket};
use smoltcp::socket::dhcpv4::{Socket as DhcpSocket, Event as DhcpEvent};
use smoltcp::socket::dns::{Socket as DnsSocket, GetQueryResultError};
use smoltcp::time::Instant;
use smoltcp::wire::{
    DnsQueryType, EthernetAddress, IpAddress, IpCidr, Ipv4Address,
    IpEndpoint,
};

use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

// ── Constants ─────────────────────────────────────────────────────────────────

const MAX_TCP_SOCKETS: usize = 8;
// Per-socket rx/tx buffer = the advertised TCP window. 4 KiB forced a window-
// update round trip every 4 KiB — a 5.4 MB package fetch crawled at ~28 KB/s
// under TCG (~1300 stop-and-wait turns) and starved the reader into timeouts.
// 64 KiB (max un-scaled window) makes bulk fetches stream properly; worst-case
// memory is 2 × 64 KiB × MAX_TCP_SOCKETS(8) = 1 MiB.
const TCP_BUF_SIZE: usize = 65535;
const UDP_BUF_SIZE: usize = 2048;

// Default static IP (overridden by SYS_DHCP_DISCOVER or SYS_NET_SET_IP).
const DEFAULT_IP:   Ipv4Address = Ipv4Address::new(10, 0, 2, 15);
const DEFAULT_GW:   Ipv4Address = Ipv4Address::new(10, 0, 2,  1);
const DEFAULT_MASK: u8          = 24;

// ── virtio-net Device wrapper ─────────────────────────────────────────────────

/// Adapts `crate::drivers::virtio_net` to smoltcp's `Device` trait.
struct VirtioNetDev;

/// Receive token — holds the frame bytes until `consume` is called.
struct RxTok(Vec<u8>);
/// Transmit token — calls virtio-net send on consume.
struct TxTok(usize); // max frame size

impl RxToken for RxTok {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.0)
    }
}

impl TxToken for TxTok {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        // smoltcp gives us `len` and a closure that fills the buffer.
        let mut buf = vec![0u8; len.min(self.0)];
        let result = f(&mut buf);
        let _ = crate::drivers::virtio_net::send_raw(&buf);
        result
    }
}

impl Device for VirtioNetDev {
    type RxToken<'a> = RxTok;
    type TxToken<'a> = TxTok;

    fn receive(&mut self, _ts: Instant) -> Option<(RxTok, TxTok)> {
        let mut buf = vec![0u8; 1514];
        match crate::drivers::virtio_net::recv_raw(&mut buf) {
            Some(n) => {
                buf.truncate(n);
                Some((RxTok(buf), TxTok(1514)))
            }
            None => None,
        }
    }

    fn transmit(&mut self, _ts: Instant) -> Option<TxTok> {
        if crate::drivers::virtio_net::is_ready() {
            Some(TxTok(1514))
        } else {
            None
        }
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = 1514;
        caps.max_burst_size = Some(1);
        caps
    }
}

// ── Monotonic timestamp ───────────────────────────────────────────────────────

fn now() -> Instant {
    // Real monotonic clock (rdtsc-based, same source as timerfds). smoltcp
    // uses this for ACK-delay/retransmit/window timers — it must track REAL
    // time. The previous source (compositor frame counter ×17ms) crawls
    // whenever the compositor isn't presenting (early boot, headless, busy
    // CPU): smoltcp then thought ~10ms passed per ~150ms of reality, so every
    // TCP timer ran ~15× slow and bulk transfers paced at ~28 KB/s no matter
    // how large the buffers were (a 5.4 MB fetch took a constant ~193 s).
    Instant::from_millis((crate::syscall::poll::monotonic_ns() / 1_000_000) as i64)
}

// ── Global state ──────────────────────────────────────────────────────────────

struct TcpStack {
    iface:   Interface,
    sockets: SocketSet<'static>,
    handles: [Option<SocketHandle>; MAX_TCP_SOCKETS],
    // spare handle for ICMPv4 ping echo replies
    icmp_handle: Option<SocketHandle>,
    // Phase 52: DHCP socket
    dhcp_handle: Option<SocketHandle>,
    // DNS resolver socket (servers seeded from DHCP; default public resolver).
    dns_handle: Option<SocketHandle>,
    ip:      Ipv4Address,
}

// We can't put TcpStack directly in a Mutex because Interface/SocketSet are
// not Send+Sync generically, but since we're single-threaded (no SMP yet)
// we use UnsafeCell inside a spin lock guard pattern.
use core::cell::UnsafeCell;

struct TcpStackCell(UnsafeCell<Option<TcpStack>>);
unsafe impl Sync for TcpStackCell {}

static TCP_STACK: TcpStackCell = TcpStackCell(UnsafeCell::new(None));
static TCP_LOCK: spin::Mutex<()> = spin::Mutex::new(());

fn with_stack<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut TcpStack) -> R,
{
    let _g = TCP_LOCK.lock();
    let stack = unsafe { &mut *TCP_STACK.0.get() };
    stack.as_mut().map(f)
}

// ── Initialisation ────────────────────────────────────────────────────────────

pub fn init_smoltcp() {
    if !crate::drivers::virtio_net::is_ready() {
        log::info!("[smoltcp] virtio-net not ready — TCP stack not started");
        return;
    }

    // Read MAC from the virtio-net driver.
    let mut mac_buf = [0u8; 64];
    let _n = crate::drivers::virtio_net::info_text(&mut mac_buf);
    // Parse "MAC: XX:XX:XX:XX:XX:XX" — just use a fixed placeholder if parsing fails.
    let mac = parse_mac_from_info(&mac_buf).unwrap_or([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);

    let eth_addr = EthernetAddress(mac);
    let ip_cidr = IpCidr::new(IpAddress::Ipv4(DEFAULT_IP), DEFAULT_MASK);

    let config = Config::new(eth_addr.into());
    let mut dev = VirtioNetDev;
    let mut iface = Interface::new(config, &mut dev, now());
    iface.update_ip_addrs(|addrs| {
        addrs.push(ip_cidr).ok();
    });
    iface.routes_mut()
        .add_default_ipv4_route(DEFAULT_GW)
        .ok();

    // Allocate sockets.
    let mut sockets = SocketSet::new(vec![]);
    // Pre-create an ICMP socket for ping.
    let icmp_rx = icmp_sock::PacketBuffer::new(
        vec![icmp_sock::PacketMetadata::EMPTY; 8],
        vec![0u8; 512],
    );
    let icmp_tx = icmp_sock::PacketBuffer::new(
        vec![icmp_sock::PacketMetadata::EMPTY; 8],
        vec![0u8; 512],
    );
    let icmp_socket = IcmpSocket::new(icmp_rx, icmp_tx);
    let icmp_handle = sockets.add(icmp_socket);

    // DNS resolver. Seed with a public resolver so name lookups work even before
    // DHCP; the DHCP ACK then swaps in the network's own servers (update_servers).
    let dns_socket = DnsSocket::new(
        &[IpAddress::Ipv4(Ipv4Address::new(8, 8, 8, 8))],
        vec![None, None],
    );
    let dns_handle = sockets.add(dns_socket);

    let _g = TCP_LOCK.lock();
    let stack = unsafe { &mut *TCP_STACK.0.get() };
    *stack = Some(TcpStack {
        iface,
        sockets,
        handles: [None; MAX_TCP_SOCKETS],
        icmp_handle: Some(icmp_handle),
        dhcp_handle: None,
        dns_handle: Some(dns_handle),
        ip: DEFAULT_IP,
    });

    let octs = DEFAULT_IP.octets();
    log::info!("[smoltcp] TCP/IP stack initialised IP={}.{}.{}.{}",
        octs[0], octs[1], octs[2], octs[3]);
}

fn parse_mac_from_info(buf: &[u8]) -> Option<[u8; 6]> {
    // Look for "MAC: " prefix.
    let s = core::str::from_utf8(buf).ok()?;
    let start = s.find("MAC: ")? + 5;
    let mac_str = &s[start..].get(..17)?;
    let parts: Vec<&str> = mac_str.split(':').collect();
    if parts.len() != 6 { return None; }
    let mut out = [0u8; 6];
    for (i, p) in parts.iter().enumerate() {
        out[i] = u8::from_str_radix(p, 16).ok()?;
    }
    Some(out)
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Poll smoltcp (process incoming packets, advance TCP state machines).
/// Must be called before any socket operation.
pub fn poll() {
    with_stack(|s| {
        let mut dev = VirtioNetDev;
        s.iface.poll(now(), &mut dev, &mut s.sockets);
    });
}

/// Open a TCP connection to (dst_ip BE u32, dst_port).
/// Returns a socket fd [0..MAX_TCP_SOCKETS) or negative errno.
pub fn tcp_connect(dst_ip: u32, dst_port: u16) -> Result<usize, i64> {
    let _g = TCP_LOCK.lock();
    let stack = unsafe { &mut *TCP_STACK.0.get() };
    let s = stack.as_mut().ok_or(-1i64)?;

    // Find a free slot.
    let slot = s.handles.iter().position(|h| h.is_none()).ok_or(-23i64)?; // ENFILE

    let rx_buf = tcp_sock::SocketBuffer::new(vec![0u8; TCP_BUF_SIZE]);
    let tx_buf = tcp_sock::SocketBuffer::new(vec![0u8; TCP_BUF_SIZE]);
    let mut socket = TcpSocket::new(rx_buf, tx_buf);

    let bytes = dst_ip.to_be_bytes();
    let dst_addr = Ipv4Address::new(bytes[0], bytes[1], bytes[2], bytes[3]);
    // Ephemeral local port from a monotonic counter — NOT the slot index. A
    // slot-derived port meant two back-to-back connections through slot 0 to
    // the same server reused the IDENTICAL 4-tuple; the peer's TIME_WAIT state
    // then swallowed the new SYN and the second connect always timed out
    // (observed: pkg catalog fetch OK, immediately-following bundle fetch dead).
    static NEXT_EPHEMERAL: core::sync::atomic::AtomicU16 =
        core::sync::atomic::AtomicU16::new(0);
    let n = NEXT_EPHEMERAL.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let local_port = 49152 + (n % 16384);
    socket
        .connect(
            s.iface.context(),
            IpEndpoint::new(IpAddress::Ipv4(dst_addr), dst_port),
            local_port,
        )
        .map_err(|_| -111i64)?; // ECONNREFUSED

    let handle = s.sockets.add(socket);
    s.handles[slot] = Some(handle);

    // Kick the stack to send SYN.
    let mut dev = VirtioNetDev;
    s.iface.poll(now(), &mut dev, &mut s.sockets);

    Ok(slot)
}

/// Write data into TCP socket `fd`.
pub fn tcp_write(fd: usize, data: &[u8]) -> Result<usize, i64> {
    poll();
    let _g = TCP_LOCK.lock();
    let stack = unsafe { &mut *TCP_STACK.0.get() };
    let s = stack.as_mut().ok_or(-1i64)?;
    let handle = s.handles.get(fd).and_then(|h| *h).ok_or(-9i64)?; // EBADF
    let socket = s.sockets.get_mut::<TcpSocket>(handle);
    if !socket.can_send() { return Err(-11); } // EAGAIN
    let n = socket.send_slice(data).map_err(|_| -5i64)?; // EIO
    Ok(n)
}

/// Read data from TCP socket `fd`.
pub fn tcp_read(fd: usize, buf: &mut [u8]) -> Result<usize, i64> {
    poll();
    let _g = TCP_LOCK.lock();
    let stack = unsafe { &mut *TCP_STACK.0.get() };
    let s = stack.as_mut().ok_or(-1i64)?;
    let handle = s.handles.get(fd).and_then(|h| *h).ok_or(-9i64)?;
    let socket = s.sockets.get_mut::<TcpSocket>(handle);
    if !socket.can_recv() { return Err(-11); } // EAGAIN
    let n = socket.recv_slice(buf).map_err(|_| -5i64)?;
    Ok(n)
}

/// True once the three-way handshake has completed (the socket may send).
/// `Ok(false)` while still connecting; `Err(-111)` if the connection was
/// refused / reset (socket left the open states).
///
/// NOTE this is the correct way to wait for an outbound connect. A zero-byte
/// `tcp_read` probe does NOT work: `can_recv()` is false on an ESTABLISHED
/// socket whose peer hasn't sent anything yet (an HTTP server says nothing
/// until it gets the request), so the probe EAGAINs forever.
pub fn tcp_is_established(fd: usize) -> Result<bool, i64> {
    poll();
    let _g = TCP_LOCK.lock();
    let stack = unsafe { &mut *TCP_STACK.0.get() };
    let s = stack.as_mut().ok_or(-1i64)?;
    let handle = s.handles.get(fd).and_then(|h| *h).ok_or(-9i64)?;
    let socket = s.sockets.get_mut::<TcpSocket>(handle);
    if !socket.is_open() {
        return Err(-111); // ECONNREFUSED — closed/reset during connect
    }
    Ok(socket.may_send())
}

/// Close a TCP socket.
pub fn tcp_close(fd: usize) -> Result<(), i64> {
    poll();
    let _g = TCP_LOCK.lock();
    let stack = unsafe { &mut *TCP_STACK.0.get() };
    let s = stack.as_mut().ok_or(-1i64)?;
    if let Some(Some(handle)) = s.handles.get(fd) {
        let socket = s.sockets.get_mut::<TcpSocket>(*handle);
        socket.close();
        s.sockets.remove(*handle);
        s.handles[fd] = None;
    }
    Ok(())
}

/// Resolve a hostname to an IPv4 address (big-endian u32) via the DNS socket.
/// Drives the stack until the query completes or a ~5s timeout. Returns the
/// first A record, or a negative errno (-22 bad name, -2 failed/no A, -110 timeout).
pub fn dns_resolve(name: &str) -> Result<u32, i64> {
    let _g = TCP_LOCK.lock();
    let stack = unsafe { &mut *TCP_STACK.0.get() };
    let s = stack.as_mut().ok_or(-1i64)?;
    let dns_h = s.dns_handle.ok_or(-1i64)?;

    // Start an A-record query (disjoint &mut borrows of iface + sockets).
    let handle = {
        let cx = s.iface.context();
        let sock = s.sockets.get_mut::<DnsSocket>(dns_h);
        sock.start_query(cx, name, DnsQueryType::A).map_err(|_| -22i64)?
    };

    // Poll the stack until the resolver answers or we time out.
    let start = crate::arch::rdtsc_ns();
    const TIMEOUT_NS: u64 = 5_000_000_000;
    loop {
        {
            let mut dev = VirtioNetDev;
            s.iface.poll(now(), &mut dev, &mut s.sockets);
        }
        let sock = s.sockets.get_mut::<DnsSocket>(dns_h);
        match sock.get_query_result(handle) {
            Ok(addrs) => {
                for a in addrs.iter() {
                    if let IpAddress::Ipv4(v4) = a {
                        return Ok(u32::from_be_bytes(v4.octets()));
                    }
                }
                return Err(-2); // resolved but no A record
            }
            Err(GetQueryResultError::Pending) => {}
            Err(_) => return Err(-2), // failed
        }
        if crate::arch::rdtsc_ns().wrapping_sub(start) > TIMEOUT_NS {
            s.sockets.get_mut::<DnsSocket>(dns_h).cancel_query(handle);
            return Err(-110); // ETIMEDOUT
        }
    }
}

/// Query current IP address (big-endian u32).
pub fn current_ip_be() -> u32 {
    with_stack(|s| {
        u32::from_be_bytes(s.ip.octets())
    })
    .unwrap_or(0)
}

/// Set the interface IP (used by DHCP response handler).
pub fn set_ip(ip: Ipv4Address) {
    let _g = TCP_LOCK.lock();
    let stack = unsafe { &mut *TCP_STACK.0.get() };
    if let Some(s) = stack.as_mut() {
        s.ip = ip;
        let cidr = IpCidr::new(IpAddress::Ipv4(ip), DEFAULT_MASK);
        s.iface.update_ip_addrs(|a| { a.clear(); a.push(cidr).ok(); });
    }
}

/// Phase 52 — Real DHCP DISCOVER/OFFER/REQUEST/ACK using smoltcp socket-dhcpv4.
/// Polls the smoltcp stack up to ~3 seconds waiting for an ACK.
/// Returns the assigned IP as a big-endian u32, or the static fallback.
pub fn dhcp_discover() -> u32 {
    // Create and add a DHCP socket if not already present.
    {
        let _g = TCP_LOCK.lock();
        let stack = unsafe { &mut *TCP_STACK.0.get() };
        if let Some(s) = stack.as_mut() {
            if s.dhcp_handle.is_none() {
                let dhcp = DhcpSocket::new();
                let handle = s.sockets.add(dhcp);
                s.dhcp_handle = Some(handle);
            }
        } else {
            return 0;
        }
    }

    // Poll up to ~3 s (180 × 17 ms busy-wait iterations).
    for _ in 0..180 {
        let mut got_ip: Option<u32> = None;
        {
            let _g = TCP_LOCK.lock();
            let stack = unsafe { &mut *TCP_STACK.0.get() };
            if let Some(s) = stack.as_mut() {
                let mut dev = VirtioNetDev;
                s.iface.poll(now(), &mut dev, &mut s.sockets);
                let mut dhcp_dns: Option<Vec<IpAddress>> = None;
                if let Some(h) = s.dhcp_handle {
                    let sock = s.sockets.get_mut::<DhcpSocket>(h);
                    if let Some(event) = sock.poll() {
                        match event {
                            DhcpEvent::Configured(cfg) => {
                                let ip = cfg.address.address();
                                let octs = ip.octets();
                                log::info!("[dhcp] ACK ip={}.{}.{}.{}",
                                    octs[0], octs[1], octs[2], octs[3]);
                                got_ip = Some(u32::from_be_bytes(octs));
                                let cidr = IpCidr::new(IpAddress::Ipv4(ip),
                                    cfg.address.prefix_len());
                                s.iface.update_ip_addrs(|a| { a.clear(); a.push(cidr).ok(); });
                                if let Some(gw) = cfg.router {
                                    s.iface.routes_mut().add_default_ipv4_route(gw).ok();
                                }
                                s.ip = ip;
                                if !cfg.dns_servers.is_empty() {
                                    let mut v: Vec<IpAddress> = Vec::new();
                                    for a in cfg.dns_servers.iter() {
                                        v.push(IpAddress::Ipv4(*a));
                                    }
                                    dhcp_dns = Some(v);
                                }
                            }
                            DhcpEvent::Deconfigured => {
                                log::info!("[dhcp] deconfigured");
                            }
                        }
                    }
                }
                // Point the resolver at the network's own DNS servers (the DHCP
                // socket borrow has ended, so s.sockets is free again).
                if let (Some(dns_h), Some(servers)) = (s.dns_handle, dhcp_dns) {
                    s.sockets.get_mut::<DnsSocket>(dns_h).update_servers(&servers);
                    log::info!("[dns] {} resolver(s) from DHCP", servers.len());
                }
            }
        }
        if let Some(ip) = got_ip { return ip; }
        for _ in 0..100_000u64 { core::hint::spin_loop(); }
    }

    log::warn!("[dhcp] timed out — keeping static IP");
    u32::from_be_bytes(DEFAULT_IP.octets())
}
