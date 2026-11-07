//! `TPM2_Hash` and hash/event sequences (`HashSequenceStart`, `SequenceUpdate`,
//! `SequenceComplete`, `EventSequenceComplete`), SHA-256 only (HASH_COUNT = 1).
//!
//! Sequence objects are transient objects (handle type `TPM_HT_TRANSIENT`), at most
//! [`MAX_SEQUENCES`] at a time; `TPM_RC_OBJECT_MEMORY` when no slot is free.
//!
//! Limitations recorded in `docs/specs/VTPM.md`:
//! - the `validation` ticket is always the NULL ticket (`TPMT_TK_HASHCHECK` with `TPM_RH_NULL`
//!   and an empty digest). A real ticket needs the hierarchy proof values (phase B, S7-23).
//! - NOT VERIFIED against TPM 2.0 Library v185: `TPM_RC_MODE` for a sequence of the wrong kind
//!   carries the handle number here; whether sequence objects are exempt from dictionary attack
//!   protection (a wrong password is answered `TPM_RC_BAD_AUTH`, there is no DA in phase A).

use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::codes::{alg, handle};
use crate::dispatch::Invocation;
use crate::marshal::{Reader, TpmlDigestValues, TpmtHa, TpmtTkHashcheck, Writer, limits};
use crate::pcr::resolve_pcr_handle;
use crate::response::{Output, Rc};
use crate::state::Tpm;

/// Sequence objects held at once (`TPM_PT_HR_TRANSIENT_MIN` is 3 in PTP 1.07, Table 2).
pub const MAX_SEQUENCES: usize = 3;

/// `TPMI_RH_HIERARCHY+` (Part 2, Table 2x): owner, platform, endorsement or null.
fn check_hierarchy(r: &Reader<'_>, h: u32) -> Result<u32, Rc> {
    match h {
        handle::RH_OWNER | handle::RH_PLATFORM | handle::RH_ENDORSEMENT | handle::RH_NULL => Ok(h),
        _ => Err(r.err(Rc::VALUE)),
    }
}

/// The NULL ticket of Part 2, Table 112 (`TPM_RH_NULL`, empty digest).
fn null_ticket() -> TpmtTkHashcheck {
    TpmtTkHashcheck {
        hierarchy: handle::RH_NULL,
        digest: Vec::new(),
    }
}

/// Authorization value of a sequence object (at most one digest long); wiped on drop.
#[derive(Clone)]
pub(crate) struct AuthValue {
    len: usize,
    bytes: [u8; limits::MAX_DIGEST_SIZE],
}

impl AuthValue {
    pub(crate) fn empty() -> AuthValue {
        AuthValue {
            len: 0,
            bytes: [0; limits::MAX_DIGEST_SIZE],
        }
    }

    pub(crate) fn from_slice(v: &[u8]) -> Option<AuthValue> {
        if v.len() > limits::MAX_DIGEST_SIZE {
            return None;
        }
        let mut a = AuthValue::empty();
        a.bytes[..v.len()].copy_from_slice(v);
        a.len = v.len();
        Some(a)
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl Drop for AuthValue {
    fn drop(&mut self) {
        self.bytes.zeroize();
        self.len = 0;
    }
}

/// Octets of a password without trailing zero octets (Part 1, clause 17.4: the TPM truncates
/// trailing zeros of both values before comparing).
fn trim(v: &[u8]) -> &[u8] {
    let end = v.iter().rposition(|b| *b != 0).map_or(0, |i| i + 1);
    &v[..end]
}

/// Password check without an early exit on the first differing octet.
pub(crate) fn password_matches(given: &[u8], auth: &[u8]) -> bool {
    let (g, a) = (trim(given), trim(auth));
    let mut diff = (g.len() ^ a.len()) as u32;
    for i in 0..limits::MAX_DIGEST_SIZE {
        let x = g.get(i).copied().unwrap_or(0);
        let y = a.get(i).copied().unwrap_or(0);
        diff |= u32::from(x ^ y);
    }
    diff == 0
}

#[derive(Clone)]
pub(crate) enum SequenceKind {
    /// Hash sequence of one algorithm (SHA-256).
    Hash(Sha256),
    /// Event sequence: one hash context per PCR bank (a single bank).
    Event(Sha256),
}

#[derive(Clone)]
pub(crate) struct Sequence {
    pub kind: SequenceKind,
    pub auth: AuthValue,
}

/// Transient sequence slots.
#[derive(Default)]
pub(crate) struct Sequences {
    slots: [Option<Sequence>; MAX_SEQUENCES],
}

impl Sequences {
    pub(crate) fn clear(&mut self) {
        self.slots = Default::default();
    }

    pub(crate) fn loaded_handles(&self) -> Vec<u32> {
        (0..MAX_SEQUENCES)
            .filter(|i| self.slots[*i].is_some())
            .map(seq_handle)
            .collect()
    }

    pub(crate) fn free_slots(&self) -> usize {
        self.slots.iter().filter(|s| s.is_none()).count()
    }

    fn insert(&mut self, s: Sequence) -> Option<usize> {
        let i = self.slots.iter().position(|s| s.is_none())?;
        self.slots[i] = Some(s);
        Some(i)
    }
}

fn seq_handle(slot: usize) -> u32 {
    (u32::from(handle::HT_TRANSIENT) << 24) | slot as u32
}

impl Tpm {
    /// Slot of the loaded sequence a handle names. `handle_idx` is the zero-based position in the
    /// handle area. Not a transient/persistent object handle: `TPM_RC_VALUE`; a persistent handle
    /// (nothing is persistent in phase A): `TPM_RC_HANDLE`; unloaded transient:
    /// `TPM_RC_REFERENCE_Hn`.
    pub(crate) fn seq_slot(&self, h: u32, handle_idx: usize) -> Result<usize, Rc> {
        let n = (handle_idx + 1) as u8;
        match handle::handle_type(h) {
            handle::HT_TRANSIENT => {
                let slot = (h & 0x00FF_FFFF) as usize;
                if slot < MAX_SEQUENCES && self.sequences.slots[slot].is_some() {
                    Ok(slot)
                } else {
                    Err(Rc::reference_handle(handle_idx))
                }
            }
            handle::HT_PERSISTENT => Err(Rc::HANDLE.in_handle(n)),
            _ => Err(Rc::VALUE.in_handle(n)),
        }
    }

    /// Authorization value of the sequence in `slot`.
    pub(crate) fn seq_auth(&self, slot: usize) -> AuthValue {
        self.sequences.slots[slot]
            .as_ref()
            .map_or_else(AuthValue::empty, |s| s.auth.clone())
    }

    /// `TPM2_Hash` (Part 3, clause 15.4).
    pub(crate) fn cmd_hash(&mut self, p: &mut Reader<'_>) -> Result<Output, Rc> {
        p.set_param(1);
        let data = p.tpm2b(limits::MAX_2B_BUFFER_SIZE)?;
        p.set_param(2);
        let hash_alg = p.u16()?;
        if hash_alg != alg::SHA256 {
            // NULL is not allowed for TPM2_Hash and is answered like any unimplemented hash.
            return Err(p.err(Rc::HASH));
        }
        p.set_param(3);
        let hierarchy = p.u32()?;
        check_hierarchy(p, hierarchy)?;
        p.finish()?;
        let digest = Sha256::digest(data);
        let mut w = Writer::new();
        w.tpm2b(&digest);
        null_ticket().marshal(&mut w);
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }

    /// `TPM2_HashSequenceStart` (Part 3, clause 17.4): `hashAlg` NULL starts an event sequence.
    pub(crate) fn cmd_hash_sequence_start(&mut self, p: &mut Reader<'_>) -> Result<Output, Rc> {
        p.set_param(1);
        let auth_bytes = p.tpm2b(limits::MAX_DIGEST_SIZE)?;
        let auth = AuthValue::from_slice(auth_bytes).ok_or_else(|| p.err(Rc::SIZE))?;
        p.set_param(2);
        let hash_alg = p.u16()?;
        let kind = match hash_alg {
            alg::SHA256 => SequenceKind::Hash(Sha256::new()),
            alg::NULL => SequenceKind::Event(Sha256::new()),
            _ => return Err(p.err(Rc::HASH)),
        };
        p.finish()?;
        let slot = self
            .sequences
            .insert(Sequence { kind, auth })
            .ok_or(Rc::OBJECT_MEMORY)?;
        Ok(Output {
            handles: seq_handle(slot).to_be_bytes().to_vec(),
            params: Vec::new(),
        })
    }

    /// `TPM2_SequenceUpdate` (Part 3, clause 17.5): works on hash and event sequences.
    pub(crate) fn cmd_sequence_update(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        let slot = self.seq_slot(inv.handles[0], 0)?;
        p.set_param(1);
        let data = p.tpm2b(limits::MAX_2B_BUFFER_SIZE)?;
        p.finish()?;
        if let Some(Sequence {
            kind: SequenceKind::Hash(h) | SequenceKind::Event(h),
            ..
        }) = self.sequences.slots[slot].as_mut()
        {
            h.update(data);
        }
        Ok(Output::default())
    }

    /// `TPM2_SequenceComplete` (Part 3, clause 17.6): hash sequences only (`TPM_RC_MODE` for an
    /// event sequence); flushes the sequence on success.
    pub(crate) fn cmd_sequence_complete(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        let slot = self.seq_slot(inv.handles[0], 0)?;
        p.set_param(1);
        let data = p.tpm2b(limits::MAX_2B_BUFFER_SIZE)?;
        p.set_param(2);
        let hierarchy = p.u32()?;
        check_hierarchy(p, hierarchy)?;
        p.finish()?;
        let Some(Sequence {
            kind: SequenceKind::Hash(h),
            ..
        }) = self.sequences.slots[slot].as_ref()
        else {
            return Err(Rc::MODE.in_handle(1));
        };
        let mut h = h.clone();
        h.update(data);
        let digest = h.finalize();
        self.sequences.slots[slot] = None;
        let mut w = Writer::new();
        w.tpm2b(&digest);
        null_ticket().marshal(&mut w);
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }

    /// `TPM2_EventSequenceComplete` (Part 3, clause 17.7): handle 1 is the PCR (or `TPM_RH_NULL`),
    /// handle 2 the event sequence (`TPM_RC_MODE` for a hash sequence). The sequence is flushed
    /// only if the extend succeeded.
    pub(crate) fn cmd_event_sequence_complete(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        let pcr = resolve_pcr_handle(inv.handles[0], true, 1)?;
        let slot = self.seq_slot(inv.handles[1], 1)?;
        p.set_param(1);
        let data = p.tpm2b(limits::MAX_2B_BUFFER_SIZE)?;
        p.finish()?;
        let Some(Sequence {
            kind: SequenceKind::Event(h),
            ..
        }) = self.sequences.slots[slot].as_ref()
        else {
            return Err(Rc::MODE.in_handle(2));
        };
        let mut h = h.clone();
        h.update(data);
        let mut digest = [0u8; 32];
        digest.copy_from_slice(&h.finalize());
        if let Some(i) = pcr {
            self.extend_pcr(i, &digest)?;
        }
        self.sequences.slots[slot] = None;
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_comparison_truncates_trailing_zeros() {
        assert!(password_matches(b"", b""));
        assert!(password_matches(&[0, 0], b""));
        assert!(password_matches(b"abc\0\0", b"abc"));
        assert!(password_matches(b"abc", b"abc\0"));
        assert!(!password_matches(b"abd", b"abc"));
        assert!(!password_matches(b"abc", b""));
        assert!(!password_matches(b"", b"\x01"));
        // A leading zero is significant.
        assert!(!password_matches(b"\0a", b"a"));
    }

    #[test]
    fn auth_value_limits_and_wipe() {
        assert!(AuthValue::from_slice(&[1; 33]).is_none());
        let a = AuthValue::from_slice(&[1, 2, 3]).unwrap();
        assert_eq!(a.as_slice(), [1, 2, 3]);
        assert!(AuthValue::empty().as_slice().is_empty());
    }

    #[test]
    fn hierarchy_values() {
        let r = Reader::new(&[]);
        for h in [
            handle::RH_OWNER,
            handle::RH_PLATFORM,
            handle::RH_ENDORSEMENT,
            handle::RH_NULL,
        ] {
            assert_eq!(check_hierarchy(&r, h), Ok(h));
        }
        assert_eq!(check_hierarchy(&r, 0x4000_000A), Err(Rc::VALUE));
    }
}
