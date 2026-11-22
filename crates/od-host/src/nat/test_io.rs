//! Helpers for the tests of the handlers that use host sockets (S11-31c).
//!
//! These tests talk to servers on `127.0.0.1` only: the backend of the test has the loopback policy
//! of NETWORK.md section 14, item 4. A test that feeds arbitrary frames uses [`NatBackend::new`]
//! with [`NatBackend::without_host_sockets`] instead and never touches the network of the machine.

use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use od_machine::{NET_MAX_FRAME, NetBackend};

use super::NatBackend;
use super::frame::{GATEWAY_MAC, GUEST_IP, Policy};
use super::tcp_tests::{FakeHost, scripted_host};
use super::test_frames::{GUEST_MAC, IP, L4, ref_checksum};

/// A backend whose guest may reach the host loopback (tests only).
pub fn io_backend() -> NatBackend {
    let policy = Policy::new(GUEST_MAC)
        .expect("the test MAC is valid")
        .allow_loopback_for_tests();
    // TCP goes to the scripted host: these tests have no TCP.
    NatBackend::with_policy_and_tcp_host(policy, Box::new(FakeHost(scripted_host())))
}

/// Every frame the guest can take now.
pub fn drain(backend: &mut NatBackend) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = [0u8; NET_MAX_FRAME];
    while let Some(len) = backend.poll_receive(&mut buf) {
        out.push(buf[..len].to_vec());
    }
    out
}

/// Polls the sockets (real time) until `count` frames for the guest have arrived or two seconds
/// have passed; returns what arrived.
pub fn pump_for(backend: &mut NatBackend, count: usize) -> Vec<Vec<u8>> {
    let started = Instant::now();
    let mut frames = Vec::new();
    while frames.len() < count && started.elapsed() < Duration::from_secs(2) {
        backend.service_at(Instant::now());
        frames.extend(drain(backend));
        if frames.len() < count {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    frames
}

/// Like [`pump_for`], but the backend is told that it is `now` (the real wait is the same).
pub fn pump_at(backend: &mut NatBackend, now: Instant, count: usize) -> Vec<Vec<u8>> {
    let started = Instant::now();
    let mut frames = Vec::new();
    while frames.len() < count && started.elapsed() < Duration::from_secs(2) {
        backend.service_at(now);
        frames.extend(drain(backend));
        if frames.len() < count {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    frames
}

/// Polls the sockets for a while (real time) and returns the frames that arrived: for tests that
/// expect silence.
pub fn pump_quiet(backend: &mut NatBackend, window: Duration) -> Vec<Vec<u8>> {
    let started = Instant::now();
    let mut frames = Vec::new();
    while started.elapsed() < window {
        backend.service_at(Instant::now());
        frames.extend(drain(backend));
        std::thread::sleep(Duration::from_millis(2));
    }
    frames
}

/// An IPv4 frame the backend built for the guest, checked field by field with the reference sum.
pub struct GuestPacket {
    pub src: Ipv4Addr,
    pub dst: Ipv4Addr,
    pub protocol: u8,
    pub identification: u16,
    /// The transport message (header and data).
    pub l4: Vec<u8>,
}

/// Checks everything NETWORK.md section 5 says about a frame for the guest (Ethernet addresses,
/// version, TTL 64, Don't Fragment, lengths, header checksum) and splits it.
pub fn parse_guest_packet(frame: &[u8]) -> GuestPacket {
    assert!(
        frame.len() >= L4 && frame.len() <= NET_MAX_FRAME,
        "length {}",
        frame.len()
    );
    assert_eq!(frame[0..6], GUEST_MAC, "to the guest");
    assert_eq!(frame[6..12], GATEWAY_MAC, "from the gateway");
    assert_eq!(frame[12..14], [0x08, 0x00], "IPv4");
    let ip = &frame[IP..IP + 20];
    assert_eq!(ip[0], 0x45);
    assert_eq!(
        usize::from(u16::from_be_bytes([ip[2], ip[3]])),
        frame.len() - IP
    );
    assert_eq!(ip[6..8], [0x40, 0x00], "Don't Fragment, no offset");
    assert_eq!(ip[8], 64, "TTL");
    assert_eq!(ref_checksum(&[ip]), 0, "header checksum");
    let dst = Ipv4Addr::new(ip[16], ip[17], ip[18], ip[19]);
    assert_eq!(dst, GUEST_IP, "addressed to the guest");
    GuestPacket {
        src: Ipv4Addr::new(ip[12], ip[13], ip[14], ip[15]),
        dst,
        protocol: ip[9],
        identification: u16::from_be_bytes([ip[4], ip[5]]),
        l4: frame[L4..].to_vec(),
    }
}
