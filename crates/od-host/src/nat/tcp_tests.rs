//! Tests of the TCP handler (S11-31d): a scripted host for the state machine and the limits, real
//! loopback sockets for the system host. Frames come from the generators of test_frames.rs; the
//! replies of the backend are checked field by field with the reference checksum, not with the
//! production one.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use od_machine::{NET_MAX_FRAME, NetBackend, NetCounters};

use super::NatBackend;
use super::flow::{FlowProto, MAX_TCP_FLOWS};
use super::frame::*;
use super::tcp::*;
use super::test_frames::*;

const SERVER: Ipv4Addr = Ipv4Addr::new(93, 184, 216, 34);
const FIN: u8 = 0x01;
const SYN: u8 = 0x02;
const RST: u8 = 0x04;
const PSH: u8 = 0x08;
const ACK: u8 = 0x10;
const MSS_1460: [u8; 4] = [2, 4, 0x05, 0xB4];

// ---- A scripted host --------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Connect {
    Pending,
    Done,
    Failed(HostError),
}

pub(super) struct StreamState {
    pub connect: Connect,
    /// Bytes the host side sends to the guest.
    pub inbound: VecDeque<u8>,
    pub eof: bool,
    pub read_fails: bool,
    /// Bytes the backend wrote to the host.
    pub written: Vec<u8>,
    /// How many more bytes a write takes before it would block.
    pub write_budget: usize,
    pub write_fails: bool,
    pub shut_wr: bool,
    pub read_total: usize,
    /// The socket is closed (the stream was dropped).
    pub closed: bool,
}

pub(super) struct HostState {
    /// The next connect fails at once with this error.
    pub immediate_error: Option<HostError>,
    /// The state new connections start in.
    pub initial: Connect,
    pub streams: Vec<(SocketAddrV4, Arc<Mutex<StreamState>>)>,
}

pub(super) struct FakeHost(pub Arc<Mutex<HostState>>);

struct FakeStream(Arc<Mutex<StreamState>>);

impl TcpHost for FakeHost {
    fn connect(&mut self, dst: SocketAddrV4) -> Result<Box<dyn HostStream>, HostError> {
        let mut host = self.0.lock().unwrap();
        if let Some(error) = host.immediate_error.take() {
            return Err(error);
        }
        let state = Arc::new(Mutex::new(StreamState {
            connect: host.initial,
            inbound: VecDeque::new(),
            eof: false,
            read_fails: false,
            written: Vec::new(),
            write_budget: usize::MAX,
            write_fails: false,
            shut_wr: false,
            read_total: 0,
            closed: false,
        }));
        host.streams.push((dst, state.clone()));
        Ok(Box::new(FakeStream(state)))
    }
}

impl HostStream for FakeStream {
    fn poll_connected(&mut self) -> Result<bool, HostError> {
        match self.0.lock().unwrap().connect {
            Connect::Pending => Ok(false),
            Connect::Done => Ok(true),
            Connect::Failed(error) => Err(error),
        }
    }

    fn read(&mut self, buf: &mut [u8]) -> IoResult {
        let mut s = self.0.lock().unwrap();
        if s.read_fails {
            return IoResult::Failed;
        }
        if s.inbound.is_empty() {
            return if s.eof {
                IoResult::Eof
            } else {
                IoResult::WouldBlock
            };
        }
        let n = buf.len().min(s.inbound.len());
        for slot in &mut buf[..n] {
            *slot = s.inbound.pop_front().unwrap();
        }
        s.read_total += n;
        IoResult::Ready(n)
    }

    fn write(&mut self, buf: &[u8]) -> IoResult {
        let mut s = self.0.lock().unwrap();
        if s.write_fails {
            return IoResult::Failed;
        }
        if s.write_budget == 0 {
            return IoResult::WouldBlock;
        }
        let n = buf.len().min(s.write_budget);
        s.written.extend_from_slice(&buf[..n]);
        if s.write_budget != usize::MAX {
            s.write_budget -= n;
        }
        IoResult::Ready(n)
    }

    fn shutdown_write(&mut self) {
        self.0.lock().unwrap().shut_wr = true;
    }
}

impl Drop for FakeStream {
    fn drop(&mut self) {
        self.0.lock().unwrap().closed = true;
    }
}

/// A backend over the scripted host with a clock the test moves.
pub(super) struct Rig {
    pub b: NatBackend,
    pub host: Arc<Mutex<HostState>>,
    pub now: Instant,
}

/// A scripted host whose connects complete at once.
pub(super) fn scripted_host() -> Arc<Mutex<HostState>> {
    Arc::new(Mutex::new(HostState {
        immediate_error: None,
        initial: Connect::Done,
        streams: Vec::new(),
    }))
}

/// A backend for the other test modules: scripted host, connects complete at once.
pub(super) fn scripted_backend() -> NatBackend {
    Rig::new().b
}

impl Rig {
    pub(super) fn new() -> Self {
        let host = scripted_host();
        let policy = Policy::new(GUEST_MAC).unwrap();
        let b = NatBackend::with_policy_and_tcp_host(policy, Box::new(FakeHost(host.clone())))
            .without_host_sockets();
        Self {
            b,
            host,
            now: Instant::now(),
        }
    }

    fn stream(&self, index: usize) -> Arc<Mutex<StreamState>> {
        self.host.lock().unwrap().streams[index].1.clone()
    }

    fn connects(&self) -> usize {
        self.host.lock().unwrap().streams.len()
    }

    fn tick(&mut self, by: Duration) {
        self.now += by;
        self.b.run_at(self.now);
    }

    /// Moves the clock to `t0 + at` (absolute from the start of the scenario) and runs a turn.
    fn at(&mut self, since: Instant, at: Duration) {
        self.now = since + at;
        self.b.run_at(self.now);
    }

    fn push(&mut self, frame: &[u8]) {
        self.b.transmit(frame).unwrap();
        self.b.run_at(self.now);
    }

    fn replies(&mut self) -> Vec<Reply> {
        let mut out = Vec::new();
        let mut buf = [0u8; NET_MAX_FRAME];
        while let Some(len) = self.b.poll_receive(&mut buf) {
            out.push(parse_reply(&buf[..len]));
        }
        out
    }

    fn tcp_replies(&mut self) -> Vec<Tcp> {
        self.replies()
            .into_iter()
            .map(|reply| match reply {
                Reply::Tcp(t) => t,
                other => panic!("expected TCP, got {other:?}"),
            })
            .collect()
    }

    fn counters(&self) -> NetCounters {
        self.b.counters()
    }

    fn flows(&self) -> usize {
        self.b.flows.count(FlowProto::Tcp)
    }

    fn send_syn(
        &mut self,
        dst: Ipv4Addr,
        guest_port: u16,
        dst_port: u16,
        isn: u32,
        options: &[u8],
    ) {
        let frame = tcp_frame(
            dst,
            &Seg {
                guest_port,
                dst_port,
                seq: isn,
                flags: SYN,
                options,
                ..Seg::default()
            },
        );
        self.push(&frame);
    }

    /// A connection that is open: the handshake is done, nothing is pending.
    fn open(&mut self) -> Conn {
        self.open_to(SERVER, 80, 40000)
    }

    fn open_to(&mut self, dst: Ipv4Addr, dst_port: u16, guest_port: u16) -> Conn {
        self.send_syn(dst, guest_port, dst_port, 1000, &MSS_1460);
        let replies = self.tcp_replies();
        assert_eq!(replies.len(), 1, "one SYN-ACK");
        assert_eq!(replies[0].flags, SYN | ACK);
        let mut conn = Conn {
            dst,
            guest_port,
            dst_port,
            g_seq: 1001,
            h_next: replies[0].seq.wrapping_add(1),
            window: 65535,
        };
        self.guest(&mut conn, ACK, &[]);
        assert!(
            self.replies().is_empty(),
            "the ACK of the handshake gets no answer"
        );
        conn
    }

    /// The guest sends a segment with `flags` and `data` (numbers and window from `conn`).
    fn guest(&mut self, conn: &mut Conn, flags: u8, data: &[u8]) {
        let frame = conn.frame(flags, data);
        conn.g_seq = conn
            .g_seq
            .wrapping_add(data.len() as u32 + u32::from(flags & FIN != 0));
        self.push(&frame);
    }

    /// The numbers the guest has to acknowledge after the backend's segments.
    fn absorb(conn: &mut Conn, replies: &[Tcp]) {
        for r in replies {
            if r.seq == conn.h_next {
                conn.h_next = conn
                    .h_next
                    .wrapping_add(r.data.len() as u32 + u32::from(r.flags & FIN != 0));
            }
        }
    }
}

#[derive(Clone)]
struct Conn {
    dst: Ipv4Addr,
    guest_port: u16,
    dst_port: u16,
    /// The guest's next sequence number, and the next one it expects from the backend.
    g_seq: u32,
    h_next: u32,
    window: u16,
}

impl Conn {
    fn frame(&self, flags: u8, data: &[u8]) -> Vec<u8> {
        tcp_frame(
            self.dst,
            &Seg {
                guest_port: self.guest_port,
                dst_port: self.dst_port,
                seq: self.g_seq,
                ack: self.h_next,
                flags,
                window: self.window,
                data,
                ..Seg::default()
            },
        )
    }
}

struct Seg<'a> {
    guest_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
    options: &'a [u8],
    data: &'a [u8],
}

impl Default for Seg<'_> {
    fn default() -> Self {
        Seg {
            guest_port: 40000,
            dst_port: 80,
            seq: 0,
            ack: 0,
            flags: 0,
            window: 65535,
            options: &[],
            data: &[],
        }
    }
}

fn tcp_frame(dst: Ipv4Addr, s: &Seg<'_>) -> Vec<u8> {
    ipv4_frame(
        &Ip::to(dst, PROTO_TCP),
        &tcp_segment_with_window(
            GUEST_IP,
            dst,
            s.guest_port,
            s.dst_port,
            s.seq,
            s.ack,
            s.flags,
            s.window,
            s.options,
            s.data,
        ),
    )
}

// ---- What the guest receives ------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Tcp {
    src_ip: Ipv4Addr,
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
    mss: Option<u16>,
    data: Vec<u8>,
}

#[derive(Debug)]
enum Reply {
    Tcp(Tcp),
    Unreachable {
        src_ip: Ipv4Addr,
        code: u8,
        quoted: Vec<u8>,
    },
}

fn be16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([b[at], b[at + 1]])
}

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Checks the Ethernet and IPv4 fields of NETWORK.md section 5 (last paragraph) and the transport
/// checksum with the reference routine, and describes the segment or the ICMP error.
fn parse_reply(f: &[u8]) -> Reply {
    assert!(
        (34..=NET_MAX_FRAME).contains(&f.len()),
        "frame length {}",
        f.len()
    );
    assert_eq!(f[0..6], GUEST_MAC);
    assert_eq!(f[6..12], GATEWAY_MAC);
    assert_eq!(f[12..14], [8, 0]);
    let ip = &f[14..34];
    assert_eq!(ip[0], 0x45);
    assert_eq!(ip[6..8], [0x40, 0], "Don't Fragment, no fragment");
    assert_eq!(ip[8], 64, "TTL");
    assert_eq!(ref_checksum(&[ip]), 0, "IPv4 header checksum");
    assert_eq!(14 + usize::from(be16(ip, 2)), f.len(), "IPv4 length");
    let src = Ipv4Addr::new(ip[12], ip[13], ip[14], ip[15]);
    assert_eq!(Ipv4Addr::new(ip[16], ip[17], ip[18], ip[19]), GUEST_IP);
    let l4 = &f[34..];
    match ip[9] {
        6 => {
            assert_eq!(
                ref_checksum(&[&pseudo(src, GUEST_IP, 6, l4.len()), l4]),
                0,
                "TCP checksum"
            );
            let header = usize::from(l4[12] >> 4) * 4;
            let mss = (header == 24).then(|| {
                assert_eq!(l4[20..22], [2, 4]);
                be16(l4, 22)
            });
            assert!(header == 20 || header == 24, "header {header}");
            Reply::Tcp(Tcp {
                src_ip: src,
                src_port: be16(l4, 0),
                dst_port: be16(l4, 2),
                seq: be32(l4, 4),
                ack: be32(l4, 8),
                flags: l4[13],
                window: be16(l4, 14),
                mss,
                data: l4[header..].to_vec(),
            })
        }
        1 => {
            assert_eq!(ref_checksum(&[l4]), 0, "ICMP checksum");
            assert_eq!(l4[0], 3, "destination unreachable");
            assert_eq!(l4[4..8], [0; 4]);
            Reply::Unreachable {
                src_ip: src,
                code: l4[1],
                quoted: l4[8..].to_vec(),
            }
        }
        other => panic!("protocol {other}"),
    }
}

fn data_of(replies: &[Tcp]) -> Vec<u8> {
    replies
        .iter()
        .flat_map(|r| r.data.iter().copied())
        .collect()
}

fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

// ---- Opening ----------------------------------------------------------------------------------

#[test]
fn syn_waits_for_the_connect_and_the_synack_has_the_documented_fields() {
    let mut rig = Rig::new();
    rig.host.lock().unwrap().initial = Connect::Pending;
    rig.send_syn(SERVER, 40000, 80, 1000, &MSS_1460);
    assert!(
        rig.replies().is_empty(),
        "nothing is sent before the connect completes"
    );
    assert_eq!(rig.connects(), 1);
    assert_eq!(
        rig.host.lock().unwrap().streams[0].0,
        SocketAddrV4::new(SERVER, 80)
    );
    // A repeated SYN is ignored while the connect is in progress.
    rig.send_syn(SERVER, 40000, 80, 1000, &MSS_1460);
    assert!(rig.replies().is_empty());
    assert_eq!(rig.connects(), 1);

    rig.stream(0).lock().unwrap().connect = Connect::Done;
    rig.tick(Duration::from_millis(10));
    let replies = rig.tcp_replies();
    assert_eq!(replies.len(), 1);
    let synack = &replies[0];
    assert_eq!(synack.flags, SYN | ACK);
    assert_eq!(synack.ack, 1001);
    assert_eq!(synack.mss, Some(1460));
    assert_eq!(synack.window, 65535);
    assert_eq!(
        (synack.src_ip, synack.src_port, synack.dst_port),
        (SERVER, 80, 40000)
    );
    assert!(synack.data.is_empty());

    // The guest repeats its SYN (our SYN-ACK was lost): the SYN-ACK is sent again.
    rig.send_syn(SERVER, 40000, 80, 1000, &MSS_1460);
    let again = rig.tcp_replies();
    assert_eq!(again.len(), 1);
    assert_eq!((again[0].seq, again[0].flags), (synack.seq, SYN | ACK));
    let c = rig.counters();
    assert_eq!((c.tcp_segments, c.tcp_opened, c.tcp_retransmits), (3, 1, 1));

    // A SYN-ACK is not repeated by a timer: nothing for 100 s.
    let t = rig.now;
    rig.at(t, Duration::from_secs(100));
    assert!(rig.replies().is_empty());
    assert_eq!(rig.flows(), 1);
}

#[test]
fn the_syn_data_and_garbage_options_are_handled_without_trusting_them() {
    // Data in the SYN is not delivered; the handshake still completes.
    let mut rig = Rig::new();
    let frame = tcp_frame(
        SERVER,
        &Seg {
            seq: 1000,
            flags: SYN,
            options: &MSS_1460,
            data: b"xyz",
            ..Seg::default()
        },
    );
    rig.push(&frame);
    let synack = rig.tcp_replies().remove(0);
    assert_eq!(synack.ack, 1001);
    let mut c = Conn {
        dst: SERVER,
        guest_port: 40000,
        dst_port: 80,
        g_seq: 1001,
        h_next: synack.seq + 1,
        window: 65535,
    };
    rig.guest(&mut c, ACK, &[]);
    assert!(rig.stream(0).lock().unwrap().written.is_empty());

    // The MSS of the guest limits the segments of the backend: down to the guest's value, at most 1460,
    // and 536 when the guest announces none or the option is broken.
    for (options, mss) in [
        (&[2u8, 4, 0x03, 0xE8][..], 1000),
        (&[2, 4, 0x23, 0x28][..], 1460),
        (&[][..], 536),
        (&[1, 1, 1, 1][..], 536),
        (&[2, 0, 0, 0][..], 536),
        (&[2, 4, 0, 0][..], 536),
        (&[3, 3, 7, 1][..], 536),
    ] {
        let mut rig = Rig::new();
        rig.send_syn(SERVER, 40000, 80, 1000, options);
        let synack = rig.tcp_replies().remove(0);
        let mut c = Conn {
            dst: SERVER,
            guest_port: 40000,
            dst_port: 80,
            g_seq: 1001,
            h_next: synack.seq + 1,
            window: 65535,
        };
        rig.guest(&mut c, ACK, &[]);
        rig.stream(0).lock().unwrap().inbound.extend(pattern(3000));
        rig.tick(Duration::from_millis(1));
        let segments = rig.tcp_replies();
        assert_eq!(segments[0].data.len(), mss, "options {options:?}");
        assert!(segments.iter().all(|s| s.data.len() <= mss));
        assert_eq!(data_of(&segments), pattern(3000));
    }
}

// ---- Data and closing -------------------------------------------------------------------------

#[test]
fn data_flows_both_ways_and_the_flow_is_freed_after_the_fin_linger() {
    let mut rig = Rig::new();
    let mut c = rig.open();
    let stream = rig.stream(0);

    // Guest to host.
    rig.guest(&mut c, ACK | PSH, b"hello");
    let acks = rig.tcp_replies();
    assert_eq!(acks.len(), 1);
    assert_eq!((acks[0].flags, acks[0].ack), (ACK, c.g_seq));
    assert!(acks[0].data.is_empty());
    assert_eq!(stream.lock().unwrap().written, b"hello");

    // Host to guest.
    stream.lock().unwrap().inbound.extend(b"world");
    rig.tick(Duration::from_millis(1));
    let data = rig.tcp_replies();
    assert_eq!(data.len(), 1);
    assert_eq!(data[0].data, b"world");
    assert_eq!((data[0].seq, data[0].ack), (c.h_next, c.g_seq));
    assert_eq!(data[0].flags, ACK | PSH);
    Rig::absorb(&mut c, &data);
    rig.guest(&mut c, ACK, &[]);
    assert!(rig.replies().is_empty(), "a pure ACK gets no answer");

    // The guest closes first: ACK, then the host side is shut down for writing.
    rig.guest(&mut c, FIN | ACK, &[]);
    let acks = rig.tcp_replies();
    assert_eq!((acks.len(), acks[0].ack), (1, c.g_seq));
    assert!(stream.lock().unwrap().shut_wr);

    // The host closes: FIN to the guest.
    stream.lock().unwrap().eof = true;
    rig.tick(Duration::from_millis(1));
    let fin = rig.tcp_replies();
    assert_eq!(fin.len(), 1);
    assert_eq!(
        (fin[0].flags, fin[0].seq, fin[0].ack),
        (FIN | ACK, c.h_next, c.g_seq)
    );
    Rig::absorb(&mut c, &fin);
    rig.guest(&mut c, ACK, &[]);
    assert!(rig.replies().is_empty());

    // L19: the flow lives 2 s after the exchange.
    let t = rig.now;
    rig.at(t, Duration::from_millis(1900));
    assert_eq!(rig.flows(), 1);
    let mut late_fin = c.clone();
    late_fin.g_seq -= 1;
    rig.push(&late_fin.frame(FIN | ACK, &[]));
    let acks = rig.tcp_replies();
    assert_eq!((acks.len(), acks[0].flags, acks[0].ack), (1, ACK, c.g_seq));
    assert!(!stream.lock().unwrap().closed);
    rig.at(t, Duration::from_secs(2));
    assert_eq!(rig.flows(), 0);
    assert!(stream.lock().unwrap().closed, "the host socket is closed");
    assert_eq!(rig.counters().tcp_idle_expired, 0);

    // A later segment of the guest finds no flow.
    rig.guest(&mut c, ACK, b"x");
    let rst = rig.tcp_replies();
    assert_eq!((rst.len(), rst[0].flags, rst[0].seq), (1, RST, c.h_next));
    assert_eq!(rig.counters().tcp_orphan_rst, 1);
}

#[test]
fn the_host_closing_first_sends_the_data_and_then_the_fin() {
    let mut rig = Rig::new();
    let mut c = rig.open();
    let stream = rig.stream(0);
    {
        let mut s = stream.lock().unwrap();
        s.inbound.extend(b"bye");
        s.eof = true;
    }
    rig.tick(Duration::from_millis(1));
    let out = rig.tcp_replies();
    assert_eq!(out.len(), 2);
    assert_eq!(
        (out[0].data.as_slice(), out[0].flags),
        (&b"bye"[..], ACK | PSH)
    );
    assert_eq!((out[1].flags, out[1].seq), (FIN | ACK, c.h_next + 3));
    Rig::absorb(&mut c, &out);
    rig.guest(&mut c, ACK, &[]);
    assert!(rig.replies().is_empty());
    // Half closed: the guest may still send; then it closes too.
    rig.guest(&mut c, ACK | PSH, b"late");
    assert_eq!(rig.tcp_replies().len(), 1);
    assert_eq!(stream.lock().unwrap().written, b"late");
    rig.guest(&mut c, FIN | ACK, &[]);
    assert_eq!(rig.tcp_replies().len(), 1);
    let t = rig.now;
    rig.at(t, Duration::from_secs(2));
    assert_eq!(rig.flows(), 0);
}

#[test]
fn rst_from_the_guest_closes_the_host_socket_at_once_if_it_is_in_the_window() {
    let mut rig = Rig::new();
    let mut c = rig.open();
    let stream = rig.stream(0);
    stream.lock().unwrap().write_budget = 0;
    rig.guest(&mut c, ACK | PSH, b"pending");
    rig.replies();
    // An RST far outside the window is ignored.
    let mut far = c.clone();
    far.g_seq = c.g_seq.wrapping_add(70_000);
    rig.push(&far.frame(RST, &[]));
    assert_eq!(rig.flows(), 1);
    assert!(!stream.lock().unwrap().closed);
    // An RST at the next expected number ends the flow without an answer.
    rig.push(&c.frame(RST, &[]));
    assert!(rig.replies().is_empty());
    assert_eq!(rig.flows(), 0);
    let s = stream.lock().unwrap();
    assert!(s.closed);
    assert!(s.written.is_empty());
}

#[test]
fn host_errors_on_an_open_connection_reset_the_guest() {
    for fail_read in [true, false] {
        let mut rig = Rig::new();
        let mut c = rig.open();
        let stream = rig.stream(0);
        if fail_read {
            stream.lock().unwrap().read_fails = true;
            rig.tick(Duration::from_millis(1));
        } else {
            stream.lock().unwrap().write_fails = true;
            rig.guest(&mut c, ACK | PSH, b"data");
        }
        let replies = rig.tcp_replies();
        // The write case acknowledges nothing before the host fails: only the RST comes.
        let rst = replies.last().unwrap();
        assert_eq!(rst.flags, RST | ACK, "fail_read={fail_read}");
        assert_eq!(rst.seq, c.h_next);
        assert_eq!(rst.ack, c.g_seq);
        let counters = rig.counters();
        assert_eq!(counters.tcp_host_reset, 1);
        assert_eq!(rig.flows(), 0);
        assert!(stream.lock().unwrap().closed);
    }
}

#[test]
fn segments_out_of_order_duplicated_or_overlapping_are_handled_by_the_stream_rules() {
    let mut rig = Rig::new();
    let mut c = rig.open();
    let stream = rig.stream(0);

    let mut ahead = c.clone();
    ahead.g_seq += 10;
    rig.guest(&mut ahead, ACK | PSH, b"abc");
    let r = rig.tcp_replies();
    assert_eq!(
        (r.len(), r[0].ack),
        (1, c.g_seq),
        "a duplicate ACK asks for the missing bytes"
    );
    assert!(stream.lock().unwrap().written.is_empty());
    assert_eq!(rig.counters().tcp_out_of_order_dropped, 1);

    rig.guest(&mut c, ACK | PSH, b"0123456789");
    assert_eq!(rig.tcp_replies()[0].ack, c.g_seq);
    assert_eq!(stream.lock().unwrap().written, b"0123456789");

    // The same bytes again (a retransmission): acknowledged, not delivered twice.
    let mut dup = c.clone();
    dup.g_seq -= 10;
    rig.guest(&mut dup, ACK | PSH, b"0123456789");
    assert_eq!(rig.tcp_replies()[0].ack, c.g_seq);
    assert_eq!(stream.lock().unwrap().written, b"0123456789");

    // Five old bytes and five new ones: only the new ones are taken.
    let mut overlap = c.clone();
    overlap.g_seq -= 5;
    rig.guest(&mut overlap, ACK | PSH, b"56789ABCDE");
    c.g_seq += 5;
    assert_eq!(rig.tcp_replies()[0].ack, c.g_seq);
    assert_eq!(stream.lock().unwrap().written, b"0123456789ABCDE");
    assert_eq!(rig.counters().tcp_out_of_order_dropped, 1);
}

#[test]
fn odd_segments_inside_a_connection_do_no_harm() {
    let mut rig = Rig::new();
    let mut c = rig.open();
    // Data without the ACK flag is not valid inside a connection: ignored.
    rig.push(&c.frame(0, b"nope"));
    assert!(rig.replies().is_empty());
    assert!(rig.stream(0).lock().unwrap().written.is_empty());
    // A SYN inside the connection gets a challenge ACK and changes nothing.
    rig.push(&c.frame(SYN, &[]));
    let r = rig.tcp_replies();
    assert_eq!((r.len(), r[0].flags, r[0].ack), (1, ACK, c.g_seq));
    // An ACK for bytes never sent: answered with the current numbers, state kept.
    let mut bad = c.clone();
    bad.h_next = c.h_next.wrapping_add(500);
    rig.push(&bad.frame(ACK, &[]));
    let r = rig.tcp_replies();
    assert_eq!((r.len(), r[0].ack), (1, c.g_seq));
    assert_eq!(rig.flows(), 1);
    // The flow still works.
    rig.guest(&mut c, ACK | PSH, b"ok");
    assert_eq!(rig.stream(0).lock().unwrap().written, b"ok");
}

#[test]
fn a_wrong_ack_in_the_handshake_gets_rst_and_the_flow_stays() {
    let mut rig = Rig::new();
    rig.send_syn(SERVER, 40000, 80, 1000, &MSS_1460);
    let synack = rig.tcp_replies().remove(0);
    let wrong = tcp_frame(
        SERVER,
        &Seg {
            seq: 1001,
            ack: synack.seq.wrapping_add(77),
            flags: ACK,
            ..Seg::default()
        },
    );
    rig.push(&wrong);
    let r = rig.tcp_replies();
    assert_eq!(
        (r.len(), r[0].flags, r[0].seq),
        (1, RST, synack.seq.wrapping_add(77))
    );
    assert_eq!(rig.flows(), 1);
    let right = tcp_frame(
        SERVER,
        &Seg {
            seq: 1001,
            ack: synack.seq.wrapping_add(1),
            flags: ACK,
            ..Seg::default()
        },
    );
    rig.push(&right);
    rig.stream(0).lock().unwrap().inbound.extend(b"up");
    rig.tick(Duration::from_millis(1));
    assert_eq!(data_of(&rig.tcp_replies()), b"up");
}

#[test]
fn a_new_syn_replaces_a_flow_that_only_waits_out_the_linger() {
    let mut rig = Rig::new();
    let mut c = rig.open();
    rig.guest(&mut c, FIN | ACK, &[]);
    rig.stream(0).lock().unwrap().eof = true;
    rig.tick(Duration::from_millis(1));
    let fin = rig.tcp_replies();
    Rig::absorb(&mut c, &fin[fin.len() - 1..]);
    rig.guest(&mut c, ACK, &[]);
    rig.replies();
    // The guest reuses the port at once with another initial number.
    rig.send_syn(SERVER, 40000, 80, 5000, &MSS_1460);
    let synack = rig.tcp_replies();
    assert_eq!(
        (synack.len(), synack[0].flags, synack[0].ack),
        (1, SYN | ACK, 5001)
    );
    assert_eq!(rig.connects(), 2);
    assert!(rig.stream(0).lock().unwrap().closed);
    assert_eq!(rig.flows(), 1);
}

// ---- Failures of connect (table of NETWORK.md section 11) -------------------------------------

fn failure_case(error: HostError, late: bool) {
    let mut rig = Rig::new();
    let syn = tcp_frame(
        SERVER,
        &Seg {
            seq: 1000,
            flags: SYN,
            options: &MSS_1460,
            ..Seg::default()
        },
    );
    if late {
        rig.host.lock().unwrap().initial = Connect::Pending;
    } else {
        rig.host.lock().unwrap().immediate_error = Some(error);
    }
    rig.push(&syn);
    if late {
        assert!(rig.replies().is_empty());
        rig.stream(0).lock().unwrap().connect = Connect::Failed(error);
        rig.tick(Duration::from_millis(1));
    }
    let replies = rig.replies();
    let c = rig.counters();
    let what = format!("{error:?} late={late}");
    match error {
        HostError::Refused | HostError::Other => {
            assert_eq!(replies.len(), 1, "{what}");
            let Reply::Tcp(rst) = &replies[0] else {
                panic!("{what}: {replies:?}")
            };
            assert_eq!(
                (rst.flags, rst.seq, rst.ack),
                (RST | ACK, 0, 1001),
                "{what}"
            );
            assert_eq!(
                (rst.src_ip, rst.src_port, rst.dst_port),
                (SERVER, 80, 40000)
            );
            let counter = if error == HostError::Refused {
                c.tcp_refused
            } else {
                c.tcp_connect_error
            };
            assert_eq!(counter, 1, "{what}");
        }
        HostError::NetUnreachable | HostError::HostUnreachable => {
            assert_eq!(replies.len(), 1, "{what}");
            let Reply::Unreachable {
                src_ip,
                code,
                quoted,
            } = &replies[0]
            else {
                panic!("{what}: {replies:?}")
            };
            assert_eq!(*src_ip, GATEWAY_IP);
            assert_eq!(*code, u8::from(error == HostError::HostUnreachable));
            // The IP header of the SYN and the first 8 bytes of its data (RFC 792).
            assert_eq!(quoted.as_slice(), &syn[14..42], "{what}");
            assert_eq!((c.tcp_unreachable, c.icmp_error_sent), (1, 1), "{what}");
        }
        HostError::Resource => {
            assert!(replies.is_empty(), "{what}");
            assert_eq!(c.drop_host_resource, 1, "{what}");
        }
        HostError::TimedOut => {
            assert!(replies.is_empty(), "{what}");
            assert_eq!(c.tcp_connect_timeout, 1, "{what}");
        }
    }
    assert_eq!(rig.flows(), 0, "{what}");
    assert_eq!(rig.b.tcp.flow_count(), 0, "{what}");
    assert_eq!(c.tcp_opened, u64::from(late), "{what}");
    if late {
        assert!(rig.stream(0).lock().unwrap().closed, "{what}");
    }
}

#[test]
fn every_connect_error_shows_the_guest_what_the_table_says() {
    for error in [
        HostError::Refused,
        HostError::NetUnreachable,
        HostError::HostUnreachable,
        HostError::Resource,
        HostError::TimedOut,
        HostError::Other,
    ] {
        failure_case(error, false);
        failure_case(error, true);
    }
}

#[test]
fn icmp_errors_to_the_guest_are_limited_to_50_per_second() {
    let mut rig = Rig::new();
    for n in 0..51u16 {
        rig.host.lock().unwrap().immediate_error = Some(HostError::NetUnreachable);
        rig.send_syn(SERVER, 41000 + n, 80, 1000, &MSS_1460);
    }
    assert_eq!(rig.replies().len(), 50);
    let c = rig.counters();
    assert_eq!(
        (
            c.tcp_unreachable,
            c.icmp_error_sent,
            c.icmp_error_suppressed
        ),
        (51, 50, 1)
    );
    // One second later the bucket is full again.
    rig.tick(Duration::from_secs(1));
    rig.host.lock().unwrap().immediate_error = Some(HostError::NetUnreachable);
    rig.send_syn(SERVER, 42000, 80, 1000, &MSS_1460);
    assert_eq!(rig.replies().len(), 1);
    assert_eq!(rig.counters().icmp_error_suppressed, 1);
}

#[test]
fn the_connect_gets_30_seconds_and_then_the_flow_goes_without_a_word() {
    let mut rig = Rig::new();
    rig.host.lock().unwrap().initial = Connect::Pending;
    rig.send_syn(SERVER, 40000, 80, 1000, &MSS_1460);
    let t = rig.now;
    rig.at(t, Duration::from_millis(29_999));
    assert_eq!(rig.flows(), 1);
    assert_eq!(rig.counters().tcp_connect_timeout, 0);
    rig.at(t, Duration::from_secs(30));
    assert_eq!(rig.flows(), 0);
    assert_eq!(rig.counters().tcp_connect_timeout, 1);
    assert!(rig.replies().is_empty());
    assert!(rig.stream(0).lock().unwrap().closed);
    // The repeated SYN of the guest starts over.
    rig.send_syn(SERVER, 40000, 80, 1000, &MSS_1460);
    assert_eq!(rig.connects(), 2);
}

// ---- Segments without a flow, the backend's own addresses, limits -----------------------------

#[test]
fn tcp_to_the_gateway_and_dns_addresses_is_refused_for_every_port() {
    let mut rig = Rig::new();
    let mut expected = 0;
    for dst in [GATEWAY_IP, DNS_IP] {
        for port in [1, 53, 67, 80, 65535] {
            let frame = tcp_frame(
                dst,
                &Seg {
                    dst_port: port,
                    seq: 1000,
                    flags: SYN,
                    options: &MSS_1460,
                    ..Seg::default()
                },
            );
            rig.push(&frame);
            let r = rig.tcp_replies();
            assert_eq!(r.len(), 1);
            assert_eq!((r[0].flags, r[0].seq, r[0].ack), (RST | ACK, 0, 1001));
            assert_eq!((r[0].src_ip, r[0].src_port), (dst, port));
            expected += 1;
            assert_eq!(rig.counters().tcp_refused, expected);
        }
        // An RST from the guest to these addresses is not answered.
        let rst = tcp_frame(
            dst,
            &Seg {
                seq: 5,
                flags: RST,
                ..Seg::default()
            },
        );
        rig.push(&rst);
        assert!(rig.replies().is_empty());
    }
    assert_eq!((rig.connects(), rig.flows()), (0, 0));
    assert_eq!(rig.counters().tcp_refused, expected);
}

#[test]
fn a_segment_without_a_flow_gets_rst_unless_it_is_an_rst_or_a_plain_syn() {
    // (flags, seq, ack, data length) and the RST expected: (seq, ack, flags).
    let cases = [
        (ACK, 500, 777, 0, (777, 0, RST)),
        (ACK | PSH, 500, 9, 5, (9, 0, RST)),
        (0, 500, 0, 3, (0, 503, RST | ACK)),
        (FIN, 500, 0, 0, (0, 501, RST | ACK)),
        (SYN | ACK, 500, 5, 0, (5, 0, RST)),
        (SYN | FIN, 500, 0, 0, (0, 502, RST | ACK)),
    ];
    let mut rig = Rig::new();
    for (n, (flags, seq, ack, len, (rst_seq, rst_ack, rst_flags))) in cases.into_iter().enumerate()
    {
        let data = vec![0u8; len];
        let frame = tcp_frame(
            SERVER,
            &Seg {
                seq,
                ack,
                flags,
                data: &data,
                ..Seg::default()
            },
        );
        rig.push(&frame);
        let r = rig.tcp_replies();
        assert_eq!(r.len(), 1, "case {n}");
        assert_eq!(
            (r[0].seq, r[0].ack, r[0].flags),
            (rst_seq, rst_ack, rst_flags),
            "case {n}"
        );
        assert_eq!(
            (r[0].src_ip, r[0].src_port, r[0].dst_port),
            (SERVER, 80, 40000)
        );
        assert_eq!(r[0].window, 0);
    }
    assert_eq!(rig.counters().tcp_orphan_rst, 6);
    let rst = tcp_frame(
        SERVER,
        &Seg {
            seq: 1,
            flags: RST,
            ..Seg::default()
        },
    );
    rig.push(&rst);
    assert!(rig.replies().is_empty());
    let c = rig.counters();
    assert_eq!((c.tcp_orphan_rst, c.tcp_refused, c.tcp_segments), (6, 0, 7));
    assert_eq!(rig.connects(), 0, "no socket for any of them");
}

#[test]
fn flow_limit_l4_refuses_the_129th_syn_before_any_socket_exists() {
    let mut rig = Rig::new();
    rig.host.lock().unwrap().initial = Connect::Pending;
    for n in 0..MAX_TCP_FLOWS as u16 {
        rig.send_syn(SERVER, 10000 + n, 80, 1000, &MSS_1460);
    }
    assert_eq!(
        (rig.connects(), rig.flows()),
        (MAX_TCP_FLOWS, MAX_TCP_FLOWS)
    );
    assert_eq!(rig.counters().drop_flow_limit_tcp, 0);
    rig.send_syn(SERVER, 20000, 80, 1000, &MSS_1460);
    assert_eq!(rig.counters().drop_flow_limit_tcp, 1);
    assert_eq!(
        (rig.connects(), rig.flows()),
        (MAX_TCP_FLOWS, MAX_TCP_FLOWS)
    );
    assert!(rig.replies().is_empty(), "the guest repeats the SYN itself");
    // A freed slot is used at once; existing flows were not evicted.
    let rst = tcp_frame(
        SERVER,
        &Seg {
            guest_port: 10000,
            seq: 1001,
            flags: RST,
            ..Seg::default()
        },
    );
    rig.push(&rst);
    assert_eq!(rig.flows(), MAX_TCP_FLOWS - 1);
    rig.send_syn(SERVER, 20001, 80, 1000, &MSS_1460);
    assert_eq!(
        (rig.connects(), rig.flows()),
        (MAX_TCP_FLOWS + 1, MAX_TCP_FLOWS)
    );
    rig.send_syn(SERVER, 20002, 80, 1000, &MSS_1460);
    assert_eq!(rig.counters().drop_flow_limit_tcp, 2);
}

#[test]
fn reset_closes_every_socket_and_forgets_the_flows_but_keeps_the_counters() {
    let mut rig = Rig::new();
    let mut c = rig.open();
    NetBackend::reset(&mut rig.b);
    assert_eq!(rig.flows(), 0);
    assert_eq!(rig.b.tcp.flow_count(), 0);
    assert!(rig.stream(0).lock().unwrap().closed);
    let counters = rig.counters();
    assert_eq!((counters.resets, counters.tcp_opened), (1, 1));
    // Nothing was said to the guest; its next segment finds no flow.
    assert!(rig.replies().is_empty());
    rig.guest(&mut c, ACK, b"x");
    assert_eq!(rig.counters().tcp_orphan_rst, 1);
}

#[test]
fn broken_tcp_segments_are_dropped_before_any_socket_or_flow() {
    let mut rig = Rig::new();
    let syn = || {
        tcp_frame(
            SERVER,
            &Seg {
                seq: 1000,
                flags: SYN,
                options: &MSS_1460,
                ..Seg::default()
            },
        )
    };
    let mut bad_sum = syn();
    bad_sum[L4 + 16] ^= 1;
    let mut short_offset = syn();
    short_offset[L4 + 12] = 0x40;
    fix_l4_checksum(&mut short_offset);
    let mut long_offset = syn();
    long_offset[L4 + 12] = 0xF0;
    fix_l4_checksum(&mut long_offset);
    let truncated = ipv4_frame(&Ip::to(SERVER, PROTO_TCP), &[0u8; 10]);
    for frame in [bad_sum, short_offset, long_offset, truncated] {
        rig.push(&frame);
    }
    let c = rig.counters();
    assert_eq!(c.drop_l4_malformed, 4);
    assert_eq!((c.tcp_segments, c.tcp_opened), (0, 0));
    assert_eq!((rig.connects(), rig.flows()), (0, 0));
    assert!(rig.replies().is_empty());
}

// ---- Buffers, windows, retransmission ---------------------------------------------------------

#[test]
fn guest_to_host_buffer_l7_closes_the_window_and_one_more_byte_is_not_taken() {
    let mut rig = Rig::new();
    let mut c = rig.open();
    let stream = rig.stream(0);
    stream.lock().unwrap().write_budget = 0;
    let data = pattern(65_536);
    for (i, chunk) in data.chunks(1024).enumerate() {
        rig.guest(&mut c, ACK | PSH, chunk);
        let r = rig.tcp_replies();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].ack, c.g_seq);
        let free = 65_536 - (i + 1) * 1024;
        assert_eq!(
            usize::from(r[0].window),
            free.min(65_535),
            "after chunk {i}"
        );
    }
    assert_eq!(rig.counters().tcp_zero_window, 1);

    // One byte above the limit: not taken, the window stays 0.
    rig.guest(&mut c, ACK | PSH, &[0xEE]);
    c.g_seq -= 1;
    let r = rig.tcp_replies();
    assert_eq!((r.len(), r[0].ack, r[0].window), (1, c.g_seq, 0));
    assert!(stream.lock().unwrap().written.is_empty());
    assert_eq!(rig.counters().tcp_zero_window, 1);

    // The host takes the data: everything arrives in order and the guest is told the window is open.
    stream.lock().unwrap().write_budget = usize::MAX;
    rig.tick(Duration::from_millis(1));
    let r = rig.tcp_replies();
    assert_eq!((r.len(), r[0].ack), (1, c.g_seq));
    assert_eq!(r[0].window, 65_535);
    assert_eq!(stream.lock().unwrap().written, data);
    // The guest sends the byte again and it is taken.
    rig.guest(&mut c, ACK | PSH, &[0xEE]);
    assert_eq!(rig.tcp_replies()[0].ack, c.g_seq);
    let s = stream.lock().unwrap();
    assert_eq!(s.written.len(), 65_537);
    assert_eq!(s.written[65_536], 0xEE);
}

#[test]
fn host_to_guest_buffer_l7_holds_64_kib_and_the_host_is_not_read_beyond_it() {
    let mut rig = Rig::new();
    let mut c = rig.open();
    let stream = rig.stream(0);
    let total = pattern(200_000);
    stream.lock().unwrap().inbound.extend(&total);
    rig.tick(Duration::from_millis(1));
    let first = rig.tcp_replies();
    // The guest window (65 535) bounds what is sent; the buffer (65 536) bounds what is read.
    assert_eq!(data_of(&first).len(), 65_535);
    assert!(first.iter().all(|s| s.data.len() <= 1460));
    assert_eq!(stream.lock().unwrap().read_total, 65_536);
    rig.tick(Duration::from_millis(100));
    assert!(rig.replies().is_empty());
    assert_eq!(
        stream.lock().unwrap().read_total,
        65_536,
        "nothing is read while the buffer is full"
    );

    // The guest acknowledges; the rest follows, in order, never more than a buffer ahead.
    let base = c.h_next;
    let mut received = data_of(&first);
    Rig::absorb(&mut c, &first);
    for _ in 0..40 {
        if received.len() == total.len() {
            break;
        }
        rig.guest(&mut c, ACK, &[]);
        let r = rig.tcp_replies();
        let acked = c.h_next.wrapping_sub(base) as usize;
        assert!(stream.lock().unwrap().read_total - acked <= 65_536);
        for s in &r {
            assert_eq!(s.seq, base.wrapping_add(received.len() as u32));
            received.extend_from_slice(&s.data);
        }
        Rig::absorb(&mut c, &r);
    }
    assert_eq!(received, total);
}

#[test]
fn nothing_is_read_from_the_host_while_the_l3_queue_is_full() {
    let mut rig = Rig::new();
    let mut c = rig.open();
    let stream = rig.stream(0);
    for _ in 0..256 {
        rig.push(&arp_request_for_gateway());
    }
    assert_eq!(rig.counters().drop_rx_queue_full, 0);
    stream.lock().unwrap().inbound.extend(pattern(5000));
    rig.tick(Duration::from_millis(10));
    assert_eq!(stream.lock().unwrap().read_total, 0);
    assert_eq!(
        rig.counters().drop_rx_queue_full,
        0,
        "TCP waits instead of losing data"
    );
    let mut buf = [0u8; NET_MAX_FRAME];
    for _ in 0..256 {
        assert!(rig.b.poll_receive(&mut buf).is_some());
    }
    rig.tick(Duration::from_millis(10));
    let r = rig.tcp_replies();
    assert_eq!(data_of(&r), pattern(5000));
    Rig::absorb(&mut c, &r);
}

#[test]
fn retransmission_follows_the_policy_and_gives_up_after_39_seconds() {
    let mut rig = Rig::new();
    let c = rig.open();
    let stream = rig.stream(0);
    stream.lock().unwrap().inbound.extend(b"0123456789");
    rig.tick(Duration::from_millis(1));
    let t = rig.now;
    let first = rig.tcp_replies();
    assert_eq!(first.len(), 1);
    let ms = Duration::from_millis;
    for (n, at) in [1u64, 3, 7, 15, 23, 31].into_iter().enumerate() {
        rig.at(t, ms(at * 1000 - 1));
        assert!(rig.replies().is_empty(), "no retransmission before {at} s");
        rig.at(t, ms(at * 1000));
        let again = rig.tcp_replies();
        assert_eq!(again.len(), 1, "retransmission at {at} s");
        assert_eq!(
            (again[0].seq, &again[0].data),
            (first[0].seq, &first[0].data)
        );
        assert_eq!(rig.counters().tcp_retransmits, n as u64 + 1);
    }
    rig.at(t, ms(38_999));
    assert!(rig.replies().is_empty());
    assert_eq!(rig.flows(), 1);
    assert_eq!(rig.counters().tcp_giveups, 0);
    rig.at(t, ms(39_000));
    let rst = rig.tcp_replies();
    assert_eq!(rst.len(), 1);
    assert_eq!(
        (rst[0].flags, rst[0].seq, rst[0].ack),
        (RST | ACK, first[0].seq + 10, c.g_seq)
    );
    let counters = rig.counters();
    assert_eq!((counters.tcp_giveups, counters.tcp_retransmits), (1, 6));
    assert_eq!(rig.flows(), 0);
    assert!(stream.lock().unwrap().closed);
}

#[test]
fn an_ack_brings_the_timeout_back_to_one_second() {
    let mut rig = Rig::new();
    let mut c = rig.open();
    let stream = rig.stream(0);
    stream.lock().unwrap().inbound.extend(b"AAAA");
    rig.tick(Duration::from_millis(1));
    let t = rig.now;
    let first = rig.tcp_replies();
    rig.at(t, Duration::from_secs(1));
    let mut retransmitted = rig.tcp_replies().len();
    rig.at(t, Duration::from_secs(3));
    retransmitted += rig.tcp_replies().len();
    assert_eq!(retransmitted, 2, "two retransmissions, RTO now 4 s");
    Rig::absorb(&mut c, &first);
    rig.guest(&mut c, ACK, &[]);
    assert!(rig.replies().is_empty());
    stream.lock().unwrap().inbound.extend(b"BBBB");
    rig.tick(Duration::ZERO);
    let t2 = rig.now;
    assert_eq!(data_of(&rig.tcp_replies()), b"BBBB");
    rig.at(t2, Duration::from_millis(999));
    assert!(rig.replies().is_empty());
    rig.at(t2, Duration::from_secs(1));
    assert_eq!(
        data_of(&rig.tcp_replies()),
        b"BBBB",
        "RTO is 1 s again, not 8 s"
    );
    assert_eq!(rig.counters().tcp_retransmits, 3);
}

#[test]
fn the_retransmission_timer_waits_while_the_nic_does_not_take_frames() {
    let mut rig = Rig::new();
    rig.open();
    rig.stream(0).lock().unwrap().inbound.extend(pattern(100));
    rig.tick(Duration::from_millis(1));
    let t = rig.now;
    // The frame stays in the L3 queue: nobody polls.
    for secs in [1u64, 2, 5, 10] {
        rig.at(t, Duration::from_secs(secs));
    }
    let counters = rig.counters();
    assert_eq!((counters.tcp_retransmits, counters.tcp_giveups), (0, 0));
    assert_eq!(rig.flows(), 1);
    // The NIC resumes: the timer runs again from the last turn (10 s + RTO 1 s).
    let frames = rig.tcp_replies();
    assert_eq!(frames.len(), 1);
    rig.at(t, Duration::from_millis(10_500));
    assert!(rig.replies().is_empty());
    rig.at(t, Duration::from_secs(11));
    let again = rig.tcp_replies();
    assert_eq!((again.len(), again[0].seq), (1, frames[0].seq));
    assert_eq!(rig.counters().tcp_retransmits, 1);
}

#[test]
fn a_closed_guest_window_is_probed_and_the_data_follows_when_it_opens() {
    let mut rig = Rig::new();
    let mut c = rig.open();
    c.window = 0;
    rig.guest(&mut c, ACK, &[]);
    rig.stream(0).lock().unwrap().inbound.extend(pattern(100));
    rig.tick(Duration::from_millis(1));
    let t = rig.now;
    assert!(rig.replies().is_empty(), "no data into a zero window");
    rig.at(t, Duration::from_millis(999));
    assert!(rig.replies().is_empty());
    rig.at(t, Duration::from_secs(1));
    let probe = rig.tcp_replies();
    assert_eq!(probe.len(), 1);
    assert_eq!(
        (probe[0].flags, probe[0].seq, probe[0].data.len()),
        (ACK, c.h_next - 1, 0)
    );
    rig.at(t, Duration::from_millis(2999));
    assert!(rig.replies().is_empty(), "the interval doubles");
    rig.at(t, Duration::from_secs(3));
    assert_eq!(rig.tcp_replies().len(), 1);
    assert_eq!(rig.counters().tcp_giveups, 0);
    c.window = 65_535;
    rig.guest(&mut c, ACK, &[]);
    assert_eq!(data_of(&rig.tcp_replies()), pattern(100));
}

// ---- Idle timeouts ----------------------------------------------------------------------------

#[test]
fn idle_limits_l11_for_established_half_closed_and_unfinished_flows() {
    let ms = Duration::from_millis;
    // Established: 7 440 s.
    let mut rig = Rig::new();
    rig.open();
    let t = rig.now;
    rig.at(t, ms(7_439_999));
    assert_eq!((rig.flows(), rig.counters().tcp_idle_expired), (1, 0));
    rig.at(t, ms(7_440_000));
    assert_eq!((rig.flows(), rig.counters().tcp_idle_expired), (0, 1));
    assert!(
        rig.replies().is_empty(),
        "an expired flow is dropped without a word"
    );
    assert!(rig.stream(0).lock().unwrap().closed);

    // Half-closed (the guest sent FIN): 240 s.
    let mut rig = Rig::new();
    let mut c = rig.open();
    rig.guest(&mut c, FIN | ACK, &[]);
    rig.replies();
    let t = rig.now;
    rig.at(t, ms(239_999));
    assert_eq!(rig.flows(), 1);
    rig.at(t, ms(240_000));
    assert_eq!((rig.flows(), rig.counters().tcp_idle_expired), (0, 1));

    // SYN-ACK sent, ACK never received: 240 s.
    let mut rig = Rig::new();
    rig.send_syn(SERVER, 40000, 80, 1000, &MSS_1460);
    rig.replies();
    let t = rig.now;
    rig.at(t, ms(239_999));
    assert_eq!(rig.flows(), 1);
    rig.at(t, ms(240_000));
    assert_eq!((rig.flows(), rig.counters().tcp_idle_expired), (0, 1));

    // Traffic in either direction postpones the expiry.
    let mut rig = Rig::new();
    let mut c = rig.open();
    let t = rig.now;
    rig.at(t, Duration::from_secs(7_000));
    rig.guest(&mut c, ACK | PSH, b"x");
    rig.replies();
    rig.at(t, Duration::from_secs(7_000 + 7_439));
    assert_eq!(rig.flows(), 1);
}

// ---- Real sockets on the loopback -------------------------------------------------------------

fn loopback_backend() -> NatBackend {
    NatBackend::with_policy(Policy::new(GUEST_MAC).unwrap().allow_loopback_for_tests())
}

/// Runs the backend on the real clock until `done` holds for the segments received so far.
fn run_until(b: &mut NatBackend, got: &mut Vec<Tcp>, done: impl Fn(&[Tcp]) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut buf = [0u8; NET_MAX_FRAME];
    loop {
        b.run_at(Instant::now());
        while let Some(len) = b.poll_receive(&mut buf) {
            match parse_reply(&buf[..len]) {
                Reply::Tcp(t) => got.push(t),
                other => panic!("{other:?}"),
            }
        }
        if done(got) {
            return;
        }
        assert!(Instant::now() < deadline, "timeout, got {got:?}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn loopback_frame(port: u16, seq: u32, ack: u32, flags: u8, data: &[u8]) -> Vec<u8> {
    tcp_frame(
        Ipv4Addr::LOCALHOST,
        &Seg {
            dst_port: port,
            seq,
            ack,
            flags,
            options: if flags == SYN { &MSS_1460 } else { &[] },
            data,
            ..Seg::default()
        },
    )
}

#[test]
fn a_real_server_on_the_loopback_gets_the_data_echoes_it_and_sees_the_fin() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut seen = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            seen.extend_from_slice(&buf[..n]);
            stream.write_all(&buf[..n]).unwrap();
        }
        seen
    });
    let mut b = loopback_backend();
    let mut got = Vec::new();
    b.transmit(&loopback_frame(port, 1000, 0, SYN, &[]))
        .unwrap();
    run_until(&mut b, &mut got, |g| g.iter().any(|t| t.flags == SYN | ACK));
    let synack = got.remove(0);
    assert_eq!(
        (synack.ack, synack.mss, synack.src_ip),
        (1001, Some(1460), Ipv4Addr::LOCALHOST)
    );
    let h_start = synack.seq + 1;
    b.transmit(&loopback_frame(port, 1001, h_start, ACK, &[]))
        .unwrap();
    b.transmit(&loopback_frame(
        port,
        1001,
        h_start,
        ACK | PSH,
        b"ping pong",
    ))
    .unwrap();
    run_until(&mut b, &mut got, |g| data_of(g).len() >= 9);
    assert_eq!(data_of(&got), b"ping pong");
    got.clear();
    b.transmit(&loopback_frame(port, 1010, h_start + 9, FIN | ACK, &[]))
        .unwrap();
    run_until(&mut b, &mut got, |g| g.iter().any(|t| t.flags & FIN != 0));
    let fin = got.iter().find(|t| t.flags & FIN != 0).unwrap();
    assert_eq!((fin.seq, fin.ack), (h_start + 9, 1011));
    b.transmit(&loopback_frame(port, 1011, h_start + 10, ACK, &[]))
        .unwrap();
    assert_eq!(server.join().unwrap(), b"ping pong");
    let c = b.counters();
    assert_eq!((c.tcp_opened, c.tcp_refused, c.tcp_host_reset), (1, 0, 0));
}

#[test]
fn a_real_closed_port_on_the_loopback_is_refused_with_rst() {
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let mut b = loopback_backend();
    let mut got = Vec::new();
    b.transmit(&loopback_frame(port, 1000, 0, SYN, &[]))
        .unwrap();
    run_until(&mut b, &mut got, |g| !g.is_empty());
    assert_eq!((got[0].flags, got[0].seq, got[0].ack), (RST | ACK, 0, 1001));
    assert_eq!(b.counters().tcp_refused, 1);
    assert_eq!(b.flows.count(FlowProto::Tcp), 0);
}

// ---- Accounting under garbage -----------------------------------------------------------------

#[test]
fn mutated_tcp_conversations_never_panic_leak_a_socket_or_lose_count_of_a_frame() {
    let mut rig = Rig::new();
    let mut rng = Rng::new(0x7C9_5EED);
    let conv = |flags: u8, seq: u32, data: &[u8], port: u16| {
        tcp_frame(
            SERVER,
            &Seg {
                guest_port: port,
                seq,
                ack: 0x4F44_0001,
                flags,
                options: if flags == SYN { &MSS_1460 } else { &[] },
                data,
                ..Seg::default()
            },
        )
    };
    let mut bases = Vec::new();
    for port in [40000u16, 40001, 40002] {
        bases.push(conv(SYN, 1000, &[], port));
        bases.push(conv(ACK, 1001, &[], port));
        bases.push(conv(ACK | PSH, 1001, b"some data here", port));
        bases.push(conv(ACK | PSH, 1015, &pattern(1460), port));
        bases.push(conv(FIN | ACK, 1015, &[], port));
        bases.push(conv(RST, 1001, &[], port));
        bases.push(conv(SYN | FIN, 1000, &[], port));
        bases.push(conv(0, 1000, b"x", port));
    }
    let mut calls = 0u64;
    for _ in 0..30_000 {
        let mut frame = bases[rng.below(bases.len())].clone();
        if rng.below(4) == 0 {
            let at = rng.below(frame.len());
            frame[at] = rng.next() as u8;
            fix_ip_checksum(&mut frame);
            if rng.below(2) == 0 {
                fix_l4_checksum(&mut frame);
            }
        }
        {
            let mut host = rig.host.lock().unwrap();
            host.initial = match rng.below(6) {
                0 => Connect::Pending,
                1 => Connect::Failed(HostError::Refused),
                2 => Connect::Failed(HostError::NetUnreachable),
                _ => Connect::Done,
            };
            if rng.below(16) == 0 {
                host.immediate_error = Some(HostError::Resource);
            }
            let n = host.streams.len();
            if n > 0 && rng.below(3) == 0 {
                let mut s = host.streams[rng.below(n)].1.lock().unwrap();
                match rng.below(6) {
                    0 => s.eof = true,
                    1 => s.read_fails = true,
                    2 => s.write_fails = true,
                    3 => s.write_budget = 0,
                    4 => s.write_budget = usize::MAX,
                    _ => s.inbound.extend(pattern(rng.below(5000))),
                }
            }
        }
        // Mostly short steps, sometimes long ones that fire the timers.
        rig.now += Duration::from_millis(match rng.below(10) {
            0 => rng.below(60_000) as u64,
            _ => rng.below(30) as u64,
        });
        calls += 1;
        let _ = rig.b.transmit(&frame);
        rig.b.run_at(rig.now);
        let mut buf = [0u8; NET_MAX_FRAME];
        while let Some(len) = rig.b.poll_receive(&mut buf) {
            let _ = parse_reply(&buf[..len]);
        }
        // Flows in the table, flows in the handler and open host sockets are the same set.
        let open_sockets = rig
            .host
            .lock()
            .unwrap()
            .streams
            .iter()
            .filter(|(_, s)| !s.lock().unwrap().closed)
            .count();
        assert_eq!(rig.flows(), rig.b.tcp.flow_count());
        assert_eq!(rig.flows(), open_sockets);
        assert!(rig.flows() <= MAX_TCP_FLOWS);
    }
    let c = rig.counters();
    assert_eq!(
        calls,
        c.tx_frames + c.drop_runt + c.drop_oversize + c.drop_tx_queue_full
    );
    assert_eq!(
        c.drop_rx_queue_full, 0,
        "the queue is emptied after every frame"
    );
    assert_eq!(c.tx_frames, super::tests::total_accounted(&c));
    assert!(
        c.tcp_segments > 1000 && c.tcp_opened > 100,
        "the stream really reached TCP: {c:?}"
    );
}
