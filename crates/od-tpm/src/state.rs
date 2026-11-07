//! TPM state: persistent part (survives power cycles, goes into the snapshot) and volatile part
//! (reset by `_TPM_Init`), the snapshot codec, and `TPM2_ReadClock`.
//!
//! Persistent (ADR-0009, decision 5; slice A of phase B added the in-memory blocks, the snapshot
//! codec is completed by slice C): the three hierarchy seeds (generated from owner entropy), the
//! clock base, `resetCount`, `restartCount` and the orderly-shutdown flag go into the snapshot;
//! the hierarchy authorization values and the NV store (`hierarchy.rs`, `nv.rs`) survive
//! `_TPM_Init` but are NOT in the snapshot yet.
//! Slice B of phase B adds the persistent objects (in memory, NOT in the snapshot yet) and the
//! transient objects (volatile, flushed by `Startup`/`_TPM_Init`). The hierarchy proof values are
//! not stored: they are derived from the seeds (`object_cmd.rs`).
//! Volatile: PCR, sequences, sessions, objects, hierarchy enables and `platformAuth`,
//! `started`/`failed`, the state saved by `Shutdown(STATE)`, the DRBG.
//!
//! Snapshot bytes contain the seeds in clear text; they are handed to the owner as is until
//! S9-27 protects them. Seeds and DRBG state are wiped on drop and never reach `Debug`.
//!
//! Format `ODTPMSTA` version 1 (all integers big-endian):
//!
//! | offset | size | field |
//! |---|---|---|
//! | 0 | 8 | magic `ODTPMSTA` |
//! | 8 | 2 | format version (1) |
//! | 10 | 2 | flags (0; non-zero is rejected) |
//! | 12 | 4 | payload length (113) |
//! | 16 | 113 | payload: platform seed 32, owner seed 32, endorsement seed 32, clock ms u64, resetCount u32, restartCount u32, orderly u8 |
//! | 129 | 32 | SHA-256 of the payload (corruption check, not authentication) |
//!
//! Until slice C of S7-23 completes "Persistent state v1" the layout may still grow; the version
//! stays 1 because no snapshot exists outside tests (recorded in `docs/specs/VTPM.md`).

use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use crate::dispatch;
use crate::hash::Sequences;
use crate::hierarchy::{HierarchyPersistent, HierarchyVolatile};
use crate::marshal::{TpmsClockInfo, TpmsTimeInfo, Writer};
use crate::nv::NvStore;
use crate::object::{Objects, PersistentObjects};
use crate::pcr::{Pcrs, SavedPcrs};
use crate::random::{Drbg, MIN_ENTROPY_LEN};
use crate::response::{Output, Rc};
use crate::session::Sessions;

const MAGIC: &[u8; 8] = b"ODTPMSTA";
/// Snapshot format version written and accepted.
pub const SNAPSHOT_VERSION: u16 = 1;
const PAYLOAD_LEN: usize = 32 * 3 + 8 + 4 + 4 + 1;
const SNAPSHOT_LEN: usize = 16 + PAYLOAD_LEN + 32;

/// Why creating or restoring a TPM failed. No variant carries state content.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StateError {
    /// Fewer than 48 octets of owner entropy ([`MIN_ENTROPY_LEN`]).
    InsufficientEntropy,
    /// Snapshot shorter or longer than the format requires.
    BadLength,
    /// Magic mismatch.
    BadMagic,
    /// Format version other than [`SNAPSHOT_VERSION`] (the version found is returned).
    UnsupportedVersion(u16),
    /// Non-zero flags, bad payload length or an invalid field value.
    BadField,
    /// Payload digest mismatch.
    BadDigest,
}

impl core::fmt::Display for StateError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            StateError::InsufficientEntropy => {
                write!(f, "TPM needs at least {MIN_ENTROPY_LEN} octets of entropy")
            }
            StateError::BadLength => f.write_str("TPM snapshot has the wrong length"),
            StateError::BadMagic => f.write_str("TPM snapshot magic mismatch"),
            StateError::UnsupportedVersion(v) => {
                write!(f, "TPM snapshot version {v} is not supported")
            }
            StateError::BadField => f.write_str("TPM snapshot field is invalid"),
            StateError::BadDigest => f.write_str("TPM snapshot digest mismatch"),
        }
    }
}

impl std::error::Error for StateError {}

pub(crate) struct Persistent {
    pub(crate) platform_seed: Zeroizing<[u8; 32]>,
    pub(crate) owner_seed: Zeroizing<[u8; 32]>,
    pub(crate) endorsement_seed: Zeroizing<[u8; 32]>,
    /// `Clock` accumulated over earlier power cycles, milliseconds.
    pub clock_base_ms: u64,
    pub reset_count: u32,
    pub restart_count: u32,
    /// Set by `Shutdown`, cleared by `Startup`; a snapshot taken without `Shutdown` is disorderly.
    pub orderly: bool,
    /// Authorization values and flags of the hierarchies (not in the snapshot yet).
    pub(crate) hier: HierarchyPersistent,
    /// NV indices (not in the snapshot yet).
    pub(crate) nv: NvStore,
    /// Persistent objects made by `TPM2_EvictControl` (not in the snapshot yet).
    pub(crate) objects: PersistentObjects,
}

/// What `Shutdown(STATE)` saves for `Startup(CLEAR)` (Restart) and `Startup(STATE)` (Resume):
/// the PCR with `TPM_PT_PCR_SAVE` and the volatile hierarchy block.
pub(crate) struct SavedState {
    pub pcrs: SavedPcrs,
    pub hier: HierarchyVolatile,
}

/// The vTPM command processor. Created with owner entropy; commands go through
/// [`Tpm::execute`].
pub struct Tpm {
    pub(crate) persistent: Persistent,
    pub(crate) drbg: Drbg,
    /// `TPM2_Startup` completed since `_TPM_Init`.
    pub(crate) started: bool,
    /// Failure mode (self-test failed).
    pub(crate) failed: bool,
    /// State of `Shutdown(STATE)`; `None` after `Shutdown(CLEAR)`, after a later PCR change or
    /// without any shutdown.
    pub(crate) saved: Option<SavedState>,
    pub(crate) pcrs: Pcrs,
    pub(crate) sequences: Sequences,
    /// Loaded HMAC authorization sessions.
    pub(crate) sessions: Sessions,
    /// Loaded transient objects (not sequences).
    pub(crate) objects: Objects,
    /// Volatile hierarchy state: `platformAuth`, enables, Null seed.
    pub(crate) hv: HierarchyVolatile,
    /// Latest `now_ms` the owner passed: milliseconds since `_TPM_Init` (power-on).
    pub(crate) power_ms: u64,
    /// `TPMS_CLOCK_INFO.safe` for the current power cycle.
    pub(crate) clock_safe: bool,
    /// `power_ms` at the moment `TPM2_Clear` zeroed `Clock`; `Clock` counts from there.
    pub(crate) clock_epoch_ms: u64,
}

impl core::fmt::Debug for Tpm {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Tpm")
            .field("started", &self.started)
            .field("failed", &self.failed)
            .field("reset_count", &self.persistent.reset_count)
            .field("restart_count", &self.persistent.restart_count)
            .finish_non_exhaustive()
    }
}

impl Tpm {
    /// Creates a fresh TPM. `entropy` (at least 48 octets of host entropy) seeds the DRBG, which
    /// in turn generates the hierarchy seeds. The TPM is in the state after `_TPM_Init`: the
    /// first command must be `TPM2_Startup`.
    pub fn new(entropy: &[u8]) -> Result<Tpm, StateError> {
        let mut drbg = Drbg::new(entropy).ok_or(StateError::InsufficientEntropy)?;
        let mut seed = || {
            drbg.generate_array()
                .map(Zeroizing::new)
                .ok_or(StateError::BadField)
        };
        let persistent = Persistent {
            platform_seed: seed()?,
            owner_seed: seed()?,
            endorsement_seed: seed()?,
            clock_base_ms: 0,
            reset_count: 0,
            restart_count: 0,
            orderly: true,
            hier: HierarchyPersistent::default(),
            nv: NvStore::default(),
            objects: PersistentObjects::default(),
        };
        Ok(Tpm::assemble(persistent, drbg))
    }

    fn assemble(persistent: Persistent, drbg: Drbg) -> Tpm {
        Tpm {
            persistent,
            drbg,
            started: false,
            failed: false,
            saved: None,
            pcrs: Pcrs::new(),
            sequences: Sequences::default(),
            sessions: Sessions::default(),
            objects: Objects::default(),
            hv: HierarchyVolatile::default(),
            power_ms: 0,
            clock_safe: false,
            clock_epoch_ms: 0,
        }
    }

    /// `Clock` in milliseconds: the base of earlier power cycles plus this cycle since the last
    /// zeroing by `TPM2_Clear`.
    pub(crate) fn clock_ms(&self) -> u64 {
        self.persistent
            .clock_base_ms
            .saturating_add(self.power_ms.saturating_sub(self.clock_epoch_ms))
    }

    /// `_TPM_Init`: models a power cycle of the same TPM object. Volatile state is lost; the
    /// persistent state and the state saved by `Shutdown(STATE)` survive; `Clock` keeps the time
    /// that passed while powered. The next command must be `TPM2_Startup`.
    pub fn init(&mut self) {
        self.persistent.clock_base_ms = self.clock_ms();
        self.power_ms = 0;
        self.clock_epoch_ms = 0;
        self.started = false;
        self.failed = false;
        self.pcrs = Pcrs::new();
        self.sequences.clear();
        self.sessions.clear();
        self.objects.clear();
        // `phEnable` is SET on any `_TPM_Init`; the rest of the platform controls is volatile.
        self.hv = HierarchyVolatile::default();
    }

    /// Runs one command: `command` is the complete command buffer, `locality` the locality of the
    /// caller (only 0 is served, ADR-0009), `now_ms` the milliseconds since `_TPM_Init` as seen
    /// by the owner (non-decreasing; a smaller value is ignored). Returns the complete response.
    /// Every error of the guest command is a response with a `TPM_RC`, never a host error.
    pub fn execute(&mut self, locality: u8, command: &[u8], now_ms: u64) -> Vec<u8> {
        self.power_ms = self.power_ms.max(now_ms);
        dispatch::execute(self, locality, command)
    }

    /// Bytes of the persistent state (format `ODTPMSTA` v1). They contain the seeds in clear text.
    pub fn persistent_snapshot(&self) -> Vec<u8> {
        let mut payload: Vec<u8> = Vec::with_capacity(PAYLOAD_LEN);
        payload.extend_from_slice(&self.persistent.platform_seed[..]);
        payload.extend_from_slice(&self.persistent.owner_seed[..]);
        payload.extend_from_slice(&self.persistent.endorsement_seed[..]);
        payload.extend_from_slice(&self.clock_ms().to_be_bytes());
        payload.extend_from_slice(&self.persistent.reset_count.to_be_bytes());
        payload.extend_from_slice(&self.persistent.restart_count.to_be_bytes());
        payload.push(u8::from(self.persistent.orderly));
        let mut out = Vec::with_capacity(SNAPSHOT_LEN);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&SNAPSHOT_VERSION.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&(PAYLOAD_LEN as u32).to_be_bytes());
        out.extend_from_slice(&payload);
        out.extend_from_slice(&Sha256::digest(&payload[..]));
        payload.as_mut_slice().zeroize();
        out
    }

    /// Restores a TPM from [`Tpm::persistent_snapshot`] bytes. A different version, a damaged
    /// snapshot or too little entropy is an error: there is no silent fallback to an empty TPM.
    /// `entropy` reseeds the DRBG (the DRBG state is not part of the snapshot). The TPM is in
    /// the state after `_TPM_Init`.
    pub fn restore(snapshot: &[u8], entropy: &[u8]) -> Result<Tpm, StateError> {
        if snapshot.len() < 12 {
            return Err(StateError::BadLength);
        }
        if &snapshot[..8] != MAGIC {
            return Err(StateError::BadMagic);
        }
        let version = u16::from_be_bytes([snapshot[8], snapshot[9]]);
        if version != SNAPSHOT_VERSION {
            return Err(StateError::UnsupportedVersion(version));
        }
        if snapshot.len() != SNAPSHOT_LEN {
            return Err(StateError::BadLength);
        }
        let flags = u16::from_be_bytes([snapshot[10], snapshot[11]]);
        let len = u32::from_be_bytes([snapshot[12], snapshot[13], snapshot[14], snapshot[15]]);
        if flags != 0 || len as usize != PAYLOAD_LEN {
            return Err(StateError::BadField);
        }
        let payload = &snapshot[16..16 + PAYLOAD_LEN];
        if Sha256::digest(payload)[..] != snapshot[16 + PAYLOAD_LEN..] {
            return Err(StateError::BadDigest);
        }
        let seed = |at: usize| {
            let mut s = Zeroizing::new([0u8; 32]);
            s.copy_from_slice(&payload[at..at + 32]);
            s
        };
        let orderly = match payload[PAYLOAD_LEN - 1] {
            0 => false,
            1 => true,
            _ => return Err(StateError::BadField),
        };
        let u64_at = |at: usize| {
            let mut b = [0u8; 8];
            b.copy_from_slice(&payload[at..at + 8]);
            u64::from_be_bytes(b)
        };
        let u32_at = |at: usize| {
            let mut b = [0u8; 4];
            b.copy_from_slice(&payload[at..at + 4]);
            u32::from_be_bytes(b)
        };
        let drbg = Drbg::new(entropy).ok_or(StateError::InsufficientEntropy)?;
        let persistent = Persistent {
            platform_seed: seed(0),
            owner_seed: seed(32),
            endorsement_seed: seed(64),
            clock_base_ms: u64_at(96),
            reset_count: u32_at(104),
            restart_count: u32_at(108),
            orderly,
            hier: HierarchyPersistent::default(),
            nv: NvStore::default(),
            objects: PersistentObjects::default(),
        };
        Ok(Tpm::assemble(persistent, drbg))
    }

    /// `Clock` and `Time` for `TPMS_TIME_INFO`.
    fn time_info(&self) -> TpmsTimeInfo {
        TpmsTimeInfo {
            time: self.power_ms,
            clock_info: TpmsClockInfo {
                clock: self.clock_ms(),
                reset_count: self.persistent.reset_count,
                restart_count: self.persistent.restart_count,
                safe: self.clock_safe,
            },
        }
    }

    /// `TPM2_ReadClock` (Part 3, clause 29.1): `TPMS_TIME_INFO`.
    pub(crate) fn cmd_read_clock(
        &mut self,
        p: &mut crate::marshal::Reader<'_>,
    ) -> Result<Output, Rc> {
        p.finish()?;
        let mut w = Writer::new();
        self.time_info().marshal(&mut w);
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tpm() -> Tpm {
        Tpm::new(&[7u8; 48]).unwrap()
    }

    #[test]
    fn new_requires_entropy() {
        assert_eq!(
            Tpm::new(&[0u8; 47]).unwrap_err(),
            StateError::InsufficientEntropy
        );
        assert!(Tpm::new(&[0u8; 48]).is_ok());
    }

    #[test]
    fn snapshot_round_trip() {
        let mut t = tpm();
        t.power_ms = 1500;
        t.persistent.reset_count = 4;
        t.persistent.restart_count = 2;
        t.persistent.orderly = false;
        let bytes = t.persistent_snapshot();
        assert_eq!(bytes.len(), SNAPSHOT_LEN);
        assert_eq!(&bytes[..8], b"ODTPMSTA");
        let r = Tpm::restore(&bytes, &[9u8; 48]).unwrap();
        assert_eq!(r.persistent.clock_base_ms, 1500);
        assert_eq!(r.persistent.reset_count, 4);
        assert_eq!(r.persistent.restart_count, 2);
        assert!(!r.persistent.orderly);
        assert_eq!(&r.persistent.owner_seed[..], &t.persistent.owner_seed[..]);
        assert_eq!(
            &r.persistent.platform_seed[..],
            &t.persistent.platform_seed[..]
        );
        assert_eq!(
            &r.persistent.endorsement_seed[..],
            &t.persistent.endorsement_seed[..]
        );
        assert!(!r.started);
        // Restored TPM yields the same snapshot (clock already folded into the base).
        assert_eq!(r.persistent_snapshot(), bytes);
    }

    #[test]
    fn restore_rejects_damage() {
        let bytes = tpm().persistent_snapshot();
        let e = [9u8; 48];
        assert_eq!(Tpm::restore(&[], &e).unwrap_err(), StateError::BadLength);
        assert_eq!(
            Tpm::restore(&bytes[..bytes.len() - 1], &e).unwrap_err(),
            StateError::BadLength
        );
        let mut b = bytes.clone();
        b[0] ^= 1;
        assert_eq!(Tpm::restore(&b, &e).unwrap_err(), StateError::BadMagic);
        let mut b = bytes.clone();
        b[9] = 2;
        assert_eq!(
            Tpm::restore(&b, &e).unwrap_err(),
            StateError::UnsupportedVersion(2)
        );
        let mut b = bytes.clone();
        b[11] = 1;
        assert_eq!(Tpm::restore(&b, &e).unwrap_err(), StateError::BadField);
        let mut b = bytes.clone();
        b[20] ^= 1;
        assert_eq!(Tpm::restore(&b, &e).unwrap_err(), StateError::BadDigest);
        assert_eq!(
            Tpm::restore(&bytes, &[0u8; 10]).unwrap_err(),
            StateError::InsufficientEntropy
        );
    }

    #[test]
    fn restore_rejects_invalid_orderly_flag_even_with_valid_digest() {
        let mut b = tpm().persistent_snapshot();
        let last = 16 + PAYLOAD_LEN - 1;
        b[last] = 2;
        let digest = Sha256::digest(&b[16..16 + PAYLOAD_LEN]);
        b[16 + PAYLOAD_LEN..].copy_from_slice(&digest);
        assert_eq!(
            Tpm::restore(&b, &[9u8; 48]).unwrap_err(),
            StateError::BadField
        );
    }

    #[test]
    fn debug_hides_state() {
        let s = format!("{:?}", tpm());
        assert!(s.contains("Tpm") && !s.contains("seed") && !s.contains("drbg"));
    }

    #[test]
    fn different_entropy_gives_different_seeds() {
        let a = Tpm::new(&[1u8; 48]).unwrap();
        let b = Tpm::new(&[2u8; 48]).unwrap();
        assert_ne!(&a.persistent.owner_seed[..], &b.persistent.owner_seed[..]);
        assert_ne!(
            &a.persistent.owner_seed[..],
            &a.persistent.platform_seed[..]
        );
    }
}
