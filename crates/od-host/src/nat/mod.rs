//! Network backend with NAT in user space: queues, frame checks, ARP (S11-31a) and TCP (S11-31d).
//!
//! Contract: docs/specs/NETWORK.md. [`NatBackend`] implements [`od_machine::NetBackend`]: the NIC
//! calls `transmit`/`poll_receive` from the device thread, [`NatBackend::process_pending`] takes the
//! frames out of the L2 queue, checks them ([`frame::classify`]) and answers ARP for the gateway.
//! DHCP ([`dhcp`]) and DNS ([`dns`], [`resolver`]) are served here (S11-31b), TCP by [`tcp::TcpEngine`]
//! (S11-31d): a proxy at connection level over non-blocking host sockets. Everything else that passes
//! the checks is counted as `drop_unhandled_l4` until S11-31c (UDP, ICMP echo) serves it. The DNS
//! lookups run on worker threads through the system resolver.
//!
//! Threads: the backend is a plain value with no thread of its own. The I/O thread of the contract
//! (NETWORK.md section 4) calls [`NatBackend::run_at`] in a loop: it handles the frames of the guest
//! and then serves the host sockets and the timers. Time comes in as a parameter so tests can move
//! it; how the device thread and the I/O thread share the backend is not fixed here.

mod ctx;
pub mod dhcp;
pub mod dns;
pub mod flow;
pub mod frame;
#[cfg(unix)]
mod host_tcp;
pub mod resolver;
pub mod tcp;
#[cfg(test)]
mod tcp_tests;
#[cfg(test)]
mod test_frames;

use std::sync::Arc;
use std::time::Instant;

use od_machine::{
    Dropped, FrameQueue, NET_MAX_FRAME, NET_MIN_FRAME, NET_RX_QUEUE_LIMIT, NET_TX_QUEUE_LIMIT,
    NetBackend, NetCounters, PushError,
};

use self::ctx::{Ctx, Shared, enqueue_for_guest};
use self::dns::{DnsService, Parsed, Plan, SubmitError};
use self::flow::{FlowError, FlowKey, FlowTable};
use self::frame::{
    DNS_IP, DropReason, DstClass, GATEWAY_IP, GUEST_IP, GuestMacError, Inbound, Ipv4Packet, L4,
    Mac, Policy, UdpDatagram, build_arp_reply, build_udp_ipv4_frame, classify,
};
use self::resolver::{Resolver, SystemResolver};
use self::tcp::{TcpEngine, TcpHost};

/// The Ethernet destination of a DHCP NAK.
const BROADCAST_MAC: Mac = [0xFF; 6];
const DNS_PORT: u16 = 53;

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
    /// Admission and limits of every flow (L4, L5, L6); the state of a flow lives in its handler.
    flows: FlowTable<()>,
    dns: DnsService,
    counters: NetCounters,
    shared: Shared,
    tcp: TcpEngine,
}

impl NatBackend {
    /// Creates a backend for a guest with the given MAC address. The address must be unicast and
    /// different from the gateway MAC (NETWORK.md section 3); otherwise no backend is created.
    pub fn new(guest_mac: Mac) -> Result<Self, GuestMacError> {
        Ok(Self::with_policy(Policy::new(guest_mac)?))
    }

    fn with_policy(policy: Policy) -> Self {
        Self::with_policy_and_tcp_host(policy, tcp::system_host())
    }

    /// The same with another way to reach the host network for TCP (tests script it).
    fn with_policy_and_tcp_host(policy: Policy, host: Box<dyn TcpHost>) -> Self {
        Self::build(policy, host, Arc::new(SystemResolver))
    }

    /// The same with another resolver for DNS (tests script it; none touches the network).
    #[cfg(test)]
    fn with_parts(policy: Policy, resolver: Arc<dyn Resolver>) -> Self {
        let host = tcp_tests::scripted_host();
        Self::build(policy, Box::new(tcp_tests::FakeHost(host)), resolver)
    }

    fn build(policy: Policy, host: Box<dyn TcpHost>, resolver: Arc<dyn Resolver>) -> Self {
        Self {
            policy,
            tx_queue: FrameQueue::new(NET_TX_QUEUE_LIMIT),
            rx_queue: FrameQueue::new(NET_RX_QUEUE_LIMIT),
            flows: FlowTable::new(),
            dns: DnsService::new(resolver),
            counters: NetCounters::default(),
            shared: Shared::new(),
            tcp: TcpEngine::new(host),
        }
    }

    /// The handlers' view of the backend: a handler's own state and the shared context.
    fn handler_parts(&mut self, rx_stalled_for: std::time::Duration) -> (&mut TcpEngine, Ctx<'_>) {
        let ctx = Ctx {
            guest_mac: self.policy.guest_mac(),
            rx_queue: &mut self.rx_queue,
            counters: &mut self.counters,
            flows: &mut self.flows,
            shared: &mut self.shared,
            rx_stalled_for,
        };
        (&mut self.tcp, ctx)
    }

    /// Admission of a new flow by a handler of S11-31b..d: enforces L4, L5 and L6 and increases the
    /// `drop_flow_limit_*` counter when the limit refuses the flow.
    pub fn open_flow(&mut self, key: FlowKey) -> Result<(), FlowError> {
        self.handler_parts(std::time::Duration::ZERO)
            .1
            .open_flow(key)
    }

    /// Takes every frame out of the L2 queue and handles it, and delivers the DNS answers that have
    /// arrived. Never blocks. Returns the number of frames handled.
    pub fn process_pending(&mut self) -> usize {
        self.process_pending_at(Instant::now())
    }

    /// [`Self::process_pending`] with the time of the handling given (host monotonic clock): the
    /// five-second DNS timeout counts from the `now` of the call that took the query.
    pub fn process_pending_at(&mut self, now: Instant) -> usize {
        self.poll_dns(now);
        let mut frame = [0u8; NET_MAX_FRAME];
        let mut handled = 0;
        while let Some(len) = self.tx_queue.pop_into(&mut frame) {
            self.handle_frame(&frame[..len], now);
            handled += 1;
        }
        handled
    }

    /// Serves the host sockets and the timers of the handlers: connection completion, data between
    /// the host and the guest, retransmission, idle expiry. Never blocks.
    pub fn service_at(&mut self, now: Instant) {
        let stalled = self
            .shared
            .rx_progress
            .stalled_for(now, self.rx_queue.is_empty());
        let (tcp, mut ctx) = self.handler_parts(stalled);
        tcp.service(&mut ctx, now);
    }

    /// One turn of the I/O loop: the frames of the guest, then the sockets and the timers.
    pub fn run_at(&mut self, now: Instant) {
        self.process_pending_at(now);
        self.service_at(now);
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
            L4::Tcp(segment) => {
                let (tcp, mut ctx) = self.handler_parts(std::time::Duration::ZERO);
                tcp.handle_segment(&mut ctx, packet, &segment, now);
            }
            L4::Udp(udp)
                if udp.dst_port == dhcp::SERVER_PORT
                    && matches!(
                        packet.dst_class,
                        DstClass::Gateway | DstClass::LimitedBroadcast
                    ) =>
            {
                self.handle_dhcp(&udp);
            }
            L4::Udp(udp) if udp.dst_port == DNS_PORT && packet.dst_class == DstClass::Dns => {
                self.handle_dns(&udp, now);
            }
            // No handler yet: S11-31c (UDP, ICMP echo) replaces this arm.
            L4::Udp(_) | L4::Icmp(_) => bump(&mut self.counters.drop_unhandled_l4),
        }
    }

    fn handle_dhcp(&mut self, udp: &UdpDatagram<'_>) {
        let mut message = [0u8; dhcp::REPLY_CAPACITY];
        let outcome = if udp.src_port == dhcp::CLIENT_PORT {
            dhcp::respond(self.policy.guest_mac(), udp.data, &mut message)
        } else {
            dhcp::Outcome::Malformed
        };
        match outcome {
            dhcp::Outcome::Malformed => bump(&mut self.counters.dhcp_malformed),
            dhcp::Outcome::Ignored => bump(&mut self.counters.dhcp_ignored),
            dhcp::Outcome::Reply {
                kind,
                len,
                destination,
            } => {
                let dst_mac = if destination.broadcast_mac {
                    BROADCAST_MAC
                } else {
                    self.policy.guest_mac()
                };
                // The message is counted as sent whether or not the L3 queue had room for it (a
                // full queue is counted separately, as `drop_rx_queue_full`).
                bump(match kind {
                    dhcp::ReplyKind::Offer => &mut self.counters.dhcp_offers,
                    dhcp::ReplyKind::Ack => &mut self.counters.dhcp_acks,
                    dhcp::ReplyKind::Nak => &mut self.counters.dhcp_naks,
                });
                self.send_udp(
                    dst_mac,
                    (GATEWAY_IP, dhcp::SERVER_PORT),
                    (destination.ip, dhcp::CLIENT_PORT),
                    &message[..len],
                    false,
                );
            }
        }
    }

    fn handle_dns(&mut self, udp: &UdpDatagram<'_>, now: Instant) {
        match dns::parse(udp.data) {
            Parsed::Malformed => bump(&mut self.counters.dns_malformed),
            Parsed::Formerr { id, rd } => {
                bump(&mut self.counters.dns_formerr);
                self.send_dns(udp.src_port, &dns::build_formerr(id, rd), false);
            }
            Parsed::Notimp { id, rd, opcode } => {
                bump(&mut self.counters.dns_queries);
                self.send_dns(udp.src_port, &dns::build_notimp(id, rd, opcode), false);
            }
            Parsed::Query(query) => match dns::plan(&query) {
                Plan::Immediate(response, kind) => {
                    bump(&mut self.counters.dns_queries);
                    self.count_dns_reply(kind);
                    self.send_dns(udp.src_port, &response, false);
                }
                Plan::Resolve(name) => match self.dns.submit(query, name, udp.src_port, now) {
                    Ok(()) => bump(&mut self.counters.dns_queries),
                    Err(SubmitError::Busy) => bump(&mut self.counters.dns_dropped_busy),
                    Err(SubmitError::NoWorker) => {
                        // Accepted, then lost for want of a thread.
                        bump(&mut self.counters.dns_queries);
                        bump(&mut self.counters.drop_host_resource);
                    }
                },
            },
        }
    }

    /// Delivers the DNS answers that are ready: finished lookups and queries that timed out.
    fn poll_dns(&mut self, now: Instant) {
        for done in self.dns.poll(now) {
            self.count_dns_reply(done.kind);
            // An answer that arrives later answers no frame of the guest that is being handled: a
            // full L3 queue is counted apart from the one-counter-per-frame identity.
            self.send_dns(done.dst_port, &done.response, true);
        }
    }

    fn count_dns_reply(&mut self, kind: dns::ReplyKind) {
        match kind {
            dns::ReplyKind::NxDomain => bump(&mut self.counters.dns_nxdomain),
            dns::ReplyKind::ServFail => bump(&mut self.counters.dns_servfail),
            dns::ReplyKind::Plain | dns::ReplyKind::Formerr => {}
        }
    }

    fn send_dns(&mut self, guest_port: u16, response: &[u8], late: bool) {
        let mac = self.policy.guest_mac();
        self.send_udp(
            mac,
            (DNS_IP, DNS_PORT),
            (GUEST_IP, guest_port),
            response,
            late,
        );
    }

    /// Builds a UDP datagram for the guest and puts it into the L3 queue. `late` marks an answer
    /// that is sent when a lookup finishes, not while a frame of the guest is handled: if the queue
    /// is full it is counted as `dns_late_reply_dropped` and not as `drop_rx_queue_full`.
    fn send_udp(
        &mut self,
        dst_mac: Mac,
        (src_ip, src_port): (std::net::Ipv4Addr, u16),
        (dst_ip, dst_port): (std::net::Ipv4Addr, u16),
        data: &[u8],
        late: bool,
    ) {
        let mut out = [0u8; NET_MAX_FRAME];
        let id = self.shared.ip_id;
        self.shared.ip_id = id.wrapping_add(1);
        match build_udp_ipv4_frame(
            dst_mac, src_ip, dst_ip, src_port, dst_port, id, data, &mut out,
        ) {
            Some(len) if late => {
                if let Err(PushError::Full) = self.rx_queue.push(&out[..len]) {
                    bump(&mut self.counters.dns_late_reply_dropped);
                }
            }
            Some(len) => {
                self.enqueue_for_guest(&out[..len]);
            }
            None => bump(&mut self.counters.drop_oversize_reply),
        }
    }

    /// Puts a frame into the L3 queue; a full queue drops it at the tail and counts it.
    fn enqueue_for_guest(&mut self, frame: &[u8]) -> bool {
        enqueue_for_guest(&mut self.rx_queue, &mut self.counters, frame)
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
        self.shared.rx_progress.popped();
        bump(&mut self.counters.rx_frames);
        self.counters.rx_bytes = self.counters.rx_bytes.saturating_add(len as u64);
        Some(len)
    }

    fn counters(&self) -> NetCounters {
        self.counters
    }

    fn reset(&mut self) {
        self.tcp.clear();
        self.flows.clear();
        self.dns.reset();
        self.shared.clear();
        self.tx_queue.clear();
        self.rx_queue.clear();
        bump(&mut self.counters.resets);
    }
}

#[cfg(test)]
mod dhcp_tests;
#[cfg(test)]
mod dns_tests;
#[cfg(test)]
mod tests;
