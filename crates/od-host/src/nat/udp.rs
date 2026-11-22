//! UDP flows of the NAT backend (S11-31c, NETWORK.md section 10).
//!
//! One non-blocking host socket per flow, created on the first datagram and `connect`ed to the
//! destination (the kernel then filters foreign senders and reports delivery errors). A flow lives
//! 120 s (L12) after the last datagram in either direction; at most 64 flows exist (L5) and a new
//! datagram beyond that is dropped, existing flows are never evicted.
//!
//! Datagrams to the backend's own addresses are not forwarded: DHCP (`10.0.2.2:67`, broadcast) and
//! DNS (`10.0.2.3:53`) belong to S11-31b and stay in `drop_unhandled_l4` until it lands; any other
//! port there gets ICMP port unreachable (`gw_port_unreachable`).
//!
//! Counters. The fate of a datagram handled by [`NatBackend::process_pending_at`] is exactly one of
//! `udp_forwarded`, `drop_flow_limit_udp`, `drop_host_resource`, `udp_unreachable`, `udp_refused`
//! or `gw_port_unreachable`. The last three count the situation; the ICMP error that tells the guest
//! has its own counters (`icmp_error_sent`, `icmp_error_suppressed`, `drop_rx_queue_full`), as for
//! TCP. A host error that the kernel reports on a later read (a refusal after the send) is counted
//! the same way but belongs to no new frame.

use std::collections::HashMap;
use std::io::{self, ErrorKind};
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use super::bump;
use super::ctx::Ctx;
use super::flow::{FlowKey, FlowProto};
use super::frame::{
    DstClass, GATEWAY_IP, GUEST_IP, IPV4_HEADER_LEN, Ipv4Packet, PROTO_UDP, UdpDatagram,
    l4_checksum,
};
use super::icmp::{
    ICMP_CODE_HOST_UNREACHABLE, ICMP_CODE_NET_UNREACHABLE, ICMP_CODE_PORT_UNREACHABLE,
};

/// L12: idle time after which a UDP flow is removed.
pub const UDP_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
/// L13: largest UDP data in either direction (1500 - 20 - 8).
pub const MAX_UDP_DATA: usize = 1472;
/// Datagrams read from one flow per [`NatBackend::service_at`] call, so that a flooding flow cannot
/// starve the others; the rest stays in the kernel buffer.
const UDP_RECV_BATCH: usize = 32;
/// Bytes of the original packet that an ICMP error quotes: the IP header and 8 bytes (RFC 792).
pub(super) const ICMP_QUOTE_LEN: usize = IPV4_HEADER_LEN + 8;
const UDP_HEADER_LEN: usize = 8;
const DHCP_SERVER_PORT: u16 = 67;
const DNS_PORT: u16 = 53;

struct UdpFlow {
    socket: UdpSocket,
    last_activity: Instant,
    /// Header and first 8 bytes of the latest datagram sent: what a delayed host error is about.
    quote: [u8; ICMP_QUOTE_LEN],
}

/// What an I/O error of a UDP socket means for the guest (NETWORK.md section 12.2).
enum HostError {
    /// `ECONNREFUSED`: ICMP port unreachable (code 3), `udp_refused`.
    Refused,
    /// `ENETUNREACH`, `ENETDOWN`: ICMP network unreachable (code 0), `udp_unreachable`.
    NetUnreachable,
    /// `EHOSTUNREACH`: ICMP host unreachable (code 1), `udp_unreachable`.
    HostUnreachable,
    /// Anything else: the datagram is lost, `drop_host_resource`.
    Other,
}

fn host_error(error: &io::Error) -> HostError {
    match error.kind() {
        ErrorKind::ConnectionRefused => HostError::Refused,
        ErrorKind::NetworkUnreachable | ErrorKind::NetworkDown => HostError::NetUnreachable,
        ErrorKind::HostUnreachable => HostError::HostUnreachable,
        _ => HostError::Other,
    }
}

/// UDP to the backend's addresses that S11-31b serves (DHCP, DNS), not this module.
pub(super) fn is_dhcp_or_dns(packet: &Ipv4Packet<'_>, udp: &UdpDatagram<'_>) -> bool {
    match packet.dst_class {
        DstClass::LimitedBroadcast => true,
        DstClass::Gateway => udp.dst_port == DHCP_SERVER_PORT,
        DstClass::Dns => udp.dst_port == DNS_PORT,
        DstClass::External => false,
    }
}

/// The IP header and the first 8 bytes of the transport data of a packet (RFC 792) with the number
/// of meaningful bytes (shorter than [`ICMP_QUOTE_LEN`] only when the transport part is).
pub(super) fn icmp_quote(packet: &Ipv4Packet<'_>) -> ([u8; ICMP_QUOTE_LEN], usize) {
    let mut quote = [0u8; ICMP_QUOTE_LEN];
    let data = packet.payload.len().min(8);
    quote[..IPV4_HEADER_LEN].copy_from_slice(packet.header);
    quote[IPV4_HEADER_LEN..IPV4_HEADER_LEN + data].copy_from_slice(&packet.payload[..data]);
    (quote, IPV4_HEADER_LEN + data)
}

/// The UDP handler: the sockets and timers of the flows (the flow table of the backend counts them
/// for L5).
pub(super) struct UdpEngine {
    flows: HashMap<FlowKey, UdpFlow>,
    /// Test-only: socket creation fails as with exhausted descriptors, so that tests that feed
    /// arbitrary frames never reach the network of the machine they run on.
    #[cfg(test)]
    sockets_denied: bool,
}

impl UdpEngine {
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

    /// A datagram from the guest to an external address, or to a port of the backend's addresses
    /// that nobody serves.
    pub(super) fn handle(
        &mut self,
        ctx: &mut Ctx<'_>,
        packet: &Ipv4Packet<'_>,
        udp: &UdpDatagram<'_>,
        now: Instant,
    ) {
        // A UDP payload has at least the 8 header bytes, so the quote is always complete.
        let (quote, quote_len) = icmp_quote(packet);
        if matches!(packet.dst_class, DstClass::Gateway | DstClass::Dns) {
            bump(&mut ctx.counters.gw_port_unreachable);
            ctx.send_icmp_dest_unreachable_from(
                packet.dst,
                ICMP_CODE_PORT_UNREACHABLE,
                &quote[..quote_len],
                now,
            );
            return;
        }
        let key = FlowKey {
            proto: FlowProto::Udp,
            guest_port: udp.src_port,
            dst_ip: packet.dst,
            dst_port: udp.dst_port,
        };
        let is_new = !self.flows.contains_key(&key);
        if is_new && !self.open_flow(ctx, key, &quote, now) {
            return;
        }
        self.forward(ctx, key, udp.data, quote, now, is_new);
    }

    /// Admission and socket of a new flow; false when the datagram is lost (the counter is increased).
    fn open_flow(
        &mut self,
        ctx: &mut Ctx<'_>,
        key: FlowKey,
        quote: &[u8; ICMP_QUOTE_LEN],
        now: Instant,
    ) -> bool {
        // L5: open_flow counts drop_flow_limit_udp itself (Exists cannot happen, the flow is new).
        if ctx.open_flow(key).is_err() {
            return false;
        }
        let socket = match self.connected_socket(key) {
            Ok(socket) => socket,
            Err(error) => {
                ctx.close_flow(&key);
                report_host_error(ctx, now, &error, key, quote);
                return false;
            }
        };
        self.flows.insert(
            key,
            UdpFlow {
                socket,
                last_activity: now,
                quote: *quote,
            },
        );
        bump(&mut ctx.counters.udp_flows_opened);
        true
    }

    fn connected_socket(&self, key: FlowKey) -> io::Result<UdpSocket> {
        #[cfg(test)]
        if self.sockets_denied {
            // Stands for the exhaustion of descriptors (EMFILE).
            return Err(io::Error::from_raw_os_error(24));
        }
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        socket.set_nonblocking(true)?;
        socket.connect(SocketAddrV4::new(key.dst_ip, key.dst_port))?;
        Ok(socket)
    }

    /// Sends the datagram. A pending error of an earlier datagram (`ECONNREFUSED`) is reported for
    /// that datagram and the send is repeated once.
    fn forward(
        &mut self,
        ctx: &mut Ctx<'_>,
        key: FlowKey,
        data: &[u8],
        quote: [u8; ICMP_QUOTE_LEN],
        now: Instant,
        is_new: bool,
    ) {
        let mut retried = false;
        loop {
            let Some(flow) = self.flows.get_mut(&key) else {
                return;
            };
            match flow.socket.send(data) {
                Ok(_) => {
                    flow.last_activity = now;
                    flow.quote = quote;
                    bump(&mut ctx.counters.udp_forwarded);
                    return;
                }
                Err(error) if error.kind() == ErrorKind::ConnectionRefused && !retried => {
                    retried = true;
                    let earlier = flow.quote;
                    report_host_error(ctx, now, &error, key, &earlier);
                }
                Err(error) => {
                    if is_new {
                        self.flows.remove(&key);
                        ctx.close_flow(&key);
                    }
                    report_host_error(ctx, now, &error, key, &quote);
                    return;
                }
            }
        }
    }

    /// Reads replies and errors of every UDP flow and removes the idle ones. Never blocks.
    pub(super) fn service(&mut self, ctx: &mut Ctx<'_>, now: Instant) {
        self.flows
            .retain(|key, flow| poll_flow(ctx, *key, flow, now));
    }
}

/// Reports a host error to the guest as the table of NETWORK.md section 12.2 says: the datagram
/// `quote` names is answered with ICMP, or lost (`drop_host_resource`) for an unknown error.
fn report_host_error(
    ctx: &mut Ctx<'_>,
    now: Instant,
    error: &io::Error,
    key: FlowKey,
    quote: &[u8; ICMP_QUOTE_LEN],
) {
    // A port unreachable comes from the destination itself; network and host unreachable come from
    // the gateway, which is the router the guest would see.
    let (code, source, counter) = match host_error(error) {
        HostError::Refused => (
            ICMP_CODE_PORT_UNREACHABLE,
            key.dst_ip,
            &mut ctx.counters.udp_refused,
        ),
        HostError::NetUnreachable => (
            ICMP_CODE_NET_UNREACHABLE,
            GATEWAY_IP,
            &mut ctx.counters.udp_unreachable,
        ),
        HostError::HostUnreachable => (
            ICMP_CODE_HOST_UNREACHABLE,
            GATEWAY_IP,
            &mut ctx.counters.udp_unreachable,
        ),
        HostError::Other => {
            bump(&mut ctx.counters.drop_host_resource);
            return;
        }
    };
    bump(counter);
    ctx.send_icmp_dest_unreachable_from(source, code, quote, now);
}

fn poll_flow(ctx: &mut Ctx<'_>, key: FlowKey, flow: &mut UdpFlow, now: Instant) -> bool {
    // One byte more than the largest datagram tells an oversize reply from a maximal one.
    let mut buf = [0u8; MAX_UDP_DATA + 1];
    for _ in 0..UDP_RECV_BATCH {
        match flow.socket.recv(&mut buf) {
            Ok(len) => {
                flow.last_activity = now;
                if len > MAX_UDP_DATA {
                    bump(&mut ctx.counters.drop_oversize_reply);
                } else {
                    send_reply(ctx, key, &buf[..len]);
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => {
                report_host_error(ctx, now, &error, key, &flow.quote);
                if matches!(host_error(&error), HostError::Other) {
                    break;
                }
            }
        }
    }
    if now.saturating_duration_since(flow.last_activity) >= UDP_IDLE_TIMEOUT {
        ctx.close_flow(&key);
        bump(&mut ctx.counters.udp_idle_expired);
        return false;
    }
    true
}

/// A datagram from the host for the guest: from the flow's destination to the guest port.
fn send_reply(ctx: &mut Ctx<'_>, key: FlowKey, data: &[u8]) {
    let mut message = [0u8; UDP_HEADER_LEN + MAX_UDP_DATA];
    let len = UDP_HEADER_LEN + data.len();
    message[0..2].copy_from_slice(&key.dst_port.to_be_bytes());
    message[2..4].copy_from_slice(&key.guest_port.to_be_bytes());
    message[4..6].copy_from_slice(&(len as u16).to_be_bytes());
    message[UDP_HEADER_LEN..len].copy_from_slice(data);
    let mut checksum = l4_checksum(key.dst_ip, GUEST_IP, PROTO_UDP, &message[..len]);
    if checksum == 0 {
        checksum = 0xFFFF;
    }
    message[6..8].copy_from_slice(&checksum.to_be_bytes());
    ctx.send_ipv4(key.dst_ip, PROTO_UDP, &message[..len]);
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, UdpSocket};
    use std::time::{Duration, Instant};

    use od_machine::{NET_MAX_FRAME, NET_RX_QUEUE_LIMIT, NetBackend};

    use super::super::NatBackend;
    use super::super::flow::{FlowKey, FlowProto, MAX_UDP_FLOWS};
    use super::super::frame::{DNS_IP, GATEWAY_IP, GUEST_IP, PROTO_ICMP, PROTO_UDP};
    use super::super::test_frames::{
        GUEST_MAC, IP, Ip, ipv4_frame, pseudo, ref_checksum, udp_segment,
    };
    use super::super::test_io::{
        GuestPacket, drain, io_backend, parse_guest_packet, pump_at, pump_for, pump_quiet,
    };
    use super::*;

    const LOOP: Ipv4Addr = Ipv4Addr::LOCALHOST;

    fn guest_udp(src_port: u16, dst: Ipv4Addr, dst_port: u16, data: &[u8]) -> Vec<u8> {
        ipv4_frame(
            &Ip::to(dst, PROTO_UDP),
            &udp_segment(GUEST_IP, dst, src_port, dst_port, data),
        )
    }

    fn server() -> UdpSocket {
        let socket = UdpSocket::bind((LOOP, 0)).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        socket
    }

    fn port_of(socket: &UdpSocket) -> u16 {
        socket.local_addr().unwrap().port()
    }

    fn send(backend: &mut NatBackend, frame: &[u8], now: Instant) {
        backend.transmit(frame).unwrap();
        backend.process_pending_at(now);
    }

    /// A UDP packet for the guest: (source port, destination port, data), checksum verified.
    fn udp_of(packet: &GuestPacket) -> (u16, u16, Vec<u8>) {
        assert_eq!(packet.protocol, PROTO_UDP);
        let l4 = &packet.l4;
        assert_eq!(
            usize::from(u16::from_be_bytes([l4[4], l4[5]])),
            l4.len(),
            "UDP length"
        );
        assert_eq!(
            ref_checksum(&[&pseudo(packet.src, packet.dst, PROTO_UDP, l4.len()), l4]),
            0,
            "UDP checksum"
        );
        (
            u16::from_be_bytes([l4[0], l4[1]]),
            u16::from_be_bytes([l4[2], l4[3]]),
            l4[8..].to_vec(),
        )
    }

    /// An ICMP error for the guest: (type, code, quoted bytes), checksum verified.
    fn icmp_error_of(packet: &GuestPacket) -> (u8, u8, Vec<u8>) {
        assert_eq!(packet.protocol, PROTO_ICMP);
        assert_eq!(ref_checksum(&[&packet.l4]), 0, "ICMP checksum");
        assert_eq!(packet.l4[4..8], [0, 0, 0, 0], "unused field");
        (packet.l4[0], packet.l4[1], packet.l4[8..].to_vec())
    }

    #[test]
    fn datagram_goes_out_and_the_reply_comes_back_with_the_documented_fields() {
        let srv = server();
        let mut b = io_backend();
        let frame = guest_udp(40000, LOOP, port_of(&srv), b"hello");
        send(&mut b, &frame, Instant::now());
        let mut buf = [0u8; 2048];
        let (len, client) = srv.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..len], b"hello");
        srv.send_to(b"world", client).unwrap();
        let frames = pump_for(&mut b, 1);
        assert_eq!(frames.len(), 1);
        let packet = parse_guest_packet(&frames[0]);
        assert_eq!(packet.src, LOOP);
        assert_eq!(udp_of(&packet), (port_of(&srv), 40000, b"world".to_vec()));
        let c = b.counters();
        assert_eq!((c.udp_flows_opened, c.udp_forwarded), (1, 1));
        assert_eq!(b.flows.count(FlowProto::Udp), 1);
        // The second datagram of the flow uses the same socket: the server sees the same client.
        send(&mut b, &frame, Instant::now());
        let (_, again) = srv.recv_from(&mut buf).unwrap();
        assert_eq!(again, client);
        assert_eq!(b.counters().udp_flows_opened, 1);
        assert_eq!(b.counters().udp_forwarded, 2);
        // Another guest port is another flow with another host socket.
        send(
            &mut b,
            &guest_udp(40001, LOOP, port_of(&srv), b"x"),
            Instant::now(),
        );
        let (_, other) = srv.recv_from(&mut buf).unwrap();
        assert_ne!(other, client);
        assert_eq!(b.counters().udp_flows_opened, 2);
    }

    #[test]
    fn data_of_1472_bytes_goes_both_ways_and_1473_from_the_host_is_dropped() {
        let srv = server();
        let mut b = io_backend();
        let data = vec![0xA5u8; MAX_UDP_DATA];
        let frame = guest_udp(40000, LOOP, port_of(&srv), &data);
        assert_eq!(frame.len(), NET_MAX_FRAME);
        send(&mut b, &frame, Instant::now());
        let mut buf = [0u8; 4096];
        let (len, client) = srv.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..len], &data[..]);
        // 1472 bytes fill a frame of 1514.
        srv.send_to(&data, client).unwrap();
        let frames = pump_for(&mut b, 1);
        assert_eq!(frames[0].len(), NET_MAX_FRAME);
        assert_eq!(udp_of(&parse_guest_packet(&frames[0])).2, data);
        // One byte more cannot be a frame: dropped, counted, the flow stays.
        srv.send_to(&vec![1u8; MAX_UDP_DATA + 1], client).unwrap();
        assert!(pump_quiet(&mut b, Duration::from_millis(150)).is_empty());
        assert_eq!(b.counters().drop_oversize_reply, 1);
        assert_eq!(b.flows.count(FlowProto::Udp), 1);
        // A much larger datagram is dropped the same way and does not break the flow.
        srv.send_to(&vec![1u8; 9000], client).unwrap();
        assert!(pump_quiet(&mut b, Duration::from_millis(150)).is_empty());
        assert_eq!(b.counters().drop_oversize_reply, 2);
        srv.send_to(b"ok", client).unwrap();
        assert_eq!(pump_for(&mut b, 1).len(), 1);
    }

    #[test]
    fn flow_limit_l5_is_64_and_the_65th_flow_is_dropped_without_evicting_anyone() {
        let srv = server();
        let mut b = io_backend();
        let t0 = Instant::now();
        for n in 0..MAX_UDP_FLOWS as u16 {
            send(
                &mut b,
                &guest_udp(40000 + n, LOOP, port_of(&srv), &n.to_be_bytes()),
                t0,
            );
        }
        let c = b.counters();
        assert_eq!(
            (c.udp_flows_opened, c.udp_forwarded, c.drop_flow_limit_udp),
            (64, 64, 0)
        );
        // One above the limit.
        send(
            &mut b,
            &guest_udp(41000, LOOP, port_of(&srv), b"too many"),
            t0,
        );
        let c = b.counters();
        assert_eq!(
            (c.udp_flows_opened, c.udp_forwarded, c.drop_flow_limit_udp),
            (64, 64, 1)
        );
        assert_eq!(b.udp.flows.len(), 64);
        assert_eq!(b.flows.count(FlowProto::Udp), 64);
        // The server got the 64 datagrams and nothing from the 65th flow.
        let mut buf = [0u8; 64];
        srv.set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let mut seen = 0;
        while let Ok((len, _)) = srv.recv_from(&mut buf) {
            assert_eq!(len, 2, "no datagram of the refused flow");
            seen += 1;
        }
        assert_eq!(seen, 64);
        // An existing flow still works at the limit.
        send(&mut b, &guest_udp(40000, LOOP, port_of(&srv), b"again"), t0);
        assert_eq!(b.counters().udp_forwarded, 65);
        // When the flows have expired the slots are free again.
        b.service_at(t0 + UDP_IDLE_TIMEOUT);
        assert_eq!(b.udp.flows.len(), 0);
        send(
            &mut b,
            &guest_udp(41000, LOOP, port_of(&srv), b"new"),
            t0 + UDP_IDLE_TIMEOUT,
        );
        assert_eq!(b.counters().drop_flow_limit_udp, 1);
        assert_eq!(b.counters().udp_flows_opened, 65);
    }

    #[test]
    fn flow_idles_out_after_exactly_120_seconds_without_datagrams_in_either_direction() {
        assert_eq!(UDP_IDLE_TIMEOUT, Duration::from_secs(120));
        let first_server = server();
        let mut b = io_backend();
        let t0 = Instant::now();
        let at = |s: u64, ms: u64| t0 + Duration::from_millis(s * 1000 + ms);
        let frame = guest_udp(40000, LOOP, port_of(&first_server), b"x");
        send(&mut b, &frame, t0);
        b.service_at(at(119, 999));
        assert_eq!(
            b.udp.flows.len(),
            1,
            "alive one millisecond before the limit"
        );
        assert_eq!(b.counters().udp_idle_expired, 0);
        b.service_at(at(120, 0));
        assert_eq!(b.udp.flows.len(), 0);
        assert_eq!(b.flows.count(FlowProto::Udp), 0);
        assert_eq!(b.counters().udp_idle_expired, 1);

        // A datagram of the guest restarts the time.
        send(&mut b, &frame, at(200, 0));
        send(&mut b, &frame, at(300, 0));
        b.service_at(at(419, 999));
        assert_eq!(b.udp.flows.len(), 1);
        b.service_at(at(420, 0));
        assert_eq!(b.udp.flows.len(), 0);
        assert_eq!(b.counters().udp_idle_expired, 2);

        // A datagram of the host restarts it too.
        let srv = server();
        send(
            &mut b,
            &guest_udp(40000, LOOP, port_of(&srv), b"x"),
            at(500, 0),
        );
        let mut buf = [0u8; 64];
        let (_, client) = srv.recv_from(&mut buf).unwrap();
        srv.send_to(b"r", client).unwrap();
        assert_eq!(pump_at(&mut b, at(600, 0), 1).len(), 1);
        b.service_at(at(719, 999));
        assert_eq!(b.udp.flows.len(), 1);
        b.service_at(at(720, 0));
        assert_eq!(b.udp.flows.len(), 0);
        assert_eq!(b.counters().udp_idle_expired, 3);
    }

    #[test]
    fn closed_host_port_gives_an_icmp_port_unreachable_that_quotes_the_datagram() {
        let closed = port_of(&server());
        let mut b = io_backend();
        let frame = guest_udp(40000, LOOP, closed, b"knock");
        send(&mut b, &frame, Instant::now());
        let frames = pump_for(&mut b, 1);
        assert_eq!(frames.len(), 1, "the refusal reaches the guest");
        let packet = parse_guest_packet(&frames[0]);
        assert_eq!(
            packet.src, LOOP,
            "a port unreachable comes from the destination"
        );
        let (kind, code, quote) = icmp_error_of(&packet);
        assert_eq!((kind, code), (3, 3));
        assert_eq!(quote, frame[IP..IP + 28], "IP header and 8 bytes of data");
        let c = b.counters();
        assert_eq!(
            (c.udp_refused, c.icmp_error_sent, c.udp_forwarded),
            (1, 1, 1)
        );
        // The flow survives: the next datagram is refused again, and nothing else is counted.
        send(&mut b, &frame, Instant::now());
        assert_eq!(pump_for(&mut b, 1).len(), 1);
        assert_eq!(b.counters().udp_refused, 2);
        assert_eq!(b.counters().udp_flows_opened, 1);
    }

    #[test]
    fn udp_to_unserved_ports_of_the_gateway_and_dns_gets_icmp_port_unreachable() {
        let mut b = io_backend();
        for (n, (dst, port)) in [
            (GATEWAY_IP, 9u16),
            (DNS_IP, 5353),
            (GATEWAY_IP, 53),
            (DNS_IP, 67),
        ]
        .into_iter()
        .enumerate()
        {
            let frame = guest_udp(40000 + n as u16, dst, port, b"?");
            send(&mut b, &frame, Instant::now());
            let frames = drain(&mut b);
            assert_eq!(frames.len(), 1, "{dst}:{port}");
            let packet = parse_guest_packet(&frames[0]);
            assert_eq!(packet.src, dst);
            let (kind, code, quote) = icmp_error_of(&packet);
            assert_eq!((kind, code), (3, 3), "{dst}:{port}");
            assert_eq!(quote, frame[IP..IP + 28]);
        }
        let c = b.counters();
        assert_eq!((c.gw_port_unreachable, c.icmp_error_sent), (4, 4));
        assert_eq!((c.udp_flows_opened, c.drop_unhandled_l4), (0, 0));
        assert_eq!(b.flows.count(FlowProto::Udp), 0, "no flow, no socket");
    }

    #[test]
    fn dhcp_and_dns_datagrams_are_left_to_their_own_task() {
        let mut b = io_backend();
        for frame in [
            guest_udp(68, GATEWAY_IP, 67, &[0; 240]),
            guest_udp(40000, DNS_IP, 53, &[0; 20]),
        ] {
            send(&mut b, &frame, Instant::now());
        }
        let c = b.counters();
        assert_eq!(c.drop_unhandled_l4, 2);
        assert_eq!((c.gw_port_unreachable, c.udp_flows_opened), (0, 0));
        assert!(drain(&mut b).is_empty());
    }

    #[test]
    fn icmp_errors_are_limited_to_50_per_second() {
        let mut b = io_backend();
        let t0 = Instant::now();
        for n in 0..50u16 {
            send(&mut b, &guest_udp(40000 + n, GATEWAY_IP, 9, b"x"), t0);
        }
        assert_eq!(drain(&mut b).len(), 50);
        assert_eq!(b.counters().icmp_error_suppressed, 0);
        // The 51st within the same instant is suppressed: no frame, but the situation is counted.
        send(&mut b, &guest_udp(40100, GATEWAY_IP, 9, b"x"), t0);
        assert!(drain(&mut b).is_empty());
        let c = b.counters();
        assert_eq!(
            (
                c.gw_port_unreachable,
                c.icmp_error_sent,
                c.icmp_error_suppressed
            ),
            (51, 50, 1)
        );
        // A second later the bucket is full again.
        for n in 0..50u16 {
            send(
                &mut b,
                &guest_udp(40000 + n, GATEWAY_IP, 9, b"x"),
                t0 + Duration::from_secs(1),
            );
        }
        assert_eq!(drain(&mut b).len(), 50);
        assert_eq!(b.counters().icmp_error_suppressed, 1);
    }

    #[test]
    fn host_errors_map_to_the_rows_of_the_table() {
        use std::io::{Error, ErrorKind};
        let key = FlowKey {
            proto: FlowProto::Udp,
            guest_port: 40000,
            dst_ip: Ipv4Addr::new(93, 184, 216, 34),
            dst_port: 9,
        };
        let frame = guest_udp(40000, key.dst_ip, 9, b"x");
        let quote: [u8; ICMP_QUOTE_LEN] = frame[IP..IP + 28].try_into().unwrap();
        let cases = [
            (ErrorKind::ConnectionRefused, Some((3u8, key.dst_ip))),
            (ErrorKind::NetworkUnreachable, Some((0, GATEWAY_IP))),
            (ErrorKind::NetworkDown, Some((0, GATEWAY_IP))),
            (ErrorKind::HostUnreachable, Some((1, GATEWAY_IP))),
            (ErrorKind::PermissionDenied, None),
            (ErrorKind::OutOfMemory, None),
        ];
        for (kind, expected) in cases {
            let mut b = io_backend();
            {
                let (_handlers, mut ctx) = b.handler_parts(Duration::ZERO);
                report_host_error(&mut ctx, Instant::now(), &Error::from(kind), key, &quote);
            }
            let frames = drain(&mut b);
            let c = b.counters();
            match expected {
                Some((code, source)) => {
                    let packet = parse_guest_packet(&frames[0]);
                    assert_eq!(packet.src, source, "{kind:?}");
                    assert_eq!(
                        icmp_error_of(&packet),
                        (3, code, quote.to_vec()),
                        "{kind:?}"
                    );
                    assert_eq!(frames.len(), 1);
                    assert_eq!(c.udp_refused + c.udp_unreachable, 1, "{kind:?}");
                    assert_eq!(c.udp_refused, u64::from(code == 3), "{kind:?}");
                    assert_eq!(c.drop_host_resource, 0);
                }
                None => {
                    // Any other error of the host socket loses the datagram without an answer.
                    assert!(frames.is_empty(), "{kind:?}");
                    assert_eq!(c.drop_host_resource, 1, "{kind:?}");
                    assert_eq!(c.udp_refused + c.udp_unreachable + c.icmp_error_sent, 0);
                }
            }
        }
    }

    #[test]
    fn exhausted_host_descriptors_lose_the_datagram_and_free_the_flow_slot() {
        let mut b = io_backend().without_host_sockets();
        send(&mut b, &guest_udp(40000, LOOP, 9, b"x"), Instant::now());
        let c = b.counters();
        assert_eq!(
            (c.drop_host_resource, c.udp_flows_opened, c.udp_forwarded),
            (1, 0, 0)
        );
        assert_eq!(b.flows.count(FlowProto::Udp), 0);
        assert!(b.udp.flows.is_empty());
        assert!(drain(&mut b).is_empty());
    }

    #[test]
    fn datagram_from_another_sender_is_not_forwarded() {
        let srv = server();
        let mut b = io_backend();
        send(
            &mut b,
            &guest_udp(40000, LOOP, port_of(&srv), b"x"),
            Instant::now(),
        );
        let mut buf = [0u8; 16];
        let (_, client) = srv.recv_from(&mut buf).unwrap();
        // Same destination host, other port: the connected socket does not take it.
        let intruder = server();
        intruder.send_to(b"intruder", client).unwrap();
        assert!(pump_quiet(&mut b, Duration::from_millis(150)).is_empty());
        srv.send_to(b"friend", client).unwrap();
        let frames = pump_for(&mut b, 1);
        assert_eq!(udp_of(&parse_guest_packet(&frames[0])).2, b"friend");
    }

    #[test]
    fn reply_that_does_not_fit_the_l3_queue_is_lost_and_counted() {
        let srv = server();
        let mut b = io_backend();
        send(
            &mut b,
            &guest_udp(40000, LOOP, port_of(&srv), b"x"),
            Instant::now(),
        );
        let mut buf = [0u8; 16];
        let (_, client) = srv.recv_from(&mut buf).unwrap();
        for _ in 0..NET_RX_QUEUE_LIMIT {
            b.rx_queue.push(&[0u8; 60]).unwrap();
        }
        srv.send_to(b"late", client).unwrap();
        // The queue stays full, so the sockets cannot be pumped through drain: wait for the datagram instead.
        let started = Instant::now();
        while b.counters().drop_rx_queue_full == 0 && started.elapsed() < Duration::from_secs(2) {
            b.service_at(Instant::now());
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(b.counters().drop_rx_queue_full, 1);
        assert_eq!(drain(&mut b).len(), NET_RX_QUEUE_LIMIT);
    }

    #[test]
    fn reset_closes_the_sockets_and_forgets_the_flows() {
        let srv = server();
        let mut b = io_backend();
        send(
            &mut b,
            &guest_udp(40000, LOOP, port_of(&srv), b"x"),
            Instant::now(),
        );
        let mut buf = [0u8; 16];
        let (_, client) = srv.recv_from(&mut buf).unwrap();
        b.reset();
        assert!(b.udp.flows.is_empty());
        assert_eq!(b.flows.count(FlowProto::Udp), 0);
        assert_eq!(b.counters().resets, 1);
        // The old socket is closed: a late datagram of the server reaches nobody.
        srv.send_to(b"late", client).unwrap();
        assert!(pump_quiet(&mut b, Duration::from_millis(100)).is_empty());
    }

    #[test]
    fn frames_for_the_guest_carry_a_counting_identification() {
        let mut b = io_backend();
        for n in 0..3u16 {
            send(
                &mut b,
                &guest_udp(40000 + n, GATEWAY_IP, 9, b"x"),
                Instant::now(),
            );
        }
        let ids: Vec<u16> = drain(&mut b)
            .iter()
            .map(|f| parse_guest_packet(f).identification)
            .collect();
        assert_eq!(ids, [0, 1, 2]);
        b.shared.ip_id = 0xFFFF;
        send(
            &mut b,
            &guest_udp(40000, GATEWAY_IP, 9, b"x"),
            Instant::now(),
        );
        send(
            &mut b,
            &guest_udp(40001, GATEWAY_IP, 9, b"x"),
            Instant::now(),
        );
        let ids: Vec<u16> = drain(&mut b)
            .iter()
            .map(|f| parse_guest_packet(f).identification)
            .collect();
        assert_eq!(ids, [0xFFFF, 0], "wraps modulo 2^16");
        assert_eq!(GUEST_MAC[0] & 1, 0);
    }
}
