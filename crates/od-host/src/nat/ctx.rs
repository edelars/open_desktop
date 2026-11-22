//! What the protocol handlers of the NAT backend share (S11-31d): the way out to the guest, flow
//! admission, the ICMP error rate limit (L15) and the progress of the RX queue.
//!
//! A handler (TCP now; DHCP, DNS, UDP and ICMP echo later) keeps its own state in its own value and
//! gets a [`Ctx`] for every call. The context borrows the fields of [`super::NatBackend`] a handler
//! may touch, so a handler never holds the backend itself.

use std::time::{Duration, Instant};

use od_machine::{FrameQueue, NET_MAX_FRAME, NetCounters, PushError};

use super::bump;
use super::flow::{FlowError, FlowKey, FlowProto, FlowTable};
use super::frame::{
    GATEWAY_IP, GUEST_IP, Mac, PROTO_ICMP, build_icmp_dest_unreachable, build_ipv4_frame,
};

/// L15: ICMP errors sent to the guest per second, and the size of the bucket.
pub const ICMP_ERROR_RATE: u64 = 50;
const MILLI: u64 = 1000;

/// Puts a frame into the L3 queue; a full queue drops it at the tail and counts it (L3).
pub(super) fn enqueue_for_guest(
    queue: &mut FrameQueue,
    counters: &mut NetCounters,
    frame: &[u8],
) -> bool {
    match queue.push(frame) {
        Ok(()) => true,
        Err(PushError::Full) => {
            bump(&mut counters.drop_rx_queue_full);
            false
        }
        Err(PushError::TooLong) => {
            bump(&mut counters.drop_oversize_reply);
            false
        }
    }
}

/// Token bucket of L15: 50 ICMP errors per second, bucket of 50, counted in thousandths of a token.
pub(super) struct ErrorBucket {
    milli_tokens: u64,
    last: Option<Instant>,
}

impl ErrorBucket {
    pub(super) fn new() -> Self {
        Self {
            milli_tokens: ICMP_ERROR_RATE * MILLI,
            last: None,
        }
    }

    fn take(&mut self, now: Instant) -> bool {
        if let Some(last) = self.last {
            let elapsed_ms = now.saturating_duration_since(last).as_millis();
            // One thousandth of a token per millisecond and per token of the rate.
            let gained = u64::try_from(elapsed_ms)
                .unwrap_or(u64::MAX)
                .saturating_mul(ICMP_ERROR_RATE);
            self.milli_tokens = self
                .milli_tokens
                .saturating_add(gained)
                .min(ICMP_ERROR_RATE * MILLI);
        }
        self.last = Some(now);
        if self.milli_tokens >= MILLI {
            self.milli_tokens -= MILLI;
            true
        } else {
            false
        }
    }

    fn clear(&mut self) {
        *self = Self::new();
    }
}

/// How long the L3 queue has made no progress: the NIC does not take frames (NETWORK.md section 11,
/// the pause condition of the L10 timer). It is measured at the ticks of the I/O loop, so its
/// resolution is the tick interval.
pub(super) struct RxProgress {
    pops: u64,
    seen_pops: u64,
    since: Option<Instant>,
}

impl RxProgress {
    pub(super) fn new() -> Self {
        Self {
            pops: 0,
            seen_pops: 0,
            since: None,
        }
    }

    /// The NIC took a frame.
    pub(super) fn popped(&mut self) {
        self.pops = self.pops.wrapping_add(1);
    }

    /// Zero when the queue is empty or the NIC took a frame since the last tick.
    pub(super) fn stalled_for(&mut self, now: Instant, queue_empty: bool) -> Duration {
        if queue_empty || self.pops != self.seen_pops || self.since.is_none() {
            self.seen_pops = self.pops;
            self.since = Some(now);
            return Duration::ZERO;
        }
        self.since
            .map_or(Duration::ZERO, |since| now.saturating_duration_since(since))
    }

    pub(super) fn clear(&mut self) {
        self.since = None;
    }
}

pub(super) struct Shared {
    pub(super) ip_id: u16,
    pub(super) icmp_errors: ErrorBucket,
    pub(super) rx_progress: RxProgress,
}

impl Shared {
    pub(super) fn new() -> Self {
        Self {
            ip_id: 0,
            icmp_errors: ErrorBucket::new(),
            rx_progress: RxProgress::new(),
        }
    }

    pub(super) fn clear(&mut self) {
        // The IP identification keeps counting (it only has to differ between packets in flight).
        self.icmp_errors.clear();
        self.rx_progress.clear();
    }
}

/// The part of the backend a handler call may use.
pub(crate) struct Ctx<'a> {
    pub(super) guest_mac: Mac,
    pub(super) rx_queue: &'a mut FrameQueue,
    pub(super) counters: &'a mut NetCounters,
    pub(super) flows: &'a mut FlowTable<()>,
    pub(super) shared: &'a mut Shared,
    /// See [`RxProgress::stalled_for`]; zero while frames from the guest are handled.
    pub(super) rx_stalled_for: Duration,
}

impl Ctx<'_> {
    /// Admission of a new flow: enforces L4, L5 and L6 and counts a refusal in `drop_flow_limit_*`.
    pub(crate) fn open_flow(&mut self, key: FlowKey) -> Result<(), FlowError> {
        let result = self.flows.insert(key, ());
        if let Err(FlowError::LimitReached(proto)) = result {
            bump(match proto {
                FlowProto::Tcp => &mut self.counters.drop_flow_limit_tcp,
                FlowProto::Udp => &mut self.counters.drop_flow_limit_udp,
                FlowProto::IcmpEcho => &mut self.counters.drop_flow_limit_icmp,
            });
        }
        result
    }

    pub(crate) fn close_flow(&mut self, key: &FlowKey) {
        self.flows.remove(key);
    }

    /// True while the L3 queue can take another frame.
    pub(crate) fn rx_has_room(&self) -> bool {
        self.rx_queue.len() < self.rx_queue.capacity()
    }

    pub(crate) fn rx_stalled_for(&self) -> Duration {
        self.rx_stalled_for
    }

    /// Puts an IPv4 frame from the gateway MAC to the guest into the L3 queue (TTL 64, checksums
    /// computed). False when the frame could not be built or the queue dropped it (counted).
    pub(crate) fn send_ipv4(&mut self, src: std::net::Ipv4Addr, protocol: u8, l4: &[u8]) -> bool {
        let mut frame = [0u8; NET_MAX_FRAME];
        let id = self.shared.ip_id;
        self.shared.ip_id = id.wrapping_add(1);
        match build_ipv4_frame(self.guest_mac, src, GUEST_IP, protocol, id, l4, &mut frame) {
            Some(len) => enqueue_for_guest(self.rx_queue, self.counters, &frame[..len]),
            None => {
                bump(&mut self.counters.drop_oversize_reply);
                false
            }
        }
    }

    /// ICMP Destination Unreachable from the gateway to the guest, limited by L15: past the limit
    /// the error is not sent and `icmp_error_suppressed` grows. `quoted` is the IP header of the
    /// packet that failed and the first 8 bytes of its data (RFC 792).
    pub(crate) fn send_icmp_dest_unreachable(&mut self, code: u8, quoted: &[u8], now: Instant) {
        if !self.shared.icmp_errors.take(now) {
            bump(&mut self.counters.icmp_error_suppressed);
            return;
        }
        let mut message = [0u8; 8 + 28];
        let Some(len) = build_icmp_dest_unreachable(code, quoted, &mut message) else {
            return;
        };
        if self.send_ipv4(GATEWAY_IP, PROTO_ICMP, &message[..len]) {
            bump(&mut self.counters.icmp_error_sent);
        }
    }
}
