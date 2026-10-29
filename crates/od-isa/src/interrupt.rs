//! Bounded caller-selected external interrupt delivery for a private Long64 CPU.

use crate::long_decode::canonical;
use crate::memory::{
    AccessContext, AccessOrigin, GuestMemory, InstructionMode, MemoryError, PrivilegeLevel,
};
use crate::state::CodeSegmentDescriptorCache;
use crate::{INTEGER_FLAGS, LongModeState};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterruptDeliveryPhase {
    IdtGateRead,
    TargetCodeDescriptorRead,
    StackWrite { slot: InterruptFrameSlot },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterruptFrameSlot {
    OldSs,
    OldRsp,
    OldRflags,
    OldCs,
    OldRip,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InterruptOutsideProfile {
    UnsupportedCr0State,
    InvalidRflagsDefined(u64),
    NotRing0Long64,
    InterruptsMasked,
    InterruptsInhibited,
    HaltedWithoutClearShadow,
    IncompatibleEntryFlags(u64),
    MissingIdtr,
    MissingVisibleCsOrSs,
    IncompatibleCurrentSelectors,
    MissingIdtEntry,
    NonCanonicalIdtSpan,
    InvalidGate,
    InvalidTargetSelector,
    MissingGdtr,
    MissingTargetDescriptor,
    NonCanonicalGdtSpan,
    InvalidTargetCodeDescriptor,
    InvalidCurrentCodeSegment,
    NonCanonicalTargetRip,
    StackAddressOverflow,
    NonCanonicalStackSpan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InterruptDeliveryResult {
    OutsideProfile(InterruptOutsideProfile),
    DeliveryAborted {
        phase: InterruptDeliveryPhase,
        cause: MemoryError,
    },
    Delivered {
        vector: u8,
        target_rip: u64,
        new_rsp: u64,
    },
}

/// Deliver exactly one caller-selected external maskable interrupt.
///
/// The caller certifies a fresh completed-instruction boundary, no pending
/// synchronous/debug/NMI event or inhibition, private synchronous RAM, and
/// CET/VMX/SMM/shadow stacks disabled. No instruction fetch or retirement is
/// performed here. A `DeliveryAborted` means some earlier stack writes may
/// already have reached memory; callers must stop and must not retry.
pub fn deliver_maskable_interrupt(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    vector: u8,
    context: AccessContext,
    rflags_defined: u64,
) -> InterruptDeliveryResult {
    if state.cr0.is_some() || state.cr4.is_some() {
        return outside(InterruptOutsideProfile::UnsupportedCr0State);
    }
    if state.sti_shadow == Some(crate::StiShadow::Blocking) {
        return outside(InterruptOutsideProfile::InterruptsInhibited);
    }
    if state.activity == Some(crate::CpuActivity::Halted)
        && state.sti_shadow != Some(crate::StiShadow::Clear)
    {
        return outside(InterruptOutsideProfile::HaltedWithoutClearShadow);
    }
    if rflags_defined & !INTEGER_FLAGS != 0 {
        return outside(InterruptOutsideProfile::InvalidRflagsDefined(
            rflags_defined,
        ));
    }
    if context.mode != InstructionMode::Long64 || context.privilege != PrivilegeLevel::Ring0 {
        return outside(InterruptOutsideProfile::NotRing0Long64);
    }
    const IF: u64 = 1 << 9;
    const ENTRY_REJECTED: u64 = (1 << 8) | (1 << 16) | (1 << 17) | (1 << 14);
    if state.rflags & IF == 0 {
        return outside(InterruptOutsideProfile::InterruptsMasked);
    }
    let incompatible = state.rflags & ENTRY_REJECTED;
    if incompatible != 0 {
        return outside(InterruptOutsideProfile::IncompatibleEntryFlags(
            incompatible,
        ));
    }
    let (Some(old_cs), Some(old_cs_cache), Some(old_ss), Some(idtr)) = (
        state.cs_selector,
        state.cs_cache,
        state.ss_selector,
        state.idtr,
    ) else {
        return outside(if state.idtr.is_none() {
            InterruptOutsideProfile::MissingIdtr
        } else {
            InterruptOutsideProfile::MissingVisibleCsOrSs
        });
    };
    let old_cs_access = old_cs_cache.access;
    let old_cs_flags = old_cs_cache.flags;
    if old_cs & 0xfffc == 0
        || old_cs & 3 != 0
        || old_ss & 3 != 0
        || old_cs_cache.base != 0
        || old_cs_cache.limit != u32::MAX
        || old_cs_access & 0x18 != 0x18
        || old_cs_access & 0x04 != 0
        || old_cs_access & 0x60 != 0
        || old_cs_access & 0x80 == 0
        || old_cs_flags & 0x2 == 0
        || old_cs_flags & 0x4 != 0
    {
        return outside(InterruptOutsideProfile::IncompatibleCurrentSelectors);
    }

    let idt_offset = u64::from(vector) * 16;
    if idt_offset + 15 > u64::from(idtr.limit) {
        return outside(InterruptOutsideProfile::MissingIdtEntry);
    }
    let Some(gate_address) = idtr.base.checked_add(idt_offset) else {
        return outside(InterruptOutsideProfile::NonCanonicalIdtSpan);
    };
    let Some(gate_last) = gate_address.checked_add(15) else {
        return outside(InterruptOutsideProfile::NonCanonicalIdtSpan);
    };
    if !canonical(gate_address) || !canonical(gate_last) {
        return outside(InterruptOutsideProfile::NonCanonicalIdtSpan);
    }
    let mut gate = [0; 16];
    if let Err(cause) = memory.read(gate_address, context.descriptor_table(), &mut gate) {
        return aborted(InterruptDeliveryPhase::IdtGateRead, cause);
    }
    let gate_type = gate[5] & 0x0f;
    if gate[4] & 0xf8 != 0
        || gate[4] & 7 != 0
        || gate[12..16] != [0, 0, 0, 0]
        || gate_type != 0x0e
        || gate[5] & 0x10 != 0
        || gate[5] & 0x80 == 0
    {
        return outside(InterruptOutsideProfile::InvalidGate);
    }
    let target_selector = u16::from_le_bytes([gate[2], gate[3]]);
    if target_selector & 0xfffc == 0 || target_selector & 4 != 0 {
        return outside(InterruptOutsideProfile::InvalidTargetSelector);
    }
    let Some(gdtr) = state.gdtr else {
        return outside(InterruptOutsideProfile::MissingGdtr);
    };
    let descriptor_offset = u64::from(target_selector & !7);
    if descriptor_offset + 7 > u64::from(gdtr.limit) {
        return outside(InterruptOutsideProfile::MissingTargetDescriptor);
    }
    let Some(descriptor_address) = gdtr.base.checked_add(descriptor_offset) else {
        return outside(InterruptOutsideProfile::NonCanonicalGdtSpan);
    };
    let Some(descriptor_last) = descriptor_address.checked_add(7) else {
        return outside(InterruptOutsideProfile::NonCanonicalGdtSpan);
    };
    if !canonical(descriptor_address) || !canonical(descriptor_last) {
        return outside(InterruptOutsideProfile::NonCanonicalGdtSpan);
    }
    let mut descriptor = [0; 8];
    if let Err(cause) = memory.read(
        descriptor_address,
        context.descriptor_table(),
        &mut descriptor,
    ) {
        return aborted(InterruptDeliveryPhase::TargetCodeDescriptorRead, cause);
    }
    let access = descriptor[5];
    let flags = descriptor[6] >> 4;
    let target_rip = u64::from(u16::from_le_bytes([gate[0], gate[1]]))
        | (u64::from(u16::from_le_bytes([gate[6], gate[7]])) << 16)
        | (u64::from(u32::from_le_bytes(gate[8..12].try_into().unwrap())) << 32);
    if access & 0x10 == 0
        || access & 0x08 == 0
        || access & 0x04 != 0
        || access & 0x60 != 0
        || access & 0x80 == 0
        || access & 1 == 0
        || flags & 0x2 == 0
        || flags & 0x4 != 0
        || !canonical(target_rip)
    {
        return outside(if !canonical(target_rip) {
            InterruptOutsideProfile::NonCanonicalTargetRip
        } else {
            InterruptOutsideProfile::InvalidTargetCodeDescriptor
        });
    }

    let aligned_rsp = state.gpr[crate::Register::Rsp as usize] & !15;
    let Some(new_rsp) = aligned_rsp.checked_sub(40) else {
        return outside(InterruptOutsideProfile::StackAddressOverflow);
    };
    let slots = [
        (aligned_rsp - 8, u64::from(old_ss)),
        (aligned_rsp - 16, state.gpr[crate::Register::Rsp as usize]),
        (aligned_rsp - 24, state.rflags),
        (aligned_rsp - 32, u64::from(old_cs)),
        (aligned_rsp - 40, state.rip),
    ];
    if slots.iter().any(|(address, _)| {
        address
            .checked_add(7)
            .is_none_or(|last| !canonical(*address) || !canonical(last))
    }) {
        return outside(InterruptOutsideProfile::NonCanonicalStackSpan);
    }
    let write_context = AccessContext {
        origin: AccessOrigin::Explicit,
        ..context
    };
    let frame_slots = [
        InterruptFrameSlot::OldSs,
        InterruptFrameSlot::OldRsp,
        InterruptFrameSlot::OldRflags,
        InterruptFrameSlot::OldCs,
        InterruptFrameSlot::OldRip,
    ];
    for ((address, value), slot) in slots.into_iter().zip(frame_slots) {
        if let Err(cause) = memory.write(address, write_context, &value.to_le_bytes()) {
            return aborted(InterruptDeliveryPhase::StackWrite { slot }, cause);
        }
    }

    state.cs_selector = Some(target_selector & !3);
    state.cs_cache = Some(CodeSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access,
        flags,
    });
    state.gpr[crate::Register::Rsp as usize] = new_rsp;
    state.rip = target_rip;
    state.rflags &= !((1 << 9) | (1 << 8) | (1 << 17) | (1 << 16) | (1 << 14));
    if state.activity == Some(crate::CpuActivity::Halted) {
        state.activity = Some(crate::CpuActivity::Running);
    }
    InterruptDeliveryResult::Delivered {
        vector,
        target_rip,
        new_rsp,
    }
}

fn outside(reason: InterruptOutsideProfile) -> InterruptDeliveryResult {
    InterruptDeliveryResult::OutsideProfile(reason)
}

fn aborted(phase: InterruptDeliveryPhase, cause: MemoryError) -> InterruptDeliveryResult {
    InterruptDeliveryResult::DeliveryAborted { phase, cause }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OperandSize;
    use crate::memory::{AccessKind, ArchitecturalFault};

    struct Ram {
        bytes: Vec<u8>,
        reads: Vec<(u64, usize, AccessContext)>,
        writes: Vec<(u64, usize, AccessContext)>,
        fetches: usize,
        fail_read: Option<usize>,
        fail_write: Option<usize>,
    }

    impl Ram {
        fn new() -> Self {
            Self {
                bytes: vec![0; 0x10_000],
                reads: Vec::new(),
                writes: Vec::new(),
                fetches: 0,
                fail_read: None,
                fail_write: None,
            }
        }

        fn range(&self, address: u64, len: usize) -> Result<std::ops::Range<usize>, MemoryError> {
            let start = usize::try_from(address).unwrap_or(usize::MAX);
            let end = start.checked_add(len).unwrap_or(usize::MAX);
            if end > self.bytes.len() {
                return Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Read,
                });
            }
            Ok(start..end)
        }

        fn configure(&mut self, vector: u8, target: u64, access: u8, flags: u8) {
            let gate_at = 0x2000 + usize::from(vector) * 16;
            self.bytes[gate_at..gate_at + 2].copy_from_slice(&(target as u16).to_le_bytes());
            self.bytes[gate_at + 2..gate_at + 4].copy_from_slice(&0x23u16.to_le_bytes());
            self.bytes[gate_at + 4] = 0;
            self.bytes[gate_at + 5] = 0x8e;
            self.bytes[gate_at + 6..gate_at + 8]
                .copy_from_slice(&((target >> 16) as u16).to_le_bytes());
            self.bytes[gate_at + 8..gate_at + 12]
                .copy_from_slice(&((target >> 32) as u32).to_le_bytes());
            self.bytes[gate_at + 12..gate_at + 16].fill(0);
            let desc_at = 0x1000 + 0x20;
            self.bytes[desc_at..desc_at + 8].fill(0);
            self.bytes[desc_at + 5] = access;
            self.bytes[desc_at + 6] = flags << 4;
        }
    }

    impl GuestMemory for Ram {
        fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
            self.fetches += 1;
            let range = self.range(address, 1)?;
            Ok(self.bytes[range.start])
        }

        fn read(
            &mut self,
            address: u64,
            context: AccessContext,
            destination: &mut [u8],
        ) -> Result<(), MemoryError> {
            let index = self.reads.len();
            self.reads.push((address, destination.len(), context));
            if self.fail_read == Some(index) {
                return Err(MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(0),
                    linear_address: Some(address),
                }));
            }
            destination.copy_from_slice(&self.bytes[self.range(address, destination.len())?]);
            Ok(())
        }

        fn write(
            &mut self,
            address: u64,
            context: AccessContext,
            source: &[u8],
        ) -> Result<(), MemoryError> {
            let index = self.writes.len();
            self.writes.push((address, source.len(), context));
            if self.fail_write == Some(index) {
                return Err(MemoryError::Backend("injected frame store failure".into()));
            }
            let range = self.range(address, source.len())?;
            self.bytes[range].copy_from_slice(source);
            Ok(())
        }

        fn atomic_exchange_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: OperandSize,
            _: u64,
        ) -> Result<u64, MemoryError> {
            unreachable!()
        }

        fn atomic_fetch_add_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: OperandSize,
            _: u64,
        ) -> Result<u64, MemoryError> {
            unreachable!()
        }

        fn atomic_compare_exchange_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: OperandSize,
            _: u64,
            _: u64,
        ) -> Result<u64, MemoryError> {
            unreachable!()
        }

        fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
            unreachable!()
        }
    }

    fn ready_state(rsp: u64) -> LongModeState {
        LongModeState {
            cr4: None,
            sti_shadow: None,
            gpr: {
                let mut gpr = [0; 16];
                gpr[crate::Register::Rsp as usize] = rsp;
                gpr
            },
            rip: 0x1234_5678_9abc,
            rflags: 0x2 | (1 << 9) | (1 << 0) | (1 << 2) | (1 << 10),
            cs_selector: Some(0x8),
            cs_cache: Some(CodeSegmentDescriptorCache {
                base: 0,
                limit: u32::MAX,
                access: 0x9b,
                flags: 2,
            }),
            ss_selector: Some(0x10),
            ss_cache: Some(crate::StackSegmentDescriptorCache {
                base: 0,
                limit: u32::MAX,
                access: 0x93,
                flags: 0,
            }),
            gdtr: Some(crate::state::Gdtr {
                base: 0x1000,
                limit: 0x2f,
            }),
            idtr: Some(crate::state::Idtr {
                base: 0x2000,
                limit: 0x0fff,
            }),
            ..LongModeState::default()
        }
    }

    fn deliver(state: &mut LongModeState, ram: &mut Ram, vector: u8) -> InterruptDeliveryResult {
        deliver_maskable_interrupt(
            state,
            ram,
            vector,
            AccessContext {
                privilege: PrivilegeLevel::Ring0,
                ..AccessContext::default()
            },
            INTEGER_FLAGS,
        )
    }

    #[test]
    fn active_sti_shadow_rejects_delivery_before_descriptor_or_stack_access() {
        let mut state = ready_state(0x8000);
        state.sti_shadow = Some(crate::StiShadow::Blocking);
        let before = state.clone();
        let mut ram = Ram::new();
        assert_eq!(
            deliver(&mut state, &mut ram, 0x20),
            InterruptDeliveryResult::OutsideProfile(InterruptOutsideProfile::InterruptsInhibited)
        );
        assert_eq!(state, before);
        assert!(ram.reads.is_empty());
        assert!(ram.writes.is_empty());
    }

    #[test]
    fn cr0_state_rejects_interrupt_delivery_before_any_memory_callback() {
        let mut state = ready_state(0x8000);
        state.cr0 = Some(crate::execute::Cr0Profile::BASE);
        let before = state.clone();
        let mut ram = Ram::new();
        assert_eq!(
            deliver(&mut state, &mut ram, 0x20),
            InterruptDeliveryResult::OutsideProfile(InterruptOutsideProfile::UnsupportedCr0State)
        );
        assert_eq!(state, before);
        assert_eq!(ram.fetches, 0);
        assert!(ram.reads.is_empty() && ram.writes.is_empty());
    }

    #[test]
    fn halted_delivery_requires_the_profiled_clear_shadow() {
        let mut state = ready_state(0x8000);
        state.activity = Some(crate::CpuActivity::Halted);
        let mut ram = Ram::new();
        assert_eq!(
            deliver(&mut state, &mut ram, 0x20),
            InterruptDeliveryResult::OutsideProfile(
                InterruptOutsideProfile::HaltedWithoutClearShadow
            )
        );
        assert!(ram.reads.is_empty());
        assert!(ram.writes.is_empty());
    }

    #[test]
    fn successful_delivery_from_halted_state_saves_post_hlt_rip_and_wakes() {
        let mut state = ready_state(0x8000);
        let post_hlt_rip = state.rip + 1;
        state.rip = post_hlt_rip;
        state.activity = Some(crate::CpuActivity::Halted);
        state.sti_shadow = Some(crate::StiShadow::Clear);
        let mut ram = Ram::new();
        let handler = 0x3000;
        ram.configure(0x20, handler, 0x9b, 0x2);
        ram.bytes[handler as usize] = 0x90;
        assert!(matches!(
            deliver(&mut state, &mut ram, 0x20),
            InterruptDeliveryResult::Delivered { target_rip, .. } if target_rip == handler
        ));
        assert_eq!(state.activity, Some(crate::CpuActivity::Running));
        let old_rip_slot = (0x8000 & !15) - 40;
        assert_eq!(
            u64::from_le_bytes(
                ram.bytes[old_rip_slot as usize..old_rip_slot as usize + 8]
                    .try_into()
                    .unwrap()
            ),
            post_hlt_rip
        );
        assert_eq!(
            crate::execute::step(
                &mut state,
                &mut ram,
                AccessContext {
                    privilege: PrivilegeLevel::Ring0,
                    ..AccessContext::default()
                }
            )
            .unwrap(),
            crate::execute::StepOutcome::Advanced
        );
        assert_eq!(state.rip, handler + 1);
        assert_eq!(state.activity, Some(crate::CpuActivity::Running));
    }

    #[test]
    fn delivery_writes_exact_frame_and_commits_only_after_all_stores() {
        for rsp in [
            0x8000, 0x8001, 0x8007, 0x8008, 0x800f, 0x8010, 0x8017, 0x801f,
        ] {
            let mut state = ready_state(rsp);
            let before = state.clone();
            let mut ram = Ram::new();
            ram.configure(0x20, 0xffff_8000_1234_5678, 0x9b, 0x2);
            assert_eq!(
                deliver(&mut state, &mut ram, 0x20),
                InterruptDeliveryResult::Delivered {
                    vector: 0x20,
                    target_rip: 0xffff_8000_1234_5678,
                    new_rsp: (rsp & !15) - 40,
                }
            );
            let old_rsp = before.gpr[crate::Register::Rsp as usize];
            let aligned = old_rsp & !15;
            let expected = [
                (aligned - 8, before.ss_selector.unwrap() as u64),
                (aligned - 16, old_rsp),
                (aligned - 24, before.rflags),
                (aligned - 32, before.cs_selector.unwrap() as u64),
                (aligned - 40, before.rip),
            ];
            assert_eq!(
                ram.writes.iter().map(|w| w.0).collect::<Vec<_>>(),
                expected.iter().map(|e| e.0).collect::<Vec<_>>()
            );
            for (address, value) in expected {
                assert_eq!(
                    &ram.bytes[address as usize..address as usize + 8],
                    &value.to_le_bytes()
                );
            }
            assert_eq!(state.cs_selector, Some(0x20));
            assert_eq!(
                state.cs_cache,
                Some(CodeSegmentDescriptorCache {
                    base: 0,
                    limit: u32::MAX,
                    access: 0x9b,
                    flags: 2,
                })
            );
            assert_eq!(state.ss_selector, before.ss_selector);
            assert_eq!(state.ss_cache, before.ss_cache);
            assert_eq!(state.gpr[crate::Register::Rsp as usize], (rsp & !15) - 40);
            assert_eq!(
                state.rflags,
                before.rflags & !((1 << 9) | (1 << 8) | (1 << 17) | (1 << 16) | (1 << 14))
            );
            assert_eq!(state.rip, 0xffff_8000_1234_5678);
            assert_eq!(ram.reads.len(), 2);
            assert_eq!(ram.fetches, 0);
            assert!(
                ram.reads
                    .iter()
                    .all(|(_, _, ctx)| ctx.origin == AccessOrigin::DescriptorTable)
            );
        }
    }

    #[test]
    fn dpl_and_rpl_do_not_mask_external_vectors_including_2_and_14() {
        for vector in [2, 14] {
            let mut state = ready_state(0x8000);
            let mut ram = Ram::new();
            ram.configure(vector, 0xffff_8000_1234_5678, 0x9b, 0x2);
            let gate = 0x2000 + usize::from(vector) * 16;
            ram.bytes[gate + 5] = 0xee; // DPL 3, type 0xe, present
            ram.bytes[gate + 2..gate + 4].copy_from_slice(&0x23u16.to_le_bytes());
            ram.bytes[0x1000 + 0x25] = 0x9b; // target RPL ignored
            assert!(matches!(
                deliver(&mut state, &mut ram, vector),
                InterruptDeliveryResult::Delivered { .. }
            ));
        }
    }

    #[test]
    fn null_ss_selector_without_cache_is_preserved_for_long64_delivery() {
        let mut state = ready_state(0x8000);
        state.ss_selector = Some(0);
        state.ss_cache = None;
        let mut ram = Ram::new();
        ram.configure(0x20, 0x1234, 0x9b, 0x2);
        assert!(matches!(
            deliver(&mut state, &mut ram, 0x20),
            InterruptDeliveryResult::Delivered { .. }
        ));
        assert_eq!(state.ss_selector, Some(0));
        assert_eq!(state.ss_cache, None);
    }

    #[test]
    fn every_stack_store_failure_keeps_cpu_state_and_prior_memory_writes() {
        for fail_at in 0..5 {
            let mut state = ready_state(0x8007);
            let before = state.clone();
            let mut ram = Ram::new();
            ram.configure(0x20, 0x1234_5678, 0x9b, 0x2);
            ram.fail_write = Some(fail_at);
            let result = deliver(&mut state, &mut ram, 0x20);
            assert!(matches!(
                result,
                InterruptDeliveryResult::DeliveryAborted {
                    phase: InterruptDeliveryPhase::StackWrite { .. },
                    ..
                }
            ));
            assert_eq!(state, before);
            assert_eq!(ram.writes.len(), fail_at + 1);
            for (address, _, _) in ram.writes.iter().take(fail_at) {
                assert_ne!(
                    &ram.bytes[*address as usize..*address as usize + 8],
                    &[0; 8]
                );
            }
        }
    }

    #[test]
    fn bad_gate_or_target_and_descriptor_read_fault_never_touch_stack() {
        for (access, flags, expected) in [
            (
                0x93,
                0x2,
                InterruptOutsideProfile::InvalidTargetCodeDescriptor,
            ),
            (
                0x9b,
                0x4,
                InterruptOutsideProfile::InvalidTargetCodeDescriptor,
            ),
        ] {
            let mut state = ready_state(0x8000);
            let mut ram = Ram::new();
            ram.configure(0x20, 0x1234, access, flags);
            assert_eq!(
                deliver(&mut state, &mut ram, 0x20),
                InterruptDeliveryResult::OutsideProfile(expected)
            );
            assert!(ram.writes.is_empty());
        }
        let mut state = ready_state(0x8000);
        let mut ram = Ram::new();
        ram.configure(0x20, 0x1234, 0x9b, 0x2);
        ram.fail_read = Some(1);
        let before = state.clone();
        assert!(matches!(
            deliver(&mut state, &mut ram, 0x20),
            InterruptDeliveryResult::DeliveryAborted {
                phase: InterruptDeliveryPhase::TargetCodeDescriptorRead,
                cause: MemoryError::Architectural(_)
            }
        ));
        assert_eq!(state, before);
        assert!(ram.writes.is_empty());
    }

    #[test]
    fn invalid_gate_reserved_type_present_and_ist_fields_do_not_write_frame() {
        let mutations: [(&str, fn(&mut [u8])); 6] = [
            ("system bit", |gate: &mut [u8]| gate[5] |= 0x10),
            ("reserved type", |gate: &mut [u8]| gate[5] = 0x8f),
            ("not present", |gate: &mut [u8]| gate[5] &= !0x80),
            ("IST nonzero", |gate: &mut [u8]| gate[4] = 1),
            ("IST reserved", |gate: &mut [u8]| gate[4] = 8),
            ("reserved dword", |gate: &mut [u8]| gate[12] = 1),
        ];
        for (label, mutate) in mutations {
            let mut state = ready_state(0x8000);
            let before = state.clone();
            let mut ram = Ram::new();
            ram.configure(0x20, 0x1234, 0x9b, 0x2);
            let gate_at = 0x2000 + 0x20 * 16;
            mutate(&mut ram.bytes[gate_at..gate_at + 16]);
            assert_eq!(
                deliver(&mut state, &mut ram, 0x20),
                InterruptDeliveryResult::OutsideProfile(InterruptOutsideProfile::InvalidGate),
                "{}",
                label
            );
            assert_eq!(state, before);
            assert!(ram.writes.is_empty());
        }
    }

    #[test]
    fn null_or_ti_set_gate_selectors_are_rejected_before_gdt_or_stack_access() {
        for selector in [0x0003u16, 0x0024] {
            let mut state = ready_state(0x8000);
            let mut ram = Ram::new();
            ram.configure(0x20, 0x1234, 0x9b, 0x2);
            let gate_at = 0x2000 + 0x20 * 16;
            ram.bytes[gate_at + 2..gate_at + 4].copy_from_slice(&selector.to_le_bytes());
            assert_eq!(
                deliver(&mut state, &mut ram, 0x20),
                InterruptDeliveryResult::OutsideProfile(
                    InterruptOutsideProfile::InvalidTargetSelector
                )
            );
            assert_eq!(ram.reads.len(), 1);
            assert!(ram.writes.is_empty());
        }
    }

    #[test]
    fn incoherent_current_cs_selector_or_long64_cache_is_rejected_before_idt_read() {
        for (selector, access, flags, base, limit) in [
            (0, 0x9b, 0x2, 0, u32::MAX),
            (0x9, 0x9b, 0x2, 0, u32::MAX),
            (0x8, 0x93, 0x2, 0, u32::MAX),
            (0x8, 0x9f, 0x2, 0, u32::MAX),
            (0x8, 0xbb, 0x2, 0, u32::MAX),
            (0x8, 0x1b, 0x2, 0, u32::MAX),
            (0x8, 0x9b, 0, 0, u32::MAX),
            (0x8, 0x9b, 0x4, 0, u32::MAX),
            (0x8, 0x9b, 0x6, 0, u32::MAX),
            (0x8, 0x9b, 0x2, 1, u32::MAX),
            (0x8, 0x9b, 0x2, 0, 0xffff),
        ] {
            let mut state = ready_state(0x8000);
            state.cs_selector = Some(selector);
            state.cs_cache = Some(CodeSegmentDescriptorCache {
                base,
                limit,
                access,
                flags,
            });
            let before = state.clone();
            let mut ram = Ram::new();
            assert_eq!(
                deliver(&mut state, &mut ram, 0x20),
                InterruptDeliveryResult::OutsideProfile(
                    InterruptOutsideProfile::IncompatibleCurrentSelectors
                )
            );
            assert_eq!(state, before);
            assert!(ram.reads.is_empty() && ram.writes.is_empty());
        }
    }

    #[test]
    fn gdt_limit_and_stack_canonicality_or_underflow_preflight_without_frame_writes() {
        let mut state = ready_state(0x8000);
        state.gdtr = Some(crate::state::Gdtr {
            base: 0x1000,
            limit: 0x26,
        });
        let mut ram = Ram::new();
        ram.configure(0x20, 0x1234, 0x9b, 0x2);
        assert_eq!(
            deliver(&mut state, &mut ram, 0x20),
            InterruptDeliveryResult::OutsideProfile(
                InterruptOutsideProfile::MissingTargetDescriptor
            )
        );
        assert_eq!(ram.reads.len(), 1);
        assert!(ram.writes.is_empty());

        for (rsp, expected) in [
            (0x20, InterruptOutsideProfile::StackAddressOverflow),
            (
                0x0000_8000_0000_0010,
                InterruptOutsideProfile::NonCanonicalStackSpan,
            ),
            (
                0xffff_8000_0000_0010,
                InterruptOutsideProfile::NonCanonicalStackSpan,
            ),
        ] {
            let mut state = ready_state(rsp);
            let before = state.clone();
            let mut ram = Ram::new();
            ram.configure(0x20, 0x1234, 0x9b, 0x2);
            assert_eq!(
                deliver(&mut state, &mut ram, 0x20),
                InterruptDeliveryResult::OutsideProfile(expected)
            );
            assert_eq!(state, before);
            assert!(ram.writes.is_empty());
        }
    }

    #[test]
    fn missing_idt_bounds_and_noncanonical_spans_do_not_read_or_write_frame() {
        let mut state = ready_state(0x8000);
        state.idtr = Some(crate::state::Idtr {
            base: 0x2000,
            limit: 0x1ff,
        });
        let mut ram = Ram::new();
        assert_eq!(
            deliver(&mut state, &mut ram, 0x20),
            InterruptDeliveryResult::OutsideProfile(InterruptOutsideProfile::MissingIdtEntry)
        );
        assert!(ram.reads.is_empty() && ram.writes.is_empty());
        state.idtr = Some(crate::state::Idtr {
            base: 0x0000_7fff_ffff_fff8,
            limit: 0xffff,
        });
        assert_eq!(
            deliver(&mut state, &mut ram, 1),
            InterruptDeliveryResult::OutsideProfile(InterruptOutsideProfile::NonCanonicalIdtSpan)
        );
        assert!(ram.reads.is_empty() && ram.writes.is_empty());
    }
}
