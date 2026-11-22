//! Deterministic frame generators for the NAT tests (S11-31a).
//!
//! Checksums here come from an independent reference routine, not from the production one, so a test
//! of the parser is not circular. The generators are used only by the tests of this module and sit
//! here rather than in `od-testkit` (which holds CPU test generators and has no use for network frames);
//! they move there when a second crate needs them.

use std::net::Ipv4Addr;

use super::frame::{
    ETHERTYPE_ARP, ETHERTYPE_IPV4, GATEWAY_IP, GATEWAY_MAC, GUEST_IP, Mac, PROTO_ICMP, PROTO_TCP,
    PROTO_UDP,
};

pub const GUEST_MAC: Mac = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];
pub const BROADCAST_MAC: Mac = [0xFF; 6];
pub const IP: usize = 14;
pub const L4: usize = 34;

/// RFC 1071 over the concatenation of `parts` (a deliberately plain implementation).
pub fn ref_checksum(parts: &[&[u8]]) -> u16 {
    let bytes: Vec<u8> = parts.iter().flat_map(|p| p.iter().copied()).collect();
    let mut sum: u32 = 0;
    let mut i = 0;
    while i < bytes.len() {
        let hi = u32::from(bytes[i]);
        let lo = if i + 1 < bytes.len() {
            u32::from(bytes[i + 1])
        } else {
            0
        };
        sum += (hi << 8) | lo;
        i += 2;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

pub fn pseudo(src: Ipv4Addr, dst: Ipv4Addr, protocol: u8, len: usize) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&src.octets());
    p.extend_from_slice(&dst.octets());
    p.extend_from_slice(&[0, protocol]);
    p.extend_from_slice(&(len as u16).to_be_bytes());
    p
}

pub fn udp_segment(
    src: Ipv4Addr,
    dst: Ipv4Addr,
    src_port: u16,
    dst_port: u16,
    data: &[u8],
) -> Vec<u8> {
    let len = 8 + data.len();
    let mut s = Vec::new();
    s.extend_from_slice(&src_port.to_be_bytes());
    s.extend_from_slice(&dst_port.to_be_bytes());
    s.extend_from_slice(&(len as u16).to_be_bytes());
    s.extend_from_slice(&[0, 0]);
    s.extend_from_slice(data);
    let mut c = ref_checksum(&[&pseudo(src, dst, PROTO_UDP, len), &s]);
    if c == 0 {
        c = 0xFFFF;
    }
    s[6..8].copy_from_slice(&c.to_be_bytes());
    s
}

#[allow(clippy::too_many_arguments)]
pub fn tcp_segment(
    src: Ipv4Addr,
    dst: Ipv4Addr,
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    options: &[u8],
    data: &[u8],
) -> Vec<u8> {
    assert_eq!(options.len() % 4, 0);
    let header_len = 20 + options.len();
    let mut s = Vec::new();
    s.extend_from_slice(&src_port.to_be_bytes());
    s.extend_from_slice(&dst_port.to_be_bytes());
    s.extend_from_slice(&seq.to_be_bytes());
    s.extend_from_slice(&ack.to_be_bytes());
    s.push(((header_len / 4) as u8) << 4);
    s.push(flags);
    s.extend_from_slice(&65535u16.to_be_bytes());
    s.extend_from_slice(&[0, 0, 0, 0]);
    s.extend_from_slice(options);
    s.extend_from_slice(data);
    let c = ref_checksum(&[&pseudo(src, dst, PROTO_TCP, s.len()), &s]);
    s[16..18].copy_from_slice(&c.to_be_bytes());
    s
}

pub fn icmp_message(icmp_type: u8, code: u8, ident: u16, seq: u16, data: &[u8]) -> Vec<u8> {
    let mut m = vec![icmp_type, code, 0, 0];
    m.extend_from_slice(&ident.to_be_bytes());
    m.extend_from_slice(&seq.to_be_bytes());
    m.extend_from_slice(data);
    let c = ref_checksum(&[&m]);
    m[2..4].copy_from_slice(&c.to_be_bytes());
    m
}

/// IPv4 header fields of a generated packet.
#[derive(Clone, Copy)]
pub struct Ip {
    pub src: Ipv4Addr,
    pub dst: Ipv4Addr,
    pub protocol: u8,
    pub ttl: u8,
    pub flags_frag: u16,
}

impl Ip {
    pub fn to(dst: Ipv4Addr, protocol: u8) -> Self {
        Self {
            src: GUEST_IP,
            dst,
            protocol,
            ttl: 64,
            flags_frag: 0x4000,
        }
    }
}

/// An IPv4 packet (header with a valid checksum, then `l4`).
pub fn ip_packet(ip: &Ip, l4: &[u8]) -> Vec<u8> {
    let mut p = vec![0x45, 0];
    p.extend_from_slice(&((20 + l4.len()) as u16).to_be_bytes());
    p.extend_from_slice(&[0, 1]);
    p.extend_from_slice(&ip.flags_frag.to_be_bytes());
    p.push(ip.ttl);
    p.push(ip.protocol);
    p.extend_from_slice(&[0, 0]);
    p.extend_from_slice(&ip.src.octets());
    p.extend_from_slice(&ip.dst.octets());
    let c = ref_checksum(&[&p]);
    p[10..12].copy_from_slice(&c.to_be_bytes());
    p.extend_from_slice(l4);
    p
}

/// Recomputes the IPv4 header checksum of a frame after a test patched the header.
pub fn fix_ip_checksum(frame: &mut [u8]) {
    frame[IP + 10] = 0;
    frame[IP + 11] = 0;
    let c = ref_checksum(&[&frame[IP..IP + 20]]);
    frame[IP + 10..IP + 12].copy_from_slice(&c.to_be_bytes());
}

/// Recomputes the transport checksum of an IPv4 frame in place, from the reference routine, when the
/// frame has a plain 20-byte IPv4 header and a TCP, UDP or ICMP payload long enough to hold the field.
/// The IP header checksum is not touched. Used by the fuzz tests so that mutations of the transport
/// header are not all stopped at the checksum.
pub fn fix_l4_checksum(frame: &mut [u8]) {
    if frame.len() < L4 || frame[12..14] != ETHERTYPE_IPV4.to_be_bytes() || frame[IP] != 0x45 {
        return;
    }
    let total = usize::from(u16::from_be_bytes([frame[IP + 2], frame[IP + 3]]));
    let end = (IP + total).min(frame.len());
    if end < L4 {
        return;
    }
    let protocol = frame[IP + 9];
    let src = Ipv4Addr::new(
        frame[IP + 12],
        frame[IP + 13],
        frame[IP + 14],
        frame[IP + 15],
    );
    let dst = Ipv4Addr::new(
        frame[IP + 16],
        frame[IP + 17],
        frame[IP + 18],
        frame[IP + 19],
    );
    let (field, end) = match protocol {
        PROTO_TCP => (16, end),
        PROTO_ICMP => (2, end),
        PROTO_UDP if end - L4 >= 8 => {
            // The UDP checksum covers the datagram up to its own length field.
            let udp_len = usize::from(u16::from_be_bytes([frame[L4 + 4], frame[L4 + 5]]));
            (
                6,
                if (8..=end - L4).contains(&udp_len) {
                    L4 + udp_len
                } else {
                    end
                },
            )
        }
        _ => return,
    };
    if end - L4 < field + 2 {
        return;
    }
    frame[L4 + field] = 0;
    frame[L4 + field + 1] = 0;
    let mut checksum = if protocol == PROTO_ICMP {
        ref_checksum(&[&frame[L4..end]])
    } else {
        let pseudo = pseudo(src, dst, protocol, end - L4);
        ref_checksum(&[&pseudo, &frame[L4..end]])
    };
    if protocol == PROTO_UDP && checksum == 0 {
        checksum = 0xFFFF;
    }
    frame[L4 + field..L4 + field + 2].copy_from_slice(&checksum.to_be_bytes());
}

/// An Ethernet frame from the guest.
pub fn guest_frame(eth_dst: Mac, ethertype: u16, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::new();
    f.extend_from_slice(&eth_dst);
    f.extend_from_slice(&GUEST_MAC);
    f.extend_from_slice(&ethertype.to_be_bytes());
    f.extend_from_slice(payload);
    f
}

/// A frame from the guest to the gateway MAC carrying `l4` in a normal IPv4 packet.
pub fn ipv4_frame(ip: &Ip, l4: &[u8]) -> Vec<u8> {
    guest_frame(GATEWAY_MAC, ETHERTYPE_IPV4, &ip_packet(ip, l4))
}

pub fn udp_frame(dst: Ipv4Addr, dst_port: u16, data: &[u8]) -> Vec<u8> {
    ipv4_frame(
        &Ip::to(dst, PROTO_UDP),
        &udp_segment(GUEST_IP, dst, 40000, dst_port, data),
    )
}

pub fn tcp_syn_frame(dst: Ipv4Addr, dst_port: u16) -> Vec<u8> {
    ipv4_frame(
        &Ip::to(dst, PROTO_TCP),
        &tcp_segment(
            GUEST_IP,
            dst,
            40001,
            dst_port,
            1000,
            0,
            0x02,
            &[2, 4, 0x05, 0xB4],
            &[],
        ),
    )
}

pub fn echo_frame(dst: Ipv4Addr) -> Vec<u8> {
    ipv4_frame(&Ip::to(dst, PROTO_ICMP), &icmp_message(8, 0, 7, 1, b"ping"))
}

/// A DHCP-client-like frame: 0.0.0.0:68 to 255.255.255.255:67 on the broadcast MAC.
pub fn dhcp_like_frame() -> Vec<u8> {
    let any = Ipv4Addr::UNSPECIFIED;
    let all = Ipv4Addr::BROADCAST;
    let ip = Ip {
        src: any,
        ..Ip::to(all, PROTO_UDP)
    };
    guest_frame(
        BROADCAST_MAC,
        ETHERTYPE_IPV4,
        &ip_packet(&ip, &udp_segment(any, all, 68, 67, &[0x55; 240])),
    )
}

pub fn arp_frame(
    eth_dst: Mac,
    operation: u16,
    sender_mac: Mac,
    sender_ip: Ipv4Addr,
    target_ip: Ipv4Addr,
) -> Vec<u8> {
    let mut a = Vec::new();
    a.extend_from_slice(&1u16.to_be_bytes());
    a.extend_from_slice(&ETHERTYPE_IPV4.to_be_bytes());
    a.extend_from_slice(&[6, 4]);
    a.extend_from_slice(&operation.to_be_bytes());
    a.extend_from_slice(&sender_mac);
    a.extend_from_slice(&sender_ip.octets());
    a.extend_from_slice(&[0; 6]);
    a.extend_from_slice(&target_ip.octets());
    guest_frame(eth_dst, ETHERTYPE_ARP, &a)
}

/// The usual ARP request of the guest for `target_ip`.
pub fn arp_request(target_ip: Ipv4Addr) -> Vec<u8> {
    arp_frame(BROADCAST_MAC, 1, GUEST_MAC, GUEST_IP, target_ip)
}

pub fn arp_request_for_gateway() -> Vec<u8> {
    arp_request(GATEWAY_IP)
}

/// xorshift64*: a fixed seed gives the same sequence everywhere.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    pub fn fill(&mut self, buf: &mut [u8]) {
        for byte in buf {
            *byte = self.next() as u8;
        }
    }
}
