//! S7-32 slice S: `decode-coverage --exec windows` — which decoded forms the CPL0 set of the unified
//! loop (the Windows members, `od_machine::UnifiedGuestCpl0Cells`) does not execute.
//!
//! Every distinct decoded byte string is executed once per sandbox variant: one instruction
//! (budget 1) of `run_with_cpl0_profile_set_and_pio` at CPL0, on a memory that accepts every data
//! access (Windows-shaped GDT, IDT and TSS, a stack frame, a fill byte elsewhere), with the cells
//! of the set reset and seeded before the step. The decision is the library's own step: the
//! sandbox models no semantics.
//!
//! Verdicts: `exec` — retired, raised an architectural exception or ended the run in a modelled way
//! in every variant; `cond` — `Unsupported` in some variants only (the refusal depends on values:
//! a policy, an MSR index, a selector, a frame); `never` — `Unsupported` in every variant;
//! `memory_error` / `other_exit` — a host memory error or another exit (not `exec`).
//!
//! Limits (FIRMWARE.md, «Статическое покрытие»): the sandbox does not see the memory adapter of the
//! board (access widths, MMIO, device errors), values outside the two variants, or code outside
//! `.pdata` and the targets of `call rel32`. `exec` is not a proof that a run passes the form.

use od_isa::execute::{
    Cpl0ProfileSetExit, Cpl0ProfileSetInputError, GuestFpState, RunConfig, RunExit,
    run_with_cpl0_profile_set_and_pio,
};
use od_isa::long_decode::{Instruction, Operation};
use od_isa::state::{Gdtr, Idtr};
use od_isa::{
    AccessContext, AccessOrigin, CodeSegmentDescriptorCache, CpuActivity,
    DataSegmentDescriptorCache, FsDescriptorCache, GuestMemory, InstructionMode, LongModeState,
    MemoryError, OperandSize, PortIo, PrivilegeLevel, StiShadow, TaskRegisterCache,
};
use od_machine::{UnifiedGuest, UnifiedGuestCellSeed, UnifiedGuestCpl0Cells};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

/// The linear address of the sandbox code (a canonical kernel address, away from a page end).
pub(super) const CODE: u64 = 0xffff_f800_0040_0100;
const CR0: u64 = 0x8005_0033;
const CR4: u64 = 0x6f8;
const CR3: u64 = 0x1a_a000;
const GDT: u64 = 0xffff_f800_0300_0000;
const IDT: u64 = 0xffff_f800_0300_1000;
const TSS: u64 = 0xffff_f800_0300_2000;
/// The ECX of an RDMSR/WRMSR without a `mov ecx, imm32` before it in the same function.
pub(super) const UNKNOWN_MSR: u32 = 0xdead_0000;

/// Code at `CODE`; the regions hold the descriptor tables and the stack frame; every other
/// read returns `fill`; every write succeeds. Widths outside the widths of the unified-loop
/// adapter (1, 2, 4, 8, 16 bytes) are recorded.
struct Sandbox<'r> {
    code: [u8; 16],
    fill: u8,
    regions: &'r [(u64, Vec<u8>)],
    odd_width: Option<usize>,
}

impl Sandbox<'_> {
    fn byte(&self, address: u64) -> u8 {
        for (base, bytes) in self.regions {
            let offset = address.wrapping_sub(*base);
            if offset < bytes.len() as u64 {
                return bytes[offset as usize];
            }
        }
        self.fill
    }

    fn note(&mut self, len: usize) {
        if !matches!(len, 1 | 2 | 4 | 8 | 16) && self.odd_width.is_none() {
            self.odd_width = Some(len);
        }
    }

    fn value(&self, address: u64, size: OperandSize) -> u64 {
        let mut bytes = [0u8; 8];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = self.byte(address.wrapping_add(index as u64));
        }
        let value = u64::from_le_bytes(bytes);
        match size {
            OperandSize::Byte => value & 0xff,
            OperandSize::Word => value & 0xffff,
            OperandSize::Dword => value & 0xffff_ffff,
            _ => value,
        }
    }
}

impl GuestMemory for Sandbox<'_> {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        address
            .checked_sub(CODE)
            .and_then(|offset| self.code.get(usize::try_from(offset).ok()?))
            .copied()
            .ok_or_else(|| MemoryError::Backend("fetch outside the sandbox code".into()))
    }

    fn read(&mut self, address: u64, _: AccessContext, out: &mut [u8]) -> Result<(), MemoryError> {
        self.note(out.len());
        for (index, byte) in out.iter_mut().enumerate() {
            *byte = self.byte(address.wrapping_add(index as u64));
        }
        Ok(())
    }

    fn write(&mut self, _: u64, _: AccessContext, data: &[u8]) -> Result<(), MemoryError> {
        self.note(data.len());
        Ok(())
    }

    fn read_integer_for_write(
        &mut self,
        address: u64,
        _: AccessContext,
        size: OperandSize,
    ) -> Result<u64, MemoryError> {
        Ok(self.value(address, size))
    }

    fn preflight_descriptor_accessed_write(
        &mut self,
        _: u64,
        _: AccessContext,
        _: &[(u64, usize)],
    ) -> Result<(), MemoryError> {
        Ok(())
    }

    fn atomic_exchange_integer(
        &mut self,
        address: u64,
        _: AccessContext,
        size: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Ok(self.value(address, size))
    }

    fn atomic_fetch_add_integer(
        &mut self,
        address: u64,
        _: AccessContext,
        size: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Ok(self.value(address, size))
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        address: u64,
        _: AccessContext,
        size: OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Ok(self.value(address, size))
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

/// Every port reads all ones and takes every write.
struct Ports;

impl PortIo for Ports {
    type Error = ();

    fn read_u8(&mut self, _: u16) -> Result<u8, ()> {
        Ok(0xff)
    }

    fn write_u8(&mut self, _: u16, _: u8) -> Result<(), ()> {
        Ok(())
    }
}

struct Variant {
    gpr: [u64; 16],
    rflags: u64,
    fill: u8,
    regions: Vec<(u64, Vec<u8>)>,
    seed: UnifiedGuestCellSeed,
}

fn qwords(values: &[u64]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// A Windows-shaped GDT (`KGDT64_*`, the TSS at `0x40`), an IDT of 64-bit interrupt gates to
/// `CODE + 0x80` (DPL 3 for vectors 3, 4, 0x29, 0x2c and 0x2d) and a TSS with RSP0 and IST1-7.
fn tables(stack: u64) -> Vec<(u64, Vec<u8>)> {
    let tss_low = 0x0000_8b00_0000_0067u64
        | ((TSS & 0xffff) << 16)
        | (((TSS >> 16) & 0xff) << 32)
        | (((TSS >> 24) & 0xff) << 56);
    let gdt = qwords(&[
        0,
        0,
        0x0020_9b00_0000_0000, // 0x10 R0 code64
        0x00cf_9300_0000_ffff, // 0x18 R0 data
        0x00cf_fb00_0000_ffff, // 0x20 R3 code32
        0x00cf_f300_0000_ffff, // 0x28 R3 data
        0x0020_fb00_0000_0000, // 0x30 R3 code64
        0,
        tss_low, // 0x40 TSS (busy)
        TSS >> 32,
        0x0040_f300_0000_3c00, // 0x50 R3 TEB data
        0,
        0,
        0,
        0,
        0,
    ]);
    let mut idt = Vec::with_capacity(4096);
    let target = CODE + 0x80;
    for vector in 0..256u64 {
        let dpl = if matches!(vector, 3 | 4 | 0x29 | 0x2c | 0x2d) {
            3
        } else {
            0
        };
        let low = (target & 0xffff)
            | (0x10 << 16)
            | ((0x8e | (dpl << 5)) << 40)
            | (((target >> 16) & 0xffff) << 48);
        idt.extend(low.to_le_bytes());
        idt.extend((target >> 32).to_le_bytes());
    }
    let mut tss = vec![0u8; 0x68];
    tss[4..12].copy_from_slice(&(stack + 0x2000).to_le_bytes());
    for ist in 0..7 {
        let at = 0x24 + ist * 8;
        tss[at..at + 8].copy_from_slice(&(stack + 0x3000 + ist as u64 * 0x100).to_le_bytes());
    }
    vec![(GDT, gdt), (IDT, idt), (TSS, tss)]
}

/// The seeds of the cells: the SYSCALL MSRs and EFER of a running 64-bit kernel (SCE, LME, LMA,
/// NXE), the FP state of the variant, CR8 0.
fn seed(fp: GuestFpState) -> UnifiedGuestCellSeed {
    UnifiedGuestCellSeed {
        fp,
        efer: 0xd01,
        star: 0x0023_0010_0000_0000,
        lstar: 0xffff_f800_0050_0000,
        cstar: 0xffff_f800_0050_1000,
        syscall_mask: 0x4700,
        cr8: 0,
    }
}

/// V0: canonical kernel addresses in every register, a CPL0 IRETQ frame on the stack, reads of 0,
/// x87 after FNINIT. V1: low-half addresses, DF = 1 with CF/SF/OF, a CPL3 IRETQ frame, reads of
/// 0xA5, two values on the x87 stack (TOP = 6: ST0 = R6 = 1.0, ST1 = R7 = 2.0).
fn variants() -> Vec<Variant> {
    let mut v0 = [0u64; 16];
    for (index, register) in v0.iter_mut().enumerate() {
        *register = 0xffff_f800_0100_0000 + (index as u64) * 0x1_0000 + 0x40;
    }
    let rsp0 = 0xffff_f800_0800_0f00u64;
    v0[4] = rsp0;
    let mut regions0 = tables(0xffff_f800_0a00_0000);
    regions0.push((
        rsp0,
        qwords(&[CODE + 0x40, 0x10, 0x202, rsp0 + 0x400, 0x18, 0, 0, 0]),
    ));
    let mut v1 = [0u64; 16];
    for (index, register) in v1.iter_mut().enumerate() {
        *register = 0x0000_0100_0000_0000 + (index as u64) * 0x1_0000 + 0x80;
    }
    let rsp1 = 0xffff_f800_0900_0800u64;
    v1[4] = rsp1;
    let mut regions1 = tables(0xffff_f800_0b00_0000);
    regions1.push((
        rsp1,
        qwords(&[
            0x7ff6_0000_1000,
            0x33,
            0x202,
            0x7ff6_0000_8000,
            0x2b,
            0,
            0,
            0,
        ]),
    ));
    let mut fp1 = GuestFpState::initial();
    fp1.x87.registers[6] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f];
    fp1.x87.registers[7] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0x00, 0x40];
    fp1.x87.status_word = 6 << 11;
    fp1.x87.tag_word = 0x0fff;
    vec![
        Variant {
            gpr: v0,
            rflags: 0x246,
            fill: 0,
            regions: regions0,
            seed: seed(GuestFpState::initial()),
        },
        Variant {
            gpr: v1,
            rflags: 0x2 | 0x200 | 0x400 | 0x1 | 0x80 | 0x800,
            fill: 0xa5,
            regions: regions1,
            seed: seed(fp1),
        },
    ]
}

fn state(variant: &Variant) -> LongModeState {
    let data = DataSegmentDescriptorCache {
        base: 0,
        limit: 0xffff_ffff,
        access: 0x93,
        flags: 0xc,
    };
    let user = DataSegmentDescriptorCache {
        access: 0xf3,
        ..data
    };
    let flat = FsDescriptorCache {
        limit: 0xffff_ffff,
        access: 0xf3,
        flags: 0xc,
    };
    LongModeState {
        gpr: variant.gpr,
        rip: CODE,
        rflags: variant.rflags,
        cr0: Some(CR0),
        cr2: Some(0),
        cr4: Some(CR4),
        cr3: Some(CR3),
        gs_base: 0xffff_f800_0200_0000,
        fs_base: 0,
        ds_selector: 0x2b,
        es_selector: 0x2b,
        fs_selector: 0x53,
        gs_selector: 0x2b,
        cs_selector: Some(0x10),
        cs_cache: Some(CodeSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x9b,
            flags: 2,
        }),
        ss_selector: Some(0x18),
        ss_cache: Some(data),
        ds_cache: Some(user),
        es_cache: Some(user),
        fs_cache: Some(flat),
        gs_cache: Some(flat),
        gdtr: Some(Gdtr {
            base: GDT,
            limit: 0x7f,
        }),
        idtr: Some(Idtr {
            base: IDT,
            limit: 0xfff,
        }),
        tr: Some(TaskRegisterCache {
            selector: 0x40,
            base: TSS,
            limit: 0x67,
            access: 0x8b,
            flags: 0,
        }),
        ldtr_null_selector: Some(0),
        debug_registers: [
            Some(0),
            Some(0),
            Some(0),
            Some(0),
            None,
            None,
            Some(0xffff_0ff0),
            Some(0x400),
        ],
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ..LongModeState::default()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Outcome {
    Exec,
    Fault(u8),
    Unsupported,
    MemoryError,
    OtherExit,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum Verdict {
    Exec,
    Cond,
    Never,
    MemoryError,
    OtherExit,
}

impl Verdict {
    const fn name(self) -> &'static str {
        match self {
            Self::Exec => "exec",
            Self::Cond => "cond",
            Self::Never => "never",
            Self::MemoryError => "memory_error",
            Self::OtherExit => "other_exit",
        }
    }
}

#[derive(Clone)]
struct Classified {
    verdict: Verdict,
    operation: String,
    /// The vector of each variant whose outcome was an exception.
    faults: Vec<u8>,
    /// Some(v) when every variant raised the same vector `v` other than #PF.
    single_fault: Option<u8>,
    odd_width: Option<usize>,
    detail: String,
}

#[derive(Default)]
struct Bucket {
    count: u64,
    distinct: u64,
    files: BTreeMap<String, u64>,
    faults: BTreeMap<u8, u64>,
    example: Option<String>,
}

impl Bucket {
    fn add(&mut self, file: &str, example: impl FnOnce() -> String, faults: &[u8]) {
        self.count += 1;
        *self.files.entry(file.to_owned()).or_default() += 1;
        for vector in faults {
            *self.faults.entry(*vector).or_default() += 1;
        }
        if self.example.is_none() {
            self.example = Some(example());
        }
    }

    fn json(&self, mut fields: serde_json::Map<String, Value>) -> Value {
        let mut files: Vec<(&String, &u64)> = self.files.iter().collect();
        files.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        fields.insert("count".into(), json!(self.count));
        if self.distinct != 0 {
            fields.insert("distinct".into(), json!(self.distinct));
        }
        fields.insert("files".into(), json!(self.files.len()));
        fields.insert(
            "top_files".into(),
            json!(
                files
                    .iter()
                    .take(6)
                    .map(|(name, count)| format!("{name}:{count}"))
                    .collect::<Vec<_>>()
            ),
        );
        fields.insert(
            "faults".into(),
            json!(
                self.faults
                    .iter()
                    .map(|(vector, count)| (vector.to_string(), *count))
                    .collect::<BTreeMap<_, _>>()
            ),
        );
        fields.insert(
            "example".into(),
            json!(self.example.clone().unwrap_or_default()),
        );
        Value::Object(fields)
    }
}

/// The name of the operation: its `Debug` text up to the first `{`, `(` or space (diagnostic).
fn operation_name(operation: &Operation) -> String {
    let text = format!("{operation:?}");
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .next()
        .unwrap_or("")
        .to_owned()
}

/// The exec coverage of one command run.
pub(super) struct ExecCoverage {
    cells: UnifiedGuestCpl0Cells,
    variants: Vec<Variant>,
    cache: HashMap<Vec<u8>, Classified>,
    verdicts: BTreeMap<Verdict, u64>,
    families: HashMap<(Verdict, String), Bucket>,
    forms: HashMap<(Verdict, String, String), Bucket>,
    single_fault: HashMap<(String, String, u8), Bucket>,
    msrs: BTreeMap<(bool, u32), (u64, BTreeMap<String, u64>)>,
    odd_widths: BTreeMap<usize, u64>,
    decoded: u64,
    extra_instructions: u64,
    extra_refusals: HashMap<String, u64>,
    call_targets_outside_pdata: u64,
}

impl ExecCoverage {
    pub(super) fn windows() -> Self {
        Self {
            cells: UnifiedGuestCpl0Cells::new(UnifiedGuest::Windows),
            variants: variants(),
            cache: HashMap::new(),
            verdicts: BTreeMap::new(),
            families: HashMap::new(),
            forms: HashMap::new(),
            single_fault: HashMap::new(),
            msrs: BTreeMap::new(),
            odd_widths: BTreeMap::new(),
            decoded: 0,
            extra_instructions: 0,
            extra_refusals: HashMap::new(),
            call_targets_outside_pdata: 0,
        }
    }

    /// One step of `window` (the instruction at the start) in `variant`.
    fn run_one(
        &self,
        window: &[u8],
        msr_index: Option<u32>,
        variant: &Variant,
    ) -> Result<(Outcome, Option<usize>, String), Cpl0ProfileSetInputError> {
        self.cells.reset(&variant.seed);
        let mut code = [0x90u8; 16];
        let length = window.len().min(16);
        code[..length].copy_from_slice(&window[..length]);
        let mut memory = Sandbox {
            code,
            fill: variant.fill,
            regions: &variant.regions,
            odd_width: None,
        };
        let mut state = state(variant);
        if let Some(index) = msr_index {
            state.gpr[1] = u64::from(index);
            state.gpr[0] = 0;
            state.gpr[2] = 0;
        }
        let mut tsc = || 0x1234_5678u64;
        let set = self.cells.set();
        let result = run_with_cpl0_profile_set_and_pio(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext {
                    mode: InstructionMode::Long64,
                    privilege: PrivilegeLevel::Ring0,
                    origin: AccessOrigin::Explicit,
                },
            },
            od_isa::INTEGER_FLAGS,
            &set,
            Some(&mut tsc),
            &mut Ports,
            || false,
        )?;
        let (outcome, detail) = match result.exit {
            Cpl0ProfileSetExit::Run(RunExit::Unsupported { .. }) => {
                (Outcome::Unsupported, String::new())
            }
            Cpl0ProfileSetExit::Run(RunExit::Fault(fault)) => {
                (Outcome::Fault(fault.vector), String::new())
            }
            Cpl0ProfileSetExit::Run(RunExit::MemoryError(error)) => {
                (Outcome::MemoryError, format!("{error:?}"))
            }
            Cpl0ProfileSetExit::Run(
                RunExit::InstructionBudgetExhausted
                | RunExit::RepIterationBudgetExhausted
                | RunExit::Halted,
            ) => (Outcome::Exec, String::new()),
            Cpl0ProfileSetExit::Run(other) => (Outcome::OtherExit, format!("{other:?}")),
            Cpl0ProfileSetExit::TranslationContextChange(_)
            | Cpl0ProfileSetExit::PortIo(_)
            | Cpl0ProfileSetExit::UserEntry(_)
            | Cpl0ProfileSetExit::UserModeEntered(_) => (Outcome::Exec, String::new()),
            // A future exit kind is not claimed as executed.
            other => (Outcome::OtherExit, format!("{other:?}")),
        };
        Ok((outcome, memory.odd_width, detail))
    }

    fn classify(
        &self,
        window: &[u8],
        msr_index: Option<u32>,
        instruction: &Instruction,
    ) -> Result<Classified, Cpl0ProfileSetInputError> {
        let mut outcomes = Vec::new();
        let mut odd_width = None;
        let mut details = Vec::new();
        for variant in &self.variants {
            let (outcome, width, detail) = self.run_one(window, msr_index, variant)?;
            odd_width = odd_width.or(width);
            if !detail.is_empty() {
                details.push(detail);
            }
            outcomes.push(outcome);
        }
        let unsupported = outcomes
            .iter()
            .filter(|outcome| **outcome == Outcome::Unsupported)
            .count();
        let verdict = if unsupported == outcomes.len() {
            Verdict::Never
        } else if unsupported > 0 {
            Verdict::Cond
        } else if outcomes.contains(&Outcome::MemoryError) {
            Verdict::MemoryError
        } else if outcomes.contains(&Outcome::OtherExit) {
            Verdict::OtherExit
        } else {
            Verdict::Exec
        };
        let faults: Vec<u8> = outcomes
            .iter()
            .filter_map(|outcome| match outcome {
                Outcome::Fault(vector) => Some(*vector),
                _ => None,
            })
            .collect();
        let single_fault = (faults.len() == outcomes.len()
            && faults.iter().all(|vector| *vector == faults[0])
            && faults[0] != 14)
            .then(|| faults[0]);
        Ok(Classified {
            verdict,
            operation: operation_name(&instruction.operation),
            faults,
            single_fault,
            odd_width,
            detail: details.join("|"),
        })
    }

    /// Record one decoded instruction of `file` at `rva`: `bytes` is the instruction, `window` the
    /// bytes from its start (up to 15), `last_ecx` the immediate of the last `mov ecx, imm32` of the
    /// same sweep, `extra` whether the sweep is one from a call target outside `.pdata`.
    pub(super) fn observe(
        &mut self,
        file: &str,
        rva: u32,
        window: &[u8],
        bytes: &[u8],
        instruction: &Instruction,
        last_ecx: Option<u32>,
        extra: bool,
    ) -> Result<(), Cpl0ProfileSetInputError> {
        self.decoded += 1;
        if extra {
            self.extra_instructions += 1;
        }
        let msr = matches!(
            instruction.operation,
            Operation::ReadMsr | Operation::WriteMsr
        );
        let msr_index = msr.then(|| last_ecx.unwrap_or(UNKNOWN_MSR));
        let mut key = bytes.to_vec();
        if let Some(index) = msr_index {
            key.extend(index.to_le_bytes());
            let entry = self
                .msrs
                .entry((instruction.operation == Operation::WriteMsr, index))
                .or_default();
            entry.0 += 1;
            *entry.1.entry(file.to_owned()).or_default() += 1;
        }
        let classified = match self.cache.get(&key) {
            Some(classified) => classified.clone(),
            None => {
                let classified = self.classify(window, msr_index, instruction)?;
                self.cache.insert(key, classified.clone());
                if let Some(width) = classified.odd_width {
                    *self.odd_widths.entry(width).or_default() += 1;
                }
                if let Some(bucket) = self
                    .families
                    .get_mut(&(classified.verdict, classified.operation.clone()))
                {
                    bucket.distinct += 1;
                } else {
                    let mut bucket = Bucket::default();
                    bucket.distinct = 1;
                    self.families
                        .insert((classified.verdict, classified.operation.clone()), bucket);
                }
                classified
            }
        };
        *self.verdicts.entry(classified.verdict).or_default() += 1;
        let example = || {
            let mut text = format!("{file}+{rva:#x} {}", super::hex(bytes));
            if !classified.detail.is_empty() {
                text.push(' ');
                text.push_str(&classified.detail);
            }
            text
        };
        if classified.verdict != Verdict::Exec {
            self.families
                .entry((classified.verdict, classified.operation.clone()))
                .or_default()
                .add(file, example, &classified.faults);
            self.forms
                .entry((
                    classified.verdict,
                    classified.operation.clone(),
                    super::signature(bytes),
                ))
                .or_default()
                .add(file, example, &classified.faults);
        } else if let Some(vector) = classified.single_fault {
            self.single_fault
                .entry((
                    classified.operation.clone(),
                    super::signature(bytes),
                    vector,
                ))
                .or_default()
                .add(file, example, &classified.faults);
        }
        Ok(())
    }

    /// A decoder refusal found by a sweep from a call target outside `.pdata`.
    pub(super) fn extra_refusal(&mut self, form: String) {
        *self.extra_refusals.entry(form).or_default() += 1;
    }

    pub(super) fn call_targets_outside_pdata(&mut self, count: u64) {
        self.call_targets_outside_pdata += count;
    }

    pub(super) fn report(&self, top: usize) -> Value {
        let mut families: Vec<(&(Verdict, String), &Bucket)> = self
            .families
            .iter()
            .filter(|((verdict, _), bucket)| *verdict != Verdict::Exec && bucket.count != 0)
            .collect();
        families.sort_by(|a, b| b.1.count.cmp(&a.1.count).then(a.0.cmp(b.0)));
        let mut forms: Vec<(&(Verdict, String, String), &Bucket)> = self.forms.iter().collect();
        forms.sort_by(|a, b| b.1.count.cmp(&a.1.count).then(a.0.cmp(b.0)));
        let mut single: Vec<(&(String, String, u8), &Bucket)> = self.single_fault.iter().collect();
        single.sort_by(|a, b| b.1.count.cmp(&a.1.count).then(a.0.cmp(b.0)));
        let mut refusals: Vec<(&String, &u64)> = self.extra_refusals.iter().collect();
        refusals.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let object = |pairs: Vec<(&str, Value)>| {
            Value::Object(
                pairs
                    .into_iter()
                    .map(|(key, value)| (key.to_owned(), value))
                    .collect(),
            )
        };
        json!({
            "set": "windows",
            "variants": self.variants.len(),
            "decoded": self.decoded,
            "distinct": self.cache.len(),
            "verdicts": self.verdicts.iter().map(|(verdict, count)| (verdict.name(), *count)).collect::<BTreeMap<_, _>>(),
            "families": families.iter().take(top).map(|((verdict, operation), bucket)| {
                let Value::Object(fields) = object(vec![("verdict", json!(verdict.name())), ("operation", json!(operation))]) else { unreachable!() };
                bucket.json(fields)
            }).collect::<Vec<_>>(),
            "forms": forms.iter().take(top).map(|((verdict, operation, form), bucket)| {
                let Value::Object(fields) = object(vec![("verdict", json!(verdict.name())), ("operation", json!(operation)), ("form", json!(form))]) else { unreachable!() };
                bucket.json(fields)
            }).collect::<Vec<_>>(),
            "exec_single_fault": single.iter().take(top).map(|((operation, form, vector), bucket)| {
                let Value::Object(fields) = object(vec![("operation", json!(operation)), ("form", json!(form)), ("vector", json!(vector))]) else { unreachable!() };
                bucket.json(fields)
            }).collect::<Vec<_>>(),
            "msr_indexes": self.msrs.iter().map(|((write, index), (count, files))| {
                let mut key = vec![0x0f, if *write { 0x30 } else { 0x32 }];
                key.extend(index.to_le_bytes());
                json!({
                    "op": if *write { "wrmsr" } else { "rdmsr" },
                    "index": format!("{index:#x}"),
                    "index_known": *index != UNKNOWN_MSR,
                    "count": count,
                    "verdict": self.cache.get(&key).map_or("?", |classified| classified.verdict.name()),
                    "files": files.iter().map(|(name, count)| format!("{name}:{count}")).collect::<Vec<_>>(),
                })
            }).collect::<Vec<_>>(),
            "call_targets_outside_pdata": self.call_targets_outside_pdata,
            "extra_instructions": self.extra_instructions,
            "extra_decoder_refusals": refusals.iter().take(top).map(|(form, count)| json!({"form": form, "count": count})).collect::<Vec<_>>(),
            "odd_widths": self.odd_widths.iter().map(|(width, count)| json!({"width": width, "distinct": count})).collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One step of `code` in `variant` with RCX/RAX/RDX given; the state after it.
    fn step(
        cells: &UnifiedGuestCpl0Cells,
        variant: &Variant,
        code: &[u8],
        rcx_rax_rdx: [u64; 3],
    ) -> LongModeState {
        let mut bytes = [0x90u8; 16];
        bytes[..code.len()].copy_from_slice(code);
        let mut memory = Sandbox {
            code: bytes,
            fill: variant.fill,
            regions: &variant.regions,
            odd_width: None,
        };
        let mut state = state(variant);
        [state.gpr[1], state.gpr[0], state.gpr[2]] = rcx_rax_rdx;
        let set = cells.set();
        let result = run_with_cpl0_profile_set_and_pio(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext {
                    mode: InstructionMode::Long64,
                    privilege: PrivilegeLevel::Ring0,
                    origin: AccessOrigin::Explicit,
                },
            },
            od_isa::INTEGER_FLAGS,
            &set,
            None,
            &mut Ports,
            || false,
        )
        .unwrap();
        assert_eq!(
            result.exit,
            Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted)
        );
        state
    }

    #[test]
    fn a_reset_gives_every_step_the_seeded_cells_back() {
        // S7-32 slice S: a WRMSR of one sandbox step must not leak into the next one.
        const IA32_STAR: u64 = 0xc000_0081;
        let coverage = ExecCoverage::windows();
        let variant = &coverage.variants[0];
        coverage.cells.reset(&variant.seed);
        step(
            &coverage.cells,
            variant,
            &[0x0f, 0x30],
            [IA32_STAR, 0, 0x0013_0008],
        );
        let read = step(&coverage.cells, variant, &[0x0f, 0x32], [IA32_STAR, 0, 0]);
        assert_eq!((read.gpr[0], read.gpr[2]), (0, 0x0013_0008), "WRMSR stored");
        coverage.cells.reset(&variant.seed);
        let read = step(&coverage.cells, variant, &[0x0f, 0x32], [IA32_STAR, 0, 0]);
        let star = variant.seed.star;
        assert_eq!(
            (read.gpr[0], read.gpr[2]),
            (star & 0xffff_ffff, star >> 32),
            "the reset gave the seeded STAR back"
        );
    }

    #[test]
    fn the_seeded_efer_is_the_one_a_step_reads() {
        // The cells of a fresh vCPU have EFER `FIRMWARE_EFER` (no SCE); the seed sets SCE.
        let coverage = ExecCoverage::windows();
        let variant = &coverage.variants[1];
        coverage.cells.reset(&variant.seed);
        let read = step(&coverage.cells, variant, &[0x0f, 0x32], [0xc000_0080, 0, 0]);
        assert_eq!(read.gpr[0], 0xd01);
    }

    #[test]
    fn every_variant_of_a_classified_form_starts_from_the_seeded_cells() {
        // S7-32 slice S review: `run_one` resets and seeds the cells before each variant. Here the
        // cells are left in another state first (EFER without SCE, as a fresh vCPU or a WRMSR of an
        // earlier form would leave them); `SYSRET` (REX.W 0F 07) must still get the verdict of the
        // seeded cells (`never`: the set refuses the return to CPL3 in both variants), not the `#UD`
        // of EFER.SCE = 0 (`exec`).
        let mut coverage = ExecCoverage::windows();
        let sysret = [0x48u8, 0x0f, 0x07];
        let mut memory = Sandbox {
            code: {
                let mut code = [0x90u8; 16];
                code[..3].copy_from_slice(&sysret);
                code
            },
            fill: 0,
            regions: &[],
            odd_width: None,
        };
        let context = AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        };
        let instruction = od_isa::long_decode::decode(&mut memory, CODE, context).unwrap();
        let mut polluted = coverage.variants[0].seed.clone();
        polluted.efer = 0xd00;
        coverage.cells.reset(&polluted);
        coverage
            .observe("t.sys", 0x1000, &sysret, &sysret, &instruction, None, false)
            .unwrap();
        assert_eq!(coverage.verdicts.get(&Verdict::Never), Some(&1));
        assert_eq!(coverage.verdicts.get(&Verdict::Exec), None);
        // The control: with the polluted cells and no reset the same form is an `#UD` (`exec`).
        let variant = &coverage.variants[0];
        coverage.cells.reset(&polluted);
        let mut code = [0x90u8; 16];
        code[..3].copy_from_slice(&sysret);
        let mut memory = Sandbox {
            code,
            fill: variant.fill,
            regions: &variant.regions,
            odd_width: None,
        };
        let mut cpu = state(variant);
        let set = coverage.cells.set();
        let result = run_with_cpl0_profile_set_and_pio(
            &mut cpu,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context,
            },
            od_isa::INTEGER_FLAGS,
            &set,
            None,
            &mut Ports,
            || false,
        )
        .unwrap();
        assert!(
            matches!(result.exit, Cpl0ProfileSetExit::Run(RunExit::Fault(ref f)) if f.vector == 6),
            "{:?}",
            result.exit
        );
    }
}
