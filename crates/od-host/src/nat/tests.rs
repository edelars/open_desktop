//! Tests of the NAT backend core (S11-31a): the parsing rules of NETWORK.md sections 5 and 6, the
//! L2/L3 queues, the counters, and a deterministic fuzz of garbage input.

use std::net::Ipv4Addr;

use od_machine::{
    Dropped, NET_MAX_FRAME, NET_RX_QUEUE_LIMIT, NET_TX_QUEUE_LIMIT, NetBackend, NetCounters,
};

use super::flow::{
    FlowError, FlowKey, FlowProto, MAX_ICMP_ECHO_FLOWS, MAX_TCP_FLOWS, MAX_UDP_FLOWS,
};
use super::frame::*;
use super::test_frames::*;
use super::{NatBackend, record_drop};

const EXTERNAL: Ipv4Addr = Ipv4Addr::new(93, 184, 216, 34);

fn policy() -> Policy {
    Policy::new(GUEST_MAC).unwrap()
}

fn drop_of(frame: &[u8]) -> DropReason {
    match classify(frame, &policy()) {
        Err(reason) => reason,
        Ok(inbound) => panic!("expected a drop, got {inbound:?}"),
    }
}

fn accepted(frame: &[u8]) -> Ipv4Packet<'_> {
    match classify(frame, &policy()) {
        Ok(Inbound::Ipv4(packet)) => packet,
        other => panic!("expected an accepted IPv4 packet, got {other:?}"),
    }
}

fn patched_ip(mut frame: Vec<u8>, patch: impl FnOnce(&mut [u8])) -> Vec<u8> {
    patch(&mut frame[IP..IP + 20]);
    fix_ip_checksum(&mut frame);
    frame
}

// ---- Item 1: length --------------------------------------------------------------------------

#[test]
fn frames_shorter_than_an_ethernet_header_are_runts() {
    for len in 0..14 {
        assert_eq!(drop_of(&vec![0u8; len]), DropReason::Runt, "len {len}");
    }
}

#[test]
fn a_bare_ethernet_header_is_not_a_runt_but_has_no_payload() {
    let frame = guest_frame(GATEWAY_MAC, ETHERTYPE_IPV4, &[]);
    assert_eq!(frame.len(), 14);
    assert_eq!(drop_of(&frame), DropReason::IpMalformed);
}

#[test]
fn frame_of_1514_bytes_is_accepted_and_1515_is_oversize() {
    let max = udp_frame(EXTERNAL, 9, &[0xAB; 1472]);
    assert_eq!(max.len(), 1514);
    assert_eq!(
        accepted(&max).l4,
        L4::Udp(UdpDatagram {
            src_port: 40000,
            dst_port: 9,
            data: &[0xAB; 1472]
        })
    );
    let mut over = max.clone();
    over.push(0);
    assert_eq!(drop_of(&over), DropReason::Oversize);
}

#[test]
fn frames_below_60_bytes_and_trailing_padding_are_accepted() {
    // ARP request: 42 bytes, no padding.
    assert!(matches!(
        classify(&arp_request_for_gateway(), &policy()),
        Ok(Inbound::ArpWhoHas { .. })
    ));
    // The same padded to 60 bytes.
    let mut padded = arp_request_for_gateway();
    padded.resize(60, 0);
    assert!(matches!(
        classify(&padded, &policy()),
        Ok(Inbound::ArpWhoHas { .. })
    ));
    // A short UDP datagram padded after the IP length: the padding is not part of the payload.
    let mut udp = udp_frame(EXTERNAL, 9, b"x");
    udp.resize(60, 0xEE);
    let packet = accepted(&udp);
    assert_eq!(packet.payload.len(), 9);
}

// ---- Item 2: MAC addresses -------------------------------------------------------------------

#[test]
fn guest_mac_must_be_unicast_and_not_the_gateway() {
    assert_eq!(
        Policy::new([0x01, 0, 0, 0, 0, 1]).unwrap_err(),
        GuestMacError::Group
    );
    assert_eq!(Policy::new([0xFF; 6]).unwrap_err(), GuestMacError::Group);
    assert_eq!(
        Policy::new(GATEWAY_MAC).unwrap_err(),
        GuestMacError::SameAsGateway
    );
    assert!(NatBackend::new(GATEWAY_MAC).is_err());
    assert!(NatBackend::new([0x01, 2, 3, 4, 5, 6]).is_err());
    assert!(NatBackend::new(GUEST_MAC).is_ok());
}

#[test]
fn wrong_source_mac_is_dropped_first() {
    let mut frame = udp_frame(EXTERNAL, 9, b"x");
    frame[6] ^= 0x10;
    assert_eq!(drop_of(&frame), DropReason::BadSrcMac);
    // Even a frame with every other defect reports the source MAC first.
    let mut broken = vec![0u8; 60];
    broken[6..12].copy_from_slice(&[0x02, 0x99, 0, 0, 0, 1]);
    assert_eq!(drop_of(&broken), DropReason::BadSrcMac);
}

#[test]
fn unicast_destination_mac_other_than_the_gateway_is_dropped() {
    let mut frame = udp_frame(EXTERNAL, 9, b"x");
    frame[0..6].copy_from_slice(&[0x02, 0x4F, 0x44, 0, 0, 3]);
    assert_eq!(drop_of(&frame), DropReason::BadDstMac);
}

#[test]
fn group_destination_mac_with_unicast_ip_destination_is_bad_dst_mac() {
    let mut frame = udp_frame(EXTERNAL, 9, b"x");
    frame[0..6].copy_from_slice(&BROADCAST_MAC);
    assert_eq!(drop_of(&frame), DropReason::BadDstMac);
    frame[0..6].copy_from_slice(&[0x01, 0x00, 0x5E, 0, 0, 1]);
    assert_eq!(drop_of(&frame), DropReason::BadDstMac);
    frame[0..6].copy_from_slice(&[0x33, 0x33, 0, 0, 0, 1]);
    assert_eq!(drop_of(&frame), DropReason::BadDstMac);
}

// ---- Item 3: EtherType -----------------------------------------------------------------------

#[test]
fn ipv6_vlan_and_unknown_ethertypes_are_unsupported() {
    for ethertype in [0x86DDu16, 0x8100, 0x88A8, 0x0000, 0x0842, 0xFFFF] {
        let frame = guest_frame(GATEWAY_MAC, ethertype, &[0u8; 60]);
        assert_eq!(
            drop_of(&frame),
            DropReason::UnsupportedEthertype,
            "{ethertype:#06x}"
        );
    }
}

// ---- Item 4: IPv4 header ---------------------------------------------------------------------

#[test]
fn ipv4_header_defects() {
    let base = udp_frame(EXTERNAL, 9, b"data");
    // Version.
    assert_eq!(
        drop_of(&patched_ip(base.clone(), |ip| ip[0] = 0x65)),
        DropReason::IpMalformed
    );
    // IHL below 5 is malformed; above 5 means options.
    assert_eq!(
        drop_of(&patched_ip(base.clone(), |ip| ip[0] = 0x44)),
        DropReason::IpMalformed
    );
    assert_eq!(
        drop_of(&patched_ip(base.clone(), |ip| ip[0] = 0x46)),
        DropReason::IpOptions
    );
    assert_eq!(
        drop_of(&patched_ip(base.clone(), |ip| ip[0] = 0x4F)),
        DropReason::IpOptions
    );
    // Total length: 19 is too small, one more than the frame holds is too big.
    assert_eq!(
        drop_of(&patched_ip(base.clone(), |ip| ip[2..4]
            .copy_from_slice(&19u16.to_be_bytes()))),
        DropReason::IpMalformed
    );
    let real_len = (base.len() - 14) as u16;
    assert!(matches!(
        classify(
            &patched_ip(base.clone(), |ip| ip[2..4]
                .copy_from_slice(&real_len.to_be_bytes())),
            &policy()
        ),
        Ok(_)
    ));
    assert_eq!(
        drop_of(&patched_ip(base.clone(), |ip| ip[2..4]
            .copy_from_slice(&(real_len + 1).to_be_bytes()))),
        DropReason::IpMalformed
    );
    // Header checksum.
    let mut bad_sum = base.clone();
    bad_sum[IP + 10] ^= 0x01;
    assert_eq!(drop_of(&bad_sum), DropReason::IpMalformed);
    // Header shorter than 20 bytes.
    assert_eq!(
        drop_of(&guest_frame(
            GATEWAY_MAC,
            ETHERTYPE_IPV4,
            &base[IP..IP + 19]
        )),
        DropReason::IpMalformed
    );
}

#[test]
fn fragments_are_dropped_and_dont_fragment_is_fine() {
    let base = udp_frame(EXTERNAL, 9, b"data");
    let set = |frag: u16| {
        patched_ip(base.clone(), move |ip| {
            ip[6..8].copy_from_slice(&frag.to_be_bytes())
        })
    };
    assert_eq!(drop_of(&set(0x2000)), DropReason::IpFragment);
    assert_eq!(drop_of(&set(0x0001)), DropReason::IpFragment);
    assert_eq!(drop_of(&set(0x1FFF)), DropReason::IpFragment);
    assert_eq!(drop_of(&set(0x4001)), DropReason::IpFragment);
    assert!(classify(&set(0x4000), &policy()).is_ok());
    assert!(classify(&set(0x0000), &policy()).is_ok());
}

// ---- Item 5: source address ------------------------------------------------------------------

#[test]
fn only_the_guest_address_may_be_the_source() {
    let mut frame = ipv4_frame(
        &Ip {
            src: Ipv4Addr::new(10, 0, 2, 16),
            ..Ip::to(EXTERNAL, PROTO_UDP)
        },
        &udp_segment(Ipv4Addr::new(10, 0, 2, 16), EXTERNAL, 1, 2, b"x"),
    );
    assert_eq!(drop_of(&frame), DropReason::BadSrcIp);
    frame = ipv4_frame(
        &Ip {
            src: GATEWAY_IP,
            ..Ip::to(EXTERNAL, PROTO_UDP)
        },
        &udp_segment(GATEWAY_IP, EXTERNAL, 1, 2, b"x"),
    );
    assert_eq!(drop_of(&frame), DropReason::BadSrcIp);
}

#[test]
fn zero_source_is_allowed_only_for_dhcp_udp_to_broadcast_or_gateway() {
    let dhcp = dhcp_like_frame();
    let packet = match classify(&dhcp, &policy()) {
        Ok(Inbound::Ipv4(p)) => p,
        other => panic!("{other:?}"),
    };
    assert_eq!(packet.dst_class, DstClass::LimitedBroadcast);
    assert_eq!(packet.src, Ipv4Addr::UNSPECIFIED);

    let any = Ipv4Addr::UNSPECIFIED;
    let from_zero = |dst: Ipv4Addr, protocol: u8, l4: Vec<u8>| {
        ipv4_frame(
            &Ip {
                src: any,
                ..Ip::to(dst, protocol)
            },
            &l4,
        )
    };
    // To the gateway with ports 68 to 67: fine.
    let to_gw = from_zero(
        GATEWAY_IP,
        PROTO_UDP,
        udp_segment(any, GATEWAY_IP, 68, 67, &[0; 240]),
    );
    assert_eq!(accepted(&to_gw).dst_class, DstClass::Gateway);
    // Other destination, other protocol, other ports.
    let to_ext = from_zero(
        EXTERNAL,
        PROTO_UDP,
        udp_segment(any, EXTERNAL, 68, 67, b"x"),
    );
    assert_eq!(drop_of(&to_ext), DropReason::BadSrcIp);
    let tcp = from_zero(
        Ipv4Addr::BROADCAST,
        PROTO_TCP,
        tcp_segment(any, Ipv4Addr::BROADCAST, 1, 2, 0, 0, 2, &[], &[]),
    );
    assert_eq!(drop_of(&tcp), DropReason::BadSrcIp);
    for (sport, dport) in [(68u16, 68u16), (67, 67), (1000, 67), (68, 53)] {
        let frame = from_zero(
            GATEWAY_IP,
            PROTO_UDP,
            udp_segment(any, GATEWAY_IP, sport, dport, b"x"),
        );
        assert_eq!(drop_of(&frame), DropReason::BadSrcIp, "{sport}->{dport}");
    }
}

// ---- Item 6: destination address -------------------------------------------------------------

#[test]
fn host_loopback_and_other_forbidden_ranges_are_policy_drops() {
    for dst in [
        Ipv4Addr::new(127, 0, 0, 1),
        Ipv4Addr::new(127, 255, 255, 254),
        Ipv4Addr::new(0, 0, 0, 0),
        Ipv4Addr::new(0, 1, 2, 3),
        Ipv4Addr::new(169, 254, 1, 1),
        Ipv4Addr::new(10, 0, 2, 15),
        Ipv4Addr::new(10, 0, 2, 0),
        Ipv4Addr::new(10, 0, 2, 1),
        Ipv4Addr::new(10, 0, 2, 4),
        Ipv4Addr::new(10, 0, 2, 254),
    ] {
        assert_eq!(
            drop_of(&udp_frame(dst, 9, b"x")),
            DropReason::PolicyDst,
            "{dst}"
        );
    }
}

#[test]
fn loopback_is_reachable_only_with_the_test_policy_and_nothing_else_opens() {
    let loopback = udp_frame(Ipv4Addr::LOCALHOST, 9, b"x");
    let open = policy().allow_loopback_for_tests();
    assert_eq!(
        accepted_with(&loopback, &open).dst_class,
        DstClass::External
    );
    // The test policy opens 127.0.0.0/8 and nothing else.
    for dst in [
        Ipv4Addr::new(0, 1, 2, 3),
        Ipv4Addr::new(169, 254, 1, 1),
        Ipv4Addr::new(10, 0, 2, 15),
    ] {
        assert_eq!(
            classify(&udp_frame(dst, 9, b"x"), &open).unwrap_err(),
            DropReason::PolicyDst
        );
    }
}

fn accepted_with<'a>(frame: &'a [u8], policy: &Policy) -> Ipv4Packet<'a> {
    match classify(frame, policy) {
        Ok(Inbound::Ipv4(packet)) => packet,
        other => panic!("{other:?}"),
    }
}

#[test]
fn multicast_broadcast_and_reserved_destinations_are_dropped() {
    for dst in [
        Ipv4Addr::new(224, 0, 0, 251),
        Ipv4Addr::new(239, 255, 255, 250),
        Ipv4Addr::new(240, 0, 0, 1),
        Ipv4Addr::new(10, 0, 2, 255),
        Ipv4Addr::BROADCAST,
    ] {
        // The MAC is a group address for the group destinations, as a real guest would send.
        let mut frame = udp_frame(dst, 9, b"x");
        frame[0..6].copy_from_slice(&BROADCAST_MAC);
        assert_eq!(drop_of(&frame), DropReason::MulticastBroadcast, "{dst}");
    }
    // Unicast MAC with a multicast IP is the same drop.
    assert_eq!(
        drop_of(&udp_frame(Ipv4Addr::new(224, 0, 0, 251), 5353, b"x")),
        DropReason::MulticastBroadcast
    );
}

#[test]
fn igmp_and_tcp_to_multicast_are_multicast_drops_not_protocol_drops() {
    let igmp = ipv4_frame(
        &Ip::to(Ipv4Addr::new(224, 0, 0, 1), 2),
        &[0x11, 0, 0, 0, 0, 0, 0, 0],
    );
    assert_eq!(drop_of(&igmp), DropReason::MulticastBroadcast);
    let tcp = ipv4_frame(
        &Ip::to(Ipv4Addr::BROADCAST, PROTO_TCP),
        &tcp_segment(GUEST_IP, Ipv4Addr::BROADCAST, 1, 2, 0, 0, 2, &[], &[]),
    );
    assert_eq!(drop_of(&tcp), DropReason::MulticastBroadcast);
}

#[test]
fn limited_broadcast_udp_needs_dhcp_port_67() {
    let mut frame = udp_frame(Ipv4Addr::BROADCAST, 67, &[0; 240]);
    frame[0..6].copy_from_slice(&BROADCAST_MAC);
    assert_eq!(accepted(&frame).dst_class, DstClass::LimitedBroadcast);
    let mut other = udp_frame(Ipv4Addr::BROADCAST, 68, &[0; 240]);
    other[0..6].copy_from_slice(&BROADCAST_MAC);
    assert_eq!(drop_of(&other), DropReason::MulticastBroadcast);
}

#[test]
fn allowed_destinations_classify_by_who_serves_them() {
    assert_eq!(
        accepted(&udp_frame(GATEWAY_IP, 67, b"x")).dst_class,
        DstClass::Gateway
    );
    assert_eq!(
        accepted(&udp_frame(DNS_IP, 53, b"x")).dst_class,
        DstClass::Dns
    );
    for dst in [
        EXTERNAL,
        Ipv4Addr::new(192, 168, 1, 1),
        Ipv4Addr::new(10, 0, 3, 1),
        Ipv4Addr::new(10, 1, 2, 3),
        Ipv4Addr::new(172, 16, 0, 1),
    ] {
        assert_eq!(
            accepted(&udp_frame(dst, 9, b"x")).dst_class,
            DstClass::External,
            "{dst}"
        );
    }
}

// ---- Item 7: protocol ------------------------------------------------------------------------

#[test]
fn other_ip_protocols_are_unsupported_for_unicast_destinations() {
    for protocol in [2u8, 47, 50, 58, 89, 132, 255, 0] {
        let frame = ipv4_frame(&Ip::to(EXTERNAL, protocol), &[0u8; 16]);
        assert_eq!(
            drop_of(&frame),
            DropReason::UnsupportedProto,
            "protocol {protocol}"
        );
    }
}

// ---- Item 8: transport layer -----------------------------------------------------------------

#[test]
fn udp_length_and_checksum_rules() {
    let good = udp_segment(GUEST_IP, EXTERNAL, 1, 2, b"hello");
    let frame_with = |l4: &[u8]| ipv4_frame(&Ip::to(EXTERNAL, PROTO_UDP), l4);
    assert_eq!(
        accepted(&frame_with(&good)).l4,
        L4::Udp(UdpDatagram {
            src_port: 1,
            dst_port: 2,
            data: b"hello"
        })
    );
    // Shorter than a UDP header.
    assert_eq!(drop_of(&frame_with(&good[..7])), DropReason::L4Malformed);
    // Length field below 8 or above the data.
    for bad_len in [0u16, 7, 14] {
        let mut l4 = good.clone();
        l4[4..6].copy_from_slice(&bad_len.to_be_bytes());
        assert_eq!(
            drop_of(&frame_with(&l4)),
            DropReason::L4Malformed,
            "len {bad_len}"
        );
    }
    // Wrong checksum; zero checksum means "not computed" and is accepted.
    let mut wrong = good.clone();
    wrong[6] ^= 0x01;
    assert_eq!(drop_of(&frame_with(&wrong)), DropReason::L4Malformed);
    let mut zero = good.clone();
    zero[6..8].copy_from_slice(&[0, 0]);
    assert!(classify(&frame_with(&zero), &policy()).is_ok());
    // Data corrupted behind a valid checksum field.
    let mut corrupted = good.clone();
    corrupted[9] ^= 0x40;
    assert_eq!(drop_of(&frame_with(&corrupted)), DropReason::L4Malformed);
    // A UDP length shorter than the IP payload: the rest is ignored.
    let mut longer = good.clone();
    longer.extend_from_slice(&[9, 9, 9]);
    assert_eq!(
        accepted(&frame_with(&longer)).l4,
        L4::Udp(UdpDatagram {
            src_port: 1,
            dst_port: 2,
            data: b"hello"
        })
    );
}

#[test]
fn tcp_header_rules_and_fields() {
    let options = [2, 4, 0x05, 0xB4];
    let good = tcp_segment(
        GUEST_IP,
        EXTERNAL,
        5000,
        80,
        0x0102_0304,
        0x0A0B_0C0D,
        0x12,
        &options,
        b"GET",
    );
    let frame_with = |l4: &[u8]| ipv4_frame(&Ip::to(EXTERNAL, PROTO_TCP), l4);
    match accepted(&frame_with(&good)).l4 {
        L4::Tcp(t) => {
            assert_eq!(
                (t.src_port, t.dst_port, t.seq, t.ack, t.flags, t.window),
                (5000, 80, 0x0102_0304, 0x0A0B_0C0D, 0x12, 65535)
            );
            assert_eq!(t.options, &options);
            assert_eq!(t.data, b"GET");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(drop_of(&frame_with(&good[..19])), DropReason::L4Malformed);
    // Data offset 4 (below the minimum) and one that exceeds the segment.
    for offset in [0u8, 4, 15] {
        let mut l4 = good.clone();
        l4[12] = offset << 4;
        let fixed = refix_tcp(&l4);
        assert_eq!(
            drop_of(&frame_with(&fixed)),
            DropReason::L4Malformed,
            "offset {offset}"
        );
    }
    // Checksum is mandatory for TCP.
    let mut wrong = good.clone();
    wrong[16] ^= 0x80;
    assert_eq!(drop_of(&frame_with(&wrong)), DropReason::L4Malformed);
}

/// Recomputes the TCP checksum of a segment from the guest to EXTERNAL after a test patched it.
fn refix_tcp(segment: &[u8]) -> Vec<u8> {
    let mut s = segment.to_vec();
    s[16] = 0;
    s[17] = 0;
    let mut pseudo = Vec::new();
    pseudo.extend_from_slice(&GUEST_IP.octets());
    pseudo.extend_from_slice(&EXTERNAL.octets());
    pseudo.extend_from_slice(&[0, PROTO_TCP]);
    pseudo.extend_from_slice(&(s.len() as u16).to_be_bytes());
    let c = ref_checksum(&[&pseudo, &s]);
    s[16..18].copy_from_slice(&c.to_be_bytes());
    s
}

#[test]
fn icmp_length_and_checksum_rules() {
    let good = icmp_message(8, 0, 0x1234, 9, b"abcd");
    let frame_with = |l4: &[u8]| ipv4_frame(&Ip::to(EXTERNAL, PROTO_ICMP), l4);
    match accepted(&frame_with(&good)).l4 {
        L4::Icmp(m) => {
            assert_eq!(
                (m.icmp_type, m.code, m.rest, m.data),
                (8, 0, [0x12, 0x34, 0, 9], &b"abcd"[..])
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(drop_of(&frame_with(&good[..7])), DropReason::L4Malformed);
    let mut wrong = good.clone();
    wrong[2] ^= 0x01;
    assert_eq!(drop_of(&frame_with(&wrong)), DropReason::L4Malformed);
    // An odd-length message (checksum over a padded last byte).
    assert!(classify(&frame_with(&icmp_message(8, 0, 1, 1, b"abc")), &policy()).is_ok());
}

// ---- Item 9: TTL -----------------------------------------------------------------------------

#[test]
fn ttl_zero_is_dropped_and_ttl_one_is_accepted_without_a_reply() {
    let frame = |ttl: u8| {
        ipv4_frame(
            &Ip {
                ttl,
                ..Ip::to(EXTERNAL, PROTO_UDP)
            },
            &udp_segment(GUEST_IP, EXTERNAL, 1, 2, b"x"),
        )
    };
    assert_eq!(drop_of(&frame(0)), DropReason::IpMalformed);
    assert_eq!(accepted(&frame(1)).ttl, 1);
    assert_eq!(accepted(&frame(255)).ttl, 255);
}

#[test]
fn checks_run_in_the_order_of_the_document() {
    // Source address (item 5) comes before destination (item 6), protocol (7), L4 (8) and TTL (9).
    let ip = Ip {
        src: Ipv4Addr::new(1, 2, 3, 4),
        ttl: 0,
        ..Ip::to(Ipv4Addr::LOCALHOST, 47)
    };
    assert_eq!(drop_of(&ipv4_frame(&ip, &[0; 8])), DropReason::BadSrcIp);
    let ip = Ip {
        ttl: 0,
        ..Ip::to(Ipv4Addr::LOCALHOST, 47)
    };
    assert_eq!(drop_of(&ipv4_frame(&ip, &[0; 8])), DropReason::PolicyDst);
    let ip = Ip {
        ttl: 0,
        ..Ip::to(EXTERNAL, 47)
    };
    assert_eq!(
        drop_of(&ipv4_frame(&ip, &[0; 8])),
        DropReason::UnsupportedProto
    );
    let ip = Ip {
        ttl: 0,
        ..Ip::to(EXTERNAL, PROTO_UDP)
    };
    assert_eq!(drop_of(&ipv4_frame(&ip, &[0; 4])), DropReason::L4Malformed);
}

// ---- ARP (section 6) -------------------------------------------------------------------------

#[test]
fn arp_request_for_gateway_or_dns_is_answered() {
    for target in [GATEWAY_IP, DNS_IP] {
        assert_eq!(
            classify(&arp_request(target), &policy()),
            Ok(Inbound::ArpWhoHas {
                target_ip: target,
                sender_ip: GUEST_IP
            })
        );
    }
    // Also with the gateway MAC as the Ethernet destination.
    let unicast = arp_frame(GATEWAY_MAC, 1, GUEST_MAC, GUEST_IP, GATEWAY_IP);
    assert!(matches!(
        classify(&unicast, &policy()),
        Ok(Inbound::ArpWhoHas { .. })
    ));
}

#[test]
fn arp_probe_and_announcements_are_ignored_quietly() {
    let probe = arp_frame(BROADCAST_MAC, 1, GUEST_MAC, Ipv4Addr::UNSPECIFIED, GUEST_IP);
    assert_eq!(classify(&probe, &policy()), Ok(Inbound::ArpIgnored));
    let announce = arp_frame(BROADCAST_MAC, 1, GUEST_MAC, GUEST_IP, GUEST_IP);
    assert_eq!(classify(&announce, &policy()), Ok(Inbound::ArpIgnored));
    let announce_reply = arp_frame(BROADCAST_MAC, 2, GUEST_MAC, GUEST_IP, GUEST_IP);
    assert_eq!(
        classify(&announce_reply, &policy()),
        Ok(Inbound::ArpIgnored)
    );
}

#[test]
fn other_arp_is_dropped_as_arp_other() {
    // A request for an address the backend does not own.
    assert_eq!(
        drop_of(&arp_request(Ipv4Addr::new(10, 0, 2, 77))),
        DropReason::ArpOther
    );
    assert_eq!(drop_of(&arp_request(EXTERNAL)), DropReason::ArpOther);
    // A reply from the guest (not an announcement).
    assert_eq!(
        drop_of(&arp_frame(
            BROADCAST_MAC,
            2,
            GUEST_MAC,
            GUEST_IP,
            GATEWAY_IP
        )),
        DropReason::ArpOther
    );
    // Unknown operation, sender MAC that is not the guest's, sender = target = the gateway.
    assert_eq!(
        drop_of(&arp_frame(
            BROADCAST_MAC,
            3,
            GUEST_MAC,
            GUEST_IP,
            GATEWAY_IP
        )),
        DropReason::ArpOther
    );
    assert_eq!(
        drop_of(&arp_frame(
            BROADCAST_MAC,
            1,
            [2, 9, 9, 9, 9, 9],
            GUEST_IP,
            GATEWAY_IP
        )),
        DropReason::ArpOther
    );
    assert_eq!(
        drop_of(&arp_frame(
            BROADCAST_MAC,
            1,
            GUEST_MAC,
            GATEWAY_IP,
            GATEWAY_IP
        )),
        DropReason::ArpOther
    );
}

#[test]
fn malformed_arp_fields_are_dropped() {
    let good = arp_request_for_gateway();
    for (at, value) in [(14 + 1, 6u8), (14 + 3, 0x01), (14 + 4, 8), (14 + 5, 6)] {
        let mut frame = good.clone();
        frame[at] = value;
        assert_eq!(drop_of(&frame), DropReason::ArpOther, "byte {at}");
    }
    for len in 14..42 {
        assert_eq!(drop_of(&good[..len]), DropReason::ArpOther, "len {len}");
    }
}

// ---- The backend: queues, counters, replies --------------------------------------------------

/// A product backend whose host sockets cannot be created: these tests feed frames that would
/// otherwise reach real hosts (the tests with sockets use `test_io::io_backend`).
fn backend() -> NatBackend {
    NatBackend::new(GUEST_MAC).unwrap().without_host_sockets()
}

fn run(backend: &mut NatBackend, frame: &[u8]) {
    backend.transmit(frame).unwrap();
    backend.process_pending();
}

fn drain(backend: &mut NatBackend) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = [0u8; NET_MAX_FRAME];
    while let Some(len) = backend.poll_receive(&mut buf) {
        out.push(buf[..len].to_vec());
    }
    out
}

#[test]
fn arp_reply_for_the_gateway_has_the_documented_bytes() {
    let mut b = backend();
    run(&mut b, &arp_request_for_gateway());
    let replies = drain(&mut b);
    assert_eq!(replies.len(), 1);
    let mut expected = Vec::new();
    expected.extend_from_slice(&GUEST_MAC); // Ethernet destination
    expected.extend_from_slice(&GATEWAY_MAC); // Ethernet source
    expected.extend_from_slice(&[0x08, 0x06]);
    expected.extend_from_slice(&[0, 1, 8, 0, 6, 4, 0, 2]);
    expected.extend_from_slice(&GATEWAY_MAC);
    expected.extend_from_slice(&[10, 0, 2, 2]);
    expected.extend_from_slice(&GUEST_MAC);
    expected.extend_from_slice(&[10, 0, 2, 15]);
    assert_eq!(replies[0], expected);
    let c = b.counters();
    assert_eq!(
        (c.arp_replies, c.tx_frames, c.rx_frames, c.rx_bytes),
        (1, 1, 1, 42)
    );
}

#[test]
fn arp_reply_for_dns_names_the_dns_address_with_the_same_mac() {
    let mut b = backend();
    run(&mut b, &arp_request(DNS_IP));
    let replies = drain(&mut b);
    assert_eq!(&replies[0][22..28], &GATEWAY_MAC);
    assert_eq!(&replies[0][28..32], &[10, 0, 2, 3]);
}

#[test]
fn arp_probe_gets_no_reply_and_is_counted_as_ignored() {
    let mut b = backend();
    run(
        &mut b,
        &arp_frame(BROADCAST_MAC, 1, GUEST_MAC, Ipv4Addr::UNSPECIFIED, GUEST_IP),
    );
    assert!(drain(&mut b).is_empty());
    let c = b.counters();
    assert_eq!(
        (c.arp_probe_ignored, c.drop_arp_other, c.arp_replies),
        (1, 0, 0)
    );
}

#[test]
fn transmit_rejects_runts_and_oversize_frames_at_once() {
    let mut b = backend();
    assert_eq!(b.transmit(&[0u8; 13]), Err(Dropped::Runt));
    assert_eq!(b.transmit(&[]), Err(Dropped::Runt));
    assert_eq!(
        b.transmit(&vec![0u8; NET_MAX_FRAME + 1]),
        Err(Dropped::Oversize)
    );
    assert_eq!(b.transmit(&vec![0u8; 14]), Ok(()));
    assert_eq!(b.transmit(&vec![0u8; NET_MAX_FRAME]), Ok(()));
    let c = b.counters();
    assert_eq!((c.drop_runt, c.drop_oversize, c.tx_frames), (2, 1, 2));
    assert_eq!(c.tx_bytes, (14 + NET_MAX_FRAME) as u64);
}

#[test]
fn l2_queue_holds_exactly_256_frames() {
    assert_eq!(NET_TX_QUEUE_LIMIT, 256);
    let mut b = backend();
    for i in 0..NET_TX_QUEUE_LIMIT {
        assert_eq!(b.transmit(&arp_request_for_gateway()), Ok(()), "frame {i}");
    }
    assert_eq!(
        b.transmit(&arp_request_for_gateway()),
        Err(Dropped::QueueFull)
    );
    let c = b.counters();
    assert_eq!((c.tx_frames, c.drop_tx_queue_full), (256, 1));
    // Processing frees the queue; the next frame is accepted again.
    assert_eq!(b.process_pending(), 256);
    assert_eq!(b.transmit(&arp_request_for_gateway()), Ok(()));
}

#[test]
fn l3_queue_holds_256_frames_and_drops_the_tail() {
    assert_eq!(NET_RX_QUEUE_LIMIT, 256);
    let mut b = backend();
    for _ in 0..NET_RX_QUEUE_LIMIT {
        b.transmit(&arp_request_for_gateway()).unwrap();
    }
    b.process_pending();
    assert_eq!(b.counters().arp_replies, 256);
    assert_eq!(b.counters().drop_rx_queue_full, 0);
    // One more reply does not fit: dropped at the tail, counted, not counted as a reply.
    run(&mut b, &arp_request(DNS_IP));
    let c = b.counters();
    assert_eq!((c.drop_rx_queue_full, c.arp_replies), (1, 256));
    // The 256 kept frames are the first ones (gateway), the dropped one was the newest (dns).
    let replies = drain(&mut b);
    assert_eq!(replies.len(), 256);
    assert!(replies.iter().all(|r| r[28..32] == [10, 0, 2, 2]));
    // After draining the queue is usable again.
    run(&mut b, &arp_request(DNS_IP));
    assert_eq!(drain(&mut b).len(), 1);
}

#[test]
fn poll_receive_with_a_short_buffer_keeps_the_frame() {
    let mut b = backend();
    run(&mut b, &arp_request_for_gateway());
    let mut short = [0u8; NET_MAX_FRAME - 1];
    assert_eq!(b.poll_receive(&mut short), None);
    assert_eq!(b.poll_receive(&mut []), None);
    assert_eq!(b.counters().rx_buf_too_small, 2);
    assert_eq!(b.counters().rx_frames, 0);
    let mut exact = [0u8; NET_MAX_FRAME];
    assert_eq!(b.poll_receive(&mut exact), Some(42));
    assert_eq!(b.poll_receive(&mut exact), None);
    // An empty queue with an adequate buffer is not an error.
    assert_eq!(b.counters().rx_buf_too_small, 2);
}

#[test]
fn reset_empties_queues_and_flows_keeps_counters_and_counts_itself() {
    let mut b = backend();
    run(&mut b, &arp_request_for_gateway());
    b.transmit(&arp_request_for_gateway()).unwrap();
    let key = FlowKey {
        proto: FlowProto::Tcp,
        guest_port: 1,
        dst_ip: EXTERNAL,
        dst_port: 80,
    };
    b.open_flow(key).unwrap();
    let before = b.counters();
    b.reset();
    assert_eq!(b.process_pending(), 0);
    assert!(drain(&mut b).is_empty());
    assert_eq!(b.open_flow(key), Ok(()));
    let after = b.counters();
    assert_eq!(after.resets, 1);
    assert_eq!(after.arp_replies, before.arp_replies);
    assert_eq!(after.tx_frames, before.tx_frames);
}

#[test]
fn flow_limits_count_their_drops() {
    let mut b = backend();
    let key = |proto, n: u16| FlowKey {
        proto,
        guest_port: n,
        dst_ip: EXTERNAL,
        dst_port: 80,
    };
    for n in 0..MAX_TCP_FLOWS as u16 {
        b.open_flow(key(FlowProto::Tcp, n)).unwrap();
    }
    for n in 0..MAX_UDP_FLOWS as u16 {
        b.open_flow(key(FlowProto::Udp, n)).unwrap();
    }
    for n in 0..MAX_ICMP_ECHO_FLOWS as u16 {
        b.open_flow(key(FlowProto::IcmpEcho, n)).unwrap();
    }
    assert_eq!(
        b.counters().drop_flow_limit_tcp
            + b.counters().drop_flow_limit_udp
            + b.counters().drop_flow_limit_icmp,
        0
    );
    assert_eq!(
        b.open_flow(key(FlowProto::Tcp, 999)),
        Err(FlowError::LimitReached(FlowProto::Tcp))
    );
    assert_eq!(
        b.open_flow(key(FlowProto::Udp, 999)),
        Err(FlowError::LimitReached(FlowProto::Udp))
    );
    assert_eq!(
        b.open_flow(key(FlowProto::IcmpEcho, 999)),
        Err(FlowError::LimitReached(FlowProto::IcmpEcho))
    );
    let c = b.counters();
    assert_eq!(
        (
            c.drop_flow_limit_tcp,
            c.drop_flow_limit_udp,
            c.drop_flow_limit_icmp
        ),
        (1, 1, 1)
    );
}

type Field = fn(&NetCounters) -> u64;

/// Each case: a frame and the one counter that must increase by one when the backend handles it
/// (a row of the table in NETWORK.md section 12.2 or a rule of section 5). Nothing is sent to the guest.
fn silent_drop_cases() -> Vec<(&'static str, Vec<u8>, Field)> {
    let mut broadcast_ip = udp_frame(Ipv4Addr::new(10, 0, 2, 255), 9, b"x");
    broadcast_ip[0..6].copy_from_slice(&BROADCAST_MAC);
    let mut wrong_src_mac = udp_frame(EXTERNAL, 9, b"x");
    wrong_src_mac[6] ^= 2;
    let mut wrong_dst_mac = udp_frame(EXTERNAL, 9, b"x");
    wrong_dst_mac[5] ^= 0x40;
    let mut bad_sum = udp_frame(EXTERNAL, 9, b"x");
    bad_sum[L4 + 6] ^= 1;
    let mut group_mac_unicast_ip = udp_frame(EXTERNAL, 9, b"x");
    group_mac_unicast_ip[0..6].copy_from_slice(&BROADCAST_MAC);
    vec![
        ("ipv6", guest_frame(GATEWAY_MAC, 0x86DD, &[0; 60]), |c| {
            c.drop_unsupported_ethertype
        }),
        ("vlan", guest_frame(GATEWAY_MAC, 0x8100, &[0; 60]), |c| {
            c.drop_unsupported_ethertype
        }),
        ("src mac", wrong_src_mac, |c| c.drop_bad_src_mac),
        ("dst mac", wrong_dst_mac, |c| c.drop_bad_dst_mac),
        ("group mac with unicast ip", group_mac_unicast_ip, |c| {
            c.drop_bad_dst_mac
        }),
        (
            "fragment",
            patched_ip(udp_frame(EXTERNAL, 9, b"x"), |ip| ip[6] = 0x20),
            |c| c.drop_ip_fragment,
        ),
        (
            "ip options",
            patched_ip(udp_frame(EXTERNAL, 9, b"x"), |ip| ip[0] = 0x46),
            |c| c.drop_ip_options,
        ),
        (
            "ip malformed",
            patched_ip(udp_frame(EXTERNAL, 9, b"x"), |ip| ip[0] = 0x65),
            |c| c.drop_ip_malformed,
        ),
        (
            "bad src ip",
            ipv4_frame(
                &Ip {
                    src: Ipv4Addr::new(10, 0, 2, 99),
                    ..Ip::to(EXTERNAL, PROTO_UDP)
                },
                &udp_segment(Ipv4Addr::new(10, 0, 2, 99), EXTERNAL, 1, 2, b"x"),
            ),
            |c| c.drop_bad_src_ip,
        ),
        ("loopback", udp_frame(Ipv4Addr::LOCALHOST, 9, b"x"), |c| {
            c.drop_policy_dst
        }),
        (
            "169.254",
            udp_frame(Ipv4Addr::new(169, 254, 0, 9), 9, b"x"),
            |c| c.drop_policy_dst,
        ),
        ("own address", udp_frame(GUEST_IP, 9, b"x"), |c| {
            c.drop_policy_dst
        }),
        (
            "other subnet address",
            udp_frame(Ipv4Addr::new(10, 0, 2, 50), 9, b"x"),
            |c| c.drop_policy_dst,
        ),
        (
            "mdns",
            udp_frame(Ipv4Addr::new(224, 0, 0, 251), 5353, b"x"),
            |c| c.drop_multicast_broadcast,
        ),
        ("subnet broadcast", broadcast_ip, |c| {
            c.drop_multicast_broadcast
        }),
        ("gre", ipv4_frame(&Ip::to(EXTERNAL, 47), &[0; 8]), |c| {
            c.drop_unsupported_proto
        }),
        ("bad udp sum", bad_sum, |c| c.drop_l4_malformed),
        (
            "foreign arp",
            arp_request(Ipv4Addr::new(10, 0, 2, 77)),
            |c| c.drop_arp_other,
        ),
    ]
}

#[test]
fn every_silent_drop_row_increases_exactly_its_counter() {
    for (name, frame, field) in silent_drop_cases() {
        let mut b = backend();
        run(&mut b, &frame);
        let c = b.counters();
        assert_eq!(field(&c), 1, "{name}: its counter");
        let all = total_accounted(&c);
        assert_eq!(all, 1, "{name}: exactly one counter, got {c:?}");
        assert!(drain(&mut b).is_empty(), "{name}: the guest sees silence");
    }
}

/// Sum of every counter that accounts for the fate of one processed frame.
fn total_accounted(c: &NetCounters) -> u64 {
    c.drop_bad_src_mac
        + c.drop_bad_dst_mac
        + c.drop_unsupported_ethertype
        + c.drop_ip_malformed
        + c.drop_ip_options
        + c.drop_ip_fragment
        + c.drop_bad_src_ip
        + c.drop_policy_dst
        + c.drop_multicast_broadcast
        + c.drop_unsupported_proto
        + c.drop_l4_malformed
        + c.drop_arp_other
        + c.drop_unhandled_l4
        + c.arp_replies
        + c.arp_probe_ignored
        + c.drop_rx_queue_full
        // S11-31c: UDP and ICMP echo (an ICMP error that is queued counts its situation, one that
        // is rate limited counts `icmp_error_suppressed`, one that does not fit L3 counts
        // `drop_rx_queue_full` above).
        + c.udp_forwarded
        + c.drop_flow_limit_udp
        + c.drop_flow_limit_icmp
        + c.drop_host_resource
        + c.udp_unreachable
        + c.udp_refused
        + c.gw_port_unreachable
        + c.icmp_error_suppressed
        + c.icmp_echo_synthetic
        + c.icmp_echo_forwarded
}

#[test]
fn well_formed_packets_without_a_handler_are_counted_unhandled_until_it_exists() {
    // DHCP and DNS (S11-31b) and TCP (S11-31d); UDP to other addresses and ICMP echo are handled.
    let mut b = backend();
    for frame in [
        udp_frame(DNS_IP, 53, b"x"),
        udp_frame(GATEWAY_IP, 67, b"x"),
        tcp_syn_frame(EXTERNAL, 80),
        tcp_syn_frame(GATEWAY_IP, 80),
        dhcp_like_frame(),
    ] {
        run(&mut b, &frame);
    }
    let c = b.counters();
    assert_eq!(c.drop_unhandled_l4, 5);
    assert_eq!(total_accounted(&c), 5);
    assert!(drain(&mut b).is_empty());
}

#[test]
fn udp_and_echo_to_external_addresses_reach_the_host_socket_layer_not_the_unhandled_counter() {
    // The sockets of this backend cannot be created, so each frame ends in drop_host_resource.
    let mut b = backend();
    run(&mut b, &udp_frame(EXTERNAL, 9, b"x"));
    run(&mut b, &echo_frame(EXTERNAL));
    let c = b.counters();
    assert_eq!(c.drop_unhandled_l4, 0);
    assert_eq!(c.drop_host_resource, 2);
    assert_eq!(total_accounted(&c), 2);
    assert!(drain(&mut b).is_empty());
}

#[test]
fn product_backend_never_reaches_the_host_loopback() {
    let mut product = backend();
    run(&mut product, &tcp_syn_frame(Ipv4Addr::LOCALHOST, 80));
    assert_eq!(product.counters().drop_policy_dst, 1);
    assert_eq!(product.counters().drop_unhandled_l4, 0);
    let mut test_backend = NatBackend::with_policy(policy().allow_loopback_for_tests());
    run(&mut test_backend, &tcp_syn_frame(Ipv4Addr::LOCALHOST, 80));
    assert_eq!(test_backend.counters().drop_policy_dst, 0);
    assert_eq!(test_backend.counters().drop_unhandled_l4, 1);
}

#[test]
fn backend_is_usable_through_the_trait_object() {
    let mut boxed: Box<dyn NetBackend> = Box::new(backend());
    assert_eq!(boxed.transmit(&[0; 3]), Err(Dropped::Runt));
    assert_eq!(boxed.counters().drop_runt, 1);
    boxed.reset();
    assert_eq!(boxed.counters().resets, 1);
}

#[test]
fn every_drop_reason_has_its_own_counter() {
    use DropReason::*;
    let reasons = [
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
    ];
    let mut seen = Vec::new();
    for reason in reasons {
        let mut counters = NetCounters::default();
        record_drop(&mut counters, reason);
        assert_ne!(counters, NetCounters::default(), "{reason:?}");
        assert!(!seen.contains(&counters), "{reason:?} shares a counter");
        seen.push(counters);
    }
}

// ---- Garbage: deterministic fuzz -------------------------------------------------------------

fn within(frame: &[u8], part: &[u8]) -> bool {
    let start = frame.as_ptr() as usize;
    let p = part.as_ptr() as usize;
    p >= start && p + part.len() <= start + frame.len()
}

/// What classify promises about every frame it accepts, checked with the independent reference routines.
fn check_accepted(frame: &[u8], inbound: &Inbound<'_>, policy: &Policy) {
    match inbound {
        Inbound::ArpIgnored => {}
        Inbound::ArpWhoHas { target_ip, .. } => {
            assert!(*target_ip == GATEWAY_IP || *target_ip == DNS_IP);
            // The ARP sender hardware address is the guest MAC (NETWORK.md section 6).
            assert_eq!(frame[22..28], GUEST_MAC, "ARP sender MAC");
        }
        Inbound::Ipv4(p) => {
            assert!(within(frame, p.header) && within(frame, p.payload));
            assert_eq!(p.header.len(), 20);
            assert_eq!(ref_checksum(&[p.header]), 0, "header checksum");
            assert!(p.ttl > 0);
            assert!(p.src == GUEST_IP || p.src.is_unspecified());
            let o = p.dst.octets();
            assert!(o[0] != 0 && !(o[0] == 169 && o[1] == 254));
            assert!(policy.allows_loopback() || o[0] != 127);
            assert!(o[..3] != [10, 0, 2] || matches!(o[3], 2 | 3));
            assert!(matches!(p.protocol, 1 | 6 | 17));
            // Destination MAC: the gateway or a group address; a group MAC needs a group IP.
            let mac_is_group = frame[0] & 1 != 0;
            assert!(
                mac_is_group || frame[0..6] == GATEWAY_MAC,
                "destination MAC"
            );
            let ip_is_group = o[0] >= 224;
            if mac_is_group {
                assert!(ip_is_group, "group MAC with unicast destination {}", p.dst);
            }
            // A group or broadcast destination is accepted only as DHCP: 255.255.255.255, UDP port 67.
            if ip_is_group {
                assert_eq!(p.dst, Ipv4Addr::BROADCAST);
                assert!(
                    matches!(p.l4, L4::Udp(u) if u.dst_port == 67),
                    "group destination"
                );
            }
            // The unspecified source is accepted only as the DHCP client: UDP 68 -> 67 to the
            // limited broadcast address or the gateway.
            if p.src.is_unspecified() {
                assert!(p.dst == Ipv4Addr::BROADCAST || p.dst == GATEWAY_IP);
                assert!(
                    matches!(p.l4, L4::Udp(u) if u.src_port == 68 && u.dst_port == 67),
                    "zero source"
                );
            }
            match p.l4 {
                L4::Udp(u) => assert!(within(frame, u.data) && u.data.len() + 8 <= p.payload.len()),
                L4::Tcp(t) => assert!(
                    within(frame, t.data)
                        && within(frame, t.options)
                        && 20 + t.options.len() + t.data.len() == p.payload.len()
                ),
                L4::Icmp(m) => assert!(
                    within(frame, m.data)
                        && 8 + m.data.len() == p.payload.len()
                        && ref_checksum(&[p.payload]) == 0
                ),
            }
        }
    }
}

fn base_frames() -> Vec<Vec<u8>> {
    vec![
        arp_request_for_gateway(),
        arp_frame(BROADCAST_MAC, 1, GUEST_MAC, Ipv4Addr::UNSPECIFIED, GUEST_IP),
        udp_frame(EXTERNAL, 53, b"a udp payload"),
        udp_frame(DNS_IP, 53, &[0x12; 33]),
        tcp_syn_frame(EXTERNAL, 443),
        echo_frame(EXTERNAL),
        dhcp_like_frame(),
        udp_frame(Ipv4Addr::LOCALHOST, 9, b"loop"),
    ]
}

#[test]
fn every_length_from_0_to_1600_with_garbage_is_survived() {
    let mut rng = Rng::new(0x5EED_0001);
    let strict = policy();
    let open = policy().allow_loopback_for_tests();
    for len in 0..=1600usize {
        for round in 0..6 {
            let mut frame = vec![0u8; len];
            match round {
                0 => {}
                1 => frame.fill(0xFF),
                _ => rng.fill(&mut frame),
            }
            // Make the interesting headers reachable now and then: guest source MAC, then a type.
            if round >= 3 && len >= 14 {
                frame[6..12].copy_from_slice(&GUEST_MAC);
                if round == 4 {
                    frame[0..6].copy_from_slice(&GATEWAY_MAC);
                    frame[12..14].copy_from_slice(&ETHERTYPE_IPV4.to_be_bytes());
                }
                if round == 5 {
                    frame[0..6].copy_from_slice(&BROADCAST_MAC);
                    frame[12..14].copy_from_slice(&ETHERTYPE_ARP.to_be_bytes());
                }
            }
            for p in [&strict, &open] {
                if let Ok(inbound) = classify(&frame, p) {
                    check_accepted(&frame, &inbound, p);
                }
            }
        }
    }
}

#[test]
fn every_prefix_and_every_single_byte_change_of_valid_frames_is_survived() {
    let strict = policy();
    let open = policy().allow_loopback_for_tests();
    for base in base_frames() {
        // Every truncation, and every extension with garbage up to the maximum size.
        for len in 0..=base.len() {
            for p in [&strict, &open] {
                if let Ok(inbound) = classify(&base[..len], p) {
                    check_accepted(&base[..len], &inbound, p);
                }
            }
        }
        // Every value of every byte of the headers and of the first bytes of the payload.
        for at in 0..base.len().min(80) {
            for value in 0..=255u8 {
                let mut frame = base.clone();
                frame[at] = value;
                // Once as it is, once with both checksums repaired.
                let mut repaired = frame.clone();
                if repaired.len() >= 34 {
                    fix_ip_checksum(&mut repaired);
                    fix_l4_checksum(&mut repaired);
                }
                for candidate in [&frame, &repaired] {
                    for p in [&strict, &open] {
                        if let Ok(inbound) = classify(candidate, p) {
                            check_accepted(candidate, &inbound, p);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn random_mutations_of_valid_frames_keep_the_promises() {
    let mut rng = Rng::new(0xC0FF_EE01);
    let strict = policy();
    let open = policy().allow_loopback_for_tests();
    let bases = base_frames();
    let mut accepted_count = 0u32;
    for round in 0..60_000u32 {
        let mut frame = bases[rng.below(bases.len())].clone();
        for _ in 0..1 + rng.below(4) {
            let at = rng.below(frame.len());
            frame[at] = rng.next() as u8;
        }
        match rng.below(6) {
            0 => frame.truncate(rng.below(frame.len() + 1)),
            1 => {
                let extra = rng.below(NET_MAX_FRAME + 4 - frame.len());
                frame.extend((0..extra).map(|_| rng.next() as u8));
            }
            _ => {}
        }
        // Frames with a repaired IPv4 header checksum get past item 4 and exercise the later items.
        if frame.len() >= 34 && round % 2 == 0 {
            fix_ip_checksum(&mut frame);
            // In a part of the rounds the transport checksum is repaired too, so that mutations of
            // the transport header reach item 8 and later instead of stopping at the checksum.
            if round % 4 == 0 {
                fix_l4_checksum(&mut frame);
            }
        }
        for p in [&strict, &open] {
            if let Ok(inbound) = classify(&frame, p) {
                accepted_count += 1;
                check_accepted(&frame, &inbound, p);
            }
        }
    }
    // The generator does reach the accepting paths, so the checks above were exercised.
    assert!(
        accepted_count > 1000,
        "only {accepted_count} frames were accepted"
    );
}

#[test]
fn backend_accounts_for_every_frame_of_a_garbage_stream() {
    let mut rng = Rng::new(0xBAD_F00D);
    let mut b = backend();
    let bases = base_frames();
    let mut calls = 0u64;
    let mut buf = [0u8; NET_MAX_FRAME];
    for _ in 0..20_000 {
        let mut frame = match rng.below(3) {
            0 => {
                let mut f = vec![0u8; rng.below(NET_MAX_FRAME + 40)];
                rng.fill(&mut f);
                f
            }
            _ => {
                let mut f = bases[rng.below(bases.len())].clone();
                let at = rng.below(f.len());
                f[at] = rng.next() as u8;
                f
            }
        };
        if frame.len() >= 34 && rng.below(2) == 0 {
            fix_ip_checksum(&mut frame);
            if rng.below(2) == 0 {
                fix_l4_checksum(&mut frame);
            }
        }
        calls += 1;
        let _ = b.transmit(&frame);
        if rng.below(8) == 0 {
            b.process_pending();
            while b.poll_receive(&mut buf).is_some() {}
        }
    }
    b.process_pending();
    let c = b.counters();
    // Every call to transmit ended in exactly one of: accepted, runt, oversize, queue full.
    assert_eq!(
        calls,
        c.tx_frames + c.drop_runt + c.drop_oversize + c.drop_tx_queue_full
    );
    // Every accepted frame ended in exactly one counter after processing.
    assert_eq!(c.tx_frames, total_accounted(&c));
}

// ---- Frames built for the guest ---------------------------------------------------------------

#[test]
fn ipv4_frame_for_the_guest_has_the_documented_fields_at_the_largest_size() {
    let mut rng = Rng::new(0x1480);
    let mut l4 = vec![0u8; 1480];
    rng.fill(&mut l4);
    let mut out = [0u8; 2000];
    let len = build_ipv4_frame(
        GUEST_MAC, EXTERNAL, GUEST_IP, PROTO_UDP, 0xBEEF, &l4, &mut out,
    )
    .expect("1480 bytes of transport data make a 1514-byte frame");
    assert_eq!(len, 1514);
    let frame = &out[..len];
    // Ethernet: to the guest, from the gateway, IPv4.
    assert_eq!(frame[0..6], GUEST_MAC);
    assert_eq!(frame[6..12], GATEWAY_MAC);
    assert_eq!(frame[12..14], ETHERTYPE_IPV4.to_be_bytes());
    // IPv4: version 4 / IHL 5, no TOS, length 1500, the identification we gave, DF and no
    // fragment offset, TTL 64, the protocol and the addresses; the header sums to zero.
    let ip = &frame[14..34];
    assert_eq!(ip[0], 0x45);
    assert_eq!(ip[1], 0);
    assert_eq!(ip[2..4], 1500u16.to_be_bytes());
    assert_eq!(ip[4..6], 0xBEEFu16.to_be_bytes());
    assert_eq!(ip[6..8], 0x4000u16.to_be_bytes());
    assert_eq!(ip[8], 64);
    assert_eq!(ip[9], PROTO_UDP);
    assert_eq!(ip[12..16], EXTERNAL.octets());
    assert_eq!(ip[16..20], GUEST_IP.octets());
    assert_eq!(ref_checksum(&[ip]), 0, "header checksum");
    assert_eq!(frame[34..], l4[..]);
}

#[test]
fn ipv4_frame_for_the_guest_is_refused_when_too_long_or_when_the_buffer_is_short() {
    let l4 = vec![7u8; 1481];
    let mut out = [0u8; 2000];
    assert_eq!(
        build_ipv4_frame(GUEST_MAC, EXTERNAL, GUEST_IP, PROTO_TCP, 1, &l4, &mut out),
        None,
        "1481 bytes make 1515"
    );
    // One byte short of the frame, and an empty buffer.
    let l4 = vec![7u8; 1480];
    assert_eq!(
        build_ipv4_frame(
            GUEST_MAC,
            EXTERNAL,
            GUEST_IP,
            PROTO_TCP,
            1,
            &l4,
            &mut out[..1513]
        ),
        None
    );
    assert_eq!(
        build_ipv4_frame(
            GUEST_MAC,
            EXTERNAL,
            GUEST_IP,
            PROTO_ICMP,
            1,
            &[1, 2, 3],
            &mut []
        ),
        None
    );
    let small = [0u8; 8];
    assert_eq!(
        build_ipv4_frame(
            GUEST_MAC,
            EXTERNAL,
            GUEST_IP,
            PROTO_UDP,
            1,
            &small,
            &mut out[..41]
        ),
        None,
        "42 bytes needed"
    );
    // Exactly long enough works and does not write past the frame.
    out.fill(0xEE);
    let len = build_ipv4_frame(
        GUEST_MAC,
        EXTERNAL,
        GUEST_IP,
        PROTO_UDP,
        1,
        &small,
        &mut out[..42],
    )
    .unwrap();
    assert_eq!(len, 42);
    assert_eq!(out[42], 0xEE);
    // An empty transport message is a 34-byte frame.
    assert_eq!(
        build_ipv4_frame(GUEST_MAC, EXTERNAL, GUEST_IP, PROTO_UDP, 1, &[], &mut out),
        Some(34)
    );
}

#[test]
fn built_ipv4_headers_sum_to_zero_for_every_transport_length() {
    let mut rng = Rng::new(0x1481);
    let mut out = [0u8; 1514];
    for l4_len in 0..=1480usize {
        let mut l4 = vec![0u8; l4_len];
        rng.fill(&mut l4);
        let id = rng.next() as u16;
        let protocol = [PROTO_ICMP, PROTO_TCP, PROTO_UDP][l4_len % 3];
        let len =
            build_ipv4_frame(GUEST_MAC, GATEWAY_IP, GUEST_IP, protocol, id, &l4, &mut out).unwrap();
        assert_eq!(len, 34 + l4_len);
        assert_eq!(ref_checksum(&[&out[14..34]]), 0, "l4 length {l4_len}");
        assert_eq!(out[16..18], ((20 + l4_len) as u16).to_be_bytes());
    }
}

#[test]
fn l4_checksum_of_odd_and_even_lengths_matches_the_reference_sum() {
    let mut rng = Rng::new(0x1482);
    for (protocol, field) in [(PROTO_UDP, 6usize), (PROTO_TCP, 16)] {
        for len in (field + 2)..=(field + 2 + 41) {
            let mut l4 = vec![0u8; len];
            rng.fill(&mut l4);
            l4[field] = 0;
            l4[field + 1] = 0;
            let expected = ref_checksum(&[&pseudo(EXTERNAL, GUEST_IP, protocol, len), &l4]);
            assert_eq!(
                l4_checksum(EXTERNAL, GUEST_IP, protocol, &l4),
                expected,
                "protocol {protocol}, length {len}"
            );
            // With the value in place the whole message sums to zero (odd lengths pad with zero).
            l4[field..field + 2].copy_from_slice(&expected.to_be_bytes());
            assert_eq!(l4_checksum(EXTERNAL, GUEST_IP, protocol, &l4), 0);
        }
    }
    // A hand-computed vector with an odd length: 10.0.2.2 -> 10.0.2.15, UDP, 9 bytes
    // (ports 53 -> 1024, length 9, checksum 0, one data byte 0x41).
    let l4 = [0, 53, 4, 0, 0, 9, 0, 0, 0x41];
    // 0x0A00+0x0202+0x0A00+0x020F+0x0011+0x0009 (pseudo, length 9) + 0x0035+0x0400+0x0009+0x4100
    // = 0x0A00+0x0202=0x0C02; +0x0A00=0x1602; +0x020F=0x1811; +0x0011=0x1822; +0x0009=0x182B;
    // +0x0035=0x1860; +0x0400=0x1C60; +0x0009=0x1C69; +0x4100=0x5D69; checksum = !0x5D69 = 0xA296.
    assert_eq!(l4_checksum(GATEWAY_IP, GUEST_IP, PROTO_UDP, &l4), 0xA296);
}
