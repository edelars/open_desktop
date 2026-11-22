//! ICMP echo of the NAT backend (S11-31c, NETWORK.md sections 9 and 12.2).
//!
//! Echo request to the gateway or DNS address: a synthetic reply, the packet never leaves the
//! backend. Echo request to an external address: the message is sent through an unprivileged ICMP
//! socket (`SOCK_DGRAM`, `IPPROTO_ICMP`; macOS ARM64 only, see [`open_icmp_socket`]), one socket per
//! request, and the reply is given back to the guest with the guest's identifier and sequence
//! number. No reply within 5 s (L14) leaves the guest in silence (`icmp_echo_timeout`). At most 8
//! requests wait at once (L6). Other ICMP types and codes are `drop_unsupported_proto`.
//!
//! Flow key of an echo request: the identifier is `guest_port` and the sequence number is
//! `dst_port`, so every request is a flow of its own and L6 counts requests, not pings. A guest
//! that repeats a request still waiting for its reply replaces the old flow.
//!
//! Counters. Every echo request (type 8, code 0) that reaches this handler increases
//! `icmp_echo_requests` once: that is its place in the frame accounting identity (`tx_frames` = the
//! sum of the per-frame counters). Other ICMP messages are `drop_unsupported_proto` and are in the
//! identity through that counter. What happens to a request afterwards is an outcome and is outside
//! the identity: `icmp_echo_synthetic`, `icmp_echo_forwarded`, `drop_flow_limit_icmp`,
//! `drop_host_resource` (also raised by TCP). A reply lost to a full L3 queue is counted by
//! `drop_rx_queue_full`, which the identity sums for ARP: the identity holds only while L3 does not
//! overflow (the accounting tests drain it), as for TCP.
//!
//! The ICMP errors for the guest and their rate limit (L15) are in [`super::ctx::Ctx`].

use std::collections::HashMap;
use std::io::{self, ErrorKind};
use std::net::{Ipv4Addr, UdpSocket};
use std::time::{Duration, Instant};

use super::bump;
use super::ctx::Ctx;
use super::flow::{FlowKey, FlowProto};
use super::frame::{DstClass, IcmpMessage, Ipv4Packet, PROTO_ICMP, internet_checksum};
use super::udp::MAX_UDP_DATA;

/// L14: how long an echo request to an external address waits for its reply.
pub const ECHO_TIMEOUT: Duration = Duration::from_secs(5);
/// L13: largest echo data (1500 - 20 - 8).
pub const MAX_ECHO_DATA: usize = MAX_UDP_DATA;

pub const ICMP_CODE_NET_UNREACHABLE: u8 = 0;
pub const ICMP_CODE_HOST_UNREACHABLE: u8 = 1;
pub const ICMP_CODE_PORT_UNREACHABLE: u8 = 3;
const ICMP_ECHO_REPLY: u8 = 0;
const ICMP_ECHO_REQUEST: u8 = 8;
const ICMP_HEADER_LEN: usize = 8;
/// Replies of the host are read into a buffer a little larger than any frame can carry.
const RECV_BUFFER_LEN: usize = 2048;
/// Datagrams read from one echo socket per service call that do not belong to the request.
const ECHO_RECV_BATCH: usize = 4;

struct EchoFlow {
    socket: UdpSocket,
    deadline: Instant,
}

/// An unprivileged ICMP socket connected to `dst` (NETWORK.md section 9). On macOS it is
/// `socket(AF_INET, SOCK_DGRAM, IPPROTO_ICMP)`, as in the source of `ping` for a non-root user; the
/// kernel returns the reply with its IP header and does not compute the ICMP checksum, which the
/// guest's message already carries. The descriptor is wrapped in a std `UdpSocket` for `connect`,
/// `send` and `recv`.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn open_icmp_socket(dst: Ipv4Addr) -> io::Result<UdpSocket> {
    use std::os::fd::{FromRawFd, OwnedFd};

    // SAFETY: socket() takes no pointers; a negative result is an error and no descriptor exists.
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, libc::IPPROTO_ICMP) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fd is a descriptor that socket() just returned and nothing else owns.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: fcntl with F_SETFD takes an integer flag and no pointer; fd is open.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let socket = UdpSocket::from(owned);
    socket.set_nonblocking(true)?;
    socket.connect(std::net::SocketAddrV4::new(dst, 0))?;
    Ok(socket)
}

/// Where unprivileged ICMP sockets are not implemented (NETWORK.md section 9, last paragraph):
/// external echo gets no reply, the synthetic echo of the gateway and DNS addresses still works.
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn open_icmp_socket(_dst: Ipv4Addr) -> io::Result<UdpSocket> {
    Err(io::Error::new(
        ErrorKind::Unsupported,
        "unprivileged ICMP sockets are implemented for macOS on ARM64 only",
    ))
}

/// The ICMP message of an echo reply (type 0, code 0) with its checksum, written to `out`.
fn write_echo_reply(
    ident: u16,
    seq: u16,
    data: &[u8],
    out: &mut [u8; ICMP_HEADER_LEN + MAX_ECHO_DATA],
) -> usize {
    let len = ICMP_HEADER_LEN + data.len();
    out[0] = ICMP_ECHO_REPLY;
    out[1] = 0;
    out[2..4].copy_from_slice(&[0, 0]);
    out[4..6].copy_from_slice(&ident.to_be_bytes());
    out[6..8].copy_from_slice(&seq.to_be_bytes());
    out[ICMP_HEADER_LEN..len].copy_from_slice(data);
    let checksum = internet_checksum(&out[..len]);
    out[2..4].copy_from_slice(&checksum.to_be_bytes());
    len
}

/// The data of the echo reply that a host sent to an ICMP socket, without the IP header that macOS
/// prepends. `None` for anything that is not a well-formed echo reply with the expected identifier
/// and sequence number.
fn parse_echo_reply(datagram: &[u8], ident: u16, seq: u16) -> Option<&[u8]> {
    let message = match datagram.first() {
        Some(first) if first >> 4 == 4 => {
            let header_len = usize::from(first & 0x0F) * 4;
            datagram.get(header_len.max(20)..)?
        }
        _ => datagram,
    };
    if message.len() < ICMP_HEADER_LEN
        || message[0] != ICMP_ECHO_REPLY
        || message[1] != 0
        || message[4..6] != ident.to_be_bytes()
        || message[6..8] != seq.to_be_bytes()
    {
        return None;
    }
    Some(&message[ICMP_HEADER_LEN..])
}

/// The reply of the host for the guest: from the pinged address, with the guest's identifier and
/// sequence number.
fn send_echo_reply(ctx: &mut Ctx<'_>, key: FlowKey, data: &[u8]) {
    if data.len() > MAX_ECHO_DATA {
        bump(&mut ctx.counters.drop_oversize_reply);
        return;
    }
    let mut message = [0u8; ICMP_HEADER_LEN + MAX_ECHO_DATA];
    let len = write_echo_reply(key.guest_port, key.dst_port, data, &mut message);
    ctx.send_ipv4(key.dst_ip, PROTO_ICMP, &message[..len]);
}

/// The ICMP echo handler: the sockets and deadlines of the requests that wait for a reply (the flow
/// table of the backend counts them for L6).
pub(super) struct EchoEngine {
    flows: HashMap<FlowKey, EchoFlow>,
    /// Test-only: socket creation fails as with exhausted descriptors, so that tests that feed
    /// arbitrary frames never reach the network of the machine they run on.
    #[cfg(test)]
    sockets_denied: bool,
}

impl EchoEngine {
    pub(super) fn new() -> Self {
        Self {
            flows: HashMap::new(),
            #[cfg(test)]
            sockets_denied: false,
        }
    }

    #[cfg(test)]
    pub(super) fn deny_sockets(&mut self) {
        self.sockets_denied = true;
    }

    /// Reset of the NIC or the machine: closing the sockets, telling the guest nothing.
    pub(super) fn clear(&mut self) {
        self.flows.clear();
    }

    /// An ICMP message from the guest that passed the checks of NETWORK.md section 5.
    pub(super) fn handle(
        &mut self,
        ctx: &mut Ctx<'_>,
        packet: &Ipv4Packet<'_>,
        icmp: &IcmpMessage<'_>,
        now: Instant,
    ) {
        if icmp.icmp_type != ICMP_ECHO_REQUEST || icmp.code != 0 {
            bump(&mut ctx.counters.drop_unsupported_proto);
            return;
        }
        bump(&mut ctx.counters.icmp_echo_requests);
        match packet.dst_class {
            DstClass::Gateway | DstClass::Dns => synthetic_reply(ctx, packet, icmp),
            DstClass::External => self.forward(ctx, packet, icmp, now),
            // Only UDP gets through to the limited broadcast address (NETWORK.md section 5).
            DstClass::LimitedBroadcast => bump(&mut ctx.counters.drop_multicast_broadcast),
        }
    }

    fn forward(
        &mut self,
        ctx: &mut Ctx<'_>,
        packet: &Ipv4Packet<'_>,
        icmp: &IcmpMessage<'_>,
        now: Instant,
    ) {
        let key = FlowKey {
            proto: FlowProto::IcmpEcho,
            guest_port: u16::from_be_bytes([icmp.rest[0], icmp.rest[1]]),
            dst_ip: packet.dst,
            dst_port: u16::from_be_bytes([icmp.rest[2], icmp.rest[3]]),
        };
        // A repeated request replaces the one that still waits (the guest gave up on it).
        if self.flows.remove(&key).is_some() {
            ctx.close_flow(&key);
        }
        // L6: open_flow counts drop_flow_limit_icmp itself.
        if ctx.open_flow(key).is_err() {
            return;
        }
        let sent = self
            .connected_socket(packet.dst)
            .and_then(|socket| socket.send(packet.payload).map(|_| socket));
        match sent {
            Ok(socket) => {
                self.flows.insert(
                    key,
                    EchoFlow {
                        socket,
                        deadline: now + ECHO_TIMEOUT,
                    },
                );
                bump(&mut ctx.counters.icmp_echo_forwarded);
            }
            Err(_) => {
                // Descriptors, no route, a refusing kernel: the request is lost like a lost packet.
                ctx.close_flow(&key);
                bump(&mut ctx.counters.drop_host_resource);
            }
        }
    }

    fn connected_socket(&self, dst: Ipv4Addr) -> io::Result<UdpSocket> {
        #[cfg(test)]
        if self.sockets_denied {
            // Stands for the exhaustion of descriptors (EMFILE).
            return Err(io::Error::from_raw_os_error(24));
        }
        open_icmp_socket(dst)
    }

    /// Reads the replies of the waiting echo requests and removes the ones that timed out.
    pub(super) fn service(&mut self, ctx: &mut Ctx<'_>, now: Instant) {
        self.flows
            .retain(|key, flow| poll_flow(ctx, *key, flow, now));
    }
}

fn synthetic_reply(ctx: &mut Ctx<'_>, packet: &Ipv4Packet<'_>, icmp: &IcmpMessage<'_>) {
    let ident = u16::from_be_bytes([icmp.rest[0], icmp.rest[1]]);
    let seq = u16::from_be_bytes([icmp.rest[2], icmp.rest[3]]);
    let mut message = [0u8; ICMP_HEADER_LEN + MAX_ECHO_DATA];
    // The data is at most 1472 bytes: the frame it came in is at most 1514.
    let len = write_echo_reply(ident, seq, icmp.data, &mut message);
    if ctx.send_ipv4(packet.dst, PROTO_ICMP, &message[..len]) {
        bump(&mut ctx.counters.icmp_echo_synthetic);
    }
}

fn poll_flow(ctx: &mut Ctx<'_>, key: FlowKey, flow: &mut EchoFlow, now: Instant) -> bool {
    let mut buf = [0u8; RECV_BUFFER_LEN];
    for _ in 0..ECHO_RECV_BATCH {
        match flow.socket.recv(&mut buf) {
            Ok(len) => {
                // Whatever else the kernel hands over (errors, other ICMP) is not our reply.
                if let Some(data) = parse_echo_reply(&buf[..len], key.guest_port, key.dst_port) {
                    ctx.close_flow(&key);
                    send_echo_reply(ctx, key, data);
                    return false;
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(_) => {
                ctx.close_flow(&key);
                bump(&mut ctx.counters.drop_host_resource);
                return false;
            }
        }
    }
    if now >= flow.deadline {
        ctx.close_flow(&key);
        bump(&mut ctx.counters.icmp_echo_timeout);
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, UdpSocket};
    use std::time::{Duration, Instant};

    use od_machine::{NET_MAX_FRAME, NetBackend};

    use super::super::NatBackend;
    use super::super::flow::{FlowKey, FlowProto, MAX_ICMP_ECHO_FLOWS};
    use super::super::frame::{DNS_IP, GATEWAY_IP, PROTO_ICMP};
    use super::super::test_frames::{Ip, icmp_message, ipv4_frame, ref_checksum};
    use super::super::test_io::{drain, io_backend, parse_guest_packet};
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    use super::super::test_io::{pump_for, pump_quiet};
    use super::*;

    const LOOP: Ipv4Addr = Ipv4Addr::LOCALHOST;

    fn echo(dst: Ipv4Addr, icmp_type: u8, code: u8, ident: u16, seq: u16, data: &[u8]) -> Vec<u8> {
        ipv4_frame(
            &Ip::to(dst, PROTO_ICMP),
            &icmp_message(icmp_type, code, ident, seq, data),
        )
    }

    fn send(backend: &mut NatBackend, frame: &[u8], now: Instant) {
        backend.transmit(frame).unwrap();
        backend.process_pending_at(now);
    }

    /// (type, code, identifier, sequence, data) of an ICMP message for the guest, checksum verified.
    fn echo_of(frame: &[u8]) -> (Ipv4Addr, u8, u8, u16, u16, Vec<u8>) {
        let packet = parse_guest_packet(frame);
        assert_eq!(packet.protocol, PROTO_ICMP);
        let m = &packet.l4;
        assert_eq!(ref_checksum(&[m]), 0, "ICMP checksum");
        (
            packet.src,
            m[0],
            m[1],
            u16::from_be_bytes([m[4], m[5]]),
            u16::from_be_bytes([m[6], m[7]]),
            m[8..].to_vec(),
        )
    }

    /// A socket that never receives anything, standing in for a pending request in the flow table.
    fn silent_socket() -> UdpSocket {
        let socket = UdpSocket::bind((LOOP, 0)).unwrap();
        socket.set_nonblocking(true).unwrap();
        socket
    }

    fn pending_flow(backend: &mut NatBackend, seq: u16, deadline: Instant) -> FlowKey {
        let key = FlowKey {
            proto: FlowProto::IcmpEcho,
            guest_port: 7,
            dst_ip: LOOP,
            dst_port: seq,
        };
        backend.open_flow(key).unwrap();
        backend.echo.flows.insert(
            key,
            EchoFlow {
                socket: silent_socket(),
                deadline,
            },
        );
        key
    }

    #[test]
    fn gateway_and_dns_answer_echo_with_a_synthetic_reply() {
        let mut b = io_backend();
        for (n, dst) in [GATEWAY_IP, DNS_IP].into_iter().enumerate() {
            let seq = 100 + n as u16;
            send(
                &mut b,
                &echo(dst, 8, 0, 0x1234, seq, b"ping data"),
                Instant::now(),
            );
            let frames = drain(&mut b);
            assert_eq!(frames.len(), 1, "{dst}");
            assert_eq!(
                echo_of(&frames[0]),
                (dst, 0, 0, 0x1234, seq, b"ping data".to_vec()),
                "{dst}"
            );
        }
        let c = b.counters();
        assert_eq!((c.icmp_echo_synthetic, c.icmp_echo_forwarded), (2, 0));
        assert_eq!(
            c.icmp_echo_requests, 2,
            "one per request, whatever came of it"
        );
        assert_eq!(
            b.flows.count(FlowProto::IcmpEcho),
            0,
            "nothing waits, nothing left the backend"
        );
    }

    #[test]
    fn synthetic_echo_handles_empty_and_maximal_data() {
        let mut b = io_backend();
        send(&mut b, &echo(GATEWAY_IP, 8, 0, 1, 1, &[]), Instant::now());
        assert_eq!(drain(&mut b).iter().map(Vec::len).collect::<Vec<_>>(), [42]);
        let data = vec![0x5Au8; MAX_ECHO_DATA];
        let frame = echo(GATEWAY_IP, 8, 0, 2, 2, &data);
        assert_eq!(frame.len(), NET_MAX_FRAME);
        send(&mut b, &frame, Instant::now());
        let frames = drain(&mut b);
        assert_eq!(frames[0].len(), NET_MAX_FRAME);
        assert_eq!(echo_of(&frames[0]).5, data);
        // Odd data length: the checksum covers the padding byte.
        send(&mut b, &echo(DNS_IP, 8, 0, 3, 3, b"odd"), Instant::now());
        assert_eq!(echo_of(&drain(&mut b)[0]).5, b"odd");
    }

    #[test]
    fn other_icmp_messages_are_unsupported_for_every_destination() {
        let mut b = io_backend();
        let mut expected = 0;
        for dst in [GATEWAY_IP, DNS_IP, Ipv4Addr::new(93, 184, 216, 34)] {
            for (icmp_type, code) in [(8, 1), (0, 0), (13, 0), (5, 1), (3, 3), (11, 0), (17, 0)] {
                send(
                    &mut b,
                    &echo(dst, icmp_type, code, 1, 1, b"x"),
                    Instant::now(),
                );
                expected += 1;
            }
        }
        let c = b.counters();
        assert_eq!(c.drop_unsupported_proto, expected);
        assert_eq!(
            c.icmp_echo_requests, 0,
            "not echo requests: they are in the identity through drop_unsupported_proto"
        );
        assert_eq!(
            c.icmp_echo_synthetic + c.icmp_echo_forwarded + c.drop_host_resource,
            0
        );
        assert!(drain(&mut b).is_empty());
    }

    #[test]
    fn echo_request_waits_exactly_5_seconds_then_is_forgotten() {
        assert_eq!(ECHO_TIMEOUT, Duration::from_secs(5));
        let mut b = io_backend();
        let t0 = Instant::now();
        let deadline = t0 + ECHO_TIMEOUT;
        let key = pending_flow(&mut b, 1, deadline);
        b.service_at(deadline - Duration::from_millis(1));
        assert!(
            b.echo.flows.contains_key(&key),
            "alive one millisecond before the deadline"
        );
        assert_eq!(b.counters().icmp_echo_timeout, 0);
        b.service_at(deadline);
        assert!(!b.echo.flows.contains_key(&key));
        assert_eq!(b.flows.count(FlowProto::IcmpEcho), 0);
        assert_eq!(b.counters().icmp_echo_timeout, 1);
        assert!(drain(&mut b).is_empty(), "silence, as after a lost packet");
    }

    #[test]
    fn pending_echo_limit_l6_is_8_and_the_9th_is_dropped_without_a_socket() {
        let mut b = io_backend();
        let t0 = Instant::now();
        for seq in 0..MAX_ICMP_ECHO_FLOWS as u16 {
            pending_flow(&mut b, seq, t0 + ECHO_TIMEOUT);
        }
        assert_eq!(b.flows.count(FlowProto::IcmpEcho), 8);
        send(&mut b, &echo(LOOP, 8, 0, 7, 8, b"ninth"), t0);
        let c = b.counters();
        assert_eq!(
            (
                c.drop_flow_limit_icmp,
                c.icmp_echo_forwarded,
                c.drop_host_resource
            ),
            (1, 0, 0)
        );
        assert_eq!(b.echo.flows.len(), 8);
        // The slots of timed out requests are free again.
        b.service_at(t0 + ECHO_TIMEOUT);
        assert_eq!(b.echo.flows.len(), 0);
        assert_eq!(b.counters().icmp_echo_timeout, 8);
        assert_eq!(b.flows.count(FlowProto::IcmpEcho), 0);
        pending_flow(&mut b, 100, t0 + ECHO_TIMEOUT);
        assert_eq!(b.flows.count(FlowProto::IcmpEcho), 1);
    }

    #[test]
    fn exhausted_descriptors_lose_the_request_and_free_the_slot() {
        let mut b = io_backend().without_host_sockets();
        send(
            &mut b,
            &echo(Ipv4Addr::new(93, 184, 216, 34), 8, 0, 1, 1, b"x"),
            Instant::now(),
        );
        let c = b.counters();
        assert_eq!((c.drop_host_resource, c.icmp_echo_forwarded), (1, 0));
        assert_eq!(b.flows.count(FlowProto::IcmpEcho), 0);
        assert!(b.echo.flows.is_empty());
        assert!(drain(&mut b).is_empty());
    }

    #[test]
    fn reset_forgets_pending_requests() {
        let mut b = io_backend();
        pending_flow(&mut b, 1, Instant::now() + ECHO_TIMEOUT);
        b.reset();
        assert!(b.echo.flows.is_empty());
        assert_eq!(b.flows.count(FlowProto::IcmpEcho), 0);
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn external_echo_goes_through_the_host_icmp_socket_and_the_reply_keeps_the_guest_numbers() {
        let mut b = io_backend();
        for (seq, data) in [
            (1u16, &b"first"[..]),
            (2, &b""[..]),
            (3, &[0xC3u8; 1000][..]),
        ] {
            send(&mut b, &echo(LOOP, 8, 0, 0xBEEF, seq, data), Instant::now());
            let frames = pump_for(&mut b, 1);
            assert_eq!(frames.len(), 1, "reply to sequence {seq}");
            assert_eq!(
                echo_of(&frames[0]),
                (LOOP, 0, 0, 0xBEEF, seq, data.to_vec())
            );
            assert_eq!(
                b.flows.count(FlowProto::IcmpEcho),
                0,
                "the flow ends with the reply"
            );
        }
        let c = b.counters();
        assert_eq!(
            (
                c.icmp_echo_forwarded,
                c.icmp_echo_timeout,
                c.icmp_echo_synthetic
            ),
            (3, 0, 0)
        );
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn eight_requests_wait_together_and_all_get_their_replies() {
        let mut b = io_backend();
        let t0 = Instant::now();
        for seq in 0..MAX_ICMP_ECHO_FLOWS as u16 {
            send(&mut b, &echo(LOOP, 8, 0, 1, seq, b"x"), t0);
        }
        assert_eq!(b.counters().icmp_echo_forwarded, 8);
        send(&mut b, &echo(LOOP, 8, 0, 1, 8, b"x"), t0);
        assert_eq!(b.counters().drop_flow_limit_icmp, 1, "the 9th while 8 wait");
        let mut seqs: Vec<u16> = pump_for(&mut b, 8).iter().map(|f| echo_of(f).4).collect();
        seqs.sort_unstable();
        assert_eq!(seqs, (0..8).collect::<Vec<u16>>());
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn repeating_a_request_that_still_waits_replaces_it() {
        let mut b = io_backend();
        let frame = echo(LOOP, 8, 0, 5, 5, b"again");
        send(&mut b, &frame, Instant::now());
        send(&mut b, &frame, Instant::now());
        assert_eq!(b.echo.flows.len(), 1);
        assert_eq!(b.counters().icmp_echo_forwarded, 2);
        assert_eq!(pump_for(&mut b, 1).len(), 1);
        assert!(pump_quiet(&mut b, Duration::from_millis(100)).is_empty());
    }

    /// What NETWORK.md section 9 says about the unprivileged ICMP socket of macOS, on 127.0.0.1: the
    /// reply carries an IP header, keeps the identifier, and a request whose checksum is zero gets none.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn unprivileged_icmp_socket_keeps_the_identifier_and_does_not_fix_the_checksum() {
        // The reply to `seq`: the kernel also hands this socket copies of replies to other ICMP
        // sockets of the process (the tests run in parallel), which are skipped.
        let wait = |socket: &UdpSocket, seq: u16, window: Duration| -> Option<Vec<u8>> {
            let started = Instant::now();
            let mut buf = [0u8; 2048];
            while started.elapsed() < window {
                match socket.recv(&mut buf) {
                    Ok(len) if parse_echo_reply(&buf[..len], 0x1234, seq).is_some() => {
                        return Some(buf[..len].to_vec());
                    }
                    Ok(_) => {}
                    Err(_) => std::thread::sleep(Duration::from_millis(2)),
                }
            }
            None
        };
        let request = icmp_message(8, 0, 0x1234, 7, b"hello");
        let socket = open_icmp_socket(LOOP).unwrap();
        socket.send(&request).unwrap();
        let datagram =
            wait(&socket, 7, Duration::from_secs(2)).expect("a reply to a correct request");
        assert_eq!(datagram[0] >> 4, 4, "the reply comes with its IP header");
        assert_eq!(parse_echo_reply(&datagram, 0x1234, 7), Some(&b"hello"[..]));
        // The same request with the checksum field zero: no reply (the kernel does not fill it in).
        let mut unsummed = icmp_message(8, 0, 0x1234, 8, b"hello");
        unsummed[2..4].copy_from_slice(&[0, 0]);
        let socket = open_icmp_socket(LOOP).unwrap();
        socket.send(&unsummed).unwrap();
        assert!(wait(&socket, 8, Duration::from_millis(400)).is_none());
    }

    fn reply(ident: u16, seq: u16, data: &[u8]) -> Vec<u8> {
        icmp_message(0, 0, ident, seq, data)
    }

    #[test]
    fn host_replies_are_parsed_with_or_without_the_ip_header_and_checked() {
        let plain = reply(9, 4, b"abc");
        assert_eq!(parse_echo_reply(&plain, 9, 4), Some(&b"abc"[..]));
        // As macOS delivers it: IP header (20 bytes, IHL 5) first.
        let mut with_header = vec![
            0x45, 0, 0, 31, 0, 0, 0, 0, 64, 1, 0, 0, 127, 0, 0, 1, 127, 0, 0, 1,
        ];
        with_header.extend_from_slice(&plain);
        assert_eq!(parse_echo_reply(&with_header, 9, 4), Some(&b"abc"[..]));
        // IP options (IHL 6) are skipped too.
        let mut options = vec![
            0x46, 0, 0, 35, 0, 0, 0, 0, 64, 1, 0, 0, 127, 0, 0, 1, 127, 0, 0, 1, 0, 0, 0, 0,
        ];
        options.extend_from_slice(&plain);
        assert_eq!(parse_echo_reply(&options, 9, 4), Some(&b"abc"[..]));
        // Not ours: other identifier or sequence, other types, other codes, too short, cut header.
        assert_eq!(parse_echo_reply(&plain, 10, 4), None);
        assert_eq!(parse_echo_reply(&plain, 9, 5), None);
        assert_eq!(
            parse_echo_reply(&icmp_message(8, 0, 9, 4, b"abc"), 9, 4),
            None
        );
        assert_eq!(
            parse_echo_reply(&icmp_message(0, 1, 9, 4, b"abc"), 9, 4),
            None
        );
        assert_eq!(
            parse_echo_reply(&icmp_message(3, 1, 9, 4, b"abc"), 9, 4),
            None
        );
        assert_eq!(parse_echo_reply(&plain[..7], 9, 4), None);
        assert_eq!(parse_echo_reply(&[], 9, 4), None);
        assert_eq!(parse_echo_reply(&with_header[..19], 9, 4), None);
        assert_eq!(parse_echo_reply(&with_header[..20], 9, 4), None);
        let mut bad_ihl = with_header.clone();
        bad_ihl[0] = 0x4F;
        assert_eq!(
            parse_echo_reply(&bad_ihl, 9, 4),
            None,
            "header longer than the datagram"
        );
        bad_ihl[0] = 0x40;
        assert_eq!(
            parse_echo_reply(&bad_ihl, 9, 4),
            Some(&b"abc"[..]),
            "IHL below 5 is read as 5"
        );
    }

    #[test]
    fn echo_reply_larger_than_a_frame_is_dropped_as_oversize() {
        let mut b = io_backend();
        let key = FlowKey {
            proto: FlowProto::IcmpEcho,
            guest_port: 1,
            dst_ip: LOOP,
            dst_port: 1,
        };
        {
            let (_handlers, mut ctx) = b.handler_parts(Duration::ZERO);
            send_echo_reply(&mut ctx, key, &vec![0u8; MAX_ECHO_DATA + 1]);
        }
        assert_eq!(b.counters().drop_oversize_reply, 1);
        assert!(drain(&mut b).is_empty());
        {
            let (_handlers, mut ctx) = b.handler_parts(Duration::ZERO);
            send_echo_reply(&mut ctx, key, &vec![0u8; MAX_ECHO_DATA]);
        }
        assert_eq!(drain(&mut b)[0].len(), NET_MAX_FRAME);
    }
}
