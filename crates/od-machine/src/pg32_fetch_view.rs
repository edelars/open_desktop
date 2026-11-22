//! The board's view of the instruction bytes of a vCPU that runs on 32-bit paging (S2-25 remainder).
//!
//! A vCPU in a legacy protected-mode model ([`ProtectedCpu`], `CR0.PG = 1`, `CR4.PAE = 0`)
//! translates through the two-level walk of `page_walk::pg32` (4 KiB pages, 4 MiB with `CR4.PSE`);
//! this is the board-level counterpart of `fetch_vcpu_view_pae` for it. It is read-only: no accessed or
//! dirty bit is written. The long-mode path (`Long64ProbeConfig`, the 4-level walk) does not know
//! this module and is not selected or changed by it: a vCPU is on this walk only when its `legacy`
//! model is a protected-mode one with 32-bit paging.

// No production caller yet (like the legacy models of S6-16c): tests exercise it now.
#![cfg_attr(not(test), allow(dead_code))]

use crate::ProbeConfigError;
use crate::vcpu::LegacyCpu;
use crate::{
    GuestPhysicalAddress, LoadedFirmwareBoard, Pg32ProbeConfig, translate_pg32_ram_read_only,
};
use od_isa::memory::{AccessContext, AccessKind, AccessOrigin, InstructionMode, PrivilegeLevel};

impl LoadedFirmwareBoard {
    /// The 32-bit paging snapshot of vCPU `cpu`: its legacy protected-mode model with its control
    /// registers and its PSE-36 model switch. An error text when the vCPU does not exist, is not in a
    /// legacy protected-mode model, or that model is not on 32-bit paging.
    pub(crate) fn pg32_paging_of(&self, cpu: usize) -> Result<Pg32ProbeConfig, String> {
        let vcpu = self.vcpu(cpu).ok_or_else(|| format!("no vCPU {cpu}"))?;
        match &vcpu.legacy {
            Some(LegacyCpu::Protected(model)) => Pg32ProbeConfig::from_protected_cpu(model)
                .map_err(|error: ProbeConfigError| format!("{error:?}")),
            Some(LegacyCpu::Real(_)) => Err(format!("vCPU {cpu} is in real mode")),
            None => Err(format!("vCPU {cpu} is not in a legacy mode")),
        }
    }

    /// Up to `len` bytes at the 32-bit linear address `linear` as an instruction fetch of vCPU `cpu`
    /// would see them through 32-bit paging (no accessed/dirty bit is written). The bytes are read with
    /// `PhysicalMemory::load_u8`, i.e. from any region with backing (RAM and ROM), not from
    /// MMIO or reserved ranges; the walk itself reads its tables from RAM only. Returns the physical
    /// address of `linear` (when it translates), the bytes read (they stop at the first byte that
    /// does not translate or whose region has no readable backing) and the text of that failure. The linear address
    /// wraps at 4 GiB like a 32-bit EIP.
    pub(crate) fn fetch_vcpu_view_pg32(
        &mut self,
        cpu: usize,
        linear: u32,
        len: usize,
        privilege: PrivilegeLevel,
    ) -> (Option<u64>, Vec<u8>, Option<String>) {
        let paging = match self.pg32_paging_of(cpu) {
            Ok(paging) => paging,
            Err(error) => return (None, Vec::new(), Some(error)),
        };
        let context = AccessContext {
            // Only fills the AccessContext shape: the 32-bit walk does not read `mode` (address width,
            // rights and the error code come from the paging model of the vCPU), so Long64 here does
            // not select the long-mode path.
            mode: InstructionMode::Long64,
            privilege,
            origin: AccessOrigin::Explicit,
        };
        let (memory, _) = self.bus_mut().split_for_pio_step();
        let mut first = None;
        let mut bytes = Vec::with_capacity(len);
        for offset in 0..len as u32 {
            let address = linear.wrapping_add(offset);
            let translation = match translate_pg32_ram_read_only(
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
