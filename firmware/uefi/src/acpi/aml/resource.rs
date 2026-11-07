//! ResourceTemplate descriptors (ACPI 6.4 "Resource Data Types for ACPI") encoded into a small
//! fixed buffer; the finished template is placed into AML as a Buffer by the builder.
//!
//! Like the ASL `ResourceTemplate` macro, [`ResourceTemplate::build`] appends the End Tag
//! (Table 6.37) itself. The End Tag checksum is written as 0, which Table 6.37 defines as "the
//! checksum operation succeeded"; how the ASL macro derives a non-zero checksum is NOT VERIFIED.

use super::AmlError;
use super::opcodes::{
    RES_DWORD_SPACE, RES_END_TAG, RES_EXTENDED_INTERRUPT, RES_IO_PORT, RES_IRQ_NO_INFO,
    RES_IRQ_WITH_INFO, RES_MEMORY32_FIXED, RES_QWORD_SPACE, RES_WORD_SPACE, SPACE_TYPE_BUS_NUMBER,
    SPACE_TYPE_MEMORY,
};

/// Capacity of one template, End Tag included.
pub const RESOURCE_TEMPLATE_CAPACITY: usize = 256;
/// Most interrupt numbers in one Extended Interrupt descriptor.
pub const MAX_EXTENDED_INTERRUPTS: usize = 8;

/// General flags shared by the Word/DWord/QWord address space descriptors (Table 6.46,
/// "General Flags"): bit 3 `_MAF`, bit 2 `_MIF`, bit 1 `_DEC`. Bit 0 is "Ignored" in the table
/// and is always written as 0 (ResourceProducer); the ASL ResourceUsage mapping is NOT VERIFIED.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpaceFlags {
    pub min_fixed: bool,
    pub max_fixed: bool,
    /// Subtractive decode (`_DEC = 1`); top-level bridges only.
    pub subtractive: bool,
}

impl SpaceFlags {
    pub const FIXED: SpaceFlags = SpaceFlags {
        min_fixed: true,
        max_fixed: true,
        subtractive: false,
    };

    fn byte(self) -> u8 {
        (u8::from(self.max_fixed) << 3)
            | (u8::from(self.min_fixed) << 2)
            | (u8::from(self.subtractive) << 1)
    }
}

/// Memory type-specific flags (Table 6.49): bits 2..1 `_MEM`, bit 0 `_RW`. The `_MTP` bits 4..3
/// and the translation bit 5 are written as 0 (AddressRangeMemory, TypeStatic).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryFlags {
    /// `_MEM`: 0 non-cacheable, 1 cacheable, 2 cacheable + write combining, 3 prefetchable.
    pub cacheability: u8,
    pub read_write: bool,
}

impl MemoryFlags {
    fn byte(self) -> Result<u8, AmlError> {
        if self.cacheability > 3 {
            return Err(AmlError::BadResource);
        }
        Ok((self.cacheability << 1) | u8::from(self.read_write))
    }
}

/// Interrupt attributes of the IRQ and Extended Interrupt descriptors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterruptFlags {
    /// `_HE`: true = edge-triggered, false = level-triggered.
    pub edge: bool,
    /// `_LL`: true = active-low, false = active-high.
    pub active_low: bool,
    /// `_SHR`.
    pub shared: bool,
    /// `_WKC`.
    pub wake: bool,
}

/// Granularity, range, translation and length of an address space descriptor, in the width of
/// the descriptor (`u16` Word, `u32` DWord, `u64` QWord). Names follow `_GRA/_MIN/_MAX/_TRA/_LEN`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddressRange<T> {
    pub granularity: T,
    pub min: T,
    pub max: T,
    pub translation: T,
    pub length: T,
}

pub struct ResourceTemplate {
    buf: [u8; RESOURCE_TEMPLATE_CAPACITY],
    len: usize,
}

impl ResourceTemplate {
    /// Runs `describe` on an empty template, then appends the End Tag.
    pub fn build(
        describe: impl FnOnce(&mut ResourceTemplate) -> Result<(), AmlError>,
    ) -> Result<Self, AmlError> {
        let mut template = ResourceTemplate {
            buf: [0; RESOURCE_TEMPLATE_CAPACITY],
            len: 0,
        };
        describe(&mut template)?;
        // End Tag: small item name 0xF, length 1 -> 0x79, then the checksum byte.
        template.push(&[RES_END_TAG, 0x00])?;
        Ok(template)
    }

    pub fn bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    fn push(&mut self, bytes: &[u8]) -> Result<(), AmlError> {
        let end = self.len + bytes.len();
        let dst = self
            .buf
            .get_mut(self.len..end)
            .ok_or(AmlError::BadResource)?;
        dst.copy_from_slice(bytes);
        self.len = end;
        Ok(())
    }

    /// `Memory32Fixed(ReadAndWrite, base, length)`: 0x86, length 9, information byte (bit 0
    /// `_RW`), `_BAS` (4), `_LEN` (4) (Table 6.43).
    pub fn memory32_fixed(
        &mut self,
        read_write: bool,
        base: u32,
        length: u32,
    ) -> Result<(), AmlError> {
        let mut d = [0u8; 12];
        d[0] = RES_MEMORY32_FIXED;
        d[1..3].copy_from_slice(&9u16.to_le_bytes());
        d[3] = u8::from(read_write);
        d[4..8].copy_from_slice(&base.to_le_bytes());
        d[8..12].copy_from_slice(&length.to_le_bytes());
        self.push(&d)
    }

    /// `IO(Decode16/Decode10, min, max, align, length)`: 0x47, information byte (bit 0 `_DEC`),
    /// `_MIN` (2), `_MAX` (2), `_ALN` (1), `_LEN` (1) (Table 6.33).
    pub fn io(
        &mut self,
        decode16: bool,
        min: u16,
        max: u16,
        align: u8,
        length: u8,
    ) -> Result<(), AmlError> {
        if min > max {
            return Err(AmlError::BadResource);
        }
        let mut d = [0u8; 8];
        d[0] = RES_IO_PORT;
        d[1] = u8::from(decode16);
        d[2..4].copy_from_slice(&min.to_le_bytes());
        d[4..6].copy_from_slice(&max.to_le_bytes());
        d[6] = align;
        d[7] = length;
        self.push(&d)
    }

    /// `IRQ(...) {list}`: 0x22 (mask only, edge/active-high implied) or 0x23 (mask + information
    /// byte: bit 0 `_HE`, bit 3 `_LL`, bit 4 `_SHR`, bit 5 `_WKC`) (Table 6.28). `irqs` are
    /// ISA IRQ numbers 0..=15; `flags = None` writes the short form.
    pub fn irq(&mut self, irqs: &[u8], flags: Option<InterruptFlags>) -> Result<(), AmlError> {
        let mut mask = 0u16;
        for &irq in irqs {
            if irq > 15 {
                return Err(AmlError::BadResource);
            }
            mask |= 1 << irq;
        }
        if mask == 0 {
            return Err(AmlError::BadResource);
        }
        let [lo, hi] = mask.to_le_bytes();
        match flags {
            None => self.push(&[RES_IRQ_NO_INFO, lo, hi]),
            Some(f) => {
                let info = u8::from(f.edge)
                    | (u8::from(f.active_low) << 3)
                    | (u8::from(f.shared) << 4)
                    | (u8::from(f.wake) << 5);
                self.push(&[RES_IRQ_WITH_INFO, lo, hi, info])
            }
        }
    }

    /// `Interrupt(ResourceConsumer/Producer, ...) {list}`: 0x89, length `2 + 4 n`, flags byte
    /// (bit 0 consumer, bit 1 `_HE`, bit 2 `_LL`, bit 3 `_SHR`, bit 4 `_WKC`), count, `n`
    /// interrupt numbers of 4 bytes (Table 6.52). No Resource Source is written.
    pub fn extended_interrupt(
        &mut self,
        consumer: bool,
        flags: InterruptFlags,
        interrupts: &[u32],
    ) -> Result<(), AmlError> {
        if interrupts.is_empty() || interrupts.len() > MAX_EXTENDED_INTERRUPTS {
            return Err(AmlError::BadResource);
        }
        let length = 2 + 4 * interrupts.len();
        let mut d = [0u8; 5 + 4 * MAX_EXTENDED_INTERRUPTS];
        d[0] = RES_EXTENDED_INTERRUPT;
        d[1..3].copy_from_slice(&(length as u16).to_le_bytes());
        d[3] = u8::from(consumer)
            | (u8::from(flags.edge) << 1)
            | (u8::from(flags.active_low) << 2)
            | (u8::from(flags.shared) << 3)
            | (u8::from(flags.wake) << 4);
        d[4] = interrupts.len() as u8;
        for (i, number) in interrupts.iter().enumerate() {
            d[5 + 4 * i..9 + 4 * i].copy_from_slice(&number.to_le_bytes());
        }
        self.push(&d[..3 + length])
    }

    /// `WordBusNumber(...)`: 0x88, length 13, resource type 2, general flags, type-specific
    /// flags (written 0, NOT VERIFIED for bus numbers), `_GRA`, `_MIN`, `_MAX`, `_TRA`, `_LEN`
    /// as 16-bit values (Table 6.47).
    pub fn word_bus_number(
        &mut self,
        flags: SpaceFlags,
        range: AddressRange<u16>,
    ) -> Result<(), AmlError> {
        if range.min > range.max {
            return Err(AmlError::BadResource);
        }
        let mut d = [0u8; 16];
        d[0] = RES_WORD_SPACE;
        d[1..3].copy_from_slice(&13u16.to_le_bytes());
        d[3] = SPACE_TYPE_BUS_NUMBER;
        d[4] = flags.byte();
        d[5] = 0;
        d[6..8].copy_from_slice(&range.granularity.to_le_bytes());
        d[8..10].copy_from_slice(&range.min.to_le_bytes());
        d[10..12].copy_from_slice(&range.max.to_le_bytes());
        d[12..14].copy_from_slice(&range.translation.to_le_bytes());
        d[14..16].copy_from_slice(&range.length.to_le_bytes());
        self.push(&d)
    }

    /// `DWordMemory(...)`: 0x87, length 23, resource type 0, general flags, memory flags,
    /// `_GRA`, `_MIN`, `_MAX`, `_TRA`, `_LEN` as 32-bit values (Tables 6.46 and 6.49).
    pub fn dword_memory(
        &mut self,
        flags: SpaceFlags,
        memory: MemoryFlags,
        range: AddressRange<u32>,
    ) -> Result<(), AmlError> {
        if range.min > range.max {
            return Err(AmlError::BadResource);
        }
        let mut d = [0u8; 26];
        d[0] = RES_DWORD_SPACE;
        d[1..3].copy_from_slice(&23u16.to_le_bytes());
        d[3] = SPACE_TYPE_MEMORY;
        d[4] = flags.byte();
        d[5] = memory.byte()?;
        d[6..10].copy_from_slice(&range.granularity.to_le_bytes());
        d[10..14].copy_from_slice(&range.min.to_le_bytes());
        d[14..18].copy_from_slice(&range.max.to_le_bytes());
        d[18..22].copy_from_slice(&range.translation.to_le_bytes());
        d[22..26].copy_from_slice(&range.length.to_le_bytes());
        self.push(&d)
    }

    /// `QWordMemory(...)`: 0x8A, length 43, same field order as `DWordMemory` with 64-bit
    /// values. The QWord layout is NOT VERIFIED directly (see `opcodes::RES_QWORD_SPACE`).
    pub fn qword_memory(
        &mut self,
        flags: SpaceFlags,
        memory: MemoryFlags,
        range: AddressRange<u64>,
    ) -> Result<(), AmlError> {
        if range.min > range.max {
            return Err(AmlError::BadResource);
        }
        let mut d = [0u8; 46];
        d[0] = RES_QWORD_SPACE;
        d[1..3].copy_from_slice(&43u16.to_le_bytes());
        d[3] = SPACE_TYPE_MEMORY;
        d[4] = flags.byte();
        d[5] = memory.byte()?;
        d[6..14].copy_from_slice(&range.granularity.to_le_bytes());
        d[14..22].copy_from_slice(&range.min.to_le_bytes());
        d[22..30].copy_from_slice(&range.max.to_le_bytes());
        d[30..38].copy_from_slice(&range.translation.to_le_bytes());
        d[38..46].copy_from_slice(&range.length.to_le_bytes());
        self.push(&d)
    }
}
