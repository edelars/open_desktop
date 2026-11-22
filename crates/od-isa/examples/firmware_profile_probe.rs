//! S7-31a diagnostic: which instruction encodings does the firmware paged profile decode?
//!
//! Reads one hex-encoded instruction per line from stdin and prints "<hex> <verdict>". The verdict
//! is "unsupported" when the firmware profile (`step_paged_firmware_probe`) returns
//! `PagedProbeExit::Unsupported`, and "decoded" for any other outcome (retired, or a fault caused
//! by the scratch register and memory state). It is a decode-level probe, not an execution test.
//! The third column is the name of the exit variant, so a caller can see what "decoded" contains.
use std::io::BufRead;

use od_isa::execute::{self, GuestFpState, PagedCpl0FirmwareBootV1, PagedProbeExit};
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, CpuActivity, GuestMemory, InstructionMode,
    LongModeState, MemoryError, OperandSize, PortIo, PrivilegeLevel, StiShadow,
};

const RIP: u64 = 0x4000;
const SCRATCH_BYTES: usize = 0x10_0000;

struct Memory {
    bytes: Vec<u8>,
}

impl Memory {
    fn range(&self, address: u64, len: usize, access: AccessKind) -> Result<usize, MemoryError> {
        let start = address as usize;
        if address < SCRATCH_BYTES as u64 && start + len <= self.bytes.len() {
            Ok(start)
        } else {
            Err(MemoryError::Unmapped { address, access })
        }
    }
}

impl GuestMemory for Memory {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        Ok(self.bytes[self.range(address, 1, AccessKind::Fetch)?])
    }

    fn read(
        &mut self,
        address: u64,
        _: AccessContext,
        destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        let start = self.range(address, destination.len(), AccessKind::Read)?;
        destination.copy_from_slice(&self.bytes[start..start + destination.len()]);
        Ok(())
    }

    fn write(&mut self, address: u64, _: AccessContext, source: &[u8]) -> Result<(), MemoryError> {
        let start = self.range(address, source.len(), AccessKind::Write)?;
        self.bytes[start..start + source.len()].copy_from_slice(source);
        Ok(())
    }

    fn read_for_write_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
    ) -> Result<u64, MemoryError> {
        self.read_integer(address, context, size)
    }

    fn atomic_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend(
            "atomic exchange is not modelled by the probe".into(),
        ))
    }

    fn atomic_fetch_add_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend(
            "atomic add is not modelled by the probe".into(),
        ))
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend(
            "atomic compare-exchange is not modelled by the probe".into(),
        ))
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

struct Pio;

impl PortIo for Pio {
    type Error = &'static str;

    fn read_u8(&mut self, _: u16) -> Result<u8, Self::Error> {
        Ok(0)
    }

    fn write_u8(&mut self, _: u16, _: u8) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// Active CR3 the board passes to the firmware profile; any aligned value gives the same verdict.
const BOARD_CR3: u64 = 0x60_0000;

fn main() {
    // `--board-context` runs the profile the way the board does (od-machine paged_probe.rs): with
    // an active CR3, guest FP state and an RDTSC source. Without it the bare `step_paged_firmware_probe`
    // is used, which reports `mov cr3`, FXSAVE/FXRSTOR and RDTSC as unsupported (S7-31a behaviour).
    let board = std::env::args().any(|argument| argument == "--board-context");
    let context = AccessContext {
        mode: InstructionMode::Long64,
        privilege: PrivilegeLevel::Ring0,
        origin: AccessOrigin::DescriptorTable,
    };
    let defined = od_isa::INTEGER_FLAGS & !(1 << 4);
    for line in std::io::stdin().lock().lines() {
        let line = line.expect("stdin");
        let text = line.trim();
        if text.is_empty() {
            continue;
        }
        let code: Vec<u8> = (0..text.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&text[at..at + 2], 16).expect("hex digits"))
            .collect();
        let mut memory = Memory {
            bytes: vec![0; SCRATCH_BYTES],
        };
        memory.bytes[RIP as usize..RIP as usize + code.len()].copy_from_slice(&code);
        let mut state = LongModeState {
            cr3: None,
            rip: RIP,
            rflags: 0x8d7,
            cr0: Some(od_isa::execute::Cr0Profile::BASE),
            cr4: Some(od_isa::execute::Cr4Profile::BASE),
            activity: Some(CpuActivity::Running),
            sti_shadow: Some(StiShadow::Clear),
            ..LongModeState::default()
        };
        state.gpr[4] = 0x8000; // RSP points into mapped scratch memory
        if board {
            // The board enables FXSAVE/SSE (CR4.OSFXSR and OSXMMEXCPT) before any FP instruction runs.
            state.cr4 = Some(od_isa::execute::Cr4Profile::BASE | (1 << 9) | (1 << 10));
        }
        let result = if board {
            let mut fp = GuestFpState::initial();
            let mut tsc = || Ok(0x1234_5678);
            execute::step_paged_firmware_probe_with_guest_fp(
                PagedCpl0FirmwareBootV1,
                &mut state,
                &mut memory,
                &mut Pio,
                context,
                defined,
                BOARD_CR3,
                &mut fp,
                Some(&mut tsc),
            )
        } else {
            execute::step_paged_firmware_probe(
                PagedCpl0FirmwareBootV1,
                &mut state,
                &mut memory,
                &mut Pio,
                context,
                defined,
            )
        };
        let verdict = match result.exit {
            PagedProbeExit::Unsupported { .. } => "unsupported",
            _ => "decoded",
        };
        let exit = format!("{:?}", result.exit);
        let kind = exit
            .split(|c: char| !c.is_alphanumeric())
            .next()
            .unwrap_or("?");
        println!("{text} {verdict} {kind}");
    }
}
