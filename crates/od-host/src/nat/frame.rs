//! Ethernet, ARP and IPv4 parsing and assembly for the NAT backend (S11-31a).
//!
//! [`classify`] applies the rules of NETWORK.md section 5 in the document's order to one frame
//! from the guest and either returns a borrowed description of it or names the reason it is dropped.
//! It reads nothing but the given slice, indexes only after a length check, and never panics for any
//! input. It touches no socket: every check that precedes an action on a socket lives here.

use std::net::Ipv4Addr;

use od_machine::{NET_MAX_FRAME, NET_MIN_FRAME};

pub type Mac = [u8; 6];

/// MAC of the gateway and of the DNS address (NETWORK.md section 3): locally administered, unicast.
pub const GATEWAY_MAC: Mac = [0x02, 0x4F, 0x44, 0x00, 0x00, 0x02];
pub const GUEST_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 15);
pub const GATEWAY_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 2);
pub const DNS_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 3);
pub const SUBNET_BROADCAST_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 255);
pub const LIMITED_BROADCAST_IP: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 255);

pub const ETHERTYPE_IPV4: u16 = 0x0800;
pub const ETHERTYPE_ARP: u16 = 0x0806;
pub const PROTO_ICMP: u8 = 1;
pub const PROTO_TCP: u8 = 6;
pub const PROTO_UDP: u8 = 17;

pub const ETH_HEADER_LEN: usize = 14;
pub const IPV4_HEADER_LEN: usize = 20;
const ARP_LEN: usize = 28;
const UDP_HEADER_LEN: usize = 8;
const TCP_MIN_HEADER_LEN: usize = 20;
const ICMP_HEADER_LEN: usize = 8;
/// Time to live of every packet the backend builds (NETWORK.md section 5).
pub const REPLY_TTL: u8 = 64;
const DHCP_SERVER_PORT: u16 = 67;
const DHCP_CLIENT_PORT: u16 = 68;

/// Why a frame is dropped; one reason per `drop_*` counter of NETWORK.md sections 5 and 6.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DropReason {
    Runt,
    Oversize,
    BadSrcMac,
    BadDstMac,
    UnsupportedEthertype,
    IpOptions,
    IpMalformed,
    IpFragment,
    BadSrcIp,
    PolicyDst,
    MulticastBroadcast,
    UnsupportedProto,
    L4Malformed,
    ArpOther,
}

/// Why a guest MAC address cannot be used.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuestMacError {
    /// The group bit is set (multicast or broadcast).
    Group,
    /// Equal to [`GATEWAY_MAC`].
    SameAsGateway,
}

/// Address policy of one backend: the guest MAC and the destination rules.
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    guest_mac: Mac,
    /// Test-only (NETWORK.md section 14, item 4): lets frames to 127.0.0.0/8 through so tests can
    /// reach a server on the loopback. It cannot be set outside `cfg(test)`: a product backend
    /// never gives the guest access to the host loopback.
    allow_loopback: bool,
}

impl Policy {
    pub fn new(guest_mac: Mac) -> Result<Self, GuestMacError> {
        if guest_mac[0] & 1 != 0 {
            return Err(GuestMacError::Group);
        }
        if guest_mac == GATEWAY_MAC {
            return Err(GuestMacError::SameAsGateway);
        }
        Ok(Self {
            guest_mac,
            allow_loopback: false,
        })
    }

    pub fn guest_mac(&self) -> Mac {
        self.guest_mac
    }

    #[cfg(test)]
    pub(crate) fn allows_loopback(&self) -> bool {
        self.allow_loopback
    }

    #[cfg(test)]
    pub(crate) fn allow_loopback_for_tests(mut self) -> Self {
        self.allow_loopback = true;
        self
    }
}

/// Where an accepted IPv4 packet is addressed, as far as the backend is concerned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DstClass {
    /// `10.0.2.2`, served by the backend.
    Gateway,
    /// `10.0.2.3`, served by the backend.
    Dns,
    /// `255.255.255.255` (only UDP port 67, DHCP, gets here).
    LimitedBroadcast,
    /// Any other unicast address that the policy allows.
    External,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UdpDatagram<'a> {
    pub src_port: u16,
    pub dst_port: u16,
    pub data: &'a [u8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TcpSegment<'a> {
    pub src_port: u16,
    pub dst_port: u16,
    pub seq: u32,
    pub ack: u32,
    /// The low eight flag bits (FIN, SYN, RST, PSH, ACK, URG, ECE, CWR).
    pub flags: u8,
    pub window: u16,
    /// Header bytes after the fixed 20 (possibly empty).
    pub options: &'a [u8],
    pub data: &'a [u8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IcmpMessage<'a> {
    pub icmp_type: u8,
    pub code: u8,
    /// The four bytes after the checksum (identifier and sequence number for echo).
    pub rest: [u8; 4],
    pub data: &'a [u8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum L4<'a> {
    Udp(UdpDatagram<'a>),
    Tcp(TcpSegment<'a>),
    Icmp(IcmpMessage<'a>),
}

/// An IPv4 packet that passed every check of NETWORK.md section 5.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ipv4Packet<'a> {
    pub src: Ipv4Addr,
    pub dst: Ipv4Addr,
    pub ttl: u8,
    pub protocol: u8,
    pub dst_class: DstClass,
    /// The 20-byte IP header (an ICMP error quotes it with the first 8 bytes of `payload`).
    pub header: &'a [u8],
    /// The IP payload: the transport header and data, without padding after the IP length.
    pub payload: &'a [u8],
    pub l4: L4<'a>,
}

/// What the backend has to do with a frame that was not dropped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Inbound<'a> {
    /// A valid request for the gateway or DNS address: answer with [`build_arp_reply`].
    ArpWhoHas {
        target_ip: Ipv4Addr,
        sender_ip: Ipv4Addr,
    },
    /// An address probe or an announcement (gratuitous ARP): normal, no reply.
    ArpIgnored,
    Ipv4(Ipv4Packet<'a>),
}

fn be16(bytes: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([bytes[at], bytes[at + 1]])
}

fn be32(bytes: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn ipv4_at(bytes: &[u8], at: usize) -> Ipv4Addr {
    Ipv4Addr::new(bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3])
}

fn mac_at(bytes: &[u8], at: usize) -> Mac {
    [
        bytes[at],
        bytes[at + 1],
        bytes[at + 2],
        bytes[at + 3],
        bytes[at + 4],
        bytes[at + 5],
    ]
}

/// Sum of the 16-bit big-endian words of `data` (an odd last byte is padded with zero) added to `acc`.
fn add_words(mut acc: u64, data: &[u8]) -> u64 {
    let mut pairs = data.chunks_exact(2);
    for pair in &mut pairs {
        acc += u64::from(u16::from_be_bytes([pair[0], pair[1]]));
    }
    if let [last] = pairs.remainder() {
        acc += u64::from(*last) << 8;
    }
    acc
}

fn fold(mut acc: u64) -> u16 {
    while acc > 0xFFFF {
        acc = (acc & 0xFFFF) + (acc >> 16);
    }
    acc as u16
}

/// RFC 1071 checksum of `data`. A buffer that already holds a correct checksum gives 0.
pub fn internet_checksum(data: &[u8]) -> u16 {
    !fold(add_words(0, data))
}

fn pseudo_header_sum(src: Ipv4Addr, dst: Ipv4Addr, protocol: u8, l4_len: usize) -> u64 {
    let mut acc = add_words(0, &src.octets());
    acc = add_words(acc, &dst.octets());
    acc += u64::from(protocol);
    acc + l4_len as u64
}

/// Checksum field value of a TCP segment or UDP datagram with the IPv4 pseudo-header (RFC 793,
/// RFC 768). The checksum field inside `l4` must be zero when computing. A UDP caller turns a
/// computed 0 into 0xFFFF (RFC 768). ICMP has no pseudo-header: use [`internet_checksum`] (RFC 792).
pub fn l4_checksum(src: Ipv4Addr, dst: Ipv4Addr, protocol: u8, l4: &[u8]) -> u16 {
    !fold(add_words(
        pseudo_header_sum(src, dst, protocol, l4.len()),
        l4,
    ))
}

fn l4_checksum_ok(src: Ipv4Addr, dst: Ipv4Addr, protocol: u8, l4: &[u8]) -> bool {
    fold(add_words(
        pseudo_header_sum(src, dst, protocol, l4.len()),
        l4,
    )) == 0xFFFF
}

fn is_multicast_or_reserved(ip: Ipv4Addr) -> bool {
    // 224.0.0.0/4 and 240.0.0.0/4 together (which includes 255.255.255.255).
    ip.octets()[0] >= 224
}

/// Destination ranges that never leave the backend (NETWORK.md section 5, item 6, point b).
fn policy_denied(dst: Ipv4Addr, policy: &Policy) -> bool {
    let o = dst.octets();
    (o[0] == 127 && !policy.allow_loopback)
        || o[0] == 0
        || (o[0] == 169 && o[1] == 254)
        || (o[..3] == [10, 0, 2] && !matches!(o[3], 2 | 3 | 255))
}

/// Checks one frame from the guest against NETWORK.md sections 5 and 6, in the order of the document.
pub fn classify<'a>(frame: &'a [u8], policy: &Policy) -> Result<Inbound<'a>, DropReason> {
    // Item 1: length.
    if frame.len() < NET_MIN_FRAME {
        return Err(DropReason::Runt);
    }
    if frame.len() > NET_MAX_FRAME {
        return Err(DropReason::Oversize);
    }
    // Item 2: MAC addresses.
    let dst_mac = mac_at(frame, 0);
    if mac_at(frame, 6) != policy.guest_mac {
        return Err(DropReason::BadSrcMac);
    }
    let dst_is_group = dst_mac[0] & 1 == 1;
    if dst_mac != GATEWAY_MAC && !dst_is_group {
        return Err(DropReason::BadDstMac);
    }
    // Item 3: EtherType.
    let payload = &frame[ETH_HEADER_LEN..];
    match be16(frame, 12) {
        ETHERTYPE_ARP => classify_arp(payload, policy),
        ETHERTYPE_IPV4 => classify_ipv4(payload, dst_is_group, policy),
        _ => Err(DropReason::UnsupportedEthertype),
    }
}

/// NETWORK.md section 6. Bytes after the 28-byte ARP packet (Ethernet padding) are ignored.
fn classify_arp<'a>(arp: &'a [u8], policy: &Policy) -> Result<Inbound<'a>, DropReason> {
    if arp.len() < ARP_LEN
        || be16(arp, 0) != 1
        || be16(arp, 2) != ETHERTYPE_IPV4
        || arp[4] != 6
        || arp[5] != 4
        || mac_at(arp, 8) != policy.guest_mac
    {
        return Err(DropReason::ArpOther);
    }
    let operation = be16(arp, 6);
    let sender_ip = ipv4_at(arp, 14);
    let target_ip = ipv4_at(arp, 24);
    // Probe (sender 0.0.0.0, target the guest address) and announcement (sender = target = the
    // guest address, as a request or a reply) are normal and need no answer.
    let probe = operation == 1 && sender_ip.is_unspecified() && target_ip == GUEST_IP;
    let announcement = sender_ip == GUEST_IP && target_ip == GUEST_IP && matches!(operation, 1 | 2);
    if probe || announcement {
        return Ok(Inbound::ArpIgnored);
    }
    let serves = target_ip == GATEWAY_IP || target_ip == DNS_IP;
    if operation == 1 && serves && sender_ip != target_ip {
        return Ok(Inbound::ArpWhoHas {
            target_ip,
            sender_ip,
        });
    }
    Err(DropReason::ArpOther)
}

/// NETWORK.md section 5, items 4 to 9. `ip` is everything after the Ethernet header.
fn classify_ipv4<'a>(
    ip: &'a [u8],
    dst_mac_is_group: bool,
    policy: &Policy,
) -> Result<Inbound<'a>, DropReason> {
    // Item 4: header.
    if ip.len() < IPV4_HEADER_LEN || ip[0] >> 4 != 4 {
        return Err(DropReason::IpMalformed);
    }
    let ihl = ip[0] & 0x0F;
    if ihl < 5 {
        return Err(DropReason::IpMalformed);
    }
    if ihl > 5 {
        return Err(DropReason::IpOptions);
    }
    let total_len = usize::from(be16(ip, 2));
    if total_len < IPV4_HEADER_LEN
        || total_len > ip.len()
        || internet_checksum(&ip[..IPV4_HEADER_LEN]) != 0
    {
        return Err(DropReason::IpMalformed);
    }
    // More Fragments (0x2000) or a non-zero offset (0x1FFF); Don't Fragment (0x4000) is fine.
    if be16(ip, 6) & 0x3FFF != 0 {
        return Err(DropReason::IpFragment);
    }
    let ttl = ip[8];
    let protocol = ip[9];
    let src = ipv4_at(ip, 12);
    let dst = ipv4_at(ip, 16);

    // Item 5: source address.
    let dhcp_without_address = protocol == PROTO_UDP
        && src.is_unspecified()
        && (dst == LIMITED_BROADCAST_IP || dst == GATEWAY_IP);
    if src != GUEST_IP && !dhcp_without_address {
        return Err(DropReason::BadSrcIp);
    }

    // Item 6: destination address, points a, b, c in order.
    let dst_is_group_ip = is_multicast_or_reserved(dst) || dst == SUBNET_BROADCAST_IP;
    if dst_mac_is_group && !dst_is_group_ip {
        return Err(DropReason::BadDstMac);
    }
    if policy_denied(dst, policy) {
        return Err(DropReason::PolicyDst);
    }
    if dst_is_group_ip && !(dst == LIMITED_BROADCAST_IP && protocol == PROTO_UDP) {
        return Err(DropReason::MulticastBroadcast);
    }

    // Item 7: protocol.
    if !matches!(protocol, PROTO_ICMP | PROTO_TCP | PROTO_UDP) {
        return Err(DropReason::UnsupportedProto);
    }

    // Item 8: transport layer.
    let payload = &ip[IPV4_HEADER_LEN..total_len];
    let l4 = parse_l4(protocol, src, dst, payload)?;
    if let L4::Udp(udp) = l4 {
        if src.is_unspecified()
            && (udp.src_port != DHCP_CLIENT_PORT || udp.dst_port != DHCP_SERVER_PORT)
        {
            return Err(DropReason::BadSrcIp);
        }
        if dst == LIMITED_BROADCAST_IP && src == GUEST_IP && udp.dst_port != DHCP_SERVER_PORT {
            return Err(DropReason::MulticastBroadcast);
        }
    }

    // Item 9: time to live.
    if ttl == 0 {
        return Err(DropReason::IpMalformed);
    }

    let dst_class = if dst == GATEWAY_IP {
        DstClass::Gateway
    } else if dst == DNS_IP {
        DstClass::Dns
    } else if dst == LIMITED_BROADCAST_IP {
        DstClass::LimitedBroadcast
    } else {
        DstClass::External
    };
    Ok(Inbound::Ipv4(Ipv4Packet {
        src,
        dst,
        ttl,
        protocol,
        dst_class,
        header: &ip[..IPV4_HEADER_LEN],
        payload,
        l4,
    }))
}

/// Lengths and checksums of the transport header (NETWORK.md section 5, item 8).
fn parse_l4<'a>(
    protocol: u8,
    src: Ipv4Addr,
    dst: Ipv4Addr,
    l4: &'a [u8],
) -> Result<L4<'a>, DropReason> {
    match protocol {
        PROTO_UDP => {
            if l4.len() < UDP_HEADER_LEN {
                return Err(DropReason::L4Malformed);
            }
            let udp_len = usize::from(be16(l4, 4));
            if udp_len < UDP_HEADER_LEN || udp_len > l4.len() {
                return Err(DropReason::L4Malformed);
            }
            // A zero checksum means "not computed" and is allowed for IPv4 (RFC 768).
            if be16(l4, 6) != 0 && !l4_checksum_ok(src, dst, PROTO_UDP, &l4[..udp_len]) {
                return Err(DropReason::L4Malformed);
            }
            Ok(L4::Udp(UdpDatagram {
                src_port: be16(l4, 0),
                dst_port: be16(l4, 2),
                data: &l4[UDP_HEADER_LEN..udp_len],
            }))
        }
        PROTO_TCP => {
            if l4.len() < TCP_MIN_HEADER_LEN {
                return Err(DropReason::L4Malformed);
            }
            let header_len = usize::from(l4[12] >> 4) * 4;
            if header_len < TCP_MIN_HEADER_LEN
                || header_len > l4.len()
                || !l4_checksum_ok(src, dst, PROTO_TCP, l4)
            {
                return Err(DropReason::L4Malformed);
            }
            Ok(L4::Tcp(TcpSegment {
                src_port: be16(l4, 0),
                dst_port: be16(l4, 2),
                seq: be32(l4, 4),
                ack: be32(l4, 8),
                flags: l4[13],
                window: be16(l4, 14),
                options: &l4[TCP_MIN_HEADER_LEN..header_len],
                data: &l4[header_len..],
            }))
        }
        _ => {
            if l4.len() < ICMP_HEADER_LEN || internet_checksum(l4) != 0 {
                return Err(DropReason::L4Malformed);
            }
            Ok(L4::Icmp(IcmpMessage {
                icmp_type: l4[0],
                code: l4[1],
                rest: [l4[4], l4[5], l4[6], l4[7]],
                data: &l4[ICMP_HEADER_LEN..],
            }))
        }
    }
}

fn write_ethernet_header(out: &mut [u8], dst: Mac, src: Mac, ethertype: u16) {
    out[0..6].copy_from_slice(&dst);
    out[6..12].copy_from_slice(&src);
    out[12..14].copy_from_slice(&ethertype.to_be_bytes());
}

/// Size of an ARP reply frame.
pub const ARP_REPLY_FRAME_LEN: usize = ETH_HEADER_LEN + ARP_LEN;

/// Writes the ARP reply of NETWORK.md section 6: `owner_ip` (the gateway or DNS address) is at
/// [`GATEWAY_MAC`]; the reply is addressed to the guest. Returns the frame length, or `None` when
/// `out` is shorter than [`ARP_REPLY_FRAME_LEN`].
pub fn build_arp_reply(
    guest_mac: Mac,
    owner_ip: Ipv4Addr,
    requester_ip: Ipv4Addr,
    out: &mut [u8],
) -> Option<usize> {
    let out = out.get_mut(..ARP_REPLY_FRAME_LEN)?;
    write_ethernet_header(out, guest_mac, GATEWAY_MAC, ETHERTYPE_ARP);
    let arp = &mut out[ETH_HEADER_LEN..];
    arp[0..2].copy_from_slice(&1u16.to_be_bytes());
    arp[2..4].copy_from_slice(&ETHERTYPE_IPV4.to_be_bytes());
    arp[4] = 6;
    arp[5] = 4;
    arp[6..8].copy_from_slice(&2u16.to_be_bytes());
    arp[8..14].copy_from_slice(&GATEWAY_MAC);
    arp[14..18].copy_from_slice(&owner_ip.octets());
    arp[18..24].copy_from_slice(&guest_mac);
    arp[24..28].copy_from_slice(&requester_ip.octets());
    Some(ARP_REPLY_FRAME_LEN)
}

/// Writes an Ethernet frame from the gateway MAC to `dst_mac` carrying one UDP datagram in a normal
/// IPv4 packet (see [`build_ipv4_frame`]); the UDP checksum is computed, a computed 0 is sent as 0xFFFF
/// (RFC 768). Returns the frame length, or `None` when the frame would exceed 1514 bytes or `out` is
/// too short.
#[allow(clippy::too_many_arguments)]
pub fn build_udp_ipv4_frame(
    dst_mac: Mac,
    src: Ipv4Addr,
    dst: Ipv4Addr,
    src_port: u16,
    dst_port: u16,
    identification: u16,
    data: &[u8],
    out: &mut [u8],
) -> Option<usize> {
    let udp_len = UDP_HEADER_LEN.checked_add(data.len())?;
    if ETH_HEADER_LEN + IPV4_HEADER_LEN + udp_len > NET_MAX_FRAME {
        return None;
    }
    let mut udp = [0u8; NET_MAX_FRAME];
    let udp = &mut udp[..udp_len];
    udp[0..2].copy_from_slice(&src_port.to_be_bytes());
    udp[2..4].copy_from_slice(&dst_port.to_be_bytes());
    // The length fits: the frame is at most 1514 bytes.
    udp[4..6].copy_from_slice(&(udp_len as u16).to_be_bytes());
    udp[UDP_HEADER_LEN..].copy_from_slice(data);
    let checksum = match l4_checksum(src, dst, PROTO_UDP, udp) {
        0 => 0xFFFF,
        sum => sum,
    };
    udp[6..8].copy_from_slice(&checksum.to_be_bytes());
    build_ipv4_frame(dst_mac, src, dst, PROTO_UDP, identification, udp, out)
}

/// Writes an Ethernet frame from the gateway MAC to `guest_mac` (the destination of the frame; the guest, or
/// the broadcast address for a DHCP NAK) carrying one IPv4 packet: TTL 64,
/// Don't Fragment, header checksum computed (NETWORK.md section 5, last paragraph). `l4` is the
/// finished transport message (its own checksum is the caller's: see [`l4_checksum`]). Returns the
/// frame length, or `None` when the frame would exceed 1514 bytes or `out` is too short.
pub fn build_ipv4_frame(
    guest_mac: Mac,
    src: Ipv4Addr,
    dst: Ipv4Addr,
    protocol: u8,
    identification: u16,
    l4: &[u8],
    out: &mut [u8],
) -> Option<usize> {
    let ip_len = IPV4_HEADER_LEN.checked_add(l4.len())?;
    let frame_len = ETH_HEADER_LEN + ip_len;
    if frame_len > NET_MAX_FRAME {
        return None;
    }
    let out = out.get_mut(..frame_len)?;
    write_ethernet_header(out, guest_mac, GATEWAY_MAC, ETHERTYPE_IPV4);
    let ip = &mut out[ETH_HEADER_LEN..];
    ip[0] = 0x45;
    ip[1] = 0;
    // The length fits: the frame is at most 1514 bytes.
    ip[2..4].copy_from_slice(&(ip_len as u16).to_be_bytes());
    ip[4..6].copy_from_slice(&identification.to_be_bytes());
    ip[6..8].copy_from_slice(&0x4000u16.to_be_bytes());
    ip[8] = REPLY_TTL;
    ip[9] = protocol;
    ip[10..12].copy_from_slice(&[0, 0]);
    ip[12..16].copy_from_slice(&src.octets());
    ip[16..20].copy_from_slice(&dst.octets());
    let checksum = internet_checksum(&ip[..IPV4_HEADER_LEN]);
    ip[10..12].copy_from_slice(&checksum.to_be_bytes());
    ip[IPV4_HEADER_LEN..].copy_from_slice(l4);
    Some(frame_len)
}
