//! Network backend with NAT in user space: the part that needs no sockets (S11-31a).
//!
//! Contract: docs/specs/NETWORK.md. [`NatBackend`] implements [`od_machine::NetBackend`]: the NIC
//! calls `transmit`/`poll_receive` from the device thread, [`NatBackend::process_pending`] takes the
//! frames out of the L2 queue, checks them ([`frame::classify`]) and answers ARP for the gateway.
//! S11-31c serves UDP to external addresses ([`udp`]) and ICMP echo ([`icmp`]) with host sockets;
//! the rest that passes the checks (DHCP and DNS datagrams, TCP) is counted as `drop_unhandled_l4`
//! until S11-31b (DHCP, DNS) and S11-31d (TCP) serve it.
//!
//! Threads: the backend is a plain value. The I/O thread of the contract (NETWORK.md section 4)
//! calls [`NatBackend::process_pending`] for the frames of the guest and [`NatBackend::poll_io`] for
//! the sockets and the timers; how the device thread and that thread share the value is decided by
//! the task that starts the thread (S11-31b..d do not fix it). Both calls have an `_at` form that
//! takes the time, so that tests do not wait for real timeouts.

pub mod flow;
pub mod frame;
mod icmp;
#[cfg(test)]
mod test_frames;
#[cfg(test)]
mod test_io;
mod udp;

use std::net::Ipv4Addr;
use std::time::Instant;

use od_machine::{
    Dropped, FrameQueue, NET_MAX_FRAME, NET_MIN_FRAME, NET_RX_QUEUE_LIMIT, NET_TX_QUEUE_LIMIT,
    NetBackend, NetCounters, PushError,
};

use self::flow::{FlowError, FlowKey, FlowProto, FlowTable};
use self::frame::{
    DropReason, GuestMacError, Inbound, Ipv4Packet, L4, Mac, Policy, build_arp_reply,
    build_ipv4_frame, classify,
};

fn bump(counter: &mut u64) {
    *counter = counter.saturating_add(1);
}

fn record_drop(counters: &mut NetCounters, reason: DropReason) {
    bump(match reason {
        DropReason::Runt => &mut counters.drop_runt,
        DropReason::Oversize => &mut counters.drop_oversize,
        DropReason::BadSrcMac => &mut counters.drop_bad_src_mac,
        DropReason::BadDstMac => &mut counters.drop_bad_dst_mac,
        DropReason::UnsupportedEthertype => &mut counters.drop_unsupported_ethertype,
        DropReason::IpOptions => &mut counters.drop_ip_options,
        DropReason::IpMalformed => &mut counters.drop_ip_malformed,
        DropReason::IpFragment => &mut counters.drop_ip_fragment,
        DropReason::BadSrcIp => &mut counters.drop_bad_src_ip,
        DropReason::PolicyDst => &mut counters.drop_policy_dst,
        DropReason::MulticastBroadcast => &mut counters.drop_multicast_broadcast,
        DropReason::UnsupportedProto => &mut counters.drop_unsupported_proto,
        DropReason::L4Malformed => &mut counters.drop_l4_malformed,
        DropReason::ArpOther => &mut counters.drop_arp_other,
    });
}

pub struct NatBackend {
    policy: Policy,
    /// L2: frames from the guest, not yet processed.
    tx_queue: FrameQueue,
    /// L3: frames for the guest, not yet taken by the NIC.
    rx_queue: FrameQueue,
    flows: FlowTable<()>,
    /// Sockets and timers of the UDP flows (the table above counts them for L5).
    udp_flows: udp::UdpFlows,
    /// Sockets and deadlines of the echo requests that wait for a reply (L6 in the table above).
    echo_flows: icmp::EchoFlows,
    icmp_limiter: icmp::ErrorLimiter,
    /// Identification field of the next IPv4 packet for the guest (NETWORK.md section 5).
    ip_id: u16,
    counters: NetCounters,
    /// Test-only: socket creation fails as with exhausted descriptors, so that tests that feed
    /// arbitrary frames never reach the network of the machine they run on.
    #[cfg(test)]
    host_sockets_denied: bool,
}

impl NatBackend {
    /// Creates a backend for a guest with the given MAC address. The address must be unicast and
    /// different from the gateway MAC (NETWORK.md section 3); otherwise no backend is created.
    pub fn new(guest_mac: Mac) -> Result<Self, GuestMacError> {
        Ok(Self::with_policy(Policy::new(guest_mac)?))
    }

    fn with_policy(policy: Policy) -> Self {
        Self {
            policy,
            tx_queue: FrameQueue::new(NET_TX_QUEUE_LIMIT),
            rx_queue: FrameQueue::new(NET_RX_QUEUE_LIMIT),
            flows: FlowTable::new(),
            udp_flows: udp::UdpFlows::new(),
            echo_flows: icmp::EchoFlows::new(),
            icmp_limiter: icmp::ErrorLimiter::new(Instant::now()),
            ip_id: 0,
            counters: NetCounters::default(),
            #[cfg(test)]
            host_sockets_denied: false,
        }
    }

    #[cfg(test)]
    fn without_host_sockets(mut self) -> Self {
        self.host_sockets_denied = true;
        self
    }

    /// Admission of a new flow by a handler of S11-31b..d: enforces L4, L5 and L6 and increases the
    /// `drop_flow_limit_*` counter when the limit refuses the flow.
    pub fn open_flow(&mut self, key: FlowKey) -> Result<(), FlowError> {
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

    /// Takes every frame out of the L2 queue and handles it. Never blocks. Returns the number of
    /// frames handled.
    pub fn process_pending(&mut self) -> usize {
        self.process_pending_at(Instant::now())
    }

    /// [`Self::process_pending`] with the time of the call given (activity of UDP flows, deadlines).
    pub fn process_pending_at(&mut self, now: Instant) -> usize {
        let mut frame = [0u8; NET_MAX_FRAME];
        let mut handled = 0;
        while let Some(len) = self.tx_queue.pop_into(&mut frame) {
            self.handle_frame(&frame[..len], now);
            handled += 1;
        }
        handled
    }

    /// Reads what the host sockets have for the guest (UDP replies, ICMP errors of the host, echo
    /// replies) and runs the timers: UDP idle time (L12), echo timeout (L14). Never blocks; the I/O
    /// thread calls it regularly.
    pub fn poll_io(&mut self) {
        self.poll_io_at(Instant::now());
    }

    /// [`Self::poll_io`] with the time of the call given.
    pub fn poll_io_at(&mut self, now: Instant) {
        self.poll_udp(now);
        self.poll_icmp(now);
    }

    fn handle_frame(&mut self, frame: &[u8], now: Instant) {
        match classify(frame, &self.policy) {
            Err(reason) => record_drop(&mut self.counters, reason),
            Ok(Inbound::ArpIgnored) => bump(&mut self.counters.arp_probe_ignored),
            Ok(Inbound::ArpWhoHas {
                target_ip,
                sender_ip,
            }) => {
                let mut reply = [0u8; frame::ARP_REPLY_FRAME_LEN];
                let built =
                    build_arp_reply(self.policy.guest_mac(), target_ip, sender_ip, &mut reply);
                // The buffer is exactly ARP_REPLY_FRAME_LEN long, so building cannot fail.
                debug_assert!(built.is_some());
                if let Some(len) = built
                    && self.enqueue_for_guest(&reply[..len])
                {
                    bump(&mut self.counters.arp_replies);
                }
            }
            Ok(Inbound::Ipv4(packet)) => self.handle_ipv4(&packet, now),
        }
    }

    fn handle_ipv4(&mut self, packet: &Ipv4Packet<'_>, now: Instant) {
        match packet.l4 {
            L4::Udp(udp) if !udp::is_dhcp_or_dns(packet, &udp) => {
                self.handle_udp(packet, &udp, now);
            }
            L4::Icmp(icmp) => self.handle_icmp(packet, &icmp, now),
            // DHCP and DNS are S11-31b's and TCP is S11-31d's: no handler yet, they replace this
            // fallback protocol by protocol.
            L4::Udp(_) | L4::Tcp(_) => bump(&mut self.counters.drop_unhandled_l4),
        }
    }

    /// Wraps a transport message in an IPv4 packet and an Ethernet frame for the guest and puts it
    /// into the L3 queue. False when it did not fit the queue (`drop_rx_queue_full`) or the frame
    /// (`drop_oversize_reply`: more than 1514 bytes); the counter is increased.
    fn send_ipv4(&mut self, src: Ipv4Addr, dst: Ipv4Addr, protocol: u8, l4: &[u8]) -> bool {
        let mut frame = [0u8; NET_MAX_FRAME];
        let id = self.ip_id;
        self.ip_id = self.ip_id.wrapping_add(1);
        match build_ipv4_frame(
            self.policy.guest_mac(),
            src,
            dst,
            protocol,
            id,
            l4,
            &mut frame,
        ) {
            Some(len) => self.enqueue_for_guest(&frame[..len]),
            None => {
                bump(&mut self.counters.drop_oversize_reply);
                false
            }
        }
    }

    /// Puts a frame into the L3 queue; a full queue drops it at the tail and counts it.
    fn enqueue_for_guest(&mut self, frame: &[u8]) -> bool {
        match self.rx_queue.push(frame) {
            Ok(()) => true,
            Err(PushError::Full) => {
                bump(&mut self.counters.drop_rx_queue_full);
                false
            }
            Err(PushError::TooLong) => {
                bump(&mut self.counters.drop_oversize_reply);
                false
            }
        }
    }
}

impl NetBackend for NatBackend {
    fn transmit(&mut self, frame: &[u8]) -> Result<(), Dropped> {
        if frame.len() < NET_MIN_FRAME {
            bump(&mut self.counters.drop_runt);
            return Err(Dropped::Runt);
        }
        match self.tx_queue.push(frame) {
            Ok(()) => {
                bump(&mut self.counters.tx_frames);
                self.counters.tx_bytes = self.counters.tx_bytes.saturating_add(frame.len() as u64);
                Ok(())
            }
            Err(PushError::Full) => {
                bump(&mut self.counters.drop_tx_queue_full);
                Err(Dropped::QueueFull)
            }
            Err(PushError::TooLong) => {
                bump(&mut self.counters.drop_oversize);
                Err(Dropped::Oversize)
            }
        }
    }

    fn poll_receive(&mut self, buf: &mut [u8]) -> Option<usize> {
        if buf.len() < NET_MAX_FRAME {
            bump(&mut self.counters.rx_buf_too_small);
            return None;
        }
        let len = self.rx_queue.pop_into(buf)?;
        bump(&mut self.counters.rx_frames);
        self.counters.rx_bytes = self.counters.rx_bytes.saturating_add(len as u64);
        Some(len)
    }

    fn counters(&self) -> NetCounters {
        self.counters
    }

    fn reset(&mut self) {
        self.flows.clear();
        // Dropping the flows closes their sockets; the guest is told nothing (NETWORK.md section 4).
        self.udp_flows.clear();
        self.echo_flows.clear();
        self.tx_queue.clear();
        self.rx_queue.clear();
        bump(&mut self.counters.resets);
    }
}

#[cfg(test)]
mod tests;
