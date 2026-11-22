//! Tests of the DNS forwarder (S11-31b): the table of NETWORK.md section 8, EDNS, limits, the slots, the
//! workers and the five-second timeout. Lookups go to test resolvers (a table, or a gate that holds
//! them), so no test needs a network; the system resolver is tried only on names that need none.
//! Replies are read back with the reference routines of `test_frames`.

use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use od_machine::{NET_RX_QUEUE_LIMIT, NetBackend};

use super::NatBackend;
use super::dns::{self, Edns, Parsed, Plan, QUERY_TIMEOUT};
use super::frame::{DNS_IP, GUEST_IP};
use super::resolver::{ResolveOutcome, Resolver, SystemResolver};
use super::test_frames::*;
use super::tests::{backend, drain, policy, run, total_accounted};

const PORT: u16 = 40053;
const EXAMPLE: Ipv4Addr = Ipv4Addr::new(93, 184, 216, 34);

fn backend_with(resolver: Arc<dyn Resolver>) -> NatBackend {
    NatBackend::with_parts(policy(), resolver)
}

fn table(entries: &[(&str, ResolveOutcome)]) -> NatBackend {
    backend_with(Arc::new(TableResolver::with(entries)))
}

/// Waits (in real time, up to 5 s) until the backend has queued `count` frames for the guest.
/// `now` is the logical time handed to the backend: it stays before every deadline.
fn wait_frames(b: &mut NatBackend, now: Instant, count: usize) -> Vec<Vec<u8>> {
    let started = Instant::now();
    let mut frames = Vec::new();
    while frames.len() < count {
        b.process_pending_at(now);
        frames.extend(drain(b));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "only {} of {count} frames arrived",
            frames.len()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(frames.len(), count, "no more frames than expected");
    frames
}

/// Sends one DNS message from [`PORT`], waits for one reply and checks the UDP part of it.
fn ask(b: &mut NatBackend, message: &[u8]) -> DnsReply {
    let now = Instant::now();
    b.transmit(&dns_frame(PORT, message)).unwrap();
    let mut frames = wait_frames(b, now, 1);
    let udp = udp_reply(&frames.remove(0));
    assert_eq!((udp.src, udp.src_port), (DNS_IP, 53));
    assert_eq!((udp.dst, udp.dst_port), (GUEST_IP, PORT));
    assert_eq!(udp.eth_dst, GUEST_MAC);
    dns_reply(&udp.data)
}

fn a_query(name: &str) -> Vec<u8> {
    DnsMsg::query(0x4242, name, TYPE_A).build()
}

// ---- Answers --------------------------------------------------------------------------------------------

#[test]
fn an_a_query_gets_the_documented_response() {
    let mut b = table(&[(
        "example.test",
        ResolveOutcome::Addrs(vec![EXAMPLE, Ipv4Addr::new(93, 184, 216, 35)]),
    )]);
    let query = DnsMsg::query(0x4242, "example.test", TYPE_A);
    let reply = ask(&mut b, &query.build());
    // Same ID; QR=1, opcode 0, AA=0, TC=0, RD copied (1), RA=1, RCODE 0.
    assert_eq!(reply.id, 0x4242);
    assert_eq!(reply.flags, 0x8180);
    assert_eq!(reply.qdcount, 1);
    assert_eq!(reply.question, query.question, "the question is repeated");
    assert_eq!(
        reply.answers,
        [(EXAMPLE, 30), (Ipv4Addr::new(93, 184, 216, 35), 30)]
    );
    assert_eq!(reply.opt, None, "no OPT without OPT");
    let c = b.counters();
    assert_eq!(c.dns_queries, 1);
    assert_eq!(total_accounted(&c), 1);
}

#[test]
fn rd_is_copied_and_case_of_the_name_is_kept() {
    let mut b = table(&[("ExAmple.TEST", ResolveOutcome::Addrs(vec![EXAMPLE]))]);
    let mut query = DnsMsg::query(1, "ExAmple.TEST", TYPE_A);
    query.flags = 0;
    let reply = ask(&mut b, &query.build());
    assert_eq!(reply.flags, 0x8080, "RD stays 0, RA is 1");
    assert_eq!(reply.question, query.question);
    assert_eq!(reply.answers, [(EXAMPLE, 30)]);
}

#[test]
fn constants_of_section_8_are_the_documented_ones() {
    assert_eq!(dns::MAX_MESSAGE, 512);
    assert_eq!(dns::MAX_SLOTS, 8);
    assert_eq!(dns::MAX_WORKERS, 4);
    assert_eq!(dns::MAX_ANSWERS, 16);
    assert_eq!(dns::ANSWER_TTL, 30);
    assert_eq!(QUERY_TIMEOUT, Duration::from_secs(5));
}

#[test]
fn addresses_the_backend_would_drop_are_not_offered() {
    let dropped = [
        "127.0.0.1",
        "127.255.255.255",
        "0.0.0.0",
        "0.1.2.3",
        "169.254.0.1",
        "169.254.255.255",
        "10.0.2.0",
        "10.0.2.2",
        "10.0.2.3",
        "10.0.2.15",
        "10.0.2.255",
        "224.0.0.1",
        "239.255.255.250",
        "240.0.0.1",
        "255.255.255.255",
    ];
    for text in dropped {
        assert!(!dns::offerable(text.parse().unwrap()), "{text}");
    }
    let kept = [
        "1.1.1.1",
        "126.255.255.255",
        "128.0.0.1",
        "169.253.255.255",
        "169.255.0.1",
        "10.0.1.255",
        "10.0.3.0",
        "11.0.2.15",
        "192.168.0.1",
        "223.255.255.255",
    ];
    for text in kept {
        assert!(dns::offerable(text.parse().unwrap()), "{text}");
    }
    // End to end, in the middle of addresses that stay, with a duplicate.
    let mut b = table(&[(
        "mixed.test",
        ResolveOutcome::Addrs(
            [
                "127.0.0.1",
                "1.1.1.1",
                "10.0.2.15",
                "10.0.1.1",
                "1.1.1.1",
                "224.0.0.1",
                "169.254.1.1",
            ]
            .iter()
            .map(|s| s.parse().unwrap())
            .collect(),
        ),
    )]);
    let reply = ask(&mut b, &a_query("mixed.test"));
    let got: Vec<Ipv4Addr> = reply.answers.iter().map(|(a, _)| *a).collect();
    assert_eq!(got, [Ipv4Addr::new(1, 1, 1, 1), Ipv4Addr::new(10, 0, 1, 1)]);
}

#[test]
fn nothing_left_after_filtering_is_noerror_without_records() {
    let mut b = table(&[(
        "localhost",
        ResolveOutcome::Addrs(vec![Ipv4Addr::LOCALHOST]),
    )]);
    let reply = ask(&mut b, &a_query("localhost"));
    assert_eq!((reply.rcode(), reply.answers.len()), (0, 0));
    let c = b.counters();
    assert_eq!((c.dns_nxdomain, c.dns_servfail), (0, 0));
}

#[test]
fn resolver_outcomes_map_to_noerror_nxdomain_and_servfail() {
    let mut b = table(&[
        ("nodata.test", ResolveOutcome::NoData),
        ("empty.test", ResolveOutcome::Addrs(vec![])),
        ("failed.test", ResolveOutcome::Failed),
    ]);
    let nodata = ask(&mut b, &a_query("nodata.test"));
    assert_eq!((nodata.rcode(), nodata.answers.len()), (0, 0));
    let empty = ask(&mut b, &a_query("empty.test"));
    assert_eq!((empty.rcode(), empty.answers.len()), (0, 0));
    let missing = ask(&mut b, &a_query("missing.test"));
    assert_eq!(
        (missing.rcode(), missing.flags & 0x8000),
        (3, 0x8000),
        "NXDOMAIN"
    );
    assert_eq!(b.counters().dns_nxdomain, 1);
    let failed = ask(&mut b, &a_query("failed.test"));
    assert_eq!(failed.rcode(), 2, "SERVFAIL");
    let c = b.counters();
    assert_eq!((c.dns_nxdomain, c.dns_servfail, c.dns_queries), (1, 1, 4));
    // The question is repeated in every one of them.
    assert_eq!(
        missing.question,
        DnsMsg::query(0, "missing.test", TYPE_A).question
    );
}

#[test]
fn aaaa_is_an_empty_noerror_for_any_name_and_never_calls_the_resolver() {
    let gate = Gate::new();
    let mut b = backend_with(gate.clone());
    for name in ["example.test", "slow.test", "localhost", "a.b.c.d.e.f"] {
        let reply = ask(&mut b, &DnsMsg::query(9, name, TYPE_AAAA).build());
        assert_eq!((reply.rcode(), reply.answers.len()), (0, 0), "{name}");
        assert_eq!(reply.qdcount, 1);
    }
    assert_eq!(gate.calls.load(Ordering::SeqCst), 0);
    assert_eq!(b.counters().dns_queries, 4);
}

#[test]
fn other_types_get_notimp() {
    let gate = Gate::new();
    let mut b = backend_with(gate.clone());
    for qtype in [2u16, 5, 6, 12, 15, 16, 33, 255] {
        let reply = ask(&mut b, &DnsMsg::query(3, "example.test", qtype).build());
        assert_eq!((reply.rcode(), reply.answers.len()), (4, 0), "type {qtype}");
        assert_eq!(reply.qdcount, 1);
    }
    assert_eq!(gate.calls.load(Ordering::SeqCst), 0);
}

fn many_addrs(count: u8) -> ResolveOutcome {
    ResolveOutcome::Addrs((0..count).map(|i| Ipv4Addr::new(8, 8, 8, i + 1)).collect())
}

/// A wire name of exactly 255 bytes: three labels of 63 and one of 61.
fn longest_name() -> String {
    let label = |n: usize| "a".repeat(n);
    format!("{}.{}.{}.{}", label(63), label(63), label(63), label(61))
}

#[test]
fn at_most_16_records_and_at_most_what_fits_into_512_bytes() {
    // Short name: the 16-record limit applies.
    let mut b = table(&[("many.test", many_addrs(20))]);
    let reply = ask(&mut b, &a_query("many.test"));
    assert_eq!(reply.answers.len(), 16);
    // The longest name leaves room for 15 records (12 + 259 + 15 * 16 = 511; a 16th gives 527).
    let long = longest_name();
    assert_eq!(dns_name(&long).len(), 255);
    let mut b = table(&[(long.as_str(), many_addrs(20))]);
    let query = DnsMsg::query(5, &long, TYPE_A);
    let reply = ask(&mut b, &query.build());
    assert_eq!(reply.answers.len(), 15);
    // With a minimal OPT (11 bytes) the room is 14 (12 + 259 + 11 + 14 * 16 = 506; a 15th gives 522).
    let reply = ask(&mut b, &query.clone().opt(0, &[]).build());
    assert_eq!(reply.answers.len(), 14);
    assert_eq!(reply.opt, Some((512, 0, 0, 0, 0)));
    // Fewer addresses than the limit are all returned.
    let mut b = table(&[("few.test", many_addrs(3))]);
    assert_eq!(ask(&mut b, &a_query("few.test")).answers.len(), 3);
    let mut b = table(&[("sixteen.test", many_addrs(16))]);
    assert_eq!(ask(&mut b, &a_query("sixteen.test")).answers.len(), 16);
}

// ---- EDNS -------------------------------------------------------------------------------------------------

#[test]
fn edns_version_0_is_answered_with_a_minimal_opt_and_options_are_ignored() {
    let mut b = table(&[("example.test", ResolveOutcome::Addrs(vec![EXAMPLE]))]);
    // A cookie option (code 10, 8 bytes) and padding in the request.
    let query =
        DnsMsg::query(11, "example.test", TYPE_A).opt(0, &[0, 10, 0, 8, 1, 2, 3, 4, 5, 6, 7, 8]);
    let reply = ask(&mut b, &query.build());
    assert_eq!(reply.rcode(), 0);
    assert_eq!(reply.answers, [(EXAMPLE, 30)]);
    // UDP size 512, extended RCODE 0, version 0, flags 0, no data.
    assert_eq!(reply.opt, Some((512, 0, 0, 0, 0)));
}

#[test]
fn opt_is_added_to_every_kind_of_answer_to_a_query_that_had_one() {
    let mut b = table(&[("failed.test", ResolveOutcome::Failed)]);
    for (name, qtype, rcode) in [
        ("example.test", TYPE_AAAA, 0),
        ("example.test", 15, 4),
        ("missing.test", TYPE_A, 3),
        ("failed.test", TYPE_A, 2),
    ] {
        let reply = ask(&mut b, &DnsMsg::query(1, name, qtype).opt(0, &[]).build());
        assert_eq!(reply.rcode(), rcode, "{name} {qtype}");
        assert_eq!(reply.opt, Some((512, 0, 0, 0, 0)), "{name} {qtype}");
    }
}

#[test]
fn another_edns_version_is_badvers_with_a_minimal_opt() {
    let gate = Gate::new();
    let mut b = backend_with(gate.clone());
    let reply = ask(
        &mut b,
        &DnsMsg::query(12, "example.test", TYPE_A)
            .opt(1, &[])
            .build(),
    );
    // RCODE 16 is 0 in the header and 1 in the extended part of the OPT record; version 0 in the response.
    assert_eq!(reply.rcode(), 0);
    assert_eq!(reply.opt, Some((512, 1, 0, 0, 0)));
    assert_eq!(reply.answers.len(), 0);
    assert_eq!(reply.qdcount, 1);
    assert_eq!(gate.calls.load(Ordering::SeqCst), 0, "no lookup");
}

#[test]
fn two_opt_records_get_formerr_without_opt() {
    let mut b = backend();
    let query = DnsMsg::query(13, "example.test", TYPE_A)
        .opt(0, &[])
        .opt(0, &[]);
    let reply = ask(&mut b, &query.build());
    assert_formerr(&reply, 13);
    let c = b.counters();
    assert_eq!((c.dns_formerr, c.dns_queries), (1, 0));
}

fn assert_formerr(reply: &DnsReply, id: u16) {
    assert_eq!(reply.id, id);
    assert_eq!(reply.rcode(), 1);
    // QR and RA set; opcode, AA, TC and Z clear; RD is copied (not checked here).
    assert_eq!(reply.flags & 0xFEF0, 0x8080, "QR, RA");
    assert_eq!(
        (reply.qdcount, reply.answers.len(), reply.opt),
        (0, 0, None)
    );
}

// ---- Queries that are not served ------------------------------------------------------------------

#[test]
fn formerr_cases_each_get_a_header_only_response_and_one_counter() {
    let name = |n: &str| dns_name(n);
    let mut cases: Vec<(&str, Vec<u8>)> = Vec::new();
    cases.push((
        "class CH",
        DnsMsg::question_bytes(21, name("a.test"), TYPE_A, 3).build(),
    ));
    cases.push((
        "class ANY",
        DnsMsg::question_bytes(21, name("a.test"), TYPE_A, 255).build(),
    ));
    let mut none = DnsMsg::query(21, "a.test", TYPE_A);
    none.qdcount = 0;
    cases.push(("QDCOUNT 0", none.build()));
    let mut two = DnsMsg::query(21, "a.test", TYPE_A);
    two.qdcount = 2;
    cases.push(("QDCOUNT 2", two.build()));
    // A compression pointer where the question name should be; and the reserved label types.
    cases.push((
        "pointer in the question",
        DnsMsg::question_bytes(21, vec![0xC0, 0x0C], TYPE_A, 1).build(),
    ));
    cases.push((
        "label type 01",
        DnsMsg::question_bytes(21, vec![0x40, 0], TYPE_A, 1).build(),
    ));
    cases.push((
        "label type 10",
        DnsMsg::question_bytes(21, vec![0x80, 0], TYPE_A, 1).build(),
    ));
    let mut truncated = DnsMsg::query(21, "a.test", TYPE_A).build();
    truncated.truncate(12 + 3);
    cases.push(("name cut short", truncated));
    let mut no_class = DnsMsg::query(21, "a.test", TYPE_A).build();
    no_class.truncate(no_class.len() - 2);
    cases.push(("no class", no_class));
    cases.push((
        "label of 64",
        DnsMsg::question_bytes(21, [vec![64], vec![b'a'; 64], vec![0]].concat(), TYPE_A, 1).build(),
    ));
    cases.push((
        "name of 256 bytes",
        DnsMsg::question_bytes(
            21,
            dns_name(&format!(
                "{}.{}.{}.{}",
                "a".repeat(63),
                "a".repeat(63),
                "a".repeat(63),
                "a".repeat(62)
            )),
            TYPE_A,
            1,
        )
        .build(),
    ));
    let mut cut_record = DnsMsg::query(21, "a.test", TYPE_A).opt(0, &[]).build();
    cut_record.truncate(cut_record.len() - 3);
    cases.push(("OPT record cut short", cut_record));
    let mut named_opt = DnsMsg::query(21, "a.test", TYPE_A);
    named_opt.additional = vec![0xC0, 0x0C, 0, 41, 0x10, 0, 0, 0, 0, 0, 0, 0];
    named_opt.arcount = 1;
    cases.push(("OPT whose name is not the root", named_opt.build()));
    for (what, message) in cases {
        let mut b = backend();
        let reply = ask(&mut b, &message);
        assert_formerr(&reply, 21);
        assert_eq!(reply.flags, 0x8181, "{what}: RD copied");
        let c = b.counters();
        assert_eq!(c.dns_formerr, 1, "{what}");
        assert_eq!(total_accounted(&c), 1, "{what}: one counter");
    }
}

#[test]
fn an_opcode_other_than_query_gets_notimp_with_the_opcode_copied() {
    for opcode in [1u16, 2, 4, 5, 15] {
        let mut b = backend();
        let mut query = DnsMsg::query(21, "a.test", TYPE_A);
        query.flags = 0x0100 | (opcode << 11);
        let reply = ask(&mut b, &query.build());
        assert_eq!(reply.id, 21);
        // QR, the opcode, RD, RA, RCODE 4; header only.
        assert_eq!(reply.flags, 0x8000 | (opcode << 11) | 0x0100 | 0x0080 | 4);
        assert_eq!(
            (reply.qdcount, reply.answers.len(), reply.opt),
            (0, 0, None)
        );
        let c = b.counters();
        assert_eq!((c.dns_queries, c.dns_formerr), (1, 0), "opcode {opcode}");
        assert_eq!(total_accounted(&c), 1);
    }
}

#[test]
fn names_at_the_limits_are_accepted() {
    let long = longest_name();
    let mut b = table(&[(long.as_str(), ResolveOutcome::Addrs(vec![EXAMPLE]))]);
    assert_eq!(
        ask(&mut b, &a_query(&long)).answers.len(),
        1,
        "255 bytes in the wire format"
    );
    let label63 = "b".repeat(63);
    let mut b = table(&[(label63.as_str(), ResolveOutcome::Addrs(vec![EXAMPLE]))]);
    assert_eq!(
        ask(&mut b, &a_query(&label63)).answers.len(),
        1,
        "a label of 63"
    );
}

#[test]
fn a_response_or_a_short_or_a_long_message_gets_no_answer_and_is_malformed() {
    let valid = a_query("example.test");
    let mut cases: Vec<(String, Vec<u8>)> = Vec::new();
    for len in 0..12 {
        cases.push((
            format!("{len} bytes"),
            valid.iter().copied().cycle().take(len).collect(),
        ));
    }
    let mut response = DnsMsg::query(1, "example.test", TYPE_A);
    response.flags = 0x8180;
    cases.push(("QR set".into(), response.build()));
    // 513 bytes: a valid query with an OPT whose data fills the message up to one byte too many.
    let base = DnsMsg::query(1, "example.test", TYPE_A);
    let overhead = base.clone().opt(0, &[]).build().len();
    let rdata = vec![0u8; 513 - overhead];
    cases.push(("513 bytes".into(), base.opt(0, &rdata).build()));
    for (what, message) in cases {
        let mut b = backend();
        run(&mut b, &dns_frame(PORT, &message));
        let c = b.counters();
        assert_eq!(c.dns_malformed, 1, "{what}");
        assert_eq!(total_accounted(&c), 1, "{what}: one counter");
        assert!(drain(&mut b).is_empty(), "{what}: silence");
    }
}

#[test]
fn a_query_of_exactly_512_bytes_is_served() {
    let base = DnsMsg::query(1, "example.test", TYPE_A);
    let overhead = base.clone().opt(0, &[]).build().len();
    let message = base.opt(0, &vec![0u8; 512 - overhead]).build();
    assert_eq!(message.len(), 512);
    let mut b = table(&[("example.test", ResolveOutcome::Addrs(vec![EXAMPLE]))]);
    let reply = ask(&mut b, &message);
    assert_eq!(reply.answers.len(), 1);
}

#[test]
fn names_that_cannot_be_host_names_are_nxdomain_without_a_lookup() {
    let gate = Gate::new();
    let mut b = backend_with(gate.clone());
    let wire_names: Vec<(&str, Vec<u8>)> = vec![
        ("root", vec![0]),
        ("a space", [vec![3], b"a b".to_vec(), vec![0]].concat()),
        (
            "a dot inside a label",
            [vec![3], b"a.b".to_vec(), vec![0]].concat(),
        ),
        (
            "a NUL byte",
            [vec![3], vec![b'a', 0, b'b'], vec![0]].concat(),
        ),
        ("non-ASCII", [vec![2], vec![0xC3, 0xA9], vec![0]].concat()),
        (
            "a control byte",
            [vec![2], vec![b'a', 0x07], vec![0]].concat(),
        ),
    ];
    for (what, wire) in wire_names {
        let reply = ask(&mut b, &DnsMsg::question_bytes(2, wire, TYPE_A, 1).build());
        assert_eq!(reply.rcode(), 3, "{what}");
    }
    assert_eq!(gate.calls.load(Ordering::SeqCst), 0);
    assert_eq!(b.counters().dns_nxdomain, 6);
}

#[test]
fn only_port_53_of_the_dns_address_is_the_forwarder() {
    // Another port of the DNS address and any port of the gateway are not the forwarder's business.
    let mut b = backend();
    run(&mut b, &udp_frame(DNS_IP, 54, &a_query("example.test")));
    assert_eq!(b.counters().drop_unhandled_l4, 1);
    assert_eq!(b.counters().dns_queries, 0);
}

// ---- Slots, workers and timeouts ------------------------------------------------------------------

/// A resolver whose names that start with "slow" wait for `release`; it records its calls and how many
/// run at the same time.
struct Gate {
    open: Mutex<bool>,
    opened: Condvar,
    calls: AtomicUsize,
    active: AtomicUsize,
    max_active: AtomicUsize,
}

impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            open: Mutex::new(false),
            opened: Condvar::new(),
            calls: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
        })
    }

    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.opened.notify_all();
    }

    fn wait_until(&self, what: &str, done: impl Fn() -> bool) {
        let started = Instant::now();
        while !done() {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "timed out waiting for {what}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

impl Resolver for Gate {
    fn resolve_a(&self, name: &str) -> ResolveOutcome {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let now_active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(now_active, Ordering::SeqCst);
        if name.starts_with("slow") {
            let mut open = self.open.lock().unwrap();
            let deadline = Instant::now() + Duration::from_secs(20);
            while !*open && Instant::now() < deadline {
                open = self
                    .opened
                    .wait_timeout(open, Duration::from_millis(50))
                    .unwrap()
                    .0;
            }
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
        ResolveOutcome::Addrs(vec![EXAMPLE])
    }
}

/// Waits until no worker is inside a lookup and every finished lookup has handed in its result.
fn wait_idle(gate: &Gate, b: &NatBackend) {
    gate.wait_until("every worker to be idle", || {
        b.dns.idle_workers() == b.dns.workers()
    });
}

fn send_slow(b: &mut NatBackend, now: Instant, index: u16) {
    let name = format!("slow{index}.test");
    let message = DnsMsg::query(100 + index, &name, TYPE_A).build();
    b.transmit(&dns_frame(PORT + index, &message)).unwrap();
    b.process_pending_at(now);
}

#[test]
fn eight_slots_four_workers_and_the_ninth_query_is_dropped_as_busy() {
    let gate = Gate::new();
    let mut b = backend_with(gate.clone());
    let t0 = Instant::now();
    let started = Instant::now();
    for index in 0..8 {
        send_slow(&mut b, t0, index);
    }
    // Processing returned at once although every lookup is stuck.
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(b.dns.occupied(), 8);
    gate.wait_until("four lookups in flight", || {
        gate.active.load(Ordering::SeqCst) == 4
    });
    // Four lookups run, four jobs wait in the queue, and no fifth worker exists.
    assert_eq!(gate.calls.load(Ordering::SeqCst), 4);
    assert_eq!(b.dns.workers(), 4);
    assert_eq!(b.dns.queued(), 4);
    // The ninth query needs a slot: no reply, counted busy.
    send_slow(&mut b, t0, 8);
    let c = b.counters();
    assert_eq!((c.dns_queries, c.dns_dropped_busy), (8, 1));
    assert!(drain(&mut b).is_empty());
    // An answer that needs no slot is still given while every slot is busy.
    let aaaa = DnsMsg::query(77, "slow.test", TYPE_AAAA).build();
    b.transmit(&dns_frame(PORT, &aaaa)).unwrap();
    let frames = wait_frames(&mut b, t0, 1);
    assert_eq!(dns_reply(&udp_reply(&frames[0]).data).id, 77);
    // Let the lookups finish: eight answers, each to its own port and with its own ID.
    gate.release();
    let frames = wait_frames(&mut b, t0, 8);
    let mut seen: Vec<(u16, u16)> = frames
        .iter()
        .map(|f| {
            let udp = udp_reply(f);
            let reply = dns_reply(&udp.data);
            assert_eq!(reply.answers, [(EXAMPLE, 30)]);
            (udp.dst_port, reply.id)
        })
        .collect();
    seen.sort_unstable();
    let expected: Vec<(u16, u16)> = (0..8).map(|i| (PORT + i, 100 + i)).collect();
    assert_eq!(seen, expected);
    assert_eq!(b.dns.occupied(), 0);
    // Slots are free again.
    send_slow(&mut b, t0, 8);
    // Eight slow queries and the AAAA query were counted before; this is the tenth.
    assert_eq!(b.counters().dns_queries, 10);
    assert_eq!(b.dns.occupied(), 1);
    wait_frames(&mut b, t0, 1);
    // Never more than four lookups at once, over the whole run.
    wait_idle(&gate, &b);
    assert_eq!(gate.max_active.load(Ordering::SeqCst), 4);
    assert_eq!(gate.calls.load(Ordering::SeqCst), 9);
}

#[test]
fn a_query_that_gets_no_answer_in_five_seconds_gets_servfail_and_the_late_result_is_dropped() {
    let gate = Gate::new();
    let mut b = backend_with(gate.clone());
    let t0 = Instant::now();
    send_slow(&mut b, t0, 0);
    gate.wait_until("the lookup to start", || {
        gate.active.load(Ordering::SeqCst) == 1
    });
    // Just before five seconds: nothing.
    b.process_pending_at(t0 + QUERY_TIMEOUT - Duration::from_nanos(1));
    assert!(drain(&mut b).is_empty());
    assert_eq!(b.dns.occupied(), 1);
    // At five seconds: SERVFAIL to the same port with the same ID, and the slot is free.
    b.process_pending_at(t0 + QUERY_TIMEOUT);
    let frames = drain(&mut b);
    assert_eq!(frames.len(), 1);
    let udp = udp_reply(&frames[0]);
    let reply = dns_reply(&udp.data);
    assert_eq!((udp.dst_port, reply.id, reply.rcode()), (PORT, 100, 2));
    assert_eq!(
        reply.question,
        DnsMsg::query(0, "slow0.test", TYPE_A).question
    );
    assert_eq!(b.dns.occupied(), 0);
    let c = b.counters();
    assert_eq!((c.dns_servfail, c.dns_queries), (1, 1));
    // The lookup finally returns: its result is dropped, the guest hears nothing more.
    gate.release();
    wait_idle(&gate, &b);
    b.process_pending_at(t0 + QUERY_TIMEOUT + Duration::from_secs(1));
    assert!(drain(&mut b).is_empty());
    assert_eq!(b.counters().dns_servfail, 1);
}

#[test]
fn queries_that_wait_for_a_free_worker_time_out_too_and_are_never_looked_up() {
    let gate = Gate::new();
    let mut b = backend_with(gate.clone());
    let t0 = Instant::now();
    for index in 0..8 {
        send_slow(&mut b, t0, index);
    }
    gate.wait_until("four lookups in flight", || {
        gate.active.load(Ordering::SeqCst) == 4
    });
    // Five seconds later all eight (four running, four waiting for a worker) have been answered.
    b.process_pending_at(t0 + QUERY_TIMEOUT);
    let frames = drain(&mut b);
    assert_eq!(frames.len(), 8);
    for frame in &frames {
        assert_eq!(dns_reply(&udp_reply(frame).data).rcode(), 2);
    }
    assert_eq!(b.counters().dns_servfail, 8);
    assert_eq!(b.dns.occupied(), 0);
    // The four jobs that no worker had taken left the queue with their slots.
    assert_eq!(b.dns.queued(), 0);
    // The four running lookups finish; the four that never started are not run.
    gate.release();
    wait_idle(&gate, &b);
    assert_eq!(
        gate.calls.load(Ordering::SeqCst),
        4,
        "cancelled jobs are skipped"
    );
    b.process_pending_at(t0 + Duration::from_secs(60));
    assert!(drain(&mut b).is_empty());
    assert_eq!(b.dns.workers(), 4, "the workers stay for the next queries");
}

#[test]
fn a_hung_lookup_does_not_block_later_queries_once_its_own_slot_has_timed_out() {
    let gate = Gate::new();
    let mut b = backend_with(gate.clone());
    let t0 = Instant::now();
    send_slow(&mut b, t0, 0);
    gate.wait_until("the lookup to start", || {
        gate.active.load(Ordering::SeqCst) == 1
    });
    // After five seconds the guest gets SERVFAIL and the slot is free; the lookup still hangs.
    b.process_pending_at(t0 + QUERY_TIMEOUT);
    let frames = drain(&mut b);
    assert_eq!(frames.len(), 1);
    assert_eq!(dns_reply(&udp_reply(&frames[0]).data).rcode(), 2);
    assert_eq!(gate.active.load(Ordering::SeqCst), 1);
    // A query for a fast name must be served by another worker, not wait behind the stuck one.
    let later = t0 + QUERY_TIMEOUT;
    b.transmit(&dns_frame(PORT + 1, &a_query("fast.test")))
        .unwrap();
    let frames = wait_frames(&mut b, later, 1);
    let udp = udp_reply(&frames[0]);
    assert_eq!(udp.dst_port, PORT + 1);
    assert_eq!(dns_reply(&udp.data).answers, [(EXAMPLE, 30)]);
    assert_eq!(gate.active.load(Ordering::SeqCst), 1, "slow0 still hangs");
    assert_eq!(b.dns.workers(), 2);
    gate.release();
    wait_idle(&gate, &b);
}

#[test]
fn reset_forgets_pending_queries_and_their_late_results() {
    let gate = Gate::new();
    let mut b = backend_with(gate.clone());
    let t0 = Instant::now();
    send_slow(&mut b, t0, 0);
    gate.wait_until("the lookup to start", || {
        gate.active.load(Ordering::SeqCst) == 1
    });
    b.reset();
    assert_eq!(b.dns.occupied(), 0);
    gate.release();
    wait_idle(&gate, &b);
    b.process_pending_at(t0 + Duration::from_secs(1));
    assert!(drain(&mut b).is_empty(), "nothing is sent after a reset");
    let c = b.counters();
    assert_eq!((c.resets, c.dns_queries, c.dns_servfail), (1, 1, 0));
}

#[test]
fn dropping_the_backend_does_not_wait_for_a_stuck_lookup() {
    let gate = Gate::new();
    let mut b = backend_with(gate.clone());
    send_slow(&mut b, Instant::now(), 0);
    gate.wait_until("the lookup to start", || {
        gate.active.load(Ordering::SeqCst) == 1
    });
    let started = Instant::now();
    drop(b);
    assert!(started.elapsed() < Duration::from_secs(1));
    gate.release();
    gate.wait_until("the worker to finish", || {
        gate.active.load(Ordering::SeqCst) == 0
    });
}

/// A backend whose L3 queue is full of ARP replies.
fn backend_with_full_rx_queue(resolver: Arc<dyn Resolver>) -> NatBackend {
    let mut b = backend_with(resolver);
    for _ in 0..NET_RX_QUEUE_LIMIT {
        run(&mut b, &arp_request_for_gateway());
    }
    assert_eq!(b.counters().arp_replies, NET_RX_QUEUE_LIMIT as u64);
    b
}

#[test]
fn an_immediate_answer_that_does_not_fit_the_l3_queue_is_counted_as_a_query_and_a_queue_drop() {
    let mut b = backend_with_full_rx_queue(Arc::new(NotFoundResolver));
    run(
        &mut b,
        &dns_frame(PORT, &DnsMsg::query(1, "example.test", TYPE_AAAA).build()),
    );
    let c = b.counters();
    assert_eq!((c.dns_queries, c.drop_rx_queue_full), (1, 1));
    // A NOTIMP answer likewise.
    run(
        &mut b,
        &dns_frame(PORT, &DnsMsg::query(1, "example.test", 12).build()),
    );
    let c = b.counters();
    assert_eq!((c.dns_queries, c.drop_rx_queue_full), (2, 2));
    assert_eq!(c.dns_late_reply_dropped, 0);
}

#[test]
fn a_formerr_that_does_not_fit_the_l3_queue_is_counted_as_formerr_and_a_queue_drop() {
    let mut b = backend_with_full_rx_queue(Arc::new(NotFoundResolver));
    let mut query = DnsMsg::query(1, "example.test", TYPE_A);
    query.qdcount = 0;
    run(&mut b, &dns_frame(PORT, &query.build()));
    let c = b.counters();
    assert_eq!((c.dns_formerr, c.drop_rx_queue_full), (1, 1));
}

#[test]
fn a_late_answer_that_does_not_fit_the_l3_queue_has_its_own_counter() {
    let table = Arc::new(TableResolver::with(&[(
        "example.test",
        ResolveOutcome::Addrs(vec![EXAMPLE]),
    )]));
    let mut b = backend_with_full_rx_queue(table);
    let now = Instant::now();
    b.transmit(&dns_frame(PORT, &a_query("example.test")))
        .unwrap();
    // The query is accepted while the queue is full; its answer is ready later and finds it full.
    let started = Instant::now();
    while b.counters().dns_late_reply_dropped == 0 {
        b.process_pending_at(now);
        assert!(started.elapsed() < Duration::from_secs(5), "no late answer");
        std::thread::sleep(Duration::from_millis(1));
    }
    let c = b.counters();
    assert_eq!(c.dns_queries, 1);
    assert_eq!(
        c.drop_rx_queue_full, 0,
        "no frame of the guest was being handled"
    );
    assert_eq!(c.dns_late_reply_dropped, 1);
    // Identity of one counter per frame: 256 ARP replies and the one query.
    assert_eq!(total_accounted(&c), NET_RX_QUEUE_LIMIT as u64 + 1);
}

// ---- The system resolver ------------------------------------------------------------------------------

#[test]
fn the_system_resolver_resolves_numeric_names_without_a_network() {
    assert_eq!(
        SystemResolver.resolve_a("93.184.216.34"),
        ResolveOutcome::Addrs(vec![EXAMPLE])
    );
    // A NUL byte cannot reach the C interface; the forwarder never offers one, the resolver refuses it.
    assert_eq!(SystemResolver.resolve_a("a\0b"), ResolveOutcome::NotFound);
}

#[test]
fn the_system_resolver_knows_localhost_from_the_hosts_file_and_the_backend_hides_it() {
    match SystemResolver.resolve_a("localhost") {
        ResolveOutcome::Addrs(addrs) => assert!(addrs.contains(&Ipv4Addr::LOCALHOST), "{addrs:?}"),
        other => panic!("localhost: {other:?}"),
    }
    // Through the whole backend: the product never gives the guest the loopback address.
    let mut b = backend_with(Arc::new(SystemResolver));
    let reply = ask(&mut b, &a_query("localhost"));
    assert_eq!((reply.rcode(), reply.answers.len()), (0, 0));
    // And a numeric name answers with itself, end to end.
    let reply = ask(&mut b, &a_query("93.184.216.34"));
    assert_eq!(reply.answers, [(EXAMPLE, 30)]);
}

#[test]
fn the_system_resolver_does_not_invent_a_name_in_the_invalid_tld() {
    // RFC 6761: ".invalid" never resolves. Without a network the error may be a failure rather than
    // "not found"; either way the guest is not given an address.
    let outcome = SystemResolver.resolve_a("od-nat-test.invalid");
    assert!(
        matches!(
            outcome,
            ResolveOutcome::NotFound | ResolveOutcome::Failed | ResolveOutcome::NoData
        ),
        "{outcome:?}"
    );
}

// ---- Garbage --------------------------------------------------------------------------------------------

fn base_queries() -> Vec<Vec<u8>> {
    vec![
        DnsMsg::query(1, "example.test", TYPE_A).build(),
        DnsMsg::query(2, "a.b.c.example.test", TYPE_AAAA)
            .opt(0, &[0, 10, 0, 2, 1, 2])
            .build(),
        DnsMsg::query(3, "example.test", 15).opt(1, &[]).build(),
        DnsMsg::query(4, &longest_name(), TYPE_A)
            .opt(0, &[])
            .opt(0, &[])
            .build(),
    ]
}

/// What the forwarder promises about every query it understood and every response it builds.
fn check_parsed(data: &[u8]) {
    match dns::parse(data) {
        Parsed::Malformed => {}
        Parsed::Formerr { id, rd } => {
            let reply = dns_reply(&dns::build_formerr(id, rd));
            assert_formerr(&reply, id);
        }
        Parsed::Notimp { id, rd, opcode } => {
            let reply = dns_reply(&dns::build_notimp(id, rd, opcode));
            assert_eq!((reply.id, reply.rcode()), (id, 4));
            assert_eq!(reply.flags >> 11 & 0xF, u16::from(opcode));
        }
        Parsed::Query(query) => {
            assert!(data.len() >= 12 && data.len() <= 512);
            match dns::plan(&query) {
                Plan::Immediate(response, _) => {
                    let reply = dns_reply(&response);
                    assert_eq!(reply.id, query.id);
                    assert_eq!(reply.qdcount, 1);
                }
                Plan::Resolve(name) => {
                    assert_eq!(query.qtype, TYPE_A);
                    assert!(query.edns != Edns::BadVersion);
                    assert!(!name.is_empty() && name.len() <= 253, "{name:?}");
                    assert!(name.bytes().all(|b| (0x21..=0x7E).contains(&b)));
                }
            }
        }
    }
}

#[test]
fn every_prefix_and_every_byte_change_of_valid_queries_is_survived() {
    for base in base_queries() {
        for len in 0..=base.len() {
            check_parsed(&base[..len]);
        }
        for at in 0..base.len() {
            for value in 0..=255u8 {
                let mut m = base.clone();
                m[at] = value;
                check_parsed(&m);
            }
        }
    }
}

#[test]
fn random_garbage_and_mutations_never_panic_and_responses_stay_well_formed() {
    let mut rng = Rng::new(0xD115_0002);
    let bases = base_queries();
    let mut served = 0u32;
    for round in 0..60_000u32 {
        let mut m = if round % 4 == 0 {
            let mut m = vec![0u8; rng.below(560)];
            rng.fill(&mut m);
            m
        } else {
            bases[rng.below(bases.len())].clone()
        };
        for _ in 0..rng.below(5) {
            if !m.is_empty() {
                let at = rng.below(m.len());
                m[at] = rng.next() as u8;
            }
        }
        if rng.below(6) == 0 {
            m.truncate(rng.below(m.len() + 1));
        }
        if let Parsed::Query(_) = dns::parse(&m) {
            served += 1;
        }
        check_parsed(&m);
    }
    assert!(
        served > 1000,
        "the generator reaches the serving path ({served})"
    );
}

#[test]
fn a_garbage_stream_through_the_backend_is_accounted_exactly() {
    let mut rng = Rng::new(0xD115_0003);
    let mut b = backend();
    let bases = base_queries();
    let mut sent = 0u64;
    for _ in 0..5000 {
        let mut m = bases[rng.below(bases.len())].clone();
        for _ in 0..1 + rng.below(3) {
            let at = rng.below(m.len());
            m[at] = rng.next() as u8;
        }
        let frame = dns_frame(PORT, &m);
        b.transmit(&frame).unwrap();
        sent += 1;
        if rng.below(4) == 0 {
            b.process_pending();
            drain(&mut b);
        }
    }
    b.process_pending();
    let c = b.counters();
    assert_eq!(c.tx_frames, sent);
    // Every frame was a UDP datagram to port 53 of the DNS address, so it ended in one of these
    // (a lookup that has not finished yet is counted as a query, its answer comes later).
    assert_eq!(
        sent,
        c.dns_queries
            + c.dns_malformed
            + c.dns_formerr
            + c.dns_dropped_busy
            + c.drop_ip_malformed
            + c.drop_l4_malformed
    );
}
