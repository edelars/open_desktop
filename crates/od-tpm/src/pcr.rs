//! PCR bank (SHA-256, PCR 0-23) and the commands `TPM2_PCR_Read`, `TPM2_PCR_Extend`,
//! `TPM2_PCR_Event`, `TPM2_PCR_Reset`.
//!
//! Attributes and initial values follow PC Client PTP 1.07 (3/23/2026), clause 4.7.1, Table 14 and
//! Table 15 (checked 2026-11-07, see `codes.rs`). The table has one column per locality
//! (4, 3, 2, 1, 0); phase A serves locality 0 only (ADR-0009, decision 4), so only that column
//! is encoded below and the other columns are recorded in `docs/specs/VTPM.md`.
//!
//! NOT VERIFIED against TPM 2.0 Library v185: the response code for an extend of a PCR that the
//! locality may not extend (`TPM_RC_LOCALITY`, the code Part 3 names for `TPM2_PCR_Reset`;
//! Part 3, clause 22.2 does not list it for `TPM2_PCR_Extend`).

use sha2::{Digest, Sha256};

use crate::codes::{alg, handle};
use crate::dispatch::Invocation;
use crate::marshal::{
    Reader, TpmlDigest, TpmlDigestValues, TpmlPcrSelection, TpmtHa, Writer, limits,
};
use crate::response::{Output, Rc};
use crate::state::Tpm;

/// Attributes of one PCR at locality 0 (PTP 1.07, Table 14).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PcrAttr {
    /// `TPM_PT_PCR_SAVE`: preserved by `Shutdown(STATE)` + `Startup(STATE)` (column 3).
    pub save: bool,
    /// `TPM_PT_PCR_NO_INCREMENT`: changes do not advance `pcrUpdateCounter` (column 6).
    pub no_increment: bool,
    /// Locality 0 may extend the PCR with `TPM2_PCR_Extend`/`Event` (column 9).
    pub extend_l0: bool,
    /// Locality 0 may reset the PCR with `TPM2_PCR_Reset` (column 8).
    pub reset_l0: bool,
    /// Value after `Startup(CLEAR)` and after a permitted reset (PTP Table 15).
    pub initial: u8,
}

/// Attributes of PCR `index` (0-23).
pub const fn pcr_attr(index: usize) -> PcrAttr {
    match index {
        // PCR 0-15 (Static RTM): saved, extendable, not resettable, initial 0.
        0..=15 => PcrAttr {
            save: true,
            no_increment: false,
            extend_l0: true,
            reset_l0: false,
            initial: 0x00,
        },
        // PCR 16 (Debug): not saved, no increment, extendable and resettable at locality 0.
        16 => PcrAttr {
            save: false,
            no_increment: true,
            extend_l0: true,
            reset_l0: true,
            initial: 0x00,
        },
        // PCR 17-20: D-RTM only; locality 0 can neither extend nor reset them. Initial all ones.
        17..=20 => PcrAttr {
            save: false,
            no_increment: false,
            extend_l0: false,
            reset_l0: false,
            initial: 0xFF,
        },
        // PCR 21-22: as above, and no increment.
        21..=22 => PcrAttr {
            save: false,
            no_increment: true,
            extend_l0: false,
            reset_l0: false,
            initial: 0xFF,
        },
        // PCR 23 (Application specific): like PCR 16.
        _ => PcrAttr {
            save: false,
            no_increment: true,
            extend_l0: true,
            reset_l0: true,
            initial: 0x00,
        },
    }
}

/// The SHA-256 bank of 24 PCR and the `pcrUpdateCounter`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Pcrs {
    values: [[u8; 32]; limits::PCR_COUNT],
    update_counter: u32,
}

impl Pcrs {
    /// Initial state after `Startup(CLEAR)` at locality 0 (PTP Table 15). PCR 0 holds the startup
    /// locality indicator, which is 0 for locality 0.
    pub fn new() -> Pcrs {
        let mut p = Pcrs {
            values: [[0u8; 32]; limits::PCR_COUNT],
            update_counter: 0,
        };
        p.set_initial();
        p
    }

    /// Resets every PCR to its initial value; `pcrUpdateCounter` is cleared on TPM Reset
    /// (Part 3, clause 9.3) by the caller via [`Pcrs::clear_counter`].
    pub fn set_initial(&mut self) {
        for (i, v) in self.values.iter_mut().enumerate() {
            *v = [pcr_attr(i).initial; 32];
        }
    }

    pub fn clear_counter(&mut self) {
        self.update_counter = 0;
    }

    pub fn update_counter(&self) -> u32 {
        self.update_counter
    }

    pub fn value(&self, index: usize) -> Option<&[u8; 32]> {
        self.values.get(index)
    }

    /// `PCR.new = H(PCR.old || digest)` (Part 3, clause 22.2.1). The caller has checked the
    /// locality attribute; returns false for an out-of-range index.
    pub fn extend(&mut self, index: usize, digest: &[u8; 32]) -> bool {
        let Some(slot) = self.values.get_mut(index) else {
            return false;
        };
        let mut h = Sha256::new();
        h.update(&slot[..]);
        h.update(digest);
        slot.copy_from_slice(&h.finalize());
        self.bump(index);
        true
    }

    /// Resets one PCR to its initial value (the caller has checked `reset_l0`).
    pub fn reset(&mut self, index: usize) -> bool {
        let Some(slot) = self.values.get_mut(index) else {
            return false;
        };
        *slot = [pcr_attr(index).initial; 32];
        self.bump(index);
        true
    }

    /// `TPM2_Clear` increments `pcrUpdateCounter` (Part 3, clause 24.6).
    pub(crate) fn increment_update_counter(&mut self) {
        self.update_counter = self.update_counter.wrapping_add(1);
    }

    fn bump(&mut self, index: usize) {
        if !pcr_attr(index).no_increment {
            self.update_counter = self.update_counter.wrapping_add(1);
        }
    }

    /// PCR values that `Shutdown(STATE)` preserves (`TPM_PT_PCR_SAVE`) and the counter.
    pub(crate) fn save(&self) -> SavedPcrs {
        SavedPcrs {
            values: self.values,
            update_counter: self.update_counter,
        }
    }

    /// Restart and Resume start from the counter saved by Shutdown(STATE).
    /// Resume restores the state-save PCRs. Reinitializing counted PCRs adds
    /// one generation for this Startup command (Part 3 v185, clauses 9.3 and 22.1).
    pub(crate) fn startup_from_saved(&mut self, saved: &SavedPcrs, resume: bool) {
        let mut counted_reset = false;
        for (i, v) in self.values.iter_mut().enumerate() {
            let attr = pcr_attr(i);
            if resume && attr.save {
                *v = saved.values[i];
            } else {
                *v = [attr.initial; 32];
                counted_reset |= !attr.no_increment;
            }
        }
        self.update_counter = saved.update_counter;
        if counted_reset {
            self.increment_update_counter();
        }
    }
}

impl Default for Pcrs {
    fn default() -> Self {
        Pcrs::new()
    }
}

/// State kept by `TPM2_Shutdown(STATE)` for PCR.
#[derive(Clone, Debug)]
pub(crate) struct SavedPcrs {
    values: [[u8; 32]; limits::PCR_COUNT],
    update_counter: u32,
}

/// Resolves a `TPMI_DH_PCR` handle: `Ok(Some(i))` for PCR `i`, `Ok(None)` for `TPM_RH_NULL` when
/// `allow_null`, otherwise `TPM_RC_VALUE` with the handle number (Part 2, `TPMI_DH_PCR`).
pub(crate) fn resolve_pcr_handle(
    h: u32,
    allow_null: bool,
    handle_no: u8,
) -> Result<Option<usize>, Rc> {
    if allow_null && h == handle::RH_NULL {
        return Ok(None);
    }
    if handle::handle_type(h) == handle::HT_PCR && (h as usize) < limits::PCR_COUNT {
        return Ok(Some(h as usize));
    }
    Err(Rc::VALUE.in_handle(handle_no))
}

/// SHA-256 of `data` as an array.
pub(crate) fn sha256(data: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(&Sha256::digest(data));
    out
}

impl Tpm {
    /// Extends PCR `index` after the locality check; used by `PCR_Extend`, `PCR_Event` and
    /// `EventSequenceComplete`. Changing a PCR voids a state saved by `Shutdown(STATE)`
    /// (Part 3, clause 9.4.1: the saved state is nullified by a later command that changes it).
    pub(crate) fn extend_pcr(&mut self, index: usize, digest: &[u8; 32]) -> Result<(), Rc> {
        if !pcr_attr(index).extend_l0 {
            return Err(Rc::LOCALITY);
        }
        if !self.pcrs.extend(index, digest) {
            return Err(Rc::VALUE);
        }
        self.saved = None;
        Ok(())
    }

    /// `TPM2_PCR_Extend` (Part 3, clause 22.2). Handle 1 is the PCR or `TPM_RH_NULL`.
    pub(crate) fn cmd_pcr_extend(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        let pcr = resolve_pcr_handle(inv.handles[0], true, 1)?;
        p.set_param(1);
        let digests = TpmlDigestValues::unmarshal(p)?;
        p.finish()?;
        if let Some(i) = pcr {
            for d in &digests.digests {
                // `unmarshal` guarantees SHA-256 and 32 octets.
                let mut a = [0u8; 32];
                a.copy_from_slice(&d.digest);
                self.extend_pcr(i, &a)?;
            }
        }
        Ok(Output::default())
    }

    /// `TPM2_PCR_Event` (Part 3, clause 22.3): hashes `eventData` and extends.
    pub(crate) fn cmd_pcr_event(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        let pcr = resolve_pcr_handle(inv.handles[0], true, 1)?;
        p.set_param(1);
        let event = p.tpm2b(limits::MAX_EVENT_SIZE)?;
        p.finish()?;
        let digest = sha256(event);
        if let Some(i) = pcr {
            self.extend_pcr(i, &digest)?;
        }
        let mut w = Writer::new();
        TpmlDigestValues {
            digests: vec![TpmtHa {
                hash: alg::SHA256,
                digest: digest.to_vec(),
            }],
        }
        .marshal(&mut w);
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }

    /// `TPM2_PCR_Reset` (Part 3, clause 22.8): `TPM_RC_LOCALITY` if locality 0 may not reset the PCR.
    pub(crate) fn cmd_pcr_reset(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        let pcr = resolve_pcr_handle(inv.handles[0], false, 1)?;
        p.finish()?;
        let Some(i) = pcr else {
            return Err(Rc::VALUE.in_handle(1));
        };
        if !pcr_attr(i).reset_l0 {
            return Err(Rc::LOCALITY);
        }
        self.pcrs.reset(i);
        self.saved = None;
        Ok(Output::default())
    }

    /// `TPM2_PCR_Read` (Part 3, clause 22.4): no authorization; values in selection order, at most
    /// 8 per response (`pcrSelectionOut` tells which ones came back).
    pub(crate) fn cmd_pcr_read(&mut self, p: &mut Reader<'_>) -> Result<Output, Rc> {
        p.set_param(1);
        let sel = TpmlPcrSelection::unmarshal(p)?;
        p.finish()?;
        let mut values = TpmlDigest::default();
        let mut out = TpmlPcrSelection::default();
        for s in &sel.selections {
            let mut returned = s.clone();
            returned.select = [0; limits::PCR_SELECT_SIZE];
            for i in 0..limits::PCR_COUNT {
                // The values form a TPML_DIGEST, which holds at most 8 digests (Part 2, Table 123):
                // the TPM stops there and the caller continues with the bits that came back clear
                // (Part 3, clause 22.4.1: "until pcrValues would be too large").
                if s.selects(i)
                    && values.digests.len() < limits::MAX_DIGEST_LIST
                    && let Some(v) = self.pcrs.value(i)
                {
                    values.digests.push(v.to_vec());
                    returned.select[i / 8] |= 1 << (i % 8);
                }
            }
            out.selections.push(returned);
        }
        let mut w = Writer::new();
        w.u32(self.pcrs.update_counter());
        out.marshal(&mut w);
        values.marshal(&mut w);
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn attributes_match_ptp_table_14_for_locality_0() {
        for i in 0..=15 {
            let a = pcr_attr(i);
            assert!(
                a.save && a.extend_l0 && !a.reset_l0 && !a.no_increment && a.initial == 0,
                "PCR {i}"
            );
        }
        assert_eq!(
            pcr_attr(16),
            PcrAttr {
                save: false,
                no_increment: true,
                extend_l0: true,
                reset_l0: true,
                initial: 0
            }
        );
        for i in 17..=22 {
            let a = pcr_attr(i);
            assert!(
                !a.extend_l0 && !a.reset_l0 && !a.save && a.initial == 0xFF,
                "PCR {i}"
            );
        }
        assert!(!pcr_attr(17).no_increment && !pcr_attr(20).no_increment);
        assert!(pcr_attr(21).no_increment && pcr_attr(22).no_increment);
        assert_eq!(pcr_attr(23), pcr_attr(16));
    }

    #[test]
    fn initial_values_match_ptp_table_15() {
        let p = Pcrs::new();
        for i in 0..24 {
            let expect = if (17..=22).contains(&i) { 0xFF } else { 0x00 };
            assert_eq!(p.value(i).unwrap(), &[expect; 32], "PCR {i}");
        }
        assert_eq!(p.update_counter(), 0);
    }

    #[test]
    fn extend_matches_independent_sha256_vector() {
        // Computed independently with Python hashlib (2026-11-07):
        //   d = sha256(b"abc"); n1 = sha256(32 x 0x00 || d); n2 = sha256(n1 || d)
        //   f = sha256(32 x 0xFF || d)
        let d = sha256(b"abc");
        let mut p = Pcrs::new();
        assert!(p.extend(0, &d));
        assert_eq!(
            p.value(0).unwrap().to_vec(),
            hex("589f9ffed4c477966bfb8d41f37895b08c69047df8f911d6f3b57fbe08faee8d")
        );
        assert!(p.extend(0, &d));
        assert_eq!(
            p.value(0).unwrap().to_vec(),
            hex("bdeb6c6dc63852834c89f67066194207ce7d3806ea40ca58dc079246ef58a926")
        );
        assert!(p.extend(17, &d));
        assert_eq!(
            p.value(17).unwrap().to_vec(),
            hex("ded4cee9953bb84c83278424b1e8256ee3483023f4ae5730affa51aad0063efb")
        );
        assert!(!p.extend(24, &d));
    }

    #[test]
    fn update_counter_skips_no_increment_pcrs() {
        let d = [1u8; 32];
        let mut p = Pcrs::new();
        p.extend(0, &d);
        assert_eq!(p.update_counter(), 1);
        p.extend(16, &d);
        p.extend(23, &d);
        assert_eq!(p.update_counter(), 1);
        p.reset(16);
        assert_eq!(p.update_counter(), 1);
        p.extend(5, &d);
        assert_eq!(p.update_counter(), 2);
    }

    #[test]
    fn resolve_pcr_handle_cases() {
        assert_eq!(resolve_pcr_handle(7, false, 1), Ok(Some(7)));
        assert_eq!(resolve_pcr_handle(handle::RH_NULL, true, 1), Ok(None));
        assert_eq!(
            resolve_pcr_handle(handle::RH_NULL, false, 1),
            Err(Rc::VALUE.in_handle(1))
        );
        assert_eq!(resolve_pcr_handle(24, true, 1), Err(Rc::VALUE.in_handle(1)));
        assert_eq!(
            resolve_pcr_handle(0x0100_0000, true, 2),
            Err(Rc::VALUE.in_handle(2))
        );
    }
}
