//! Checked host-code ranges and guest-state boundaries for generated blocks.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostCodeRange {
    start: u32,
    end: u32,
}

impl HostCodeRange {
    pub(crate) const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    pub const fn start(self) -> u32 {
        self.start
    }

    pub const fn end(self) -> u32 {
        self.end
    }

    pub const fn contains(self, offset: u32) -> bool {
        self.start <= offset && offset < self.end
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatePhase {
    Service,
    BeforeInstruction,
    PendingEffects,
    HelperCall,
    AfterHelper,
    CommitInProgress,
    Committed,
    Exit,
}

/// What guest state the map identifies at a host-code range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotAdmissibility {
    /// Canonical state is at the RIP selected by `StatePointRange::guest_rip`.
    CanonicalBoundary,
    /// Current instruction effects are pending; canonical state still identifies
    /// the fixed faulting RIP, so a planned helper failure may be reported there.
    ReplayCurrentInstruction,
    /// Canonical fields may be partially written by the commit sequence.
    InadmissibleDuringCommit,
    /// A checked write may already have reached guest memory; replaying this
    /// instruction from the snapshot could duplicate a side effect.
    InadmissibleAfterEffect,
    /// Guest registers/RIP are canonical, but RFLAGS still represent the
    /// previous materialization point while a full arithmetic writer is pending.
    InadmissiblePendingFlags,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StatePointRange {
    pub range: HostCodeRange,
    pub guest_rip: GuestRipSource,
    pub instruction_index: Option<u32>,
    pub phase: StatePhase,
    pub snapshot: SnapshotAdmissibility,
}

/// Source of the architectural RIP represented by a host-code range. Shared
/// epilogue/branch paths read canonical state because multiple planned exits
/// can reach them with different committed RIPs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuestRipSource {
    Fixed(u64),
    CanonicalState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuestRipBoundary {
    Fixed(u64),
    BranchSelection {
        target_rip: u64,
        fallthrough_rip: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GuestBoundaryPoint {
    pub host_offset: u32,
    pub rip: GuestRipBoundary,
    pub instruction_index: u32,
    pub committed_instructions: u32,
    pub phase: StatePhase,
    /// Whether the committed boundary has canonical RFLAGS.
    pub snapshot: SnapshotAdmissibility,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PcMapError {
    EmptyCode,
    CodeLengthOverflow,
    UnalignedCodeLength(u32),
    EmptyRange {
        index: usize,
        offset: u32,
    },
    ReversedRange {
        index: usize,
        start: u32,
        end: u32,
    },
    UnalignedRange {
        index: usize,
        start: u32,
        end: u32,
    },
    RangeOutOfBounds {
        index: usize,
        end: u32,
        code_len: u32,
    },
    RangeGapOrOverlap {
        index: usize,
        expected_start: u32,
        actual_start: u32,
    },
    IncompleteCoverage {
        covered_end: u32,
        code_len: u32,
    },
    BoundaryOutOfBounds {
        index: usize,
        offset: u32,
        code_len: u32,
    },
    UnalignedBoundary {
        index: usize,
        offset: u32,
    },
    UnsortedBoundary {
        index: usize,
        previous: u32,
        actual: u32,
    },
    TooManyInstructions(usize),
    InvalidBoundaryPhase {
        index: usize,
        phase: StatePhase,
    },
}

/// Immutable map owned by an emitted/published block. Execution ranges are
/// non-empty half-open byte ranges. Guest boundaries are separate point
/// records, so no range pretends to contain the PC at its exclusive end.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PcMap {
    code_len: u32,
    instruction_count: u32,
    ranges: Vec<StatePointRange>,
    boundaries: Vec<GuestBoundaryPoint>,
}

impl PcMap {
    /// Return the snapshot classification only for a valid host instruction PC.
    pub fn snapshot_admissibility_at(&self, offset: u32) -> Option<SnapshotAdmissibility> {
        self.lookup(offset).map(|point| point.snapshot)
    }
    pub(crate) fn new(
        code_len: usize,
        instruction_count: usize,
        ranges: Vec<StatePointRange>,
        boundaries: Vec<GuestBoundaryPoint>,
    ) -> Result<Self, PcMapError> {
        if code_len == 0 {
            return Err(PcMapError::EmptyCode);
        }
        let code_len = u32::try_from(code_len).map_err(|_| PcMapError::CodeLengthOverflow)?;
        if code_len % 4 != 0 {
            return Err(PcMapError::UnalignedCodeLength(code_len));
        }
        let instruction_count = u32::try_from(instruction_count)
            .map_err(|_| PcMapError::TooManyInstructions(instruction_count))?;

        let mut expected_start = 0;
        for (index, state_point) in ranges.iter().enumerate() {
            let start = state_point.range.start;
            let end = state_point.range.end;
            if start > end {
                return Err(PcMapError::ReversedRange { index, start, end });
            }
            if start == end {
                return Err(PcMapError::EmptyRange {
                    index,
                    offset: start,
                });
            }
            if start % 4 != 0 || end % 4 != 0 {
                return Err(PcMapError::UnalignedRange { index, start, end });
            }
            if end > code_len {
                return Err(PcMapError::RangeOutOfBounds {
                    index,
                    end,
                    code_len,
                });
            }
            if start != expected_start {
                return Err(PcMapError::RangeGapOrOverlap {
                    index,
                    expected_start,
                    actual_start: start,
                });
            }
            if let Some(instruction_index) = state_point.instruction_index {
                if instruction_index >= instruction_count {
                    return Err(PcMapError::TooManyInstructions(
                        instruction_index as usize + 1,
                    ));
                }
            }
            expected_start = end;
        }
        if expected_start != code_len {
            return Err(PcMapError::IncompleteCoverage {
                covered_end: expected_start,
                code_len,
            });
        }

        let mut previous = None;
        for (index, boundary) in boundaries.iter().enumerate() {
            let offset = boundary.host_offset;
            if boundary.phase != StatePhase::Committed {
                return Err(PcMapError::InvalidBoundaryPhase {
                    index,
                    phase: boundary.phase,
                });
            }
            if offset > code_len {
                return Err(PcMapError::BoundaryOutOfBounds {
                    index,
                    offset,
                    code_len,
                });
            }
            if offset % 4 != 0 {
                return Err(PcMapError::UnalignedBoundary { index, offset });
            }
            if boundary.instruction_index >= instruction_count {
                return Err(PcMapError::TooManyInstructions(
                    boundary.instruction_index as usize + 1,
                ));
            }
            if let Some(previous) = previous {
                if offset < previous {
                    return Err(PcMapError::UnsortedBoundary {
                        index,
                        previous,
                        actual: offset,
                    });
                }
            }
            previous = Some(offset);
        }

        Ok(Self {
            code_len,
            instruction_count,
            ranges,
            boundaries,
        })
    }

    pub const fn code_len(&self) -> u32 {
        self.code_len
    }

    pub const fn instruction_count(&self) -> u32 {
        self.instruction_count
    }

    pub fn ranges(&self) -> &[StatePointRange] {
        &self.ranges
    }

    pub fn boundaries(&self) -> &[GuestBoundaryPoint] {
        &self.boundaries
    }

    /// Logical bytes occupied by the immutable map contents, excluding Vec
    /// allocator spare capacity.
    pub(crate) fn logical_storage_bytes(&self) -> Option<usize> {
        std::mem::size_of::<Self>()
            .checked_add(
                self.ranges
                    .len()
                    .checked_mul(std::mem::size_of::<StatePointRange>())?,
            )?
            .checked_add(
                self.boundaries
                    .len()
                    .checked_mul(std::mem::size_of::<GuestBoundaryPoint>())?,
            )
    }

    /// Look up an instruction PC in emitted code. The exclusive end, misaligned PCs,
    /// and code-adjacent padding never map to a guest instruction.
    pub fn lookup(&self, offset: u32) -> Option<&StatePointRange> {
        if offset % 4 != 0 || offset >= self.code_len {
            return None;
        }
        let index = self
            .ranges
            .partition_point(|entry| entry.range.start <= offset);
        self.ranges
            .get(index.checked_sub(1)?)
            .filter(|entry| entry.range.contains(offset))
    }

    pub fn boundary_at(&self, offset: u32) -> impl Iterator<Item = &GuestBoundaryPoint> {
        self.boundaries
            .iter()
            .filter(move |boundary| boundary.host_offset == offset)
    }

    pub fn has_helper_failure_site(&self, guest_rip: u64) -> bool {
        self.ranges.iter().any(|entry| {
            entry.phase == StatePhase::Exit
                && matches!(
                    entry.snapshot,
                    SnapshotAdmissibility::ReplayCurrentInstruction
                        | SnapshotAdmissibility::InadmissiblePendingFlags
                )
                && entry.guest_rip == GuestRipSource::Fixed(guest_rip)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(start: u32, end: u32, phase: StatePhase) -> StatePointRange {
        StatePointRange {
            range: HostCodeRange { start, end },
            guest_rip: GuestRipSource::Fixed(0x1000),
            instruction_index: Some(0),
            phase,
            snapshot: SnapshotAdmissibility::CanonicalBoundary,
        }
    }

    #[test]
    fn maps_cover_code_with_checked_half_open_ranges_and_separate_boundaries() {
        let map = PcMap::new(
            16,
            1,
            vec![
                point(0, 4, StatePhase::Service),
                point(4, 8, StatePhase::BeforeInstruction),
                point(8, 12, StatePhase::CommitInProgress),
                point(12, 16, StatePhase::Exit),
            ],
            vec![GuestBoundaryPoint {
                host_offset: 12,
                rip: GuestRipBoundary::Fixed(0x1001),
                instruction_index: 0,
                committed_instructions: 1,
                phase: StatePhase::Committed,
                snapshot: SnapshotAdmissibility::CanonicalBoundary,
            }],
        )
        .unwrap();

        assert_eq!(map.lookup(0).unwrap().phase, StatePhase::Service);
        assert_eq!(map.lookup(8).unwrap().phase, StatePhase::CommitInProgress);
        assert_eq!(map.lookup(16), None);
        assert_eq!(map.lookup(6), None);
        assert_eq!(
            map.snapshot_admissibility_at(4),
            Some(SnapshotAdmissibility::CanonicalBoundary)
        );
        for offset in [1, 6, 15, 16, 20, u32::MAX] {
            assert_eq!(map.snapshot_admissibility_at(offset), None, "PC {offset}");
        }
        assert_eq!(map.boundary_at(12).count(), 1);
    }

    #[test]
    fn pending_flags_ranges_are_not_canonical_but_remain_attributed_failure_sites() {
        let map = PcMap::new(
            8,
            1,
            vec![
                StatePointRange {
                    range: HostCodeRange { start: 0, end: 4 },
                    guest_rip: GuestRipSource::Fixed(0x1000),
                    instruction_index: Some(0),
                    phase: StatePhase::Exit,
                    snapshot: SnapshotAdmissibility::InadmissiblePendingFlags,
                },
                StatePointRange {
                    range: HostCodeRange { start: 4, end: 8 },
                    guest_rip: GuestRipSource::Fixed(0x1000),
                    instruction_index: Some(0),
                    phase: StatePhase::Exit,
                    snapshot: SnapshotAdmissibility::CanonicalBoundary,
                },
            ],
            vec![],
        )
        .unwrap();

        assert_eq!(
            map.snapshot_admissibility_at(0),
            Some(SnapshotAdmissibility::InadmissiblePendingFlags)
        );
        assert_eq!(
            map.snapshot_admissibility_at(4),
            Some(SnapshotAdmissibility::CanonicalBoundary)
        );
        assert!(map.has_helper_failure_site(0x1000));
        assert!(!map.has_helper_failure_site(0x2000));
    }

    #[test]
    fn rejects_gaps_overlaps_and_unaligned_or_out_of_range_edges() {
        assert!(matches!(
            PcMap::new(
                12,
                1,
                vec![
                    point(0, 8, StatePhase::Service),
                    point(8, 4, StatePhase::Exit),
                    point(4, 12, StatePhase::Service),
                ],
                vec![]
            ),
            Err(PcMapError::ReversedRange {
                index: 1,
                start: 8,
                end: 4
            })
        ));
        assert!(matches!(
            PcMap::new(
                8,
                1,
                vec![
                    point(0, 4, StatePhase::Service),
                    point(8, 8, StatePhase::Exit)
                ],
                vec![]
            ),
            Err(PcMapError::EmptyRange { .. }) | Err(PcMapError::RangeOutOfBounds { .. })
        ));
        assert!(matches!(
            PcMap::new(
                8,
                1,
                vec![
                    point(0, 4, StatePhase::Service),
                    point(4, 8, StatePhase::Exit)
                ],
                vec![GuestBoundaryPoint {
                    host_offset: 6,
                    rip: GuestRipBoundary::Fixed(0x1001),
                    instruction_index: 0,
                    committed_instructions: 1,
                    phase: StatePhase::Committed,
                    snapshot: SnapshotAdmissibility::CanonicalBoundary
                }]
            ),
            Err(PcMapError::UnalignedBoundary { .. })
        ));
        assert!(matches!(
            PcMap::new(8, 1, vec![point(0, 8, StatePhase::Service)], vec![])
                .unwrap()
                .lookup(8),
            None
        ));
        assert!(matches!(
            PcMap::new(
                8,
                1,
                vec![
                    point(0, 4, StatePhase::Service),
                    point(8, 8, StatePhase::Exit)
                ],
                vec![]
            ),
            Err(_)
        ));
    }
}
