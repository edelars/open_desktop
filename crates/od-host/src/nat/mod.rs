//! Network backend with NAT in user space: the part that needs no sockets (S11-31a).
//!
//! Contract: docs/specs/NETWORK.md. [`NatBackend`] implements [`od_machine::NetBackend`]: the NIC
//! calls `transmit`/`poll_receive` from the device thread, [`NatBackend::process_pending`] takes the
//! frames out of the L2 queue, checks them ([`frame::classify`]) and answers ARP for the gateway.
//! Everything else that passes the checks is counted as `drop_unhandled_l4` until S11-31b (DHCP,
//! DNS), S11-31c (UDP, ICMP echo) and S11-31d (TCP) serve it. This task opens no sockets.
//!
//! Threads: the backend is a plain value. S11-31b..d run `process_pending` on the I/O thread of the
//! contract (NETWORK.md section 4) and decide how the device thread and that thread share it; the
//! shape of that sharing is not fixed here.

pub mod flow;
pub mod frame;
#[cfg(test)]
mod test_frames;

use od_machine::{
    Dropped, FrameQueue, NET_MAX_FRAME, NET_MIN_FRAME, NET_RX_QUEUE_LIMIT, NET_TX_QUEUE_LIMIT,
    NetBackend, NetCounters, PushError,
};

use self::flow::{FlowError, FlowKey, FlowProto, FlowTable};
use self::frame::{DropReason, GuestMacError, Inbound, Mac, Policy, build_arp_reply, classify};

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
    counters: NetCounters,
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
            counters: NetCounters::default(),
        }
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
        let mut frame = [0u8; NET_MAX_FRAME];
        let mut handled = 0;
        while let Some(len) = self.tx_queue.pop_into(&mut frame) {
            self.handle_frame(&frame[..len]);
            handled += 1;
        }
        handled
    }

    fn handle_frame(&mut self, frame: &[u8]) {
        match classify(frame, &self.policy) {
            Err(reason) => record_drop(&mut self.counters, reason),
            Ok(Inbound::ArpIgnored) => bump(&mut self.counters.arp_probe_ignored),
            Ok(Inbound::ArpWhoHas {
                target_ip,
                sender_ip,
            }) => {
                let mut reply = [0u8; frame::ARP_REPLY_FRAME_LEN];
                if let Some(len) =
                    build_arp_reply(self.policy.guest_mac(), target_ip, sender_ip, &mut reply)
                    && self.enqueue_for_guest(&reply[..len])
                {
                    bump(&mut self.counters.arp_replies);
                }
            }
            // No handler yet: S11-31b..d replace this arm protocol by protocol.
            Ok(Inbound::Ipv4(_)) => bump(&mut self.counters.drop_unhandled_l4),
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
        self.tx_queue.clear();
        self.rx_queue.clear();
        bump(&mut self.counters.resets);
    }
}

#[cfg(test)]
mod tests;
