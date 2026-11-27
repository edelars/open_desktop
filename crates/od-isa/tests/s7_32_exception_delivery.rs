//! S7-32 slice Q: `deliver_exception_cpl0_set` against an independent model of Intel SDM
//! 325462-093US, Vol. 3A chapter 7 (Table 7-1 vectors and error codes, §7.12.1.3 flags, §7.13 error
//! codes, §7.14 the 64-bit frame, IST and stack switch, Tables 7-4/7-5 double fault and shutdown),
//! Vol. 3B §20.3.1.1 (RF in the frame) and the IA-32e branch of the Vol. 2A "INT n" pseudocode (the
//! order of the checks). The model below is written from the SDM text, not from the
//! implementation: it walks the same checks on a read-only view of the test memory and predicts
//! the outcome, the frame and the state.

use od_isa::memory::{
    AccessContext, AccessOrigin, ArchitecturalFault, GuestMemory, InstructionMode, MemoryError,
    PrivilegeLevel,
};
use od_isa::state::{Gdtr, Idtr};
use od_isa::{
    CodeSegmentDescriptorCache, Cpl0ExceptionDelivery as D, Cpl0ExceptionPhase,
    Cpl0ExceptionRefusal as R, CpuActivity, CpuException, ExceptionClass, LongModeState,
    NestedException, Register, StackSegmentDescriptorCache, StiShadow, TaskRegisterCache,
    deliver_exception_cpl0_set,
};
use std::collections::BTreeMap;

// ---- memory ------------------------------------------------------------------------------------

#[derive(Clone)]
struct Page {
    bytes: Box<[u8; 4096]>,
    writable: bool,
    user: bool,
}

/// Linear = physical pages; a missing page or a protection violation is the architectural `#PF`
/// with the error code of Vol. 3A "Event 14" (P, W/R, U/S of the page privilege of the access).
#[derive(Clone, Default)]
struct Memory {
    pages: BTreeMap<u64, Page>,
    writes: Vec<(u64, usize)>,
    /// A backend (non-architectural) error for any access to this page.
    backend_error_page: Option<u64>,
}

impl Memory {
    fn map(&mut self, page: u64, writable: bool, user: bool) {
        self.pages.insert(
            page & !0xfff,
            Page {
                bytes: Box::new([0; 4096]),
                writable,
                user,
            },
        );
    }

    fn put(&mut self, address: u64, bytes: &[u8]) {
        for (index, byte) in bytes.iter().enumerate() {
            let address = address + index as u64;
            self.pages
                .get_mut(&(address & !0xfff))
                .expect("mapped")
                .bytes[(address & 0xfff) as usize] = *byte;
        }
    }

    fn put_qword(&mut self, address: u64, value: u64) {
        self.put(address, &value.to_le_bytes());
    }

    fn qword(&self, address: u64) -> u64 {
        let page = &self.pages[&(address & !0xfff)];
        let offset = (address & 0xfff) as usize;
        u64::from_le_bytes(page.bytes[offset..offset + 8].try_into().unwrap())
    }

    /// The `#PF` error code of an access, or `None` when it succeeds (the model's view).
    fn page_fault_code(&self, address: u64, write: bool, user: bool) -> Option<u32> {
        let w = u32::from(write) << 1;
        let u = u32::from(user) << 2;
        match self.pages.get(&(address & !0xfff)) {
            None => Some(w | u),
            Some(page) if (user && !page.user) || (write && !page.writable) => Some(1 | w | u),
            Some(_) => None,
        }
    }

    fn check(
        &self,
        address: u64,
        context: AccessContext,
        write: bool,
        len: usize,
    ) -> Result<(), MemoryError> {
        assert_eq!(
            address & !0xfff,
            (address + len as u64 - 1) & !0xfff,
            "no page crossing"
        );
        if self.backend_error_page == Some(address & !0xfff) {
            return Err(MemoryError::Backend("injected".into()));
        }
        let user = context.page_privilege() == PrivilegeLevel::Ring3;
        match self.page_fault_code(address, write, user) {
            Some(code) => Err(MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(code),
                linear_address: Some(address),
            })),
            None => Ok(()),
        }
    }
}

impl GuestMemory for Memory {
    fn fetch(&mut self, _address: u64, _context: AccessContext) -> Result<u8, MemoryError> {
        Err(MemoryError::Backend("no fetch in these tests".into()))
    }

    fn read(
        &mut self,
        address: u64,
        context: AccessContext,
        destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        self.check(address, context, false, destination.len())?;
        let page = &self.pages[&(address & !0xfff)];
        let offset = (address & 0xfff) as usize;
        destination.copy_from_slice(&page.bytes[offset..offset + destination.len()]);
        Ok(())
    }

    fn write(
        &mut self,
        address: u64,
        context: AccessContext,
        source: &[u8],
    ) -> Result<(), MemoryError> {
        self.check(address, context, true, source.len())?;
        self.put(address, source);
        self.writes.push((address, source.len()));
        Ok(())
    }

    fn mfence(&mut self, _context: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

// ---- the fixture ---------------------------------------------------------------------------------

const GDT: u64 = 0x10_0000;
const IDT: u64 = 0x11_0000;
const TSS: u64 = 0x12_0000;
const KERNEL_STACK: u64 = 0x20_4000;
const IST_STACKS: u64 = 0x30_0000;
const USER_STACK: u64 = 0x40_0800;
const RIP: u64 = 0x50_0123;
const USER_RIP: u64 = 0x0000_7FF6_1234_5678;
const HANDLERS: u64 = 0xFFFF_F805_B000_0000;
const TR_SELECTOR: u16 = 0x40;

const CODE64: u64 = 0x00AF_9B00_0000_FFFF;
const DATA: u64 = 0x00CF_9300_0000_FFFF;
const USER_CODE64: u64 = 0x00AF_FB00_0000_FFFF;
const USER_DATA: u64 = 0x00CF_F300_0000_FFFF;

const CF: u64 = 1;
const ZF: u64 = 1 << 6;
const TF: u64 = 1 << 8;
const IF: u64 = 1 << 9;
const NT: u64 = 1 << 14;
const RF: u64 = 1 << 16;
const AC: u64 = 1 << 18;

fn context(privilege: PrivilegeLevel) -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege,
        origin: AccessOrigin::Explicit,
    }
}

fn handler(vector: u8) -> u64 {
    HANDLERS + u64::from(vector) * 0x100
}

fn gate(offset: u64, selector: u16, ist: u8, attributes: u8) -> [u8; 16] {
    let mut gate = [0u8; 16];
    gate[0..2].copy_from_slice(&(offset as u16).to_le_bytes());
    gate[2..4].copy_from_slice(&selector.to_le_bytes());
    gate[4] = ist;
    gate[5] = attributes;
    gate[6..8].copy_from_slice(&((offset >> 16) as u16).to_le_bytes());
    gate[8..12].copy_from_slice(&((offset >> 32) as u32).to_le_bytes());
    gate
}

struct Fixture {
    state: LongModeState,
    memory: Memory,
}

impl Fixture {
    /// GDT (0x08 ring 0 code, 0x10 data, 0x1b user code, 0x23 user data), an IDT with an interrupt
    /// gate to `handler(v)` for every vector 0..=31 (selector 0x08, IST 0), a 64-bit TSS (RSP0 and
    /// IST1..IST7 set to their stacks), and a CPL0 state at `RIP`.
    fn new(cpl3: bool) -> Self {
        let mut memory = Memory::default();
        memory.map(GDT, true, false);
        memory.map(IDT, true, false);
        memory.map(TSS, true, false);
        for page in 0..4 {
            memory.map(KERNEL_STACK - 0x1000 * (page + 1), true, false);
        }
        for ist in 1..=7u64 {
            memory.map(IST_STACKS + ist * 0x1000 - 0x1000, true, false);
        }
        memory.map(USER_STACK, true, true);
        for (index, descriptor) in [0, CODE64, DATA, USER_CODE64, USER_DATA]
            .into_iter()
            .enumerate()
        {
            memory.put_qword(GDT + 8 * index as u64, descriptor);
        }
        for vector in 0..32u8 {
            memory.put(
                IDT + 16 * u64::from(vector),
                &gate(handler(vector), 0x08, 0, 0x8e),
            );
        }
        memory.put_qword(TSS + 4, KERNEL_STACK);
        for ist in 1..=7u64 {
            memory.put_qword(
                TSS + 0x24 + 8 * (ist - 1),
                IST_STACKS + ist * 0x1000 - 0x18 * ist,
            );
        }
        let (cs, cs_access, ss, ss_access, rip, rsp) = if cpl3 {
            (0x1b, 0xfb, 0x23, 0xf3, USER_RIP, USER_STACK + 0x7f3)
        } else {
            (0x08, 0x9b, 0x10, 0x93, RIP, KERNEL_STACK - 0x2000 + 0x123)
        };
        let mut state = LongModeState {
            rip,
            rflags: 2 | IF | CF,
            cs_selector: Some(cs),
            cs_cache: Some(CodeSegmentDescriptorCache {
                base: 0,
                limit: u32::MAX,
                access: cs_access,
                flags: 0xa,
            }),
            ss_selector: Some(ss),
            ss_cache: Some(StackSegmentDescriptorCache {
                base: 0,
                limit: u32::MAX,
                access: ss_access,
                flags: 0xc,
            }),
            idtr: Some(Idtr {
                base: IDT,
                limit: 0xfff,
            }),
            gdtr: Some(Gdtr {
                base: GDT,
                limit: 0x2f,
            }),
            tr: Some(TaskRegisterCache {
                selector: TR_SELECTOR,
                base: TSS,
                limit: 0x67,
                access: 0x8b,
                flags: 0,
            }),
            sti_shadow: Some(StiShadow::Clear),
            activity: Some(CpuActivity::Running),
            ..LongModeState::default()
        };
        state.gpr[Register::Rsp as usize] = rsp;
        for (index, value) in state.gpr.iter_mut().enumerate() {
            if index != Register::Rsp as usize {
                *value = 0x1111_0000 + index as u64;
            }
        }
        Self { state, memory }
    }

    fn privilege(&self) -> PrivilegeLevel {
        if self.state.cs_selector.unwrap() & 3 == 3 {
            PrivilegeLevel::Ring3
        } else {
            PrivilegeLevel::Ring0
        }
    }

    fn set_gate(&mut self, vector: u8, bytes: [u8; 16]) {
        self.memory.put(IDT + 16 * u64::from(vector), &bytes);
    }

    fn deliver(&mut self, exception: CpuException) -> D {
        let context = context(self.privilege());
        deliver_exception_cpl0_set(&mut self.state, &mut self.memory, exception, context)
    }
}

// ---- the model ---------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Fault,
    Trap,
    Abort,
}

/// An event of the model. Its saved RIP is the boundary RIP of the state (`before.rip`): for a
/// fault the faulting instruction, for a trap the next one, for an abort the boundary (the CPU
/// saves the RIP of the boundary it delivers at).
#[derive(Clone, Copy, Debug)]
struct Event {
    vector: u8,
    error_code: Option<u32>,
    cr2: Option<u64>,
    class: Class,
}

/// Table 7-4 (benign: everything not named below).
fn contributory(vector: u8) -> bool {
    [0, 10, 11, 12, 13, 21].contains(&vector)
}

fn page_fault_class(vector: u8) -> bool {
    vector == 14 || vector == 20
}

#[derive(Debug, PartialEq, Eq)]
enum Expected {
    Delivered {
        vector: u8,
        target_rip: u64,
        new_rsp: u64,
        trap_gate: bool,
        /// The stores of the successful attempt in order: the frame words, then the Accessed-bit
        /// write of the descriptor (address, length) when the bit was clear.
        stores: Vec<(u64, usize)>,
        frame: Vec<(u64, u64)>,
        state: LongModeState,
        nested: Vec<NestedException>,
    },
    Shutdown {
        state: LongModeState,
        nested: Vec<NestedException>,
    },
}

struct Entered {
    target: u64,
    new_rsp: u64,
    trap_gate: bool,
    stores: Vec<(u64, usize)>,
    frame: Vec<(u64, u64)>,
    after: LongModeState,
}

/// One attempt of the model: `Ok` with the frame and the state of the handler, or the second
/// exception (vector, error code, linear address of a #PF). The checks follow the IA-32e branch of
/// the "INT n" pseudocode: gate limit and type, present; `TRAP-OR-INTERRUPT-GATE` (null selector,
/// descriptor limit, code and DPL <= CPL, present, 64-bit by §7.14.1); the new stack and
/// "IF NewRSP contains a non-canonical address THEN #SS(EXT)" (the TSS value or, at CPL0 without
/// IST, the current RSP); then the canonical handler RIP; then the pushes and the Accessed bit.
fn model_attempt(
    before: &LongModeState,
    memory: &Memory,
    event: &Event,
) -> Result<Entered, (u8, u32, Option<u64>)> {
    let ext = 1u32;
    let cpl = before.cs_selector.unwrap() & 3;
    let idt = before.idtr.unwrap();
    let gdt = before.gdtr.unwrap();
    let idt_code = u32::from(event.vector) * 8 + 2 + ext;
    // IA-32e-MODE: limit and type, then present.
    let offset = u64::from(event.vector) * 16;
    if offset + 15 > u64::from(idt.limit) {
        return Err((13, idt_code, None));
    }
    for half in [0, 8] {
        if let Some(code) = memory.page_fault_code(idt.base + offset + half, false, false) {
            return Err((14, code, Some(idt.base + offset + half)));
        }
    }
    let low = memory.qword(idt.base + offset);
    let high = memory.qword(idt.base + offset + 8);
    let attributes = (low >> 40) as u8;
    if attributes & 0x1f != 0x0e && attributes & 0x1f != 0x0f {
        return Err((13, idt_code, None));
    }
    if attributes & 0x80 == 0 {
        return Err((11, idt_code, None));
    }
    let trap_gate = attributes & 1 == 1;
    let ist = (low >> 32) as u8 & 7;
    let selector = (low >> 16) as u16;
    let target = (low & 0xffff) | ((low >> 32) & 0xffff_0000) | (high << 32);
    // TRAP-OR-INTERRUPT-GATE.
    if selector & 0xfffc == 0 {
        return Err((13, ext, None));
    }
    let descriptor_address = gdt.base + u64::from(selector & !7);
    if u64::from(selector & !7) + 7 > u64::from(gdt.limit) {
        return Err((13, u32::from(selector & 0xfffc) | ext, None));
    }
    if let Some(code) = memory.page_fault_code(descriptor_address, false, false) {
        return Err((14, code, Some(descriptor_address)));
    }
    let descriptor = memory.qword(descriptor_address);
    let access = (descriptor >> 40) as u8;
    let dpl = (access >> 5) & 3;
    if access & 0x18 != 0x18 || u16::from(dpl) > cpl {
        return Err((13, u32::from(selector & 0xfffc) | ext, None));
    }
    if access & 0x80 == 0 {
        return Err((11, u32::from(selector & 0xfffc) | ext, None));
    }
    let long = descriptor >> 53 & 1 == 1;
    let default_big = descriptor >> 54 & 1 == 1;
    if !long || default_big {
        return Err((13, idt_code, None));
    }
    // The new stack: IST, else RSP0 from CPL3, else the current RSP.
    let tss_field = if ist != 0 {
        Some(0x24 + 8 * (u64::from(ist) - 1))
    } else if cpl == 3 {
        Some(4)
    } else {
        None
    };
    let old_rsp = before.gpr[Register::Rsp as usize];
    let base = match tss_field {
        Some(field) => {
            let tr = before.tr.unwrap();
            if u64::from(tr.limit) < field + 7 {
                return Err((10, u32::from(tr.selector & 0xfffc) | ext, None));
            }
            if let Some(code) = memory.page_fault_code(tr.base + field, false, false) {
                return Err((14, code, Some(tr.base + field)));
            }
            memory.qword(tr.base + field)
        }
        None => old_rsp,
    };
    if !canonical(base) {
        return Err((12, ext, None));
    }
    if !canonical(target) {
        return Err((13, ext, None));
    }
    let aligned = base & !15;
    let words: u64 = if event.error_code.is_some() { 6 } else { 5 };
    let new_rsp = aligned.wrapping_sub(8 * words);
    if !canonical(new_rsp) || !canonical(aligned.wrapping_sub(1)) {
        return Err((12, ext, None));
    }
    let image = match event.class {
        Class::Fault => before.rflags | RF | 2,
        Class::Trap | Class::Abort => before.rflags | 2,
    };
    let values = [
        u64::from(before.ss_selector.unwrap()),
        old_rsp,
        image,
        u64::from(before.cs_selector.unwrap()),
        before.rip,
        u64::from(event.error_code.unwrap_or(0)),
    ];
    let mut frame = Vec::new();
    let mut stores = Vec::new();
    for (index, value) in values.iter().take(words as usize).enumerate() {
        let address = aligned - 8 * (index as u64 + 1);
        // The stores of the handler privilege (CPL0): supervisor accesses.
        if let Some(code) = memory.page_fault_code(address, true, false) {
            return Err((14, code, Some(address)));
        }
        frame.push((address, *value));
        stores.push((address, 8));
    }
    if access & 1 == 0 {
        if let Some(code) = memory.page_fault_code(descriptor_address, true, false) {
            return Err((14, code, Some(descriptor_address)));
        }
        stores.push((descriptor_address, 8));
    }
    let mut after = before.clone();
    after.cs_selector = Some(selector);
    after.cs_cache = Some(CodeSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: access | 1,
        flags: (descriptor >> 52) as u8 & 0xf,
    });
    after.rip = target;
    after.gpr[Register::Rsp as usize] = new_rsp;
    if cpl == 3 {
        after.ss_selector = Some(0);
        after.ss_unusable = true;
    }
    let mut cleared = TF | NT | RF | (1 << 17);
    if !trap_gate {
        cleared |= IF;
    }
    after.rflags = (before.rflags & !cleared) | 2;
    after.sti_shadow = Some(StiShadow::Clear);
    Ok(Entered {
        target,
        new_rsp,
        trap_gate,
        stores,
        frame,
        after,
    })
}

fn canonical(address: u64) -> bool {
    let top = address >> 47;
    top == 0 || top == 0x1_ffff
}

/// The whole delivery of the model (Table 7-5 over [`model_attempt`]).
fn model(before: &LongModeState, memory: &Memory, first: Event) -> Expected {
    let mut current = first;
    let mut cr2 = first.cr2;
    let mut nested = Vec::new();
    loop {
        match model_attempt(before, memory, &current) {
            Ok(mut entered) => {
                if cr2.is_some() {
                    entered.after.cr2 = cr2;
                }
                return Expected::Delivered {
                    vector: current.vector,
                    target_rip: entered.target,
                    new_rsp: entered.new_rsp,
                    trap_gate: entered.trap_gate,
                    stores: entered.stores,
                    frame: entered.frame,
                    state: entered.after,
                    nested,
                };
            }
            Err((vector, error_code, linear_address)) => {
                if linear_address.is_some() && vector == 14 {
                    cr2 = linear_address;
                }
                nested.push(NestedException {
                    vector,
                    error_code: Some(error_code),
                    linear_address,
                });
                let second = Event {
                    vector,
                    error_code: Some(error_code),
                    cr2: linear_address,
                    class: Class::Fault,
                };
                let first_double = current.vector == 8;
                let first_contributory = contributory(current.vector);
                let first_page = page_fault_class(current.vector);
                let second_contributory = contributory(vector);
                let second_page = page_fault_class(vector);
                if first_double && (second_contributory || second_page) {
                    let mut state = before.clone();
                    state.activity = Some(CpuActivity::Shutdown);
                    if cr2.is_some() {
                        state.cr2 = cr2;
                    }
                    return Expected::Shutdown { state, nested };
                }
                if (first_contributory && second_contributory)
                    || (first_page && (second_contributory || second_page))
                {
                    nested.push(NestedException {
                        vector: 8,
                        error_code: Some(0),
                        linear_address: None,
                    });
                    current = Event {
                        vector: 8,
                        error_code: Some(0),
                        cr2: None,
                        class: Class::Abort,
                    };
                } else {
                    current = second;
                }
            }
        }
    }
}

/// Run the implementation on a copy of `fixture` and compare with the model: the result, the
/// whole state, every frame word, and the stores — exactly the frame and the Accessed-bit write
/// for a delivery without nested exceptions, and those as the last stores after a chain (the
/// earlier attempts may have left dead frame words).
fn check(fixture: &Fixture, exception: CpuException, event: Event, label: &str) -> D {
    let expected = model(&fixture.state, &fixture.memory, event);
    let mut run = Fixture {
        state: fixture.state.clone(),
        memory: fixture.memory.clone(),
    };
    let result = run.deliver(exception);
    match (&expected, &result) {
        (
            Expected::Delivered {
                vector,
                target_rip,
                new_rsp,
                trap_gate,
                stores,
                frame,
                state,
                nested,
            },
            D::Delivered {
                vector: v,
                target_rip: t,
                new_rsp: n,
                trap_gate: g,
                nested: c,
            },
        ) => {
            assert_eq!(
                (vector, target_rip, new_rsp, trap_gate, nested),
                (v, t, n, g, c),
                "{label}"
            );
            assert_eq!(&run.state, state, "{label}: state");
            for (address, value) in frame {
                assert_eq!(
                    run.memory.qword(*address),
                    *value,
                    "{label}: frame at {address:#x}"
                );
            }
            let writes = &run.memory.writes;
            if nested.is_empty() {
                assert_eq!(writes, stores, "{label}: the stores");
            } else {
                assert!(
                    writes.len() >= stores.len()
                        && writes[writes.len() - stores.len()..] == stores[..],
                    "{label}: the last stores {writes:x?}, expected {stores:x?}"
                );
            }
        }
        (Expected::Shutdown { state, nested }, D::Shutdown { nested: c }) => {
            assert_eq!(nested, c, "{label}");
            assert_eq!(&run.state, state, "{label}: state");
        }
        _ => panic!("{label}: model {expected:?}, implementation {result:?}"),
    }
    result
}

fn fault_event(vector: u8, error_code: Option<u32>, cr2: Option<u64>) -> (CpuException, Event) {
    let exception = CpuException::fault(&ArchitecturalFault {
        vector,
        error_code,
        linear_address: cr2,
    })
    .expect("a fault-class exception");
    (
        exception,
        Event {
            vector,
            error_code,
            cr2,
            class: Class::Fault,
        },
    )
}

/// The fault-class vectors of Table 7-1 and their error code (`None`: the vector has none).
fn fault_vectors() -> Vec<(u8, Option<u32>)> {
    vec![
        (0, None),
        (5, None),
        (6, None),
        (7, None),
        (10, Some(0x40)),
        (11, Some(0x2b)),
        (12, Some(0)),
        (13, Some(0x1a)),
        (14, Some(0x13)),
        (16, None),
        (17, Some(0)),
        (19, None),
        (20, None),
        (21, Some(1)),
    ]
}

/// Every event the API delivers for vectors 0..=21 (except NMI and the reserved ones): the faults,
/// the `#DB` trap and the `#MC` abort (the state of the fixture stands for the boundary of each:
/// before the faulting instruction, after the trapping one).
fn all_events() -> Vec<(CpuException, Event)> {
    let mut events: Vec<_> = fault_vectors()
        .into_iter()
        .map(|(vector, code)| fault_event(vector, code, (vector == 14).then_some(0xdead_b000)))
        .collect();
    events.push((
        CpuException::debug_trap(),
        Event {
            vector: 1,
            error_code: None,
            cr2: None,
            class: Class::Trap,
        },
    ));
    events.push((
        CpuException::abort(18).unwrap(),
        Event {
            vector: 18,
            error_code: None,
            cr2: None,
            class: Class::Abort,
        },
    ));
    events
}

// ---- tests ---------------------------------------------------------------------------------------

#[test]
fn every_vector_from_cpl0_and_cpl3_matches_the_model() {
    let mut count = 0;
    for cpl3 in [false, true] {
        for attributes in [0x8e, 0x8f, 0xee] {
            for ist in [0u8, 1, 7] {
                for rsp_low in [0u64, 8, 0xf] {
                    for rflags in [2 | IF | CF, 2 | TF | NT | RF | AC | IF | ZF, 2] {
                        for accessed in [true, false] {
                            let mut fixture = Fixture::new(cpl3);
                            fixture.state.rflags = rflags;
                            let rsp = &mut fixture.state.gpr[Register::Rsp as usize];
                            *rsp = (*rsp & !0xf) | rsp_low;
                            if !accessed {
                                fixture.memory.put_qword(GDT + 8, CODE64 & !(1 << 40));
                            }
                            for (exception, event) in all_events() {
                                fixture.set_gate(
                                    event.vector,
                                    gate(handler(event.vector), 0x08, ist, attributes),
                                );
                                let label = format!(
                                    "vector {} cpl3 {cpl3} gate {attributes:#x} ist {ist} rsp {rsp_low:#x} rflags {rflags:#x} A {accessed}",
                                    event.vector
                                );
                                let result = check(&fixture, exception, event, &label);
                                assert!(
                                    matches!(result, D::Delivered { ref nested, .. } if nested.is_empty()),
                                    "{label}: {result:?}"
                                );
                                count += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    assert_eq!(count, 2 * 3 * 3 * 3 * 3 * 2 * 16);
}

#[test]
fn hand_checked_frames_of_de_pf_and_db() {
    // #DE at CPL0: five qwords, RF = 1 in the image, IF cleared by the interrupt gate, AC kept.
    let mut fixture = Fixture::new(false);
    fixture.state.rflags = 2 | IF | AC | TF;
    let rsp = fixture.state.gpr[Register::Rsp as usize];
    let (exception, _) = fault_event(0, None, None);
    let result = fixture.deliver(exception);
    let aligned = rsp & !15;
    assert_eq!(
        result,
        D::Delivered {
            vector: 0,
            target_rip: handler(0),
            new_rsp: aligned - 40,
            trap_gate: false,
            nested: Vec::new(),
        }
    );
    let frame: Vec<u64> = (1..=5)
        .map(|i| fixture.memory.qword(aligned - 8 * i))
        .collect();
    assert_eq!(frame, [0x10, rsp, 2 | IF | AC | TF | RF, 0x08, RIP]);
    assert_eq!(fixture.state.rflags, 2 | AC);
    assert_eq!(fixture.state.cr2, None);
    // #PF from CPL3 through RSP0: six qwords on the kernel stack, SS null, CR2 set.
    let mut fixture = Fixture::new(true);
    let user_rsp = fixture.state.gpr[Register::Rsp as usize];
    let (exception, _) = fault_event(14, Some(7), Some(0x1234));
    let result = fixture.deliver(exception);
    assert!(
        matches!(result, D::Delivered { vector: 14, new_rsp, .. } if new_rsp == KERNEL_STACK - 48)
    );
    let frame: Vec<u64> = (1..=6)
        .map(|i| fixture.memory.qword(KERNEL_STACK - 8 * i))
        .collect();
    assert_eq!(frame, [0x23, user_rsp, 2 | IF | CF | RF, 0x1b, USER_RIP, 7]);
    assert_eq!(
        (fixture.state.ss_selector, fixture.state.ss_unusable),
        (Some(0), true)
    );
    assert_eq!(fixture.state.cs_selector, Some(0x08));
    assert_eq!(fixture.state.cr2, Some(0x1234));
    // #DB trap: the state is the one after the trapping instruction, so the saved RIP is the next
    // instruction; RF as it is (0 here), no error code.
    let mut fixture = Fixture::new(false);
    fixture.state.rip = RIP + 2;
    let result = fixture.deliver(CpuException::debug_trap());
    assert!(matches!(result, D::Delivered { vector: 1, .. }));
    let aligned = (KERNEL_STACK - 0x2000 + 0x123) & !15;
    assert_eq!(fixture.memory.qword(aligned - 40), RIP + 2);
    assert_eq!(fixture.memory.qword(aligned - 24) & RF, 0);
}

#[test]
fn the_constructors_follow_table_7_1() {
    let fault = |vector, error_code, linear_address| {
        CpuException::fault(&ArchitecturalFault {
            vector,
            error_code,
            linear_address,
        })
    };
    for vector in [2u8, 9, 15, 22, 31, 32, 0x80, 0xff] {
        assert_eq!(fault(vector, None, None), Err(R::NotAnException(vector)));
        assert_eq!(CpuException::abort(vector), Err(R::NotAnException(vector)));
    }
    for vector in [1u8, 3, 4, 8, 18] {
        let code = (vector == 8).then_some(0);
        assert_eq!(
            fault(vector, code, None),
            Err(R::ClassMismatch(vector)),
            "{vector}"
        );
    }
    for vector in [0u8, 8, 13, 14] {
        assert_eq!(CpuException::abort(vector), Err(R::ClassMismatch(vector)));
    }
    for (vector, code) in fault_vectors() {
        let wrong = if code.is_some() { None } else { Some(0) };
        assert_eq!(
            fault(vector, wrong, None),
            Err(R::ErrorCodeMismatch(vector))
        );
        let exception = fault(vector, code, (vector == 14).then_some(1)).unwrap();
        assert_eq!(
            (
                exception.vector(),
                exception.error_code(),
                exception.class()
            ),
            (vector, code, ExceptionClass::Fault)
        );
    }
    assert_eq!(fault(14, Some(2), None), Err(R::MissingFaultMetadata));
    assert_eq!(CpuException::debug_trap().class(), ExceptionClass::Trap);
    assert_eq!(
        CpuException::abort(18).unwrap().class(),
        ExceptionClass::Abort
    );
}

/// A way to break the delivery of one vector (its gate, its descriptor, its stack), each a
/// different check of the pseudocode.
#[derive(Clone, Copy, Debug)]
enum Breakage {
    ZeroGate,
    TaskGate,
    NotPresentGate,
    BeyondIdtLimit,
    NullSelector,
    SelectorBeyondGdt,
    DataDescriptor,
    UserCodeDescriptor,
    NotPresentDescriptor,
    Code32Descriptor,
    NonCanonicalHandler,
    IstBeyondTss,
    IstNonCanonical,
    IstStackUnmapped,
    IstStackReadOnly,
    IstReadFaults,
    StackNonCanonical,
    GdtReadFaults,
    AccessedWriteFaults,
    /// S = 1 with the type of an interrupt gate and P = 0: not a gate type (#GP), not #NP.
    SystemGateNotPresent,
    /// A DPL 3 code descriptor with P = 0: DPL > CPL comes before P (#GP(sel) at CPL0, #NP(sel) at
    /// CPL3).
    UserCodeNotPresent,
    /// The current RSP is not canonical (the stack of a CPL0 delivery without IST).
    CurrentRspNonCanonical,
}

const BREAKAGES: [Breakage; 22] = [
    Breakage::ZeroGate,
    Breakage::TaskGate,
    Breakage::NotPresentGate,
    Breakage::BeyondIdtLimit,
    Breakage::NullSelector,
    Breakage::SelectorBeyondGdt,
    Breakage::DataDescriptor,
    Breakage::UserCodeDescriptor,
    Breakage::NotPresentDescriptor,
    Breakage::Code32Descriptor,
    Breakage::NonCanonicalHandler,
    Breakage::IstBeyondTss,
    Breakage::IstNonCanonical,
    Breakage::IstStackUnmapped,
    Breakage::IstStackReadOnly,
    Breakage::IstReadFaults,
    Breakage::StackNonCanonical,
    Breakage::GdtReadFaults,
    Breakage::AccessedWriteFaults,
    Breakage::SystemGateNotPresent,
    Breakage::UserCodeNotPresent,
    Breakage::CurrentRspNonCanonical,
];

/// Break the delivery of `vector` (other vectors keep their good gates where the breakage allows).
fn break_vector(fixture: &mut Fixture, vector: u8, breakage: Breakage) {
    let good = |selector, ist| gate(handler(vector), selector, ist, 0x8e);
    match breakage {
        Breakage::ZeroGate => fixture.set_gate(vector, [0; 16]),
        Breakage::TaskGate => fixture.set_gate(vector, gate(0, 0x08, 0, 0x85)),
        Breakage::NotPresentGate => fixture.set_gate(vector, gate(handler(vector), 0x08, 0, 0x0e)),
        Breakage::BeyondIdtLimit => {
            // The limit ends just before the gate: every higher vector is beyond it too.
            fixture.state.idtr.as_mut().unwrap().limit = (u16::from(vector) * 16).max(1) - 1;
        }
        Breakage::SystemGateNotPresent => {
            fixture.set_gate(vector, gate(handler(vector), 0x08, 0, 0x1e))
        }
        Breakage::UserCodeNotPresent => {
            fixture
                .memory
                .put_qword(GDT + 0x48, USER_CODE64 & !(1 << 47));
            fixture.state.gdtr.as_mut().unwrap().limit = 0x4f;
            fixture.set_gate(vector, good(0x48, 0));
        }
        Breakage::CurrentRspNonCanonical => {
            fixture.state.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0100;
        }
        Breakage::NullSelector => fixture.set_gate(vector, good(0, 0)),
        Breakage::SelectorBeyondGdt => fixture.set_gate(vector, good(0x80, 0)),
        Breakage::DataDescriptor => fixture.set_gate(vector, good(0x10, 0)),
        Breakage::UserCodeDescriptor => {
            // DPL 3 > CPL 0 (from CPL3 the model runs no CPL3 handler: not used there).
            fixture.memory.put_qword(GDT + 0x28, USER_CODE64);
            fixture.state.gdtr.as_mut().unwrap().limit = 0x3f;
            fixture.set_gate(vector, good(0x28, 0));
        }
        Breakage::NotPresentDescriptor => {
            fixture.memory.put_qword(GDT + 0x30, CODE64 & !(1 << 47));
            fixture.state.gdtr.as_mut().unwrap().limit = 0x3f;
            fixture.set_gate(vector, good(0x30, 0));
        }
        Breakage::Code32Descriptor => {
            fixture.memory.put_qword(GDT + 0x38, 0x00CF_9B00_0000_FFFF);
            fixture.state.gdtr.as_mut().unwrap().limit = 0x3f;
            fixture.set_gate(vector, good(0x38, 0));
        }
        Breakage::NonCanonicalHandler => {
            fixture.set_gate(vector, gate(0x0000_8000_0000_1000, 0x08, 0, 0x8e))
        }
        Breakage::IstBeyondTss => {
            // IST6 lies at 0x4c..=0x53: beyond a limit of 0x4b.
            fixture.state.tr.as_mut().unwrap().limit = 0x4b;
            fixture.set_gate(vector, good(0x08, 6));
        }
        Breakage::IstNonCanonical => {
            fixture
                .memory
                .put_qword(TSS + 0x24 + 8 * 2, 0x0000_9000_0000_0000);
            fixture.set_gate(vector, good(0x08, 3));
        }
        Breakage::IstStackUnmapped => {
            fixture.memory.put_qword(TSS + 0x24 + 8 * 3, 0x60_0000);
            fixture.set_gate(vector, good(0x08, 4));
        }
        Breakage::IstStackReadOnly => {
            fixture.memory.map(0x61_0000, false, false);
            fixture.memory.put_qword(TSS + 0x24 + 8 * 4, 0x61_0800);
            fixture.set_gate(vector, good(0x08, 5));
        }
        Breakage::IstReadFaults => {
            // A TSS whose IST entries lie on an unmapped page (RSP0 stays readable).
            let tss = 0x70_0fe0;
            fixture.memory.map(tss, true, false);
            fixture.memory.put_qword(tss + 4, KERNEL_STACK);
            let tr = fixture.state.tr.as_mut().unwrap();
            tr.base = tss;
            fixture.set_gate(vector, good(0x08, 1));
        }
        Breakage::StackNonCanonical => {
            fixture
                .memory
                .put_qword(TSS + 0x24 + 8 * 6, 0xFFFF_8000_0000_0010);
            fixture.set_gate(vector, good(0x08, 7));
        }
        Breakage::GdtReadFaults => {
            // Selector 0x1008 lies in the second page of the GDT, which is not mapped.
            fixture.state.gdtr.as_mut().unwrap().limit = 0x1fff;
            fixture.set_gate(vector, good(0x1008, 0));
        }
        Breakage::AccessedWriteFaults => {
            // A read-only GDT page with a code descriptor whose Accessed bit is clear.
            let gdt = 0x80_0000;
            fixture.memory.map(gdt, true, false);
            for (index, descriptor) in
                [0, CODE64, DATA, USER_CODE64, USER_DATA, CODE64 & !(1 << 40)]
                    .into_iter()
                    .enumerate()
            {
                fixture.memory.put_qword(gdt + 8 * index as u64, descriptor);
            }
            fixture.memory.pages.get_mut(&gdt).unwrap().writable = false;
            fixture.state.gdtr = Some(Gdtr {
                base: gdt,
                limit: 0x2f,
            });
            fixture.set_gate(vector, good(0x28, 0));
        }
    }
}

#[test]
fn a_broken_delivery_raises_the_sdm_exception_and_table_7_5_decides() {
    // Every event x every breakage of its own delivery, from CPL0 and from CPL3, with the #DF gate
    // good (IST 1, as Windows installs it), broken in the same way, or zero.
    let mut outcomes = BTreeMap::<String, usize>::new();
    for cpl3 in [false, true] {
        for double_fault_gate in ["ist1", "same", "zero"] {
            for breakage in BREAKAGES {
                if cpl3 && matches!(breakage, Breakage::UserCodeDescriptor) {
                    continue;
                }
                for (exception, event) in all_events() {
                    let mut fixture = Fixture::new(cpl3);
                    match double_fault_gate {
                        "ist1" => fixture.set_gate(8, gate(handler(8), 0x08, 1, 0x8e)),
                        "zero" => fixture.set_gate(8, [0; 16]),
                        _ => break_vector(&mut fixture, 8, breakage),
                    }
                    break_vector(&mut fixture, event.vector, breakage);
                    let label = format!(
                        "vector {} {breakage:?} cpl3 {cpl3} df {double_fault_gate}",
                        event.vector
                    );
                    let result = check(&fixture, exception, event, &label);
                    let kind = match &result {
                        D::Delivered { vector, nested, .. } => {
                            format!("delivered {vector} after {}", nested.len())
                        }
                        D::Shutdown { .. } => "shutdown".to_owned(),
                        other => panic!("{label}: {other:?}"),
                    };
                    *outcomes.entry(kind).or_default() += 1;
                }
            }
        }
    }
    // The matrix reaches serial deliveries, double faults and shutdown.
    assert!(
        outcomes.keys().any(|kind| kind.starts_with("delivered 8")),
        "{outcomes:?}"
    );
    assert!(
        outcomes.keys().any(|kind| kind.starts_with("delivered 14")),
        "{outcomes:?}"
    );
    assert!(
        outcomes.keys().any(|kind| kind.starts_with("delivered 13")),
        "{outcomes:?}"
    );
    assert!(
        outcomes.keys().any(|kind| kind.starts_with("delivered 11")),
        "{outcomes:?}"
    );
    assert!(
        outcomes.keys().any(|kind| kind.starts_with("delivered 10")),
        "{outcomes:?}"
    );
    assert!(
        outcomes.keys().any(|kind| kind.starts_with("delivered 12")),
        "{outcomes:?}"
    );
    assert!(outcomes.contains_key("shutdown"), "{outcomes:?}");
}

#[test]
fn two_broken_checks_raise_the_exception_of_the_earlier_one() {
    // The order of the IA-32e branch of the "INT n" pseudocode, by pairs of failed checks of the
    // #UD gate at CPL0 (the #DF gate has IST 1): the first nested exception is the one of the check
    // that comes first, and the whole delivery matches the model.
    const NON_CANONICAL: u64 = 0x0000_8000_0000_1000;
    let cases: Vec<(&str, Box<dyn Fn(&mut Fixture)>, (u8, u32))> = vec![
        (
            "IST beyond the TSS before a non-canonical handler",
            Box::new(|f| {
                f.state.tr.as_mut().unwrap().limit = 0x4b;
                f.set_gate(6, gate(NON_CANONICAL, 0x08, 6, 0x8e));
            }),
            (10, u32::from(TR_SELECTOR) | 1),
        ),
        (
            "a non-canonical IST value before a non-canonical handler",
            Box::new(|f| {
                f.memory
                    .put_qword(TSS + 0x24 + 8 * 2, 0x0000_9000_0000_0000);
                f.set_gate(6, gate(NON_CANONICAL, 0x08, 3, 0x8e));
            }),
            (12, 1),
        ),
        (
            "a non-canonical current RSP before a non-canonical handler",
            Box::new(|f| {
                f.state.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0100;
                f.set_gate(6, gate(NON_CANONICAL, 0x08, 0, 0x8e));
            }),
            (12, 1),
        ),
        (
            "a non-canonical handler before the stores to an unmapped IST stack",
            Box::new(|f| {
                f.memory.put_qword(TSS + 0x24 + 8 * 3, 0x60_0000);
                f.set_gate(6, gate(NON_CANONICAL, 0x08, 4, 0x8e));
            }),
            (13, 1),
        ),
        (
            "DPL 3 before P = 0 of the code descriptor",
            Box::new(|f| {
                f.memory.put_qword(GDT + 0x48, USER_CODE64 & !(1 << 47));
                f.state.gdtr.as_mut().unwrap().limit = 0x4f;
                f.set_gate(6, gate(handler(6), 0x48, 0, 0x8e));
            }),
            (13, 0x49),
        ),
        (
            "the S bit before P = 0 of the gate",
            Box::new(|f| f.set_gate(6, gate(handler(6), 0x08, 0, 0x1e))),
            (13, 6 * 8 + 3),
        ),
        (
            "the task gate type before P = 0",
            Box::new(|f| f.set_gate(6, gate(handler(6), 0x08, 0, 0x05))),
            (13, 6 * 8 + 3),
        ),
        (
            "P = 0 of the gate before a null selector",
            Box::new(|f| f.set_gate(6, gate(handler(6), 0x00, 0, 0x0e))),
            (11, 6 * 8 + 3),
        ),
        (
            "a selector beyond the GDT before a non-canonical handler",
            Box::new(|f| f.set_gate(6, gate(NON_CANONICAL, 0x80, 0, 0x8e))),
            (13, 0x81),
        ),
        (
            "a 32-bit code descriptor before a non-canonical handler",
            Box::new(|f| {
                f.memory.put_qword(GDT + 0x38, 0x00CF_9B00_0000_FFFF);
                f.state.gdtr.as_mut().unwrap().limit = 0x3f;
                f.set_gate(6, gate(NON_CANONICAL, 0x38, 0, 0x8e));
            }),
            (13, 6 * 8 + 3),
        ),
    ];
    for (label, prepare, first_nested) in cases {
        let mut fixture = Fixture::new(false);
        fixture.set_gate(8, gate(handler(8), 0x08, 1, 0x8e));
        prepare(&mut fixture);
        let (exception, event) = fault_event(6, None, None);
        let result = check(&fixture, exception, event, label);
        let nested = match &result {
            D::Delivered { nested, .. } | D::Shutdown { nested } => nested,
            other => panic!("{label}: {other:?}"),
        };
        assert_eq!(
            (nested[0].vector, nested[0].error_code),
            (first_nested.0, Some(first_nested.1)),
            "{label}: {result:?}"
        );
    }
}

#[test]
fn the_classic_chains_by_hand() {
    // #DE through a not-present gate: #NP(0 * 8 + 3) is contributory after the contributory #DE,
    // a double fault through IST1.
    let mut fixture = Fixture::new(false);
    fixture.set_gate(0, gate(handler(0), 0x08, 0, 0x0e));
    fixture.set_gate(8, gate(handler(8), 0x08, 1, 0x8e));
    let (exception, _) = fault_event(0, None, None);
    let result = fixture.deliver(exception);
    assert_eq!(
        result,
        D::Delivered {
            vector: 8,
            target_rip: handler(8),
            new_rsp: ((IST_STACKS + 0x1000 - 0x18) & !15) - 48,
            trap_gate: false,
            nested: vec![
                NestedException {
                    vector: 11,
                    error_code: Some(3),
                    linear_address: None
                },
                NestedException {
                    vector: 8,
                    error_code: Some(0),
                    linear_address: None
                },
            ],
        }
    );
    let df_frame = ((IST_STACKS + 0x1000 - 0x18) & !15) - 48;
    assert_eq!(fixture.memory.qword(df_frame), 0, "#DF error code 0");
    assert_eq!(fixture.memory.qword(df_frame + 8), RIP, "the boundary RIP");
    assert_eq!(
        fixture.memory.qword(df_frame + 24) & RF,
        0,
        "#DF: RF as it was"
    );
    // #UD (benign) through a zero gate: #GP(6 * 8 + 3) serially (the type before P).
    let mut fixture = Fixture::new(false);
    fixture.set_gate(6, [0; 16]);
    let (exception, _) = fault_event(6, None, None);
    let result = fixture.deliver(exception);
    assert!(
        matches!(&result, D::Delivered { vector: 13, nested, .. }
            if nested == &[NestedException { vector: 13, error_code: Some(51), linear_address: None }]),
        "{result:?}"
    );
    // #GP whose frame store faults (#PF serially): the #PF handler gets CR2 and the store's error
    // code (supervisor write, not present).
    let mut fixture = Fixture::new(false);
    fixture.state.gpr[Register::Rsp as usize] = 0x60_0010;
    fixture.set_gate(14, gate(handler(14), 0x08, 2, 0x8e));
    let (exception, _) = fault_event(13, Some(0), None);
    let result = fixture.deliver(exception);
    assert!(
        matches!(result, D::Delivered { vector: 14, .. }),
        "{result:?}"
    );
    assert_eq!(fixture.state.cr2, Some(0x60_0008));
    let pf_frame = ((IST_STACKS + 0x2000 - 0x30) & !15) - 48;
    assert_eq!(fixture.memory.qword(pf_frame), 2);
    // #PF whose frame faults on the same stack, #DF through a zero gate: shutdown.
    let mut fixture = Fixture::new(false);
    fixture.state.gpr[Register::Rsp as usize] = 0x60_0010;
    fixture.set_gate(8, [0; 16]);
    let before = fixture.state.clone();
    let (exception, _) = fault_event(14, Some(2), Some(0xabc));
    let result = fixture.deliver(exception);
    assert_eq!(
        result,
        D::Shutdown {
            nested: vec![
                NestedException {
                    vector: 14,
                    error_code: Some(2),
                    linear_address: Some(0x60_0008)
                },
                NestedException {
                    vector: 8,
                    error_code: Some(0),
                    linear_address: None
                },
                NestedException {
                    vector: 13,
                    error_code: Some(67),
                    linear_address: None
                },
            ],
        }
    );
    let mut expected = before;
    expected.activity = Some(CpuActivity::Shutdown);
    expected.cr2 = Some(0x60_0008);
    assert_eq!(fixture.state, expected);
}

#[test]
fn a_frame_crossing_into_a_missing_page_leaves_its_first_words() {
    // The aligned RSP is 16 bytes into a page whose predecessor is not mapped: SS and RSP are
    // stored, the RFLAGS store faults. #GP then #PF: serial; the #PF is delivered on IST2.
    let mut fixture = Fixture::new(false);
    fixture.memory.map(0x65_0000, true, false);
    fixture.state.gpr[Register::Rsp as usize] = 0x65_0010;
    fixture.set_gate(14, gate(handler(14), 0x08, 2, 0x8e));
    let rsp = 0x65_0010;
    let (exception, event) = fault_event(13, Some(0), None);
    let result = check(&fixture, exception, event, "crossing");
    assert!(matches!(result, D::Delivered { vector: 14, .. }));
    let mut run = Fixture {
        state: fixture.state.clone(),
        memory: fixture.memory.clone(),
    };
    run.deliver(exception);
    assert_eq!(run.memory.qword(rsp - 8), 0x10, "the dead SS word");
    assert_eq!(run.memory.qword(rsp - 16), rsp, "the dead RSP word");
    assert_eq!(run.state.cr2, Some(rsp - 24));
}

#[test]
fn model_forms_are_refused_without_a_write_or_a_state_change() {
    let cases: Vec<(&str, Box<dyn Fn(&mut Fixture)>, R)> = vec![
        (
            "halted",
            Box::new(|f| f.state.activity = Some(CpuActivity::Halted)),
            R::ActivityOutsideProfile,
        ),
        (
            "blocking shadow with IF = 0",
            Box::new(|f| {
                f.state.sti_shadow = Some(StiShadow::Blocking);
                f.state.rflags &= !IF;
            }),
            R::StiShadowActive,
        ),
        (
            "shadow unknown",
            Box::new(|f| f.state.sti_shadow = None),
            R::StiShadowActive,
        ),
        ("no IDTR", Box::new(|f| f.state.idtr = None), R::MissingIdtr),
        ("no GDTR", Box::new(|f| f.state.gdtr = None), R::MissingGdtr),
        (
            "IDT span",
            Box::new(|f| {
                f.state.idtr = Some(Idtr {
                    base: 0x0000_7fff_ffff_fff8,
                    limit: 0xfff,
                })
            }),
            R::NonCanonicalIdtSpan,
        ),
        (
            "reserved gate bits",
            Box::new(|f| f.set_gate(0, gate(handler(0), 0x08, 0x08, 0x8e))),
            R::GateReservedBitsSet,
        ),
        (
            "LDT selector",
            Box::new(|f| f.set_gate(0, gate(handler(0), 0x0c, 0, 0x8e))),
            R::LdtTargetSelector,
        ),
        (
            "selector RPL",
            Box::new(|f| f.set_gate(0, gate(handler(0), 0x09, 0, 0x8e))),
            R::TargetSelectorRpl,
        ),
        (
            "conforming handler segment",
            Box::new(|f| f.memory.put_qword(GDT + 8, CODE64 | (1 << 42))),
            R::UnsupportedTargetCodeSegment,
        ),
        (
            "no task register for an IST gate",
            Box::new(|f| {
                f.state.tr = None;
                f.set_gate(0, gate(handler(0), 0x08, 1, 0x8e));
            }),
            R::MissingTaskRegister,
        ),
        (
            "TSS span",
            Box::new(|f| {
                f.state.tr.as_mut().unwrap().base = 0x0000_7fff_ffff_ffe0;
                f.set_gate(0, gate(handler(0), 0x08, 1, 0x8e));
            }),
            R::NonCanonicalTssSpan,
        ),
        (
            "a frame below address 0",
            Box::new(|f| f.state.gpr[Register::Rsp as usize] = 0x20),
            R::StackAddressOverflow,
        ),
    ];
    for (label, prepare, reason) in cases {
        let mut fixture = Fixture::new(false);
        prepare(&mut fixture);
        let before = fixture.state.clone();
        let (exception, _) = fault_event(0, None, None);
        assert_eq!(
            fixture.deliver(exception),
            D::Refused {
                vector: 0,
                reason,
                nested: Vec::new()
            },
            "{label}"
        );
        assert_eq!(fixture.state, before, "{label}");
        assert!(fixture.memory.writes.is_empty(), "{label}");
    }
    // Not Long64 CPL0/CPL3: a Ring1 context.
    let mut fixture = Fixture::new(false);
    let (exception, _) = fault_event(0, None, None);
    let mut state = fixture.state.clone();
    assert!(matches!(
        deliver_exception_cpl0_set(
            &mut state,
            &mut fixture.memory,
            exception,
            context(PrivilegeLevel::Ring1)
        ),
        D::Refused {
            reason: R::NotRing0Long64,
            ..
        }
    ));
    // A blocking shadow at CPL3.
    let mut fixture = Fixture::new(true);
    fixture.state.sti_shadow = Some(StiShadow::Blocking);
    let (exception, _) = fault_event(0, None, None);
    assert!(matches!(
        fixture.deliver(exception),
        D::Refused {
            reason: R::StiShadowActive,
            ..
        }
    ));
}

#[test]
fn a_blocking_shadow_with_if_set_at_cpl0_is_delivered_and_ends() {
    let mut fixture = Fixture::new(false);
    fixture.state.sti_shadow = Some(StiShadow::Blocking);
    let (exception, event) = fault_event(6, None, None);
    let mut expected_state = fixture.state.clone();
    expected_state.sti_shadow = Some(StiShadow::Clear);
    let fixture_clear = Fixture {
        state: expected_state,
        memory: fixture.memory.clone(),
    };
    let expected = model(&fixture_clear.state, &fixture_clear.memory, event);
    let result = fixture.deliver(exception);
    assert!(matches!(result, D::Delivered { vector: 6, .. }));
    let Expected::Delivered { state, .. } = expected else {
        panic!()
    };
    assert_eq!(fixture.state, state);
}

#[test]
fn a_backend_error_aborts_with_the_state_unchanged() {
    for (page, phase) in [
        (IDT, Cpl0ExceptionPhase::IdtGateRead),
        (GDT, Cpl0ExceptionPhase::TargetCodeDescriptorRead),
        (TSS, Cpl0ExceptionPhase::TssStackRead),
    ] {
        let mut fixture = Fixture::new(false);
        fixture.set_gate(0, gate(handler(0), 0x08, 1, 0x8e));
        fixture.memory.backend_error_page = Some(page);
        let before = fixture.state.clone();
        let (exception, _) = fault_event(0, None, None);
        let result = fixture.deliver(exception);
        assert!(
            matches!(&result, D::Aborted { vector: 0, phase: p, cause: MemoryError::Backend(_), nested }
                if *p == phase && nested.is_empty()),
            "{result:?}"
        );
        assert_eq!(fixture.state, before);
    }
    let mut fixture = Fixture::new(false);
    fixture.memory.backend_error_page = Some((KERNEL_STACK - 0x2000 + 0x123) & !0xfff);
    let (exception, _) = fault_event(0, None, None);
    assert!(matches!(
        fixture.deliver(exception),
        D::Aborted {
            phase: Cpl0ExceptionPhase::StackWrite {
                slot: od_isa::PageFaultFrameSlot::OldSs
            },
            ..
        }
    ));
}

#[test]
fn a_shutdown_in_an_ist_double_fault_keeps_cr2_of_the_last_page_fault() {
    // #PF (CR2 = 0xabc) whose gate is not present: #NP -> #DF through IST4 on an unmapped stack:
    // #PF -> shutdown; CR2 is the address of that last #PF.
    let mut fixture = Fixture::new(true);
    fixture.set_gate(14, gate(handler(14), 0x08, 0, 0x0e));
    fixture.memory.put_qword(TSS + 0x24 + 8 * 3, 0x60_0000);
    fixture.set_gate(8, gate(handler(8), 0x08, 4, 0x8e));
    let (exception, event) = fault_event(14, Some(5), Some(0xabc));
    let result = check(&fixture, exception, event, "shutdown");
    assert!(matches!(result, D::Shutdown { .. }), "{result:?}");
    let mut run = Fixture {
        state: fixture.state.clone(),
        memory: fixture.memory.clone(),
    };
    run.deliver(exception);
    assert_eq!(run.state.cr2, Some(0x60_0000 - 8));
    assert_eq!(run.state.activity, Some(CpuActivity::Shutdown));
    assert_eq!(run.state.cs_selector, Some(0x1b), "no handler entered");
}

#[test]
fn the_windows_divide_error_shape() {
    // `ntkrnlmp` + 0xbed1de: `idiv r8d` with EDX:EAX = -2^31 and R8D = -1 at CPL0, the IDT of
    // Windows (vector 0: interrupt gate, IST 0; vector 8: IST 1). The #DE is a fault: the frame has
    // the RIP of the IDIV and RF = 1; RAX and RDX are as before the instruction.
    let mut fixture = Fixture::new(false);
    fixture.state.rip = 0xFFFF_F805_B17E_D1DE;
    fixture.state.gpr[Register::Rax as usize] = 0x8000_0000;
    fixture.state.gpr[Register::Rdx as usize] = 0xFFFF_FFFF;
    fixture.state.gpr[Register::R8 as usize] = 0xFFFF_FFFF;
    fixture.state.rflags = 0xa87;
    fixture.set_gate(8, gate(handler(8), 0x08, 1, 0x8e));
    let (exception, event) = fault_event(0, None, None);
    let result = check(&fixture, exception, event, "windows #DE");
    assert!(matches!(result, D::Delivered { vector: 0, ref nested, .. } if nested.is_empty()));
}
