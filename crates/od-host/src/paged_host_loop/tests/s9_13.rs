//! S9-13: guest storage and flush, end to end: a guest program rings real NVMe doorbells
//! through the paged JIT host loop, the controller executes Write/Flush over PRP, the
//! persistent backend (`PersistentNvmeBackend` / the production `PersistentStorage`)
//! makes the data durable, and the guest completion is observed in the CQ memory and as MSI-X
//! interrupts handled in the guest IDT (one handler run per command; see the note below on what
//! the counters can and cannot distinguish).
//!
//! What is real here: the guest machine code (Local APIC enable, doorbell stores through
//! guest page tables, `HLT`, interrupt handlers that count), the controller, its DMA/PRP
//! and CQE/MSI-X paths, the production durable write/flush state machine. What is NOT a guest:
//! the admin queue setup, the submission-queue entries and the PRP buffers are placed by the
//! host test (there is no guest NVMe driver in this slice), so the guest-level gate of S9-13
//! stays open (docs/specs/NVME_DEVICE.md, "S9-13").
//!
//! Interrupt counting: every queue has its own vector and handler counter. The test checks one
//! handler run per command, no missed interrupt and no extra delivery after handling. It does
//! NOT prove that the controller raised exactly one MSI per command: a repeated MSI of the same
//! vector before delivery merges into the Local APIC IRR and is indistinguishable here
//! (docs/specs/NVME_DEVICE.md §14.5).
//!
//! The fixture is a deliberate copy of the pieces of `s6_42c` it needs (its helpers are
//! private to that module); extracting a shared fixture is an open item, see
//! docs/specs/NVME_DEVICE.md §14.5.

use super::*;
use crate::host_threads::JitHostQuantum;
use crate::jit_region::SharedJitRegion;
use crate::persistent_nvme::PersistentNvmeBackend;
use crate::persistent_nvme::s9_13_support::observed_backend;
use od_isa::state::{Gdtr, Idtr, StiShadow};
use od_isa::{CodeSegmentDescriptorCache, CpuActivity, StackSegmentDescriptorCache};
use od_machine::{
    BackendError, HostParallelConfig, HostParallelFinish, LoadedFirmwareBoard, NvmeBackend,
    NvmeConfig, NvmeHandle, host_install_vcpu, host_parallel_board_with_nvme,
    run_accounted_host_threads,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(60);
const SECTOR: usize = 512;
const DISK_SECTORS: usize = 16;

const LOCAL_APIC: u64 = 0xFEE0_0000;
const PML4: u64 = 0x1_0000;
const PDPT: u64 = 0x1_1000;
const PD: u64 = 0x1_2000;
const PT: u64 = 0x1_3000;
const PD_APIC: u64 = 0x1_4000;
const PT_APIC: u64 = 0x1_5000;
const PT_NVME: u64 = 0x1_C000;
const CODE_0: u64 = 0x2_0000;
const IDT_AT: u64 = 0x3_0000;
const GDT_AT: u64 = 0x3_1000;
const STACK_TOP_0: u64 = 0x6_0800;
/// One interrupt vector, handler and counter cell per I/O completion queue (CQ n uses index n-1).
const VECTORS: [u8; 3] = [0x40, 0x41, 0x42];
const HANDLERS: [u64; 3] = [0x2_3000, 0x2_4000, 0x2_5000];
const COUNTERS: [u64; 3] = [0x4_0000, 0x4_0008, 0x4_0010];

const ASQ: u64 = 0x8_0000;
const ACQ: u64 = 0x8_1000;
const DEPTH: u32 = 16;
/// Write A: LBA 2..4, PRP1 = the last sector of a page, PRP2 = the next page.
const BUF_A: u64 = 0xB_0000;
const WRITE_A_LBA: u64 = 2;
/// Write B: LBA 8..10, both sectors behind PRP1.
const BUF_B: u64 = 0xB_3000;
const WRITE_B_LBA: u64 = 8;

fn io_sq(qid: u16) -> u64 {
    0x9_0000 + u64::from(qid) * 0x1000
}

fn io_cq(qid: u16) -> u64 {
    0xA_0000 + u64::from(qid) * 0x1000
}

fn config() -> PagedHostLoopConfig {
    PagedHostLoopConfig {
        cr3: PML4,
        efer: EFER,
        access_context: context(),
    }
}

fn put(board: &mut LoadedFirmwareBoard, at: u64, bytes: &[u8]) {
    board
        .bus_mut()
        .patch_physical_ram(GuestPhysicalAddress::new(at), bytes)
        .expect("the board RAM covers the address");
}

fn put_u64(board: &mut LoadedFirmwareBoard, at: u64, value: u64) {
    put(board, at, &value.to_le_bytes());
}

fn load_u32(board: &mut LoadedFirmwareBoard, at: u64) -> u32 {
    board
        .bus_mut()
        .atomic_inline_memory()
        .load_u32(GuestPhysicalAddress::new(at))
        .unwrap()
}

fn mmio_write(board: &mut LoadedFirmwareBoard, at: u64, width: AccessWidth, value: u64) {
    board
        .bus_mut()
        .write_physical(GuestPhysicalAddress::new(at), width, value, |_| {})
        .unwrap();
}

// ---- the guest ------------------------------------------------------------------------------

fn imm(opcode: &[u8], value: u32) -> Vec<u8> {
    let mut code = opcode.to_vec();
    code.extend(value.to_le_bytes());
    code
}

fn mov_eax(value: u32) -> Vec<u8> {
    imm(&[0xB8], value)
}

fn mov_ebx(value: u32) -> Vec<u8> {
    imm(&[0xBB], value)
}

/// `MOV [RBX + disp32], EAX`.
fn store_eax(offset: u32) -> Vec<u8> {
    imm(&[0x89, 0x83], offset)
}

/// Software-enable the Local APIC (SVR bit 8, spurious vector 0xFF).
fn enable_lapic() -> Vec<u8> {
    [mov_ebx(LOCAL_APIC as u32), mov_eax(0x1FF), store_eax(0xF0)].concat()
}

/// `counter += 1` through R8/R9 (a dword read-modify-write).
fn count(counter: u64) -> Vec<u8> {
    [
        imm(&[0x41, 0xB8], counter as u32), // mov r8d, counter
        vec![0x45, 0x8B, 0x08],             // mov r9d, [r8]
        vec![0x41, 0xFF, 0xC1],             // inc r9d
        vec![0x45, 0x89, 0x08],             // mov [r8], r9d
    ]
    .concat()
}

/// Handler of an interrupt vector: count, EOI (R8 the base, R10 zero), `IRETQ`.
fn irq_handler(counter: u64) -> Vec<u8> {
    [
        count(counter),
        imm(&[0x41, 0xB8], LOCAL_APIC as u32),
        vec![0x45, 0x31, 0xD2],         // xor r10d, r10d
        imm(&[0x45, 0x89, 0x90], 0xB0), // mov [r8+0xB0], r10d  (EOI)
        vec![0x48, 0xCF],               // iretq
    ]
    .concat()
}

/// The guest: enable the Local APIC, then store each `(queue, new SQ tail)` into the real
/// doorbell register through the guest page tables, then sleep (`hlt; jmp hlt`).
fn ring_program(doorbell: u64, rings: &[(u16, u32)]) -> Vec<u8> {
    let mut code = enable_lapic();
    for &(qid, tail) in rings {
        code.extend(mov_eax(tail));
        code.extend(mov_ebx(
            u32::try_from(doorbell + 8 * u64::from(qid)).unwrap(),
        ));
        code.extend(store_eax(0));
    }
    code.extend([0xF4, 0xEB, 0xFD]);
    code
}

fn gate(handler: u64) -> [u8; 16] {
    let mut gate = [0u8; 16];
    gate[0..2].copy_from_slice(&(handler as u16).to_le_bytes());
    gate[2..4].copy_from_slice(&0x08u16.to_le_bytes());
    gate[5] = 0x8E; // present, DPL 0, 64-bit interrupt gate
    gate[6..8].copy_from_slice(&((handler >> 16) as u16).to_le_bytes());
    gate
}

/// Identity tables for the first 2 MiB, the Local APIC page, the GDT, the IDT with one counting
/// handler per completion-queue vector.
fn with_paged_fixture(mut board: LoadedFirmwareBoard) -> LoadedFirmwareBoard {
    let flags = 0x63u64; // present, writable, accessed, dirty
    put_u64(&mut board, PML4, PDPT | flags);
    put_u64(&mut board, PDPT, PD | flags);
    put_u64(&mut board, PD, PT | flags);
    for index in 0..512u64 {
        put_u64(&mut board, PT + index * 8, (index << 12) | flags);
    }
    put_u64(&mut board, PDPT + 3 * 8, PD_APIC | flags);
    put_u64(&mut board, PD_APIC + 0x1F7 * 8, PT_APIC | flags);
    put_u64(&mut board, PT_APIC, LOCAL_APIC | flags | 0x10);
    let mut gdt = [0u8; 24];
    gdt[8..16].copy_from_slice(&0x00AF_9B00_0000_FFFFu64.to_le_bytes());
    gdt[16..24].copy_from_slice(&0x00CF_9300_0000_FFFFu64.to_le_bytes());
    put(&mut board, GDT_AT, &gdt);
    for index in 0..VECTORS.len() {
        put(
            &mut board,
            IDT_AT + u64::from(VECTORS[index]) * 16,
            &gate(HANDLERS[index]),
        );
        put(&mut board, HANDLERS[index], &irq_handler(COUNTERS[index]));
    }
    board
}

/// A processor in 64-bit mode at CPL0 with IF = 1 on the board's tables.
fn state(rip: u64, rsp: u64) -> LongModeState {
    let mut state = LongModeState {
        rip,
        rflags: 2 | (1 << 9),
        cr0: Some(CR0),
        cr2: Some(0),
        cr3: None,
        cr4: Some(CR4),
        cs_selector: Some(0x08),
        cs_cache: Some(CodeSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x9B,
            flags: 2,
        }),
        ss_selector: Some(0x10),
        ss_cache: Some(StackSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x93,
            flags: 0,
        }),
        gdtr: Some(Gdtr {
            base: GDT_AT,
            limit: 0x17,
        }),
        idtr: Some(Idtr {
            base: IDT_AT,
            limit: 0xFFF,
        }),
        sti_shadow: Some(StiShadow::Clear),
        activity: Some(CpuActivity::Running),
        ..LongModeState::default()
    };
    state.gpr[Register::Rsp as usize] = rsp;
    state
}

// ---- the NVMe side --------------------------------------------------------------------------

/// A board with one vCPU, the controller enabled, `io_queues` I/O queue pairs (SQ n -> CQ n,
/// interrupts on, vector n) created through the admin queue, and, with `msix`, MSI-X enabled
/// with the vectors of the I/O queues routed to the vCPU's Local APIC.
struct Rig {
    board: LoadedFirmwareBoard,
    nvme: NvmeHandle,
    doorbell: u64,
}

fn rig(backend: Box<dyn NvmeBackend + Send>, io_queues: u16, msix: bool) -> Rig {
    const PCI_FUNCTION: u64 = od_machine::PCI_ECAM_DESCRIPTOR.base + (4 << 15);
    let mut board =
        with_paged_fixture(host_parallel_board_with_nvme(1, NvmeConfig::default()).unwrap());
    let nvme = board.nvme().unwrap();
    nvme.attach_backend(backend).unwrap();
    let read_bar = |board: &mut LoadedFirmwareBoard, register| {
        board
            .bus_mut()
            .read_physical(
                GuestPhysicalAddress::new(PCI_FUNCTION + register),
                AccessWidth::Dword,
            )
            .unwrap()
    };
    let bar = (read_bar(&mut board, 0x14) << 32) | (read_bar(&mut board, 0x10) & !0xF);
    for (addr, width, value) in [
        (PCI_FUNCTION + 4, AccessWidth::Word, 6), // memory decode and bus mastering
        (bar + 0x24, AccessWidth::Dword, 0x000F_000F),
        (bar + 0x28, AccessWidth::Qword, ASQ),
        (bar + 0x30, AccessWidth::Qword, ACQ),
        (bar + 0x14, AccessWidth::Dword, 0x0046_0001),
    ] {
        mmio_write(&mut board, addr, width, value);
    }
    assert!(nvme.is_ready());
    let doorbell = bar + 0x1000;
    // The doorbell page, visible to the guest through its page tables.
    put_u64(
        &mut board,
        PD_APIC + ((doorbell >> 21) & 0x1ff) * 8,
        PT_NVME | 0x63,
    );
    put_u64(
        &mut board,
        PT_NVME + ((doorbell >> 12) & 0x1ff) * 8,
        doorbell | 0x73,
    );
    // MSI-X Enable (message control of the capability) comes first: Create I/O CQ with an
    // interrupt vector other than 0 is valid only in MSI-X mode. The vector table entry of a
    // queue is programmed after the admin commands; the admin vector 0 stays masked.
    if msix {
        mmio_write(&mut board, PCI_FUNCTION + 0x50, AccessWidth::Dword, 1 << 31);
    }
    // Admin: Create I/O CQ n (IEN, IV = n) then Create I/O SQ n for every pair.
    let mut slot = 0u64;
    for qid in 1..=io_queues {
        for (opcode, ring, cdw11) in [
            (
                0x05u8,
                io_cq(qid),
                if msix {
                    0b11 | (u32::from(qid) << 16)
                } else {
                    1 // polled: no interrupt
                },
            ),
            (0x01, io_sq(qid), (u32::from(qid) << 16) | 1),
        ] {
            let mut sqe = [0u8; 64];
            sqe[0] = opcode;
            sqe[2..4].copy_from_slice(&(0x10 + slot as u16).to_le_bytes());
            sqe[24..32].copy_from_slice(&ring.to_le_bytes());
            sqe[40..44].copy_from_slice(&(((DEPTH - 1) << 16) | u32::from(qid)).to_le_bytes());
            sqe[44..48].copy_from_slice(&cdw11.to_le_bytes());
            put(&mut board, ASQ + slot * 64, &sqe);
            slot += 1;
        }
    }
    mmio_write(&mut board, doorbell, AccessWidth::Dword, slot);
    let report = nvme.service(board.bus_mut(), |_| {});
    assert_eq!(u64::from(report.completions), slot, "{report:?}");
    for done in 0..slot {
        // Admin CQE: status 0 (success), phase 1.
        assert_eq!(load_u32(&mut board, ACQ + done * 16 + 12) >> 16, 1);
    }
    if msix {
        // Entry n: address = the Local APIC of vCPU 0, data = the vector, unmasked. Entry 0
        // (admin) stays masked. MSI-X Enable was already set above, before the admin commands.
        for qid in 1..=io_queues {
            let entry = bar + 0x2000 + u64::from(qid) * 16;
            mmio_write(&mut board, entry, AccessWidth::Qword, LOCAL_APIC);
            mmio_write(
                &mut board,
                entry + 8,
                AccessWidth::Dword,
                u64::from(VECTORS[usize::from(qid) - 1]),
            );
            mmio_write(&mut board, entry + 12, AccessWidth::Dword, 0);
        }
    }
    Rig {
        board,
        nvme,
        doorbell,
    }
}

/// Places one SQE at slot 0 of I/O SQ `qid`.
fn put_sqe(rig: &mut Rig, qid: u16, opcode: u8, nsid: u32, prp: (u64, u64), slba: u64, nlb: u32) {
    let mut sqe = [0u8; 64];
    let cid = 0x90 + qid;
    sqe[0..4].copy_from_slice(&((u32::from(cid) << 16) | u32::from(opcode)).to_le_bytes());
    sqe[4..8].copy_from_slice(&nsid.to_le_bytes());
    sqe[24..32].copy_from_slice(&prp.0.to_le_bytes());
    sqe[32..40].copy_from_slice(&prp.1.to_le_bytes());
    sqe[40..48].copy_from_slice(&slba.to_le_bytes());
    sqe[48..52].copy_from_slice(&nlb.to_le_bytes()); // 0-based sector count
    put(&mut rig.board, io_sq(qid), &sqe);
}

fn pattern(first: u8) -> Vec<u8> {
    [vec![first; SECTOR], vec![first + 1; SECTOR]].concat()
}

/// Write A (queue 1) and Write B (queue 2): the data in guest RAM and the SQEs.
fn put_two_writes(rig: &mut Rig) {
    // A: PRP1 = the last sector of the page at BUF_A, PRP2 = the next page.
    put(&mut rig.board, BUF_A + 0xE00, &vec![0xA1; SECTOR]);
    put(&mut rig.board, BUF_A + 0x1000, &vec![0xA2; SECTOR]);
    put_sqe(
        rig,
        1,
        0x01,
        1,
        (BUF_A + 0xE00, BUF_A + 0x1000),
        WRITE_A_LBA,
        1,
    );
    // B: both sectors in one page behind PRP1.
    put(&mut rig.board, BUF_B, &pattern(0xB1));
    put_sqe(rig, 2, 0x01, 1, (BUF_B, 0), WRITE_B_LBA, 1);
}

/// What one completion queue holds in its first two slots.
#[derive(Debug, Eq, PartialEq)]
struct Cqe {
    cid: u16,
    sqhd: u16,
    sqid: u16,
    phase: u32,
    sct: u32,
    sc: u32,
    dnr: u32,
}

fn cqe(rig: &mut Rig, qid: u16, slot: u64) -> Option<Cqe> {
    let at = io_cq(qid) + slot * 16;
    let (dw2, dw3) = (
        load_u32(&mut rig.board, at + 8),
        load_u32(&mut rig.board, at + 12),
    );
    // The CQ memory starts zero with expected phase 1: a zero dword 3 is "no entry".
    (dw3 != 0).then_some(Cqe {
        cid: dw3 as u16,
        sqhd: dw2 as u16,
        sqid: (dw2 >> 16) as u16,
        phase: (dw3 >> 16) & 1,
        sc: (dw3 >> 17) & 0xFF,
        sct: (dw3 >> 25) & 7,
        dnr: dw3 >> 31,
    })
}

fn success(qid: u16) -> Cqe {
    failure(qid, 0)
}

fn failure(qid: u16, sc: u32) -> Cqe {
    Cqe {
        cid: 0x90 + qid,
        sqhd: 1,
        sqid: qid,
        phase: 1,
        sct: 0,
        sc,
        dnr: 0,
    }
}

/// The result of running the guest to idle.
#[derive(Debug)]
struct GuestRun {
    finished: HostParallelFinish,
    /// Interrupts the vCPU took.
    irqs_delivered: u64,
    /// The counter each guest interrupt handler maintains, per completion-queue vector.
    handler_counts: [u32; 3],
}

fn run_guest(rig: &mut Rig, program: &[u8]) -> GuestRun {
    put(&mut rig.board, CODE_0, program);
    assert!(host_install_vcpu(
        &mut rig.board,
        0,
        state(CODE_0, STACK_TOP_0)
    ));
    let region = SharedJitRegion::new(1, 4).unwrap();
    let parts = vec![Some(region.take_part().unwrap())];
    let quantum = JitHostQuantum::new(parts, vec![config()], RFLAGS_DEFINED).with_cpl0_set();
    let (report, ()) = run_accounted_host_threads(
        &mut rig.board,
        HostParallelConfig {
            quantum: 64,
            budget: u64::MAX,
            end_when_idle: true,
        },
        &quantum.nmi_paging(),
        &|call| quantum.run_accounted(call),
        &|_| quantum.thread_exit(),
        |control| assert!(control.wait_finished(WAIT)),
    )
    .unwrap();
    assert_eq!(quantum.last_failure(), None);
    let irqs_delivered = quantum.stats(0).irqs_delivered;
    drop(quantum);
    drop(region);
    GuestRun {
        finished: report.finished,
        irqs_delivered,
        handler_counts: COUNTERS.map(|at| load_u32(&mut rig.board, at)),
    }
}

// ---- the observing backend ------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq)]
enum Event {
    WriteEnter {
        lba: u64,
        sectors: usize,
    },
    WriteReturn {
        ok: bool,
        durable_flushes: Option<usize>,
    },
    FlushEnter,
    FlushReturn {
        ok: bool,
        durable_flushes: Option<usize>,
    },
    /// The controller wrote a range of guest RAM (the CQE; Write and Flush DMA nothing into RAM).
    GuestWrite(u64),
}

type Log = Arc<Mutex<Vec<Event>>>;

fn push(log: &Log, event: Event) {
    log.lock().unwrap().push(event);
}

/// Records every Write/Flush entering and returning the wrapped backend, in order. With a
/// `durable_flushes` reader it also records the number of durable flushes of the storage layer
/// at the moment the call returns.
struct Recording<B> {
    inner: B,
    log: Log,
    durable_flushes: Option<Box<dyn Fn() -> usize + Send>>,
}

impl<B> Recording<B> {
    fn flushes(&self) -> Option<usize> {
        self.durable_flushes.as_ref().map(|read| read())
    }
}

impl<B: NvmeBackend> NvmeBackend for Recording<B> {
    fn sector_count(&self) -> u64 {
        self.inner.sector_count()
    }

    fn read_sectors(&mut self, lba: u64, buffer: &mut [u8]) -> Result<(), BackendError> {
        self.inner.read_sectors(lba, buffer)
    }

    fn write_sectors(&mut self, lba: u64, data: &[u8]) -> Result<(), BackendError> {
        push(
            &self.log,
            Event::WriteEnter {
                lba,
                sectors: data.len() / SECTOR,
            },
        );
        let result = self.inner.write_sectors(lba, data);
        let durable_flushes = self.flushes();
        push(
            &self.log,
            Event::WriteReturn {
                ok: result.is_ok(),
                durable_flushes,
            },
        );
        result
    }

    fn flush(&mut self) -> Result<(), BackendError> {
        push(&self.log, Event::FlushEnter);
        let result = self.inner.flush();
        let durable_flushes = self.flushes();
        push(
            &self.log,
            Event::FlushReturn {
                ok: result.is_ok(),
                durable_flushes,
            },
        );
        result
    }
}

struct ScratchFile(std::path::PathBuf);

impl Drop for ScratchFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn scratch_raw(tag: &str) -> ScratchFile {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "od-s9-13-{tag}-{}-{unique}.raw",
        std::process::id()
    ));
    let file = std::fs::File::create_new(&path).unwrap();
    let scratch = ScratchFile(path);
    file.set_len((DISK_SECTORS * SECTOR) as u64).unwrap();
    scratch
}

fn expected_image() -> Vec<u8> {
    let mut image = vec![0u8; DISK_SECTORS * SECTOR];
    image[2 * SECTOR..3 * SECTOR].fill(0xA1);
    image[3 * SECTOR..4 * SECTOR].fill(0xA2);
    image[8 * SECTOR..9 * SECTOR].fill(0xB1);
    image[9 * SECTOR..10 * SECTOR].fill(0xB2);
    image
}

// ---- the scenarios --------------------------------------------------------------------------

/// (a) Two 2-sector Writes (PRP1+PRP2 and PRP1 alone) and a Flush, each on its own queue, rung
/// by guest machine code: one CQE and one guest handler run per command, Write before Flush at the
/// backend, and the bytes are on the file after the controller (and the writer lock) is gone.
#[test]
fn s9_13_guest_doorbells_on_three_queues_persist_writes_and_flush() {
    let _guard = jit_guard();
    let scratch = scratch_raw("persist");
    let log = Log::default();
    let disk = PersistentNvmeBackend::open(&scratch.0).unwrap();
    let backend = Box::new(Recording {
        inner: disk,
        log: Arc::clone(&log),
        durable_flushes: None,
    });
    let mut rig = rig(backend, 3, true);
    put_two_writes(&mut rig);
    put_sqe(&mut rig, 3, 0x00, 1, (0, 0), 0, 0); // Flush

    let program = ring_program(rig.doorbell, &[(1, 1), (2, 1), (3, 1)]);
    let run = run_guest(&mut rig, &program);
    assert_eq!(run.finished, HostParallelFinish::AllIdle, "{run:?}");

    for qid in 1..=3 {
        assert_eq!(cqe(&mut rig, qid, 0), Some(success(qid)), "queue {qid}");
        assert_eq!(cqe(&mut rig, qid, 1), None, "queue {qid}: exactly one CQE");
    }
    assert_eq!(
        run.handler_counts,
        [1, 1, 1],
        "one handler run per command (a merged duplicate MSI is not distinguishable): {run:?}"
    );
    assert_eq!(run.irqs_delivered, 3, "{run:?}");
    assert!(!rig.nvme.doorbell_pending());

    assert_eq!(
        *log.lock().unwrap(),
        [
            Event::WriteEnter {
                lba: WRITE_A_LBA,
                sectors: 2
            },
            Event::WriteReturn {
                ok: true,
                durable_flushes: None
            },
            Event::WriteEnter {
                lba: WRITE_B_LBA,
                sectors: 2
            },
            Event::WriteReturn {
                ok: true,
                durable_flushes: None
            },
            Event::FlushEnter,
            Event::FlushReturn {
                ok: true,
                durable_flushes: None
            },
        ]
    );

    // Drop the machine, so the controller drops the backend and its writer lock, then reopen.
    drop(rig);
    let mut reopened =
        PersistentNvmeBackend::open(&scratch.0).expect("the writer lock is released");
    let mut image = vec![0u8; DISK_SECTORS * SECTOR];
    reopened.read_sectors(0, &mut image).unwrap();
    assert_eq!(image, expected_image());
    assert_eq!(std::fs::read(&scratch.0).unwrap(), expected_image());
}

/// (b) A refused durable flush after a guest Write: the guest sees Internal Error (SC 0x06) for
/// that Write and for every following command, exactly one CQE and one guest handler run each; the
/// backend is terminal and the RAW lifecycle is quarantined; the storage layer is not touched
/// again (`write_calls`/`flush_calls`). The unchanged durable bytes are a property of the scripted
/// fixture (the flush is refused before it makes anything durable), not an independent check.
#[test]
fn s9_13_failed_durable_flush_gives_the_guest_internal_error_and_quarantines_raw() {
    let _guard = jit_guard();
    let (probe, observed) = observed_backend(&vec![0u8; DISK_SECTORS * SECTOR]);
    probe.fail_flush_number(1);
    let mut rig = rig(Box::new(observed.clone()), 3, true);
    put_two_writes(&mut rig);
    put_sqe(&mut rig, 3, 0x00, 1, (0, 0), 0, 0); // Flush

    let program = ring_program(rig.doorbell, &[(1, 1), (2, 1), (3, 1)]);
    let run = run_guest(&mut rig, &program);
    assert_eq!(run.finished, HostParallelFinish::AllIdle, "{run:?}");

    for qid in 1..=3 {
        assert_eq!(
            cqe(&mut rig, qid, 0),
            Some(failure(qid, 0x06)),
            "queue {qid}"
        );
        assert_eq!(cqe(&mut rig, qid, 1), None, "queue {qid}: exactly one CQE");
    }
    assert_eq!(
        run.handler_counts,
        [1, 1, 1],
        "an error completion interrupts too: {run:?}"
    );
    assert_eq!(run.irqs_delivered, 3, "{run:?}");

    assert!(observed.is_terminal_failed());
    assert!(observed.is_quarantined());
    // The first Write reached the storage layer and its flush failed; the second Write and the
    // Flush were refused by the terminal adapter without touching storage.
    assert_eq!(probe.write_calls(), 1);
    assert_eq!(probe.flush_calls(), 1);
    // Fixture property, not a behaviour check: the scripted flush failure never makes data durable.
    assert_eq!(probe.durable_bytes(), vec![0u8; DISK_SECTORS * SECTOR]);
    let mut volatile_after_first_write = vec![0u8; DISK_SECTORS * SECTOR];
    volatile_after_first_write[2 * SECTOR..3 * SECTOR].fill(0xA1);
    volatile_after_first_write[3 * SECTOR..4 * SECTOR].fill(0xA2);
    assert_eq!(probe.volatile_bytes(), volatile_after_first_write);
}

/// (c) Ordering and counts at the backend boundary, observed through the ranges the controller
/// reports as committed guest writes: the Write CQE is published only after `write_sectors`
/// (and its durable flush) returned, and the Flush calls the storage layer's durable flush exactly
/// once. The doorbells are written by the host here (no guest code): the order of events inside
/// one controller service is not observable from a running guest.
#[test]
fn s9_13_write_cqe_follows_the_backend_write_and_flush_runs_one_durable_flush() {
    let _guard = jit_guard();
    let (probe, observed) = observed_backend(&vec![0u8; DISK_SECTORS * SECTOR]);
    let log = Log::default();
    let reader = probe.clone();
    let backend = Box::new(Recording {
        inner: observed.clone(),
        log: Arc::clone(&log),
        durable_flushes: Some(Box::new(move || reader.flush_calls())),
    });
    let mut rig = rig(backend, 2, false);
    put_two_writes(&mut rig);
    put_sqe(&mut rig, 2, 0x00, 1, (0, 0), 0, 0); // Flush on queue 2 instead of Write B
    assert_eq!(probe.flush_calls(), 0);

    for qid in [1u16, 2] {
        mmio_write(
            &mut rig.board,
            rig.doorbell + 8 * u64::from(qid),
            AccessWidth::Dword,
            1,
        );
        let committed = Arc::clone(&log);
        let report = rig.nvme.service(rig.board.bus_mut(), |range| {
            push(&committed, Event::GuestWrite(range.start().get()));
        });
        assert_eq!((report.commands, report.completions), (1, 1), "{report:?}");
    }

    assert_eq!(
        *log.lock().unwrap(),
        [
            Event::WriteEnter {
                lba: WRITE_A_LBA,
                sectors: 2
            },
            Event::WriteReturn {
                ok: true,
                durable_flushes: Some(1)
            },
            Event::GuestWrite(io_cq(1)),
            Event::FlushEnter,
            Event::FlushReturn {
                ok: true,
                durable_flushes: Some(2)
            },
            Event::GuestWrite(io_cq(2)),
        ]
    );
    assert_eq!(probe.write_calls(), 1);
    assert_eq!(
        probe.flush_calls(),
        2,
        "one durable flush per Write, one for the Flush"
    );
    assert_eq!(cqe(&mut rig, 1, 0), Some(success(1)));
    assert_eq!(cqe(&mut rig, 2, 0), Some(success(2)));
    assert!(!observed.is_terminal_failed());
    let durable = probe.durable_bytes();
    assert_eq!(durable[2 * SECTOR..3 * SECTOR], [0xA1; SECTOR]);
    assert_eq!(durable[3 * SECTOR..4 * SECTOR], [0xA2; SECTOR]);
}
