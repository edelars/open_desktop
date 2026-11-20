//! SHA-256 and SHA-384 hashing, SHA-256 HMAC, and their sequences.
//!
//! Sequence objects are transient objects (handle type `TPM_HT_TRANSIENT`), at most
//! [`MAX_SEQUENCES`] at a time; `TPM_RC_OBJECT_MEMORY` when no slot is free.
//!
//! **`TPM2_HMAC`** (Part 3 v184, clause 15.5) computes HMAC-SHA-256 with the key of an unrestricted
//! keyed-hash object that has `sign` SET (`TPM_RC_KEY` without `sign`, `TPM_RC_TYPE` for another
//! object type, `TPM_RC_ATTRIBUTES` for a restricted key). The key is created by `TPM2_Create` or
//! `TPM2_CreatePrimary` from a `KEYEDHASH` template with `sign` and scheme HMAC (SHA-256) or NULL
//! (`object.rs`): the key bytes come from `inSensitive.data` (at most 128 octets; `sensitiveDataOrigin`
//! CLEAR) or the TPM generates 32 octets (data empty, `sensitiveDataOrigin` SET). `hashAlg` is the
//! hash of the key's scheme or NULL, or SHA-256 for a key with a NULL scheme (`TPM_RC_VALUE`, hash
//! selection matrix of Part 3, Table 75); another algorithm is `TPM_RC_HASH`. The TPM may
//! implement `TPM2_HMAC` or `TPM2_MAC`, not both (same command code): this TPM implements
//! `TPM2_HMAC`. The Windows requirement for `TPM2_HMAC` is T-03 of
//! `docs/specs/WINDOWS_PLATFORM_REQUIREMENTS.md` (by Microsoft documentation; use is a hypothesis
//! until the S7-32 trace).
//!
//! Limitations recorded in `docs/specs/VTPM.md`:
//! - the `validation` ticket is always the NULL ticket (`TPMT_TK_HASHCHECK` with `TPM_RH_NULL`
//!   and an empty digest). A real ticket needs the hierarchy proof values (phase B, S7-23).
//! - NOT VERIFIED against TPM 2.0 Library v185: `TPM_RC_MODE` for a sequence of the wrong kind
//!   carries the handle number here; whether sequence objects are exempt from dictionary attack
//!   protection (a wrong password is answered `TPM_RC_BAD_AUTH`, there is no DA in phase A).

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256, Sha384};
use zeroize::Zeroize;

use crate::codes::{alg, handle, object_attr};
use crate::crypto::{hmac_sha256, hmac_sha256_state};
use crate::dispatch::Invocation;
use crate::marshal::{PublicParams, Reader, TpmtTkHashcheck, Writer, digest_size, limits};
use crate::object::Secret;
use crate::pcr::{EventHash, resolve_pcr_handle};
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

    /// Like [`AuthValue::from_slice`] but without trailing zero octets: "trailing zeros are always
    /// removed from an authValue before it is used in an authorization computation" (Part 1 v184,
    /// clause 17.6.5). The size limit applies to the value as given.
    pub(crate) fn from_slice_trimmed(v: &[u8]) -> Option<AuthValue> {
        if v.len() > limits::MAX_DIGEST_SIZE {
            return None;
        }
        AuthValue::from_slice(trim(v))
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
pub(crate) fn trim(v: &[u8]) -> &[u8] {
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

pub(crate) enum SequenceKind {
    /// Hash sequence of one algorithm (SHA-256).
    Hash(Sha256),
    /// Hash sequence of one algorithm (SHA-384).
    Hash384(Sha384),
    /// Incremental HMAC-SHA-256 state, independent of the original key handle.
    Hmac(Hmac<Sha256>),
    /// Event sequence with both PCR bank algorithms.
    Event(EventHash),
}

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

    /// Flushes the sequence in `slot` (`TPM2_FlushContext`).
    pub(crate) fn release(&mut self, slot: usize) {
        if let Some(s) = self.slots.get_mut(slot) {
            *s = None;
        }
    }

    fn start(&mut self, kind: SequenceKind, auth: AuthValue) -> Result<Output, Rc> {
        let slot = self
            .slots
            .iter()
            .position(|s| s.is_none())
            .ok_or(Rc::OBJECT_MEMORY)?;
        self.slots[slot] = Some(Sequence { kind, auth });
        Ok(Output {
            handles: seq_handle(slot).to_be_bytes().to_vec(),
            params: Vec::new(),
        })
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
        if digest_size(hash_alg).is_none() {
            // NULL is not allowed for TPM2_Hash and is answered like any unimplemented hash.
            return Err(p.err(Rc::HASH));
        }
        p.set_param(3);
        let hierarchy = p.u32()?;
        check_hierarchy(p, hierarchy)?;
        p.finish()?;
        let mut w = Writer::new();
        match hash_alg {
            alg::SHA256 => w.tpm2b(&Sha256::digest(data)),
            alg::SHA384 => w.tpm2b(&Sha384::digest(data)),
            _ => unreachable!("hash algorithm has already been checked"),
        }
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
            alg::SHA384 => SequenceKind::Hash384(Sha384::new()),
            alg::NULL => SequenceKind::Event(EventHash::default()),
            _ => return Err(p.err(Rc::HASH)),
        };
        p.finish()?;
        self.sequences.start(kind, auth)
    }

    /// The key and hash selection rules shared by HMAC and HMAC_Start (Part 3, Tables 75 and 79).
    fn hmac_key_for_hash(&self, handle: u32, hash_alg: u16) -> Result<&[u8], Rc> {
        let object = self.object(handle).ok_or(Rc::reference_handle(0))?;
        if !object.has(object_attr::SIGN) {
            return Err(Rc::KEY.in_handle(1));
        }
        let PublicParams::KeyedHash { scheme } = &object.public.params else {
            return Err(Rc::TYPE.in_handle(1));
        };
        if object.has(object_attr::RESTRICTED) {
            return Err(Rc::ATTRIBUTES.in_handle(1));
        }
        if !matches!(hash_alg, alg::SHA256 | alg::NULL) {
            return Err(Rc::HASH.in_param(2));
        }
        let valid_hash = if scheme.alg != alg::NULL {
            hash_alg == alg::NULL || hash_alg == scheme.hash
        } else {
            hash_alg == alg::SHA256
        };
        if !valid_hash {
            return Err(Rc::VALUE.in_param(2));
        }
        let Some(Secret::Data(key)) = object.sensitive.as_ref().map(|s| &s.secret) else {
            return Err(Rc::AUTH_UNAVAILABLE);
        };
        Ok(key)
    }

    /// `TPM2_HMAC` (Part 3, clause 15.5).
    pub(crate) fn cmd_hmac(&mut self, inv: &Invocation, p: &mut Reader<'_>) -> Result<Output, Rc> {
        p.set_param(1);
        let buffer = p.tpm2b(limits::MAX_2B_BUFFER_SIZE)?;
        p.set_param(2);
        let hash_alg = p.u16()?;
        p.finish()?;
        let key = self.hmac_key_for_hash(inv.handles[0], hash_alg)?;
        let mut w = Writer::new();
        w.tpm2b(&hmac_sha256(key, &[buffer]));
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }

    /// `TPM2_HMAC_Start` (Part 3, clause 17.2).
    pub(crate) fn cmd_hmac_start(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let auth = AuthValue::from_slice(p.tpm2b(limits::MAX_DIGEST_SIZE)?)
            .ok_or_else(|| p.err(Rc::SIZE))?;
        p.set_param(2);
        let hash_alg = p.u16()?;
        p.finish()?;
        let key = self.hmac_key_for_hash(inv.handles[0], hash_alg)?;
        let kind = SequenceKind::Hmac(hmac_sha256_state(key));
        self.sequences.start(kind, auth)
    }

    /// `TPM2_SequenceUpdate` (Part 3, clause 17.5).
    pub(crate) fn cmd_sequence_update(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        let slot = self.seq_slot(inv.handles[0], 0)?;
        p.set_param(1);
        let data = p.tpm2b(limits::MAX_2B_BUFFER_SIZE)?;
        p.finish()?;
        if let Some(sequence) = self.sequences.slots[slot].as_mut() {
            match &mut sequence.kind {
                SequenceKind::Hash(hash) => hash.update(data),
                SequenceKind::Hash384(hash) => hash.update(data),
                SequenceKind::Event(hash) => hash.update(data),
                SequenceKind::Hmac(mac) => mac.update(data),
            }
        }
        Ok(Output::default())
    }

    /// `TPM2_SequenceComplete` (Part 3, clause 17.6): hash and HMAC sequences (`TPM_RC_MODE` for an
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
        let sequence = self.sequences.slots[slot]
            .take()
            .expect("the sequence slot was validated");
        let mut w = Writer::new();
        match sequence.kind {
            SequenceKind::Hash(mut hash) => {
                hash.update(data);
                w.tpm2b(&hash.finalize());
            }
            SequenceKind::Hash384(mut hash) => {
                hash.update(data);
                w.tpm2b(&hash.finalize());
            }
            SequenceKind::Hmac(mut mac) => {
                mac.update(data);
                w.tpm2b(&mac.finalize().into_bytes());
            }
            SequenceKind::Event(_) => {
                self.sequences.slots[slot] = Some(sequence);
                return Err(Rc::MODE.in_handle(1));
            }
        }
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
        let response = self.complete_pcr_event(pcr, h.finish())?;
        self.sequences.slots[slot] = None;
        Ok(response)
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
