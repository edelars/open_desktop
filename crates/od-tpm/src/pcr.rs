//! PCR banks (SHA-256 and SHA-384, PCR 0-23) and the commands `TPM2_PCR_Read`, `TPM2_PCR_Extend`,
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

use sha2::{Digest, Sha256, Sha384};

use crate::codes::{alg, handle};
use crate::dispatch::Invocation;
use crate::marshal::{
    Reader, TpmlDigest, TpmlDigestValues, TpmlPcrSelection, TpmsPcrSelection, TpmtHa, Writer,
    limits,
};
use crate::response::{Output, Rc};
use crate::state::Tpm;

const SHA256_BANK: u8 = 1;
const SHA384_BANK: u8 = 2;
const BOTH_BANKS: u8 = SHA256_BANK | SHA384_BANK;
const BANKS: [(u16, u8, u32); 2] = [
    (alg::SHA256, SHA256_BANK, 32),
    (alg::SHA384, SHA384_BANK, 48),
];
const SELECT_NONE: [u8; limits::PCR_SELECT_SIZE] = [0; limits::PCR_SELECT_SIZE];
const SELECT_ALL: [u8; limits::PCR_SELECT_SIZE] = [0xff; limits::PCR_SELECT_SIZE];

/// The PTP allocation profile allows a complete 24-PCR bank or an empty one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PcrAllocation(u8);

impl Default for PcrAllocation {
    fn default() -> Self {
        Self(BOTH_BANKS)
    }
}

impl PcrAllocation {
    pub(crate) fn from_bits(bits: u8) -> Option<Self> {
        matches!(bits, SHA256_BANK | SHA384_BANK | BOTH_BANKS).then_some(Self(bits))
    }

    pub(crate) fn bits(self) -> u8 {
        self.0
    }

    fn contains(self, hash: u16) -> bool {
        BANKS
            .iter()
            .any(|&(bank, bit, _)| bank == hash && self.0 & bit != 0)
    }

    fn with_selections(self, selection: &TpmlPcrSelection) -> Result<Self, Rc> {
        let mut bits = 0;
        for (hash, bit, _) in BANKS {
            let selected = selection
                .selections
                .iter()
                .rfind(|entry| entry.hash == hash);
            let selected = match selected.map(|entry| entry.select) {
                None => self.0 & bit != 0,
                Some(SELECT_NONE) => false,
                Some(SELECT_ALL) => true,
                Some(_) => return Err(Rc::NO_RESULT),
            };
            if selected {
                bits |= bit;
            }
        }
        // Keeping one complete bank ensures that both PCR0 and PCR17 are allocated.
        Self::from_bits(bits).ok_or(Rc::PCR)
    }

    fn size(self) -> u32 {
        BANKS
            .iter()
            .filter(|(_, bit, _)| self.0 & bit != 0)
            .map(|(_, _, bytes)| bytes * limits::PCR_COUNT as u32)
            .sum()
    }
}

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

/// The two banks of 24 PCRs and their common `pcrUpdateCounter`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Pcrs {
    active: PcrAllocation,
    sha256: [[u8; 32]; limits::PCR_COUNT],
    sha384: [[u8; 48]; limits::PCR_COUNT],
    update_counter: u32,
}

impl Pcrs {
    /// Initial state after `Startup(CLEAR)` at locality 0 (PTP Table 15). PCR 0 holds the startup
    /// locality indicator, which is 0 for locality 0.
    pub fn new() -> Pcrs {
        Self::with_allocation(PcrAllocation::default())
    }

    pub(crate) fn with_allocation(active: PcrAllocation) -> Self {
        let mut p = Pcrs {
            active,
            sha256: [[0; 32]; limits::PCR_COUNT],
            sha384: [[0; 48]; limits::PCR_COUNT],
            update_counter: 0,
        };
        p.set_initial();
        p
    }

    /// Resets every PCR to its initial value; `pcrUpdateCounter` is cleared on TPM Reset
    /// (Part 3, clause 9.3) by the caller via [`Pcrs::clear_counter`].
    pub fn set_initial(&mut self) {
        for (i, (slot256, slot384)) in self.sha256.iter_mut().zip(&mut self.sha384).enumerate() {
            let initial = pcr_attr(i).initial;
            slot256.fill(initial);
            slot384.fill(initial);
        }
    }

    pub fn clear_counter(&mut self) {
        self.update_counter = 0;
    }

    pub fn update_counter(&self) -> u32 {
        self.update_counter
    }

    pub fn value(&self, hash: u16, index: usize) -> Option<&[u8]> {
        if !self.active.contains(hash) {
            return None;
        }
        match hash {
            alg::SHA256 => self.sha256.get(index).map(|slot| &slot[..]),
            alg::SHA384 => self.sha384.get(index).map(|slot| &slot[..]),
            _ => None,
        }
    }

    /// `PCR.new = H(PCR.old || digest)` (Part 3 v185 §22.2.1), in wire order. Duplicates
    /// extend the same bank again while the update counter counts each changed bank once.
    fn extend(&mut self, index: usize, digests: &[TpmtHa]) -> bool {
        if index >= limits::PCR_COUNT {
            return false;
        }
        let modified = self.extend_banks(index, digests);
        if !pcr_attr(index).no_increment {
            self.update_counter = self.update_counter.wrapping_add(modified.count_ones());
        }
        modified != 0
    }

    fn extend_banks(&mut self, index: usize, digests: &[TpmtHa]) -> u32 {
        let mut modified = 0u32;
        for value in digests {
            if !self.active.contains(value.hash) {
                continue;
            }
            match value.hash {
                alg::SHA256 => {
                    extend_value::<Sha256>(&mut self.sha256[index], &value.digest);
                    modified |= 1;
                }
                alg::SHA384 => {
                    extend_value::<Sha384>(&mut self.sha384[index], &value.digest);
                    modified |= 2;
                }
                _ => unreachable!("only decoded PCR digest algorithms reach an extend"),
            }
        }
        modified
    }

    pub(crate) fn measure_static_hcrtm(&mut self, digests: &TpmlDigestValues) {
        self.sha256[0] = [0; 32];
        self.sha384[0] = [0; 48];
        self.sha256[0][31] = 4;
        self.sha384[0][47] = 4;
        self.extend_banks(0, &digests.digests);
    }

    pub(crate) fn measure_dynamic_hcrtm(&mut self, digests: &TpmlDigestValues) {
        // PTP 1.07 Tables 14–15: D-RTM resets PCR17–22 to zero in each bank before extending PCR17.
        self.sha256[17..=22].fill([0; 32]);
        self.sha384[17..=22].fill([0; 48]);
        self.extend_banks(17, &digests.digests);
        self.increment_update_counter();
    }

    /// Resets one PCR across both banks, accounting for the reset once (§22.1).
    pub fn reset(&mut self, index: usize) -> bool {
        if index >= limits::PCR_COUNT {
            return false;
        }
        let attr = pcr_attr(index);
        self.sha256[index].fill(attr.initial);
        self.sha384[index].fill(attr.initial);
        if !attr.no_increment {
            self.increment_update_counter();
        }
        true
    }

    /// `TPM2_Clear` increments `pcrUpdateCounter` (Part 3, clause 24.6).
    pub(crate) fn increment_update_counter(&mut self) {
        self.update_counter = self.update_counter.wrapping_add(1);
    }

    /// PCR values that `Shutdown(STATE)` preserves (`TPM_PT_PCR_SAVE`) and the counter.
    pub(crate) fn save(&self) -> SavedPcrs {
        SavedPcrs {
            sha256: self.sha256,
            sha384: self.sha384,
            update_counter: self.update_counter,
        }
    }

    /// Restart and Resume start from the counter saved by Shutdown(STATE).
    /// Resume restores the state-save PCRs. Reinitializing counted PCRs adds
    /// one generation for this Startup command (Part 3 v185, clauses 9.3 and 22.1).
    pub(crate) fn startup_from_saved(&mut self, saved: &SavedPcrs, resume: bool) {
        let mut counted_reset = false;
        for (i, (slot256, slot384)) in self.sha256.iter_mut().zip(&mut self.sha384).enumerate() {
            let attr = pcr_attr(i);
            if resume && attr.save {
                *slot256 = saved.sha256[i];
                *slot384 = saved.sha384[i];
            } else {
                slot256.fill(attr.initial);
                slot384.fill(attr.initial);
                counted_reset |= !attr.no_increment;
            }
        }
        self.update_counter = saved.update_counter;
        if counted_reset {
            self.increment_update_counter();
        }
    }

    /// Report both supported banks, including an empty selection for a disabled bank.
    pub(crate) fn allocation(&self) -> TpmlPcrSelection {
        TpmlPcrSelection {
            selections: BANKS
                .into_iter()
                .map(|(hash, _, _)| TpmsPcrSelection {
                    hash,
                    select: if self.active.contains(hash) {
                        SELECT_ALL
                    } else {
                        SELECT_NONE
                    },
                })
                .collect(),
        }
    }

    /// Exclude unallocated PCR while preserving the caller's selector order and shape.
    pub(crate) fn filter_selection(&self, selection: &mut TpmlPcrSelection) {
        for bank in &mut selection.selections {
            if !self.active.contains(bank.hash) {
                bank.select = SELECT_NONE;
            }
        }
    }

    /// SHA-256 over concatenated PCR values in selector order and PCR index order.
    /// No values produces an absent creation digest.
    pub(crate) fn selection_digest(&self, selection: &TpmlPcrSelection) -> Option<[u8; 32]> {
        let mut hash = Sha256::new();
        let mut any = false;
        for bank in &selection.selections {
            for index in 0..limits::PCR_COUNT {
                if bank.selects(index)
                    && let Some(value) = self.value(bank.hash, index)
                {
                    hash.update(value);
                    any = true;
                }
            }
        }
        any.then(|| hash.finalize().into())
    }
}

fn extend_value<H: Digest>(slot: &mut [u8], value: &[u8]) {
    let mut hash = H::new();
    hash.update(&*slot);
    hash.update(value);
    slot.copy_from_slice(&hash.finalize());
}

impl Default for Pcrs {
    fn default() -> Self {
        Pcrs::new()
    }
}

/// State kept by `TPM2_Shutdown(STATE)` for PCR.
#[derive(Clone, Debug)]
pub(crate) struct SavedPcrs {
    sha256: [[u8; 32]; limits::PCR_COUNT],
    sha384: [[u8; 48]; limits::PCR_COUNT],
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

/// PCR_Event and event sequences calculate the same list over the same bytes.
#[derive(Clone, Default)]
pub(crate) struct EventHash {
    sha256: Sha256,
    sha384: Sha384,
}

impl EventHash {
    pub(crate) fn update(&mut self, data: &[u8]) {
        self.sha256.update(data);
        self.sha384.update(data);
    }

    pub(crate) fn finish(self) -> TpmlDigestValues {
        TpmlDigestValues {
            digests: vec![
                TpmtHa {
                    hash: alg::SHA256,
                    digest: self.sha256.finalize().to_vec(),
                },
                TpmtHa {
                    hash: alg::SHA384,
                    digest: self.sha384.finalize().to_vec(),
                },
            ],
        }
    }
}

impl Tpm {
    /// `TPM2_PCR_Allocate` (Part 3 v185 §22.5): a successful request replaces the
    /// pending allocation. The current banks remain in use until the next init.
    pub(crate) fn cmd_pcr_allocate(&mut self, p: &mut Reader<'_>) -> Result<Output, Rc> {
        p.set_param(1);
        let selection = TpmlPcrSelection::unmarshal(p)?;
        p.finish()?;
        let desired = self.persistent.pcr_allocation.with_selections(&selection)?;
        self.persistent.pcr_allocation = desired;
        self.pcr_allocation_pending = true;
        self.saved = None;
        let mut w = Writer::new();
        w.u8(1);
        w.u32(limits::PCR_COUNT as u32);
        w.u32(desired.size());
        w.u32(PcrAllocation::default().size());
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }

    /// Extends PCR `index` after the locality check; used by `PCR_Extend`, `PCR_Event` and
    /// `EventSequenceComplete`. Changing a PCR voids a state saved by `Shutdown(STATE)`
    /// (Part 3, clause 9.4.1: the saved state is nullified by a later command that changes it).
    fn extend_pcr(&mut self, index: usize, digests: &TpmlDigestValues) -> Result<(), Rc> {
        if digests.digests.is_empty() {
            return Ok(());
        }
        if !pcr_attr(index).extend_l0 {
            return Err(Rc::LOCALITY);
        }
        if self.pcrs.extend(index, &digests.digests) {
            self.saved = None;
        }
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
            self.extend_pcr(i, &digests)?;
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
        let mut hash = EventHash::default();
        hash.update(event);
        self.complete_pcr_event(pcr, hash.finish())
    }

    pub(crate) fn complete_pcr_event(
        &mut self,
        index: Option<usize>,
        digests: TpmlDigestValues,
    ) -> Result<Output, Rc> {
        if let Some(i) = index {
            self.extend_pcr(i, &digests)?;
        }
        let mut w = Writer::new();
        digests.marshal(&mut w);
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
                    && let Some(v) = self.pcrs.value(s.hash, i)
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
            assert_eq!(p.value(alg::SHA256, i).unwrap(), &[expect; 32], "PCR {i}");
            assert_eq!(p.value(alg::SHA384, i).unwrap(), &[expect; 48], "PCR {i}");
        }
        assert_eq!(p.update_counter(), 0);
    }

    #[test]
    fn extend_matches_independent_sha256_vector() {
        // Computed independently with Python hashlib (2026-11-07):
        //   d = sha256(b"abc"); n1 = sha256(32 x 0x00 || d); n2 = sha256(n1 || d)
        //   f = sha256(32 x 0xFF || d)
        let d = [TpmtHa {
            hash: alg::SHA256,
            digest: sha256(b"abc").to_vec(),
        }];
        let mut p = Pcrs::new();
        assert!(p.extend(0, &d));
        assert_eq!(
            p.value(alg::SHA256, 0).unwrap(),
            hex("589f9ffed4c477966bfb8d41f37895b08c69047df8f911d6f3b57fbe08faee8d")
        );
        assert!(p.extend(0, &d));
        assert_eq!(
            p.value(alg::SHA256, 0).unwrap(),
            hex("bdeb6c6dc63852834c89f67066194207ce7d3806ea40ca58dc079246ef58a926")
        );
        assert!(p.extend(17, &d));
        assert_eq!(
            p.value(alg::SHA256, 17).unwrap(),
            hex("ded4cee9953bb84c83278424b1e8256ee3483023f4ae5730affa51aad0063efb")
        );
        assert!(!p.extend(24, &d));
    }

    #[test]
    fn update_counter_skips_no_increment_pcrs() {
        let d = [TpmtHa {
            hash: alg::SHA256,
            digest: vec![1; 32],
        }];
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
