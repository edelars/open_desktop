//! S4-38L4i: the opt-in checkpoint recorder of the Linux CPL0-set loop (checkpoints C0..C17 of
//! `docs/plans/STAGE_4.md`, S4-38 L4).
//!
//! The recorder only OBSERVES. The loop hands it one [`BoundaryView`] per instruction boundary
//! (after the stop, budget and interrupt checks, before the CPUID boundary and the step); nothing
//! the recorder does reaches the CPU state, the memory, the clock or the exit of the run, so a run
//! with the recorder retires the same instructions in the same quanta as the run without it. The
//! board has no recorder until [`crate::firmware_board::LoadedFirmwareBoard::enable_checkpoints`]
//! installs one; without it the loop does not build a view at all.
//!
//! What a checkpoint is (the table of addresses comes from the caller, from the S4-38L2 oracle;
//! nothing here is made up):
//!
//! | id | predicate | kind |
//! | --- | --- | --- |
//! | C0 | the first boundary of the first run | `boundary` |
//! | C1 | RIP is the first fetch address of the relocated copy | `boundary` (address) |
//! | C2 | the IDTR differs from the one at the first boundary (the first `LIDT` that retired) | `after_event` |
//! | C3 | the first retired `MOV CR3` | `after_event` |
//! | C4 | the first `#PF` delivered through the guest IDT | `after_event` |
//! | C5 | first fetch at `e_entry` (`startup_64`) | `boundary` (address) |
//! | C6 | the first `CPUID` with EAX = 0 (RIP is on it) | `boundary` |
//! | C7 | first fetch at the oracle address of `CPUID(1)` | `boundary` (address) |
//! | C8..C15 | first fetch at the symbol of the oracle | `boundary` (address) |
//! | C16 | `kernel_execve`, counted only after C15 | `boundary` (address) |
//! | C17 | first fetch in CPL3 at the `e_entry` of `/init` | `boundary` (address, CPL3) |
//!
//! `boundary` records describe the state BEFORE the instruction at `rip` (like the stop record of
//! a run that stopped there); `after_event` records describe the first boundary after the event,
//! so `retired` already counts the instruction that caused it. Every checkpoint is recorded the
//! FIRST time its predicate holds, and never again.
//!
//! An address checkpoint matches when RIP equals the symbol's virtual address or its physical
//! address AND the fetch translation of RIP through the current CR3 lands on the physical address
//! of the table (so an identity-mapped fetch and a kernel-virtual fetch of the same code both
//! count, and a RIP that maps somewhere else does not). A checkpoint without a physical address
//! (C17: a user address) matches on RIP alone.

/// How many checkpoints there are (C0..C17).
pub const LINUX_CHECKPOINT_COUNT: usize = 18;

/// Checkpoint ids that are events of the loop and not addresses of the table.
const C0_ENTRY: u8 = 0;
const C2_LIDT: u8 = 2;
const C3_CR3_WRITE: u8 = 3;
const C4_PAGE_FAULT: u8 = 4;
const C6_CPUID0: u8 = 6;

/// One address checkpoint of the table (C1, C5, C7..C17). The caller builds the list from the
/// oracle of S4-38L2.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinuxCheckpointTarget {
    /// 1..=17, not an event id (0, 2, 3, 4, 6 are recorded by the loop's own predicates).
    pub id: u8,
    /// Stable name (the oracle's symbol, for the report).
    pub name: String,
    /// Virtual address (kernel address space), when the checkpoint has one.
    pub va: Option<u64>,
    /// Physical address; when present the fetch of RIP must translate to it.
    pub pa: Option<u64>,
    /// Counts only when the CPL of the boundary is 3 (C17).
    pub cpl3: bool,
    /// Counts only after the checkpoint with this id was recorded (C16 after C15).
    pub after: Option<u8>,
}

/// The table handed to `enable_checkpoints`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LinuxCheckpointTable {
    pub targets: Vec<LinuxCheckpointTarget>,
}

/// Why a table is refused when the recorder is created (never a panic in the loop).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinuxCheckpointTableError {
    /// `id` is not below [`LINUX_CHECKPOINT_COUNT`].
    IdOutOfRange { id: u8 },
    /// `id` is the id of an event the loop records itself (C0, C2, C3, C4, C6): a table row may
    /// not take that slot.
    EventId { id: u8 },
    /// Two rows with the same `id`.
    DuplicateId { id: u8 },
    /// `after` is not below [`LINUX_CHECKPOINT_COUNT`] or is the row's own id.
    InvalidAfter { id: u8, after: u8 },
}

impl LinuxCheckpointTable {
    /// Checks every row: `id` below 18 and not an event id, unique, `after` below 18 and not the
    /// row itself.
    pub fn validate(&self) -> Result<(), LinuxCheckpointTableError> {
        let mut seen = [false; LINUX_CHECKPOINT_COUNT];
        for target in &self.targets {
            let id = target.id;
            if usize::from(id) >= LINUX_CHECKPOINT_COUNT {
                return Err(LinuxCheckpointTableError::IdOutOfRange { id });
            }
            if matches!(
                id,
                C0_ENTRY | C2_LIDT | C3_CR3_WRITE | C4_PAGE_FAULT | C6_CPUID0
            ) {
                return Err(LinuxCheckpointTableError::EventId { id });
            }
            if std::mem::replace(&mut seen[usize::from(id)], true) {
                return Err(LinuxCheckpointTableError::DuplicateId { id });
            }
            if let Some(after) = target.after
                && (usize::from(after) >= LINUX_CHECKPOINT_COUNT || after == id)
            {
                return Err(LinuxCheckpointTableError::InvalidAfter { id, after });
            }
        }
        Ok(())
    }
}

/// One recorded checkpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinuxCheckpointRecord {
    pub id: u8,
    pub name: String,
    /// `boundary` (the state before the instruction at `rip`) or `after_event`.
    pub kind: &'static str,
    pub rip: u64,
    /// The physical address of `rip` through the CR3 of the boundary (`None` when it does not
    /// translate).
    pub gpa: Option<u64>,
    /// Instructions retired in the run before this boundary (a partial REP counts when it
    /// completes, like everywhere in the board).
    pub retired: u64,
    /// Quanta of the run up to and including the one the boundary is in, i.e. the `quanta` the
    /// stop record would show if the run had stopped at this boundary.
    pub quanta: u64,
    /// Virtual time at the boundary: the time at the start of the quantum plus one ns per
    /// instruction retired in it (the board charges the clock after the quantum).
    pub virtual_ns: u64,
    /// The CR3 of the last retired `MOV CR3` (the board CR3 before any).
    pub cr3: u64,
    /// RPL of the CS selector.
    pub cpl: u8,
    /// Up to 16 bytes at `gpa` (translated byte by byte like a fetch); shorter when a byte does
    /// not translate.
    pub bytes: Vec<u8>,
}

/// What the loop shows the recorder at an instruction boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BoundaryView {
    pub rip: u64,
    pub cpl: u8,
    /// CR3 of the last retired `MOV CR3` of the run (the initial root before any).
    pub cr3: u64,
    /// Instructions retired in the current set call before this boundary.
    pub call_retired: u64,
    /// `(base, limit)` of the guest IDTR, `None` while none was loaded.
    pub idtr: Option<(u64, u16)>,
    /// The EAX of a `CPUID` at RIP (the loop's read-only peek), `None` for any other instruction.
    pub cpuid_leaf: Option<u32>,
}

/// The fetch translation of RIP the loop computes on demand: the physical address and up to 16
/// bytes there.
pub(crate) type FetchView = (Option<u64>, Vec<u8>);

/// The recorder the board owns while checkpoints are enabled.
#[derive(Debug)]
pub(crate) struct CheckpointRecorder {
    /// Address checkpoints not recorded yet.
    pending: Vec<LinuxCheckpointTarget>,
    records: Vec<Option<LinuxCheckpointRecord>>,
    /// The first boundary was seen.
    started: bool,
    /// The IDTR at the first boundary; C2 is any later difference.
    idtr_at_start: Option<(u64, u16)>,
    pending_cr3_write: bool,
    pending_page_fault: bool,
    retired_before_call: u64,
    quanta_of_call: u64,
    ns_at_call_start: u64,
}

impl CheckpointRecorder {
    /// Refuses a table that [`LinuxCheckpointTable::validate`] refuses.
    pub(crate) fn new(table: LinuxCheckpointTable) -> Result<Self, LinuxCheckpointTableError> {
        table.validate()?;
        Ok(Self {
            pending: table.targets,
            records: vec![None; LINUX_CHECKPOINT_COUNT],
            started: false,
            idtr_at_start: None,
            pending_cr3_write: false,
            pending_page_fault: false,
            retired_before_call: 0,
            quanta_of_call: 0,
            ns_at_call_start: 0,
        })
    }

    /// The board announces a set call: the run's retired count before it, the number of the
    /// quantum (1-based) and the virtual time at its start.
    pub(crate) fn begin_call(&mut self, retired_before: u64, quantum: u64, ns_at_start: u64) {
        self.retired_before_call = retired_before;
        self.quanta_of_call = quantum;
        self.ns_at_call_start = ns_at_start;
    }

    /// A `MOV CR3` retired (the loop calls this when the set reports the context change).
    pub(crate) fn note_cr3_write(&mut self) {
        self.pending_cr3_write = true;
    }

    /// A `#PF` was delivered through the guest IDT.
    pub(crate) fn note_page_fault_delivered(&mut self) {
        self.pending_page_fault = true;
    }

    /// The records, in id order, of the checkpoints reached so far.
    pub(crate) fn records(&self) -> Vec<LinuxCheckpointRecord> {
        self.records.iter().flatten().cloned().collect()
    }

    /// One instruction boundary. `fetch_view` is called at most once, and only when a predicate
    /// holds (it translates RIP and reads the bytes there).
    pub(crate) fn boundary(
        &mut self,
        view: &BoundaryView,
        fetch_view: &mut dyn FnMut() -> FetchView,
    ) {
        // Event predicates first: cheap flags and compares.
        let mut firing: Vec<(u8, String, &'static str)> = Vec::new();
        if !self.started {
            self.started = true;
            self.idtr_at_start = view.idtr;
            firing.push((C0_ENTRY, "entry".to_string(), "boundary"));
        } else if self.records[usize::from(C2_LIDT)].is_none() && view.idtr != self.idtr_at_start {
            firing.push((C2_LIDT, "first_lidt".to_string(), "after_event"));
        }
        if std::mem::take(&mut self.pending_cr3_write)
            && self.records[usize::from(C3_CR3_WRITE)].is_none()
        {
            firing.push((C3_CR3_WRITE, "first_cr3_write".to_string(), "after_event"));
        }
        if std::mem::take(&mut self.pending_page_fault)
            && self.records[usize::from(C4_PAGE_FAULT)].is_none()
        {
            firing.push((C4_PAGE_FAULT, "first_page_fault".to_string(), "after_event"));
        }
        if view.cpuid_leaf == Some(0) && self.records[usize::from(C6_CPUID0)].is_none() {
            firing.push((C6_CPUID0, "cpuid_leaf_0".to_string(), "boundary"));
        }
        // Address candidates: a compare of RIP; the translation is asked only for a match.
        let candidates: Vec<usize> = self
            .pending
            .iter()
            .enumerate()
            .filter(|(_, target)| self.is_address_candidate(target, view))
            .map(|(index, _)| index)
            .collect();
        if firing.is_empty() && candidates.is_empty() {
            return;
        }
        let (gpa, bytes) = fetch_view();
        let mut confirmed = Vec::new();
        for index in candidates {
            let target = &self.pending[index];
            // The physical address of the table must be the translation of RIP.
            if target.pa.is_none_or(|pa| gpa == Some(pa)) {
                confirmed.push(index);
            }
        }
        // Remove from the back so the indices stay valid; keep the id order of the report.
        confirmed.sort_unstable_by(|a, b| b.cmp(a));
        for index in confirmed {
            let target = self.pending.remove(index);
            firing.push((target.id, target.name, "boundary"));
        }
        for (id, name, kind) in firing {
            let slot = &mut self.records[usize::from(id)];
            if slot.is_some() {
                continue;
            }
            *slot = Some(LinuxCheckpointRecord {
                id,
                name,
                kind,
                rip: view.rip,
                gpa,
                retired: self.retired_before_call + view.call_retired,
                quanta: self.quanta_of_call,
                virtual_ns: self.ns_at_call_start + view.call_retired,
                cr3: view.cr3,
                cpl: view.cpl,
                bytes: bytes.clone(),
            });
        }
    }

    fn is_address_candidate(&self, target: &LinuxCheckpointTarget, view: &BoundaryView) -> bool {
        (target.va == Some(view.rip) || target.pa == Some(view.rip))
            && (!target.cpl3 || view.cpl == 3)
            && target
                .after
                .is_none_or(|after| self.records[usize::from(after)].is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(rip: u64) -> BoundaryView {
        BoundaryView {
            rip,
            cpl: 0,
            cr3: 0x1000,
            call_retired: 0,
            idtr: None,
            cpuid_leaf: None,
        }
    }

    fn target(id: u8, va: Option<u64>, pa: Option<u64>) -> LinuxCheckpointTarget {
        LinuxCheckpointTarget {
            id,
            name: format!("t{id}"),
            va,
            pa,
            cpl3: false,
            after: None,
        }
    }

    fn recorder(targets: Vec<LinuxCheckpointTarget>) -> CheckpointRecorder {
        let mut recorder = CheckpointRecorder::new(LinuxCheckpointTable { targets }).unwrap();
        recorder.begin_call(100, 3, 5_000);
        recorder
    }

    fn feed(recorder: &mut CheckpointRecorder, view: BoundaryView, gpa: Option<u64>) {
        recorder.boundary(&view, &mut || (gpa, vec![0xf4; 16]));
    }

    #[test]
    fn the_first_boundary_is_c0_and_counts_the_call_offset() {
        let mut recorder = recorder(Vec::new());
        let mut at = view(0x100_0200);
        at.call_retired = 7;
        feed(&mut recorder, at, Some(0x100_0200));
        let records = recorder.records();
        assert_eq!(records.len(), 1);
        let c0 = &records[0];
        assert_eq!((c0.id, c0.kind, c0.rip), (0, "boundary", 0x100_0200));
        assert_eq!(
            (c0.retired, c0.quanta, c0.virtual_ns, c0.cr3),
            (107, 3, 5_007, 0x1000)
        );
        assert_eq!(c0.gpa, Some(0x100_0200));
        assert_eq!(c0.bytes, vec![0xf4; 16]);
    }

    #[test]
    fn an_address_checkpoint_is_recorded_the_first_time_only() {
        let mut recorder = recorder(vec![target(
            8,
            Some(0xffff_ffff_8000_0000),
            Some(0x100_0000),
        )]);
        feed(&mut recorder, view(0x4000), Some(0x4000));
        let mut first = view(0xffff_ffff_8000_0000);
        first.call_retired = 2;
        feed(&mut recorder, first, Some(0x100_0000));
        let mut second = view(0xffff_ffff_8000_0000);
        second.call_retired = 9;
        feed(&mut recorder, second, Some(0x100_0000));
        let c8: Vec<_> = recorder
            .records()
            .into_iter()
            .filter(|record| record.id == 8)
            .collect();
        assert_eq!(c8.len(), 1, "{c8:?}");
        assert_eq!(c8[0].retired, 102, "the first hit, not the last");
    }

    #[test]
    fn a_physical_or_a_virtual_rip_matches_when_the_translation_agrees() {
        let pa = 0x3f2_53e0;
        let mut identity_run = recorder(vec![target(5, Some(0xffff_ffff_83f2_53e0), Some(pa))]);
        feed(&mut identity_run, view(pa), Some(pa));
        assert!(identity_run.records().iter().any(|record| record.id == 5));
        let mut virtual_run = recorder(vec![target(5, Some(0xffff_ffff_83f2_53e0), Some(pa))]);
        feed(&mut virtual_run, view(0xffff_ffff_83f2_53e0), Some(pa));
        assert!(virtual_run.records().iter().any(|record| record.id == 5));
    }

    #[test]
    fn a_rip_that_translates_elsewhere_or_not_at_all_is_not_a_hit() {
        let pa = 0x3f2_53e0;
        for gpa in [Some(pa + 1), None] {
            let mut recorder = recorder(vec![target(5, None, Some(pa))]);
            feed(&mut recorder, view(pa), gpa);
            assert!(
                recorder.records().iter().all(|record| record.id != 5),
                "{gpa:?}"
            );
        }
    }

    #[test]
    fn an_address_shifted_by_one_is_not_reached() {
        let pa = 0x139_c287;
        let mut recorder = recorder(vec![target(7, None, Some(pa + 1))]);
        feed(&mut recorder, view(pa), Some(pa));
        assert!(recorder.records().iter().all(|record| record.id != 7));
    }

    #[test]
    fn kernel_execve_counts_only_after_run_init_process() {
        let mut execve = target(16, Some(0xffff_ffff_818f_4570), Some(0x18f_4570));
        execve.after = Some(15);
        let run_init = target(15, Some(0xffff_ffff_8120_01c0), Some(0x120_01c0));
        let mut recorder = recorder(vec![run_init, execve]);
        // A usermode helper reaches `kernel_execve` before C15: not counted ...
        feed(&mut recorder, view(0xffff_ffff_818f_4570), Some(0x18f_4570));
        assert!(recorder.records().iter().all(|record| record.id != 16));
        feed(&mut recorder, view(0xffff_ffff_8120_01c0), Some(0x120_01c0));
        assert!(recorder.records().iter().any(|record| record.id == 15));
        // ... and the same address after it is C16 (the early hit did not use it up).
        feed(&mut recorder, view(0xffff_ffff_818f_4570), Some(0x18f_4570));
        assert!(recorder.records().iter().any(|record| record.id == 16));
    }

    #[test]
    fn user_entry_needs_cpl3_and_the_e_entry_of_init() {
        let mut user = target(17, Some(0x40_00b0), None);
        user.cpl3 = true;
        // A foreign e_entry (or CPL0 at the right address) is not C17.
        let mut recorder = recorder(vec![user.clone()]);
        let mut foreign = view(0x40_00c0);
        foreign.cpl = 3;
        feed(&mut recorder, foreign, Some(0x7000));
        feed(&mut recorder, view(0x40_00b0), Some(0x7000));
        assert!(recorder.records().iter().all(|record| record.id != 17));
        let mut entry = view(0x40_00b0);
        entry.cpl = 3;
        feed(&mut recorder, entry, Some(0x7000));
        let c17: Vec<_> = recorder
            .records()
            .into_iter()
            .filter(|record| record.id == 17)
            .collect();
        assert_eq!(c17.len(), 1);
        assert_eq!((c17[0].rip, c17[0].cpl), (0x40_00b0, 3));
    }

    #[test]
    fn events_are_recorded_once_and_after_the_event() {
        let mut recorder = recorder(Vec::new());
        feed(&mut recorder, view(0x1000), Some(0x1000));
        // A guest LIDT: the IDTR changes.
        let mut lidt = view(0x1008);
        lidt.idtr = Some((0x2000, 0xff));
        lidt.call_retired = 3;
        feed(&mut recorder, lidt, Some(0x1008));
        let mut again = view(0x1010);
        again.idtr = Some((0x3000, 0xff));
        again.call_retired = 4;
        feed(&mut recorder, again, Some(0x1010));
        // The first MOV CR3, then a second one.
        recorder.note_cr3_write();
        let mut cr3 = view(0x1018);
        cr3.call_retired = 5;
        cr3.cr3 = 0x9000;
        cr3.idtr = Some((0x3000, 0xff));
        feed(&mut recorder, cr3, Some(0x1018));
        recorder.note_cr3_write();
        cr3.call_retired = 6;
        feed(&mut recorder, cr3, Some(0x1020));
        // The first delivered #PF.
        recorder.note_page_fault_delivered();
        let mut pf = view(0x5000);
        pf.call_retired = 8;
        pf.idtr = Some((0x3000, 0xff));
        feed(&mut recorder, pf, Some(0x5000));
        recorder.note_page_fault_delivered();
        pf.call_retired = 9;
        feed(&mut recorder, pf, Some(0x5000));
        let by_id = |id: u8| -> Vec<LinuxCheckpointRecord> {
            recorder
                .records()
                .into_iter()
                .filter(|record| record.id == id)
                .collect()
        };
        for (id, retired) in [(2, 103), (3, 105), (4, 108)] {
            let found = by_id(id);
            assert_eq!(found.len(), 1, "C{id}");
            assert_eq!((found[0].retired, found[0].kind), (retired, "after_event"));
        }
        assert_eq!(by_id(3)[0].cr3, 0x9000);
    }

    #[test]
    fn the_first_cpuid_leaf_0_is_c6_and_other_leaves_are_not() {
        let mut recorder = recorder(Vec::new());
        feed(&mut recorder, view(0x1000), Some(0x1000));
        let mut leaf1 = view(0x1004);
        leaf1.cpuid_leaf = Some(1);
        feed(&mut recorder, leaf1, Some(0x1004));
        assert!(recorder.records().iter().all(|record| record.id != 6));
        let mut leaf0 = view(0x1008);
        leaf0.cpuid_leaf = Some(0);
        feed(&mut recorder, leaf0, Some(0x1008));
        let mut leaf0_again = view(0x100c);
        leaf0_again.cpuid_leaf = Some(0);
        feed(&mut recorder, leaf0_again, Some(0x100c));
        let c6: Vec<_> = recorder
            .records()
            .into_iter()
            .filter(|record| record.id == 6)
            .collect();
        assert_eq!(c6.len(), 1);
        assert_eq!(c6[0].rip, 0x1008);
    }

    #[test]
    fn the_translation_is_asked_only_when_a_predicate_holds() {
        let mut recorder = recorder(vec![target(7, None, Some(0x139_c287))]);
        feed(&mut recorder, view(0x1000), Some(0x1000));
        let mut asked = 0;
        for rip in [0x1004u64, 0x1008] {
            recorder.boundary(&view(rip), &mut || {
                asked += 1;
                (None, Vec::new())
            });
        }
        assert_eq!(asked, 0);
    }

    #[test]
    fn a_table_that_could_index_out_of_range_or_take_an_event_slot_is_refused() {
        let table = |targets: Vec<LinuxCheckpointTarget>| LinuxCheckpointTable { targets };
        let new = |targets| CheckpointRecorder::new(table(targets)).err();
        assert_eq!(
            new(vec![target(18, None, Some(1))]),
            Some(LinuxCheckpointTableError::IdOutOfRange { id: 18 })
        );
        assert_eq!(
            new(vec![target(255, None, Some(1))]),
            Some(LinuxCheckpointTableError::IdOutOfRange { id: 255 })
        );
        for id in [0u8, 2, 3, 4, 6] {
            assert_eq!(
                new(vec![target(id, None, Some(1))]),
                Some(LinuxCheckpointTableError::EventId { id })
            );
        }
        assert_eq!(
            new(vec![target(8, None, Some(1)), target(8, None, Some(2))]),
            Some(LinuxCheckpointTableError::DuplicateId { id: 8 })
        );
        for after in [18u8, 200, 8] {
            let mut row = target(8, None, Some(1));
            row.after = Some(after);
            assert_eq!(
                new(vec![row]),
                Some(LinuxCheckpointTableError::InvalidAfter { id: 8, after })
            );
        }
        // The rows the runner builds are accepted.
        let mut execve = target(16, None, Some(1));
        execve.after = Some(15);
        assert_eq!(
            new(vec![
                target(15, None, Some(2)),
                execve,
                target(17, Some(3), None)
            ]),
            None
        );
    }
}
