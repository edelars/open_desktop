//! S7-32: the profile of the instructions the turbo engine leaves to the general step (opt-in:
//! `TurboEngine::enable_profile`, the runner's `--turbo-profile`).
//!
//! It observes only. Every slow exit is counted under its RIP, the reason the engine left the
//! instruction (no template, a template that refused at run time and why, no code at the RIP) and
//! the CPL; the host time from the end of that run to the start of the next one (the general step
//! and the loop around it) is added to the same site, and the time inside runs and after the other
//! exits is summed. At the report the sites are grouped by form: the `Operation` variant, the kind
//! of the operands and the operand size (the classification of the PERF-20 histogram), and the
//! FS/GS, LOCK, REP and address-size prefixes.
//!
//! Without the profile the engine pays two tests of an `Option` per run (at its start and its
//! end), one per general step (`note_general_step`) and one per call of the REP helper, and on the
//! refusal paths of its helpers (cold next to the general step that follows them) the store of
//! the reason and one test. The profile reads the host clock twice per run: its times are shares,
//! not a speed measurement.

use super::{HashMap, TurboExit};
use od_isa::long_decode::Instruction;
use std::time::Instant;

/// Why generated code left an instruction it has a template for (the last refusal of a helper, or
/// a check of the code itself).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// A check of generated code: a non-canonical target, a selector the context does not model,
    /// #DE, an STI in the shadow of another one, the closed gate of the SSE forms (no FP member,
    /// CR0.EM, CR0.TS, CR4.OSFXSR clear).
    #[default]
    Check = 0,
    /// The access crosses a page.
    CrossPage = 1,
    /// A write or a locked access to a page-table frame or to bytes of translated code.
    Watched = 2,
    /// The address is not RAM (MMIO, a hole).
    NotRam = 3,
    /// The walk faults (#PF, a non-canonical address).
    Fault = 4,
    /// Another refusal of the walk (a table outside RAM, a page size it does not take).
    Walk = 5,
    /// The REP helper refused (DF = 1, an element crossing a page, a page the fast walk does not
    /// cover, a written table frame).
    Rep = 6,
}

const REFUSALS: usize = 7;

impl Refusal {
    fn name(self) -> &'static str {
        match self {
            Refusal::Check => "check",
            Refusal::CrossPage => "cross-page",
            Refusal::Watched => "watched",
            Refusal::NotRam => "not-ram",
            Refusal::Fault => "fault",
            Refusal::Walk => "walk",
            Refusal::Rep => "rep",
        }
    }

    fn from_index(index: usize) -> Refusal {
        [
            Refusal::Check,
            Refusal::CrossPage,
            Refusal::Watched,
            Refusal::NotRam,
            Refusal::Fault,
            Refusal::Walk,
            Refusal::Rep,
        ][index]
    }

    /// The refusal of a failed tracked walk or access.
    pub(crate) fn of(error: &crate::page_walk::TrackedAccessError) -> Refusal {
        use crate::page_walk::TrackedAccessError as E;
        match error {
            E::CrossesPage { .. } => Refusal::CrossPage,
            E::NonCanonical(_) | E::PageFault { .. } => Refusal::Fault,
            E::OperandPhysical { .. } | E::DeviceUnavailable { .. } | E::MmioBackend { .. } => {
                Refusal::NotRam
            }
            _ => Refusal::Walk,
        }
    }
}

/// Why the engine left the instruction at a RIP to the general step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlowReason {
    /// The translator has no template for the instruction (`plan` refuses it).
    NoTemplate,
    /// A template exists; generated code left before the instruction at run time.
    Refused(Refusal),
    /// The engine has no code at the RIP: the fetch needs the general path (a fault, not RAM), or
    /// the instruction did not decode where the engine looked.
    NoCode,
}

impl SlowReason {
    fn code(self) -> u8 {
        match self {
            SlowReason::NoTemplate => 0,
            SlowReason::NoCode => 1,
            SlowReason::Refused(refusal) => 8 + refusal as u8,
        }
    }

    fn name(code: u8) -> String {
        match code {
            0 => "no-template".to_owned(),
            1 => "no-code".to_owned(),
            2 => "not-admitted".to_owned(),
            _ => format!(
                "refused:{}",
                Refusal::from_index(usize::from(code - 8)).name()
            ),
        }
    }
}

/// The reason code of a general step the engine was not asked to run (CPL3, TF/RF, the shadow of
/// MOV SS, a CPU that does not run).
const NOT_ADMITTED: u8 = 2;

struct Site {
    instruction: Option<Instruction>,
    count: u64,
    nanos: u64,
}

/// The counters of the profile.
pub(crate) struct SlowProfile {
    /// (RIP, reason code | CPL << 8) -> the instructions seen there (one unless code changed).
    sites: HashMap<(u64, u16), Vec<Site>>,
    /// The site the time up to the next run belongs to, with the instant the last run ended.
    pending: Option<((u64, u16), usize, Instant)>,
    /// Whether the general step of the pending slow exit is still to come.
    armed: bool,
    /// When the last run started; when the last run that did not end slow ended.
    entered: Option<Instant>,
    left_other: Option<Instant>,
    /// Host time inside runs; after slow exits up to the next run; after other exits up to the
    /// next run.
    in_runs: u64,
    after_slow: u64,
    after_other: u64,
    runs_by_exit: [u64; 5],
    /// Refusals of the memory helper, by `Refusal`.
    mem_refusals: [u64; REFUSALS],
    /// The linear addresses of the not-RAM refusals (the device registers generated code reaches).
    not_ram: HashMap<u64, u64>,
    /// REP helper: refused calls, calls that stopped at the iteration limit, host time inside.
    pub(crate) rep_refused: u64,
    pub(crate) rep_partial: u64,
    pub(crate) rep_nanos: u64,
}

fn nanos(from: Instant, to: Instant) -> u64 {
    to.saturating_duration_since(from).as_nanos() as u64
}

impl SlowProfile {
    pub(crate) fn new() -> Self {
        Self {
            sites: HashMap::default(),
            pending: None,
            armed: false,
            entered: None,
            left_other: None,
            in_runs: 0,
            after_slow: 0,
            after_other: 0,
            runs_by_exit: [0; 5],
            mem_refusals: [0; REFUSALS],
            not_ram: HashMap::default(),
            rep_refused: 0,
            rep_partial: 0,
            rep_nanos: 0,
        }
    }

    /// A run starts: the time since the last run ended goes to its slow site or to the other
    /// exits.
    pub(crate) fn enter(&mut self) {
        self.armed = false;
        let now = Instant::now();
        if let Some((key, index, left)) = self.pending.take() {
            let spent = nanos(left, now);
            self.after_slow += spent;
            if let Some(site) = self
                .sites
                .get_mut(&key)
                .and_then(|sites| sites.get_mut(index))
            {
                site.nanos += spent;
            }
        } else if let Some(left) = self.left_other.take() {
            self.after_other += nanos(left, now);
        }
        self.entered = Some(now);
    }

    /// A run ended with `exit`; a slow exit at `rip` (CPL `cpl`) for `reason`, with the decode of
    /// the instruction when the engine has one.
    pub(crate) fn leave(
        &mut self,
        exit: TurboExit,
        slow: Option<(u64, u8, SlowReason, Option<Instruction>)>,
    ) {
        let now = Instant::now();
        if let Some(entered) = self.entered.take() {
            self.in_runs += nanos(entered, now);
        }
        self.runs_by_exit[match exit {
            TurboExit::Budget => 0,
            TurboExit::Slow => 1,
            TurboExit::Rdtsc => 2,
            TurboExit::RepPartial => 3,
            TurboExit::DeviceError => 4,
        }] += 1;
        match slow {
            Some((rip, cpl, reason, instruction)) => {
                let key = (rip, u16::from(reason.code()) | u16::from(cpl & 3) << 8);
                let index = self.count(key, instruction);
                self.pending = Some((key, index, now));
                self.armed = true;
            }
            None => self.left_other = Some(now),
        }
    }

    /// The memory helper refused an access at linear `address` for `refusal`.
    pub(crate) fn mem_refusal(&mut self, refusal: Refusal, address: u64) {
        self.mem_refusals[refusal as usize] += 1;
        if refusal == Refusal::NotRam {
            *self.not_ram.entry(address).or_default() += 1;
        }
    }

    /// The loop runs a general step at `rip` (CPL `cpl`): the one of the last slow exit, or one
    /// the engine was not asked to run.
    pub(crate) fn general_step(&mut self, rip: u64, cpl: u8) {
        if self.armed {
            self.armed = false;
            return;
        }
        self.count(
            (rip, u16::from(NOT_ADMITTED) | u16::from(cpl & 3) << 8),
            None,
        );
    }

    fn count(&mut self, key: (u64, u16), instruction: Option<Instruction>) -> usize {
        let sites = self.sites.entry(key).or_default();
        let index = match sites
            .iter()
            .position(|site| site.instruction == instruction)
        {
            Some(index) => index,
            None => {
                sites.push(Site {
                    instruction,
                    count: 0,
                    nanos: 0,
                });
                sites.len() - 1
            }
        };
        sites[index].count += 1;
        index
    }

    /// The profile as text: the totals, the forms by host time, the RIPs by host time.
    pub(crate) fn report(&self, top: usize) -> String {
        use std::fmt::Write;
        struct Row {
            count: u64,
            nanos: u64,
        }
        let mut forms: HashMap<String, Row> = HashMap::default();
        let mut rips: Vec<(u64, String, u64, u64)> = Vec::new();
        let mut total = 0u64;
        for (&(rip, code), sites) in &self.sites {
            for site in sites {
                total += site.count;
                let form = format!(
                    "{} cpl{} {}",
                    SlowReason::name(code as u8),
                    code >> 8,
                    form_of(site.instruction.as_ref())
                );
                let row = forms
                    .entry(form.clone())
                    .or_insert(Row { count: 0, nanos: 0 });
                row.count += site.count;
                row.nanos += site.nanos;
                rips.push((rip, form, site.count, site.nanos));
            }
        }
        let mut forms: Vec<(String, Row)> = forms.into_iter().collect();
        forms.sort_by(|a, b| {
            (b.1.nanos, b.1.count)
                .cmp(&(a.1.nanos, a.1.count))
                .then_with(|| a.0.cmp(&b.0))
        });
        rips.sort_by(|a, b| (b.3, b.2, a.0).cmp(&(a.3, a.2, b.0)));
        let seconds = |nanos: u64| nanos as f64 / 1e9;
        let share = |part: u64, whole: u64| {
            if whole == 0 {
                0.0
            } else {
                100.0 * part as f64 / whole as f64
            }
        };
        let measured = self.in_runs + self.after_slow + self.after_other;
        let mut out = String::new();
        let order = "by host time after the exit";
        let _ = writeln!(
            out,
            "turbo profile: host {:.2} s = in runs {:.2} s ({:.1} %) + after slow exits {:.2} s \
             ({:.1} %) + after other exits {:.2} s ({:.1} %); runs by exit: budget {} slow {} \
             rdtsc {} rep-partial {} device-error {}; general steps {}",
            seconds(measured),
            seconds(self.in_runs),
            share(self.in_runs, measured),
            seconds(self.after_slow),
            share(self.after_slow, measured),
            seconds(self.after_other),
            share(self.after_other, measured),
            self.runs_by_exit[0],
            self.runs_by_exit[1],
            self.runs_by_exit[2],
            self.runs_by_exit[3],
            self.runs_by_exit[4],
            total,
        );
        let _ = write!(out, "turbo profile: memory helper refusals:");
        for (index, count) in self.mem_refusals.iter().enumerate() {
            let _ = write!(out, " {} {}", Refusal::from_index(index).name(), count);
        }
        let _ = writeln!(
            out,
            "; rep helper: refused {} partial {} host {:.2} s",
            self.rep_refused,
            self.rep_partial,
            seconds(self.rep_nanos)
        );
        let mut not_ram: Vec<(u64, u64)> = self.not_ram.iter().map(|(&a, &c)| (c, a)).collect();
        not_ram.sort_by(|a, b| b.cmp(a));
        let _ = write!(out, "turbo profile: not-RAM addresses (top 16):");
        for (count, address) in not_ram.iter().take(16) {
            let _ = write!(out, " {address:#x} {count}");
        }
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "turbo profile: forms {order} (top {top}): time% count% count ns/exit form"
        );
        for (form, row) in forms.iter().take(top) {
            let _ = writeln!(
                out,
                "  {:5.1} {:5.1} {:>11} {:>7} {}",
                share(row.nanos, self.after_slow),
                share(row.count, total),
                row.count,
                row.nanos / row.count.max(1),
                form
            );
        }
        let _ = writeln!(
            out,
            "turbo profile: RIPs {order} (top {top}): time% count rip form"
        );
        for (rip, form, count, nanos) in rips.iter().take(top) {
            let _ = writeln!(
                out,
                "  {:5.1} {:>11} {:#018x} {}",
                share(*nanos, self.after_slow),
                count,
                rip,
                form
            );
        }
        out
    }
}

/// The form of a decoded instruction: the PERF-20 classification and its FS/GS, LOCK, REP and
/// 32-bit address prefixes.
fn form_of(instruction: Option<&Instruction>) -> String {
    let Some(instruction) = instruction else {
        return "<no decode>".to_owned();
    };
    let (operation, operands, bytes) = crate::op_histogram::classify(instruction);
    let prefixes = instruction.prefixes;
    let mut form = format!("{operation} {operands} {bytes}");
    match prefixes.segment {
        Some(0x64) => form.push_str(" fs"),
        Some(0x65) => form.push_str(" gs"),
        _ => {}
    }
    match prefixes.group1 {
        Some(0xf0) => form.push_str(" lock"),
        Some(0xf2) => form.push_str(" f2"),
        Some(0xf3) => form.push_str(" f3"),
        _ => {}
    }
    if prefixes.address_override {
        form.push_str(" a32");
    }
    let map = match instruction.opcode.map {
        od_isa::long_decode::OpcodeMap::Primary => "",
        od_isa::long_decode::OpcodeMap::Escape0f => "0f ",
        od_isa::long_decode::OpcodeMap::Escape0f38 => "0f38 ",
        od_isa::long_decode::OpcodeMap::Escape0f3a => "0f3a ",
    };
    let _ = std::fmt::Write::write_fmt(
        &mut form,
        format_args!(" [{map}{:02x}]", instruction.opcode.byte),
    );
    form
}
