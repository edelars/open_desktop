//! TPM state: persistent part (survives power cycles, goes into the snapshot) and volatile part
//! (reset by `_TPM_Init`), the snapshot codec, and TPM clock commands.
//!
//! Persistent (ADR-0009, decision 5): the three hierarchy seeds (generated from owner entropy), the
//! clock base, `resetCount`, `totalResetCount`, `restartCount` and the orderly-shutdown flag, the authorization values
//! and flags of the hierarchies (`hierarchy.rs`), the dictionary-attack state (`da.rs`), the NV store
//! (`nv.rs`) and the persistent objects with their private parts (`object.rs`) go into the
//! snapshot (slice C1 of phase B; the codec is `snapshot.rs`). The transient objects are volatile
//! (flushed by `Startup`/`_TPM_Init`). The hierarchy proof values are not stored: they are derived
//! from the seeds (`object_cmd.rs`).
//! Volatile: PCR, sequences, sessions, saved-context state, objects, hierarchy enables and `platformAuth`,
//! `started`/`failed`, the state saved by `Shutdown(STATE)`, the DRBG, the DA timers.
//!
//! **The snapshot is sensitive data.** Its bytes contain the seeds, the authorization values, the
//! NV data and the private keys in clear text; they are handed to the owner as is until S9-27
//! protects them. They must not be logged or stored unprotected. Seeds and DRBG state are wiped on
//! drop and never reach `Debug`.
//!
//! Format `ODTPMSTA` version 8 (all integers big-endian):
//!
//! | offset | size | field |
//! |---|---|---|
//! | 0 | 8 | magic `ODTPMSTA` |
//! | 8 | 2 | format version (8) |
//! | 10 | 2 | flags (0; non-zero is rejected) |
//! | 12 | 4 | payload length `L` (at most 1 MiB) |
//! | 16 | `L` | payload (layout in `snapshot.rs`) |
//! | 16 + `L` | 32 | SHA-256 of the payload (corruption check, not authentication) |
//!
//! Version 1 (seeds and counters only, 113 octets of payload) never left the tests; it is rejected
//! as `UnsupportedVersion(1)` rather than restored into a TPM with silently empty authorization
//! values and NV (recorded in `docs/specs/VTPM.md`). Version 2 (slice C1, ordinary NV indices
//! only, no counter maximum) is rejected as `UnsupportedVersion(2)`: slice C3 added NV counters,
//! bit fields and extend indices and the persistent counter maximum, and a version-2 snapshot
//! cannot be read as version 3 without inventing the maximum.
//! Version 3 is accepted with all hierarchy policies empty. Versions 3 and 4 initialize
//! `totalResetCount` from the saved `resetCount`, its available lower bound.
//! Versions 3 through 5 use the default allocation of both PCR banks.
//! Versions 3 through 6 restore with no established dynamic RTM.
//! Versions 3 through 7 extend the original primary seeds using fresh host entropy.

use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::context::ContextState;
use crate::da::{DaPersistent, DaVolatile};
use crate::dispatch;
use crate::hash::Sequences;
use crate::hierarchy::{
    HierarchyPersistent, HierarchyVolatile, LEGACY_SEED_BYTES, PRIMARY_SEED_BYTES, PrimarySeed,
};
use crate::marshal::{TpmlDigestValues, TpmsClockInfo, TpmsTimeInfo, Writer};
use crate::nv::NvStore;
use crate::object::{Objects, PersistentObjects};
use crate::pcr::{PcrAllocation, Pcrs, SavedPcrs};
use crate::random::{Drbg, MIN_ENTROPY_LEN};
use crate::response::{Output, Rc};
use crate::session::Sessions;
use crate::snapshot;

const MAGIC: &[u8; 8] = b"ODTPMSTA";
/// Snapshot format version written by the TPM.
pub const SNAPSHOT_VERSION: u16 = 8;
const MIN_SNAPSHOT_VERSION: u16 = 3;
/// Header size: magic, version, flags, payload length.
const HEADER_LEN: usize = 16;
/// The largest payload `restore` accepts (the real maximum is about 60 KiB: 128 NV indices with
/// 16 KiB of data and 16 persistent objects).
const MAX_PAYLOAD: usize = 1 << 20;

/// Why creating or restoring a TPM failed. No variant carries state content.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StateError {
    /// Fewer than 48 octets of owner entropy ([`MIN_ENTROPY_LEN`]).
    InsufficientEntropy,
    /// Snapshot shorter or longer than the format requires.
    BadLength,
    /// Magic mismatch.
    BadMagic,
    /// An unrecognized format version (the version found is returned).
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
    pub(crate) platform_seed: PrimarySeed,
    pub(crate) owner_seed: PrimarySeed,
    pub(crate) endorsement_seed: PrimarySeed,
    /// `Clock` accumulated over earlier power cycles, milliseconds.
    pub clock_base_ms: u64,
    pub reset_count: u32,
    pub restart_count: u32,
    /// Persistent TPM Reset count. Neither Restart, Resume nor Clear change this value.
    pub total_reset_count: u64,
    /// Allocation to activate at the next `_TPM_Init`.
    pub(crate) pcr_allocation: PcrAllocation,
    /// Whether a dynamic RTM has been established since the last privileged reset of this bit.
    pub(crate) established: bool,
    /// Set by `Shutdown`, cleared by `Startup`; a snapshot taken without `Shutdown` is disorderly.
    pub orderly: bool,
    /// Authorization values and flags of the hierarchies.
    pub(crate) hier: HierarchyPersistent,
    /// Dictionary-attack parameters and counters.
    pub(crate) da: DaPersistent,
    /// NV indices.
    pub(crate) nv: NvStore,
    /// Persistent objects made by `TPM2_EvictControl`.
    pub(crate) objects: PersistentObjects,
}

/// What `Shutdown(STATE)` saves for `Startup(CLEAR)` (Restart) and `Startup(STATE)` (Resume):
/// the PCR with `TPM_PT_PCR_SAVE` and the volatile hierarchy block.
pub(crate) struct SavedState {
    pub pcrs: SavedPcrs,
    pub hier: HierarchyVolatile,
    /// The context protection state: saved contexts stay loadable across Restart and Resume.
    pub ctx: ContextState,
    pub startup_hcrtm: bool,
}

/// The vTPM command processor. Created with owner entropy; commands go through
/// [`Tpm::execute`].
pub struct Tpm {
    pub(crate) persistent: Persistent,
    pub(crate) drbg: Drbg,
    pub(crate) seed_extension_drbg: Drbg,
    /// `TPM2_Startup` completed since `_TPM_Init`.
    pub(crate) started: bool,
    /// Failure mode (self-test failed).
    pub(crate) failed: bool,
    /// State of `Shutdown(STATE)`; `None` after `Shutdown(CLEAR)`, after a later PCR change or
    /// without any shutdown.
    pub(crate) saved: Option<SavedState>,
    pub(crate) pcrs: Pcrs,
    /// A successful PCR_Allocate forbids Shutdown(STATE) until init, even for a no-op.
    pub(crate) pcr_allocation_pending: bool,
    /// Completed S-HCRTM measurement, staged until Startup so initial PCR setup preserves it.
    pub(crate) pre_startup_hcrtm: Option<TpmlDigestValues>,
    pub(crate) startup_hcrtm: bool,
    pub(crate) sequences: Sequences,
    /// Loaded authorization sessions (HMAC, policy, trial).
    pub(crate) sessions: Sessions,
    /// Context protection (`context.rs`): saved sessions and the counters of saved contexts.
    pub(crate) context: ContextState,
    /// Loaded transient objects (not sequences).
    pub(crate) objects: Objects,
    /// Volatile hierarchy state: `platformAuth`, enables, Null seed.
    pub(crate) hv: HierarchyVolatile,
    /// TPM `Time`: calibrated elapsed milliseconds since `_TPM_Init` (power-on).
    pub(crate) power_ms: u64,
    time_calibration: TimeCalibration,
    /// Fresh nonce for each uninterrupted measurement of `Time` (Part 1, §16.7.12).
    pub(crate) time_epoch: [u8; 16],
    /// `TPMS_CLOCK_INFO.safe` for the current power cycle.
    pub(crate) clock_safe: bool,
    /// `power_ms` when `Clock` last took its current persistent base.
    pub(crate) clock_epoch_ms: u64,
    /// Volatile dictionary-attack timers (`da.rs`).
    pub(crate) da_v: DaVolatile,
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

fn generate_primary_seed(drbg: &mut Drbg, extension: &mut Drbg) -> Option<PrimarySeed> {
    let mut seed = Zeroizing::new([0; PRIMARY_SEED_BYTES]);
    drbg.generate(&mut seed[..LEGACY_SEED_BYTES])?;
    extension.generate(&mut seed[LEGACY_SEED_BYTES..])?;
    Some(seed)
}

impl Tpm {
    /// Creates a fresh TPM. `entropy` (at least 48 octets of host entropy) seeds the DRBG, which
    /// in turn generates the hierarchy seeds. The TPM is in the state after `_TPM_Init`: the
    /// first command must be `TPM2_Startup`.
    pub fn new(entropy: &[u8]) -> Result<Tpm, StateError> {
        let mut drbg = Drbg::new(entropy).ok_or(StateError::InsufficientEntropy)?;
        let mut extension =
            Drbg::for_seed_extensions(entropy).ok_or(StateError::InsufficientEntropy)?;
        let mut seed =
            || generate_primary_seed(&mut drbg, &mut extension).ok_or(StateError::BadField);
        let persistent = Persistent {
            platform_seed: seed()?,
            owner_seed: seed()?,
            endorsement_seed: seed()?,
            clock_base_ms: 0,
            reset_count: 0,
            restart_count: 0,
            total_reset_count: 0,
            pcr_allocation: PcrAllocation::default(),
            established: false,
            orderly: true,
            hier: HierarchyPersistent::default(),
            da: DaPersistent::default(),
            nv: NvStore::default(),
            objects: PersistentObjects::default(),
        };
        Ok(Tpm::assemble(persistent, drbg, extension))
    }

    fn assemble(persistent: Persistent, drbg: Drbg, seed_extension_drbg: Drbg) -> Tpm {
        let pcrs = Pcrs::with_allocation(persistent.pcr_allocation);
        Tpm {
            persistent,
            drbg,
            seed_extension_drbg,
            started: false,
            failed: false,
            saved: None,
            pcrs,
            pcr_allocation_pending: false,
            pre_startup_hcrtm: None,
            startup_hcrtm: false,
            sequences: Sequences::default(),
            sessions: Sessions::default(),
            context: ContextState::default(),
            objects: Objects::default(),
            hv: HierarchyVolatile::default(),
            power_ms: 0,
            time_calibration: TimeCalibration::default(),
            time_epoch: [0; 16],
            clock_safe: false,
            clock_epoch_ms: 0,
            da_v: DaVolatile::default(),
        }
    }

    pub(crate) fn fresh_primary_seed(&mut self) -> Result<PrimarySeed, Rc> {
        generate_primary_seed(&mut self.drbg, &mut self.seed_extension_drbg).ok_or(Rc::FAILURE)
    }

    /// `Clock` in milliseconds: the persistent base plus elapsed power-on time since it was set.
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
        self.time_calibration = TimeCalibration::default();
        self.clock_epoch_ms = 0;
        self.started = false;
        self.failed = false;
        self.pcrs = Pcrs::with_allocation(self.persistent.pcr_allocation);
        self.pcr_allocation_pending = false;
        self.pre_startup_hcrtm = None;
        self.startup_hcrtm = false;
        self.sequences.clear();
        self.sessions.clear();
        self.context = ContextState::default();
        self.objects.clear();
        // `phEnable` is SET on any `_TPM_Init`; the rest of the platform controls is volatile.
        self.hv = HierarchyVolatile::default();
        self.da_v = DaVolatile::default();
    }

    /// Runs one command. `command` is the complete command buffer, `locality` the locality of the
    /// caller (only 0 is served, ADR-0009), and `now_ms` the owner's uncalibrated milliseconds since
    /// `_TPM_Init` (non-decreasing; a smaller value is ignored). Returns the complete response.
    /// Every error of the guest command is a response with a `TPM_RC`, never a host error.
    pub fn execute(&mut self, locality: u8, command: &[u8], now_ms: u64) -> Vec<u8> {
        drop(self.sequences.take_hcrtm());
        self.advance_power_time(now_ms);
        dispatch::execute(self, locality, command)
    }

    /// `_TPM_Hash_Start`, delivered by the owning platform (Part 3 v185 §22.9).
    pub fn platform_hash_start(&mut self) {
        if !self.failed {
            self.sequences.start_hcrtm();
            self.sessions.exclusive_audit = None;
        }
    }

    /// `_TPM_Hash_Data`, delivered by the owning platform (Part 3 v185 §22.10).
    pub fn platform_hash_data(&mut self, data: &[u8]) {
        self.sequences.update_hcrtm(data);
    }

    /// `_TPM_Hash_End`, delivered by the owning platform (Part 3 v185 §22.11).
    pub fn platform_hash_end(&mut self, now_ms: u64) {
        let Some(hash) = self.sequences.take_hcrtm() else {
            return;
        };
        self.advance_power_time(now_ms);
        let digests = hash.finish();
        if self.started {
            self.persistent.established = true;
            self.persistent.restart_count = self.persistent.restart_count.wrapping_add(1);
            self.pcrs.measure_dynamic_hcrtm(&digests);
            self.saved = None;
        } else {
            self.pre_startup_hcrtm = Some(digests);
        }
    }

    pub fn platform_hash_active(&self) -> bool {
        self.sequences.hcrtm_active()
    }

    pub fn platform_established(&self) -> bool {
        self.persistent.established
    }

    pub fn platform_reset_establishment(&mut self) {
        self.persistent.established = false;
    }

    /// A malformed active platform indication terminates the measurement and enters failure mode.
    pub fn platform_hash_failure(&mut self) {
        drop(self.sequences.take_hcrtm());
        self.failed = true;
    }

    pub fn in_failure_mode(&self) -> bool {
        self.failed
    }

    /// Updates the absolute time on this power cycle, including powered idle
    /// time before the owning host takes a persistent snapshot.
    pub fn advance_power_time(&mut self, now_ms: u64) {
        self.power_ms = self
            .power_ms
            .saturating_add(self.time_calibration.elapsed(now_ms));
        self.da_tick();
    }

    /// Bytes of the persistent state (format `ODTPMSTA` v6). **Sensitive data**: they contain the
    /// seeds, the authorization values, the NV data and the private keys in clear text (S9-27
    /// protects them); do not log or store them unprotected. In the unreachable case that a
    /// private part cannot be marshaled the result is empty, which `restore` rejects.
    pub fn persistent_snapshot(&self) -> Vec<u8> {
        let Some(payload) = snapshot::encode(&self.persistent, self.clock_ms()) else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(HEADER_LEN + payload.len() + 32);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&SNAPSHOT_VERSION.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        out.extend_from_slice(&payload);
        out.extend_from_slice(&Sha256::digest(&payload[..]));
        out
    }

    /// Restores a TPM from a version 3 through 8 snapshot. An unsupported version,
    /// a damaged snapshot or too little entropy is an error: there is no silent
    /// fallback to an empty TPM. `entropy` reseeds the DRBG (the DRBG state is not part of the
    /// snapshot). The TPM is in the state after `_TPM_Init`.
    pub fn restore(snapshot: &[u8], entropy: &[u8]) -> Result<Tpm, StateError> {
        if snapshot.len() < 12 {
            return Err(StateError::BadLength);
        }
        if &snapshot[..8] != MAGIC {
            return Err(StateError::BadMagic);
        }
        let version = u16::from_be_bytes([snapshot[8], snapshot[9]]);
        if !(MIN_SNAPSHOT_VERSION..=SNAPSHOT_VERSION).contains(&version) {
            return Err(StateError::UnsupportedVersion(version));
        }
        if snapshot.len() < HEADER_LEN + 32 {
            return Err(StateError::BadLength);
        }
        let flags = u16::from_be_bytes([snapshot[10], snapshot[11]]);
        let len =
            u32::from_be_bytes([snapshot[12], snapshot[13], snapshot[14], snapshot[15]]) as usize;
        if len > MAX_PAYLOAD || snapshot.len() != HEADER_LEN + len + 32 {
            return Err(StateError::BadLength);
        }
        if flags != 0 {
            return Err(StateError::BadField);
        }
        let payload = &snapshot[HEADER_LEN..HEADER_LEN + len];
        if Sha256::digest(payload)[..] != snapshot[HEADER_LEN + len..] {
            return Err(StateError::BadDigest);
        }
        let mut persistent = snapshot::decode(payload, version)?;
        let drbg = Drbg::new(entropy).ok_or(StateError::InsufficientEntropy)?;
        let mut extension =
            Drbg::for_seed_extensions(entropy).ok_or(StateError::InsufficientEntropy)?;
        if version < 8 {
            for seed in [
                &mut persistent.platform_seed,
                &mut persistent.owner_seed,
                &mut persistent.endorsement_seed,
            ] {
                extension
                    .generate(&mut seed[LEGACY_SEED_BYTES..])
                    .ok_or(StateError::BadField)?;
            }
        }
        Ok(Tpm::assemble(persistent, drbg, extension))
    }

    /// `Clock` and `Time` for `TPMS_TIME_INFO`.
    pub(crate) fn time_info(&self) -> TpmsTimeInfo {
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

    pub(crate) fn time_point(&self) -> TimePoint {
        TimePoint {
            epoch: self.time_epoch,
            ms: self.power_ms,
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

    /// `TPM2_ClockSet` (Part 3 v185, clause 29.2) moves the Clock forward
    /// without changing power-on Time or the DA recovery timers.
    pub(crate) fn cmd_clock_set(
        &mut self,
        p: &mut crate::marshal::Reader<'_>,
    ) -> Result<Output, Rc> {
        const MAX_CLOCK_SET: u64 = 0xffff_0000_0000_0000;
        p.set_param(1);
        let new_time = p.u64()?;
        p.finish()?;
        if new_time < self.clock_ms() || new_time > MAX_CLOCK_SET {
            return Err(Rc::VALUE.in_param(1));
        }
        self.persistent.clock_base_ms = new_time;
        self.clock_epoch_ms = self.power_ms;
        Ok(Output::default())
    }

    /// `TPM2_ClockRateAdjust` (Part 3 v185, §29.3) calibrates the rate shared by Clock and Time.
    pub(crate) fn cmd_clock_rate_adjust(
        &mut self,
        p: &mut crate::marshal::Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let step_ppm = match i8::from_be_bytes([p.u8()?]) {
            -3 => -10_000,
            -2 => -1_000,
            -1 => -1,
            0 => 0,
            1 => 1,
            2 => 1_000,
            3 => 10_000,
            _ => return Err(Rc::VALUE.in_param(1)),
        };
        p.finish()?;
        const MAX_ADJUSTMENT_PPM: i32 = 150_000;
        let adjusted = self.time_calibration.adjustment_ppm + step_ppm;
        if !(-MAX_ADJUSTMENT_PPM..=MAX_ADJUSTMENT_PPM).contains(&adjusted) {
            return Err(Rc::VALUE.in_param(1));
        }
        self.time_calibration.adjustment_ppm = adjusted;
        Ok(Output::default())
    }
}

/// Volatile calibration of the owner's unadjusted power clock.
#[derive(Default)]
struct TimeCalibration {
    reference_ms: u64,
    adjustment_ppm: i32,
    fractional: u32,
}

impl TimeCalibration {
    fn elapsed(&mut self, now_ms: u64) -> u64 {
        let delta = now_ms.saturating_sub(self.reference_ms);
        if delta == 0 {
            return 0;
        }
        self.reference_ms = now_ms;
        const PARTS_PER_MILLION: i32 = 1_000_000;
        let scale = (PARTS_PER_MILLION + self.adjustment_ppm) as u128;
        let adjusted = u128::from(delta) * scale + u128::from(self.fractional);
        self.fractional = (adjusted % PARTS_PER_MILLION as u128) as u32;
        u64::try_from(adjusted / PARTS_PER_MILLION as u128).unwrap_or(u64::MAX)
    }
}

/// A session's nonce creation time or a deadline in the TPM `Time` domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TimePoint {
    pub epoch: [u8; 16],
    pub ms: u64,
}

impl TimePoint {
    pub(crate) fn has_passed(self, now: Self) -> bool {
        self.epoch != now.epoch || self.ms < now.ms
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
        t.persistent.total_reset_count = 18;
        t.persistent.restart_count = 2;
        t.persistent.orderly = false;
        let bytes = t.persistent_snapshot();
        // 113 octets of seeds and counters, 3 empty auth values (3), hierarchy flags (1), DA (17),
        // NV count (2), NV counter maximum (8), object count (1) and three NULL primary
        // policies (6), reset count (8), PCR allocation (1), establishment (1) and seed tails (96).
        assert_eq!(bytes.len(), HEADER_LEN + 257 + 32);
        assert_eq!(&bytes[..8], b"ODTPMSTA");
        assert_eq!(u16::from_be_bytes([bytes[8], bytes[9]]), 8);
        let r = Tpm::restore(&bytes, &[9u8; 48]).unwrap();
        assert_eq!(r.persistent.clock_base_ms, 1500);
        assert_eq!(r.persistent.reset_count, 4);
        assert_eq!(r.persistent.total_reset_count, 18);
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
    fn restore_v3_and_v4_without_a_total_reset_count() {
        // Legacy bytes formed independently of the current writer.
        let mut payload = vec![0x42; 96];
        payload.extend_from_slice(&1234u64.to_be_bytes());
        payload.extend_from_slice(&4u32.to_be_bytes());
        payload.extend_from_slice(&5u32.to_be_bytes());
        payload.extend_from_slice(&[1, 3, b'o', b'l', b'd', 0, 0, 1]);
        for word in [2u32, 32, 600, 86_400] {
            payload.extend_from_slice(&word.to_be_bytes());
        }
        payload.push(1); // Lockout auth enabled.
        payload.extend_from_slice(&[0; 11]); // NV count, counter maximum and object count.
        let wrap = |version: u16, body: &[u8]| {
            let mut bytes = b"ODTPMSTA".to_vec();
            bytes.extend_from_slice(&version.to_be_bytes());
            bytes.extend_from_slice(&[0; 2]);
            bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
            bytes.extend_from_slice(body);
            bytes.extend_from_slice(&Sha256::digest(body));
            bytes
        };
        let legacy = wrap(3, &payload);
        let t = Tpm::restore(&legacy, &[9; 48]).unwrap();
        assert_eq!(t.persistent.clock_base_ms, 1234);
        assert_eq!(t.persistent.hier.owner_auth.as_slice(), b"old");
        assert_eq!(t.persistent.da.failed_tries, 2);
        assert_eq!(t.persistent.total_reset_count, 4);
        assert!(t.persistent.hier.owner_auth_set);
        for handle in [
            crate::codes::handle::RH_OWNER,
            crate::codes::handle::RH_ENDORSEMENT,
            crate::codes::handle::RH_LOCKOUT,
            crate::codes::handle::RH_PLATFORM,
        ] {
            assert_eq!(t.permanent_auth_policy(handle), None);
        }
        let v8 = t.persistent_snapshot();
        let legacy_end = v8.len() - 32 - 96;
        assert_eq!(&v8[8..10], &8u16.to_be_bytes());
        assert_eq!(&v8[16..16 + payload.len()], &payload);
        assert_eq!(
            &v8[16 + payload.len()..legacy_end],
            &[0, 0x10, 0, 0x10, 0, 0x10, 0, 0, 0, 0, 0, 0, 0, 4, 3, 0]
        );
        let v6 = wrap(6, &v8[16..legacy_end - 1]);
        assert!(!Tpm::restore(&v6, &[9; 48]).unwrap().platform_established());

        let old_length = payload.len();
        payload.extend_from_slice(&[0, 0x0b, 0x22]);
        assert_eq!(
            Tpm::restore(&wrap(4, &payload), &[9; 48]).unwrap_err(),
            StateError::BadLength
        );
        payload.truncate(old_length);
        payload.extend_from_slice(&[0, 0x01, 0, 0x10, 0, 0x10]);
        assert_eq!(
            Tpm::restore(&wrap(4, &payload), &[9; 48]).unwrap_err(),
            StateError::BadField
        );
        payload.truncate(old_length);
        payload.extend_from_slice(&[0, 0x0b]);
        payload.extend_from_slice(&[0x55; 32]);
        payload.extend_from_slice(&[0, 0x10, 0, 0x10]);
        let restored = Tpm::restore(&wrap(4, &payload), &[9; 48]).unwrap();
        assert_eq!(restored.persistent.total_reset_count, 4);
        assert_eq!(
            restored.permanent_auth_policy(crate::codes::handle::RH_OWNER),
            Some(&[0x55; 32][..])
        );
        assert_eq!(
            Tpm::restore(&wrap(5, &payload), &[9; 48]).unwrap_err(),
            StateError::BadLength
        );
        payload.extend_from_slice(&3u64.to_be_bytes());
        assert_eq!(
            Tpm::restore(&wrap(5, &payload), &[9; 48]).unwrap_err(),
            StateError::BadField
        );

        payload.truncate(payload.len() - 8);
        payload.extend_from_slice(&17u64.to_be_bytes());
        let restored = Tpm::restore(&wrap(5, &payload), &[9; 48]).unwrap();
        assert_eq!(restored.persistent.total_reset_count, 17);
        assert_eq!(restored.persistent.pcr_allocation, PcrAllocation::default());
        assert_eq!(
            Tpm::restore(&wrap(6, &payload), &[9; 48]).unwrap_err(),
            StateError::BadLength
        );
        payload.push(0);
        assert_eq!(
            Tpm::restore(&wrap(6, &payload), &[9; 48]).unwrap_err(),
            StateError::BadField
        );
        *payload.last_mut().unwrap() = 4;
        assert_eq!(
            Tpm::restore(&wrap(6, &payload), &[9; 48]).unwrap_err(),
            StateError::BadField
        );
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
        // The former version 1 and a future version are refused, whatever the rest looks like.
        for v in [1, 2, SNAPSHOT_VERSION + 1] {
            let mut b = bytes.clone();
            b[8..10].copy_from_slice(&v.to_be_bytes());
            assert_eq!(
                Tpm::restore(&b, &e).unwrap_err(),
                StateError::UnsupportedVersion(v)
            );
        }
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
        let payload_len = b.len() - HEADER_LEN - 32;
        // The orderly flag follows the seeds, `Clock`, `resetCount` and `restartCount`.
        b[HEADER_LEN + 32 * 3 + 8 + 4 + 4] = 2;
        let digest = Sha256::digest(&b[HEADER_LEN..HEADER_LEN + payload_len]);
        b[HEADER_LEN + payload_len..].copy_from_slice(&digest);
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
