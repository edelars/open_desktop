//! Network backend with NAT in user space: queues, frame checks, ARP (S11-31a), TCP (S11-31d), UDP and ICMP echo (S11-31c).
//!
//! Contract: docs/specs/NETWORK.md. [`NatBackend`] implements [`od_machine::NetBackend`]: the NIC
//! calls `transmit`/`poll_receive` from the device thread, [`NatBackend::process_pending`] takes the
//! frames out of the L2 queue, checks them ([`frame::classify`]) and answers ARP for the gateway.
//! UDP to external addresses ([`udp`]) and ICMP echo ([`icmp`]) are served over host sockets too
//! (S11-31c). TCP is served by [`tcp::TcpEngine`] (S11-31d): a proxy at connection level over non-blocking host
//! sockets. Everything else that passes the checks (DHCP and DNS datagrams) is counted as
//! `drop_unhandled_l4` until S11-31b serves it.
//!
//! Threads: the backend is a plain value with no thread of its own. The I/O thread of the contract
//! (NETWORK.md section 4) calls [`NatBackend::run_at`] in a loop: it handles the frames of the guest
//! and then serves the host sockets and the timers. Time comes in as a parameter so tests can move
//! it; how the device thread and the I/O thread share the backend is not fixed here.

mod ctx;
pub mod flow;
pub mod frame;
#[cfg(unix)]
mod host_tcp;
mod icmp;
pub mod tcp;
#[cfg(test)]
mod tcp_tests;
#[cfg(test)]
mod test_frames;
#[cfg(test)]
mod test_io;
mod udp;

use std::time::{Duration, Instant};

use od_machine::{
    Dropped, FrameQueue, NET_MAX_FRAME, NET_MIN_FRAME, NET_RX_QUEUE_LIMIT, NET_TX_QUEUE_LIMIT,
    NetBackend, NetCounters, PushError,
};

use self::ctx::{Ctx, Shared, enqueue_for_guest};
use self::flow::{FlowError, FlowKey, FlowTable};
use self::frame::{DropReason, GuestMacError, Inbound, L4, Mac, Policy, build_arp_reply, classify};
use self::icmp::EchoEngine;
use self::tcp::{TcpEngine, TcpHost};
use self::udp::UdpEngine;

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
    counters: NetCounters,
    shared: Shared,
    tcp: TcpEngine,
    udp: UdpEngine,
    echo: EchoEngine,
}

/// The protocol handlers of the backend, each with its own state (see [`NatBackend::handler_parts`]).
struct Handlers<'a> {
    tcp: &'a mut TcpEngine,
    udp: &'a mut UdpEngine,
    echo: &'a mut EchoEngine,
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
        Self {
            policy,
            tx_queue: FrameQueue::new(NET_TX_QUEUE_LIMIT),
            rx_queue: FrameQueue::new(NET_RX_QUEUE_LIMIT),
            flows: FlowTable::new(),
            counters: NetCounters::default(),
            shared: Shared::new(),
            tcp: TcpEngine::new(host),
            udp: UdpEngine::new(),
            echo: EchoEngine::new(),
        }
    }

    /// Test-only: the UDP and ICMP sockets of this backend cannot be created (as with exhausted
    /// descriptors), so that tests that feed arbitrary frames never reach the network of the
    /// machine they run on.
    #[cfg(test)]
    fn without_host_sockets(mut self) -> Self {
        self.udp.deny_sockets();
        self.echo.deny_sockets();
        self
    }

    /// The handlers' view of the backend: a handler's own state and the shared context.
    fn handler_parts(&mut self, rx_stalled_for: Duration) -> (Handlers<'_>, Ctx<'_>) {
        let ctx = Ctx {
            guest_mac: self.policy.guest_mac(),
            rx_queue: &mut self.rx_queue,
            counters: &mut self.counters,
            flows: &mut self.flows,
            shared: &mut self.shared,
            rx_stalled_for,
        };
        let handlers = Handlers {
            tcp: &mut self.tcp,
            udp: &mut self.udp,
            echo: &mut self.echo,
        };
        (handlers, ctx)
    }

    /// Admission of a new flow by a handler of S11-31b..d: enforces L4, L5 and L6 and increases the
    /// `drop_flow_limit_*` counter when the limit refuses the flow.
    pub fn open_flow(&mut self, key: FlowKey) -> Result<(), FlowError> {
        self.handler_parts(Duration::ZERO).1.open_flow(key)
    }

    /// Takes every frame out of the L2 queue and handles it. Never blocks. Returns the number of
    /// frames handled.
    pub fn process_pending(&mut self) -> usize {
        self.process_pending_at(Instant::now())
    }

    /// [`Self::process_pending`] with the time of the handling given (host monotonic clock).
    pub fn process_pending_at(&mut self, now: Instant) -> usize {
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
        let (handlers, mut ctx) = self.handler_parts(stalled);
        handlers.tcp.service(&mut ctx, now);
        handlers.udp.service(&mut ctx, now);
        handlers.echo.service(&mut ctx, now);
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
            Ok(Inbound::Ipv4(packet)) => {
                let (handlers, mut ctx) = self.handler_parts(Duration::ZERO);
                match packet.l4 {
                    L4::Tcp(segment) => {
                        handlers
                            .tcp
                            .handle_segment(&mut ctx, &packet, &segment, now);
                    }
                    L4::Udp(datagram) if !udp::is_dhcp_or_dns(&packet, &datagram) => {
                        handlers.udp.handle(&mut ctx, &packet, &datagram, now);
                    }
                    L4::Icmp(message) => {
                        handlers.echo.handle(&mut ctx, &packet, &message, now);
                    }
                    // DHCP and DNS: no handler yet, S11-31b replaces this arm.
                    L4::Udp(_) => bump(&mut ctx.counters.drop_unhandled_l4),
                }
            }
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
        // Dropping the flows closes their sockets; the guest is told nothing (NETWORK.md section 4).
        self.udp.clear();
        self.echo.clear();
        self.flows.clear();
        self.shared.clear();
        self.tx_queue.clear();
        self.rx_queue.clear();
        bump(&mut self.counters.resets);
    }
}

#[cfg(test)]
mod tests;
