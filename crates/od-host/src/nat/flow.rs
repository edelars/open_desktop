//! Flow table of the NAT backend, without handlers (S11-31a).
//!
//! It only stores and counts flows and enforces the limits L4 (TCP), L5 (UDP) and L6 (ICMP echo to
//! external addresses) of NETWORK.md section 12.1. Sockets, timers and state machines belong to
//! S11-31b..d; this table does no I/O and never evicts: when a limit is reached the new flow is
//! refused and the existing ones stay.

use std::collections::HashMap;
use std::net::Ipv4Addr;

/// L4: simultaneous TCP flows.
pub const MAX_TCP_FLOWS: usize = 128;
/// L5: simultaneous UDP flows.
pub const MAX_UDP_FLOWS: usize = 64;
/// L6: simultaneous ICMP echo requests to external addresses.
pub const MAX_ICMP_ECHO_FLOWS: usize = 8;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FlowProto {
    Tcp,
    Udp,
    IcmpEcho,
}

impl FlowProto {
    fn index(self) -> usize {
        match self {
            FlowProto::Tcp => 0,
            FlowProto::Udp => 1,
            FlowProto::IcmpEcho => 2,
        }
    }

    pub fn limit(self) -> usize {
        match self {
            FlowProto::Tcp => MAX_TCP_FLOWS,
            FlowProto::Udp => MAX_UDP_FLOWS,
            FlowProto::IcmpEcho => MAX_ICMP_ECHO_FLOWS,
        }
    }
}

/// The guest address is always 10.0.2.15, so a flow is identified by the guest port, the
/// destination address and the destination port. For ICMP echo `guest_port` is the echo identifier
/// and `dst_port` is 0.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FlowKey {
    pub proto: FlowProto,
    pub guest_port: u16,
    pub dst_ip: Ipv4Addr,
    pub dst_port: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlowError {
    /// The limit of this protocol is reached (the matching `drop_flow_limit_*` counter applies).
    LimitReached(FlowProto),
    /// A flow with this key exists; it is not replaced.
    Exists,
}

/// Flows of all protocols with one value of type `V` per flow (the handlers' state; `()` here).
pub struct FlowTable<V> {
    flows: HashMap<FlowKey, V>,
    counts: [usize; 3],
}

impl<V> FlowTable<V> {
    pub fn new() -> Self {
        Self {
            flows: HashMap::new(),
            counts: [0; 3],
        }
    }

    pub fn insert(&mut self, key: FlowKey, value: V) -> Result<(), FlowError> {
        if self.flows.contains_key(&key) {
            return Err(FlowError::Exists);
        }
        let index = key.proto.index();
        if self.counts[index] >= key.proto.limit() {
            return Err(FlowError::LimitReached(key.proto));
        }
        self.flows.insert(key, value);
        self.counts[index] += 1;
        Ok(())
    }

    pub fn get_mut(&mut self, key: &FlowKey) -> Option<&mut V> {
        self.flows.get_mut(key)
    }

    pub fn remove(&mut self, key: &FlowKey) -> Option<V> {
        let value = self.flows.remove(key)?;
        let index = key.proto.index();
        self.counts[index] = self.counts[index].saturating_sub(1);
        Some(value)
    }

    pub fn count(&self, proto: FlowProto) -> usize {
        self.counts[proto.index()]
    }

    pub fn len(&self) -> usize {
        self.flows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.flows.is_empty()
    }

    /// Forgets every flow (reset of the NIC or the machine).
    pub fn clear(&mut self) {
        self.flows.clear();
        self.counts = [0; 3];
    }
}

impl<V> Default for FlowTable<V> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(proto: FlowProto, n: u32) -> FlowKey {
        FlowKey {
            proto,
            guest_port: (n & 0xFFFF) as u16,
            dst_ip: Ipv4Addr::new(93, 184, 216, 34),
            dst_port: 80,
        }
    }

    fn fill_to_limit_and_one_over(proto: FlowProto) {
        let limit = proto.limit();
        let mut table = FlowTable::<()>::new();
        for n in 0..limit {
            assert_eq!(table.insert(key(proto, n as u32), ()), Ok(()), "flow {n}");
        }
        assert_eq!(table.count(proto), limit);
        assert_eq!(
            table.insert(key(proto, limit as u32), ()),
            Err(FlowError::LimitReached(proto))
        );
        assert_eq!(table.count(proto), limit);
        // Existing flows are not evicted; a freed slot can be reused.
        assert!(table.remove(&key(proto, 0)).is_some());
        assert_eq!(table.insert(key(proto, limit as u32), ()), Ok(()));
    }

    #[test]
    fn limits_are_the_documented_numbers() {
        assert_eq!(MAX_TCP_FLOWS, 128);
        assert_eq!(MAX_UDP_FLOWS, 64);
        assert_eq!(MAX_ICMP_ECHO_FLOWS, 8);
    }

    #[test]
    fn tcp_limit_l4_boundary() {
        fill_to_limit_and_one_over(FlowProto::Tcp);
    }

    #[test]
    fn udp_limit_l5_boundary() {
        fill_to_limit_and_one_over(FlowProto::Udp);
    }

    #[test]
    fn icmp_echo_limit_l6_boundary() {
        fill_to_limit_and_one_over(FlowProto::IcmpEcho);
    }

    #[test]
    fn protocols_are_limited_independently() {
        let mut table = FlowTable::<()>::new();
        for n in 0..MAX_UDP_FLOWS {
            table.insert(key(FlowProto::Udp, n as u32), ()).unwrap();
        }
        assert_eq!(table.insert(key(FlowProto::Tcp, 0), ()), Ok(()));
        assert_eq!(table.insert(key(FlowProto::IcmpEcho, 0), ()), Ok(()));
    }

    #[test]
    fn duplicate_key_is_refused_without_replacing() {
        let mut table = FlowTable::new();
        table.insert(key(FlowProto::Udp, 1), 10u8).unwrap();
        assert_eq!(
            table.insert(key(FlowProto::Udp, 1), 20),
            Err(FlowError::Exists)
        );
        assert_eq!(table.get_mut(&key(FlowProto::Udp, 1)), Some(&mut 10));
        assert_eq!(table.count(FlowProto::Udp), 1);
    }

    #[test]
    fn clear_forgets_everything() {
        let mut table = FlowTable::<()>::new();
        table.insert(key(FlowProto::Tcp, 1), ()).unwrap();
        table.insert(key(FlowProto::Udp, 1), ()).unwrap();
        table.clear();
        assert!(table.is_empty());
        assert_eq!(table.count(FlowProto::Tcp), 0);
        assert_eq!(table.insert(key(FlowProto::Tcp, 1), ()), Ok(()));
    }

    #[test]
    fn removing_an_unknown_key_changes_nothing() {
        let mut table = FlowTable::<()>::new();
        table.insert(key(FlowProto::Tcp, 1), ()).unwrap();
        assert!(table.remove(&key(FlowProto::Tcp, 2)).is_none());
        assert_eq!(table.count(FlowProto::Tcp), 1);
        assert_eq!(table.len(), 1);
    }
}
