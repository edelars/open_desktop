//! Deterministic frame generators for the NAT tests (S11-31a).
//!
//! Checksums here come from an independent reference routine, not from the production one, so a test
//! of the parser is not circular. The generators are used only by the tests of this module and sit
//! here rather than in `od-testkit` (which holds CPU test generators and has no use for network frames);
//! they move there when a second crate needs them.

use std::net::Ipv4Addr;

use std::collections::HashMap;
use std::sync::Mutex;

use super::frame::{
    ETHERTYPE_ARP, ETHERTYPE_IPV4, GATEWAY_IP, GATEWAY_MAC, GUEST_IP, Mac, PROTO_ICMP, PROTO_TCP,
    PROTO_UDP,
};
use super::resolver::{ResolveOutcome, Resolver};

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

// ---- Replies of the backend, read back with the reference routines ------------------------------

/// A UDP datagram that the backend put into the queue for the guest, checked field by field.
#[derive(Debug)]
pub struct UdpReply {
    pub eth_dst: Mac,
    pub src: Ipv4Addr,
    pub dst: Ipv4Addr,
    pub src_port: u16,
    pub dst_port: u16,
    pub ident: u16,
    pub data: Vec<u8>,
}

/// Parses a frame for the guest as an IPv4 UDP datagram and checks everything NETWORK.md section 5
/// promises about frames the backend builds: sent from the gateway MAC, TTL 64, Don't Fragment, no
/// padding, valid header and UDP checksums (a UDP checksum is never 0 on the wire here).
pub fn udp_reply(frame: &[u8]) -> UdpReply {
    assert!(
        frame.len() >= 42 && frame.len() <= 1514,
        "length {}",
        frame.len()
    );
    assert_eq!(frame[6..12], GATEWAY_MAC, "source MAC");
    assert_eq!(frame[12..14], ETHERTYPE_IPV4.to_be_bytes());
    let ip = &frame[IP..IP + 20];
    assert_eq!(ip[0], 0x45);
    assert_eq!(
        usize::from(u16::from_be_bytes([ip[2], ip[3]])),
        frame.len() - IP
    );
    assert_eq!(ip[6..8], 0x4000u16.to_be_bytes(), "DF only");
    assert_eq!(ip[8], 64, "TTL");
    assert_eq!(ip[9], PROTO_UDP);
    assert_eq!(ref_checksum(&[ip]), 0, "IPv4 header checksum");
    let src = Ipv4Addr::new(ip[12], ip[13], ip[14], ip[15]);
    let dst = Ipv4Addr::new(ip[16], ip[17], ip[18], ip[19]);
    let udp = &frame[L4..];
    let udp_len = usize::from(u16::from_be_bytes([udp[4], udp[5]]));
    assert_eq!(udp_len, udp.len(), "UDP length");
    assert_ne!(udp[6..8], [0, 0], "UDP checksum present");
    assert_eq!(
        ref_checksum(&[&pseudo(src, dst, PROTO_UDP, udp.len()), udp]),
        0,
        "UDP checksum"
    );
    UdpReply {
        eth_dst: frame[0..6].try_into().unwrap(),
        src,
        dst,
        src_port: u16::from_be_bytes([udp[0], udp[1]]),
        dst_port: u16::from_be_bytes([udp[2], udp[3]]),
        ident: u16::from_be_bytes([ip[4], ip[5]]),
        data: udp[8..].to_vec(),
    }
}

// ---- DHCP ------------------------------------------------------------------------------------------

pub const DHCP_DISCOVER: u8 = 1;
pub const DHCP_REQUEST: u8 = 3;
pub const DHCP_DECLINE: u8 = 4;
pub const DHCP_RELEASE: u8 = 7;
pub const DHCP_INFORM: u8 = 8;

/// A DHCP message of a client under construction.
#[derive(Clone)]
pub struct DhcpMsg {
    pub op: u8,
    pub chaddr: Mac,
    pub ciaddr: Ipv4Addr,
    pub giaddr: Ipv4Addr,
    pub xid: u32,
    pub flags: u16,
    pub cookie: bool,
    pub options: Vec<u8>,
}

impl DhcpMsg {
    /// A client message of the given type (option 53 first).
    pub fn new(message_type: u8) -> Self {
        Self {
            op: 1,
            chaddr: GUEST_MAC,
            ciaddr: Ipv4Addr::UNSPECIFIED,
            giaddr: Ipv4Addr::UNSPECIFIED,
            xid: 0x1234_5678,
            flags: 0,
            cookie: true,
            options: vec![53, 1, message_type],
        }
    }

    pub fn opt(mut self, code: u8, value: &[u8]) -> Self {
        self.options.push(code);
        self.options.push(value.len() as u8);
        self.options.extend_from_slice(value);
        self
    }

    pub fn requested(self, ip: Ipv4Addr) -> Self {
        self.opt(50, &ip.octets())
    }

    pub fn server(self, ip: Ipv4Addr) -> Self {
        self.opt(54, &ip.octets())
    }

    pub fn ciaddr(mut self, ip: Ipv4Addr) -> Self {
        self.ciaddr = ip;
        self
    }

    pub fn build(&self) -> Vec<u8> {
        let mut m = vec![0u8; 236];
        m[0] = self.op;
        m[1] = 1;
        m[2] = 6;
        m[4..8].copy_from_slice(&self.xid.to_be_bytes());
        m[10..12].copy_from_slice(&self.flags.to_be_bytes());
        m[12..16].copy_from_slice(&self.ciaddr.octets());
        m[24..28].copy_from_slice(&self.giaddr.octets());
        m[28..34].copy_from_slice(&self.chaddr);
        if self.cookie {
            m.extend_from_slice(&[0x63, 0x82, 0x53, 0x63]);
        } else {
            m.extend_from_slice(&[0; 4]);
        }
        m.extend_from_slice(&self.options);
        m.push(255);
        m
    }
}

/// A DHCP client frame with explicit addresses and ports.
pub fn dhcp_client_frame(
    eth_dst: Mac,
    src: Ipv4Addr,
    dst: Ipv4Addr,
    src_port: u16,
    message: &[u8],
) -> Vec<u8> {
    let ip = Ip {
        src,
        ..Ip::to(dst, PROTO_UDP)
    };
    guest_frame(
        eth_dst,
        ETHERTYPE_IPV4,
        &ip_packet(&ip, &udp_segment(src, dst, src_port, 67, message)),
    )
}

/// The usual first message of a client without an address: 0.0.0.0:68 to the broadcast address.
pub fn dhcp_broadcast(message: &DhcpMsg) -> Vec<u8> {
    dhcp_client_frame(
        BROADCAST_MAC,
        Ipv4Addr::UNSPECIFIED,
        Ipv4Addr::BROADCAST,
        68,
        &message.build(),
    )
}

/// A message sent to the gateway address by a client that has an address.
pub fn dhcp_unicast(message: &DhcpMsg) -> Vec<u8> {
    dhcp_client_frame(GATEWAY_MAC, GUEST_IP, GATEWAY_IP, 68, &message.build())
}

/// Options of a server message as (code, value) pairs, in the order they were sent.
pub fn dhcp_options(message: &[u8]) -> Vec<(u8, Vec<u8>)> {
    let mut out = Vec::new();
    let mut at = 240;
    while message[at] != 255 {
        let (code, len) = (message[at], usize::from(message[at + 1]));
        out.push((code, message[at + 2..at + 2 + len].to_vec()));
        at += 2 + len;
    }
    out
}

// ---- DNS ------------------------------------------------------------------------------------------

pub const TYPE_A: u16 = 1;
pub const TYPE_AAAA: u16 = 28;

/// A name in wire format from a dotted string.
pub fn dns_name(name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for label in name.split('.').filter(|l| !l.is_empty()) {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    out
}

/// A DNS query under construction: recursion desired, one question, class IN, no additional records.
#[derive(Clone)]
pub struct DnsMsg {
    pub id: u16,
    pub flags: u16,
    pub qdcount: u16,
    pub question: Vec<u8>,
    pub arcount: u16,
    pub additional: Vec<u8>,
}

impl DnsMsg {
    pub fn query(id: u16, name: &str, qtype: u16) -> Self {
        Self::question_bytes(id, dns_name(name), qtype, 1)
    }

    pub fn question_bytes(id: u16, wire_name: Vec<u8>, qtype: u16, qclass: u16) -> Self {
        let mut question = wire_name;
        question.extend_from_slice(&qtype.to_be_bytes());
        question.extend_from_slice(&qclass.to_be_bytes());
        Self {
            id,
            flags: 0x0100,
            qdcount: 1,
            question,
            arcount: 0,
            additional: Vec::new(),
        }
    }

    /// Adds an OPT record: root name, type 41, UDP size 4096, extended RCODE 0, the version, flags 0.
    pub fn opt(mut self, version: u8, rdata: &[u8]) -> Self {
        self.additional
            .extend_from_slice(&[0, 0, 41, 0x10, 0x00, 0, version, 0, 0]);
        self.additional
            .extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        self.additional.extend_from_slice(rdata);
        self.arcount += 1;
        self
    }

    pub fn build(&self) -> Vec<u8> {
        let mut m = Vec::new();
        m.extend_from_slice(&self.id.to_be_bytes());
        m.extend_from_slice(&self.flags.to_be_bytes());
        m.extend_from_slice(&self.qdcount.to_be_bytes());
        m.extend_from_slice(&[0, 0, 0, 0]);
        m.extend_from_slice(&self.arcount.to_be_bytes());
        m.extend_from_slice(&self.question);
        m.extend_from_slice(&self.additional);
        m
    }
}

/// A frame carrying a DNS query to 10.0.2.3:53 from the given guest port.
pub fn dns_frame(src_port: u16, message: &[u8]) -> Vec<u8> {
    let dst = super::frame::DNS_IP;
    ipv4_frame(
        &Ip::to(dst, PROTO_UDP),
        &udp_segment(GUEST_IP, dst, src_port, 53, message),
    )
}

/// A DNS response read back by an independent routine.
#[derive(Debug)]
pub struct DnsReply {
    pub id: u16,
    pub flags: u16,
    pub qdcount: u16,
    pub question: Vec<u8>,
    /// (address, TTL) of every answer; each answer name must be the pointer to the question.
    pub answers: Vec<(Ipv4Addr, u32)>,
    /// The OPT record of the response, if any: (UDP size, extended RCODE, version, flags, data length).
    pub opt: Option<(u16, u8, u8, u16, u16)>,
}

impl DnsReply {
    pub fn rcode(&self) -> u8 {
        (self.flags & 0xF) as u8
    }
}

pub fn dns_reply(data: &[u8]) -> DnsReply {
    assert!(
        data.len() >= 12 && data.len() <= 512,
        "response length {}",
        data.len()
    );
    let u16_at = |at: usize| u16::from_be_bytes([data[at], data[at + 1]]);
    let (qdcount, ancount, nscount, arcount) = (u16_at(4), u16_at(6), u16_at(8), u16_at(10));
    let mut at = 12;
    let mut question = Vec::new();
    if qdcount == 1 {
        let start = at;
        while data[at] != 0 {
            at += 1 + usize::from(data[at]);
        }
        at += 1 + 4;
        question = data[start..at].to_vec();
    }
    let mut answers = Vec::new();
    for _ in 0..ancount {
        assert_eq!(data[at..at + 2], [0xC0, 0x0C], "answer name");
        assert_eq!(u16_at(at + 2), 1, "type A");
        assert_eq!(u16_at(at + 4), 1, "class IN");
        let ttl = u32::from_be_bytes([data[at + 6], data[at + 7], data[at + 8], data[at + 9]]);
        assert_eq!(u16_at(at + 10), 4, "rdlength");
        answers.push((
            Ipv4Addr::new(data[at + 12], data[at + 13], data[at + 14], data[at + 15]),
            ttl,
        ));
        at += 16;
    }
    assert_eq!(nscount, 0);
    let mut opt = None;
    for _ in 0..arcount {
        assert_eq!(data[at], 0, "OPT name is the root");
        assert_eq!(u16_at(at + 1), 41, "OPT type");
        opt = Some((
            u16_at(at + 3),
            data[at + 5],
            data[at + 6],
            u16_at(at + 7),
            u16_at(at + 9),
        ));
        at += 11 + usize::from(u16_at(at + 9));
    }
    assert_eq!(at, data.len(), "no trailing bytes");
    DnsReply {
        id: u16_at(0),
        flags: u16_at(2),
        qdcount,
        question,
        answers,
        opt,
    }
}

/// A resolver for tests that must not touch the network: every name is unknown.
pub struct NotFoundResolver;

impl Resolver for NotFoundResolver {
    fn resolve_a(&self, _name: &str) -> ResolveOutcome {
        ResolveOutcome::NotFound
    }
}

/// A resolver that answers from a table (an unknown name is NotFound).
pub struct TableResolver(pub Mutex<HashMap<String, ResolveOutcome>>);

impl TableResolver {
    pub fn with(entries: &[(&str, ResolveOutcome)]) -> Self {
        Self(Mutex::new(
            entries
                .iter()
                .map(|(n, o)| ((*n).to_owned(), o.clone()))
                .collect(),
        ))
    }
}

impl Resolver for TableResolver {
    fn resolve_a(&self, name: &str) -> ResolveOutcome {
        self.0
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .unwrap_or(ResolveOutcome::NotFound)
    }
}
