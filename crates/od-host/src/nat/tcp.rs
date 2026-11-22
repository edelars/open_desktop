//! TCP of the NAT backend (S11-31d, NETWORK.md sections 11 and 12).
//!
//! The backend ends the guest's TCP on its own side and opens a separate connection from the host to
//! the destination (a proxy at connection level, not packet translation). A SYN from the guest creates
//! a flow and starts a non-blocking host connect; the SYN-ACK goes to the guest when the connect
//! completes. Data moves in both directions through two buffers of 64 KiB per flow (L7); the window
//! the backend advertises is the free space of the guest-to-host buffer, so back pressure reaches the
//! guest. Segments to the guest are retransmitted by the L10 policy; the flow is removed on FIN/RST,
//! on a host error, on a timeout (L9, L11, L19) or when the guest stops acknowledging (L10).
//!
//! The profile is deliberately cut down (NETWORK.md section 11): MSS 1460, window 65 535 without
//! scaling, no SACK, no timestamps, no ECN, no reassembly of out-of-order data (the guest repeats it
//! after our ACK), no congestion window (the guest window and the buffers bound the data in flight),
//! no round-trip estimation (the retransmission timeout is fixed by the policy). Choices the document
//! leaves open and this code makes:
//!
//! - After a timeout the unacknowledged data is sent again from the oldest byte (go back N); the first
//!   segment is sent even when the guest window has shrunk meanwhile.
//! - A zero guest window with data waiting and nothing in flight starts a probe (an empty ACK one byte
//!   behind the sequence number) with the interval doubling from 1 s to 8 s; it never gives up
//!   (L10 is about unacknowledged segments, and a guest may keep a window closed for long).
//! - The initial sequence number advances by a fixed odd step per connection (no randomness: the
//!   guest is the only peer and sees no other flows).
//! - A segment without a flow gets RST unless it is a plain SYN (opens a flow) or carries RST
//!   (ignored). A plain SYN with another sequence number than the old flow, while that flow waits
//!   out the FIN linger (L19), replaces it.
//!
//! No socket is opened before the flow limit and the address policy (frame.rs) have passed, and no
//! listening socket is ever opened.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddrV4;
use std::time::{Duration, Instant};

use super::bump;
use super::ctx::Ctx;
use super::flow::{FlowError, FlowKey, FlowProto};
use super::frame::{
    DstClass, GUEST_IP, Ipv4Packet, PROTO_TCP, TcpOut, TcpSegment, build_tcp_segment,
};

/// L8: the MSS the backend announces and the largest segment it sends.
pub const TCP_MSS: u16 = 1460;
/// L8: the largest window the backend announces (no window scaling).
pub const TCP_WINDOW: u16 = 65_535;
/// L7: the buffer of each direction of a flow.
pub const TCP_BUFFER: usize = 65_536;
/// L9: how long a host connect may take.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// L10: the first retransmission timeout, its ceiling, and the number of retransmissions of the
/// SYN-ACK, data and FIN.
pub const RTO_INITIAL: Duration = Duration::from_secs(1);
pub const RTO_MAX: Duration = Duration::from_secs(8);
pub const MAX_RETRANSMITS: u8 = 6;
/// L11: idle limits of an established flow (also with one FIN) and of an opening or closing one
/// (RFC 5382, REQ-5: 2 h 4 min established, 4 min transitory).
pub const IDLE_ESTABLISHED: Duration = Duration::from_secs(7_440);
pub const IDLE_TRANSITORY: Duration = Duration::from_secs(240);
/// L19: how long a flow stays after the FIN exchange.
pub const FIN_LINGER: Duration = Duration::from_secs(2);
/// RFC 6691: the MSS of a peer that announces none.
const DEFAULT_GUEST_MSS: u16 = 536;
/// The step of the initial sequence number between connections.
const ISN_STEP: u32 = 64_001;

const FIN: u8 = 0x01;
const SYN: u8 = 0x02;
const RST: u8 = 0x04;
const PSH: u8 = 0x08;
const ACK: u8 = 0x10;

// ---- The host side ---------------------------------------------------------------------------

/// Why the host refused or lost a connection; the class decides what the guest sees (NETWORK.md
/// section 11, table "Ошибка connect").
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostError {
    /// `ECONNREFUSED`.
    Refused,
    /// `ENETUNREACH`, `ENETDOWN`.
    NetUnreachable,
    /// `EHOSTUNREACH`, `EHOSTDOWN`.
    HostUnreachable,
    /// `EMFILE`, `ENFILE`, `ENOBUFS`, `ENOMEM`.
    Resource,
    /// The host gave up on the connect itself (`ETIMEDOUT`).
    TimedOut,
    /// Anything else (`EACCES`, `EPERM`, ...).
    Other,
}

/// Result of one non-blocking read or write on a host connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IoResult {
    Ready(usize),
    WouldBlock,
    /// The peer closed its side (read only).
    Eof,
    /// Any other error on an established connection (`ECONNRESET`, `EPIPE`, ...): the guest gets RST.
    Failed,
}

/// One host connection. Dropping it closes the socket. No call blocks.
pub trait HostStream: Send {
    /// `Ok(true)` when the connect has finished, `Ok(false)` while it is in progress.
    fn poll_connected(&mut self) -> Result<bool, HostError>;
    fn read(&mut self, buf: &mut [u8]) -> IoResult;
    fn write(&mut self, buf: &[u8]) -> IoResult;
    /// `shutdown(SHUT_WR)`; errors surface in the next read or write.
    fn shutdown_write(&mut self);
}

/// The way out to the host network: starts a non-blocking connect.
pub trait TcpHost: Send {
    fn connect(&mut self, dst: SocketAddrV4) -> Result<Box<dyn HostStream>, HostError>;
}

#[cfg(unix)]
pub(super) fn system_host() -> Box<dyn TcpHost> {
    Box::new(super::host_tcp::SystemTcpHost)
}

/// A build for a platform without host sockets (the product is macOS): every connect fails.
#[cfg(not(unix))]
struct NoHostSockets;

#[cfg(not(unix))]
impl TcpHost for NoHostSockets {
    fn connect(&mut self, _dst: SocketAddrV4) -> Result<Box<dyn HostStream>, HostError> {
        Err(HostError::Other)
    }
}

#[cfg(not(unix))]
pub(super) fn system_host() -> Box<dyn TcpHost> {
    Box::new(NoHostSockets)
}

// ---- Buffers and sequence numbers ------------------------------------------------------------

/// Bytes in flight to one side, bounded by [`TCP_BUFFER`].
struct ByteBuf {
    data: VecDeque<u8>,
}

impl ByteBuf {
    fn new() -> Self {
        Self {
            data: VecDeque::new(),
        }
    }

    fn len(&self) -> usize {
        self.data.len()
    }

    fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    fn free(&self) -> usize {
        TCP_BUFFER.saturating_sub(self.data.len())
    }

    /// Appends as much of `bytes` as fits and returns how much that was.
    fn push(&mut self, bytes: &[u8]) -> usize {
        let take = bytes.len().min(self.free());
        self.data.extend(&bytes[..take]);
        take
    }

    fn consume(&mut self, n: usize) {
        self.data.drain(..n.min(self.data.len()));
    }

    /// The first contiguous run of bytes.
    fn front(&self) -> &[u8] {
        let (first, second) = self.data.as_slices();
        if first.is_empty() { second } else { first }
    }

    /// Copies up to `len` bytes starting `offset` bytes from the front into `out`.
    fn copy_range(&self, offset: usize, len: usize, out: &mut Vec<u8>) {
        let (first, second) = self.data.as_slices();
        let mut offset = offset;
        let mut left = len;
        for part in [first, second] {
            if offset >= part.len() {
                offset -= part.len();
                continue;
            }
            let take = left.min(part.len() - offset);
            out.extend_from_slice(&part[offset..offset + take]);
            left -= take;
            offset = 0;
            if left == 0 {
                break;
            }
        }
    }
}

fn seq_lt(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}

/// MSS option of a SYN, limited to 1460 (NETWORK.md section 11); 536 when there is none.
fn guest_mss(options: &[u8]) -> u16 {
    let mut at = 0;
    while at < options.len() {
        match options[at] {
            0 => break,
            1 => at += 1,
            kind => {
                let Some(&len) = options.get(at + 1) else {
                    break;
                };
                let len = usize::from(len);
                if len < 2 || at + len > options.len() {
                    break;
                }
                if kind == 2 && len == 4 {
                    let mss = u16::from_be_bytes([options[at + 2], options[at + 3]]);
                    return if mss == 0 {
                        DEFAULT_GUEST_MSS
                    } else {
                        mss.min(TCP_MSS)
                    };
                }
                at += len;
            }
        }
    }
    DEFAULT_GUEST_MSS
}

// ---- Segments to the guest -------------------------------------------------------------------

/// Sends one segment of a flow (source: the address the guest connected to).
fn emit(ctx: &mut Ctx<'_>, key: &FlowKey, out: &TcpOut, data: &[u8]) -> bool {
    let mut buf = [0u8; 24 + TCP_MSS as usize];
    match build_tcp_segment(key.dst_ip, GUEST_IP, out, data, &mut buf) {
        Some(len) => ctx.send_ipv4(key.dst_ip, PROTO_TCP, &buf[..len]),
        None => false,
    }
}

fn send_reset(ctx: &mut Ctx<'_>, key: &FlowKey, seq: u32, ack: u32, flags: u8) {
    let out = TcpOut {
        src_port: key.dst_port,
        dst_port: key.guest_port,
        seq,
        ack,
        flags,
        window: 0,
        mss: None,
    };
    emit(ctx, key, &out, &[]);
}

/// RST for a segment that belongs to no flow (RFC 9293, section 3.10.7.1).
fn send_reset_for(ctx: &mut Ctx<'_>, key: &FlowKey, seg: &TcpSegment<'_>) {
    if seg.flags & ACK != 0 {
        send_reset(ctx, key, seg.ack, 0, RST);
    } else {
        let len = seg.data.len() as u32
            + u32::from(seg.flags & SYN != 0)
            + u32::from(seg.flags & FIN != 0);
        send_reset(ctx, key, 0, seg.seq.wrapping_add(len), RST | ACK);
    }
}

/// What the guest sees when the host connect fails (table "Ошибка connect"); `irs` is the sequence
/// number of the guest's SYN and `quoted` the IP header and 8 bytes of that SYN.
fn connect_failed(
    ctx: &mut Ctx<'_>,
    key: &FlowKey,
    irs: u32,
    quoted: &[u8; 28],
    error: HostError,
    now: Instant,
) {
    match error {
        HostError::Refused => {
            send_reset(ctx, key, 0, irs.wrapping_add(1), RST | ACK);
            bump(&mut ctx.counters.tcp_refused);
        }
        HostError::NetUnreachable | HostError::HostUnreachable => {
            bump(&mut ctx.counters.tcp_unreachable);
            let code = u8::from(error == HostError::HostUnreachable);
            ctx.send_icmp_dest_unreachable(code, quoted, now);
        }
        HostError::Resource => bump(&mut ctx.counters.drop_host_resource),
        HostError::TimedOut => bump(&mut ctx.counters.tcp_connect_timeout),
        HostError::Other => {
            send_reset(ctx, key, 0, irs.wrapping_add(1), RST | ACK);
            bump(&mut ctx.counters.tcp_connect_error);
        }
    }
}

// ---- A flow ----------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    /// The SYN is accepted, the host connect is in progress, nothing was sent to the guest.
    Connecting,
    /// The SYN-ACK is sent, the ACK of the guest is awaited.
    SynReceived,
    Open,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Outcome {
    Keep,
    /// The flow ends: its socket is closed and its slot freed.
    Remove,
}

struct TcpFlow {
    phase: Phase,
    stream: Box<dyn HostStream>,
    /// The IP header and the first 8 bytes of the guest's SYN, for an ICMP error.
    quoted: [u8; 28],
    guest_mss: u16,
    /// Our and the guest's initial sequence numbers.
    iss: u32,
    irs: u32,
    /// The next sequence number expected from the guest.
    rcv_nxt: u32,
    /// Oldest unacknowledged number, next number to send, highest number ever sent.
    snd_una: u32,
    snd_nxt: u32,
    snd_max: u32,
    /// The window the guest announced.
    snd_wnd: u32,
    /// Guest to host, not yet written to the host socket.
    to_host: ByteBuf,
    /// Host to guest, from `snd_una`: sent and unacknowledged, then not yet sent.
    to_guest: ByteBuf,
    guest_fin: bool,
    host_eof: bool,
    fin_acked: bool,
    shut_wr: bool,
    /// The window of the last segment sent to the guest.
    last_adv_window: u16,
    started: Instant,
    last_activity: Instant,
    rto: Duration,
    retries: u8,
    rto_deadline: Option<Instant>,
    persist_interval: Duration,
    persist_deadline: Option<Instant>,
    linger_deadline: Option<Instant>,
}

impl TcpFlow {
    fn advertised_window(&self) -> u16 {
        self.to_host.free().min(usize::from(TCP_WINDOW)) as u16
    }

    fn send(
        &mut self,
        ctx: &mut Ctx<'_>,
        key: &FlowKey,
        seq: u32,
        flags: u8,
        with_mss: bool,
        data: &[u8],
    ) -> bool {
        let window = self.advertised_window();
        let out = TcpOut {
            src_port: key.dst_port,
            dst_port: key.guest_port,
            seq,
            ack: self.rcv_nxt,
            flags,
            window,
            mss: with_mss.then_some(TCP_MSS),
        };
        let sent = emit(ctx, key, &out, data);
        // What the guest was told changes only when the segment reached the L3 queue, so a lost
        // window update is tried again.
        if sent {
            if window == 0 && self.last_adv_window != 0 {
                bump(&mut ctx.counters.tcp_zero_window);
            }
            self.last_adv_window = window;
        }
        sent
    }

    fn send_ack(&mut self, ctx: &mut Ctx<'_>, key: &FlowKey) {
        let seq = self.snd_nxt;
        self.send(ctx, key, seq, ACK, false, &[]);
    }

    fn send_synack(&mut self, ctx: &mut Ctx<'_>, key: &FlowKey) {
        let iss = self.iss;
        self.send(ctx, key, iss, SYN | ACK, true, &[]);
    }

    /// RST valid when its number lies inside the window we announce (RFC 9293, section 3.10.7.4).
    fn rst_in_window(&self, seg: &TcpSegment<'_>) -> bool {
        seg.seq.wrapping_sub(self.rcv_nxt) < u32::from(self.advertised_window()).max(1)
    }

    /// New data acknowledged by the guest: frees the buffer and restarts the timer with RTO 1 s.
    fn on_new_ack(&mut self, ack: u32, now: Instant) {
        let acked = ack.wrapping_sub(self.snd_una) as usize;
        let data_acked = acked.min(self.to_guest.len());
        self.to_guest.consume(data_acked);
        if self.host_eof && acked > data_acked {
            self.fin_acked = true;
        }
        self.snd_una = ack;
        if seq_lt(self.snd_nxt, ack) {
            self.snd_nxt = ack;
        }
        self.retries = 0;
        self.rto = RTO_INITIAL;
        self.rto_deadline = (self.snd_una != self.snd_max).then_some(now + RTO_INITIAL);
    }

    /// Data and FIN of a segment that starts at or before `rcv_nxt`; later data is not queued.
    fn accept_data(&mut self, seg: &TcpSegment<'_>, counters: &mut od_machine::NetCounters) {
        let ahead = seg.seq.wrapping_sub(self.rcv_nxt) as i32;
        if ahead > 0 {
            bump(&mut counters.tcp_out_of_order_dropped);
            return;
        }
        if self.guest_fin {
            return;
        }
        let skip = (-i64::from(ahead)) as usize;
        let fresh = seg.data.get(skip..).unwrap_or(&[]);
        let taken = self.to_host.push(fresh);
        self.rcv_nxt = self.rcv_nxt.wrapping_add(taken as u32);
        if seg.flags & FIN != 0 && taken == fresh.len() {
            let fin_seq = seg.seq.wrapping_add(seg.data.len() as u32);
            if fin_seq == self.rcv_nxt {
                self.guest_fin = true;
                self.rcv_nxt = self.rcv_nxt.wrapping_add(1);
            }
        }
    }

    fn check_linger(&mut self, now: Instant) {
        if self.guest_fin && self.fin_acked && self.linger_deadline.is_none() {
            self.linger_deadline = Some(now + FIN_LINGER);
        }
    }

    /// A segment of the guest for this flow.
    fn on_segment(
        &mut self,
        ctx: &mut Ctx<'_>,
        key: &FlowKey,
        seg: &TcpSegment<'_>,
        now: Instant,
    ) -> Outcome {
        self.last_activity = now;
        let rst = seg.flags & RST != 0;
        let syn = seg.flags & SYN != 0;
        match self.phase {
            Phase::Connecting => {
                // A repeated SYN is ignored until the connect is done; only an RST means something.
                return if rst && self.rst_in_window(seg) {
                    Outcome::Remove
                } else {
                    Outcome::Keep
                };
            }
            Phase::SynReceived => {
                if rst {
                    return if self.rst_in_window(seg) {
                        Outcome::Remove
                    } else {
                        Outcome::Keep
                    };
                }
                if syn {
                    if seg.seq == self.irs {
                        bump(&mut ctx.counters.tcp_retransmits);
                        self.send_synack(ctx, key);
                    }
                    return Outcome::Keep;
                }
                if seg.flags & ACK == 0 {
                    return Outcome::Keep;
                }
                if seg.ack != self.iss.wrapping_add(1) {
                    send_reset(ctx, key, seg.ack, 0, RST);
                    return Outcome::Keep;
                }
                self.phase = Phase::Open;
                self.retries = 0;
                self.rto = RTO_INITIAL;
                self.rto_deadline = None;
                self.snd_una = seg.ack;
                self.snd_nxt = seg.ack;
                self.snd_max = seg.ack;
                self.snd_wnd = u32::from(seg.window);
            }
            Phase::Open => {}
        }
        if rst {
            return if self.rst_in_window(seg) {
                Outcome::Remove
            } else {
                Outcome::Keep
            };
        }
        if syn {
            // A SYN inside a connection: only a challenge ACK (RFC 9293, section 3.10.7.4).
            self.send_ack(ctx, key);
            return Outcome::Keep;
        }
        if seg.flags & ACK == 0 {
            return Outcome::Keep;
        }
        if seq_lt(self.snd_max, seg.ack) {
            // Acknowledges data never sent.
            self.send_ack(ctx, key);
            return Outcome::Keep;
        }
        if seq_lt(self.snd_una, seg.ack) {
            self.on_new_ack(seg.ack, now);
        }
        if !seq_lt(seg.ack, self.snd_una) {
            self.snd_wnd = u32::from(seg.window);
        }
        if !seg.data.is_empty() || seg.flags & FIN != 0 {
            self.accept_data(seg, ctx.counters);
            self.send_ack(ctx, key);
        } else if seg.seq != self.rcv_nxt {
            // An empty segment outside the sequence (keep-alive, window probe) is answered with an ACK
            // (RFC 9293, section 3.10.7.4); it is not counted as out of order data.
            self.send_ack(ctx, key);
        }
        self.check_linger(now);
        Outcome::Keep
    }

    /// Moves bytes between the buffers and the host socket. `false` when the host connection failed
    /// (the guest got RST and the flow must go).
    fn pump_host(&mut self, ctx: &mut Ctx<'_>, key: &FlowKey, now: Instant) -> bool {
        while !self.to_host.is_empty() {
            match self.stream.write(self.to_host.front()) {
                IoResult::Ready(n) if n > 0 => {
                    self.to_host.consume(n);
                    self.last_activity = now;
                }
                IoResult::Ready(_) | IoResult::WouldBlock => break,
                IoResult::Eof | IoResult::Failed => return self.host_failed(ctx, key),
            }
        }
        if self.guest_fin && self.to_host.is_empty() && !self.shut_wr {
            self.shut_wr = true;
            self.stream.shutdown_write();
        }
        // Reading only while the buffer has room and the guest takes frames (back pressure).
        let mut chunk = [0u8; 16 * 1024];
        while !self.host_eof && self.to_guest.free() > 0 && ctx.rx_has_room() {
            let want = self.to_guest.free().min(chunk.len());
            match self.stream.read(&mut chunk[..want]) {
                IoResult::Ready(n) if n > 0 => {
                    self.to_guest.push(&chunk[..n.min(want)]);
                    self.last_activity = now;
                }
                IoResult::Ready(_) | IoResult::WouldBlock => break,
                IoResult::Eof => {
                    self.host_eof = true;
                    self.last_activity = now;
                }
                IoResult::Failed => return self.host_failed(ctx, key),
            }
        }
        true
    }

    fn host_failed(&mut self, ctx: &mut Ctx<'_>, key: &FlowKey) -> bool {
        bump(&mut ctx.counters.tcp_host_reset);
        send_reset(ctx, key, self.snd_max, self.rcv_nxt, RST | ACK);
        false
    }

    /// Sends buffered data and the FIN as far as the guest window and the L3 queue allow.
    fn send_data(&mut self, ctx: &mut Ctx<'_>, key: &FlowKey, now: Instant) {
        let mss = usize::from(self.guest_mss);
        let mut segment = Vec::with_capacity(mss);
        // A window of 65 535 holds at most 45 segments of 1460 bytes; the bound only guards the loop.
        for _ in 0..256 {
            if !ctx.rx_has_room() {
                break;
            }
            let in_flight = self.snd_nxt.wrapping_sub(self.snd_una) as usize;
            let buffered = self.to_guest.len();
            let unsent = buffered.saturating_sub(in_flight);
            if unsent > 0 {
                let room = (self.snd_wnd as usize).saturating_sub(in_flight);
                let n = unsent.min(mss).min(room);
                if n == 0 {
                    break;
                }
                segment.clear();
                self.to_guest.copy_range(in_flight, n, &mut segment);
                let flags = if n == unsent { ACK | PSH } else { ACK };
                let seq = self.snd_nxt;
                self.send(ctx, key, seq, flags, false, &segment);
                self.snd_nxt = self.snd_nxt.wrapping_add(n as u32);
            } else if self.host_eof && !self.fin_acked && in_flight == buffered {
                let seq = self.snd_nxt;
                self.send(ctx, key, seq, FIN | ACK, false, &[]);
                self.snd_nxt = self.snd_nxt.wrapping_add(1);
            } else {
                break;
            }
            if seq_lt(self.snd_max, self.snd_nxt) {
                self.snd_max = self.snd_nxt;
            }
            if self.rto_deadline.is_none() {
                self.rto_deadline = Some(now + self.rto);
            }
        }
    }

    /// A window update after the host socket drained a buffer that had closed the window.
    fn send_window_update(&mut self, ctx: &mut Ctx<'_>, key: &FlowKey) {
        if self.last_adv_window == 0
            && ctx.rx_has_room()
            && (self.to_host.free() >= usize::from(TCP_MSS) || self.to_host.is_empty())
        {
            self.send_ack(ctx, key);
        }
    }

    /// One more retransmission: counts it, doubles the timeout up to the ceiling, restarts the timer.
    fn back_off(&mut self, ctx: &mut Ctx<'_>, now: Instant) {
        bump(&mut ctx.counters.tcp_retransmits);
        self.retries += 1;
        self.rto = (self.rto * 2).min(RTO_MAX);
        self.rto_deadline = Some(now + self.rto);
    }

    /// Sends the oldest unacknowledged segment again and goes back to it (go back N).
    fn retransmit(&mut self, ctx: &mut Ctx<'_>, key: &FlowKey, now: Instant) {
        self.back_off(ctx, now);
        self.snd_nxt = self.snd_una;
        let seq = self.snd_una;
        let buffered = self.to_guest.len();
        if buffered > 0 {
            let n = buffered.min(usize::from(self.guest_mss));
            let mut segment = Vec::with_capacity(n);
            self.to_guest.copy_range(0, n, &mut segment);
            self.send(ctx, key, seq, ACK, false, &segment);
            self.snd_nxt = seq.wrapping_add(n as u32);
        } else if self.host_eof && !self.fin_acked {
            self.send(ctx, key, seq, FIN | ACK, false, &[]);
            self.snd_nxt = seq.wrapping_add(1);
        }
    }

    /// The retransmission timer of L10 for the SYN-ACK, data and FIN: when it has run out, wait while
    /// the NIC does not take frames (a loss now would be ours, not the network's), else send the
    /// oldest unacknowledged segment again, or give up with RST after the last retransmission.
    fn check_rto(&mut self, ctx: &mut Ctx<'_>, key: &FlowKey, now: Instant) -> Outcome {
        if !self.rto_deadline.is_some_and(|deadline| now >= deadline) {
            return Outcome::Keep;
        }
        if ctx.rx_stalled_for() >= self.rto {
            self.rto_deadline = Some(now + self.rto);
        } else if self.retries >= MAX_RETRANSMITS {
            bump(&mut ctx.counters.tcp_giveups);
            send_reset(ctx, key, self.snd_max, self.rcv_nxt, RST | ACK);
            return Outcome::Remove;
        } else if self.phase == Phase::SynReceived {
            self.back_off(ctx, now);
            self.send_synack(ctx, key);
        } else {
            self.retransmit(ctx, key, now);
        }
        Outcome::Keep
    }

    /// Timers of an open flow: linger, idle, retransmission and the zero-window probe.
    fn run_timers(&mut self, ctx: &mut Ctx<'_>, key: &FlowKey, now: Instant) -> Outcome {
        if self.linger_deadline.is_some_and(|deadline| now >= deadline) {
            return Outcome::Remove;
        }
        if self.idle_expired(now) {
            bump(&mut ctx.counters.tcp_idle_expired);
            return Outcome::Remove;
        }
        if self.snd_una == self.snd_max {
            self.rto_deadline = None;
        } else if self.check_rto(ctx, key, now) == Outcome::Remove {
            return Outcome::Remove;
        }
        let waiting = self.to_guest.len() > self.snd_nxt.wrapping_sub(self.snd_una) as usize;
        if self.snd_wnd == 0 && self.snd_una == self.snd_max && waiting {
            match self.persist_deadline {
                None => self.persist_deadline = Some(now + self.persist_interval),
                Some(deadline) if now >= deadline => {
                    let seq = self.snd_una.wrapping_sub(1);
                    self.send(ctx, key, seq, ACK, false, &[]);
                    self.persist_interval = (self.persist_interval * 2).min(RTO_MAX);
                    self.persist_deadline = Some(now + self.persist_interval);
                }
                Some(_) => {}
            }
        } else {
            self.persist_deadline = None;
            self.persist_interval = RTO_INITIAL;
        }
        Outcome::Keep
    }

    fn idle_expired(&self, now: Instant) -> bool {
        // RFC 5382 treats a connection with one FIN as established; it is transitory while opening
        // (SYN-ACK sent, no ACK) and closing (FIN from both sides).
        let limit = if self.phase != Phase::Open || (self.guest_fin && self.host_eof) {
            IDLE_TRANSITORY
        } else {
            IDLE_ESTABLISHED
        };
        now.saturating_duration_since(self.last_activity) >= limit
    }

    /// One turn of the I/O loop for this flow.
    fn serve(&mut self, ctx: &mut Ctx<'_>, key: &FlowKey, now: Instant) -> Outcome {
        match self.phase {
            Phase::Connecting => match self.stream.poll_connected() {
                Ok(true) => {
                    self.phase = Phase::SynReceived;
                    self.last_activity = now;
                    self.rto = RTO_INITIAL;
                    self.retries = 0;
                    self.rto_deadline = Some(now + RTO_INITIAL);
                    self.send_synack(ctx, key);
                    Outcome::Keep
                }
                Ok(false) => {
                    if now.saturating_duration_since(self.started) >= CONNECT_TIMEOUT {
                        bump(&mut ctx.counters.tcp_connect_timeout);
                        Outcome::Remove
                    } else {
                        Outcome::Keep
                    }
                }
                Err(error) => {
                    connect_failed(ctx, key, self.irs, &self.quoted, error, now);
                    Outcome::Remove
                }
            },
            Phase::SynReceived => {
                if self.idle_expired(now) {
                    bump(&mut ctx.counters.tcp_idle_expired);
                    Outcome::Remove
                } else {
                    self.check_rto(ctx, key, now)
                }
            }
            Phase::Open => {
                if !self.pump_host(ctx, key, now) {
                    return Outcome::Remove;
                }
                self.send_window_update(ctx, key);
                self.send_data(ctx, key, now);
                self.check_linger(now);
                self.run_timers(ctx, key, now)
            }
        }
    }
}

// ---- The engine ------------------------------------------------------------------------------

/// All TCP flows of one backend and the way out to the host.
pub struct TcpEngine {
    host: Box<dyn TcpHost>,
    flows: HashMap<FlowKey, TcpFlow>,
    next_isn: u32,
}

impl TcpEngine {
    #[cfg(test)]
    pub(crate) fn flow_count(&self) -> usize {
        self.flows.len()
    }

    pub fn new(host: Box<dyn TcpHost>) -> Self {
        Self {
            host,
            flows: HashMap::new(),
            next_isn: 0x4F44_0000,
        }
    }

    /// Forgets every flow and closes its host socket, without telling the guest (reset).
    pub fn clear(&mut self) {
        self.flows.clear();
    }

    /// A TCP segment from the guest that passed the checks of NETWORK.md section 5.
    pub(crate) fn handle_segment(
        &mut self,
        ctx: &mut Ctx<'_>,
        packet: &Ipv4Packet<'_>,
        seg: &TcpSegment<'_>,
        now: Instant,
    ) {
        bump(&mut ctx.counters.tcp_segments);
        let key = FlowKey {
            proto: FlowProto::Tcp,
            guest_port: seg.src_port,
            dst_ip: packet.dst,
            dst_port: seg.dst_port,
        };
        if matches!(packet.dst_class, DstClass::Gateway | DstClass::Dns) {
            // The backend has no TCP service of its own: RST (and none for an RST).
            if seg.flags & RST == 0 {
                send_reset_for(ctx, &key, seg);
                bump(&mut ctx.counters.tcp_refused);
            }
            return;
        }
        let plain_syn = seg.flags & (SYN | ACK | FIN | RST) == SYN;
        let Some(mut flow) = self.flows.remove(&key) else {
            self.without_flow(ctx, &key, packet, seg, plain_syn, now);
            return;
        };
        if plain_syn && flow.linger_deadline.is_some() && seg.seq != flow.irs {
            // The guest reuses the port while the old flow only waits out its linger.
            ctx.close_flow(&key);
            drop(flow);
            self.open(ctx, &key, packet, seg, now);
            return;
        }
        match flow.on_segment(ctx, &key, seg, now) {
            Outcome::Keep => {
                self.flows.insert(key, flow);
            }
            Outcome::Remove => ctx.close_flow(&key),
        }
    }

    fn without_flow(
        &mut self,
        ctx: &mut Ctx<'_>,
        key: &FlowKey,
        packet: &Ipv4Packet<'_>,
        seg: &TcpSegment<'_>,
        plain_syn: bool,
        now: Instant,
    ) {
        if seg.flags & RST != 0 {
            return;
        }
        if plain_syn {
            self.open(ctx, key, packet, seg, now);
        } else {
            send_reset_for(ctx, key, seg);
            bump(&mut ctx.counters.tcp_orphan_rst);
        }
    }

    /// A plain SYN without a flow: admission by L4, then the non-blocking host connect.
    fn open(
        &mut self,
        ctx: &mut Ctx<'_>,
        key: &FlowKey,
        packet: &Ipv4Packet<'_>,
        seg: &TcpSegment<'_>,
        now: Instant,
    ) {
        match ctx.open_flow(*key) {
            Ok(()) => {}
            // The limit refused it and the counter of L4 is already increased.
            Err(FlowError::LimitReached(_)) => return,
            Err(FlowError::Exists) => {
                // The shared table holds a TCP flow this engine does not know: a handler bug, not
                // something a guest can cause. Nothing is lost silently in a release build: the SYN
                // still counts in `tcp_segments`.
                debug_assert!(false, "flow table and TCP engine disagree about {key:?}");
                return;
            }
        }
        let mut quoted = [0u8; 28];
        let header = packet.header.len().min(20);
        quoted[..header].copy_from_slice(&packet.header[..header]);
        let tail = packet.payload.len().min(8);
        quoted[20..20 + tail].copy_from_slice(&packet.payload[..tail]);
        let stream = match self
            .host
            .connect(SocketAddrV4::new(key.dst_ip, key.dst_port))
        {
            Ok(stream) => stream,
            Err(error) => {
                connect_failed(ctx, key, seg.seq, &quoted, error, now);
                ctx.close_flow(key);
                return;
            }
        };
        bump(&mut ctx.counters.tcp_opened);
        let iss = self.next_isn;
        self.next_isn = self.next_isn.wrapping_add(ISN_STEP);
        let rcv_nxt = seg.seq.wrapping_add(1);
        let flow = TcpFlow {
            phase: Phase::Connecting,
            stream,
            quoted,
            guest_mss: guest_mss(seg.options),
            iss,
            irs: seg.seq,
            rcv_nxt,
            snd_una: iss,
            snd_nxt: iss.wrapping_add(1),
            snd_max: iss.wrapping_add(1),
            snd_wnd: 0,
            to_host: ByteBuf::new(),
            to_guest: ByteBuf::new(),
            guest_fin: false,
            host_eof: false,
            fin_acked: false,
            shut_wr: false,
            last_adv_window: TCP_WINDOW,
            started: now,
            last_activity: now,
            rto: RTO_INITIAL,
            retries: 0,
            rto_deadline: None,
            persist_interval: RTO_INITIAL,
            persist_deadline: None,
            linger_deadline: None,
        };
        self.flows.insert(*key, flow);
        // A connect that finished (or failed) at once is handled without waiting for the next turn.
        self.serve_one(ctx, key, now);
    }

    fn serve_one(&mut self, ctx: &mut Ctx<'_>, key: &FlowKey, now: Instant) {
        let Some(mut flow) = self.flows.remove(key) else {
            return;
        };
        match flow.serve(ctx, key, now) {
            Outcome::Keep => {
                self.flows.insert(*key, flow);
            }
            Outcome::Remove => ctx.close_flow(key),
        }
    }

    /// One turn of the I/O loop: host sockets and timers of every flow.
    pub(crate) fn service(&mut self, ctx: &mut Ctx<'_>, now: Instant) {
        let mut keys: Vec<FlowKey> = self.flows.keys().copied().collect();
        keys.sort_by_key(|key| (key.guest_port, key.dst_ip.octets(), key.dst_port));
        for key in keys {
            self.serve_one(ctx, &key, now);
        }
    }
}
