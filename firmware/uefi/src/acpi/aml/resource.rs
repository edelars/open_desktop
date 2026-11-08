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
use crate::acpi::bytes::{copy_at, tri};

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

    const fn byte(self) -> u8 {
        ((self.max_fixed as u8) << 3)
            | ((self.min_fixed as u8) << 2)
            | ((self.subtractive as u8) << 1)
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
    const fn byte(self) -> Result<u8, AmlError> {
        if self.cacheability > 3 {
            return Err(AmlError::BadResource);
        }
        Ok((self.cacheability << 1) | self.read_write as u8)
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

impl Default for ResourceTemplate {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceTemplate {
    /// An empty template without the End Tag; describe it with the descriptor methods and
    /// close it with [`finish`](Self::finish). This is the `const fn` form of
    /// [`build`](Self::build).
    pub const fn new() -> Self {
        ResourceTemplate {
            buf: [0; RESOURCE_TEMPLATE_CAPACITY],
            len: 0,
        }
    }

    /// Appends the End Tag (Table 6.37): small item name 0xF, length 1 -> 0x79, then the
    /// checksum byte.
    pub const fn finish(mut self) -> Result<Self, AmlError> {
        tri!(self.push(&[RES_END_TAG, 0x00]));
        Ok(self)
    }

    /// Runs `describe` on an empty template, then appends the End Tag.
    pub fn build(
        describe: impl FnOnce(&mut ResourceTemplate) -> Result<(), AmlError>,
    ) -> Result<Self, AmlError> {
        let mut template = ResourceTemplate::new();
        describe(&mut template)?;
        template.finish()
    }

    pub const fn bytes(&self) -> &[u8] {
        self.buf.split_at(self.len).0
    }

    const fn push(&mut self, bytes: &[u8]) -> Result<(), AmlError> {
        let end = self.len + bytes.len();
        if end > self.buf.len() {
            return Err(AmlError::BadResource);
        }
        copy_at(&mut self.buf, self.len, bytes);
        self.len = end;
        Ok(())
    }

    /// `Memory32Fixed(ReadAndWrite, base, length)`: 0x86, length 9, information byte (bit 0
    /// `_RW`), `_BAS` (4), `_LEN` (4) (Table 6.43).
    pub const fn memory32_fixed(
        &mut self,
        read_write: bool,
        base: u32,
        length: u32,
    ) -> Result<(), AmlError> {
        let mut d = [0u8; 12];
        d[0] = RES_MEMORY32_FIXED;
        copy_at(&mut d, 1, &9u16.to_le_bytes());
        d[3] = read_write as u8;
        copy_at(&mut d, 4, &base.to_le_bytes());
        copy_at(&mut d, 8, &length.to_le_bytes());
        self.push(&d)
    }

    /// `IO(Decode16/Decode10, min, max, align, length)`: 0x47, information byte (bit 0 `_DEC`),
    /// `_MIN` (2), `_MAX` (2), `_ALN` (1), `_LEN` (1) (Table 6.33).
    pub const fn io(
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
        d[1] = decode16 as u8;
        copy_at(&mut d, 2, &min.to_le_bytes());
        copy_at(&mut d, 4, &max.to_le_bytes());
        d[6] = align;
        d[7] = length;
        self.push(&d)
    }

    /// `IRQ(...) {list}`: 0x22 (mask only, edge/active-high implied) or 0x23 (mask + information
    /// byte: bit 0 `_HE`, bit 3 `_LL`, bit 4 `_SHR`, bit 5 `_WKC`) (Table 6.28). `irqs` are
    /// ISA IRQ numbers 0..=15; `flags = None` writes the short form.
    pub const fn irq(
        &mut self,
        irqs: &[u8],
        flags: Option<InterruptFlags>,
    ) -> Result<(), AmlError> {
        let mut mask = 0u16;
        let mut i = 0;
        while i < irqs.len() {
            let irq = irqs[i];
            if irq > 15 {
                return Err(AmlError::BadResource);
            }
            mask |= 1 << irq;
            i += 1;
        }
        if mask == 0 {
            return Err(AmlError::BadResource);
        }
        let [lo, hi] = mask.to_le_bytes();
        match flags {
            None => self.push(&[RES_IRQ_NO_INFO, lo, hi]),
            Some(f) => {
                let info = (f.edge as u8)
                    | ((f.active_low as u8) << 3)
                    | ((f.shared as u8) << 4)
                    | ((f.wake as u8) << 5);
                self.push(&[RES_IRQ_WITH_INFO, lo, hi, info])
            }
        }
    }

    /// `Interrupt(ResourceConsumer/Producer, ...) {list}`: 0x89, length `2 + 4 n`, flags byte
    /// (bit 0 consumer, bit 1 `_HE`, bit 2 `_LL`, bit 3 `_SHR`, bit 4 `_WKC`), count, `n`
    /// interrupt numbers of 4 bytes (Table 6.52). No Resource Source is written.
    pub const fn extended_interrupt(
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
        copy_at(&mut d, 1, &(length as u16).to_le_bytes());
        d[3] = (consumer as u8)
            | ((flags.edge as u8) << 1)
            | ((flags.active_low as u8) << 2)
            | ((flags.shared as u8) << 3)
            | ((flags.wake as u8) << 4);
        d[4] = interrupts.len() as u8;
        let mut i = 0;
        while i < interrupts.len() {
            copy_at(&mut d, 5 + 4 * i, &interrupts[i].to_le_bytes());
            i += 1;
        }
        self.push(d.split_at(3 + length).0)
    }

    /// `WordBusNumber(...)`: 0x88, length 13, resource type 2, general flags, type-specific
    /// flags (written 0, NOT VERIFIED for bus numbers), `_GRA`, `_MIN`, `_MAX`, `_TRA`, `_LEN`
    /// as 16-bit values (Table 6.47).
    pub const fn word_bus_number(
        &mut self,
        flags: SpaceFlags,
        range: AddressRange<u16>,
    ) -> Result<(), AmlError> {
        if range.min > range.max {
            return Err(AmlError::BadResource);
        }
        let mut d = [0u8; 16];
        d[0] = RES_WORD_SPACE;
        copy_at(&mut d, 1, &13u16.to_le_bytes());
        d[3] = SPACE_TYPE_BUS_NUMBER;
        d[4] = flags.byte();
        d[5] = 0;
        copy_at(&mut d, 6, &range.granularity.to_le_bytes());
        copy_at(&mut d, 8, &range.min.to_le_bytes());
        copy_at(&mut d, 10, &range.max.to_le_bytes());
        copy_at(&mut d, 12, &range.translation.to_le_bytes());
        copy_at(&mut d, 14, &range.length.to_le_bytes());
        self.push(&d)
    }

    /// `DWordMemory(...)`: 0x87, length 23, resource type 0, general flags, memory flags,
    /// `_GRA`, `_MIN`, `_MAX`, `_TRA`, `_LEN` as 32-bit values (Tables 6.46 and 6.49).
    pub const fn dword_memory(
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
        copy_at(&mut d, 1, &23u16.to_le_bytes());
        d[3] = SPACE_TYPE_MEMORY;
        d[4] = flags.byte();
        d[5] = tri!(memory.byte());
        copy_at(&mut d, 6, &range.granularity.to_le_bytes());
        copy_at(&mut d, 10, &range.min.to_le_bytes());
        copy_at(&mut d, 14, &range.max.to_le_bytes());
        copy_at(&mut d, 18, &range.translation.to_le_bytes());
        copy_at(&mut d, 22, &range.length.to_le_bytes());
        self.push(&d)
    }

    /// `QWordMemory(...)`: 0x8A, length 43, same field order as `DWordMemory` with 64-bit
    /// values. The QWord layout is NOT VERIFIED directly (see `opcodes::RES_QWORD_SPACE`).
    pub const fn qword_memory(
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
        copy_at(&mut d, 1, &43u16.to_le_bytes());
        d[3] = SPACE_TYPE_MEMORY;
        d[4] = flags.byte();
        d[5] = tri!(memory.byte());
        copy_at(&mut d, 6, &range.granularity.to_le_bytes());
        copy_at(&mut d, 14, &range.min.to_le_bytes());
        copy_at(&mut d, 22, &range.max.to_le_bytes());
        copy_at(&mut d, 30, &range.translation.to_le_bytes());
        copy_at(&mut d, 38, &range.length.to_le_bytes());
        self.push(&d)
    }
}
