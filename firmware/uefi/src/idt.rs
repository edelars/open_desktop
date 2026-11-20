//! Encoding of the firmware's 64-bit interrupt gates.

const VECTOR_COUNT: usize = 256;
const STUB_BYTES: u64 = 10;
const CODE_SELECTOR: u64 = 0x08;
const PRESENT_INTERRUPT_GATE: u64 = 0x8e;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
struct Gate {
    low: u64,
    high: u64,
}

impl Gate {
    const EMPTY: Self = Self { low: 0, high: 0 };

    const fn interrupt(target: u64) -> Self {
        Self {
            low: (target & 0xffff)
                | (CODE_SELECTOR << 16)
                | (PRESENT_INTERRUPT_GATE << 40)
                | ((target & 0xffff_0000) << 32),
            high: target >> 32,
        }
    }
}

#[repr(C, align(16))]
pub struct Idt([Gate; VECTOR_COUNT]);

impl Idt {
    pub const EMPTY: Self = Self([Gate::EMPTY; VECTOR_COUNT]);
    pub const LIMIT: u16 = (core::mem::size_of::<Self>() - 1) as u16;

    /// The linked handlers are 256 contiguous fixed-width stubs.
    pub fn fill(&mut self, stubs: u64) {
        for (vector, gate) in self.0.iter_mut().enumerate() {
            *gate = Gate::interrupt(stubs + vector as u64 * STUB_BYTES);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_vector_has_a_ring_zero_interrupt_gate_to_its_own_stub() {
        let mut idt = Idt::EMPTY;
        let start = 0xffff_8000_abcd_7ffe;
        idt.fill(start);
        assert_eq!(Idt::LIMIT, 0xfff);
        assert_eq!(core::mem::size_of::<Gate>(), 16);
        assert_eq!(core::mem::align_of::<Idt>(), 16);
        for (vector, gate) in idt.0.iter().enumerate() {
            assert_eq!(gate.low & 0x0000_ffff_ffff_0000, 0x0000_8e00_0008_0000);
            assert_eq!(gate.high >> 32, 0);
            let target = (gate.low & 0xffff) | ((gate.low >> 32) & 0xffff_0000) | (gate.high << 32);
            assert_eq!(target, start + vector as u64 * 10);
        }
    }
}
