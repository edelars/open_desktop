//! The board's view of the instruction bytes of a vCPU that runs on PAE paging (S2-25h).
//!
//! A vCPU in a legacy protected-mode model ([`ProtectedCpu`], `CR0.PG = 1`, `CR4.PAE = 1`,
//! `EFER.LME = 0`) translates through the PDPTE registers of the model; this is the board-level
//! counterpart of `LoadedFirmwareBoard::fetch_vcpu_view_2m` for it. It is read-only: no accessed or
//! dirty bit is written. The long-mode path (`Long64ProbeConfig`, the 4-level walk) does not know
//! this module and is not selected or changed by it: a vCPU is on this walk only when its `legacy`
//! model is a protected-mode one with PAE paging.

// No production caller yet (like the legacy models of S6-16c): tests exercise it now.
#![cfg_attr(not(test), allow(dead_code))]

use crate::vcpu::LegacyCpu;
use crate::{
    GuestPhysicalAddress, LoadedFirmwareBoard, PaeConfigError, PaeProbeConfig,
    translate_pae_ram_read_only,
};
use od_isa::memory::{AccessContext, AccessKind, AccessOrigin, InstructionMode, PrivilegeLevel};

impl LoadedFirmwareBoard {
    /// The PAE paging snapshot of vCPU `cpu`: its legacy protected-mode model with its control
    /// registers and PDPTE registers. An error text when the vCPU does not exist, is not in a
    /// legacy protected-mode model, or that model is not on PAE paging.
    pub(crate) fn pae_paging_of(&self, cpu: usize) -> Result<PaeProbeConfig, String> {
        let vcpu = self.vcpu(cpu).ok_or_else(|| format!("no vCPU {cpu}"))?;
        match &vcpu.legacy {
            Some(LegacyCpu::Protected(model)) => PaeProbeConfig::from_protected_cpu(model)
                .map_err(|error: PaeConfigError| format!("{error:?}")),
            Some(LegacyCpu::Real(_)) => Err(format!("vCPU {cpu} is in real mode")),
            None => Err(format!("vCPU {cpu} is not in a legacy mode")),
        }
    }

    /// Up to `len` bytes at the 32-bit linear address `linear` as an instruction fetch of vCPU `cpu`
    /// would see them through PAE paging (no accessed/dirty bit is written), read from RAM. Returns
    /// the physical address of `linear` (when it translates), the bytes read (they stop at the first
    /// byte that does not translate or is not RAM) and the text of that failure. The linear address
    /// wraps at 4 GiB like a 32-bit EIP.
    pub(crate) fn fetch_vcpu_view_pae(
        &mut self,
        cpu: usize,
        linear: u32,
        len: usize,
        privilege: PrivilegeLevel,
    ) -> (Option<u64>, Vec<u8>, Option<String>) {
        let paging = match self.pae_paging_of(cpu) {
            Ok(paging) => paging,
            Err(error) => return (None, Vec::new(), Some(error)),
        };
        let context = AccessContext {
            mode: InstructionMode::Long64,
            privilege,
            origin: AccessOrigin::Explicit,
        };
        let (memory, _) = self.bus_mut().split_for_pio_step();
        let mut first = None;
        let mut bytes = Vec::with_capacity(len);
        for offset in 0..len as u32 {
            let address = linear.wrapping_add(offset);
            let translation = match translate_pae_ram_read_only(
                memory,
                &paging,
                u64::from(linear),
                address,
                context,
                AccessKind::Fetch,
            ) {
                Ok(translation) => translation,
                Err(error) => return (first, bytes, Some(format!("{error:?}"))),
            };
            let physical: GuestPhysicalAddress = translation.physical;
            if offset == 0 {
                first = Some(physical.get());
            }
            match memory.load_u8(physical) {
                Ok(byte) => bytes.push(byte),
                Err(error) => return (first, bytes, Some(format!("{error:?}"))),
            }
        }
        (first, bytes, None)
    }
}
