//! Tests of the DHCP server (S11-31b): the table of NETWORK.md section 7, reply bytes, malformed
//! messages, the lease and the counters. Replies are read back with the reference routines of
//! `test_frames`, not with the production builders.

use std::net::Ipv4Addr;

use od_machine::{NET_RX_QUEUE_LIMIT, NetBackend};

use super::NatBackend;
use super::dhcp::{self, Outcome, REPLY_CAPACITY};
use super::frame::{DNS_IP, GATEWAY_IP, GATEWAY_MAC, GUEST_IP};
use super::test_frames::*;
use super::tests::{backend, drain, run, total_accounted};

const OFFER: u8 = 2;
const ACK: u8 = 5;
const NAK: u8 = 6;
const OTHER_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 99);

/// Sends one frame, expects exactly one reply and checks what every DHCP reply has in common.
fn exchange(b: &mut NatBackend, frame: &[u8]) -> UdpReply {
    run(b, frame);
    let mut frames = drain(b);
    assert_eq!(frames.len(), 1, "one reply");
    let reply = udp_reply(&frames.remove(0));
    assert_eq!((reply.src, reply.src_port), (GATEWAY_IP, 67));
    assert_eq!(reply.dst_port, 68);
    let m = &reply.data;
    assert_eq!(m[0], 2, "BOOTREPLY");
    assert_eq!(m[1..3], [1, 6]);
    assert_eq!(m[236..240], [0x63, 0x82, 0x53, 0x63], "magic cookie");
    assert_eq!(m[28..34], GUEST_MAC, "chaddr");
    assert_eq!(m[34..44], [0; 10], "chaddr padding");
    assert_eq!(m[44..236], [0; 192], "sname and file are empty");
    reply
}

fn message_type(reply: &UdpReply) -> u8 {
    let options = dhcp_options(&reply.data);
    assert_eq!(options[0].0, 53, "option 53 comes first");
    options[0].1[0]
}

fn yiaddr(reply: &UdpReply) -> Ipv4Addr {
    let m = &reply.data;
    Ipv4Addr::new(m[16], m[17], m[18], m[19])
}

fn expect_silence(b: &mut NatBackend, frame: &[u8]) {
    run(b, frame);
    assert!(drain(b).is_empty(), "the guest hears nothing");
}

fn lease_options(kind: u8) -> Vec<(u8, Vec<u8>)> {
    vec![
        (53, vec![kind]),
        (54, GATEWAY_IP.octets().to_vec()),
        (51, 86_400u32.to_be_bytes().to_vec()),
        (58, 43_200u32.to_be_bytes().to_vec()),
        (59, 75_600u32.to_be_bytes().to_vec()),
        (1, vec![255, 255, 255, 0]),
        (3, GATEWAY_IP.octets().to_vec()),
        (6, DNS_IP.octets().to_vec()),
        (28, vec![10, 0, 2, 255]),
        (26, 1500u16.to_be_bytes().to_vec()),
    ]
}

#[test]
fn discover_gets_an_offer_with_the_documented_bytes() {
    let mut b = backend();
    let mut discover = DhcpMsg::new(DHCP_DISCOVER);
    discover.flags = 0x8000;
    discover.giaddr = Ipv4Addr::new(10, 0, 2, 9);
    let reply = exchange(&mut b, &dhcp_broadcast(&discover));
    // Ethernet to the guest MAC; IPv4 to the limited broadcast address (the client has no address).
    assert_eq!(reply.eth_dst, GUEST_MAC);
    assert_eq!(reply.dst, Ipv4Addr::BROADCAST);
    let m = &reply.data;
    assert_eq!(m[4..8], 0x1234_5678u32.to_be_bytes(), "xid is copied");
    assert_eq!(m[8..10], [0, 0], "secs");
    assert_eq!(m[10..12], 0x8000u16.to_be_bytes(), "flags are copied");
    assert_eq!(m[12..16], [0; 4], "ciaddr");
    assert_eq!(yiaddr(&reply), GUEST_IP);
    assert_eq!(m[20..24], [0; 4], "siaddr");
    assert_eq!(m[24..28], [10, 0, 2, 9], "giaddr is copied");
    assert_eq!(dhcp_options(m), lease_options(OFFER));
    let c = b.counters();
    assert_eq!(c.dhcp_offers, 1);
    assert_eq!(total_accounted(&c), 1);
}

#[test]
fn the_full_sequence_gives_the_one_lease_and_renewals_keep_it() {
    let mut b = backend();
    // DISCOVER -> OFFER.
    let offer = exchange(&mut b, &dhcp_broadcast(&DhcpMsg::new(DHCP_DISCOVER)));
    assert_eq!((message_type(&offer), yiaddr(&offer)), (OFFER, GUEST_IP));
    // REQUEST (selecting: option 50 and the server identifier) -> ACK to the broadcast address.
    let selecting = DhcpMsg::new(DHCP_REQUEST)
        .requested(GUEST_IP)
        .server(GATEWAY_IP);
    let ack = exchange(&mut b, &dhcp_broadcast(&selecting));
    assert_eq!((message_type(&ack), yiaddr(&ack)), (ACK, GUEST_IP));
    assert_eq!(ack.dst, Ipv4Addr::BROADCAST);
    assert_eq!(ack.eth_dst, GUEST_MAC);
    assert_eq!(dhcp_options(&ack.data), lease_options(ACK));
    // INIT-REBOOT: option 50 and no server identifier.
    let reboot = DhcpMsg::new(DHCP_REQUEST).requested(GUEST_IP);
    let ack = exchange(&mut b, &dhcp_broadcast(&reboot));
    assert_eq!((message_type(&ack), yiaddr(&ack)), (ACK, GUEST_IP));
    // RENEWING: unicast from the guest address, the address in ciaddr -> ACK to ciaddr.
    let renew = DhcpMsg::new(DHCP_REQUEST).ciaddr(GUEST_IP);
    let ack = exchange(&mut b, &dhcp_unicast(&renew));
    assert_eq!((message_type(&ack), yiaddr(&ack)), (ACK, GUEST_IP));
    assert_eq!(ack.dst, GUEST_IP);
    assert_eq!(ack.eth_dst, GUEST_MAC);
    assert_eq!(ack.data[12..16], GUEST_IP.octets(), "ciaddr is echoed");
    // REBINDING: broadcast from the guest address, the address in ciaddr.
    let frame = dhcp_client_frame(
        BROADCAST_MAC,
        GUEST_IP,
        Ipv4Addr::BROADCAST,
        68,
        &renew.build(),
    );
    let ack = exchange(&mut b, &frame);
    assert_eq!((message_type(&ack), ack.dst), (ACK, GUEST_IP));
    // A client that still has no address may also talk to the gateway address.
    let frame = dhcp_client_frame(
        GATEWAY_MAC,
        Ipv4Addr::UNSPECIFIED,
        GATEWAY_IP,
        68,
        &selecting.build(),
    );
    let ack = exchange(&mut b, &frame);
    assert_eq!((message_type(&ack), ack.dst), (ACK, Ipv4Addr::BROADCAST));
    let c = b.counters();
    assert_eq!((c.dhcp_offers, c.dhcp_acks), (1, 5));
    assert_eq!(total_accounted(&c), 6);
}

#[test]
fn a_guest_with_a_static_address_gets_the_same_answers() {
    let mut b = backend();
    let reply = exchange(&mut b, &dhcp_unicast(&DhcpMsg::new(DHCP_DISCOVER)));
    assert_eq!(
        (message_type(&reply), reply.dst),
        (OFFER, Ipv4Addr::BROADCAST)
    );
}

#[test]
fn the_lease_survives_a_reset_and_a_new_backend() {
    let mut first = backend();
    let discover = dhcp_broadcast(&DhcpMsg::new(DHCP_DISCOVER));
    let before = exchange(&mut first, &discover).data;
    first.reset();
    let after_reset = exchange(&mut first, &discover).data;
    let mut second = backend();
    let fresh = exchange(&mut second, &discover).data;
    assert_eq!(before, after_reset);
    assert_eq!(before, fresh);
}

#[test]
fn a_request_for_another_address_gets_a_nak_on_the_broadcast_addresses() {
    let mut b = backend();
    let cases = [
        // Option 50 names another address.
        DhcpMsg::new(DHCP_REQUEST)
            .requested(OTHER_IP)
            .server(GATEWAY_IP),
        DhcpMsg::new(DHCP_REQUEST).requested(OTHER_IP),
        // No option 50: ciaddr names it (renewing another address).
        DhcpMsg::new(DHCP_REQUEST).ciaddr(OTHER_IP),
        // Option 50 wins over ciaddr.
        DhcpMsg::new(DHCP_REQUEST)
            .requested(OTHER_IP)
            .ciaddr(GUEST_IP),
    ];
    for request in cases {
        let nak = exchange(&mut b, &dhcp_broadcast(&request));
        assert_eq!(nak.eth_dst, BROADCAST_MAC, "NAK goes to ff:ff:ff:ff:ff:ff");
        assert_eq!(nak.dst, Ipv4Addr::BROADCAST);
        assert_eq!(yiaddr(&nak), Ipv4Addr::UNSPECIFIED);
        assert_eq!(nak.data[12..16], [0; 4], "ciaddr");
        assert_eq!(
            dhcp_options(&nak.data),
            vec![(53, vec![NAK]), (54, GATEWAY_IP.octets().to_vec())]
        );
    }
    let c = b.counters();
    assert_eq!(c.dhcp_naks, 4);
    assert_eq!(total_accounted(&c), 4);
}

#[test]
fn a_request_for_a_foreign_server_is_ignored_and_ours_by_name_is_served() {
    let mut b = backend();
    let foreign = DhcpMsg::new(DHCP_REQUEST)
        .requested(GUEST_IP)
        .server(OTHER_IP);
    expect_silence(&mut b, &dhcp_broadcast(&foreign));
    // Also when the address is wrong: the client chose another server, we do not NAK it.
    let foreign = DhcpMsg::new(DHCP_REQUEST)
        .requested(OTHER_IP)
        .server(OTHER_IP);
    expect_silence(&mut b, &dhcp_broadcast(&foreign));
    let c = b.counters();
    assert_eq!(c.dhcp_ignored, 2);
    assert_eq!(total_accounted(&c), 2);
    let ours = DhcpMsg::new(DHCP_REQUEST)
        .requested(GUEST_IP)
        .server(GATEWAY_IP);
    assert_eq!(message_type(&exchange(&mut b, &dhcp_broadcast(&ours))), ACK);
}

#[test]
fn decline_and_release_are_ignored_and_the_lease_does_not_change() {
    let mut b = backend();
    for kind in [DHCP_DECLINE, DHCP_RELEASE] {
        let message = DhcpMsg::new(kind)
            .requested(GUEST_IP)
            .server(GATEWAY_IP)
            .ciaddr(GUEST_IP);
        expect_silence(&mut b, &dhcp_unicast(&message));
    }
    let c = b.counters();
    assert_eq!(c.dhcp_ignored, 2);
    assert_eq!(total_accounted(&c), 2);
    let reply = exchange(&mut b, &dhcp_broadcast(&DhcpMsg::new(DHCP_DISCOVER)));
    assert_eq!((message_type(&reply), yiaddr(&reply)), (OFFER, GUEST_IP));
}

#[test]
fn inform_gets_an_ack_without_lease_options_only_for_the_guest_address() {
    let mut b = backend();
    let inform = DhcpMsg::new(DHCP_INFORM).ciaddr(GUEST_IP);
    let ack = exchange(&mut b, &dhcp_unicast(&inform));
    assert_eq!(ack.dst, GUEST_IP, "to ciaddr");
    assert_eq!(ack.eth_dst, GUEST_MAC);
    assert_eq!(yiaddr(&ack), Ipv4Addr::UNSPECIFIED);
    assert_eq!(ack.data[12..16], GUEST_IP.octets());
    let options = dhcp_options(&ack.data);
    let codes: Vec<u8> = options.iter().map(|(code, _)| *code).collect();
    assert_eq!(codes, [53, 54, 1, 3, 6, 28, 26], "no 51, 58, 59");
    assert_eq!(options[0].1, [ACK]);
    assert_eq!(b.counters().dhcp_acks, 1);
    // Another ciaddr, and none at all, are ignored.
    for other in [OTHER_IP, Ipv4Addr::UNSPECIFIED] {
        expect_silence(
            &mut b,
            &dhcp_unicast(&DhcpMsg::new(DHCP_INFORM).ciaddr(other)),
        );
    }
    let c = b.counters();
    assert_eq!((c.dhcp_acks, c.dhcp_ignored), (1, 2));
    assert_eq!(total_accounted(&c), 3);
}

/// Each case is one malformed message of the table of section 7, with the reason in its name.
fn malformed_cases() -> Vec<(&'static str, Vec<u8>)> {
    let discover = || DhcpMsg::new(DHCP_DISCOVER);
    let bytes = |m: &DhcpMsg| dhcp_broadcast(m);
    let mut cases: Vec<(&'static str, Vec<u8>)> = Vec::new();
    let mut other_mac = discover();
    other_mac.chaddr = [0x02, 0xAA, 0, 0, 0, 1];
    cases.push(("chaddr is not the guest MAC", bytes(&other_mac)));
    let mut bootp = discover();
    bootp.cookie = false;
    cases.push(("no magic cookie (BOOTP)", bytes(&bootp)));
    let mut reply_op = discover();
    reply_op.op = 2;
    cases.push(("BOOTREPLY from a client", bytes(&reply_op)));
    let mut no_type = discover();
    no_type.options = vec![50, 4, 10, 0, 2, 15];
    cases.push(("no message type", bytes(&no_type)));
    for kind in [0u8, OFFER, ACK, NAK, 9, 255] {
        let mut m = discover();
        m.options = vec![53, 1, kind];
        cases.push(("message type that a client does not send", bytes(&m)));
    }
    let mut long_type = discover();
    long_type.options = vec![53, 2, 1, 1];
    cases.push(("option 53 of two bytes", bytes(&long_type)));
    let mut short_request = DhcpMsg::new(DHCP_REQUEST);
    short_request.options.extend_from_slice(&[50, 3, 10, 0, 2]);
    cases.push(("option 50 of three bytes", bytes(&short_request)));
    let mut short_server = DhcpMsg::new(DHCP_REQUEST).requested(GUEST_IP);
    short_server
        .options
        .extend_from_slice(&[54, 5, 10, 0, 2, 2, 1]);
    cases.push(("option 54 of five bytes", bytes(&short_server)));
    let mut cut = DhcpMsg::new(DHCP_REQUEST);
    cut.options.extend_from_slice(&[50, 4, 10]);
    cases.push(("an option longer than the message", bytes(&cut)));
    let mut cut_length = DhcpMsg::new(DHCP_REQUEST);
    cut_length.options.push(99);
    let mut frame = bytes(&cut_length);
    // Make the message end right after the option code: no length byte (and no end marker).
    let ip_total = u16::from_be_bytes([frame[IP + 2], frame[IP + 3]]) - 1;
    frame.truncate(frame.len() - 1);
    frame[IP + 2..IP + 4].copy_from_slice(&ip_total.to_be_bytes());
    frame[L4 + 4..L4 + 6].copy_from_slice(&(ip_total - 20).to_be_bytes());
    fix_ip_checksum(&mut frame);
    fix_l4_checksum(&mut frame);
    cases.push(("an option code without a length", frame));
    let request_without_address = DhcpMsg::new(DHCP_REQUEST);
    cases.push((
        "a REQUEST that names no address",
        bytes(&request_without_address),
    ));
    let wrong_port = discover().build();
    cases.push((
        "client port is not 68",
        dhcp_client_frame(GATEWAY_MAC, GUEST_IP, GATEWAY_IP, 1234, &wrong_port),
    ));
    let mut htype = discover().build();
    htype[1] = 6;
    cases.push((
        "hardware type is not Ethernet",
        dhcp_client_frame(
            BROADCAST_MAC,
            Ipv4Addr::UNSPECIFIED,
            Ipv4Addr::BROADCAST,
            68,
            &htype,
        ),
    ));
    let mut hlen = discover().build();
    hlen[2] = 8;
    cases.push((
        "hardware address length is not 6",
        dhcp_client_frame(
            BROADCAST_MAC,
            Ipv4Addr::UNSPECIFIED,
            Ipv4Addr::BROADCAST,
            68,
            &hlen,
        ),
    ));
    let short = discover().build()[..239].to_vec();
    cases.push((
        "239 bytes: shorter than the fixed part and the cookie",
        dhcp_client_frame(
            BROADCAST_MAC,
            Ipv4Addr::UNSPECIFIED,
            Ipv4Addr::BROADCAST,
            68,
            &short,
        ),
    ));
    let bare = discover().build()[..240].to_vec();
    cases.push((
        "240 bytes: no options at all",
        dhcp_client_frame(
            BROADCAST_MAC,
            Ipv4Addr::UNSPECIFIED,
            Ipv4Addr::BROADCAST,
            68,
            &bare,
        ),
    ));
    cases
}

#[test]
fn every_malformed_message_is_counted_once_and_never_answered() {
    for (name, frame) in malformed_cases() {
        let mut b = backend();
        run(&mut b, &frame);
        let c = b.counters();
        assert_eq!(c.dhcp_malformed, 1, "{name}: {c:?}");
        assert_eq!(total_accounted(&c), 1, "{name}: exactly one counter");
        assert!(drain(&mut b).is_empty(), "{name}: silence");
    }
}

#[test]
fn message_size_limits_are_1472_and_240() {
    let mut out = [0u8; REPLY_CAPACITY];
    let mut padded = DhcpMsg::new(DHCP_DISCOVER).build();
    // Pad options (code 0) after the end marker never hurt; the sizes are what is tested.
    padded.resize(dhcp::MAX_MESSAGE, 0);
    assert!(matches!(
        dhcp::respond(GUEST_MAC, &padded, &mut out),
        Outcome::Reply { .. }
    ));
    padded.push(0);
    assert_eq!(
        dhcp::respond(GUEST_MAC, &padded, &mut out),
        Outcome::Malformed
    );
    let valid = DhcpMsg::new(DHCP_DISCOVER).build();
    assert_eq!(dhcp::MIN_MESSAGE, 240);
    assert_eq!(
        dhcp::respond(GUEST_MAC, &valid[..239], &mut out),
        Outcome::Malformed
    );
    // 243 bytes (no end marker) is the smallest message that carries a type.
    assert!(matches!(
        dhcp::respond(GUEST_MAC, &valid[..243], &mut out),
        Outcome::Reply { .. }
    ));
    // 1472 bytes also travels as a whole frame of 1514 bytes.
    let mut b = backend();
    let mut big = DhcpMsg::new(DHCP_DISCOVER).build();
    big.resize(1472, 0);
    let frame = dhcp_client_frame(
        BROADCAST_MAC,
        Ipv4Addr::UNSPECIFIED,
        Ipv4Addr::BROADCAST,
        68,
        &big,
    );
    assert_eq!(frame.len(), 1514);
    assert_eq!(message_type(&exchange(&mut b, &frame)), OFFER);
}

#[test]
fn constants_of_the_lease_are_the_documented_ones() {
    assert_eq!(dhcp::LEASE_SECS, 86_400);
    assert_eq!(dhcp::T1_SECS, 43_200);
    assert_eq!(dhcp::T2_SECS, 75_600);
    assert_eq!((dhcp::CLIENT_PORT, dhcp::SERVER_PORT), (68, 67));
}

#[test]
fn pad_options_and_unknown_options_are_skipped() {
    let mut b = backend();
    let mut message = DhcpMsg::new(DHCP_DISCOVER);
    message.options = vec![0, 0, 99, 3, 1, 2, 3, 53, 1, DHCP_DISCOVER, 0, 52, 1, 3];
    let reply = exchange(&mut b, &dhcp_broadcast(&message));
    assert_eq!(message_type(&reply), OFFER);
}

#[test]
fn a_reply_that_does_not_fit_the_l3_queue_is_a_queue_drop_not_an_offer() {
    let mut b = backend();
    for _ in 0..NET_RX_QUEUE_LIMIT {
        run(&mut b, &arp_request_for_gateway());
    }
    assert_eq!(b.counters().arp_replies, NET_RX_QUEUE_LIMIT as u64);
    run(&mut b, &dhcp_broadcast(&DhcpMsg::new(DHCP_DISCOVER)));
    let c = b.counters();
    assert_eq!((c.dhcp_offers, c.drop_rx_queue_full), (0, 1));
    assert_eq!(drain(&mut b).len(), NET_RX_QUEUE_LIMIT);
}

#[test]
fn the_ip_identification_counts_and_wraps_at_65536() {
    let mut b = backend();
    b.shared.ip_id = 0xFFFE;
    let discover = dhcp_broadcast(&DhcpMsg::new(DHCP_DISCOVER));
    let ids: Vec<u16> = (0..3).map(|_| exchange(&mut b, &discover).ident).collect();
    assert_eq!(ids, [0xFFFE, 0xFFFF, 0]);
}

#[test]
fn garbage_messages_never_panic_and_replies_are_well_formed() {
    let mut rng = Rng::new(0xD4C9_0001);
    let mut out = [0u8; REPLY_CAPACITY];
    let base = DhcpMsg::new(DHCP_REQUEST)
        .requested(GUEST_IP)
        .server(GATEWAY_IP)
        .build();
    for round in 0..40_000usize {
        let len = if round % 3 == 0 {
            rng.below(1600)
        } else {
            base.len() + rng.below(40)
        };
        let mut data = vec![0u8; len];
        let keep = base.len().min(len);
        data[..keep].copy_from_slice(&base[..keep]);
        for _ in 0..rng.below(6) {
            if len > 0 {
                let at = rng.below(len);
                data[at] = rng.next() as u8;
            }
        }
        if round % 5 == 0 && len > 0 {
            rng.fill(&mut data[len / 2..]);
        }
        if let Outcome::Reply {
            len, destination, ..
        } = dhcp::respond(GUEST_MAC, &data, &mut out)
        {
            assert!((240..=REPLY_CAPACITY).contains(&len));
            assert_eq!(out[0], 2);
            assert_eq!(out[236..240], [0x63, 0x82, 0x53, 0x63]);
            assert_eq!(out[len - 1], 255, "end marker");
            assert!(destination.ip == Ipv4Addr::BROADCAST || !destination.ip.is_unspecified());
            dhcp_options(&out[..len]);
        }
    }
}
