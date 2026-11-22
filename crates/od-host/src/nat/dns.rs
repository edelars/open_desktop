//! DNS forwarder of the NAT backend (S11-31b, NETWORK.md section 8).
//!
//! Two parts. The wire part ([`parse`], [`plan`], the `build_*` functions) reads one query of the guest
//! and writes responses; it indexes only after a length check and never panics. The service part
//! ([`DnsService`]) owns the eight query slots, the worker threads (at most four) and the five-second
//! timeout; the workers call a [`Resolver`] and nothing on the device thread ever waits for it.
//!
//! Decisions for what the document leaves open (reported in the task result):
//! * a message with QR set (a response) gets no answer and is counted as malformed;
//! * a query with an opcode other than 0 gets NOTIMP (RFC 1035; the owner has not confirmed it);
//! * a query with a question that cannot be read gets FORMERR;
//! * a FORMERR response carries no question and no `OPT`;
//! * a name with a byte outside printable ASCII or a dot inside a label, and the root name, cannot be
//!   passed to the system resolver and get NXDOMAIN without calling it;
//! * only A queries need a slot: AAAA, NOTIMP and BADVERS answers are immediate and never "busy".

use std::collections::VecDeque;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use super::resolver::{ResolveOutcome, Resolver};

/// L16: the largest query and response (UDP data), the slots, the timeout, the records and the TTL.
pub const MAX_MESSAGE: usize = 512;
pub const MAX_SLOTS: usize = 8;
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(5);
pub const MAX_ANSWERS: usize = 16;
pub const ANSWER_TTL: u32 = 30;
/// L18: worker threads of the resolver.
pub const MAX_WORKERS: usize = 4;

const HEADER_LEN: usize = 12;
const MAX_NAME_WIRE: usize = 255;
const MAX_LABEL: usize = 63;
const TYPE_A: u16 = 1;
const TYPE_AAAA: u16 = 28;
const TYPE_OPT: u16 = 41;
const CLASS_IN: u16 = 1;
const RCODE_NOERROR: u8 = 0;
const RCODE_FORMERR: u8 = 1;
const RCODE_SERVFAIL: u8 = 2;
const RCODE_NXDOMAIN: u8 = 3;
const RCODE_NOTIMP: u8 = 4;
/// Extended RCODE of BADVERS (16): the high eight bits (1) go into the `OPT` record, the low four are 0.
const EXT_RCODE_BADVERS_HIGH: u8 = 1;
/// `OPT` record of a response: root name, type 41, UDP size 512, extended RCODE, version 0, flags 0, no data.
const OPT_RECORD_LEN: usize = 11;
const ANSWER_LEN: usize = 16;

fn be16(bytes: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([bytes[at], bytes[at + 1]])
}

/// What the `OPT` record of a query asked for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Edns {
    /// No `OPT`: the response has none.
    None,
    /// One `OPT` of version 0: the response has a minimal `OPT`.
    V0,
    /// One `OPT` of another version: BADVERS.
    BadVersion,
}

/// A query the forwarder understood.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Query {
    pub id: u16,
    pub rd: bool,
    pub qtype: u16,
    pub edns: Edns,
    /// The question exactly as the guest sent it: name in wire format, type and class.
    question: Vec<u8>,
    /// The labels joined by dots (no trailing dot); empty for the root name.
    name: Vec<u8>,
}

/// Result of [`parse`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Parsed {
    /// No answer: shorter than a header, longer than 512 bytes, or a response (QR set).
    Malformed,
    /// The header was read but the query is not one we serve: FORMERR.
    Formerr {
        id: u16,
        rd: bool,
    },
    /// An opcode other than 0 (QUERY): NOTIMP, with the opcode copied (RFC 1035, section 4.1.1).
    Notimp {
        id: u16,
        rd: bool,
        opcode: u8,
    },
    Query(Query),
}

/// Reads the name of the question at `at`: no compression, labels of at most 63 bytes, at most 255
/// bytes in the wire format. Returns the dotted name and the offset after the root label.
fn read_question_name(data: &[u8], mut at: usize) -> Option<(Vec<u8>, usize)> {
    let start = at;
    let mut dotted = Vec::new();
    loop {
        let len = usize::from(*data.get(at)?);
        at += 1;
        if len == 0 {
            break;
        }
        // 0xC0 is a compression pointer, 0x40 and 0x80 are reserved: none is allowed in the question.
        if len > MAX_LABEL {
            return None;
        }
        let label = data.get(at..at + len)?;
        at += len;
        if !dotted.is_empty() {
            dotted.push(b'.');
        }
        dotted.extend_from_slice(label);
    }
    // The wire format counts the length bytes and the final root byte.
    if at - start > MAX_NAME_WIRE {
        return None;
    }
    Some((dotted, at))
}

/// What the forwarder needs from one resource record of the later sections.
struct Record {
    rtype: u16,
    ttl_field: u32,
    root_name: bool,
    end: usize,
}

/// Skips one resource record (name, 10 bytes of fixed fields, data); `None` when it is cut short.
fn read_record(data: &[u8], mut at: usize) -> Option<Record> {
    let root_name = *data.get(at)? == 0;
    // Names in the later sections may be compressed (a pointer ends the name).
    loop {
        let len = *data.get(at)?;
        if len & 0xC0 == 0xC0 {
            at += 2;
            break;
        }
        if len & 0xC0 != 0 {
            return None;
        }
        at += 1;
        if len == 0 {
            break;
        }
        at += usize::from(len);
    }
    let fixed = data.get(at..at.checked_add(10)?)?;
    let rtype = be16(fixed, 0);
    let ttl_field = u32::from_be_bytes([fixed[4], fixed[5], fixed[6], fixed[7]]);
    let rdlen = usize::from(be16(fixed, 8));
    let end = at.checked_add(10)?.checked_add(rdlen)?;
    if end > data.len() {
        return None;
    }
    Some(Record {
        rtype,
        ttl_field,
        root_name,
        end,
    })
}

/// Reads one query (the data of a UDP datagram to port 53).
pub fn parse(data: &[u8]) -> Parsed {
    if data.len() < HEADER_LEN || data.len() > MAX_MESSAGE {
        return Parsed::Malformed;
    }
    let id = be16(data, 0);
    let flags = be16(data, 2);
    if flags & 0x8000 != 0 {
        return Parsed::Malformed;
    }
    let rd = flags & 0x0100 != 0;
    let formerr = Parsed::Formerr { id, rd };
    let opcode = (flags >> 11) & 0xF;
    let (qdcount, ancount, nscount, arcount) = (
        be16(data, 4),
        usize::from(be16(data, 6)),
        usize::from(be16(data, 8)),
        usize::from(be16(data, 10)),
    );
    if opcode != 0 {
        return Parsed::Notimp {
            id,
            rd,
            opcode: opcode as u8,
        };
    }
    if qdcount != 1 {
        return formerr;
    }
    let Some((name, name_end)) = read_question_name(data, HEADER_LEN) else {
        return formerr;
    };
    let Some(type_class) = data.get(name_end..name_end + 4) else {
        return formerr;
    };
    let qtype = be16(type_class, 0);
    if be16(type_class, 2) != CLASS_IN {
        return formerr;
    }
    let question = data[HEADER_LEN..name_end + 4].to_vec();

    // The later sections: only the number and version of `OPT` records matter.
    let mut at = name_end + 4;
    let mut opts = 0;
    let mut edns = Edns::None;
    for index in 0..ancount.saturating_add(nscount).saturating_add(arcount) {
        let Some(record) = read_record(data, at) else {
            return formerr;
        };
        at = record.end;
        if index >= ancount + nscount && record.rtype == TYPE_OPT {
            // RFC 6891, section 6.1.2: the name of an OPT record is the root.
            if !record.root_name {
                return formerr;
            }
            opts += 1;
            let version = (record.ttl_field >> 16) as u8;
            edns = if version == 0 {
                Edns::V0
            } else {
                Edns::BadVersion
            };
        }
    }
    if opts > 1 {
        return formerr;
    }
    Parsed::Query(Query {
        id,
        rd,
        qtype,
        edns,
        question,
        name,
    })
}

impl Query {
    /// The name as a host name for the system resolver, or `None` when no such name can exist: the
    /// root name, a byte outside printable ASCII, or a dot inside a label.
    pub fn host_name(&self) -> Option<String> {
        if self.name.is_empty() {
            return None;
        }
        // The dotted form is ambiguous for a label that itself contains a dot: check the wire form.
        let wire = &self.question[..self.question.len() - 4];
        let mut at = 0;
        while at < wire.len() {
            let len = usize::from(wire[at]);
            at += 1;
            let label = wire.get(at..at + len)?;
            if label
                .iter()
                .any(|b| !(0x21..=0x7E).contains(b) || *b == b'.')
            {
                return None;
            }
            at += len;
        }
        String::from_utf8(self.name.clone()).ok()
    }
}

/// How a response is counted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplyKind {
    Plain,
    NxDomain,
    ServFail,
    Formerr,
}

fn push_header(out: &mut Vec<u8>, id: u16, rd: bool, rcode: u8, qd: u16, an: u16, ar: u16) {
    out.extend_from_slice(&id.to_be_bytes());
    // QR, opcode 0, AA 0, TC 0, RD copied, RA 1.
    let flags = 0x8000 | (u16::from(rd) << 8) | 0x0080 | u16::from(rcode);
    out.extend_from_slice(&flags.to_be_bytes());
    out.extend_from_slice(&qd.to_be_bytes());
    out.extend_from_slice(&an.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&ar.to_be_bytes());
}

fn push_opt(out: &mut Vec<u8>, extended_rcode: u8) {
    out.push(0);
    out.extend_from_slice(&TYPE_OPT.to_be_bytes());
    out.extend_from_slice(&(MAX_MESSAGE as u16).to_be_bytes());
    // Extended RCODE, version 0, flags 0.
    out.extend_from_slice(&[extended_rcode, 0, 0, 0]);
    out.extend_from_slice(&0u16.to_be_bytes());
}

/// FORMERR: the header only, no question, no `OPT`.
pub fn build_formerr(id: u16, rd: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN);
    push_header(&mut out, id, rd, RCODE_FORMERR, 0, 0, 0);
    out
}

/// NOTIMP for an opcode other than QUERY: the header only, the opcode copied from the query.
pub fn build_notimp(id: u16, rd: bool, opcode: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN);
    push_header(&mut out, id, rd, RCODE_NOTIMP, 0, 0, 0);
    // The opcode sits in bits 14..11 of the flags, bits 6..3 of the first flag byte.
    out[2] |= (opcode & 0xF) << 3;
    out
}

/// A response to a parsed query: the question is repeated, `addrs` become A records (the caller
/// has filtered them; at most 16 and at most what fits into 512 bytes are written).
fn build_response(query: &Query, rcode: u8, extended_rcode: u8, addrs: &[Ipv4Addr]) -> Vec<u8> {
    let with_opt = query.edns != Edns::None;
    let fixed = HEADER_LEN + query.question.len() + if with_opt { OPT_RECORD_LEN } else { 0 };
    let room = MAX_MESSAGE.saturating_sub(fixed) / ANSWER_LEN;
    let addrs = &addrs[..addrs.len().min(MAX_ANSWERS).min(room)];
    let mut out = Vec::with_capacity(fixed + addrs.len() * ANSWER_LEN);
    push_header(
        &mut out,
        query.id,
        query.rd,
        rcode,
        1,
        addrs.len() as u16,
        u16::from(with_opt),
    );
    out.extend_from_slice(&query.question);
    for addr in addrs {
        // Name: a pointer to the question (offset 12); type A, class IN, TTL, 4 bytes of data.
        out.extend_from_slice(&[0xC0, 0x0C]);
        out.extend_from_slice(&TYPE_A.to_be_bytes());
        out.extend_from_slice(&CLASS_IN.to_be_bytes());
        out.extend_from_slice(&ANSWER_TTL.to_be_bytes());
        out.extend_from_slice(&4u16.to_be_bytes());
        out.extend_from_slice(&addr.octets());
    }
    if with_opt {
        push_opt(&mut out, extended_rcode);
    }
    out
}

/// Addresses the backend would drop on the way back to the guest are not offered (NETWORK.md
/// section 8): 127/8, 0/8, 169.254/16, the subnet 10.0.2.0/24, 224/4, 240/4 (with 255.255.255.255).
pub fn offerable(addr: Ipv4Addr) -> bool {
    let o = addr.octets();
    !(o[0] == 127
        || o[0] == 0
        || (o[0] == 169 && o[1] == 254)
        || o[..3] == [10, 0, 2]
        || o[0] >= 224)
}

/// What to do with a parsed query.
#[derive(Debug, Eq, PartialEq)]
pub enum Plan {
    /// Answer at once.
    Immediate(Vec<u8>, ReplyKind),
    /// Look the name up (type A).
    Resolve(String),
}

pub fn plan(query: &Query) -> Plan {
    if query.edns == Edns::BadVersion {
        // BADVERS (16): RCODE 0 in the header, 1 in the extended part of the OPT record.
        return Plan::Immediate(
            build_response(query, RCODE_NOERROR, EXT_RCODE_BADVERS_HIGH, &[]),
            ReplyKind::Plain,
        );
    }
    match query.qtype {
        TYPE_AAAA => Plan::Immediate(
            build_response(query, RCODE_NOERROR, 0, &[]),
            ReplyKind::Plain,
        ),
        TYPE_A => match query.host_name() {
            Some(name) => Plan::Resolve(name),
            None => Plan::Immediate(
                build_response(query, RCODE_NXDOMAIN, 0, &[]),
                ReplyKind::NxDomain,
            ),
        },
        _ => Plan::Immediate(
            build_response(query, RCODE_NOTIMP, 0, &[]),
            ReplyKind::Plain,
        ),
    }
}

/// The response to a finished lookup.
fn build_answer(query: &Query, outcome: &ResolveOutcome) -> (Vec<u8>, ReplyKind) {
    match outcome {
        ResolveOutcome::Addrs(addrs) => {
            let mut kept: Vec<Ipv4Addr> = Vec::new();
            for addr in addrs {
                if offerable(*addr) && !kept.contains(addr) {
                    kept.push(*addr);
                }
            }
            (
                build_response(query, RCODE_NOERROR, 0, &kept),
                ReplyKind::Plain,
            )
        }
        ResolveOutcome::NoData => (
            build_response(query, RCODE_NOERROR, 0, &[]),
            ReplyKind::Plain,
        ),
        ResolveOutcome::NotFound => (
            build_response(query, RCODE_NXDOMAIN, 0, &[]),
            ReplyKind::NxDomain,
        ),
        ResolveOutcome::Failed => (
            build_response(query, RCODE_SERVFAIL, 0, &[]),
            ReplyKind::ServFail,
        ),
    }
}

// ---- Service -------------------------------------------------------------------------------------

struct Job {
    token: u64,
    name: String,
    /// Set when the slot is gone (timeout, reset): a worker that took the job just before it was
    /// removed from the queue skips it.
    cancel: Arc<AtomicBool>,
}

struct Done {
    token: u64,
    outcome: ResolveOutcome,
}

struct Pending {
    token: u64,
    query: Query,
    src_port: u16,
    deadline: Instant,
    cancel: Arc<AtomicBool>,
}

/// An answer ready for the guest.
#[derive(Debug, Eq, PartialEq)]
pub struct Completed {
    /// The guest port the query came from.
    pub dst_port: u16,
    pub response: Vec<u8>,
    pub kind: ReplyKind,
}

/// Why a query could not be accepted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubmitError {
    /// All eight slots are taken.
    Busy,
    /// No worker thread exists and none could be started.
    NoWorker,
}

/// Jobs that no worker has taken yet. A job leaves the queue when a worker takes it, when its slot
/// times out and when the service is reset, so the queue never holds more than [`MAX_SLOTS`] jobs
/// (L16) however long the workers hang.
struct JobQueue {
    jobs: VecDeque<Job>,
    /// Workers that wait for a job, or have been started and not yet looked at the queue. A new
    /// worker is started only when the queued jobs outnumber them (and fewer than [`MAX_WORKERS`]
    /// exist), so a worker stuck in `getaddrinfo` never keeps a later query waiting.
    idle: usize,
    closed: bool,
}

struct Shared {
    queue: Mutex<JobQueue>,
    wake: Condvar,
    alive: AtomicUsize,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, JobQueue> {
        // The queue holds plain data and is never left half-updated: a poisoned lock is still usable.
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Counts a live worker thread; the count drops when the thread ends, however it ends.
struct AliveGuard(Arc<Shared>);

impl Drop for AliveGuard {
    fn drop(&mut self) {
        self.0.alive.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct DnsService {
    resolver: Arc<dyn Resolver>,
    slots: [Option<Pending>; MAX_SLOTS],
    next_token: u64,
    shared: Arc<Shared>,
    done_tx: Sender<Done>,
    done_rx: Receiver<Done>,
}

impl DnsService {
    pub fn new(resolver: Arc<dyn Resolver>) -> Self {
        let (done_tx, done_rx) = mpsc::channel();
        Self {
            resolver,
            slots: Default::default(),
            next_token: 0,
            shared: Arc::new(Shared {
                queue: Mutex::new(JobQueue {
                    jobs: VecDeque::new(),
                    idle: 0,
                    closed: false,
                }),
                wake: Condvar::new(),
                alive: AtomicUsize::new(0),
            }),
            done_tx,
            done_rx,
        }
    }

    pub fn occupied(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    /// Worker threads that are alive now (at most [`MAX_WORKERS`]).
    pub fn workers(&self) -> usize {
        self.shared.alive.load(Ordering::SeqCst)
    }

    /// Workers that are waiting for a job (or just started). Equal to [`Self::workers`] when no
    /// lookup is running and every finished lookup has handed in its result.
    pub fn idle_workers(&self) -> usize {
        self.shared.lock().idle
    }

    /// Jobs no worker has taken yet.
    pub fn queued(&self) -> usize {
        self.shared.lock().jobs.len()
    }

    /// Takes a type A query into a slot. Never blocks (the queue lock is held only for a push).
    pub fn submit(
        &mut self,
        query: Query,
        name: String,
        src_port: u16,
        now: Instant,
    ) -> Result<(), SubmitError> {
        let Some(free) = self.slots.iter().position(Option::is_none) else {
            return Err(SubmitError::Busy);
        };
        let token = self.next_token;
        self.next_token += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        let need_worker = {
            let mut queue = self.shared.lock();
            queue.jobs.push_back(Job {
                token,
                name,
                cancel: Arc::clone(&cancel),
            });
            let need = queue.jobs.len() > queue.idle && self.workers() < MAX_WORKERS;
            if need {
                // The new worker is idle from the start.
                queue.idle += 1;
            }
            need
        };
        if need_worker && !self.spawn_worker() {
            let mut queue = self.shared.lock();
            queue.idle = queue.idle.saturating_sub(1);
            // Without any worker the query would only wait for its timeout: refuse it.
            if self.workers() == 0 {
                queue.jobs.retain(|job| job.token != token);
                return Err(SubmitError::NoWorker);
            }
        }
        self.shared.wake.notify_one();
        self.slots[free] = Some(Pending {
            token,
            query,
            src_port,
            deadline: now + QUERY_TIMEOUT,
            cancel,
        });
        Ok(())
    }

    /// Starts one worker thread; false when the system refused.
    fn spawn_worker(&self) -> bool {
        self.shared.alive.fetch_add(1, Ordering::SeqCst);
        let guard = AliveGuard(Arc::clone(&self.shared));
        let resolver = Arc::clone(&self.resolver);
        let done = self.done_tx.clone();
        // On failure the closure (and with it the guard) is dropped: the count is back already.
        thread::Builder::new()
            .name("od-nat-dns".into())
            .spawn(move || worker(guard, resolver, done))
            .is_ok()
    }

    /// Removes a slot's job from the queue (if no worker took it yet) and tells a worker that took
    /// it to skip it.
    fn cancel(&self, pending: &Pending) {
        pending.cancel.store(true, Ordering::SeqCst);
        self.shared
            .lock()
            .jobs
            .retain(|job| job.token != pending.token);
    }

    /// Collects finished lookups and answers the queries whose five seconds have passed with SERVFAIL.
    /// A result for a slot that no longer exists (timeout, reset) is dropped. Never blocks.
    pub fn poll(&mut self, now: Instant) -> Vec<Completed> {
        let mut completed = Vec::new();
        while let Ok(done) = self.done_rx.try_recv() {
            let Some(slot) = self
                .slots
                .iter_mut()
                .find(|s| s.as_ref().is_some_and(|p| p.token == done.token))
            else {
                continue;
            };
            if let Some(pending) = slot.take() {
                let (response, kind) = build_answer(&pending.query, &done.outcome);
                completed.push(Completed {
                    dst_port: pending.src_port,
                    response,
                    kind,
                });
            }
        }
        for index in 0..MAX_SLOTS {
            if self.slots[index]
                .as_ref()
                .is_some_and(|p| now >= p.deadline)
                && let Some(pending) = self.slots[index].take()
            {
                self.cancel(&pending);
                let (response, kind) = build_answer(&pending.query, &ResolveOutcome::Failed);
                completed.push(Completed {
                    dst_port: pending.src_port,
                    response,
                    kind,
                });
            }
        }
        completed
    }

    /// Forgets every query without an answer (reset of the NIC or the machine). A lookup that is
    /// still running finishes on its worker and its result is dropped.
    pub fn reset(&mut self) {
        for index in 0..MAX_SLOTS {
            if let Some(pending) = self.slots[index].take() {
                self.cancel(&pending);
            }
        }
    }
}

impl Drop for DnsService {
    fn drop(&mut self) {
        self.reset();
        // Idle workers end now; one inside `getaddrinfo` ends when it returns.
        self.shared.lock().closed = true;
        self.shared.wake.notify_all();
    }
}

fn worker(guard: AliveGuard, resolver: Arc<dyn Resolver>, done: Sender<Done>) {
    let shared = Arc::clone(&guard.0);
    // The first pass starts idle (counted by whoever started the worker).
    let mut counted_idle = true;
    loop {
        let job = {
            let mut queue = shared.lock();
            if !counted_idle {
                queue.idle += 1;
            }
            counted_idle = false;
            loop {
                if let Some(job) = queue.jobs.pop_front() {
                    queue.idle -= 1;
                    break job;
                }
                if queue.closed {
                    queue.idle -= 1;
                    return;
                }
                queue = shared
                    .wake
                    .wait(queue)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        if job.cancel.load(Ordering::SeqCst) {
            continue;
        }
        let outcome = resolver.resolve_a(&job.name);
        if done
            .send(Done {
                token: job.token,
                outcome,
            })
            .is_err()
        {
            return;
        }
    }
}
