//! Network backend interface of the machine (S11-31a, [NETWORK.md](../../../docs/specs/NETWORK.md) section 4).
//!
//! This is the whole boundary between a NIC model and a network backend: a trait that moves
//! complete Ethernet frames, two bounded frame queues and a flat counter structure. The module
//! holds no sockets, file descriptors, host threads, host clocks or `od-host` types; the
//! backend implementation (NAT in user space) lives in `od-host`.

/// Largest Ethernet frame without FCS and without a VLAN tag (L1: MTU 1500 plus the 14-byte header).
pub const NET_MAX_FRAME: usize = 1514;
/// Smallest Ethernet frame a backend accepts: one Ethernet header (L1).
pub const NET_MIN_FRAME: usize = 14;
/// L2: frames accepted by [`NetBackend::transmit`] but not yet processed.
pub const NET_TX_QUEUE_LIMIT: usize = 256;
/// L3: frames produced by the backend and not yet taken by the NIC.
pub const NET_RX_QUEUE_LIMIT: usize = 256;

/// Why [`NetBackend::transmit`] rejected a frame at once. The counter of the reason is increased.
///
/// For a guest this is still a successful transmission: a real network loses packets too, so the
/// NIC completes the TX descriptor as usual (NETWORK.md section 4, item 2 of the NIC duties).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Dropped {
    /// Shorter than [`NET_MIN_FRAME`] (`drop_runt`).
    Runt,
    /// Longer than [`NET_MAX_FRAME`] (`drop_oversize`).
    Oversize,
    /// The L2 queue already holds [`NET_TX_QUEUE_LIMIT`] frames (`drop_tx_queue_full`).
    QueueFull,
}

/// Monotonic counters of a backend (NETWORK.md section 13). They survive `reset` and are
/// only ever increased. The names follow the document; one counter per drop reason.
///
/// `drop_unhandled_l4` is temporary: it counts well-formed TCP/UDP/ICMP packets that no handler
/// serves yet (S11-31a). S11-31b..d take the protocols over; the last of them removes the field.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NetCounters {
    // Frames and bytes.
    pub tx_frames: u64,
    pub tx_bytes: u64,
    pub rx_frames: u64,
    pub rx_bytes: u64,
    pub resets: u64,
    pub rx_buf_too_small: u64,
    // Dropped frames: one counter per reason (NETWORK.md sections 5, 6, 12).
    pub drop_runt: u64,
    pub drop_oversize: u64,
    pub drop_bad_src_mac: u64,
    pub drop_bad_dst_mac: u64,
    pub drop_unsupported_ethertype: u64,
    pub drop_ip_malformed: u64,
    pub drop_ip_options: u64,
    pub drop_ip_fragment: u64,
    pub drop_bad_src_ip: u64,
    pub drop_policy_dst: u64,
    pub drop_multicast_broadcast: u64,
    pub drop_unsupported_proto: u64,
    pub drop_l4_malformed: u64,
    pub drop_arp_other: u64,
    pub drop_tx_queue_full: u64,
    pub drop_rx_queue_full: u64,
    pub drop_flow_limit_tcp: u64,
    pub drop_flow_limit_udp: u64,
    pub drop_flow_limit_icmp: u64,
    pub drop_host_resource: u64,
    pub drop_oversize_reply: u64,
    pub drop_unhandled_l4: u64,
    // ARP, DHCP, DNS.
    pub arp_replies: u64,
    pub arp_probe_ignored: u64,
    pub dhcp_offers: u64,
    pub dhcp_acks: u64,
    pub dhcp_naks: u64,
    pub dhcp_ignored: u64,
    pub dhcp_malformed: u64,
    pub dns_queries: u64,
    pub dns_malformed: u64,
    pub dns_nxdomain: u64,
    pub dns_servfail: u64,
    pub dns_formerr: u64,
    pub dns_dropped_busy: u64,
    // Transport.
    pub tcp_segments: u64,
    pub tcp_opened: u64,
    pub tcp_refused: u64,
    pub tcp_unreachable: u64,
    pub tcp_connect_timeout: u64,
    pub tcp_connect_error: u64,
    pub tcp_idle_expired: u64,
    pub tcp_zero_window: u64,
    pub udp_idle_expired: u64,
    pub icmp_echo_timeout: u64,
    pub tcp_host_reset: u64,
    pub tcp_retransmits: u64,
    pub tcp_giveups: u64,
    pub tcp_orphan_rst: u64,
    pub tcp_out_of_order_dropped: u64,
    pub udp_flows_opened: u64,
    /// UDP datagrams of the guest that reached the UDP handler (S11-31c), including those for the
    /// unserved ports of the gateway and DNS address: one per frame, so it takes part in the frame
    /// accounting identity. The outcomes below (`udp_forwarded`, `udp_refused`, `udp_unreachable`,
    /// `gw_port_unreachable`) and `drop_flow_limit_udp` do not.
    pub udp_datagrams: u64,
    /// Datagrams of the guest handed to a host socket (S11-31c); an outcome, not part of the identity.
    pub udp_forwarded: u64,
    /// ICMP echo requests (type 8, code 0) of the guest that reached the echo handler (S11-31c): one
    /// per frame, part of the identity; other ICMP messages are `drop_unsupported_proto`. The outcomes
    /// `icmp_echo_synthetic`, `icmp_echo_forwarded` and `drop_flow_limit_icmp` are not part of it.
    pub icmp_echo_requests: u64,
    pub udp_refused: u64,
    pub udp_unreachable: u64,
    pub icmp_echo_synthetic: u64,
    pub icmp_echo_forwarded: u64,
    pub icmp_error_sent: u64,
    pub icmp_error_suppressed: u64,
    pub gw_port_unreachable: u64,
}

/// The interface a NIC model uses to reach a network backend (NETWORK.md section 4).
///
/// Neither method blocks: not on a socket, a resolver or a queue. No input, whatever its length
/// or bytes, may panic an implementation or touch memory outside the given slices.
pub trait NetBackend: Send {
    /// Takes one complete frame from the guest: from the destination MAC to the end of the data,
    /// without FCS, with valid IPv4/TCP/UDP/ICMP checksums (a NIC finishes any offload first).
    ///
    /// `Ok` means the frame was copied into the L2 queue; it does not mean delivery.
    /// `Err` means the frame was dropped at once and its counter increased.
    fn transmit(&mut self, frame: &[u8]) -> Result<(), Dropped>;

    /// Copies the next frame for the guest into `buf` and returns its length; `None` when there
    /// is none. A `buf` shorter than [`NET_MAX_FRAME`] gives `None`, takes no frame out of the
    /// queue and increases `rx_buf_too_small` (a caller error, not a panic).
    fn poll_receive(&mut self, buf: &mut [u8]) -> Option<usize>;

    /// A copy of the counters.
    fn counters(&self) -> NetCounters;

    /// Reset of the NIC or the machine: forget every flow, empty both queues, keep the counters,
    /// increase `resets`. The guest is not told anything.
    fn reset(&mut self);
}

/// A bounded FIFO of Ethernet frames over preallocated storage (L2 and L3 queues).
///
/// A frame of up to [`NET_MAX_FRAME`] bytes is copied in; there is no allocation after creation.
/// The queue itself never drops silently: a full queue refuses the frame and the owner counts it.
pub struct FrameQueue {
    slots: Box<[u8]>,
    lens: Box<[u16]>,
    head: usize,
    len: usize,
}

/// Why [`FrameQueue::push`] refused a frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PushError {
    Full,
    TooLong,
}

impl FrameQueue {
    /// A queue for `capacity` frames; `capacity` must not be zero.
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "a frame queue needs a non-zero capacity");
        Self {
            slots: vec![0u8; capacity * NET_MAX_FRAME].into_boxed_slice(),
            lens: vec![0u16; capacity].into_boxed_slice(),
            head: 0,
            len: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.lens.len()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Copies `frame` to the tail.
    pub fn push(&mut self, frame: &[u8]) -> Result<(), PushError> {
        if frame.len() > NET_MAX_FRAME {
            return Err(PushError::TooLong);
        }
        if self.len == self.capacity() {
            return Err(PushError::Full);
        }
        let slot = (self.head + self.len) % self.capacity();
        let base = slot * NET_MAX_FRAME;
        self.slots[base..base + frame.len()].copy_from_slice(frame);
        // The length fits: it is at most NET_MAX_FRAME (1514).
        self.lens[slot] = frame.len() as u16;
        self.len += 1;
        Ok(())
    }

    /// Length of the head frame, if any.
    pub fn peek_len(&self) -> Option<usize> {
        (self.len > 0).then(|| usize::from(self.lens[self.head]))
    }

    /// Moves the head frame into `buf` and returns its length. `buf` shorter than the head
    /// frame leaves the queue unchanged and gives `None`.
    pub fn pop_into(&mut self, buf: &mut [u8]) -> Option<usize> {
        let frame_len = self.peek_len()?;
        if buf.len() < frame_len {
            return None;
        }
        let base = self.head * NET_MAX_FRAME;
        buf[..frame_len].copy_from_slice(&self.slots[base..base + frame_len]);
        self.head = (self.head + 1) % self.capacity();
        self.len -= 1;
        Some(frame_len)
    }

    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_start_at_zero() {
        assert_eq!(NetCounters::default().tx_frames, 0);
        assert_eq!(NetCounters::default(), NetCounters::default());
    }

    #[test]
    fn queue_accepts_exactly_capacity_frames_and_refuses_the_next() {
        let mut queue = FrameQueue::new(NET_TX_QUEUE_LIMIT);
        for i in 0..NET_TX_QUEUE_LIMIT {
            assert_eq!(queue.push(&[i as u8; 20]), Ok(()), "frame {i}");
        }
        assert_eq!(queue.len(), NET_TX_QUEUE_LIMIT);
        assert_eq!(queue.push(&[0; 20]), Err(PushError::Full));
        assert_eq!(queue.len(), NET_TX_QUEUE_LIMIT);
    }

    #[test]
    fn queue_is_fifo_across_wrap_around() {
        let mut queue = FrameQueue::new(3);
        let mut buf = [0u8; NET_MAX_FRAME];
        for round in 0..10u8 {
            queue.push(&[round; 15]).unwrap();
            queue.push(&[round.wrapping_add(100); 16]).unwrap();
            assert_eq!(queue.pop_into(&mut buf), Some(15));
            assert_eq!(&buf[..15], &[round; 15]);
            assert_eq!(queue.pop_into(&mut buf), Some(16));
            assert_eq!(&buf[..16], &[round.wrapping_add(100); 16]);
            assert_eq!(queue.pop_into(&mut buf), None);
        }
    }

    #[test]
    fn queue_takes_frames_up_to_the_maximum_and_refuses_longer_ones() {
        let mut queue = FrameQueue::new(2);
        let max = [0xA5u8; NET_MAX_FRAME];
        assert_eq!(queue.push(&max), Ok(()));
        assert_eq!(
            queue.push(&[0u8; NET_MAX_FRAME + 1]),
            Err(PushError::TooLong)
        );
        let mut buf = [0u8; NET_MAX_FRAME];
        assert_eq!(queue.pop_into(&mut buf), Some(NET_MAX_FRAME));
        assert_eq!(buf, max);
    }

    #[test]
    fn short_buffer_leaves_the_frame_in_the_queue() {
        let mut queue = FrameQueue::new(2);
        queue.push(&[7u8; 100]).unwrap();
        let mut small = [0u8; 99];
        assert_eq!(queue.pop_into(&mut small), None);
        assert_eq!(queue.len(), 1);
        let mut exact = [0u8; 100];
        assert_eq!(queue.pop_into(&mut exact), Some(100));
        assert_eq!(exact, [7u8; 100]);
        assert!(queue.is_empty());
    }

    #[test]
    fn clear_empties_the_queue_and_it_stays_usable() {
        let mut queue = FrameQueue::new(2);
        queue.push(&[1; 20]).unwrap();
        queue.push(&[2; 20]).unwrap();
        queue.clear();
        assert_eq!(queue.len(), 0);
        assert_eq!(queue.peek_len(), None);
        queue.push(&[3; 21]).unwrap();
        let mut buf = [0u8; NET_MAX_FRAME];
        assert_eq!(queue.pop_into(&mut buf), Some(21));
    }

    #[test]
    fn zero_length_frame_round_trips() {
        let mut queue = FrameQueue::new(1);
        queue.push(&[]).unwrap();
        let mut buf = [0u8; NET_MAX_FRAME];
        assert_eq!(queue.pop_into(&mut buf), Some(0));
    }
}
