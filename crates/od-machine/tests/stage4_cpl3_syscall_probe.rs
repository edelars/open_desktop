use od_isa::syscall::{SyscallInstruction, SyscallStepExit};
use od_isa::{CpuActivity, Register};
use od_machine::{
    AccessWidth, GuestPhysicalAddress, STAGE4_CPL3_SYSCALL_FINAL_RIP,
    STAGE4_CPL3_SYSCALL_INITIAL_RAX, STAGE4_CPL3_SYSCALL_INITIAL_RSP,
    STAGE4_CPL3_SYSCALL_KERNEL_CODE_GVA, STAGE4_FLAT_PROBE_IMAGE_BYTES,
    STAGE4_FLAT_PROBE_IMAGE_GPA, Stage4Cpl3SyscallInjection, Stage4Cpl3SyscallStop,
    Stage4FlatProbeLayoutError, Stage4FlatProbeLoadError, load_stage4_cpl3_syscall_probe,
};

const INITIAL: u64 = 0x1122334455667788;
const STORE: u64 = 0x8877665544332211;
const NX: u64 = 1 << 63;

fn put(b: &mut [u8], gpa: u64, v: u64) {
    let o = (gpa - STAGE4_FLAT_PROBE_IMAGE_GPA) as usize;
    b[o..o + 8].copy_from_slice(&v.to_le_bytes())
}

fn image(initial: u64, store: u64) -> Vec<u8> {
    let mut b = vec![0; STAGE4_FLAT_PROBE_IMAGE_BYTES];
    for (g, v) in [
        (0x1000, 0x2007),
        (0x2000, 0x3007),
        (0x3010, 0x4007),
        (0x3018, 0x6003),
        (0x3020, 0x5007),
        (0x4000, 0x7005),
        (0x5000, 0x9003 | NX),
        (0x5008, 0x9005 | NX),
        (0x6000, 0x8001),
        (0x9000, initial),
        (0x9008, store),
    ] {
        put(&mut b, g, v)
    }
    b[0x6000..0x6005].copy_from_slice(&[0x0f, 0x05, 0x48, 0x8b, 0x03]);
    b[0x7000..0x7006].copy_from_slice(&[0x4c, 0x89, 0x17, 0x48, 0x0f, 0x07]);
    b
}

fn qword(m: &mut od_machine::LoadedStage4Cpl3SyscallProbe, gpa: u64) -> u64 {
    m.bus_mut()
        .read_physical(GuestPhysicalAddress::new(gpa), AccessWidth::Qword)
        .unwrap()
}

#[test]
fn syscall_kernel_store_sysretq_user_load_roundtrip_in_cpl3() {
    use SyscallInstruction::*;
    let mut m = load_stage4_cpl3_syscall_probe(&image(INITIAL, STORE)).unwrap();
    assert_eq!(m.cpl(), Some(3));
    assert_eq!(qword(&mut m, 0x9000), INITIAL);
    let mut callbacks = Vec::new();
    let report = m.run(16, |r| callbacks.push((r.start().get(), r.len())));
    assert_eq!(report.stop, Stage4Cpl3SyscallStop::Completed);
    assert_eq!(report.retired, 4);
    let steps: Vec<_> = report
        .events
        .iter()
        .map(|e| (e.instruction, e.rip_after, e.cs_after, e.cpl_after))
        .collect();
    assert_eq!(
        steps,
        [
            (
                Syscall,
                STAGE4_CPL3_SYSCALL_KERNEL_CODE_GVA,
                Some(8),
                Some(0)
            ),
            (
                KernelStore,
                STAGE4_CPL3_SYSCALL_KERNEL_CODE_GVA + 3,
                Some(8),
                Some(0)
            ),
            (Sysret, 0x400002, Some(0x23), Some(3)),
            (UserLoad, STAGE4_CPL3_SYSCALL_FINAL_RIP, Some(0x23), Some(3)),
        ]
    );
    assert!(
        report
            .events
            .iter()
            .all(|e| e.report.cpu.exit == SyscallStepExit::Retired)
    );
    // Only the kernel store changes the data page, and it does so exactly once.
    let stores: Vec<_> = report
        .events
        .iter()
        .map(|e| {
            e.report
                .committed_writes
                .iter()
                .filter(|r| r.start().get() == 0x9000)
                .count()
        })
        .collect();
    assert_eq!(stores, [0, 1, 0, 0]);
    assert_eq!(callbacks.iter().filter(|&&(g, _)| g == 0x9000).count(), 1);
    assert_eq!(
        callbacks,
        [
            (0x1000, 8),
            (0x2000, 8),
            (0x3010, 8),
            (0x4000, 8), // SYSCALL fetch walk: user code
            (0x3018, 8),
            (0x6000, 8), // kernel fetch walk
            (0x3020, 8),
            (0x5000, 8),
            (0x5000, 8), // kernel store walk: A then D on the RW alias
            (0x9000, 8), // the kernel qword
            (0x5008, 8), // user load walk: A on the RO alias
        ]
    );
    assert_eq!(qword(&mut m, 0x9000), STORE);
    let s = m.state();
    assert_eq!(s.gpr[Register::Rax as usize], STORE);
    assert_eq!(s.gpr[Register::Rcx as usize], 0x400002);
    assert_eq!(s.gpr[Register::R11 as usize], 0x202);
    assert_eq!(
        s.gpr[Register::Rsp as usize],
        STAGE4_CPL3_SYSCALL_INITIAL_RSP
    );
    assert_eq!(s.rip, STAGE4_CPL3_SYSCALL_FINAL_RIP);
    assert_eq!(s.cs_selector, Some(0x23));
    assert_eq!(s.ss_selector, Some(0x1b));
    assert_eq!(s.rflags & 0x200, 0x200);
    assert_eq!(s.cr3, Some(0x1000));
    assert_eq!(s.activity, Some(CpuActivity::Running));
    // A bits are set on the walked leaf PTEs; Dirty only on the supervisor RW alias.
    assert_eq!(qword(&mut m, 0x5000) & 0x60, 0x60);
    assert_eq!(qword(&mut m, 0x5008) & 0x60, 0x20);
    assert_eq!(qword(&mut m, 0x4000) & 0x60, 0x20);
    assert_eq!(qword(&mut m, 0x6000) & 0x60, 0x20);
}

#[test]
fn budget_limits_the_roundtrip_and_each_prefix_state_is_observable() {
    for budget in 0..=4_u64 {
        let mut m = load_stage4_cpl3_syscall_probe(&image(INITIAL, STORE)).unwrap();
        let report = m.run(budget, |_| {});
        assert_eq!(report.retired, budget);
        assert_eq!(
            report.stop,
            if budget == 4 {
                Stage4Cpl3SyscallStop::Completed
            } else {
                Stage4Cpl3SyscallStop::BudgetExhausted
            }
        );
        assert_eq!(
            m.data_qword(),
            Some(if budget >= 2 { STORE } else { INITIAL })
        );
        assert_eq!(
            m.state().gpr[Register::Rax as usize],
            if budget == 4 {
                STORE
            } else {
                STAGE4_CPL3_SYSCALL_INITIAL_RAX
            }
        );
        assert_eq!(m.cpl(), Some(if matches!(budget, 1 | 2) { 0 } else { 3 }));
    }
}

#[test]
fn injected_kernel_that_misses_the_qword_completes_but_user_reads_old_value() {
    let mut m = load_stage4_cpl3_syscall_probe(&image(INITIAL, STORE)).unwrap();
    m.inject(Stage4Cpl3SyscallInjection::KernelStoreMisdirected);
    let mut callbacks = Vec::new();
    let report = m.run(16, |r| callbacks.push(r.start().get()));
    assert_eq!(report.stop, Stage4Cpl3SyscallStop::Completed);
    assert_eq!(report.retired, 4);
    assert_eq!(m.cpl(), Some(3));
    assert_eq!(m.state().gpr[Register::Rax as usize], INITIAL);
    assert_ne!(m.state().gpr[Register::Rax as usize], STORE);
    assert_eq!(qword(&mut m, 0x9000), INITIAL);
    assert_eq!(qword(&mut m, 0x9ff8), STORE);
    assert!(callbacks.contains(&0x9ff8));
    assert!(!callbacks.contains(&0x9000));
}

#[test]
fn injected_sysretq_that_stays_in_cpl0_cannot_run_the_user_load() {
    let mut m = load_stage4_cpl3_syscall_probe(&image(INITIAL, STORE)).unwrap();
    m.inject(Stage4Cpl3SyscallInjection::SysretStaysCpl0);
    let report = m.run(16, |_| {});
    assert_eq!(
        report.stop,
        Stage4Cpl3SyscallStop::Step(SyscallStepExit::Unsupported)
    );
    assert_eq!(report.retired, 3);
    assert_eq!(report.events.len(), 4);
    assert_eq!(m.cpl(), Some(0));
    assert_eq!(m.state().cs_selector, Some(8));
    // The kernel store did happen; the guest simply never got back to user mode.
    assert_eq!(qword(&mut m, 0x9000), STORE);
    assert_eq!(
        m.state().gpr[Register::Rax as usize],
        STAGE4_CPL3_SYSCALL_INITIAL_RAX
    );
    assert_eq!(m.state().rip, 0x400002);
}

#[test]
fn loader_rejects_foreign_images() {
    assert!(matches!(
        load_stage4_cpl3_syscall_probe(&[0; 16]),
        Err(Stage4FlatProbeLoadError::ImageSize { .. })
    ));
    // Kernel page made user-accessible.
    let mut b = image(INITIAL, STORE);
    put(&mut b, 0x6000, 0x8005);
    assert!(matches!(
        load_stage4_cpl3_syscall_probe(&b),
        Err(Stage4FlatProbeLoadError::Layout(
            Stage4FlatProbeLayoutError::PageTableEntry { gpa: 0x6000, .. }
        ))
    ));
    // Extra mapping.
    let mut b = image(INITIAL, STORE);
    put(&mut b, 0x5010, 0x9003);
    assert!(matches!(
        load_stage4_cpl3_syscall_probe(&b),
        Err(Stage4FlatProbeLoadError::Layout(
            Stage4FlatProbeLayoutError::PageTableEntry { gpa: 0x5010, .. }
        ))
    ));
    // Altered kernel code (MOV [RDI],R10 -> MOV [RDI],R11) and hidden bytes.
    let mut b = image(INITIAL, STORE);
    b[0x7000] = 0x4d;
    assert!(matches!(
        load_stage4_cpl3_syscall_probe(&b),
        Err(Stage4FlatProbeLoadError::Layout(
            Stage4FlatProbeLayoutError::CodeBytes { gpa: 0x8000 }
        ))
    ));
    let mut b = image(INITIAL, STORE);
    b[0x6100] = 0xf4;
    assert!(matches!(
        load_stage4_cpl3_syscall_probe(&b),
        Err(Stage4FlatProbeLoadError::Layout(
            Stage4FlatProbeLayoutError::CodeBytes { gpa: 0x7000 }
        ))
    ));
    assert!(matches!(
        load_stage4_cpl3_syscall_probe(&image(STORE, STORE)),
        Err(Stage4FlatProbeLoadError::Layout(
            Stage4FlatProbeLayoutError::DataQwordsEqual
        ))
    ));
}
