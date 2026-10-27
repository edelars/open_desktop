//! Deterministic fixed-home allocation for the initial linear typed IR.
//!
//! This is a correctness baseline, not a optimizing allocator: definition
//! order assigns permanent homes and values are never evicted or coalesced.

use std::collections::{BTreeMap, BTreeSet};

use crate::arm64::{AddressBase, EncodeError, Encoder, XReg, XRegOrZero};
use crate::ir::{self, IntWidth, Operation, Program, TempId, VerifyError};

const FIRST_HOME_REGISTER: u8 = 9;
const MAX_HOME_REGISTERS: u8 = 4;
const FIRST_SCRATCH_REGISTER: u8 = 13;
const MAX_LDR_STR_OFFSET: u32 = 4095 * 8;
const STACK_ALIGNMENT: u32 = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AllocatorConfig {
    /// Number of fixed caller-saved value homes, selected from x9..x12.
    pub register_homes: u8,
    /// Bytes owned by the entry/helper frame before allocator-owned slots.
    pub reserved_prefix_bytes: u32,
}

impl Default for AllocatorConfig {
    fn default() -> Self {
        Self {
            register_homes: MAX_HOME_REGISTERS,
            reserved_prefix_bytes: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpillSlot {
    index: u32,
    byte_offset: u32,
}

impl SpillSlot {
    pub const fn index(self) -> u32 {
        self.index
    }

    pub const fn byte_offset(self) -> u32 {
        self.byte_offset
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueLocation {
    Register(XReg),
    Spill(SpillSlot),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HelperCapture {
    pub temp: TempId,
    pub home: XReg,
    pub slot: SpillSlot,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum HelperBoundaryPosition {
    BeforeOperation(usize),
    AfterOperation(usize),
}

/// A caller-declared call site immediately before or after an IR operation.
/// The allocator does not infer calls from arithmetic or other IR effects.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HelperBoundary {
    pub position: HelperBoundaryPosition,
    pub live_through: Vec<TempId>,
    pub captures: Vec<HelperCapture>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AllocationCounters {
    pub value_spill_slots: u32,
    pub value_spill_loads: u32,
    pub value_spill_stores: u32,
    pub helper_capture_slots: u32,
    pub helper_saves: u32,
    pub helper_reloads: u32,
    /// Each planned load/store/capture/restore is one ARM64 transfer word.
    pub planned_transfer_words: u32,
    pub planned_transfer_bytes: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameLayout {
    pub reserved_prefix_bytes: u32,
    pub allocator_bytes: u32,
    pub frame_bytes: u32,
    pub alignment: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AllocatorError {
    InvalidProgram(VerifyError),
    RegisterBudgetOutOfRange(u8),
    InvalidScratchRegister(u8),
    UnsupportedWidth {
        temp: TempId,
        width: IntWidth,
    },
    DuplicateHelperBoundary(usize),
    HelperBoundaryOutOfRange {
        index: usize,
        operation_count: usize,
    },
    InvalidAfterOperation(usize),
    HelperBoundaryAtOrAfterTerminal(usize),
    FrameLayoutOverflow,
    FrameOffsetOutOfRange(u32),
    UnknownTemporary(TempId),
    UnknownHelperBoundary(usize),
    ConflictingLiveHomes {
        operation: usize,
        first: TempId,
        second: TempId,
    },
    Encoding(EncodeError),
}

impl From<EncodeError> for AllocatorError {
    fn from(error: EncodeError) -> Self {
        Self::Encoding(error)
    }
}

/// Immutable mapping tied to the verified program it was built from.
pub struct Allocation<'program> {
    program: &'program Program,
    locations: BTreeMap<TempId, ValueLocation>,
    helper_boundaries: BTreeMap<usize, HelperBoundary>,
    frame: FrameLayout,
    counters: AllocationCounters,
}

impl<'program> Allocation<'program> {
    pub fn build(
        program: &'program Program,
        config: AllocatorConfig,
        helper_boundaries: &[HelperBoundaryPosition],
    ) -> Result<Self, AllocatorError> {
        if config.register_homes > MAX_HOME_REGISTERS {
            return Err(AllocatorError::RegisterBudgetOutOfRange(
                config.register_homes,
            ));
        }
        if config.reserved_prefix_bytes % 8 != 0 {
            return Err(AllocatorError::FrameLayoutOverflow);
        }
        ir::verify(program).map_err(AllocatorError::InvalidProgram)?;
        let definitions = collect_definitions(program)?;
        let liveness = compute_liveness(program);

        let mut locations = BTreeMap::new();
        let mut spill_slots = BTreeMap::new();
        let mut next_slot = 0u32;
        for (definition_index, (temp, _)) in definitions.iter().enumerate() {
            if definition_index < usize::from(config.register_homes) {
                let number = FIRST_HOME_REGISTER
                    + u8::try_from(definition_index)
                        .map_err(|_| AllocatorError::FrameLayoutOverflow)?;
                locations.insert(*temp, ValueLocation::Register(XReg::new(number)?));
            } else {
                let slot = make_slot(next_slot, config.reserved_prefix_bytes)?;
                next_slot = next_slot
                    .checked_add(1)
                    .ok_or(AllocatorError::FrameLayoutOverflow)?;
                locations.insert(*temp, ValueLocation::Spill(slot));
                spill_slots.insert(*temp, slot);
            }
        }

        let mut unique_boundaries = BTreeSet::new();
        let terminal_index = program
            .operations
            .iter()
            .position(|operation| {
                matches!(
                    operation,
                    Operation::Branch { .. }
                        | Operation::DispatchBranch { .. }
                        | Operation::Exit { .. }
                )
            })
            .ok_or_else(|| AllocatorError::InvalidProgram(VerifyError::MissingTerminal))?;
        for &position in helper_boundaries {
            let index = match position {
                HelperBoundaryPosition::BeforeOperation(index) => index,
                HelperBoundaryPosition::AfterOperation(index) => index
                    .checked_add(1)
                    .ok_or(AllocatorError::InvalidAfterOperation(index))?,
            };
            if index > program.operations.len() {
                return Err(AllocatorError::HelperBoundaryOutOfRange {
                    index,
                    operation_count: program.operations.len(),
                });
            }
            if index >= terminal_index {
                return Err(AllocatorError::HelperBoundaryAtOrAfterTerminal(index));
            }
            if !unique_boundaries.insert(index) {
                return Err(AllocatorError::DuplicateHelperBoundary(index));
            }
        }

        let mut crossing_register_temps = BTreeSet::new();
        let mut boundary_liveness = BTreeMap::new();
        for &index in &unique_boundaries {
            let live: Vec<_> = liveness[index].iter().copied().collect();
            for temp in &live {
                if matches!(locations.get(temp), Some(ValueLocation::Register(_))) {
                    crossing_register_temps.insert(*temp);
                }
            }
            boundary_liveness.insert(index, live);
        }

        let mut capture_slots = BTreeMap::new();
        for (temp, _) in &definitions {
            if crossing_register_temps.contains(temp) {
                let slot = make_slot(next_slot, config.reserved_prefix_bytes)?;
                next_slot = next_slot
                    .checked_add(1)
                    .ok_or(AllocatorError::FrameLayoutOverflow)?;
                capture_slots.insert(*temp, slot);
            }
        }

        let positions: BTreeMap<_, _> = helper_boundaries
            .iter()
            .map(|position| {
                let index = match position {
                    HelperBoundaryPosition::BeforeOperation(index) => *index,
                    HelperBoundaryPosition::AfterOperation(index) => index + 1,
                };
                (index, *position)
            })
            .collect();
        let mut boundary_map = BTreeMap::new();
        for (index, live_through) in boundary_liveness {
            let mut captures = Vec::new();
            for temp in &live_through {
                if let Some(ValueLocation::Register(home)) = locations.get(temp) {
                    captures.push(HelperCapture {
                        temp: *temp,
                        home: *home,
                        slot: capture_slots[temp],
                    });
                }
            }
            boundary_map.insert(
                index,
                HelperBoundary {
                    position: positions[&index],
                    live_through,
                    captures,
                },
            );
        }

        let allocator_bytes = next_slot
            .checked_mul(8)
            .ok_or(AllocatorError::FrameLayoutOverflow)?;
        let unaligned_frame = config
            .reserved_prefix_bytes
            .checked_add(allocator_bytes)
            .ok_or(AllocatorError::FrameLayoutOverflow)?;
        let frame_bytes = align_up(unaligned_frame, STACK_ALIGNMENT)?;
        let frame = FrameLayout {
            reserved_prefix_bytes: config.reserved_prefix_bytes,
            allocator_bytes,
            frame_bytes,
            alignment: STACK_ALIGNMENT,
        };
        validate_home_conflicts(&locations, &liveness)?;
        for location in locations.values() {
            if let ValueLocation::Spill(slot) = location {
                validate_slot_in_frame(*slot, frame)?;
            }
        }
        for boundary in boundary_map.values() {
            for capture in &boundary.captures {
                validate_slot_in_frame(capture.slot, frame)?;
            }
        }
        let capture_count =
            u32::try_from(capture_slots.len()).map_err(|_| AllocatorError::FrameLayoutOverflow)?;
        let save_reload_count = boundary_map.values().try_fold(0u32, |sum, boundary| {
            sum.checked_add(
                u32::try_from(boundary.captures.len())
                    .map_err(|_| AllocatorError::FrameLayoutOverflow)?,
            )
            .ok_or(AllocatorError::FrameLayoutOverflow)
        })?;
        let value_spill_loads = count_spilled_uses(program, &spill_slots)?;
        let value_spill_stores =
            u32::try_from(spill_slots.len()).map_err(|_| AllocatorError::FrameLayoutOverflow)?;
        let planned_transfer_words = value_spill_loads
            .checked_add(value_spill_stores)
            .and_then(|count| count.checked_add(save_reload_count))
            .and_then(|count| count.checked_add(save_reload_count))
            .ok_or(AllocatorError::FrameLayoutOverflow)?;
        let counters = AllocationCounters {
            value_spill_slots: value_spill_stores,
            value_spill_loads,
            value_spill_stores,
            helper_capture_slots: capture_count,
            helper_saves: save_reload_count,
            helper_reloads: save_reload_count,
            planned_transfer_words,
            planned_transfer_bytes: planned_transfer_words
                .checked_mul(4)
                .ok_or(AllocatorError::FrameLayoutOverflow)?,
        };

        Ok(Self {
            program,
            locations,
            helper_boundaries: boundary_map,
            frame,
            counters,
        })
    }

    pub fn program(&self) -> &'program Program {
        self.program
    }

    pub fn location(&self, temp: TempId) -> Result<ValueLocation, AllocatorError> {
        self.locations
            .get(&temp)
            .copied()
            .ok_or(AllocatorError::UnknownTemporary(temp))
    }

    pub fn helper_boundary(
        &self,
        position: HelperBoundaryPosition,
    ) -> Result<&HelperBoundary, AllocatorError> {
        let before_operation = match position {
            HelperBoundaryPosition::BeforeOperation(index) => index,
            HelperBoundaryPosition::AfterOperation(index) => index
                .checked_add(1)
                .ok_or(AllocatorError::InvalidAfterOperation(index))?,
        };
        self.helper_boundaries
            .get(&before_operation)
            .filter(|boundary| boundary.position == position)
            .ok_or(AllocatorError::UnknownHelperBoundary(before_operation))
    }

    pub const fn frame(&self) -> FrameLayout {
        self.frame
    }

    pub const fn counters(&self) -> AllocationCounters {
        self.counters
    }

    /// Load a temporary into the dedicated x13..x15 scratch set.
    pub fn emit_load_scratch(
        &self,
        encoder: &mut Encoder,
        temp: TempId,
        scratch: XReg,
    ) -> Result<(), AllocatorError> {
        validate_scratch(scratch)?;
        match self.location(temp)? {
            ValueLocation::Register(home) => encoder.mov_x(scratch, XRegOrZero::Reg(home)),
            ValueLocation::Spill(slot) => {
                self.validate_slot(slot)?;
                encoder.ldr_x(XRegOrZero::Reg(scratch), AddressBase::Sp, slot.byte_offset)?;
            }
        }
        Ok(())
    }

    /// Store a value from dedicated x13..x15 scratch into a temporary home.
    pub fn emit_store_scratch(
        &self,
        encoder: &mut Encoder,
        temp: TempId,
        scratch: XReg,
    ) -> Result<(), AllocatorError> {
        validate_scratch(scratch)?;
        match self.location(temp)? {
            ValueLocation::Register(home) => encoder.mov_x(home, XRegOrZero::Reg(scratch)),
            ValueLocation::Spill(slot) => {
                self.validate_slot(slot)?;
                encoder.str_x(XRegOrZero::Reg(scratch), AddressBase::Sp, slot.byte_offset)?;
            }
        }
        Ok(())
    }

    /// Save all caller-saved register homes live through an explicitly declared
    /// helper boundary. Stack homes already survive the call and need no copy.
    pub fn emit_save_before_helper(
        &self,
        encoder: &mut Encoder,
        position: HelperBoundaryPosition,
    ) -> Result<(), AllocatorError> {
        let boundary = self.helper_boundary(position)?;
        for capture in &boundary.captures {
            self.validate_slot(capture.slot)?;
            encoder.str_x(
                XRegOrZero::Reg(capture.home),
                AddressBase::Sp,
                capture.slot.byte_offset,
            )?;
        }
        Ok(())
    }

    /// Restore the register homes saved by `emit_save_before_helper`.
    pub fn emit_restore_after_helper(
        &self,
        encoder: &mut Encoder,
        position: HelperBoundaryPosition,
    ) -> Result<(), AllocatorError> {
        let boundary = self.helper_boundary(position)?;
        for capture in &boundary.captures {
            self.validate_slot(capture.slot)?;
            encoder.ldr_x(
                XRegOrZero::Reg(capture.home),
                AddressBase::Sp,
                capture.slot.byte_offset,
            )?;
        }
        Ok(())
    }

    fn validate_slot(&self, slot: SpillSlot) -> Result<(), AllocatorError> {
        if slot.byte_offset > MAX_LDR_STR_OFFSET {
            return Err(AllocatorError::FrameOffsetOutOfRange(slot.byte_offset));
        }
        let end = slot
            .byte_offset
            .checked_add(8)
            .ok_or(AllocatorError::FrameLayoutOverflow)?;
        if end > self.frame.frame_bytes {
            return Err(AllocatorError::FrameOffsetOutOfRange(slot.byte_offset));
        }
        Ok(())
    }
}

fn collect_definitions(program: &Program) -> Result<Vec<(TempId, IntWidth)>, AllocatorError> {
    let mut definitions = Vec::new();
    for operation in &program.operations {
        match operation {
            Operation::Constant { dst, width, .. } | Operation::ReadGpr { dst, width, .. } => {
                definitions.push((*dst, *width))
            }
            Operation::CheckedLoad { dst, .. } => definitions.push((*dst, IntWidth::W64)),
            Operation::Integer {
                result: Some(dst),
                width,
                ..
            } => definitions.push((*dst, *width)),
            _ => {}
        }
    }
    for (temp, width) in &definitions {
        if *width != IntWidth::W64 {
            return Err(AllocatorError::UnsupportedWidth {
                temp: *temp,
                width: *width,
            });
        }
    }
    Ok(definitions)
}

fn compute_liveness(program: &Program) -> Vec<BTreeSet<TempId>> {
    let mut live_at = vec![BTreeSet::new(); program.operations.len() + 1];
    for index in (0..program.operations.len()).rev() {
        let mut live = live_at[index + 1].clone();
        match &program.operations[index] {
            Operation::Constant { dst, .. }
            | Operation::ReadGpr { dst, .. }
            | Operation::CheckedLoad { dst, .. } => {
                live.remove(dst);
            }
            Operation::Integer {
                lhs, rhs, result, ..
            } => {
                if let Some(dst) = result {
                    live.remove(dst);
                }
                live.insert(*lhs);
                live.insert(*rhs);
            }
            Operation::WriteGpr { src, .. } => {
                live.insert(*src);
            }
            _ => {}
        }
        live_at[index] = live;
    }
    live_at
}

fn count_spilled_uses(
    program: &Program,
    spill_slots: &BTreeMap<TempId, SpillSlot>,
) -> Result<u32, AllocatorError> {
    let mut loads = 0u32;
    for operation in &program.operations {
        match operation {
            Operation::Integer { lhs, rhs, .. } => {
                for temp in [lhs, rhs] {
                    if spill_slots.contains_key(temp) {
                        loads = loads
                            .checked_add(1)
                            .ok_or(AllocatorError::FrameLayoutOverflow)?;
                    }
                }
            }
            Operation::WriteGpr { src, .. } => {
                if spill_slots.contains_key(src) {
                    loads = loads
                        .checked_add(1)
                        .ok_or(AllocatorError::FrameLayoutOverflow)?;
                }
            }
            _ => {}
        }
    }
    Ok(loads)
}

fn validate_home_conflicts(
    locations: &BTreeMap<TempId, ValueLocation>,
    liveness: &[BTreeSet<TempId>],
) -> Result<(), AllocatorError> {
    for (operation, live) in liveness.iter().enumerate() {
        let mut owners = Vec::<(ValueLocation, TempId)>::new();
        for temp in live {
            let Some(location) = locations.get(temp).copied() else {
                continue;
            };
            if let Some((_, first)) = owners.iter().find(|(owner, _)| *owner == location) {
                return Err(AllocatorError::ConflictingLiveHomes {
                    operation,
                    first: *first,
                    second: *temp,
                });
            }
            owners.push((location, *temp));
        }
    }
    Ok(())
}

fn validate_slot_in_frame(slot: SpillSlot, frame: FrameLayout) -> Result<(), AllocatorError> {
    if slot.byte_offset > MAX_LDR_STR_OFFSET {
        return Err(AllocatorError::FrameOffsetOutOfRange(slot.byte_offset));
    }
    let end = slot
        .byte_offset
        .checked_add(8)
        .ok_or(AllocatorError::FrameLayoutOverflow)?;
    if end > frame.frame_bytes {
        return Err(AllocatorError::FrameOffsetOutOfRange(slot.byte_offset));
    }
    Ok(())
}

fn make_slot(index: u32, reserved_prefix: u32) -> Result<SpillSlot, AllocatorError> {
    let byte_offset = index
        .checked_mul(8)
        .and_then(|offset| reserved_prefix.checked_add(offset))
        .ok_or(AllocatorError::FrameLayoutOverflow)?;
    if byte_offset % 8 != 0 {
        return Err(AllocatorError::FrameLayoutOverflow);
    }
    if byte_offset > MAX_LDR_STR_OFFSET {
        return Err(AllocatorError::FrameOffsetOutOfRange(byte_offset));
    }
    Ok(SpillSlot { index, byte_offset })
}

fn align_up(value: u32, alignment: u32) -> Result<u32, AllocatorError> {
    value
        .checked_add(alignment - 1)
        .map(|sum| sum & !(alignment - 1))
        .ok_or(AllocatorError::FrameLayoutOverflow)
}

fn validate_scratch(register: XReg) -> Result<(), AllocatorError> {
    if (FIRST_SCRATCH_REGISTER..FIRST_SCRATCH_REGISTER + 3).contains(&register.number()) {
        Ok(())
    } else {
        Err(AllocatorError::InvalidScratchRegister(register.number()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{ExitKind, GuestGpr, GuestInstruction};

    fn program(ids: &[u32], width: IntWidth) -> Program {
        let mut operations = vec![Operation::BeginInstruction { index: 0 }];
        for (index, id) in ids.iter().copied().enumerate() {
            operations.push(Operation::Constant {
                dst: TempId(id),
                width,
                value: index as u64 + 1,
            });
        }
        for (index, id) in ids.iter().copied().enumerate() {
            operations.push(Operation::WriteGpr {
                register: GuestGpr((index % 4) as u8),
                src: TempId(id),
                width,
            });
        }
        operations.push(Operation::CommitInstruction { index: 0 });
        operations.push(Operation::Exit {
            kind: ExitKind::BlockComplete,
            target_rip: 0x1001,
        });
        Program {
            guest_instructions: vec![GuestInstruction {
                guest_rip: 0x1000,
                next_rip: 0x1001,
            }],
            operations,
            code_end: 0x1001,
        }
    }

    #[test]
    fn checked_store_declares_an_empty_helper_boundary() {
        let program = Program {
            guest_instructions: vec![GuestInstruction {
                guest_rip: 0x2000,
                next_rip: 0x2003,
            }],
            operations: vec![
                Operation::BeginInstruction { index: 0 },
                Operation::CheckedStore {
                    src: GuestGpr(0),
                    operand: od_isa::long_decode::MemoryOperand {
                        base: od_isa::long_decode::AddressBase::Register(od_isa::Register::Rax),
                        index: None,
                        scale: 1,
                        displacement: 0,
                        address_size: od_isa::OperandSize::Qword,
                        size: od_isa::OperandSize::Qword,
                    },
                },
                Operation::CommitInstruction { index: 0 },
                Operation::Exit {
                    kind: ExitKind::BlockComplete,
                    target_rip: 0x2003,
                },
            ],
            code_end: 0x2003,
        };
        let allocation = Allocation::build(
            &program,
            AllocatorConfig {
                register_homes: 4,
                reserved_prefix_bytes: 144,
            },
            &[HelperBoundaryPosition::BeforeOperation(1)],
        )
        .unwrap();
        let boundary = allocation
            .helper_boundary(HelperBoundaryPosition::BeforeOperation(1))
            .unwrap();
        assert!(boundary.live_through.is_empty());
        assert!(boundary.captures.is_empty());
    }

    #[test]
    fn budgets_spill_in_definition_order_and_sparse_ids_are_not_indices() {
        let program = program(&[0xFFFF_FF00, 3, 900], IntWidth::W64);
        let zero = Allocation::build(
            &program,
            AllocatorConfig {
                register_homes: 0,
                reserved_prefix_bytes: 16,
            },
            &[],
        )
        .unwrap();
        assert!(matches!(
            zero.location(TempId(0xFFFF_FF00)),
            Ok(ValueLocation::Spill(_))
        ));
        assert_eq!(zero.frame().reserved_prefix_bytes, 16);
        assert_eq!(zero.frame().allocator_bytes, 24);
        assert_eq!(zero.frame().frame_bytes % 16, 0);

        let one = Allocation::build(
            &program,
            AllocatorConfig {
                register_homes: 1,
                reserved_prefix_bytes: 0,
            },
            &[],
        )
        .unwrap();
        assert!(
            matches!(one.location(TempId(0xFFFF_FF00)), Ok(ValueLocation::Register(r)) if r.number() == 9)
        );
        assert!(matches!(
            one.location(TempId(3)),
            Ok(ValueLocation::Spill(_))
        ));

        let four = Allocation::build(&program, AllocatorConfig::default(), &[]).unwrap();
        assert_eq!(four.counters().value_spill_slots, 0);
    }

    #[test]
    fn mapping_is_deterministic_and_reports_helper_capture_cost() {
        let program = program(&[70, 2, 500], IntWidth::W64);
        let config = AllocatorConfig {
            register_homes: 1,
            reserved_prefix_bytes: 16,
        };
        let first = Allocation::build(
            &program,
            config,
            &[HelperBoundaryPosition::BeforeOperation(4)],
        )
        .unwrap();
        let second = Allocation::build(
            &program,
            config,
            &[HelperBoundaryPosition::BeforeOperation(4)],
        )
        .unwrap();
        assert_eq!(first.location(TempId(70)), second.location(TempId(70)));
        assert_eq!(first.location(TempId(2)), second.location(TempId(2)));
        assert_eq!(
            first
                .helper_boundary(HelperBoundaryPosition::BeforeOperation(4))
                .unwrap(),
            second
                .helper_boundary(HelperBoundaryPosition::BeforeOperation(4))
                .unwrap()
        );
        assert_eq!(first.counters().value_spill_slots, 2);
        assert_eq!(first.counters().helper_capture_slots, 1);
        assert_eq!(first.counters().helper_saves, 1);
        assert_eq!(first.counters().helper_reloads, 1);
        let after_position = Allocation::build(
            &program,
            config,
            &[HelperBoundaryPosition::AfterOperation(3)],
        )
        .unwrap();
        assert_eq!(
            after_position
                .helper_boundary(HelperBoundaryPosition::AfterOperation(3))
                .unwrap()
                .live_through,
            first
                .helper_boundary(HelperBoundaryPosition::BeforeOperation(4))
                .unwrap()
                .live_through
        );
    }

    #[test]
    fn read_gpr_value_is_captured_across_multiple_helper_boundaries() {
        let program = Program {
            guest_instructions: vec![GuestInstruction {
                guest_rip: 0x1000,
                next_rip: 0x1001,
            }],
            operations: vec![
                Operation::BeginInstruction { index: 0 },
                Operation::ReadGpr {
                    dst: TempId(77),
                    register: GuestGpr(0),
                    width: IntWidth::W64,
                },
                Operation::Constant {
                    dst: TempId(88),
                    width: IntWidth::W64,
                    value: 5,
                },
                Operation::Integer {
                    kind: crate::ir::IntegerKind::Add,
                    lhs: TempId(77),
                    rhs: TempId(88),
                    result: Some(TempId(99)),
                    width: IntWidth::W64,
                    flags: crate::ir::FlagEffect::Arithmetic,
                },
                Operation::Integer {
                    kind: crate::ir::IntegerKind::Add,
                    lhs: TempId(77),
                    rhs: TempId(99),
                    result: Some(TempId(100)),
                    width: IntWidth::W64,
                    flags: crate::ir::FlagEffect::Arithmetic,
                },
                Operation::WriteGpr {
                    register: GuestGpr(0),
                    src: TempId(100),
                    width: IntWidth::W64,
                },
                Operation::CommitInstruction { index: 0 },
                Operation::Exit {
                    kind: ExitKind::BlockComplete,
                    target_rip: 0x1001,
                },
            ],
            code_end: 0x1001,
        };
        let allocation = Allocation::build(
            &program,
            AllocatorConfig {
                register_homes: 1,
                reserved_prefix_bytes: 0,
            },
            &[
                HelperBoundaryPosition::BeforeOperation(3),
                HelperBoundaryPosition::BeforeOperation(4),
            ],
        )
        .unwrap();

        let first = allocation
            .helper_boundary(HelperBoundaryPosition::BeforeOperation(3))
            .unwrap();
        let second = allocation
            .helper_boundary(HelperBoundaryPosition::BeforeOperation(4))
            .unwrap();
        assert!(first.live_through.contains(&TempId(77)));
        assert!(second.live_through.contains(&TempId(77)));
        assert!(second.live_through.contains(&TempId(99)));
        assert_eq!(first.captures.len(), 1);
        assert_eq!(second.captures.len(), 1);
        assert_eq!(first.captures[0].temp, TempId(77));
        assert_eq!(second.captures[0].temp, TempId(77));
        assert_eq!(first.captures[0].home, second.captures[0].home);
        assert_eq!(first.captures[0].slot, second.captures[0].slot);
        assert_eq!(allocation.counters().helper_capture_slots, 1);
        assert_eq!(allocation.counters().helper_saves, 2);
        assert_eq!(allocation.counters().helper_reloads, 2);
    }

    #[test]
    fn rejects_invalid_budget_width_boundary_and_frame_offset() {
        let p = program(&[1], IntWidth::W64);
        assert!(matches!(
            Allocation::build(
                &p,
                AllocatorConfig {
                    register_homes: 5,
                    reserved_prefix_bytes: 0
                },
                &[]
            ),
            Err(AllocatorError::RegisterBudgetOutOfRange(5))
        ));
        assert!(matches!(
            Allocation::build(
                &program(&[1], IntWidth::W32),
                AllocatorConfig::default(),
                &[]
            ),
            Err(AllocatorError::UnsupportedWidth { .. })
        ));
        assert!(matches!(
            Allocation::build(
                &p,
                AllocatorConfig::default(),
                &[HelperBoundaryPosition::BeforeOperation(usize::MAX)]
            ),
            Err(AllocatorError::HelperBoundaryOutOfRange { .. })
        ));
        assert!(matches!(
            Allocation::build(
                &p,
                AllocatorConfig::default(),
                &[HelperBoundaryPosition::AfterOperation(usize::MAX)]
            ),
            Err(AllocatorError::InvalidAfterOperation(usize::MAX))
        ));
        assert!(matches!(
            Allocation::build(
                &p,
                AllocatorConfig::default(),
                &[HelperBoundaryPosition::BeforeOperation(4)]
            ),
            Err(AllocatorError::HelperBoundaryAtOrAfterTerminal(4))
        ));
        assert!(matches!(
            Allocation::build(
                &p,
                AllocatorConfig {
                    register_homes: 0,
                    reserved_prefix_bytes: u32::MAX - 7
                },
                &[]
            ),
            Err(AllocatorError::FrameLayoutOverflow | AllocatorError::FrameOffsetOutOfRange(_))
        ));
    }

    #[test]
    fn emits_spill_and_explicit_helper_save_restore_transfers() {
        let program = program(&[4, 90], IntWidth::W64);
        let allocation = Allocation::build(
            &program,
            AllocatorConfig {
                register_homes: 1,
                reserved_prefix_bytes: 16,
            },
            &[HelperBoundaryPosition::BeforeOperation(3)],
        )
        .unwrap();
        let mut encoder = Encoder::new();
        let scratch = XReg::new(13).unwrap();
        allocation
            .emit_load_scratch(&mut encoder, TempId(90), scratch)
            .unwrap();
        allocation
            .emit_save_before_helper(&mut encoder, HelperBoundaryPosition::BeforeOperation(3))
            .unwrap();
        allocation
            .emit_restore_after_helper(&mut encoder, HelperBoundaryPosition::BeforeOperation(3))
            .unwrap();
        let code = encoder.finish().unwrap();
        assert_eq!(code.words().len(), 3);
        assert_eq!(allocation.counters().value_spill_loads, 1);
        assert_eq!(allocation.counters().value_spill_stores, 1);
        assert_eq!(allocation.counters().planned_transfer_words, 4);
        assert_eq!(allocation.counters().planned_transfer_bytes, 16);
    }
}
