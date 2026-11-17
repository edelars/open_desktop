//! Representation of processor state in real mode and 32-bit protected mode (S6-14c).
//!
//! Representation only: there is no instruction execution, memory access or LAPIC access
//! here. Values come from Intel SDM Vol. 3A: Table 12-1 (state after INIT, footnote 2 on
//! CR0), section 11.4.3 step 6 and section 11.4.4.1 step 10 (SIPI sets `CS = vector << 8`,
//! base `vector << 12`, `EIP = 0`), section 12.1.4 (real-mode segment base rule) and
//! SMP_CONTRACT.md section 4.
//!
//! Not modelled here (left to task S6-13):
//! - real-mode segment attributes (SMP_CONTRACT does not define them for the reset state);
//! - CR2, CR3, CR4 and EFER (all 0 after INIT);
//! - GDTR and IDTR (base 0, limit 0xFFFF);
//! - LDTR and TR;
//! - zeroed general-purpose registers;
//! - the processor signature in EDX;
//! - the activity state (WaitForSipi);
//! - writing these values into `state.rs`.

use crate::state::OperandSize;

/// CS selector after INIT (Table 12-1).
pub const INIT_CS_SELECTOR: u16 = 0xF000;
/// CS base after INIT (Table 12-1).
pub const INIT_CS_BASE: u32 = 0xFFFF_0000;
/// EIP after INIT (Table 12-1).
pub const INIT_EIP: u32 = 0xFFF0;
/// EFLAGS after INIT: only the reserved bit 1 is set.
pub const INIT_EFLAGS: u32 = 0x2;
/// Limit of every segment in the real-mode reset state.
pub const REAL_MODE_LIMIT: u32 = 0xFFFF;
/// The ET bit in CR0, which INIT always sets.
pub const CR0_ET: u32 = 0x10;
/// The CD and NW bits (`0x6000_0000`): the only CR0 bits that INIT preserves.
pub const CR0_INIT_PRESERVED_MASK: u32 = 0x6000_0000;
/// Default operand size in real mode is 16 bits. Address size, also 16-bit by default, is
/// not represented in this module.
pub const REAL_MODE_DEFAULT_OPERAND_SIZE: OperandSize = OperandSize::Word;

/// Real-mode segment: selector, base and limit. Attributes are not modelled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RealSegment {
    pub selector: u16,
    pub base: u32,
    pub limit: u32,
}

impl RealSegment {
    /// Segment with reset-state values: selector and base 0, limit `0xFFFF`.
    pub const fn reset_zero() -> Self {
        Self {
            selector: 0,
            base: 0,
            limit: REAL_MODE_LIMIT,
        }
    }
}

/// Visible state defined by real mode: segment registers, IP, EFLAGS and CR0.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RealModeState {
    pub cs: RealSegment,
    pub ds: RealSegment,
    pub es: RealSegment,
    pub ss: RealSegment,
    pub fs: RealSegment,
    pub gs: RealSegment,
    pub eip: u32,
    pub eflags: u32,
    pub cr0: u32,
}

impl RealModeState {
    /// State after INIT. `old_cr0` is the CR0 value before INIT: only CD and NW are
    /// preserved from it, everything else is cleared and ET is set (footnote 2 of
    /// Table 12-1).
    pub const fn after_init(old_cr0: u32) -> Self {
        Self {
            cs: RealSegment {
                selector: INIT_CS_SELECTOR,
                base: INIT_CS_BASE,
                limit: REAL_MODE_LIMIT,
            },
            ds: RealSegment::reset_zero(),
            es: RealSegment::reset_zero(),
            ss: RealSegment::reset_zero(),
            fs: RealSegment::reset_zero(),
            gs: RealSegment::reset_zero(),
            eip: INIT_EIP,
            eflags: INIT_EFLAGS,
            cr0: (old_cr0 & CR0_INIT_PRESERVED_MASK) | CR0_ET,
        }
    }

    /// State of the first instruction after a SIPI with vector `vector`, applied to the
    /// state after INIT. SIPI changes only CS and EIP: `CS.selector = vector << 8`,
    /// `CS.base = vector << 12`, `EIP = 0`. CR0, EFLAGS and the other segments are kept.
    pub const fn after_sipi(self, vector: u8) -> Self {
        Self {
            cs: RealSegment {
                selector: (vector as u16) << 8,
                base: (vector as u32) << 12,
                limit: self.cs.limit,
            },
            eip: 0,
            ..self
        }
    }

    /// Linear (physical, no paging) address where execution will start:
    /// `CS.base + EIP`. For a SIPI with vector `v` this is `v << 12`, as required by
    /// SMP_CONTRACT section 4.
    pub const fn fetch_linear_address(&self) -> u32 {
        self.cs.base.wrapping_add(self.eip)
    }
}

/// Protected-mode segment: selector, base, limit after applying granularity, and the
/// access byte and flags from the descriptor. Used for GDT and LDT descriptors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtectedSegment {
    pub selector: u16,
    pub base: u32,
    /// Limit after granularity: with G=1 the limit in pages is expanded to end in `FFF`.
    pub limit: u32,
    /// Descriptor access byte (P, DPL, S, type).
    pub access: u8,
    /// High half of descriptor byte 6 in bit positions: bit3 G, bit2 D/B, bit1 L, bit0 AVL.
    pub flags: u8,
}

impl ProtectedSegment {
    /// Parses an 8-byte legacy segment descriptor (Intel SDM Vol. 3A section 3.4.5).
    /// This is a pure function: neither memory nor processor state is read or modified.
    /// It does not validate the S bit or the type: the caller must check S and the type
    /// itself (for example, system descriptors with S=0 are not segments of this kind).
    pub const fn from_descriptor(selector: u16, descriptor: [u8; 8]) -> Self {
        let base = (descriptor[2] as u32)
            | ((descriptor[3] as u32) << 8)
            | ((descriptor[4] as u32) << 16)
            | ((descriptor[7] as u32) << 24);
        let raw_limit = (descriptor[0] as u32)
            | ((descriptor[1] as u32) << 8)
            | (((descriptor[6] & 0x0f) as u32) << 16);
        let flags = descriptor[6] >> 4;
        let limit = if flags & 0x8 != 0 {
            (raw_limit << 12) | 0xFFF
        } else {
            raw_limit
        };
        Self {
            selector,
            base,
            limit,
            access: descriptor[5],
            flags,
        }
    }

    /// Default operand size selected by the D/B bit (bit 2 of `flags`): 32 bits when set,
    /// 16 bits otherwise.
    pub const fn default_operand_size(&self) -> OperandSize {
        if self.flags & 0x4 != 0 {
            OperandSize::Dword
        } else {
            OperandSize::Word
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_state_matches_table_12_1() {
        let s = RealModeState::after_init(0);
        assert_eq!(s.cs.selector, 0xF000);
        assert_eq!(s.cs.base, 0xFFFF_0000);
        assert_eq!(s.cs.limit, 0xFFFF);
        assert_eq!(s.eip, 0xFFF0);
        assert_eq!(s.eflags, 2);
        assert_eq!(s.fetch_linear_address(), 0xFFFF_FFF0);
        for seg in [s.ds, s.es, s.ss, s.fs, s.gs] {
            assert_eq!(
                seg,
                RealSegment {
                    selector: 0,
                    base: 0,
                    limit: 0xFFFF
                }
            );
        }
    }

    #[test]
    fn init_keeps_only_cd_and_nw_and_sets_et() {
        // All CR0 bits set: only CD (bit 30) and NW (bit 29) are preserved.
        assert_eq!(RealModeState::after_init(0xFFFF_FFFF).cr0, 0x6000_0010);
        // PE, PG, WP and the rest are cleared.
        assert_eq!(RealModeState::after_init(0x8005_003B).cr0, 0x0000_0010);
        assert_eq!(RealModeState::after_init(0).cr0, 0x10);
    }

    #[test]
    fn sipi_changes_only_cs_and_eip() {
        // Non-default starting state, so that unchanged fields cannot match by accident.
        let start = RealModeState {
            cs: RealSegment {
                selector: 0xF000,
                base: 0xFFFF_0000,
                limit: 0x1234,
            },
            ds: RealSegment {
                selector: 0x1234,
                base: 0x12340,
                limit: 0x00FF,
            },
            eflags: 0x0000_0202,
            ..RealModeState::after_init(0x6000_0031)
        };
        let s = start.after_sipi(0x9F);
        let expected = RealModeState {
            cs: RealSegment {
                selector: 0x9F00,
                base: 0x9F000,
                limit: 0x1234,
            },
            ds: RealSegment {
                selector: 0x1234,
                base: 0x12340,
                limit: 0x00FF,
            },
            es: RealSegment {
                selector: 0,
                base: 0,
                limit: 0xFFFF,
            },
            ss: RealSegment {
                selector: 0,
                base: 0,
                limit: 0xFFFF,
            },
            fs: RealSegment {
                selector: 0,
                base: 0,
                limit: 0xFFFF,
            },
            gs: RealSegment {
                selector: 0,
                base: 0,
                limit: 0xFFFF,
            },
            eip: 0,
            eflags: 0x0000_0202,
            cr0: 0x6000_0010,
        };
        assert_eq!(s, expected);
    }

    #[test]
    fn sipi_start_address_is_vector_shifted_by_twelve() {
        let init = RealModeState::after_init(0);
        let cases: [(u8, u32); 5] = [
            (0x00, 0),
            (0x01, 0x1000),
            (0x10, 0x10000),
            (0x9F, 0x9F000),
            (0xFF, 0xFF000),
        ];
        for (vector, expected) in cases {
            let s = init.after_sipi(vector);
            assert_eq!(s.fetch_linear_address(), expected, "vector {vector:#x}");
        }
    }

    #[test]
    fn real_mode_default_operand_size_is_16_bit() {
        assert_eq!(REAL_MODE_DEFAULT_OPERAND_SIZE.bytes(), 2);
    }

    #[test]
    fn flat_code_and_data_descriptors_decode_to_full_4g_segments() {
        // Classic flat descriptors from a 32-bit loader: G=1, D=1.
        let code = ProtectedSegment::from_descriptor(0x08, [0xFF, 0xFF, 0, 0, 0, 0x9A, 0xCF, 0]);
        assert_eq!(
            code,
            ProtectedSegment {
                selector: 0x08,
                base: 0,
                limit: 0xFFFF_FFFF,
                access: 0x9A,
                flags: 0xC
            }
        );
        let data = ProtectedSegment::from_descriptor(0x10, [0xFF, 0xFF, 0, 0, 0, 0x92, 0xCF, 0]);
        assert_eq!(
            data,
            ProtectedSegment {
                selector: 0x10,
                base: 0,
                limit: 0xFFFF_FFFF,
                access: 0x92,
                flags: 0xC
            }
        );
    }

    #[test]
    fn descriptor_base_and_byte_granular_limit() {
        // Base 0x1234_5678 (bytes 2, 3, 4 and 7). Limit: bytes 0-1 give 0xABCD, the low
        // nibble of byte 6 (0xA) gives bits 16-19; G=0, so the limit is in bytes: 0x000A_ABCD.
        let d = [0xCD, 0xAB, 0x78, 0x56, 0x34, 0x93, 0x0A, 0x12];
        let s = ProtectedSegment::from_descriptor(0x18, d);
        assert_eq!(s.base, 0x1234_5678);
        assert_eq!(s.limit, 0x000A_ABCD);
        assert_eq!(s.access, 0x93);
        assert_eq!(s.flags, 0x0);
    }

    #[test]
    fn granularity_bit_scales_limit_to_pages() {
        // Byte 6 = 0x80: G=1, D=0, limit bits 16-19 = 0. Raw limit 0x00001 is one page, so
        // the byte limit is 2 pages minus one byte: 0x1FFF.
        let g1 = ProtectedSegment::from_descriptor(0x20, [0x01, 0x00, 0, 0, 0, 0x92, 0x80, 0]);
        assert_eq!(g1.limit, 0x1FFF);
        assert_eq!(g1.flags, 0x8);

        // Byte 6 = 0x4F: G=0, D=1, limit bits 16-19 = 0xF. Raw limit 0xFFFFF stays in bytes.
        let g0 = ProtectedSegment::from_descriptor(0x28, [0xFF, 0xFF, 0, 0, 0, 0x92, 0x4F, 0]);
        assert_eq!(g0.limit, 0xFFFFF);
        assert_eq!(g0.flags, 0x4);
    }

    #[test]
    fn db_bit_selects_default_operand_size() {
        // Flat code segment, D=1: 32-bit default operand size.
        let code = ProtectedSegment::from_descriptor(0x08, [0xFF, 0xFF, 0, 0, 0, 0x9A, 0xCF, 0]);
        assert_eq!(code.default_operand_size(), OperandSize::Dword);
        // Byte 6 = 0x8F: G=1, D=0: 16-bit default operand size.
        let seg16 = ProtectedSegment::from_descriptor(0x30, [0xFF, 0xFF, 0, 0, 0, 0x9A, 0x8F, 0]);
        assert_eq!(seg16.default_operand_size(), OperandSize::Word);
    }
}
