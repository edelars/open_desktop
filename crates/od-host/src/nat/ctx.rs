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
/// Time that yields one thousandth of a token at the rate of L15 (20 000 ns at 50 per second).
const MILLI_TOKEN_NS: u128 = 1_000_000_000 / (ICMP_ERROR_RATE as u128 * MILLI as u128);

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
        match self.last {
            None => self.last = Some(now),
            Some(last) => {
                let elapsed_ns = now.saturating_duration_since(last).as_nanos();
                let gained = elapsed_ns / MILLI_TOKEN_NS;
                // `last` moves only by the time that was turned into tokens: calls closer together
                // than one thousandth of a token (20 us) must not throw their time away.
                if gained > 0 {
                    let total = u128::from(self.milli_tokens) + gained;
                    if total >= u128::from(ICMP_ERROR_RATE * MILLI) {
                        self.milli_tokens = ICMP_ERROR_RATE * MILLI;
                        self.last = Some(now);
                    } else {
                        // total is below the bucket size, so it and gained fit in 64 bits.
                        self.milli_tokens = total as u64;
                        self.last =
                            Some(last + Duration::from_nanos((gained * MILLI_TOKEN_NS) as u64));
                    }
                }
            }
        }
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
        self.send_icmp_dest_unreachable_from(GATEWAY_IP, code, quoted, now);
    }

    /// The same from `source`: the destination itself answers "port unreachable" (S11-31c, UDP), the
    /// gateway answers "network" and "host unreachable".
    pub(crate) fn send_icmp_dest_unreachable_from(
        &mut self,
        source: std::net::Ipv4Addr,
        code: u8,
        quoted: &[u8],
        now: Instant,
    ) {
        if !self.shared.icmp_errors.take(now) {
            bump(&mut self.counters.icmp_error_suppressed);
            return;
        }
        let mut message = [0u8; 8 + 28];
        let Some(len) = build_icmp_dest_unreachable(code, quoted, &mut message) else {
            return;
        };
        if self.send_ipv4(source, PROTO_ICMP, &message[..len]) {
            bump(&mut self.counters.icmp_error_sent);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_bucket_gives_50_at_once_then_one_per_20_ms() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut bucket = ErrorBucket::new();
        assert!((0..50).all(|_| bucket.take(t0)));
        assert!(!bucket.take(t0));
        assert!(!bucket.take(at(19)));
        assert!(bucket.take(at(20)));
        assert!(!bucket.take(at(20)));
        // A long pause does not store more than the bucket.
        assert!((0..50).all(|_| bucket.take(at(60_000))));
        assert!(!bucket.take(at(60_000)));
    }

    #[test]
    fn error_bucket_keeps_the_time_between_calls_closer_than_a_millisecond() {
        let t0 = Instant::now();
        let mut bucket = ErrorBucket::new();
        assert!((0..50).all(|_| bucket.take(t0)));
        // Calls every 5 us for 100 ms: 20 000 calls, none of them a millisecond after the last.
        // The bucket earns 100 ms * 50 per second = 5 tokens; losing the fractions would earn none.
        let allowed = (1..=20_000u64)
            .filter(|n| bucket.take(t0 + Duration::from_micros(5 * n)))
            .count();
        assert_eq!(allowed, 5);
    }
}
