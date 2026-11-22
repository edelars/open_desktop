//! A 32-bit protected-mode step over [`GuestMemory`] on PAE paging (S2-25o, bounded slice).
//!
//! [`ProtectedCpu`] (`od-isa`, `protected_exec.rs`) is the closed profile of the AP trampoline: it decodes
//! 32-bit instructions and executes them over a [`RealModeMemory`] that is *physical* memory (it walks
//! the paging structures itself). [`PagedPaeMemoryAdapter`] (S2-25l) is the `GuestMemory` of a vCPU on
//! PAE paging: *linear* addresses, the walk, the accessed and dirty bits and the MMIO dispatch are its
//! own. This module joins them without a second decoder or a second instruction semantics:
//!
//! - [`LinearView`] is a `RealModeMemory` over a `GuestMemory` that says `walks_linear`: the model takes the
//!   address it computes (segment base plus offset) as a linear address of the adapter, does not walk
//!   (and so keeps no `Touch` list), asks for an instruction byte through `fetch_u8` (the adapter's
//!   `fetch`: the walk of a fetch, `XD`, RAM only) and for a DWORD operand through `read_u32_linear` /
//!   `write_u32_linear` (one adapter access: one 4-byte access of a device, one dirty-bit write, a page
//!   crossing handled by the adapter). Control registers, PDPTEs, `CR0.PG`, `EFER` are those of the
//!   model, unchanged.
//! - [`step_pae_protected`] steps a copy of the model through [`ProtectedCpu::step_admitting`] with the
//!   covered set of [`pae_covers`]; the copy replaces the model only when the instruction retired, so a
//!   fault or a refusal leaves the model as it was (the accessed bits the adapter set stay, as in the
//!   long-mode adapter).
//! - [`LoadedFirmwareBoard::pae_step`] runs it for a vCPU of the board that is in the legacy
//!   protected-mode model on PAE paging, over the whole bus (an MMIO operand reaches the bound device as
//!   an access of that vCPU).
//! - [`run_pae_state_step`] runs it for a `LongModeState` that is such a vCPU (the host loop of
//!   `od-host`, S2-25o): the model is built from the state under the assumptions listed there.
//!
//! The covered set (the decision of this slice): the forms of `ProtectedCpu` that need only registers
//! and linear operands - `ADD ESP, imm32`, `BT m32, imm8`, `JAE`/`JNE`/`JE rel8`, `MOV EAX, moffs32`,
//! `MOV EDX, m32`, `MOV moffs32, EAX`, `CMP EAX|EDX, m32`, `RDMSR`, `WRMSR`, `WBINVD`, `MOV EAX|ECX,
//! imm32`. Not covered, with the instruction named in [`PaeProtectedStop::Uncovered`]: `MOV Sreg, EDX` and
//! `JMP ptr16:32` (they read the descriptor table, an access the adapter refuses: S2-25l), `MOV
//! CR0|CR3|CR4, EAX` (they reload the PDPTEs from *physical* memory and change the paging the adapter was
//! built from) and `JMP [rip+disp32]` (64-bit mode, not PAE). A `ProtectedCpu` refusal of its own (an
//! unsupported form, a limit, a segment check) is [`PaeProtectedStop::Refused`], the `#PF` of an access
//! is [`PaeStepOutcome::Fault`] with the record of the adapter.
//!
//! Differences from [`ProtectedCpu::step`] over physical memory, on purpose and checked by the
//! differential of the tests: the fetch is one adapter walk per byte read (the model walks the same
//! bytes), the accessed bit of an entry of a walk that later faults stays set (the model discards the
//! walks of a faulting instruction), the lookahead bytes of an unsupported-form refusal are fetched
//! through the adapter (their accessed bits are set), a DWORD operand that wraps past 4 GiB is a
//! memory refusal (the adapter's rule), an MMIO operand reaches its device (the model refuses
//! non-RAM). No hardware oracle: the SDM rules of the model and of the adapter are the reference.

use crate::vcpu::LegacyCpu;
use crate::{GuestPhysicalRange, LoadedFirmwareBoard, MachineBus, PagedPaeMemoryAdapter};
use crate::{PaeConfigError, PaeProbeConfig};
use od_isa::LongModeState;
use od_isa::legacy_mode::ProtectedSegment;
use od_isa::memory::{
    AccessContext, AccessOrigin, ArchitecturalFault, GuestMemory, InstructionMode, MemoryError,
    PageFaultRecord, PrivilegeLevel,
};
use od_isa::protected_exec::{
    CR0_PG, CR4_PAE, EFER_LMA, EFER_LME, ProtectedCpu, ProtectedInstruction, ProtectedRefusal,
};
use od_isa::real_mode_exec::{CR0_PE, RealModeMemory, TableRegister};
use std::cell::RefCell;

#[cfg(test)]
mod tests;

/// Whether the step of this module executes `instruction` (see the module documentation).
pub fn pae_covers(instruction: &ProtectedInstruction) -> bool {
    use ProtectedInstruction::{
        AddEspImm32, BtDsImm8, CmpEaxWithDs, CmpEdxWithDs, JaeRel8, JeRel8, JneRel8, MovDsFromEax,
        MovEaxFromDs, MovEaxImm32, MovEcxImm32, MovEdxFromDs, Rdmsr, Wbinvd, Wrmsr,
    };
    matches!(
        instruction,
        AddEspImm32(_)
            | BtDsImm8 { .. }
            | JaeRel8(_)
            | JneRel8(_)
            | JeRel8(_)
            | MovEaxFromDs(_)
            | MovEdxFromDs(_)
            | MovDsFromEax(_)
            | CmpEaxWithDs(_)
            | CmpEdxWithDs(_)
            | Rdmsr
            | Wrmsr
            | Wbinvd
            | MovEaxImm32(_)
            | MovEcxImm32(_)
    )
}

/// [`pae_covers`] without `WRMSR`: for a caller that owns `EFER` outside the model (the host loop
/// takes it from its configuration and cannot take a written value back).
pub fn pae_covers_without_efer_write(instruction: &ProtectedInstruction) -> bool {
    !matches!(instruction, ProtectedInstruction::Wrmsr) && pae_covers(instruction)
}

/// Why a step that retired nothing stopped, other than by a guest fault.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PaeProtectedStop {
    /// The model is not a protected-mode one on PAE paging (`CR0.PG`, `CR4.PAE` or `LMA` say otherwise).
    NotPaePaging,
    /// A `LongModeState` cannot be seen as the model (see [`run_pae_state_step`]).
    State(PaeStateRefusal),
    /// The decoded instruction is outside [`pae_covers`]; nothing of it was executed.
    Uncovered {
        eip: u32,
        instruction: ProtectedInstruction,
        length: u32,
    },
    /// `ProtectedCpu` refused (an unsupported form, a limit, a #GP of the model, ...).
    Refused(ProtectedRefusal),
    /// The memory refused without a guest fault (a device error, an address the adapter does not
    /// serve, RAM that is not there): the typed text of the adapter's error.
    Memory(MemoryError),
}

/// The result of one step.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PaeStepOutcome {
    /// The instruction retired; the model is the state after it.
    Retired(ProtectedInstruction),
    /// The access of the instruction (or the fetch) raised an architectural fault (a `#PF`); nothing
    /// retired and the model is as it was.
    Fault(ArchitecturalFault),
    /// Nothing retired and nothing was faulted.
    Stopped(PaeProtectedStop),
}

/// A [`RealModeMemory`] over a [`GuestMemory`] of linear addresses (the module documentation).
struct LinearView<'a, G: GuestMemory + ?Sized> {
    memory: RefCell<&'a mut G>,
    /// The first error of the guest memory during the step (later ones are lookahead).
    failure: RefCell<Option<MemoryError>>,
}

impl<'a, G: GuestMemory + ?Sized> LinearView<'a, G> {
    fn new(memory: &'a mut G) -> Self {
        Self {
            memory: RefCell::new(memory),
            failure: RefCell::new(None),
        }
    }

    fn context() -> AccessContext {
        // The model is CPL 0 (`ProtectedCpu`); the adapter normalizes the origin.
        AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        }
    }

    fn note<T>(&self, result: Result<T, MemoryError>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                self.failure.borrow_mut().get_or_insert(error);
                None
            }
        }
    }

    fn take_failure(&self) -> Option<MemoryError> {
        self.failure.borrow_mut().take()
    }
}

impl<G: GuestMemory + ?Sized> RealModeMemory for LinearView<'_, G> {
    fn walks_linear(&self) -> bool {
        true
    }

    fn fetch_u8(&self, linear: u32) -> Option<u8> {
        let result = self
            .memory
            .borrow_mut()
            .fetch(u64::from(linear), Self::context());
        self.note(result)
    }

    fn read_u8(&self, linear: u32) -> Option<u8> {
        let mut byte = [0u8];
        let result = self
            .memory
            .borrow_mut()
            .read(u64::from(linear), Self::context(), &mut byte);
        self.note(result).map(|()| byte[0])
    }

    // The byte write of the model is the descriptor accessed bit and the page tables' bits: the
    // instructions that do either are not covered, so nothing writes by byte here.
    fn can_write(&self, _linear: u32) -> bool {
        false
    }

    fn write_u8(&mut self, _linear: u32, _value: u8) -> bool {
        false
    }

    fn read_u32_linear(&self, linear: u32) -> Option<u32> {
        let mut bytes = [0u8; 4];
        let result = self
            .memory
            .borrow_mut()
            .read(u64::from(linear), Self::context(), &mut bytes);
        self.note(result).map(|()| u32::from_le_bytes(bytes))
    }

    fn write_u32_linear(&mut self, linear: u32, value: u32) -> bool {
        let result =
            self.memory
                .get_mut()
                .write(u64::from(linear), Self::context(), &value.to_le_bytes());
        self.note(result).is_some()
    }
}

/// One instruction of `cpu` through `memory`, the `GuestMemory` of its PAE paging (a
/// [`PagedPaeMemoryAdapter`] built from `cpu` before the call), with `admit` choosing the instructions
/// ([`pae_covers`] is the set of this slice). `cpu` changes only when the instruction retired.
pub fn step_pae_protected_with<G: GuestMemory + ?Sized>(
    cpu: &mut ProtectedCpu,
    memory: &mut G,
    admit: &dyn Fn(&ProtectedInstruction) -> bool,
) -> PaeStepOutcome {
    if !cpu.pae_paging() || cpu.cr0 & CR0_PE == 0 {
        return PaeStepOutcome::Stopped(PaeProtectedStop::NotPaePaging);
    }
    let mut next = *cpu;
    let mut view = LinearView::new(memory);
    let result = next.step_admitting(&mut view, admit);
    let failure = view.take_failure();
    match result {
        Ok(instruction) => {
            *cpu = next;
            PaeStepOutcome::Retired(instruction)
        }
        Err(ProtectedRefusal::InstructionNotAdmitted {
            eip,
            instruction,
            length,
        }) => PaeStepOutcome::Stopped(PaeProtectedStop::Uncovered {
            eip,
            instruction,
            length,
        }),
        Err(
            refusal @ (ProtectedRefusal::FetchUnreadable { .. }
            | ProtectedRefusal::DataUnreadable { .. }
            | ProtectedRefusal::DataUnwritable { .. }),
        ) => match failure {
            Some(MemoryError::Architectural(fault)) => PaeStepOutcome::Fault(fault),
            Some(other) => PaeStepOutcome::Stopped(PaeProtectedStop::Memory(other)),
            None => PaeStepOutcome::Stopped(PaeProtectedStop::Refused(refusal)),
        },
        Err(refusal) => PaeStepOutcome::Stopped(PaeProtectedStop::Refused(refusal)),
    }
}

/// [`step_pae_protected_with`] with the covered set of this slice.
pub fn step_pae_protected<G: GuestMemory + ?Sized>(
    cpu: &mut ProtectedCpu,
    memory: &mut G,
) -> PaeStepOutcome {
    step_pae_protected_with(cpu, memory, &pae_covers)
}

/// One step of a vCPU of the board, and the `#PF` record when it faulted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PaeBoardStep {
    pub(crate) outcome: PaeStepOutcome,
    pub(crate) page_fault: Option<PageFaultRecord>,
}

impl LoadedFirmwareBoard {
    /// One instruction of vCPU `cpu`, a legacy protected-mode model on PAE paging, through the
    /// [`PagedPaeMemoryAdapter`] over the whole bus ([`Self::pae_memory_adapter`]): the covered set of
    /// [`pae_covers`], MMIO operands to the bound device as accesses of that vCPU. `committed_write`
    /// receives every write of the access in order. The model of the vCPU changes only when the
    /// instruction retired. An error text when the vCPU does not exist or is not a protected-mode
    /// model on PAE paging.
    // No production caller yet (like the adapter): the tests exercise it now.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn pae_step(
        &mut self,
        cpu: usize,
        committed_write: &mut dyn FnMut(GuestPhysicalRange),
    ) -> Result<PaeBoardStep, String> {
        let mut model = match self
            .vcpu(cpu)
            .ok_or_else(|| format!("no vCPU {cpu}"))?
            .legacy
        {
            Some(LegacyCpu::Protected(model)) => model,
            Some(LegacyCpu::Real(_)) => return Err(format!("vCPU {cpu} is in real mode")),
            None => return Err(format!("vCPU {cpu} is not in a legacy mode")),
        };
        let mut adapter = self.pae_memory_adapter(cpu, u64::from(model.eip), committed_write)?;
        let outcome = step_pae_protected(&mut model, &mut adapter);
        let page_fault = adapter.page_fault().cloned();
        drop(adapter);
        if matches!(outcome, PaeStepOutcome::Retired(_)) {
            self.vcpu_mut(cpu)
                .ok_or_else(|| format!("no vCPU {cpu}"))?
                .legacy = Some(LegacyCpu::Protected(model));
        }
        Ok(PaeBoardStep {
            outcome,
            page_fault,
        })
    }
}

/// Why a [`LongModeState`] is not a protected-mode vCPU that [`run_pae_state_step`] can run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PaeStateRefusal {
    /// `CR0` or `CR4` is not carried by the state (`None`).
    MissingRegister { register: &'static str },
    /// The state is not PAE paging of a protected-mode vCPU: `CR0.PE`, `CR0.PG`, `CR4.PAE` set,
    /// `EFER.LME` and `EFER.LMA` clear.
    NotPaeProtected { cr0: u64, cr4: u64, efer: u64 },
    /// A value does not fit the 32-bit model (`what` names it).
    Wide { what: &'static str, value: u64 },
    /// `state.cr3` and the CR3 of the call disagree.
    Cr3Mismatch { state_cr3: u64, cr3: u64 },
    /// The CS selector has RPL != 0: the model is CPL 0.
    NotCpl0 { cs_selector: u16 },
    /// DS is marked unusable: the model has no null DS.
    DataSegmentUnusable,
    /// The PAE snapshot cannot be built (control registers, or the PDPTEs of `CR3`: reserved bits,
    /// not readable RAM).
    Paging(PaeConfigError),
}

/// A flat 32-bit segment of the model: base 0, 4 GiB limit, 32-bit default size, 4 KiB granularity.
fn flat_segment(selector: u16, access: u8) -> ProtectedSegment {
    ProtectedSegment {
        selector,
        base: 0,
        limit: 0xFFFF_FFFF,
        access,
        flags: 0x0C,
    }
}

fn narrow(what: &'static str, value: u64) -> Result<u32, PaeStateRefusal> {
    u32::try_from(value).map_err(|_| PaeStateRefusal::Wide { what, value })
}

/// Whether `state` with `efer` is PAE paging of a protected-mode vCPU (`CR0.PE = PG = 1`, `CR4.PAE = 1`,
/// `EFER.LME = LMA = 0`): what the long-mode entries refuse as `ProbeConfigError::PaeProtectedMode` and
/// the host loop runs through [`run_pae_state_step`].
pub fn is_pae_protected_state(state: &LongModeState, efer: u64) -> bool {
    let (Some(cr0), Some(cr4)) = (state.cr0, state.cr4) else {
        return false;
    };
    cr0 & u64::from(CR0_PE | CR0_PG) == u64::from(CR0_PE | CR0_PG)
        && cr4 & u64::from(CR4_PAE) != 0
        && efer & (EFER_LME | EFER_LMA) == 0
}

/// The protected-mode model of a PAE [`LongModeState`]. The assumptions of the host loop's PAE path,
/// each one a refusal when it does not hold: registers `EAX..EDI` are the low halves of the state's
/// `gpr[0..8]`, `EIP` is `rip` (below 4 GiB), `EFLAGS` the low half of `rflags`; `CR0`, `CR4` and `CR3`
/// (`state.cr3` if carried, which must equal `cr3`) fit 32 bits; `EFER` is the argument; the PDPTEs are
/// loaded from `CR3` now (the state carries none: the model re-reads them at every step, so a change of
/// the PDPT in memory without a `MOV CR3` is seen, which hardware does not); CPL is 0; CS and DS are the
/// hidden caches of the state (`cs_cache`, `ds_cache`) or, when it has none, flat 32-bit segments
/// (base 0, limit 4 GiB, CS `D = 1`); ES, SS, FS, GS are flat (no covered instruction uses them); GDTR and
/// IDTR are empty (no covered instruction reads a table).
fn model_of_state(
    state: &LongModeState,
    bus: &MachineBus,
    cr3: u64,
    efer: u64,
) -> Result<(ProtectedCpu, PaeProbeConfig), PaeStateRefusal> {
    let cr0 = state
        .cr0
        .ok_or(PaeStateRefusal::MissingRegister { register: "CR0" })?;
    let cr4 = state
        .cr4
        .ok_or(PaeStateRefusal::MissingRegister { register: "CR4" })?;
    if !is_pae_protected_state(state, efer) {
        return Err(PaeStateRefusal::NotPaeProtected { cr0, cr4, efer });
    }
    if let Some(state_cr3) = state.cr3
        && state_cr3 != cr3
    {
        return Err(PaeStateRefusal::Cr3Mismatch { state_cr3, cr3 });
    }
    let cs_selector = state.cs_selector.unwrap_or(0x08);
    if cs_selector & 3 != 0 {
        return Err(PaeStateRefusal::NotCpl0 { cs_selector });
    }
    if state.ds_unusable {
        return Err(PaeStateRefusal::DataSegmentUnusable);
    }
    let cs = match state.cs_cache {
        Some(cache) => ProtectedSegment {
            selector: cs_selector,
            base: narrow("CS base", cache.base)?,
            limit: cache.limit,
            access: cache.access,
            flags: cache.flags,
        },
        None => flat_segment(cs_selector, 0x9B),
    };
    let ds = match state.ds_cache {
        Some(cache) => ProtectedSegment {
            selector: state.ds_selector,
            base: cache.base,
            limit: cache.limit,
            access: cache.access,
            flags: cache.flags,
        },
        None => flat_segment(state.ds_selector, 0x93),
    };
    let paging = PaeProbeConfig::load(bus.physical_memory(), cr0, cr3, cr4, efer)
        .map_err(PaeStateRefusal::Paging)?;
    let mut gpr = [0u32; 8];
    for (slot, value) in gpr.iter_mut().zip(state.gpr) {
        *slot = value as u32;
    }
    let data = |selector| flat_segment(selector, 0x93);
    let model = ProtectedCpu {
        gpr,
        eip: narrow("RIP", state.rip)?,
        rip_high: 0,
        target_entered: false,
        eflags: state.rflags as u32,
        cr0: narrow("CR0", cr0)?,
        cr3: narrow("CR3", cr3)?,
        cr4: narrow("CR4", cr4)?,
        efer,
        pdptes: paging.pdptes(),
        pse36: false,
        cs,
        ds,
        es: data(state.es_selector),
        ss: data(state.ss_selector.unwrap_or(0x10)),
        fs: data(state.fs_selector),
        gs: data(state.gs_selector),
        gdtr: TableRegister { base: 0, limit: 0 },
        idtr: TableRegister { base: 0, limit: 0 },
        mov_ss_shadow: false,
    };
    Ok((model, paging))
}

/// One step of a [`LongModeState`] that is PAE paging of a protected-mode vCPU (the host loop's PAE
/// path, S2-25o): the model of [`model_of_state`] through the [`PagedPaeMemoryAdapter`] over the
/// whole bus as vCPU `vcpu` (an MMIO operand reaches its device), the covered set without `WRMSR`
/// ([`pae_covers_without_efer_write`]: `EFER` is the caller's, a written value could not be handed
/// back). When the instruction retired the state takes `EAX..EDI` (the low halves; the high ones are
/// kept), `rip`, the low half of `rflags`; nothing else of it changes, as no covered instruction
/// changes another register. Returns the outcome and the `#PF` record of a fault.
pub fn run_pae_state_step(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    vcpu: usize,
    cr3: u64,
    efer: u64,
    committed_write: &mut dyn FnMut(GuestPhysicalRange),
) -> (PaeStepOutcome, Option<PageFaultRecord>) {
    let (mut model, paging) = match model_of_state(state, bus, cr3, efer) {
        Ok(pair) => pair,
        Err(refusal) => {
            return (
                PaeStepOutcome::Stopped(PaeProtectedStop::State(refusal)),
                None,
            );
        }
    };
    let mut adapter = PagedPaeMemoryAdapter::over_machine(
        bus,
        vcpu,
        paging,
        u64::from(model.eip),
        committed_write,
    );
    let outcome = step_pae_protected_with(&mut model, &mut adapter, &pae_covers_without_efer_write);
    let page_fault = adapter.page_fault().cloned();
    drop(adapter);
    if matches!(outcome, PaeStepOutcome::Retired(_)) {
        for (slot, value) in state.gpr.iter_mut().zip(model.gpr) {
            *slot = (*slot & !0xFFFF_FFFF) | u64::from(value);
        }
        state.rip = u64::from(model.eip);
        state.rflags = (state.rflags & !0xFFFF_FFFF) | u64::from(model.eflags);
    }
    (outcome, page_fault)
}
