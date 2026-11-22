//! DHCP server of the NAT backend (S11-31b, NETWORK.md section 7).
//!
//! One lease: the address is always [`GUEST_IP`] for the MAC of the guest, and the server keeps no state, so a
//! restart of the machine or of the backend gives the client the same address. [`respond`] takes the
//! data of one UDP datagram (already checked by [`super::frame::classify`]) and either writes the reply message
//! or says why there is none. It reads only the given slice, indexes only after a length check and never
//! panics. Wrapping the message into UDP/IPv4/Ethernet is the caller's job.
//!
//! Decisions for what the document leaves open (reported in the task result):
//! * a REQUEST that names no address at all (no option 50 and `ciaddr` zero) is malformed;
//! * a REQUEST with a foreign server identifier gets no reply and is counted as ignored (like
//!   DECLINE and RELEASE), so that every message is counted exactly once;
//! * a client message from a source port other than 68 is malformed;
//! * `giaddr` is copied into the reply and never used for addressing (the guest has no relay).

use std::net::Ipv4Addr;

use super::frame::{DNS_IP, GATEWAY_IP, GUEST_IP, Mac, SUBNET_BROADCAST_IP};

/// The client's port (the server answers to it) and the server's port.
pub const CLIENT_PORT: u16 = 68;
pub const SERVER_PORT: u16 = 67;
/// Smallest message from a client: 236 fixed bytes and the 4-byte magic cookie (L17).
pub const MIN_MESSAGE: usize = 240;
/// Largest message from a client: the data of a UDP datagram in a 1514-byte frame (L17).
pub const MAX_MESSAGE: usize = 1472;
/// Lease time, T1 and T2 in seconds (NETWORK.md section 7).
pub const LEASE_SECS: u32 = 86_400;
pub const T1_SECS: u32 = 43_200;
pub const T2_SECS: u32 = 75_600;
const MTU: u16 = 1500;
const SUBNET_MASK: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 0);

/// Longest reply: 240 fixed bytes, ten options and the end marker (302 bytes); the buffer has room to spare.
pub const REPLY_CAPACITY: usize = 320;

const COOKIE: [u8; 4] = [0x63, 0x82, 0x53, 0x63];
const OP_REQUEST: u8 = 1;
const OP_REPLY: u8 = 2;
const HTYPE_ETHERNET: u8 = 1;
const HLEN_ETHERNET: u8 = 6;

const MSG_DISCOVER: u8 = 1;
const MSG_OFFER: u8 = 2;
const MSG_REQUEST: u8 = 3;
const MSG_DECLINE: u8 = 4;
const MSG_ACK: u8 = 5;
const MSG_NAK: u8 = 6;
const MSG_RELEASE: u8 = 7;
const MSG_INFORM: u8 = 8;

const OPT_PAD: u8 = 0;
const OPT_SUBNET_MASK: u8 = 1;
const OPT_ROUTER: u8 = 3;
const OPT_DNS: u8 = 6;
const OPT_MTU: u8 = 26;
const OPT_BROADCAST: u8 = 28;
const OPT_REQUESTED_IP: u8 = 50;
const OPT_LEASE_TIME: u8 = 51;
const OPT_MESSAGE_TYPE: u8 = 53;
const OPT_SERVER_ID: u8 = 54;
const OPT_T1: u8 = 58;
const OPT_T2: u8 = 59;
const OPT_END: u8 = 255;

/// Which message the server sends.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplyKind {
    Offer,
    Ack,
    Nak,
}

/// Where the reply goes (NETWORK.md section 7).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReplyDestination {
    /// Layer 3 address: `255.255.255.255` or the `ciaddr` of the client.
    pub ip: Ipv4Addr,
    /// Layer 2: `ff:ff:ff:ff:ff:ff` (a NAK) instead of the MAC of the guest.
    pub broadcast_mac: bool,
}

/// What the server does with one client message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// A reply message of `len` bytes is in the buffer.
    Reply {
        kind: ReplyKind,
        len: usize,
        destination: ReplyDestination,
    },
    /// A valid message that needs no reply (DECLINE, RELEASE, a foreign server, a foreign INFORM).
    Ignored,
    /// A message the server cannot use: counted, never answered.
    Malformed,
}

struct Options {
    message_type: u8,
    requested_ip: Option<Ipv4Addr>,
    server_id: Option<Ipv4Addr>,
}

fn ipv4_at(bytes: &[u8], at: usize) -> Ipv4Addr {
    Ipv4Addr::new(bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3])
}

/// Walks the options after the magic cookie. `None` for a truncated option, a wrong length of an
/// option the server reads, or a missing message type.
fn parse_options(options: &[u8]) -> Option<Options> {
    let mut message_type = None;
    let mut requested_ip = None;
    let mut server_id = None;
    let mut at = 0;
    while at < options.len() {
        let code = options[at];
        at += 1;
        match code {
            OPT_PAD => continue,
            OPT_END => break,
            _ => {}
        }
        let len = usize::from(*options.get(at)?);
        at += 1;
        let value = options.get(at..at.checked_add(len)?)?;
        at += len;
        match code {
            OPT_MESSAGE_TYPE => {
                if value.len() != 1 {
                    return None;
                }
                message_type.get_or_insert(value[0]);
            }
            OPT_REQUESTED_IP => {
                if value.len() != 4 {
                    return None;
                }
                requested_ip.get_or_insert(ipv4_at(value, 0));
            }
            OPT_SERVER_ID => {
                if value.len() != 4 {
                    return None;
                }
                server_id.get_or_insert(ipv4_at(value, 0));
            }
            _ => {}
        }
    }
    Some(Options {
        message_type: message_type?,
        requested_ip,
        server_id,
    })
}

/// Writes the reply to `request` into `out` and says what it is, or says that there is none.
pub fn respond(guest_mac: Mac, request: &[u8], out: &mut [u8; REPLY_CAPACITY]) -> Outcome {
    if request.len() < MIN_MESSAGE || request.len() > MAX_MESSAGE {
        return Outcome::Malformed;
    }
    // BOOTP header: a request for Ethernet whose client hardware address is the guest MAC.
    if request[0] != OP_REQUEST
        || request[1] != HTYPE_ETHERNET
        || request[2] != HLEN_ETHERNET
        || request[28..34] != guest_mac
        || request[236..240] != COOKIE
    {
        return Outcome::Malformed;
    }
    let Some(options) = parse_options(&request[240..]) else {
        return Outcome::Malformed;
    };
    let ciaddr = ipv4_at(request, 12);

    let (kind, with_lease_options, yiaddr, reply_ciaddr) = match options.message_type {
        MSG_DISCOVER => (ReplyKind::Offer, true, GUEST_IP, Ipv4Addr::UNSPECIFIED),
        MSG_REQUEST => {
            // A foreign server identifier: the client chose another server.
            if options.server_id.is_some_and(|id| id != GATEWAY_IP) {
                return Outcome::Ignored;
            }
            // Selecting and init-reboot name the address in option 50; renewing and rebinding in ciaddr.
            let wanted = match options.requested_ip {
                Some(ip) => ip,
                None if !ciaddr.is_unspecified() => ciaddr,
                None => return Outcome::Malformed,
            };
            if wanted == GUEST_IP {
                (ReplyKind::Ack, true, GUEST_IP, ciaddr)
            } else {
                (
                    ReplyKind::Nak,
                    false,
                    Ipv4Addr::UNSPECIFIED,
                    Ipv4Addr::UNSPECIFIED,
                )
            }
        }
        MSG_INFORM => {
            if ciaddr != GUEST_IP {
                return Outcome::Ignored;
            }
            // RFC 2131, section 4.3.5: yiaddr zero, no lease options.
            (ReplyKind::Ack, false, Ipv4Addr::UNSPECIFIED, ciaddr)
        }
        MSG_DECLINE | MSG_RELEASE => return Outcome::Ignored,
        // A client never sends OFFER, ACK or NAK; anything else is not a DHCP message type.
        _ => return Outcome::Malformed,
    };

    out.fill(0);
    out[0] = OP_REPLY;
    out[1] = HTYPE_ETHERNET;
    out[2] = HLEN_ETHERNET;
    // xid, secs is zero, flags.
    out[4..8].copy_from_slice(&request[4..8]);
    out[10..12].copy_from_slice(&request[10..12]);
    out[12..16].copy_from_slice(&reply_ciaddr.octets());
    out[16..20].copy_from_slice(&yiaddr.octets());
    // siaddr stays zero; giaddr and chaddr are copied (RFC 2131, table 3).
    out[24..28].copy_from_slice(&request[24..28]);
    out[28..44].copy_from_slice(&request[28..44]);
    out[236..240].copy_from_slice(&COOKIE);

    let mut at = 240;
    let mut put = |code: u8, value: &[u8]| {
        out[at] = code;
        out[at + 1] = value.len() as u8;
        out[at + 2..at + 2 + value.len()].copy_from_slice(value);
        at += 2 + value.len();
    };
    let message_type = match kind {
        ReplyKind::Offer => MSG_OFFER,
        ReplyKind::Ack => MSG_ACK,
        ReplyKind::Nak => MSG_NAK,
    };
    put(OPT_MESSAGE_TYPE, &[message_type]);
    put(OPT_SERVER_ID, &GATEWAY_IP.octets());
    if with_lease_options {
        put(OPT_LEASE_TIME, &LEASE_SECS.to_be_bytes());
        put(OPT_T1, &T1_SECS.to_be_bytes());
        put(OPT_T2, &T2_SECS.to_be_bytes());
    }
    if kind != ReplyKind::Nak {
        put(OPT_SUBNET_MASK, &SUBNET_MASK.octets());
        put(OPT_ROUTER, &GATEWAY_IP.octets());
        put(OPT_DNS, &DNS_IP.octets());
        put(OPT_BROADCAST, &SUBNET_BROADCAST_IP.octets());
        put(OPT_MTU, &MTU.to_be_bytes());
    }
    out[at] = OPT_END;
    at += 1;

    let destination = if kind == ReplyKind::Nak {
        // giaddr is zero for a guest on the same link: always the limited broadcast (RFC 2131, section 4.1).
        ReplyDestination {
            ip: Ipv4Addr::BROADCAST,
            broadcast_mac: true,
        }
    } else {
        ReplyDestination {
            ip: if ciaddr.is_unspecified() {
                Ipv4Addr::BROADCAST
            } else {
                ciaddr
            },
            broadcast_mac: false,
        }
    };
    Outcome::Reply {
        kind,
        len: at,
        destination,
    }
}
