//! PERF-20: the histogram of retired operations of the unified loop.
//!
//! A diagnostic that only observes: before each instruction the loop decodes it with a read-only
//! fetch (no accessed/dirty bit, RAM only, nothing of the guest or of the caches changes), and
//! after the step, when the instruction retired, it counts it under the key
//! `Operation` variant + kind of the operands (register, memory, none) + operand size + CPL.
//! It is not a time profile: a count says how often a form executes, not what it costs.
//!
//! The mode is on only when the board was asked for it (`--op-histogram-out` of the runner): the
//! loop takes it as a constant generic parameter, so an ordinary run has no check in the hot loop.

use od_isa::long_decode::{Instruction, Operation};
use std::collections::HashMap;

/// Direct-mapped table of the instructions seen at a RIP (a power of two).
const SITE_BITS: u32 = 16;
const SITE_COUNT: usize = 1 << SITE_BITS;
/// The class of an instruction the read-only decoder could not decode (it still retired).
const UNDECODED: u32 = 0;

/// One counted form: the key of the histogram and how many instructions of it retired.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpHistogramEntry {
    /// The variant of `od_isa::long_decode::Operation` (`<undecoded>` for a retired instruction
    /// the read-only decode did not decode).
    pub operation: String,
    /// `mem` when the operation has a memory operand, `reg` when it has register operands only,
    /// `none` when it has no explicit operand.
    pub operands: &'static str,
    /// The operand size of the instruction in bytes.
    pub operand_bytes: u8,
    /// The privilege level (the RPL of CS) at the instruction.
    pub cpl: u8,
    pub count: u64,
}

/// The histogram of one run: the entries by descending count and their sums.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OpHistogramReport {
    /// By descending `count`; equal counts by operation, operands, size, CPL.
    pub entries: Vec<OpHistogramEntry>,
    /// The sum of every count: equals the instructions retired in the observed loop.
    pub total: u64,
    /// The sum of the counts by CPL (index = CPL).
    pub by_cpl: [u64; 4],
}

#[derive(Debug)]
struct Site {
    rip: u64,
    instruction: Instruction,
    class: u32,
}

#[derive(Debug)]
struct Class {
    operation: String,
    operands: &'static str,
    operand_bytes: u8,
}

/// The counters. `sites` skips the classification (a text of the decoded operation) for an
/// instruction already seen at the same RIP: the stored instruction is compared whole, so changed
/// code is classified again.
#[derive(Debug)]
pub(crate) struct OpHistogram {
    sites: Vec<Option<Site>>,
    classes: Vec<Class>,
    index: HashMap<(String, &'static str, u8), u32>,
    counts: Vec<[u64; 4]>,
}

impl OpHistogram {
    pub(crate) fn new() -> Self {
        let mut sites = Vec::new();
        sites.resize_with(SITE_COUNT, || None);
        Self {
            sites,
            classes: vec![Class {
                operation: "<undecoded>".to_owned(),
                operands: "none",
                operand_bytes: 0,
            }],
            index: HashMap::new(),
            counts: vec![[0; 4]],
        }
    }

    /// `retired` instructions (the step's count, one) of `instruction` (decoded before the step;
    /// `None` when the read-only decode failed) retired at `rip` in `cpl`.
    pub(crate) fn record(
        &mut self,
        rip: u64,
        instruction: Option<Instruction>,
        cpl: u8,
        retired: u64,
    ) {
        let class = match instruction {
            Some(instruction) => self.class_of(rip, instruction),
            None => UNDECODED,
        };
        self.counts[class as usize][usize::from(cpl & 3)] += retired;
    }

    fn class_of(&mut self, rip: u64, instruction: Instruction) -> u32 {
        let slot = (rip.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> (64 - SITE_BITS)) as usize;
        if let Some(site) = &self.sites[slot]
            && site.rip == rip
            && site.instruction == instruction
        {
            return site.class;
        }
        let (operation, operands, operand_bytes) = classify(&instruction);
        let next = self.classes.len() as u32;
        let class = *self
            .index
            .entry((operation.clone(), operands, operand_bytes))
            .or_insert(next);
        if class == next {
            self.classes.push(Class {
                operation,
                operands,
                operand_bytes,
            });
            self.counts.push([0; 4]);
        }
        self.sites[slot] = Some(Site {
            rip,
            instruction,
            class,
        });
        class
    }

    pub(crate) fn report(&self) -> OpHistogramReport {
        let mut entries = Vec::new();
        let mut by_cpl = [0u64; 4];
        for (class, counts) in self.classes.iter().zip(&self.counts) {
            for (cpl, &count) in counts.iter().enumerate() {
                by_cpl[cpl] += count;
                if count > 0 {
                    entries.push(OpHistogramEntry {
                        operation: class.operation.clone(),
                        operands: class.operands,
                        operand_bytes: class.operand_bytes,
                        cpl: cpl as u8,
                        count,
                    });
                }
            }
        }
        entries.sort_by(|a, b| {
            b.count.cmp(&a.count).then_with(|| {
                (&a.operation, a.operands, a.operand_bytes, a.cpl).cmp(&(
                    &b.operation,
                    b.operands,
                    b.operand_bytes,
                    b.cpl,
                ))
            })
        });
        OpHistogramReport {
            total: by_cpl.iter().sum(),
            entries,
            by_cpl,
        }
    }
}

/// The variant name, the kind of the operands and the operand size of a decoded instruction. The
/// variant and the kind are read from the `Debug` text of the operation, which names the variant
/// first and every operand by its type: `MemoryOperand` is a memory operand, `RegisterView` a
/// register. The FS/GS forms (`MovFsLoad`, `AddGsMem`, ...) carry an offset instead of a
/// `MemoryOperand` and are memory by their name. Runs once per distinct instruction at a RIP.
///
/// The text rule does not see an operand that is a plain `Register` or a SSE/x87 register index
/// (`u8`), or a memory operand that is implied by the variant (`...MemoryRax`, `F*M32...Rax`):
/// an explicit match and the shape of the name classify those before the text rule. Known limits:
/// `Movs`/`Stos`/`Lods`/`Scas`/`Cmps` read memory implicitly and stay `none`; `Lea` carries a
/// `MemoryOperand` and counts as `mem` though it does not access memory.
fn classify(instruction: &Instruction) -> (String, &'static str, u8) {
    let operand_bytes = instruction.operand_size.bytes() as u8;
    // PUSH/POP of a register: one variant, the direction is a field; the key names the direction.
    if let Operation::StackRegister { push, .. } = &instruction.operation {
        let name = if *push {
            "StackRegisterPush"
        } else {
            "StackRegisterPop"
        };
        return (name.to_owned(), "reg", operand_bytes);
    }
    let text = format!("{:?}", instruction.operation);
    let end = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(text.len());
    let name = &text[..end];
    if let Some(operands) = classify_by_shape(&instruction.operation, name) {
        return (name.to_owned(), operands, operand_bytes);
    }
    let segment_memory = ["FsLoad", "GsLoad", "FsMem", "GsMem", "FsStore", "GsStore"]
        .iter()
        .any(|part| name.contains(part));
    let operands = if segment_memory || text.contains("MemoryOperand") {
        "mem"
    } else if text.contains("RegisterView") {
        "reg"
    } else {
        "none"
    };
    (name.to_owned(), operands, operand_bytes)
}

/// Operand kind of the variants whose operands the `Debug` text does not show: a `Register` field
/// or a register index is `reg`; a memory operand implied by the variant (address in RAX/RDI) is
/// `mem`. `None` leaves the variant to the text rule.
fn classify_by_shape(operation: &Operation, name: &str) -> Option<&'static str> {
    if matches!(
        operation,
        Operation::LoadTr { .. }
            | Operation::LoadLdt { .. }
            | Operation::WriteDebugRegister { .. }
            | Operation::LoadFsSelectorReg { .. }
            | Operation::LoadGsSelectorReg { .. }
            | Operation::LoadSegmentSelectorReg { .. }
            | Operation::ReadCr2Into(_)
            | Operation::ReadCr3Into(_)
            | Operation::ReadCr4Into(_)
            | Operation::WriteCr3From(_)
            | Operation::WriteCr4From(_)
            | Operation::ReadControl { .. }
            | Operation::WriteControl { .. }
            | Operation::ReadDebugRegister { .. }
    ) {
        return Some("reg");
    }
    // SSE/MMX forms with register indices: `AddpdRegister`, `MinssRegisterMasked`, `Psll*ImmediateRegister`.
    if name.ends_with("Register") || name.ends_with("RegisterMasked") {
        return Some("reg");
    }
    // The address is implied by the variant: `...MemoryRax`, x87 `F*M16/M32/M64/M80...Rax`,
    // `Fnstenv/Fnsave/Frstor/FldenvRax`, `WriteCr0Rdi`, `Fxsave64RaxDisp8`.
    let x87_sized = ["M16", "M32", "M64", "M80"]
        .iter()
        .any(|size| name.contains(size))
        && name.ends_with("Rax");
    let x87_environment = ["Fnstenv", "Fnsave", "Frstor", "Fldenv"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
        && name.ends_with("Rax");
    if name.ends_with("MemoryRax")
        || x87_sized
        || x87_environment
        || name == "WriteCr0Rdi"
        || name.starts_with("Fxsave64Rax")
    {
        return Some("mem");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use od_isa::long_decode::decode;
    use od_isa::memory::{AccessContext, AccessKind, GuestMemory, MemoryError};

    struct Bytes(Vec<u8>);

    impl GuestMemory for Bytes {
        fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
            self.0
                .get(address as usize)
                .copied()
                .ok_or(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Fetch,
                })
        }
        fn read(&mut self, _: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
            Err(MemoryError::Backend("read-only".into()))
        }
        fn write(&mut self, _: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
            Err(MemoryError::Backend("read-only".into()))
        }
        fn atomic_exchange_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: od_isa::OperandSize,
            _: u64,
        ) -> Result<u64, MemoryError> {
            Err(MemoryError::Backend("read-only".into()))
        }
        fn atomic_fetch_add_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: od_isa::OperandSize,
            _: u64,
        ) -> Result<u64, MemoryError> {
            Err(MemoryError::Backend("read-only".into()))
        }
        fn atomic_compare_exchange_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: od_isa::OperandSize,
            _: u64,
            _: u64,
        ) -> Result<u64, MemoryError> {
            Err(MemoryError::Backend("read-only".into()))
        }
        fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
            Err(MemoryError::Backend("read-only".into()))
        }
    }

    fn decoded(bytes: &[u8]) -> Instruction {
        decode(&mut Bytes(bytes.to_vec()), 0, AccessContext::default()).expect("decodes")
    }

    #[test]
    fn classes_split_by_operand_kind_size_and_cpl_and_sum_to_the_records() {
        let mov_load = decoded(&[0x48, 0x8B, 0x03]); // mov rax, [rbx]
        let mov_reg = decoded(&[0x48, 0x89, 0xD8]); // mov rax, rbx
        let nop = decoded(&[0x90]);
        let (name, operands, bytes) = classify(&mov_load);
        assert_eq!((name.as_str(), operands, bytes), ("Mov", "mem", 8));
        assert_eq!(classify(&mov_reg).1, "reg");
        let mut histogram = OpHistogram::new();
        for _ in 0..3 {
            histogram.record(0x1000, Some(mov_load), 0, 1);
        }
        histogram.record(0x2000, Some(mov_reg), 0, 1);
        histogram.record(0x3000, Some(nop), 3, 1);
        histogram.record(0x1000, Some(mov_load), 3, 1);
        histogram.record(0x4000, None, 0, 1);
        let report = histogram.report();
        assert_eq!(report.total, 7);
        assert_eq!(report.by_cpl, [5, 0, 0, 2]);
        assert_eq!(report.entries.iter().map(|e| e.count).sum::<u64>(), 7);
        assert_eq!(report.entries[0].count, 3);
        assert_eq!(
            (
                report.entries[0].operation.as_str(),
                report.entries[0].operands,
                report.entries[0].cpl
            ),
            ("Mov", "mem", 0)
        );
        assert!(
            report
                .entries
                .windows(2)
                .all(|pair| pair[0].count >= pair[1].count)
        );
    }

    #[test]
    fn push_pop_sse_register_and_fs_forms_are_classified_by_shape() {
        let push = classify(&decoded(&[0x50])); // push rax
        assert_eq!((push.0.as_str(), push.1), ("StackRegisterPush", "reg"));
        let pop = classify(&decoded(&[0x58])); // pop rax
        assert_eq!((pop.0.as_str(), pop.1), ("StackRegisterPop", "reg"));
        let movaps = classify(&decoded(&[0x0F, 0x28, 0xC1])); // movaps xmm0, xmm1
        assert_eq!((movaps.0.as_str(), movaps.1), ("MovapsRegister", "reg"));
        // mov rax, fs:[0x28]
        let fs = classify(&decoded(&[0x64, 0x48, 0x8B, 0x04, 0x25, 0x28, 0, 0, 0]));
        assert!(fs.0.contains("Fs"), "{fs:?}");
        assert_eq!(fs.1, "mem");
        assert_eq!(classify(&decoded(&[0x90])).1, "none"); // nop
    }

    #[test]
    fn changed_code_at_the_same_rip_is_classified_again() {
        let mut histogram = OpHistogram::new();
        histogram.record(0x1000, Some(decoded(&[0x48, 0x8B, 0x03])), 0, 1);
        histogram.record(0x1000, Some(decoded(&[0x48, 0x89, 0xD8])), 0, 1);
        let report = histogram.report();
        assert_eq!(report.entries.len(), 2);
        assert_eq!(report.total, 2);
    }
}
