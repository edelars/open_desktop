//! S2-53: PREFETCHW (`0F 0D /1 m8`).
//!
//! Every expected value comes from the Intel SDM text of PREFETCHW (Vol. 2B, pp. 4-416..4-417,
//! combined SDM order numbers 253665/325383/325384/335592), read for this task:
//!
//! * "The PREFETCHW instruction is merely a hint and does not affect program behavior." Operation:
//!   FETCH_WITH_EXCLUSIVE_OWNERSHIP (m8); "Prefetches from non-writeback memory are ignored".
//!   Flags affected: none.
//! * Exceptions in every mode, 64-bit mode included: only "#UD If the LOCK prefix is used". The
//!   list has no #PF, #GP or #SS, so a non-canonical, unmapped or protected address is not a fault
//!   and the instruction reads and writes no guest memory (the model has no cache).
//! * The register form (mod = 3) and the other /r members of 0F 0D are not defined by the PREFETCHW
//!   page (/2 is PREFETCHWT1, Vol. 2D, with its own CPUID bit; the SDM only says that undefined
//!   0F 0D encodings behave as a reserved NOP). This slice decodes /1 with a memory operand only;
//!   the others stay Unsupported (no state change). The oracle (work/stage8/s2_52_53_oracle) shows
//!   that Intel hardware without PRFCHW runs all of them as NOP; that is recorded as an open
//!   decision in the stage 8 report.
//!
//! The SDM names CPUID.80000001H:ECX.PREFETCHW[8] as the enumeration; the interpreter does not
//! gate on it (S2-57 declares the bit), so no case below covers the bit.

use od_isa::execute::{
    self, ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, Cr0Profile, Cr4Profile,
    RunConfig, RunExit, StepOutcome, run_with_cpl0_profile_set, run_with_rflags_defined,
};
use od_isa::{
    AccessContext, AccessKind, ArchitecturalFault, CpuActivity, GuestMemory, LongModeState,
    MemoryError, PrivilegeLevel, Register, StiShadow,
};

const CODE: u64 = 0x2000;
const ALL_FLAGS: u64 = 0x2 | 0xcd5;

fn ring0() -> AccessContext {
    AccessContext {
        privilege: PrivilegeLevel::Ring0,
        ..AccessContext::default()
    }
}

/// Code only; every data access is an error and is counted (a prefetch must not touch memory).
struct Ram {
    bytes: Vec<u8>,
    data_accesses: usize,
}

impl Ram {
    fn with_code(code: &[u8]) -> Self {
        let mut bytes = vec![0; 0x4000];
        bytes[CODE as usize..CODE as usize + code.len()].copy_from_slice(code);
        Self {
            bytes,
            data_accesses: 0,
        }
    }
}

impl GuestMemory for Ram {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        self.bytes
            .get(address as usize)
            .copied()
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }

    fn read(&mut self, address: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        self.data_accesses += 1;
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })
    }

    fn write(&mut self, address: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        self.data_accesses += 1;
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Write,
        })
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

/// Distinct non-zero values in every register so that an unwanted write is visible.
fn state(rdi: u64) -> LongModeState {
    let mut state = LongModeState {
        rip: CODE,
        rflags: ALL_FLAGS,
        ..LongModeState::default()
    };
    for (index, slot) in state.gpr.iter_mut().enumerate() {
        *slot = 0x0101_0101_0101_0101 * (index as u64 + 0x11);
    }
    state.gpr[Register::Rdi as usize] = rdi;
    state
}

fn step(code: &[u8], cpu: &mut LongModeState) -> (Result<StepOutcome, MemoryError>, Ram) {
    let mut ram = Ram::with_code(code);
    let result = execute::step(cpu, &mut ram, ring0());
    (result, ram)
}

/// Addresses that a real access would fault on or that are outside the test RAM: inside RAM,
/// beyond it, NULL, the lowest non-canonical, the highest canonical low half, canonical kernel
/// half (unmapped here), and non-canonical garbage.
const ADDRESSES: [u64; 9] = [
    0x1000,
    0x8000_0000,
    0,
    0x0000_8000_0000_0000,
    0x0000_7fff_ffff_ffff,
    0xffff_8000_0000_0000,
    0xffff_ffff_ffff_ffff,
    0x1234_5678_1234_5678,
    0xffff_7fff_ffff_ffff,
];

// ---- the hint: no fault, no memory access, no state change but RIP ----

#[test]
fn prefetchw_m8_is_a_hint_for_every_address_and_changes_only_rip() {
    for address in ADDRESSES {
        let mut cpu = state(address);
        let before = cpu.clone();
        let (result, ram) = step(&[0x0f, 0x0d, 0x0f], &mut cpu);
        assert_eq!(result, Ok(StepOutcome::Advanced), "{address:#x}");
        assert_eq!(ram.data_accesses, 0, "{address:#x}");
        let mut expected = before;
        expected.rip = CODE + 3;
        assert_eq!(cpu, expected, "{address:#x}");
    }
}

#[test]
fn every_memory_addressing_form_is_consumed_to_its_full_length_without_an_access() {
    // (bytes, name). ModRM /1 = reg field 001. The base registers hold the unmapped or
    // non-canonical addresses, so an effective address that is (wrongly) computed and used faults.
    let forms: Vec<(Vec<u8>, &str)> = vec![
        (vec![0x0f, 0x0d, 0x0f], "[rdi]"),
        (vec![0x0f, 0x0d, 0x4f, 0x10], "[rdi+disp8]"),
        (
            vec![0x0f, 0x0d, 0x8f, 0x00, 0x10, 0x00, 0x00],
            "[rdi+disp32]",
        ),
        (vec![0x0f, 0x0d, 0x0c, 0xc7], "[rdi+rax*8]"),
        (vec![0x0f, 0x0d, 0x0c, 0x24], "[rsp]"),
        (vec![0x0f, 0x0d, 0x4c, 0x24, 0x08], "[rsp+disp8]"),
        (vec![0x0f, 0x0d, 0x4d, 0x00], "[rbp+disp8] (SS-based)"),
        (
            vec![0x0f, 0x0d, 0x0d, 0x00, 0x00, 0x00, 0x00],
            "[rip+disp32]",
        ),
        (
            vec![0x0f, 0x0d, 0x0c, 0x25, 0x00, 0x10, 0x00, 0x00],
            "[disp32] (SIB, no base)",
        ),
        (vec![0x41, 0x0f, 0x0d, 0x08], "[r8] (REX.B)"),
        (vec![0x43, 0x0f, 0x0d, 0x0c, 0xc1], "[r9+r8*8] (REX.X+B)"),
        (vec![0x48, 0x0f, 0x0d, 0x0f], "REX.W"),
        (vec![0x40, 0x0f, 0x0d, 0x0f], "REX"),
        (vec![0x66, 0x0f, 0x0d, 0x0f], "66"),
        (vec![0x67, 0x0f, 0x0d, 0x0f], "67 (32-bit address)"),
        (vec![0x66, 0x67, 0x0f, 0x0d, 0x0f], "66 67"),
        (vec![0x2e, 0x0f, 0x0d, 0x0f], "cs"),
        (vec![0x36, 0x0f, 0x0d, 0x0f], "ss"),
        (vec![0x3e, 0x0f, 0x0d, 0x0f], "ds"),
        (vec![0x26, 0x0f, 0x0d, 0x0f], "es"),
        (vec![0x64, 0x0f, 0x0d, 0x0f], "fs"),
        (vec![0x65, 0x0f, 0x0d, 0x0f], "gs"),
        (vec![0x64, 0x48, 0x0f, 0x0d, 0x4f, 0x08], "fs REX.W disp8"),
        (vec![0x48, 0x66, 0x0f, 0x0d, 0x0f], "REX before 66"),
    ];
    for (code, name) in forms {
        for address in [0x1000, 0x0000_8000_0000_0000, 0xffff_ffff_ffff_ffff, 0] {
            let mut cpu = state(address);
            // Every register that an address could come from holds the stress address.
            for register in [
                Register::Rax,
                Register::Rbx,
                Register::Rsp,
                Register::Rbp,
                Register::R8,
                Register::R9,
            ] {
                cpu.gpr[register as usize] = address;
            }
            let before = cpu.clone();
            let (result, ram) = step(&code, &mut cpu);
            assert_eq!(result, Ok(StepOutcome::Advanced), "{name} {address:#x}");
            assert_eq!(ram.data_accesses, 0, "{name} {address:#x}");
            let mut expected = before;
            expected.rip = CODE + code.len() as u64;
            assert_eq!(cpu, expected, "{name} {address:#x}");
        }
    }
}

// ---- LOCK is #UD, nothing else is ----

#[test]
fn lock_is_ud_for_the_memory_form_and_the_register_form() {
    let ud = StepOutcome::Fault(ArchitecturalFault {
        vector: 6,
        error_code: None,
        linear_address: None,
    });
    for code in [
        vec![0xf0, 0x0f, 0x0d, 0x0f],
        vec![0xf0, 0x48, 0x0f, 0x0d, 0x0f],
        vec![0x48, 0xf0, 0x0f, 0x0d, 0x0f],
        vec![0xf0, 0x0f, 0x0d, 0x4f, 0x10],
        vec![0xf0, 0x0f, 0x0d, 0xc8],
        vec![0xf0, 0x48, 0x0f, 0x0d, 0xc8],
    ] {
        let mut cpu = state(0xffff_ffff_ffff_ffff);
        let before = cpu.clone();
        let (result, ram) = step(&code, &mut cpu);
        assert_eq!(result, Ok(ud.clone()), "{code:02x?}");
        assert_eq!(cpu, before, "{code:02x?}");
        assert_eq!(ram.data_accesses, 0);
    }
}

// ---- neighbouring encodings stay refused, with no state change ----

#[test]
fn register_form_and_the_other_reg_fields_stay_unsupported_without_change() {
    let mut forms: Vec<(Vec<u8>, String)> = Vec::new();
    for reg in [0u8, 2, 3, 4, 5, 6, 7] {
        forms.push((
            vec![0x0f, 0x0d, 0x07 | (reg << 3)],
            format!("0F 0D /{reg} [rdi]"),
        ));
    }
    for reg in 0..8u8 {
        for rm in [1u8, 7] {
            forms.push((
                vec![0x0f, 0x0d, 0xc0 | (reg << 3) | rm],
                format!("0F 0D /{reg} register rm={rm}"),
            ));
        }
    }
    forms.push((vec![0x48, 0x0f, 0x0d, 0xc8], "REX.W register /1".into()));
    forms.push((vec![0x41, 0x0f, 0x0d, 0xc9], "REX.B register /1 r9".into()));
    forms.push((vec![0x66, 0x0f, 0x0d, 0xc8], "66 register /1".into()));
    forms.push((vec![0xf3, 0x0f, 0x0d, 0x0f], "rep".into()));
    forms.push((vec![0xf2, 0x0f, 0x0d, 0x0f], "repne".into()));
    for (code, name) in forms {
        let mut cpu = state(0xffff_ffff_ffff_ffff);
        let before = cpu.clone();
        let (result, ram) = step(&code, &mut cpu);
        assert_eq!(
            result,
            Ok(StepOutcome::Unsupported { opcode: 0x0d }),
            "{name}"
        );
        assert_eq!(cpu, before, "{name}");
        assert_eq!(ram.data_accesses, 0, "{name}");
    }
}

#[test]
fn the_neighbouring_prefetch_group_0f18_is_not_affected() {
    // 0F 18 /1 [rdi] stays whatever it was before this slice (a hint admitted only in the CPL0
    // set; unsupported in the bare run).
    let mut cpu = state(0x1000);
    let before = cpu.clone();
    let (result, _) = step(&[0x0f, 0x18, 0x0f], &mut cpu);
    assert_eq!(result, Ok(StepOutcome::Unsupported { opcode: 0x18 }));
    assert_eq!(cpu, before);
}

// ---- flag definedness: no flag is affected ----

#[test]
fn prefetchw_changes_no_flag_definedness() {
    let config = RunConfig {
        instruction_budget: 1,
        stop_rip: None,
        context: ring0(),
    };
    for initial in [0x02, 0x02 | 0x800, od_isa::INTEGER_FLAGS] {
        let mut cpu = state(0x1000);
        let mut ram = Ram::with_code(&[0x0f, 0x0d, 0x0f]);
        let result =
            run_with_rflags_defined(&mut cpu, &mut ram, config, initial, || false).unwrap();
        assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
        assert_eq!(result.rflags_defined, initial);
        assert_eq!(cpu.rflags, ALL_FLAGS);
    }
}

// ---- the CPL0 profile set (Ring0) executes it; the bare run gives the same result ----

#[test]
fn the_cpl0_profile_set_executes_it_at_ring0_without_touching_memory() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let config = RunConfig {
        instruction_budget: 1,
        stop_rip: None,
        context: ring0(),
    };
    for address in [0x1000u64, 0x0000_8000_0000_0000, 0xffff_ffff_ffff_ffff] {
        let mut cpu = state(address);
        cpu.cr0 = Some(Cr0Profile::BASE);
        cpu.cr4 = Some(Cr4Profile::BASE);
        cpu.sti_shadow = Some(StiShadow::Clear);
        cpu.activity = Some(CpuActivity::Running);
        let before = cpu.clone();
        let mut ram = Ram::with_code(&[0x0f, 0x0d, 0x0f]);
        let result = run_with_cpl0_profile_set(
            &mut cpu,
            &mut ram,
            config,
            od_isa::INTEGER_FLAGS,
            &set,
            None,
            || false,
        )
        .unwrap();
        assert_eq!(
            result.exit,
            Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted),
            "{address:#x}"
        );
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(ram.data_accesses, 0);
        let mut expected = before;
        expected.rip = CODE + 3;
        assert_eq!(cpu, expected, "{address:#x}");
    }
}
